//! Shared project runner used by the CLI and the desktop shell.
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const START_READY_DEFAULT_SECS: u64 = 180;
const LOG_TAIL_MAX_LINES: usize = 500;
const LOG_TAIL_WINDOW: u64 = 512 * 1024;

#[derive(Debug)]
pub struct Loaded {
    path: PathBuf,
    config: FileConfig,
}

#[derive(Debug, serde::Deserialize)]
struct FileConfig {
    projects: HashMap<String, Project>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Project {
    name: String,
    root: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    start: Vec<String>,
    #[serde(default)]
    stop: Vec<String>,
    #[serde(default)]
    tomcat: String,
    #[serde(default)]
    log_dir: String,
    #[serde(default)]
    log_file: String,
    #[serde(default)]
    pid_file: String,
    #[serde(default)]
    port: u16,
    #[serde(default)]
    login_url: String,
    #[serde(default)]
    health_url: String,
    /// Long-running dev servers (mvn, pnpm) are detached. Tomcat's start script exits.
    #[serde(default)]
    background: bool,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    java: Option<String>,
}

struct Vars {
    root: String,
    tomcat: String,
    log_dir: String,
}

struct Resolved {
    id: String,
    name: String,
    enabled: bool,
    reason: String,
    root: PathBuf,
    tomcat: Option<PathBuf>,
    log_dir: PathBuf,
    log_file: PathBuf,
    pid_file: PathBuf,
    port: u16,
    /// `login_url` when set, otherwise `health_url`. Empty means ready == port open.
    login_url: String,
    background: bool,
    start: Vec<String>,
    stop: Vec<String>,
    node: Option<String>,
    java: Option<String>,
}

impl Resolved {
    fn catalina_env(&self) -> Vec<(String, String)> {
        let Some(tomcat) = &self.tomcat else {
            return Vec::new();
        };
        vec![
            ("CATALINA_HOME".into(), path_to_string(tomcat)),
            ("CATALINA_BASE".into(), path_to_string(tomcat)),
            ("CATALINA_PID".into(), path_to_string(&self.pid_file)),
        ]
    }

    pub fn assembled_start(&self) -> Vec<String> {
        assemble_start_cmd(&self.start, self.node.as_deref(), self.java.as_deref())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub root: String,
    pub enabled: bool,
    pub port: u16,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub config_path: String,
    pub projects: Vec<ProjectRow>,
}

#[derive(Debug, Serialize)]
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
    /// True when the port is taken by a process that is not this project's server.
    pub port_conflict: bool,
    /// `pid comm` of the other listener, with a project id when it matches one. Empty if unknown.
    pub port_holder: String,
}

#[derive(Debug, Serialize)]
pub struct LoginProbe {
    pub url: String,
    pub http_code: Option<u16>,
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub struct LogTail {
    pub path: String,
    pub missing: bool,
    pub text: String,
}

impl Loaded {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<ProjectList> {
        let mut ids: Vec<_> = self.config.projects.keys().cloned().collect();
        ids.sort();
        let mut projects = Vec::with_capacity(ids.len());
        for id in ids {
            let resolved = self.resolve(&id)?;
            projects.push(ProjectRow {
                id: resolved.id,
                name: resolved.name,
                root: path_to_string(&resolved.root),
                enabled: resolved.enabled,
                port: resolved.port,
                reason: resolved.reason,
                node: resolved.node,
                java: resolved.java,
            });
        }
        Ok(ProjectList {
            config_path: path_to_string(&self.path),
            projects,
        })
    }

    pub fn status(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        let peers = self.peer_pids(&project.id);
        Ok(status_of(&project, &peers))
    }

    fn peer_pids(&self, current: &str) -> Vec<(String, u32)> {
        let mut ids: Vec<_> = self.config.projects.keys().cloned().collect();
        ids.sort();
        let mut peers = Vec::new();
        for id in ids {
            if id == current {
                continue;
            }
            let Ok(other) = self.resolve(&id) else {
                continue;
            };
            if let Some(pid) = read_pid(&other.pid_file) {
                if process_alive(pid) {
                    peers.push((other.id, pid));
                }
            }
        }
        peers
    }

    pub fn start(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        cmd_start(&project)?;
        let peers = self.peer_pids(&project.id);
        Ok(status_of(&project, &peers))
    }

    pub fn stop(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        cmd_stop(&project)?;
        let peers = self.peer_pids(&project.id);
        Ok(status_of(&project, &peers))
    }

    /// Last lines of the catalina log. `missing` is true when the file is not there yet.
    pub fn log_tail(&self, id: &str, lines: usize) -> Result<LogTail> {
        let project = self.resolve(id)?;
        ensure_log_parent(&project.log_file)?;
        let path = path_to_string(&project.log_file);
        let lines = lines.clamp(1, LOG_TAIL_MAX_LINES);
        if !project.log_file.is_file() {
            return Ok(LogTail {
                path,
                missing: true,
                text: String::new(),
            });
        }
        let text = read_tail(&project.log_file, lines)?;
        Ok(LogTail {
            path,
            missing: false,
            text,
        })
    }

    /// Block and stream the log with `tail -F`, for the CLI `--follow` path.
    pub fn follow_log(&self, id: &str, lines: usize) -> Result<()> {
        let project = self.resolve(id)?;
        let path = &project.log_file;
        ensure_log_parent(path)?;
        if !path.is_file() {
            eprintln!("waiting for {} (Ctrl-C to stop)", path.display());
            while !path.is_file() {
                thread::sleep(Duration::from_millis(300));
            }
        }
        Command::new("tail")
            .args([
                format!("-n{lines}"),
                "-F".into(),
                path.display().to_string(),
            ])
            .status()
            .context("spawn tail")?;
        Ok(())
    }

    fn resolve(&self, id: &str) -> Result<Resolved> {
        let project = self
            .config
            .projects
            .get(id)
            .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))?;
        resolve_project(id, project, current_home().as_deref())
    }
}

pub fn load(explicit: Option<&Path>) -> Result<Loaded> {
    let path = resolve_config_path(explicit)?;
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let config = parse_config(&text)?;
    Ok(Loaded { path, config })
}

fn resolve_config_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return expand_config_arg(p);
    }
    if let Ok(p) = std::env::var("TONGYUN_DEV_CONFIG") {
        return expand_config_arg(Path::new(p.trim()));
    }
    if let Some(path) = std::env::current_dir().ok().and_then(walk_for_config) {
        return Ok(path);
    }
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .and_then(walk_for_config)
    {
        return Ok(path);
    }
    bail!("config/projects.toml not found; pass --config or set TONGYUN_DEV_CONFIG");
}

fn walk_for_config(start: PathBuf) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        let cand = cur.join("config/projects.toml");
        if cand.is_file() {
            return Some(cand);
        }
        if !cur.pop() {
            return None;
        }
    }
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

fn default_under_log(id: &str, suffix: &str) -> String {
    let mut raw = String::from("{{log_dir}}/");
    raw.push_str(id);
    raw.push_str(suffix);
    raw
}

fn resolve_project(id: &str, p: &Project, home: Option<&Path>) -> Result<Resolved> {
    if p.enabled && p.port == 0 {
        bail!("port must be non-zero");
    }
    if !p.enabled && p.reason.trim().is_empty() {
        bail!("disabled project `{id}` needs a reason");
    }
    let root = expand_path("root", &p.root, home)?;
    let tomcat = if p.tomcat.trim().is_empty() {
        None
    } else {
        Some(expand_path("tomcat", &p.tomcat, home)?)
    };
    let log_dir_raw = if p.log_dir.trim().is_empty() {
        "~/.tongyun-dev/logs".to_string()
    } else {
        p.log_dir.clone()
    };
    let log_dir = expand_path("log_dir", &log_dir_raw, home)?;
    let vars = Vars {
        root: path_to_string(&root),
        tomcat: tomcat
            .as_ref()
            .map(|t| path_to_string(t))
            .unwrap_or_default(),
        log_dir: path_to_string(&log_dir),
    };
    let log_file_raw = if p.log_file.trim().is_empty() {
        default_under_log(id, ".out")
    } else {
        p.log_file.clone()
    };
    let pid_file_raw = if p.pid_file.trim().is_empty() {
        default_under_log(id, ".pid")
    } else {
        p.pid_file.clone()
    };
    let log_file = expand_path("log_file", &subst(&log_file_raw, &vars), home)?;
    let pid_file = expand_path("pid_file", &subst(&pid_file_raw, &vars), home)?;
    let login = p.login_url.trim();
    let login_url = if login.is_empty() {
        p.health_url.trim().to_string()
    } else {
        login.to_string()
    };
    if p.enabled && p.start.is_empty() {
        bail!("project `{id}` has an empty start command");
    }
    if p.enabled && tomcat.is_some() && p.stop.is_empty() {
        bail!("project `{id}` needs a stop command");
    }
    let start = if p.start.is_empty() {
        Vec::new()
    } else {
        render_argv(&p.start, &vars, home)?
    };
    let stop = if p.stop.is_empty() {
        Vec::new()
    } else {
        render_argv(&p.stop, &vars, home)?
    };
    let node = p
        .node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let java = p
        .java
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(Resolved {
        id: id.to_string(),
        name: p.name.clone(),
        enabled: p.enabled,
        reason: p.reason.trim().to_string(),
        root,
        tomcat,
        log_dir,
        log_file,
        pid_file,
        port: p.port,
        login_url,
        background: p.background,
        start,
        stop,
        node,
        java,
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

fn read_pid(path: &Path) -> Option<u32> {
    let s = std::fs::read_to_string(path).ok()?;
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
    if port == 0 {
        return Vec::new();
    }
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

/// Dev servers (Spring / pnpm) count as running only when the pid file is alive
/// or a listener is a child of that process. A stranger on the port is a conflict.
fn dev_pids(p: &Resolved) -> Vec<u32> {
    let mut roots = Vec::new();
    if let Some(pid) = read_pid(&p.pid_file) {
        if process_alive(pid) {
            roots.push(pid);
        }
    }
    let mut pids = roots.clone();
    for pid in listener_pids(p.port) {
        if !pids.contains(&pid) && process_alive(pid) && belongs_to(pid, &roots) {
            pids.push(pid);
        }
    }
    pids
}

fn tracked_pids(p: &Resolved) -> Vec<u32> {
    if p.tomcat.is_some() {
        server_pids(p)
    } else {
        dev_pids(p)
    }
}

fn port_open(port: u16) -> bool {
    if port == 0 {
        return false;
    }
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

fn ppid(pid: u32) -> Option<u32> {
    let text = ps_field(pid, "ppid=")?;
    let parent = text.trim().parse::<u32>().ok()?;
    (parent > 0).then_some(parent)
}

fn belongs_to(pid: u32, roots: &[u32]) -> bool {
    belongs_to_with(pid, roots, ppid)
}

fn belongs_to_with(pid: u32, roots: &[u32], mut parent_of: impl FnMut(u32) -> Option<u32>) -> bool {
    if roots.is_empty() {
        return false;
    }
    if roots.contains(&pid) {
        return true;
    }
    let mut current = pid;
    for _ in 0..12 {
        let Some(parent) = parent_of(current) else {
            return false;
        };
        if parent <= 1 || parent == current {
            return false;
        }
        if roots.contains(&parent) {
            return true;
        }
        current = parent;
    }
    false
}

fn describe_holder(pid: u32, peers: &[(String, u32)]) -> String {
    let raw = ps_field(pid, "comm=").unwrap_or_else(|| "?".into());
    let comm = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw.as_str())
        .trim();
    let comm = if comm.is_empty() { "?" } else { comm };
    match peers
        .iter()
        .find(|(_, peer)| belongs_to(pid, &[*peer]))
        .map(|(id, _)| id.as_str())
    {
        Some(id) => format!("{pid} {comm} ({id})"),
        None => format!("{pid} {comm}"),
    }
}

fn port_conflict_of(
    p: &Resolved,
    own: &[u32],
    peers: &[(String, u32)],
    open: bool,
) -> (bool, String) {
    let mut holders = Vec::new();
    for pid in listener_pids(p.port) {
        if !process_alive(pid) || belongs_to(pid, own) {
            continue;
        }
        holders.push(describe_holder(pid, peers));
    }
    if holders.is_empty() && open && own.is_empty() && p.port != 0 {
        return (true, String::new());
    }
    if holders.is_empty() {
        (false, String::new())
    } else {
        (true, holders.join(", "))
    }
}

fn status_of(p: &Resolved, peers: &[(String, u32)]) -> StatusJson {
    let own = tracked_pids(p);
    let open = port_open(p.port);
    let (port_conflict, port_holder) = port_conflict_of(p, &own, peers, open);
    let pid = own.into_iter().next();
    StatusJson {
        id: p.id.clone(),
        name: p.name.clone(),
        running: pid.is_some(),
        pid,
        port: p.port,
        port_open: open,
        login_probe: login_probe(&p.login_url),
        log_path: path_to_string(&p.log_file),
        pid_path: path_to_string(&p.pid_file),
        root: path_to_string(&p.root),
        port_conflict,
        port_holder,
    }
}

fn is_down(p: &Resolved) -> bool {
    tracked_pids(p).is_empty() && !port_open(p.port)
}

fn is_ready(p: &Resolved) -> bool {
    if p.login_url.is_empty() {
        port_open(p.port)
    } else {
        login_probe(&p.login_url).ok
    }
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

fn ensure_log_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            bail!(
                "log directory missing: {} (log {})",
                parent.display(),
                path.display()
            );
        }
    }
    Ok(())
}

fn ensure_log_dir(dir: &Path, home: Option<&Path>) -> Result<()> {
    if dir == Path::new("/") || home.is_some_and(|h| dir == h) {
        bail!("refusing to create log dir {}", dir.display());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("create log dir {}", dir.display()))?;
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

pub fn assemble_start_cmd(start: &[String], node: Option<&str>, java: Option<&str>) -> Vec<String> {
    if start.is_empty() {
        return Vec::new();
    }
    let mut cmd = start.to_vec();
    if let Some(v) = node.map(str::trim).filter(|s| !s.is_empty()) {
        let mut fnm_prefix = vec![
            "fnm".to_string(),
            "exec".to_string(),
            format!("--using={v}"),
            "--".to_string(),
        ];
        fnm_prefix.extend(cmd);
        cmd = fnm_prefix;
    }
    if let Some(v) = java.map(str::trim).filter(|s| !s.is_empty()) {
        let mut sdk_prefix = vec![
            "bash".to_string(),
            "-lc".to_string(),
            format!("sdk use java {v} && exec \"$@\""),
            "--".to_string(),
        ];
        sdk_prefix.extend(cmd);
        cmd = sdk_prefix;
    }
    cmd
}

pub fn check_toolchain_availability(
    node: Option<&str>,
    java: Option<&str>,
    home: Option<&Path>,
) -> Result<()> {
    if let Some(v) = node.map(str::trim).filter(|s| !s.is_empty()) {
        check_fnm_installed(v)?;
    }
    if let Some(v) = java.map(str::trim).filter(|s| !s.is_empty()) {
        check_sdkman_installed(v, home)?;
    }
    Ok(())
}

fn check_fnm_installed(v: &str) -> Result<()> {
    let status = Command::new("fnm")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(_) => bail!("fnm failed to execute; required for node = \"{v}\""),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let bash_check = Command::new("bash")
                .args(["-lc", "command -v fnm >/dev/null 2>&1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if let Ok(st) = bash_check {
                if st.success() {
                    return Ok(());
                }
            }
            bail!("fnm not found in PATH; required for node = \"{v}\"");
        }
        Err(err) => bail!("failed to check fnm: {err}; required for node = \"{v}\""),
    }
}

fn check_sdkman_installed(v: &str, home: Option<&Path>) -> Result<()> {
    let has_sdk_file = if let Ok(sdk_dir) = std::env::var("SDKMAN_DIR") {
        Path::new(&sdk_dir).join("bin/sdkman-init.sh").is_file()
    } else if let Some(h) = home {
        h.join(".sdkman/bin/sdkman-init.sh").is_file()
    } else {
        false
    };
    if has_sdk_file {
        return Ok(());
    }

    let status = Command::new("bash")
        .args(["-lc", "command -v sdk >/dev/null 2>&1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if let Ok(st) = status {
        if st.success() {
            return Ok(());
        }
    }

    bail!("sdkman not found; required for java = \"{v}\"");
}

fn spawn_background(p: &Resolved, start_cmd: &[String]) -> Result<()> {
    if start_cmd.is_empty() {
        bail!("empty start command");
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.log_file)
        .with_context(|| format!("open log {}", p.log_file.display()))?;
    let log_err = log
        .try_clone()
        .with_context(|| format!("clone log handle {}", p.log_file.display()))?;
    let mut cmd = Command::new(&start_cmd[0]);
    cmd.args(&start_cmd[1..])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .current_dir(&p.root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", start_cmd[0]))?;
    let pid = child.id();
    if let Err(err) = std::fs::write(&p.pid_file, format!("{pid}\n")) {
        signal_pid(pid, "-KILL");
        return Err(err).with_context(|| format!("write pid {}", p.pid_file.display()));
    }
    thread::spawn(move || {
        let _ = child.wait();
    });
    eprintln!("  pid {pid}");
    Ok(())
}

fn cmd_start(p: &Resolved) -> Result<()> {
    if !p.enabled {
        let why = if p.reason.is_empty() {
            "disabled"
        } else {
            p.reason.as_str()
        };
        bail!("project `{}` is disabled: {why}", p.id);
    }
    check_toolchain_availability(
        p.node.as_deref(),
        p.java.as_deref(),
        current_home().as_deref(),
    )?;
    let start_cmd = p.assembled_start();
    eprintln!("starting {} …", p.id);
    eprintln!("  root {}", p.root.display());
    if p.background {
        eprintln!("  background port {}", p.port);
    } else {
        eprintln!(
            "  script {}",
            script_tokens(&start_cmd)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    eprintln!("  log {}", p.log_file.display());
    if !p.root.is_dir() {
        bail!("project root not found: {}", p.root.display());
    }
    if let Some(missing) = first_missing_script(&start_cmd) {
        bail!("start script not found: {missing}");
    }
    ensure_log_dir(&p.log_dir, current_home().as_deref())?;
    if p.background && is_ready(p) && !tracked_pids(p).is_empty() {
        eprintln!("{} already up on port {}", p.id, p.port);
        return Ok(());
    }
    if p.background {
        spawn_background(p, &start_cmd)?;
    } else {
        run_cmd(&start_cmd, Some(&p.root), &p.catalina_env())?;
    }
    let timeout = start_ready_timeout()?;
    if p.login_url.is_empty() {
        eprintln!("waiting up to {}s for port {}", timeout.as_secs(), p.port);
    } else {
        eprintln!("waiting up to {}s for {}", timeout.as_secs(), p.login_url);
    }
    let started = Instant::now();
    let ready = loop {
        if is_ready(p) || started.elapsed() >= timeout {
            break is_ready(p);
        }
        thread::sleep(Duration::from_secs(1));
    };
    if !ready {
        if p.login_url.is_empty() {
            bail!("port {} not open. log: {}", p.port, p.log_file.display());
        }
        let probe = login_probe(&p.login_url);
        bail!(
            "login page not ready at {} (http {:?}). log: {}",
            p.login_url,
            probe.http_code,
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

fn signal_group(pid: u32, sig: &str) -> bool {
    if pid <= 1 {
        return false;
    }
    Command::new("kill")
        .args([sig, "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn terminate_pid(pid: u32, group: bool) -> Result<()> {
    if pid <= 1 {
        bail!("refusing to signal pid {pid}");
    }
    let comm = ps_field(pid, "comm=").unwrap_or_else(|| "process".into());
    eprintln!("sending TERM to {pid} ({comm})");
    if group {
        signal_group(pid, "-TERM");
    }
    signal_pid(pid, "-TERM");
    if wait_until(Duration::from_secs(8), || !process_alive(pid)) {
        return Ok(());
    }
    eprintln!("sending KILL to {pid} ({comm})");
    if group {
        signal_group(pid, "-KILL");
    }
    signal_pid(pid, "-KILL");
    if wait_until(Duration::from_secs(3), || !process_alive(pid)) {
        Ok(())
    } else {
        bail!("pid {pid} still alive after KILL");
    }
}

fn terminate_server(pid: u32) -> Result<()> {
    terminate_pid(pid, false)
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
    if p.tomcat.is_some() {
        cmd_stop_catalina(p)
    } else {
        cmd_stop_dev(p)
    }
}

fn cmd_stop_catalina(p: &Resolved) -> Result<()> {
    let Some(tomcat) = p.tomcat.as_ref() else {
        bail!("internal: catalina stop without tomcat");
    };
    eprintln!("stopping {} …", p.id);
    eprintln!("  catalina {}", tomcat.join("bin/catalina.sh").display());
    eprintln!("  pid file {}", p.pid_file.display());
    if is_down(p) {
        eprintln!("{} already stopped", p.id);
        return Ok(());
    }
    if let Some(missing) = first_missing_script(&p.stop) {
        eprintln!("stop script not found: {missing}");
    } else if let Err(e) = run_cmd(&p.stop, Some(tomcat), &p.catalina_env()) {
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

fn cmd_stop_dev(p: &Resolved) -> Result<()> {
    eprintln!("stopping {} …", p.id);
    eprintln!("  pid file {}", p.pid_file.display());
    eprintln!("  port {}", p.port);
    if is_down(p) {
        eprintln!("{} already stopped", p.id);
        return Ok(());
    }
    if !p.stop.is_empty() {
        if let Some(missing) = first_missing_script(&p.stop) {
            eprintln!("stop script not found: {missing}");
        } else if let Err(e) = run_cmd(&p.stop, Some(&p.root), &[]) {
            eprintln!("stop command reported an error: {e}");
        }
    }
    if let Some(pid) = read_pid(&p.pid_file) {
        if process_alive(pid) {
            terminate_pid(pid, true)?;
        }
    }
    for pid in listener_pids(p.port) {
        if process_alive(pid) {
            terminate_server(pid)?;
        }
    }
    if !wait_until(Duration::from_secs(10), || is_down(p)) {
        bail!("stop did not release the port: {}", listener_hint(p.port));
    }
    Ok(())
}

fn read_tail(path: &Path, max_lines: usize) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("read {}", path.display()))?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(LOG_TAIL_WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    if start > 0 {
        if let Some(i) = buf.iter().position(|b| *b == b'\n') {
            buf.drain(..=i);
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(max_lines);
    let mut out = lines[skip..].join("\n");
    if text.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Registry: add / remove projects in config/projects.toml.
// Used by the CLI (`add` / `remove`) and the web console. Edits go through
// toml_edit so comments and the layout of other projects are preserved.
// ---------------------------------------------------------------------------

/// Port used by `add` when the caller does not give one.
pub const DEFAULT_ADD_PORT: u16 = 8080;

/// A project registered from the CLI or the web console.
/// `start` / `stop` are shell command lines; they are stored as `["bash", "-c", <cmd>]`
/// (or `["bash", "-lc", <cmd>]` when no `node` / `java` is declared, so the user's
/// login PATH is available). Added projects run in the background by default,
/// because dev servers (`npm run dev`, `mvn spring-boot:run`) do not exit.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NewProject {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub root: String,
    pub start: String,
    #[serde(default)]
    pub stop: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub login_url: Option<String>,
    #[serde(default)]
    pub health_url: Option<String>,
    #[serde(default)]
    pub log_file: Option<String>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub java: Option<String>,
    #[serde(default)]
    pub background: Option<bool>,
}

fn non_empty(v: &Option<String>) -> Option<String> {
    v.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn valid_project_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn valid_version(v: &str) -> bool {
    !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn shell_argv(cmd: &str, login: bool) -> Vec<String> {
    let flag = if login { "-lc" } else { "-c" };
    vec!["bash".into(), flag.into(), cmd.to_string()]
}

impl NewProject {
    /// Validate and turn the request into a config entry.
    fn into_project(self) -> Result<(String, Project)> {
        let id = self.id.trim().to_string();
        if !valid_project_id(&id) {
            bail!("project id must be 1-64 chars of letters, digits, `_` or `-`, got `{id}`");
        }
        let root = self.root.trim().to_string();
        if root.is_empty() {
            bail!("root is required");
        }
        let start = self.start.trim().to_string();
        if start.is_empty() {
            bail!("start command is required");
        }
        let port = self.port.unwrap_or(DEFAULT_ADD_PORT);
        if port == 0 {
            bail!("port must be non-zero");
        }
        let node = non_empty(&self.node);
        let java = non_empty(&self.java);
        for (label, v) in [("node", &node), ("java", &java)] {
            if let Some(v) = v {
                if !valid_version(v) {
                    bail!("{label} version `{v}` may only contain letters, digits, `.`, `-`, `_`");
                }
            }
        }
        // A login shell would re-run fnm / sdkman init and undo the version switch.
        let login = node.is_none() && java.is_none();
        let name = non_empty(&self.name).unwrap_or_else(|| id.clone());
        let project = Project {
            name,
            root,
            enabled: true,
            reason: String::new(),
            start: shell_argv(&start, login),
            stop: non_empty(&self.stop)
                .map(|s| shell_argv(&s, login))
                .unwrap_or_default(),
            tomcat: String::new(),
            log_dir: String::new(),
            log_file: non_empty(&self.log_file).unwrap_or_default(),
            pid_file: String::new(),
            port,
            login_url: non_empty(&self.login_url).unwrap_or_default(),
            health_url: non_empty(&self.health_url).unwrap_or_default(),
            background: self.background.unwrap_or(true),
            node,
            java,
        };
        Ok((id, project))
    }
}

fn project_table(p: &Project) -> toml_edit::Table {
    use toml_edit::{value, Array, Table};
    let mut t = Table::new();
    t["name"] = value(p.name.as_str());
    t["root"] = value(p.root.as_str());
    t["port"] = value(i64::from(p.port));
    if p.background {
        t["background"] = value(true);
    }
    if let Some(v) = &p.node {
        t["node"] = value(v.as_str());
    }
    if let Some(v) = &p.java {
        t["java"] = value(v.as_str());
    }
    let arr = |argv: &[String]| {
        let mut a = Array::new();
        for s in argv {
            a.push(s.as_str());
        }
        a
    };
    t["start"] = value(arr(&p.start));
    if !p.stop.is_empty() {
        t["stop"] = value(arr(&p.stop));
    }
    for (key, v) in [
        ("login_url", &p.login_url),
        ("health_url", &p.health_url),
        ("log_file", &p.log_file),
    ] {
        if !v.is_empty() {
            t[key] = value(v.as_str());
        }
    }
    t
}

fn read_doc(path: &Path) -> Result<toml_edit::DocumentMut> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    text.parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parse {}", path.display()))
}

fn write_doc(path: &Path, doc: &toml_edit::DocumentMut) -> Result<()> {
    let text = doc.to_string();
    // Refuse to write something the loader cannot read back.
    parse_config(&text)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(())
}

impl Loaded {
    /// Register a project and write it to the config file. Existing ids are refused.
    pub fn add_project(&mut self, req: NewProject) -> Result<ProjectRow> {
        let (id, project) = req.into_project()?;
        if self.config.projects.contains_key(&id) {
            bail!("project `{id}` already exists; remove it first or pick another id");
        }
        // Same validation as loading: absolute root, templates resolve, etc.
        resolve_project(&id, &project, current_home().as_deref())?;
        let mut doc = read_doc(&self.path)?;
        let projects = doc
            .entry("projects")
            .or_insert_with(|| {
                let mut t = toml_edit::Table::new();
                t.set_implicit(true);
                toml_edit::Item::Table(t)
            })
            .as_table_mut()
            .context("`projects` in config is not a table")?;
        if projects.contains_key(&id) {
            bail!("project `{id}` already exists in {}", self.path.display());
        }
        projects.insert(&id, toml_edit::Item::Table(project_table(&project)));
        write_doc(&self.path, &doc)?;
        self.config.projects.insert(id.clone(), project);
        let r = self.resolve(&id)?;
        Ok(ProjectRow {
            id: r.id,
            name: r.name,
            root: path_to_string(&r.root),
            enabled: r.enabled,
            port: r.port,
            reason: r.reason,
            node: r.node,
            java: r.java,
        })
    }

    /// Remove a project from the config file. Does not touch the project's files.
    pub fn remove_project(&mut self, id: &str) -> Result<()> {
        if !self.config.projects.contains_key(id) {
            bail!("unknown project id `{id}`; try `tongyun-dev list`");
        }
        let mut doc = read_doc(&self.path)?;
        let removed = doc
            .get_mut("projects")
            .and_then(|p| p.as_table_mut())
            .and_then(|t| t.remove(id));
        if removed.is_none() {
            bail!("project `{id}` not found in {}", self.path.display());
        }
        write_doc(&self.path, &doc)?;
        self.config.projects.remove(id);
        Ok(())
    }

    /// Page to open in the browser: `login_url`, else `health_url`, else `http://127.0.0.1:<port>/`.
    pub fn page_url(&self, id: &str) -> Result<String> {
        let p = self.resolve(id)?;
        if !p.login_url.is_empty() {
            return Ok(p.login_url);
        }
        if p.port == 0 {
            bail!("project `{id}` has no port or page url");
        }
        Ok(format!("http://127.0.0.1:{}/", p.port))
    }

    /// Absolute project root.
    pub fn root_dir(&self, id: &str) -> Result<PathBuf> {
        Ok(self.resolve(id)?.root)
    }
}

// ---------------------------------------------------------------------------
// Openers shared by the CLI, the web console and the desktop shell.
// ---------------------------------------------------------------------------

fn open_with_system(target: &str) -> Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(target);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", target]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .with_context(|| format!("cannot open {target}"))
}

/// Open an http(s) URL in the default browser.
pub fn open_url(url: &str) -> Result<()> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("only http(s) URLs can be opened");
    }
    if url
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'' || c == '&' && cfg!(windows))
    {
        bail!("url is not usable: {url}");
    }
    open_with_system(url)
}

/// Open an existing absolute file or directory with the system default program.
pub fn open_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("path must be absolute: {}", path.display());
    }
    if !path.exists() {
        bail!("path does not exist: {}", path.display());
    }
    open_with_system(&path_to_string(path))
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
            p.tomcat.as_deref(),
            Some(Path::new("/Users/dev/opt/tomcat-9"))
        );
        assert!(p.enabled);
        assert!(!p.background);
        assert!(p.reason.is_empty());
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
        let tomcat = p.tomcat.as_ref().unwrap();
        assert!(env.contains(&("CATALINA_HOME".into(), path_to_string(tomcat))));
        assert!(env.contains(&("CATALINA_BASE".into(), path_to_string(tomcat))));
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
            port_conflict: false,
            port_holder: String::new(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        for key in [
            "running",
            "pid",
            "port",
            "login_probe",
            "log_path",
            "port_conflict",
            "port_holder",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["port_conflict"], false);
        assert_eq!(v["port_holder"], "");
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

    #[test]
    fn listener_belongs_to_our_process_tree() {
        fn parent(pid: u32) -> Option<u32> {
            match pid {
                10 => Some(5),
                5 => Some(2),
                2 => Some(1),
                _ => None,
            }
        }
        assert!(belongs_to_with(10, &[2], parent));
        assert!(belongs_to_with(2, &[2], parent));
        assert!(!belongs_to_with(10, &[9], parent));
        assert!(!belongs_to_with(10, &[], parent));
        assert!(!belongs_to_with(2, &[10], parent));
    }

    #[test]
    fn enabled_defaults_and_log_paths() {
        let cfg: FileConfig = toml::from_str(
            r#"
[projects.demo]
name = "demo"
root = "/tmp/demo"
start = ["true"]
port = 9
"#,
        )
        .unwrap();
        let p = resolve_project(
            "demo",
            cfg.projects.get("demo").unwrap(),
            Some(Path::new("/Users/dev")),
        )
        .unwrap();
        assert!(p.enabled);
        assert!(!p.background);
        assert!(p.tomcat.is_none());
        assert!(p.login_url.is_empty());
        assert_eq!(
            p.log_file,
            PathBuf::from("/Users/dev/.tongyun-dev/logs/demo.out")
        );
        assert_eq!(
            p.pid_file,
            PathBuf::from("/Users/dev/.tongyun-dev/logs/demo.pid")
        );
    }

    #[test]
    fn registered_lines_resolve() {
        let home = Path::new("/Users/dev");
        let cfg = parse_config(PROJECTS_TOML).unwrap();
        let enabled = [
            "legacy_app",
            "demo_exam",
            "demo_exam_vue",
            "demo_exam_student_vue",
            "ap_docs-site",
            "ap_docs-site_web",
            "ap_admin_test_center",
            "ops_class_admin_manager",
            "ops_class_admin_web",
        ];
        let disabled = [
            "ap_admin_test_center_vue",
            "acme_system_ap_java_backend",
            "opsstudentmanageserver",
            "ops_student_manage_vue",
            "papers_cn",
            "acme_papers_server",
            "tdmockserver",
            "acme_wechat",
            "acme_shop_center",
            "acme_wechat_friend_manager",
            "wxcenter",
            "admin_acme_cn",
            "acme_web_viewer",
            "offlinestudent",
            "g5_admin_vue",
        ];
        assert_eq!(cfg.projects.len(), enabled.len() + disabled.len());
        for id in enabled {
            let p = resolve_project(id, cfg.projects.get(id).unwrap(), Some(home)).unwrap();
            assert!(p.enabled, "{id}");
            assert!(!p.start.is_empty(), "{id}");
            assert_ne!(p.port, 0, "{id}");
            assert!(p.root.is_absolute(), "{id}");
            assert!(!path_to_string(&p.root).contains('~'), "{id}");
            assert!(!path_to_string(&p.log_file).contains("{{"), "{id}");
        }
        for id in disabled {
            let p = resolve_project(id, cfg.projects.get(id).unwrap(), Some(home)).unwrap();
            assert!(!p.enabled, "{id}");
            assert!(!p.reason.is_empty(), "{id}");
            let err = cmd_start(&p).unwrap_err().to_string();
            assert!(err.contains("disabled"), "{err}");
            assert!(err.contains(&p.reason), "{err}");
        }

        let be = resolve_project(
            "demo_exam",
            cfg.projects.get("demo_exam").unwrap(),
            Some(home),
        )
        .unwrap();
        assert!(be.background);
        assert!(be.tomcat.is_none());
        assert_eq!(be.port, 8034);
        assert!(be.login_url.is_empty());
        let start = be.start.join(" ");
        assert!(start.contains("sdkman-init.sh"), "{start}");
        assert!(start.contains("spring-boot:run"), "{start}");
        assert!(start.contains("profiles=dev"), "{start}");
        assert_eq!(
            be.root,
            PathBuf::from("/Users/dev/code/acme/01-exam/demo_exam")
        );
        assert_eq!(
            be.log_file,
            PathBuf::from("/Users/dev/.tongyun-dev/logs/demo_exam.out")
        );

        let vue = resolve_project(
            "demo_exam_vue",
            cfg.projects.get("demo_exam_vue").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(vue.port, 5777);
        assert!(vue.background);
        let vue_start = vue.start.join(" ");
        assert!(vue_start.contains("fnm"), "{vue_start}");
        assert!(vue_start.contains("5777"), "{vue_start}");
        assert_eq!(vue.login_url, "http://127.0.0.1:5777/");

        let student = resolve_project(
            "demo_exam_student_vue",
            cfg.projects.get("demo_exam_student_vue").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(student.port, 5999);
        assert_eq!(student.login_url, "http://127.0.0.1:5999/");
        assert_eq!(
            student.root,
            PathBuf::from("/Users/dev/code/acme/01-exam/demo-exam-student-vue")
        );

        let paper = resolve_project(
            "ap_docs-site",
            cfg.projects.get("ap_docs-site").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(paper.port, 8888);
        assert!(paper.background);
        assert_eq!(
            paper.root,
            PathBuf::from("/Users/dev/code/acme/02-ap/acme-ap-docs-site-backend")
        );
        let web = resolve_project(
            "ap_docs-site_web",
            cfg.projects.get("ap_docs-site_web").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(web.port, 7888);
        assert_eq!(web.login_url, "http://127.0.0.1:7888/");
        assert_eq!(
            web.root,
            PathBuf::from("/Users/dev/code/acme/02-ap/acme-ap-docs-site-react")
        );

        let center = resolve_project(
            "ap_admin_test_center",
            cfg.projects.get("ap_admin_test_center").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(center.port, 8110);
        assert_eq!(
            center.root,
            PathBuf::from("/Users/dev/code/acme/02-ap/acme-ap-admin-test-center")
        );
        let center_vue = resolve_project(
            "ap_admin_test_center_vue",
            cfg.projects.get("ap_admin_test_center_vue").unwrap(),
            Some(home),
        )
        .unwrap();
        assert!(!center_vue.enabled);
        assert!(center_vue.reason.contains("admin-test-center-vue"));

        let ops = resolve_project(
            "ops_class_admin_manager",
            cfg.projects.get("ops_class_admin_manager").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(ops.port, 8147);
        assert_eq!(
            ops.root,
            PathBuf::from(
                "/Users/dev/code/acme/03-ops-class/ops-class-admin/ops_class_admin_manager"
            )
        );
        let ops_web = resolve_project(
            "ops_class_admin_web",
            cfg.projects.get("ops_class_admin_web").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(ops_web.port, 8146);
        assert!(ops_web.start.join(" ").contains("fnm"));
        assert_eq!(
            ops_web.root,
            PathBuf::from(
                "/Users/dev/code/acme/03-ops-class/ops-class-admin/ops_class_admin_web"
            )
        );

        let ap_blocked = cfg
            .projects
            .get("acme_system_ap_java_backend")
            .unwrap();
        assert!(!ap_blocked.enabled);
        assert!(ap_blocked.reason.to_ascii_lowercase().contains("maven"));
        let ops_blocked = cfg.projects.get("opsstudentmanageserver").unwrap();
        assert!(!ops_blocked.enabled);
        assert!(ops_blocked.reason.contains("acme-job-sdk"));
    }

    #[test]
    fn log_tail_reads_last_lines() {
        let dir = std::env::temp_dir().join(format!("tongyun-dev-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy_app_catalina.out");
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        let text = read_tail(&path, 2).unwrap();
        assert_eq!(text, "c\nd\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_and_resolves_node_and_java_toolchain() {
        let toml_str = r#"
[projects.fullstack]
name = "fullstack"
root = "/tmp/fullstack"
node = "20"
java = "17"
start = ["pnpm", "dev"]
port = 3000

[projects.none]
name = "none"
root = "/tmp/none"
start = ["true"]
port = 3001

[projects.whitespace]
name = "whitespace"
root = "/tmp/whitespace"
node = "  "
java = ""
start = ["true"]
port = 3002
"#;
        let cfg: FileConfig = parse_config(toml_str).unwrap();
        let home = Path::new("/Users/dev");

        let fs = resolve_project(
            "fullstack",
            cfg.projects.get("fullstack").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(fs.node.as_deref(), Some("20"));
        assert_eq!(fs.java.as_deref(), Some("17"));

        let none = resolve_project("none", cfg.projects.get("none").unwrap(), Some(home)).unwrap();
        assert_eq!(none.node, None);
        assert_eq!(none.java, None);

        let ws = resolve_project(
            "whitespace",
            cfg.projects.get("whitespace").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(ws.node, None);
        assert_eq!(ws.java, None);

        // ProjectRow serialization check
        let row_fs = ProjectRow {
            id: fs.id.clone(),
            name: fs.name.clone(),
            root: path_to_string(&fs.root),
            enabled: fs.enabled,
            port: fs.port,
            reason: fs.reason.clone(),
            node: fs.node.clone(),
            java: fs.java.clone(),
        };
        let json_fs: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&row_fs).unwrap()).unwrap();
        assert_eq!(json_fs["node"], "20");
        assert_eq!(json_fs["java"], "17");

        let row_none = ProjectRow {
            id: none.id.clone(),
            name: none.name.clone(),
            root: path_to_string(&none.root),
            enabled: none.enabled,
            port: none.port,
            reason: none.reason.clone(),
            node: none.node.clone(),
            java: none.java.clone(),
        };
        let json_none: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&row_none).unwrap()).unwrap();
        assert!(json_none.get("node").is_none());
        assert!(json_none.get("java").is_none());
    }

    #[test]
    fn assemble_start_cmd_cases() {
        let base = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];

        // node only -> fnm exec --using=<v> --
        let with_node = assemble_start_cmd(&base, Some("20"), None);
        assert_eq!(
            with_node,
            vec!["fnm", "exec", "--using=20", "--", "npm", "run", "dev"]
        );

        // java only -> bash -lc 'sdk use java <v> && exec "$@"' --
        let with_java = assemble_start_cmd(&base, None, Some("17"));
        assert_eq!(
            with_java,
            vec![
                "bash",
                "-lc",
                "sdk use java 17 && exec \"$@\"",
                "--",
                "npm",
                "run",
                "dev"
            ]
        );

        // both -> sdkman wrapping fnm wrapping command
        let with_both = assemble_start_cmd(&base, Some("20"), Some("17"));
        assert_eq!(
            with_both,
            vec![
                "bash",
                "-lc",
                "sdk use java 17 && exec \"$@\"",
                "--",
                "fnm",
                "exec",
                "--using=20",
                "--",
                "npm",
                "run",
                "dev"
            ]
        );

        // neither -> unchanged
        let without_any = assemble_start_cmd(&base, None, None);
        assert_eq!(without_any, base);

        // empty base command
        let empty: Vec<String> = Vec::new();
        assert_eq!(
            assemble_start_cmd(&empty, Some("20"), Some("17")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn toolchain_missing_reports_readable_errors() {
        let toml_str = r#"
[projects.needs_fnm]
name = "needs_fnm"
root = "/tmp/needs_fnm"
node = "999"
start = ["pnpm", "dev"]
port = 9001

[projects.needs_sdkman]
name = "needs_sdkman"
root = "/tmp/needs_sdkman"
java = "999"
start = ["mvn", "spring-boot:run"]
port = 9002
"#;
        let cfg: FileConfig = parse_config(toml_str).unwrap();
        let p_fnm = resolve_project(
            "needs_fnm",
            cfg.projects.get("needs_fnm").unwrap(),
            Some(Path::new("/nonexistent")),
        )
        .unwrap();
        let p_sdk = resolve_project(
            "needs_sdkman",
            cfg.projects.get("needs_sdkman").unwrap(),
            Some(Path::new("/nonexistent")),
        )
        .unwrap();

        if Command::new("fnm").arg("--version").status().is_err() {
            let err =
                check_toolchain_availability(Some("999"), None, Some(Path::new("/nonexistent")))
                    .unwrap_err()
                    .to_string();
            assert!(err.contains("fnm"), "{err}");
            assert!(err.contains("999"), "{err}");

            let start_err = cmd_start(&p_fnm).unwrap_err().to_string();
            assert!(start_err.contains("fnm"), "{start_err}");
            assert!(start_err.contains("999"), "{start_err}");
        }

        let err_sdk = check_sdkman_installed("999", Some(Path::new("/nonexistent/sdkman")))
            .unwrap_err()
            .to_string();
        assert!(err_sdk.contains("sdkman"), "{err_sdk}");
        assert!(err_sdk.contains("999"), "{err_sdk}");

        let start_err_sdk = cmd_start(&p_sdk).unwrap_err().to_string();
        assert!(start_err_sdk.contains("sdkman"), "{start_err_sdk}");
        assert!(start_err_sdk.contains("999"), "{start_err_sdk}");
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    fn temp_config(tag: &str, body: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tongyun_dev_registry_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let path = dir.join("projects.toml");
        std::fs::write(&path, body.replace("@ROOT@", &path_to_string(&dir))).unwrap();
        path
    }

    const BASE: &str = r#"# keep this header comment
[projects.legacy_app]
name = "legacy_app"
root = "@ROOT@/app"
# keep this inline note
start = ["bash", "{{root}}/scripts/start-legacy.sh"]
port = 8080
"#;

    fn req(id: &str, root: &Path) -> NewProject {
        NewProject {
            id: id.into(),
            root: path_to_string(root),
            start: "pnpm dev".into(),
            port: Some(5173),
            ..Default::default()
        }
    }

    #[test]
    fn add_then_remove_keeps_comments_and_other_projects() {
        let path = temp_config("roundtrip", BASE);
        let root = path.parent().unwrap().join("app");
        let mut loaded = load(Some(&path)).unwrap();
        let mut r = req("web_app", &root);
        r.node = Some("22".into());
        r.java = Some("17.0.10-tem".into());
        let row = loaded.add_project(r).unwrap();
        assert_eq!(row.node.as_deref(), Some("22"));
        assert_eq!(row.java.as_deref(), Some("17.0.10-tem"));

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep this header comment"));
        assert!(text.contains("# keep this inline note"));
        assert!(text.contains("[projects.web_app]"));

        // Reload from disk: toolchain fields and background survive.
        let reloaded = load(Some(&path)).unwrap();
        let p = reloaded.config.projects.get("web_app").unwrap();
        assert!(p.background);
        assert_eq!(p.node.as_deref(), Some("22"));
        assert_eq!(p.java.as_deref(), Some("17.0.10-tem"));
        // Toolchain declared -> non-login shell so fnm / sdkman are not reset.
        assert_eq!(p.start, vec!["bash", "-c", "pnpm dev"]);
        let resolved = reloaded.resolve("web_app").unwrap();
        assert_eq!(
            resolved.assembled_start()[..4],
            [
                "bash",
                "-lc",
                "sdk use java 17.0.10-tem && exec \"$@\"",
                "--"
            ]
        );

        let mut loaded = reloaded;
        loaded.remove_project("web_app").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("web_app"));
        assert!(text.contains("# keep this header comment"));
        assert!(load(Some(&path))
            .unwrap()
            .config
            .projects
            .contains_key("legacy_app"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn add_rejects_duplicates_and_bad_input() {
        let path = temp_config("reject", BASE);
        let root = path.parent().unwrap().join("app");
        let mut loaded = load(Some(&path)).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        assert!(loaded.add_project(req("legacy_app", &root)).is_err());
        assert!(loaded.add_project(req("bad id", &root)).is_err());
        assert!(loaded.add_project(req("x/../y", &root)).is_err());
        let mut r = req("relroot", &root);
        r.root = "relative/path".into();
        assert!(loaded.add_project(r).is_err());
        let mut r = req("badnode", &root);
        r.node = Some("22; rm -rf /".into());
        assert!(loaded.add_project(r).is_err());
        let mut r = req("zeroport", &root);
        r.port = Some(0);
        assert!(loaded.add_project(r).is_err());
        assert!(loaded.remove_project("nope").is_err());

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn plain_add_uses_login_shell_and_port_page() {
        let path = temp_config("plain", BASE);
        let root = path.parent().unwrap().join("app");
        let mut loaded = load(Some(&path)).unwrap();
        let mut r = req("plain", &root);
        r.stop = Some("pkill -f vite".into());
        loaded.add_project(r).unwrap();
        let p = loaded.config.projects.get("plain").unwrap();
        assert_eq!(p.start, vec!["bash", "-lc", "pnpm dev"]);
        assert_eq!(p.stop, vec!["bash", "-lc", "pkill -f vite"]);
        assert_eq!(loaded.page_url("plain").unwrap(), "http://127.0.0.1:5173/");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn openers_validate_input() {
        assert!(open_url("").is_err());
        assert!(open_url("   ").is_err());
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("http://a b").is_err());
        assert!(open_path(Path::new("relative")).is_err());
        assert!(open_path(Path::new("/definitely/not/here/tongyun")).is_err());
    }
}
