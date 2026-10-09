//! tongyun-dev — local project start/stop/status/logs
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const START_READY_DEFAULT_SECS: u64 = 180;

#[derive(Parser, Debug)]
#[command(
    name = "tongyun-dev",
    about = "Local project runner (start/stop/status/logs)"
)]
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
    /// Start a project and wait until the login page responds
    Start { id: String },
    /// Stop a project and wait until its port is free
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

struct Vars {
    root: String,
    tomcat: String,
    log_dir: String,
}

struct Resolved {
    id: String,
    name: String,
    root: PathBuf,
    tomcat: PathBuf,
    log_dir: PathBuf,
    log_file: PathBuf,
    pid_file: PathBuf,
    port: u16,
    login_url: String,
    start: Vec<String>,
    stop: Vec<String>,
}

impl Resolved {
    fn catalina_env(&self) -> Vec<(String, String)> {
        vec![
            ("CATALINA_HOME".into(), path_to_string(&self.tomcat)),
            ("CATALINA_BASE".into(), path_to_string(&self.tomcat)),
            ("CATALINA_PID".into(), path_to_string(&self.pid_file)),
        ]
    }
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

fn path_to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn current_home() -> Option<PathBuf> {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => return Some(PathBuf::from(h)),
        _ => {}
    }
    dirs::home_dir()
}

fn strip_home_prefix(s: &str) -> Option<&str> {
    if let Some(rest) = s.strip_prefix("~/") {
        return Some(rest);
    }
    if s == "~" {
        return Some("");
    }
    if let Some(rest) = s.strip_prefix("$HOME/") {
        return Some(rest);
    }
    if let Some(rest) = s.strip_prefix("${HOME}/") {
        return Some(rest);
    }
    if s == "$HOME" || s == "${HOME}" {
        return Some("");
    }
    None
}

fn expand_tilde_with(input: &str, home: Option<&Path>) -> Result<String> {
    let s = input.trim();
    if let Some(rest) = strip_home_prefix(s) {
        let home = home.with_context(|| format!("cannot expand ~ in `{s}`: HOME is not set"))?;
        if rest.is_empty() {
            return Ok(path_to_string(home));
        }
        return Ok(path_to_string(&home.join(rest)));
    }
    Ok(s.to_string())
}

fn subst(s: &str, vars: &Vars) -> String {
    s.replace("{{root}}", &vars.root)
        .replace("{{tomcat}}", &vars.tomcat)
        .replace("{{log_dir}}", &vars.log_dir)
}

fn expand_path(label: &str, raw: &str, home: Option<&Path>) -> Result<PathBuf> {
    let expanded = expand_tilde_with(raw, home)?;
    if expanded.contains("{{") {
        bail!("{label} has an unresolved template: {expanded}");
    }
    let path = PathBuf::from(&expanded);
    if !path.is_absolute() {
        bail!("{label} must be an absolute path after expansion, got `{expanded}`");
    }
    Ok(path)
}

fn render_token(raw: &str, vars: &Vars, home: Option<&Path>) -> Result<String> {
    let expanded = expand_tilde_with(&subst(raw, vars), home)?;
    if expanded.contains("{{") {
        bail!("unresolved template in `{raw}` -> `{expanded}`");
    }
    Ok(expanded)
}

fn render_argv(argv: &[String], vars: &Vars, home: Option<&Path>) -> Result<Vec<String>> {
    if argv.is_empty() {
        bail!("empty command");
    }
    argv.iter().map(|a| render_token(a, vars, home)).collect()
}

fn resolve_project(id: &str, p: &Project, home: Option<&Path>) -> Result<Resolved> {
    let root = expand_path("root", &p.root, home)?;
    let tomcat = expand_path("tomcat", &p.tomcat, home)?;
    let log_dir = expand_path("log_dir", &p.log_dir, home)?;
    let vars = Vars {
        root: path_to_string(&root),
        tomcat: path_to_string(&tomcat),
        log_dir: path_to_string(&log_dir),
    };
    let log_file = expand_path("log_file", &subst(&p.log_file, &vars), home)?;
    let pid_file = expand_path("pid_file", &subst(&p.pid_file, &vars), home)?;
    if p.port == 0 {
        bail!("port must be non-zero");
    }
    let login_url = p.login_url.trim().to_string();
    if login_url.is_empty() {
        bail!("login_url is empty");
    }
    Ok(Resolved {
        id: id.to_string(),
        name: p.name.clone(),
        root,
        tomcat,
        log_dir,
        log_file,
        pid_file,
        port: p.port,
        login_url,
        start: render_argv(&p.start, &vars, home)?,
        stop: render_argv(&p.stop, &vars, home)?,
    })
}

fn parse_config(text: &str) -> Result<FileConfig> {
    toml::from_str(text).context("parse projects.toml")
}

fn expand_config_arg(p: &Path) -> Result<PathBuf> {
    let s = p.to_str().context("config path is not utf-8")?;
    Ok(PathBuf::from(expand_tilde_with(
        s,
        current_home().as_deref(),
    )?))
}

fn load_config(cli: &Cli) -> Result<(PathBuf, FileConfig)> {
    let path = if let Some(p) = &cli.config {
        expand_config_arg(p)?
    } else if let Ok(p) = std::env::var("TONGYUN_DEV_CONFIG") {
        expand_config_arg(Path::new(p.trim()))?
    } else {
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
    let cfg = parse_config(&text)?;
    Ok((path, cfg))
}

fn get_proj<'a>(cfg: &'a FileConfig, id: &str) -> Result<&'a Project> {
    cfg.projects
        .get(id)
        .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))
}

fn load_resolved(cfg: &FileConfig, id: &str) -> Result<Resolved> {
    let p = get_proj(cfg, id)?;
    resolve_project(id, p, current_home().as_deref())
}

fn read_pid(path: &Path) -> Option<u32> {
    let s = fs::read_to_string(path).ok()?;
    let pid = s.trim().parse().ok()?;
    if pid > 1 {
        Some(pid)
    } else {
        None
    }
}

fn process_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ps_field(pid: u32, field: &str) -> Option<String> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next()?;
    let value = if first.eq_ignore_ascii_case("COMMAND")
        || first.eq_ignore_ascii_case("COMM")
        || first.eq_ignore_ascii_case("ARGS")
    {
        lines.next()?.to_string()
    } else {
        first.to_string()
    };
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn looks_like_server(comm: &str, args: &str) -> bool {
    let args_l = args.to_ascii_lowercase();
    if args_l.contains("org.apache.catalina")
        || args_l.contains("catalina.base")
        || args_l.contains("catalina.home")
    {
        return true;
    }
    // `ps args` failed: a Java comm is the best signal we have. A Java process
    // with a readable command line that is not Catalina is left alone.
    if !args.is_empty() {
        return false;
    }
    let comm_name = comm
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(comm)
        .to_ascii_lowercase();
    comm_name == "java" || comm_name == "jsvc"
}

fn is_server_process(pid: u32) -> bool {
    let comm = ps_field(pid, "comm=").unwrap_or_default();
    let args = ps_field(pid, "args=").unwrap_or_default();
    if comm.is_empty() && args.is_empty() {
        return false;
    }
    looks_like_server(&comm, &args)
}

fn listener_pids(port: u16) -> Vec<u32> {
    let out = Command::new("lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-t"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pids = Vec::new();
    for line in text.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            if pid > 1 && !pids.contains(&pid) {
                pids.push(pid);
            }
        }
    }
    pids.sort_unstable();
    pids
}

fn server_pids(p: &Resolved) -> Vec<u32> {
    let mut pids = Vec::new();
    if let Some(pid) = read_pid(&p.pid_file) {
        if process_alive(pid) && is_server_process(pid) {
            pids.push(pid);
        }
    }
    for pid in listener_pids(p.port) {
        if !pids.contains(&pid) && process_alive(pid) && is_server_process(pid) {
            pids.push(pid);
        }
    }
    pids
}

fn port_open(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    let Ok(addr) = addr.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()
}

fn probe_addr(url: &str) -> Option<SocketAddr> {
    let url = url.trim();
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else {
        (false, url.strip_prefix("http://")?)
    };
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    if hostport.is_empty() {
        return None;
    }
    let hp = if hostport.contains(':') {
        hostport.to_string()
    } else if https {
        format!("{hostport}:443")
    } else {
        format!("{hostport}:80")
    };
    hp.parse().ok()
}

fn parse_http_code(raw: &str) -> Option<u16> {
    let s = raw.trim();
    if s.is_empty() || s == "000" {
        return None;
    }
    let code = s.parse::<u16>().ok()?;
    if code == 0 {
        None
    } else {
        Some(code)
    }
}

fn http_ok(code: Option<u16>) -> bool {
    matches!(code, Some(200) | Some(301) | Some(302))
}

fn login_probe(url: &str) -> LoginProbe {
    let closed = probe_addr(url)
        .map(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_err())
        .unwrap_or(false);
    if closed {
        return LoginProbe {
            url: url.to_string(),
            http_code: None,
            ok: false,
        };
    }
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) => {
            let code = parse_http_code(&String::from_utf8_lossy(&o.stdout));
            LoginProbe {
                url: url.to_string(),
                http_code: code,
                ok: http_ok(code),
            }
        }
        Err(_) => LoginProbe {
            url: url.to_string(),
            http_code: None,
            ok: false,
        },
    }
}

fn status_of(p: &Resolved) -> StatusJson {
    let pid = server_pids(p).into_iter().next();
    let probe = login_probe(&p.login_url);
    StatusJson {
        id: p.id.clone(),
        name: p.name.clone(),
        running: pid.is_some(),
        pid,
        port: p.port,
        port_open: port_open(p.port),
        login_probe: probe,
        log_path: path_to_string(&p.log_file),
        pid_path: path_to_string(&p.pid_file),
        root: path_to_string(&p.root),
    }
}

fn is_down(p: &Resolved) -> bool {
    server_pids(p).is_empty() && !port_open(p.port)
}

fn run_cmd(argv: &[String], dir: Option<&Path>, env: &[(String, String)]) -> Result<()> {
    if argv.is_empty() {
        bail!("empty command");
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null());
    for (k, v) in env {
        if !v.is_empty() {
            cmd.env(k, v);
        }
    }
    if let Some(dir) = dir {
        if dir.is_dir() {
            cmd.current_dir(dir);
        }
    }
    let status = cmd.status().with_context(|| format!("spawn {}", argv[0]))?;
    if !status.success() {
        bail!("{} exited with {status}", argv.join(" "));
    }
    Ok(())
}

fn script_tokens(argv: &[String]) -> impl Iterator<Item = &String> {
    argv.iter()
        .filter(|a| a.contains('/') && a.ends_with(".sh"))
}

fn first_missing_script(argv: &[String]) -> Option<&str> {
    script_tokens(argv)
        .find(|a| !Path::new(a.as_str()).is_file())
        .map(String::as_str)
}

fn ensure_log_dir(dir: &Path, home: Option<&Path>) -> Result<()> {
    if dir == Path::new("/") || home.is_some_and(|h| dir == h) {
        bail!("refusing to create log dir {}", dir.display());
    }
    fs::create_dir_all(dir).with_context(|| format!("create log dir {}", dir.display()))?;
    Ok(())
}

fn start_ready_timeout() -> Result<Duration> {
    match std::env::var("TONGYUN_DEV_START_TIMEOUT") {
        Ok(s) => {
            let secs: u64 = s.trim().parse().with_context(|| {
                format!("TONGYUN_DEV_START_TIMEOUT must be a number of seconds, got `{s}`")
            })?;
            if secs == 0 {
                bail!("TONGYUN_DEV_START_TIMEOUT must be > 0");
            }
            Ok(Duration::from_secs(secs))
        }
        Err(_) => Ok(Duration::from_secs(START_READY_DEFAULT_SECS)),
    }
}

fn wait_until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pred() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn cmd_start(p: &Resolved) -> Result<()> {
    eprintln!("starting {} …", p.id);
    eprintln!("  root {}", p.root.display());
    eprintln!(
        "  script {}",
        script_tokens(&p.start)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    );
    eprintln!("  log {}", p.log_file.display());
    if !p.root.is_dir() {
        bail!("project root not found: {}", p.root.display());
    }
    if let Some(missing) = first_missing_script(&p.start) {
        bail!("start script not found: {missing}");
    }
    ensure_log_dir(&p.log_dir, current_home().as_deref())?;
    run_cmd(&p.start, Some(&p.root), &p.catalina_env())?;
    let timeout = start_ready_timeout()?;
    eprintln!("waiting up to {}s for {}", timeout.as_secs(), p.login_url);
    let started = Instant::now();
    let st = loop {
        let st = status_of(p);
        if st.login_probe.ok || started.elapsed() >= timeout {
            break st;
        }
        thread::sleep(Duration::from_secs(1));
    };
    print_status(&st, false)?;
    if !st.login_probe.ok {
        bail!(
            "login page not ready at {} (http {:?}). log: {}",
            p.login_url,
            st.login_probe.http_code,
            p.log_file.display()
        );
    }
    Ok(())
}

fn signal_pid(pid: u32, sig: &str) -> bool {
    Command::new("kill")
        .args([sig, &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn terminate_server(pid: u32) -> Result<()> {
    if pid <= 1 {
        bail!("refusing to signal pid {pid}");
    }
    let comm = ps_field(pid, "comm=").unwrap_or_else(|| "java".into());
    eprintln!("sending TERM to {pid} ({comm})");
    signal_pid(pid, "-TERM");
    let gone = wait_until(Duration::from_secs(8), || !process_alive(pid));
    if gone {
        return Ok(());
    }
    eprintln!("sending KILL to {pid} ({comm})");
    signal_pid(pid, "-KILL");
    if wait_until(Duration::from_secs(3), || !process_alive(pid)) {
        Ok(())
    } else {
        bail!("pid {pid} still alive after KILL");
    }
}

fn listener_hint(port: u16) -> String {
    let pids = listener_pids(port);
    if pids.is_empty() {
        return format!("port {port} is open; lsof found no listener");
    }
    let parts: Vec<String> = pids
        .into_iter()
        .map(|pid| {
            let comm = ps_field(pid, "comm=").unwrap_or_else(|| "?".into());
            format!("{pid} {comm}")
        })
        .collect();
    format!("port {port} held by {}", parts.join(", "))
}

fn cmd_stop(p: &Resolved) -> Result<()> {
    eprintln!("stopping {} …", p.id);
    eprintln!("  catalina {}", p.tomcat.join("bin/catalina.sh").display());
    eprintln!("  pid file {}", p.pid_file.display());
    if is_down(p) {
        eprintln!("{} already stopped", p.id);
        return Ok(());
    }
    if let Some(missing) = first_missing_script(&p.stop) {
        eprintln!("stop script not found: {missing}");
    } else if let Err(e) = run_cmd(&p.stop, Some(&p.tomcat), &p.catalina_env()) {
        eprintln!("catalina stop reported an error: {e}");
    }
    if !wait_until(Duration::from_secs(20), || is_down(p)) {
        let pids = server_pids(p);
        if pids.is_empty() {
            bail!("stop did not release the port: {}", listener_hint(p.port));
        }
        for pid in pids {
            terminate_server(pid)?;
        }
        if !wait_until(Duration::from_secs(10), || is_down(p)) {
            bail!("stop did not release the port: {}", listener_hint(p.port));
        }
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

fn cmd_logs(p: &Resolved, follow: bool, lines: usize) -> Result<()> {
    let path = &p.log_file;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            bail!(
                "log directory missing: {} (log {})",
                parent.display(),
                path.display()
            );
        }
    }
    if follow && !path.is_file() {
        eprintln!("waiting for {} (Ctrl-C to stop)", path.display());
        while !path.is_file() {
            thread::sleep(Duration::from_millis(300));
        }
    }
    if !path.is_file() {
        bail!("log file missing: {}", path.display());
    }
    let mut args = vec![format!("-n{lines}")];
    if follow {
        args.push("-F".into());
    }
    args.push(path.display().to_string());
    let status = Command::new("tail")
        .args(&args)
        .status()
        .context("spawn tail")?;
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
            let home = current_home();
            let mut rows = Vec::new();
            for id in ids {
                let p = &cfg.projects[&id];
                let resolved = resolve_project(&id, p, home.as_deref())?;
                rows.push(resolved);
            }
            if json {
                let arr: Vec<_> = rows
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id,
                            "name": p.name,
                            "root": path_to_string(&p.root),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
            } else {
                for p in &rows {
                    println!("{}\t{}\t{}", p.id, p.name, p.root.display());
                }
            }
        }
        Cmd::Start { id } => {
            let p = load_resolved(&cfg, &id)?;
            cmd_start(&p)?;
        }
        Cmd::Stop { id } => {
            let p = load_resolved(&cfg, &id)?;
            cmd_stop(&p)?;
            let st = status_of(&p);
            print_status(&st, false)?;
        }
        Cmd::Status { id, json } => {
            let p = load_resolved(&cfg, &id)?;
            let st = status_of(&p);
            print_status(&st, json)?;
        }
        Cmd::Logs { id, follow, lines } => {
            let p = load_resolved(&cfg, &id)?;
            cmd_logs(&p, follow, lines)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECTS_TOML: &str = include_str!("../../../config/projects.toml");

    fn legacy_app(home: &Path) -> Resolved {
        let cfg = parse_config(PROJECTS_TOML).unwrap();
        resolve_project("legacy_app", cfg.projects.get("legacy_app").unwrap(), Some(home)).unwrap()
    }

    #[test]
    fn expands_tilde_and_home_forms() {
        let home = Path::new("/Users/dev");
        assert_eq!(
            expand_tilde_with("~/code/acme/legacy-app", Some(home)).unwrap(),
            "/Users/dev/code/acme/legacy-app"
        );
        assert_eq!(
            expand_tilde_with("$HOME/.tongyun-dev/logs/legacy_app_catalina.out", Some(home)).unwrap(),
            "/Users/dev/.tongyun-dev/logs/legacy_app_catalina.out"
        );
        assert_eq!(
            expand_tilde_with("${HOME}/opt/tomcat-9", Some(home)).unwrap(),
            "/Users/dev/opt/tomcat-9"
        );
        let err = expand_tilde_with("~/code/acme/legacy-app", None).unwrap_err();
        assert!(err.to_string().contains("HOME is not set"));
    }

    #[test]
    fn legacy_app_paths_resolve_without_tilde() {
        let p = legacy_app(Path::new("/Users/dev"));
        assert_eq!(
            p.root,
            PathBuf::from("/Users/dev/code/acme/legacy-app")
        );
        assert_eq!(
            p.tomcat,
            PathBuf::from("/Users/dev/opt/tomcat-9")
        );
        assert_eq!(
            p.log_file,
            PathBuf::from("/Users/dev/.tongyun-dev/logs/legacy_app_catalina.out")
        );
        assert_eq!(
            p.pid_file,
            PathBuf::from("/Users/dev/.tongyun-dev/logs/legacy_app_tomcat.pid")
        );
        assert_eq!(
            p.start,
            vec![
                "bash".to_string(),
                "/Users/dev/code/acme/legacy-app/scripts/start-legacy.sh".to_string(),
            ]
        );
        assert_eq!(
            p.stop,
            vec![
                "bash".to_string(),
                "/Users/dev/opt/tomcat-9/bin/catalina.sh".to_string(),
                "stop".to_string(),
                "15".to_string(),
                "-force".to_string(),
            ]
        );
        let joined = format!(
            "{} {} {} {:?} {:?}",
            path_to_string(&p.root),
            path_to_string(&p.log_file),
            path_to_string(&p.pid_file),
            p.start,
            p.stop
        );
        assert!(!joined.contains('~'));
        assert!(!joined.contains("{{"));
        let env = p.catalina_env();
        assert!(env.contains(&("CATALINA_HOME".into(), path_to_string(&p.tomcat))));
        assert!(env.contains(&("CATALINA_BASE".into(), path_to_string(&p.tomcat))));
        assert!(env.contains(&("CATALINA_PID".into(), path_to_string(&p.pid_file))));
        assert_eq!(p.port, 8080);
        assert_eq!(
            p.login_url,
            "http://127.0.0.1:8080/legacy_app/admin/login.html"
        );
    }

    #[test]
    fn config_has_no_secret_material() {
        let lower = PROJECTS_TOML.to_ascii_lowercase();
        for bad in ["password", "secret", "token", "jdbc", "api_key", "apikey"] {
            assert!(!lower.contains(bad), "{bad} appeared in projects.toml");
        }
    }

    #[test]
    fn status_json_fields_stay_stable() {
        let st = StatusJson {
            id: "legacy_app".into(),
            name: "legacy_app".into(),
            running: false,
            pid: None,
            port: 8080,
            port_open: false,
            login_probe: LoginProbe {
                url: "http://127.0.0.1:8080/legacy_app/admin/login.html".into(),
                http_code: None,
                ok: false,
            },
            log_path: "/Users/dev/.tongyun-dev/logs/legacy_app_catalina.out".into(),
            pid_path: "/Users/dev/.tongyun-dev/logs/legacy_app_tomcat.pid".into(),
            root: "/Users/dev/code/acme/legacy-app".into(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        for key in ["running", "pid", "port", "login_probe", "log_path"] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["running"], false);
        assert!(v["pid"].is_null());
        assert_eq!(v["port"], 8080);
        assert!(v["login_probe"]["http_code"].is_null());
        assert_eq!(v["login_probe"]["ok"], false);
        assert_eq!(
            v["log_path"],
            "/Users/dev/.tongyun-dev/logs/legacy_app_catalina.out"
        );
    }

    #[test]
    fn http_probe_helpers() {
        assert_eq!(parse_http_code("000"), None);
        assert_eq!(parse_http_code("200"), Some(200));
        assert!(http_ok(Some(200)));
        assert!(http_ok(Some(302)));
        assert!(!http_ok(Some(500)));
        assert!(!http_ok(None));
        assert_eq!(
            probe_addr("http://127.0.0.1:8080/legacy_app/admin/login.html"),
            "127.0.0.1:8080".parse().ok()
        );
        assert!(looks_like_server("java", ""));
        assert!(looks_like_server(
            "bash",
            "org.apache.catalina.startup.Bootstrap start"
        ));
        assert!(!looks_like_server("java", "my-other-app.jar"));
        assert!(!looks_like_server("nginx", "nginx: master process"));
    }
}
