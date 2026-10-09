use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs;
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{expand_user, render, Project};

#[derive(Debug, Clone, Serialize)]
pub struct StatusJson {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub port: u16,
    pub port_open: bool,
    pub login_probe: LoginProbe,
    pub log_path: String,
    pub pid_path: String,
    pub root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginProbe {
    pub url: String,
    pub http_code: Option<u16>,
    pub ok: bool,
}

pub fn read_pid(path: &Path) -> Option<u32> {
    let s = fs::read_to_string(path).ok()?;
    s.trim().parse().ok()
}

pub fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

pub fn port_open(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(400),
    )
    .is_ok()
}

pub fn login_probe(url: &str) -> LoginProbe {
    if url.trim().is_empty() {
        return LoginProbe {
            url: String::new(),
            http_code: None,
            ok: false,
        };
    }
    let null_dev = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let out = Command::new("curl")
        .args([
            "-sS",
            "-o",
            null_dev,
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
            let code: Option<u16> = code_s.parse().ok().filter(|c| *c > 0);
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

pub fn status_of(id: &str, p: &Project) -> StatusJson {
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

pub fn run_argv(argv: &[String], proj: &Project) -> Result<()> {
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

pub fn cmd_start(id: &str, p: &Project) -> Result<()> {
    eprintln!("starting {id} …");
    run_argv(&p.start, p)?;
    Ok(())
}

pub fn cmd_stop(id: &str, p: &Project) -> Result<()> {
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

pub fn open_url(url: &str) -> Result<()> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        bail!("URL is empty");
    }
    eprintln!("opening {trimmed} in default browser …");
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/c", "start", "", trimmed])
            .spawn()
            .with_context(|| format!("spawn start for {trimmed}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(trimmed)
            .spawn()
            .with_context(|| format!("spawn open for {trimmed}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open")
            .arg(trimmed)
            .spawn()
            .with_context(|| format!("spawn xdg-open for {trimmed}"))?;
    }
    Ok(())
}

pub fn read_last_lines(path: &Path, count: usize) -> Result<Vec<String>> {
    if !path.is_file() {
        bail!("log file missing: {}", path.display());
    }
    let bytes = fs::read(path).with_context(|| format!("read log file {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    Ok(lines[start..].iter().map(|s| s.to_string()).collect())
}

pub fn cmd_logs(p: &Project, follow: bool, lines: usize) -> Result<()> {
    let path = expand_user(&render(&p.log_file, p));
    if !path.is_file() {
        bail!("log file missing: {}", path.display());
    }
    let initial_lines = read_last_lines(&path, lines)?;
    for l in initial_lines {
        println!("{}", l);
    }
    if follow {
        let mut last_len = fs::metadata(&path)?.len();
        loop {
            thread::sleep(Duration::from_millis(500));
            if let Ok(meta) = fs::metadata(&path) {
                let len = meta.len();
                if len > last_len {
                    use std::io::{Read, Seek, SeekFrom};
                    if let Ok(mut file) = fs::File::open(&path) {
                        if file.seek(SeekFrom::Start(last_len)).is_ok() {
                            let mut buf = Vec::new();
                            if file.read_to_end(&mut buf).is_ok() {
                                let new_text = String::from_utf8_lossy(&buf);
                                print!("{}", new_text);
                            }
                        }
                    }
                    last_len = len;
                } else if len < last_len {
                    // truncated or rotated
                    last_len = len;
                }
            }
        }
    }
    Ok(())
}
