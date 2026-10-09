//! tongyun-dev — local project start/stop/status/logs
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
 
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "tongyun-dev", about = "Local project runner (start/stop/status/logs)")]
struct Cli {
    /// Config file (default: ./config/projects.toml or $TONGYUN_DEV_CONFIG)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List registered projects
    List {
        #[arg(long)]
        json: bool,
    },
    /// Start a project
    Start { id: String },
    /// Stop a project
    Stop { id: String },
    /// Show status (process + port + login probe)
    Status {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Tail logs
    Logs {
        id: String,
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
}

#[derive(Debug, Deserialize)]
struct FileConfig {
    projects: HashMap<String, Project>,
}

#[derive(Debug, Clone, Deserialize)]
struct Project {
    name: String,
    root: String,
    start: Vec<String>,
    stop: Vec<String>,
    tomcat: String,
    log_dir: String,
    log_file: String,
    pid_file: String,
    port: u16,
    login_url: String,
}

#[derive(Debug, Serialize)]
struct StatusJson {
    id: String,
    name: String,
    running: bool,
    pid: Option<u32>,
    port: u16,
    port_open: bool,
    login_probe: LoginProbe,
    log_path: String,
    pid_path: String,
    root: String,
}

#[derive(Debug, Serialize)]
struct LoginProbe {
    url: String,
    http_code: Option<u16>,
    ok: bool,
}

fn expand_user(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(rest)
    } else if p == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    } else {
        PathBuf::from(p)
    }
}

fn render(s: &str, proj: &Project) -> String {
    let root = expand_user(&proj.root).display().to_string();
    let tomcat = expand_user(&proj.tomcat).display().to_string();
    let log_dir = expand_user(&proj.log_dir).display().to_string();
    s.replace("{{root}}", &root)
        .replace("{{tomcat}}", &tomcat)
        .replace("{{log_dir}}", &log_dir)
}

fn load_config(cli: &Cli) -> Result<(PathBuf, FileConfig)> {
    let path = if let Some(p) = &cli.config {
        p.clone()
    } else if let Ok(p) = std::env::var("TONGYUN_DEV_CONFIG") {
        PathBuf::from(p)
    } else {
        // walk up from cwd for config/projects.toml
        let mut cur = std::env::current_dir()?;
        loop {
            let cand = cur.join("config/projects.toml");
            if cand.is_file() {
                break cand;
            }
            if !cur.pop() {
                bail!("config/projects.toml not found; pass --config or set TONGYUN_DEV_CONFIG");
            }
        }
    };
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg: FileConfig = toml::from_str(&text).context("parse projects.toml")?;
    Ok((path, cfg))
}

fn get_proj<'a>(cfg: &'a FileConfig, id: &str) -> Result<&'a Project> {
    cfg.projects
        .get(id)
        .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))
}

fn read_pid(path: &Path) -> Option<u32> {
    let s = fs::read_to_string(path).ok()?;
    s.trim().parse().ok()
}

fn pid_alive(pid: u32) -> bool {
    // Unix: kill -0
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(400),
    )
    .is_ok()
}

fn login_probe(url: &str) -> LoginProbe {
    // Prefer curl for HTTPS/redirect simplicity; only need status code.
    let out = Command::new("curl")
        .args([
            "-sS",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--connect-timeout",
            "2",
            "--max-time",
            "5",
            url,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() || o.status.code().is_some() => {
            let code_s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let code: Option<u16> = code_s.parse().ok();
            let ok = matches!(code, Some(200) | Some(302) | Some(301));
            LoginProbe {
                url: url.to_string(),
                http_code: code,
                ok,
            }
        }
        _ => LoginProbe {
            url: url.to_string(),
            http_code: None,
            ok: false,
        },
    }
}

fn status_of(id: &str, p: &Project) -> StatusJson {
    let pid_path = expand_user(&render(&p.pid_file, p));
    let log_path = expand_user(&render(&p.log_file, p));
    let pid = read_pid(&pid_path).filter(|pid| pid_alive(*pid));
    let running = pid.is_some();
    let probe = login_probe(&p.login_url);
    StatusJson {
        id: id.to_string(),
        name: p.name.clone(),
        running,
        pid,
        port: p.port,
        port_open: port_open(p.port),
        login_probe: probe,
        log_path: log_path.display().to_string(),
        pid_path: pid_path.display().to_string(),
        root: expand_user(&p.root).display().to_string(),
    }
}

fn run_argv(argv: &[String], proj: &Project) -> Result<()> {
    if argv.is_empty() {
        bail!("empty command");
    }
    let prog = render(&argv[0], proj);
    let args: Vec<String> = argv[1..].iter().map(|a| render(a, proj)).collect();
    let status = Command::new(&prog)
        .args(&args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawn {prog}"))?;
    if !status.success() {
        bail!("{prog} exited with {status}");
    }
    Ok(())
}

fn cmd_start(id: &str, p: &Project) -> Result<()> {
    eprintln!("starting {id} …");
    run_argv(&p.start, p)?;
    Ok(())
}

fn cmd_stop(id: &str, p: &Project) -> Result<()> {
    eprintln!("stopping {id} …");
    // Best-effort stop; treat already-stopped as ok if port closed.
    match run_argv(&p.stop, p) {
        Ok(()) => {}
        Err(e) => {
            let st = status_of(id, p);
            if !st.running && !st.port_open {
                eprintln!("stop reported error but process/port already down: {e}");
            } else {
                return Err(e);
            }
        }
    }
    // Wait a bit for port release
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if !port_open(p.port) && status_of(id, p).pid.is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

fn print_status(st: &StatusJson, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(st)?);
    } else {
        println!(
            "{id}  running={running}  pid={pid}  port={port} open={open}  login={code:?} ok={ok}",
            id = st.id,
            running = st.running,
            pid = st.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            port = st.port,
            open = st.port_open,
            code = st.login_probe.http_code,
            ok = st.login_probe.ok
        );
        println!("  log {}", st.log_path);
        println!("  root {}", st.root);
    }
    Ok(())
}

fn cmd_logs(p: &Project, follow: bool, lines: usize) -> Result<()> {
    let path = expand_user(&render(&p.log_file, p));
    if !path.is_file() {
        bail!("log file missing: {}", path.display());
    }
    // Use `tail` for portability on macOS
    let mut args = vec![format!("-n{lines}")];
    if follow {
        args.push("-F".into());
    }
    args.push(path.display().to_string());
    let status = Command::new("tail").args(&args).status()?;
    if !status.success() && !follow {
        bail!("tail failed: {status}");
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (_cfg_path, cfg) = load_config(&cli)?;
    match cli.cmd {
        Cmd::List { json } => {
            let mut ids: Vec<_> = cfg.projects.keys().cloned().collect();
            ids.sort();
            if json {
                let arr: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        let p = &cfg.projects[id];
                        serde_json::json!({"id": id, "name": p.name, "root": expand_user(&p.root)})
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
            } else {
                for id in ids {
                    let p = &cfg.projects[&id];
                    println!("{id}\t{}\t{}", p.name, expand_user(&p.root).display());
                }
            }
        }
        Cmd::Start { id } => {
            let p = get_proj(&cfg, &id)?;
            cmd_start(&id, p)?;
            let st = status_of(&id, p);
            print_status(&st, false)?;
        }
        Cmd::Stop { id } => {
            let p = get_proj(&cfg, &id)?;
            cmd_stop(&id, p)?;
            let st = status_of(&id, p);
            print_status(&st, false)?;
        }
        Cmd::Status { id, json } => {
            let p = get_proj(&cfg, &id)?;
            let st = status_of(&id, p);
            print_status(&st, json)?;
        }
        Cmd::Logs { id, follow, lines } => {
            let p = get_proj(&cfg, &id)?;
            cmd_logs(p, follow, lines)?;
        }
    }
    Ok(())
}
