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
        "~/code/_index/logs".to_string()
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

pub fn assemble_start_cmd(
    start: &[String],
    node: Option<&str>,
    java: Option<&str>,
) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECTS_TOML: &str = include_str!("../../../config/projects.toml");

    fn td_work(home: &Path) -> Resolved {
        let cfg = parse_config(PROJECTS_TOML).unwrap();
        resolve_project("td_work", cfg.projects.get("td_work").unwrap(), Some(home)).unwrap()
    }

    #[test]
    fn expands_tilde_and_home_forms() {
        let home = Path::new("/Users/dev");
        assert_eq!(
            expand_tilde_with("~/code/testdaily/tdprojects", Some(home)).unwrap(),
            "/Users/dev/code/testdaily/tdprojects"
        );
        assert_eq!(
            expand_tilde_with("$HOME/code/_index/logs/td_work_catalina.out", Some(home)).unwrap(),
            "/Users/dev/code/_index/logs/td_work_catalina.out"
        );
        assert_eq!(
            expand_tilde_with("${HOME}/opt/apache-tomcat-9.0.98", Some(home)).unwrap(),
            "/Users/dev/opt/apache-tomcat-9.0.98"
        );
        let err = expand_tilde_with("~/code/testdaily/tdprojects", None).unwrap_err();
        assert!(err.to_string().contains("HOME is not set"));
    }

    #[test]
    fn td_work_paths_resolve_without_tilde() {
        let p = td_work(Path::new("/Users/dev"));
        assert_eq!(
            p.root,
            PathBuf::from("/Users/dev/code/testdaily/tdprojects")
        );
        assert_eq!(
            p.tomcat.as_deref(),
            Some(Path::new("/Users/dev/opt/apache-tomcat-9.0.98"))
        );
        assert!(p.enabled);
        assert!(!p.background);
        assert!(p.reason.is_empty());
        assert_eq!(
            p.log_file,
            PathBuf::from("/Users/dev/code/_index/logs/td_work_catalina.out")
        );
        assert_eq!(
            p.pid_file,
            PathBuf::from("/Users/dev/code/_index/logs/td_work_tomcat.pid")
        );
        assert_eq!(
            p.start,
            vec![
                "bash".to_string(),
                "/Users/dev/code/testdaily/tdprojects/scripts/start-td-work.sh".to_string(),
            ]
        );
        assert_eq!(
            p.stop,
            vec![
                "bash".to_string(),
                "/Users/dev/opt/apache-tomcat-9.0.98/bin/catalina.sh".to_string(),
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
            "http://127.0.0.1:8080/td_work/admin/login.html"
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
            id: "td_work".into(),
            name: "td_work".into(),
            running: false,
            pid: None,
            port: 8080,
            port_open: false,
            login_probe: LoginProbe {
                url: "http://127.0.0.1:8080/td_work/admin/login.html".into(),
                http_code: None,
                ok: false,
            },
            log_path: "/Users/dev/code/_index/logs/td_work_catalina.out".into(),
            pid_path: "/Users/dev/code/_index/logs/td_work_tomcat.pid".into(),
            root: "/Users/dev/code/testdaily/tdprojects".into(),
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
            "/Users/dev/code/_index/logs/td_work_catalina.out"
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
            probe_addr("http://127.0.0.1:8080/td_work/admin/login.html"),
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
            PathBuf::from("/Users/dev/code/_index/logs/demo.out")
        );
        assert_eq!(
            p.pid_file,
            PathBuf::from("/Users/dev/code/_index/logs/demo.pid")
        );
    }

    #[test]
    fn registered_lines_resolve() {
        let home = Path::new("/Users/dev");
        let cfg = parse_config(PROJECTS_TOML).unwrap();
        let enabled = [
            "td_work",
            "td_new_toefl",
            "td_new_toefl_vue",
            "td_new_toefl_student_vue",
            "ap_whitepaper",
            "ap_whitepaper_web",
            "ap_admin_test_center",
            "lj_class_admin_manager",
            "lj_class_admin_web",
        ];
        let disabled = [
            "ap_admin_test_center_vue",
            "testdaily_system_ap_java_backend",
            "ljstudentmanageserver",
            "lj_student_manage_vue",
            "pastpaper_cn",
            "td_pastpaper_server",
            "tdqumokaoserver",
            "td_wechat",
            "td_youzan_center",
            "td_wechat_friend_manager",
            "tdwxcenter",
            "admin_testdaily_cn",
            "td_web_viewer",
            "tdofflinestudent",
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
            "td_new_toefl",
            cfg.projects.get("td_new_toefl").unwrap(),
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
            PathBuf::from("/Users/dev/code/testdaily/01-new-toefl/td_new_toefl")
        );
        assert_eq!(
            be.log_file,
            PathBuf::from("/Users/dev/code/_index/logs/td_new_toefl.out")
        );

        let vue = resolve_project(
            "td_new_toefl_vue",
            cfg.projects.get("td_new_toefl_vue").unwrap(),
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
            "td_new_toefl_student_vue",
            cfg.projects.get("td_new_toefl_student_vue").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(student.port, 5999);
        assert_eq!(student.login_url, "http://127.0.0.1:5999/");
        assert_eq!(
            student.root,
            PathBuf::from("/Users/dev/code/testdaily/01-new-toefl/td-new-toefl-student-vue")
        );

        let paper = resolve_project(
            "ap_whitepaper",
            cfg.projects.get("ap_whitepaper").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(paper.port, 8888);
        assert!(paper.background);
        assert_eq!(
            paper.root,
            PathBuf::from("/Users/dev/code/testdaily/02-ap/td-ap-whitepaper-backend")
        );
        let web = resolve_project(
            "ap_whitepaper_web",
            cfg.projects.get("ap_whitepaper_web").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(web.port, 7888);
        assert_eq!(web.login_url, "http://127.0.0.1:7888/");
        assert_eq!(
            web.root,
            PathBuf::from("/Users/dev/code/testdaily/02-ap/td-ap-whitepaper-react")
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
            PathBuf::from("/Users/dev/code/testdaily/02-ap/td-ap-admin-test-center")
        );
        let center_vue = resolve_project(
            "ap_admin_test_center_vue",
            cfg.projects.get("ap_admin_test_center_vue").unwrap(),
            Some(home),
        )
        .unwrap();
        assert!(!center_vue.enabled);
        assert!(center_vue.reason.contains("admin-test-center-vue"));

        let lj = resolve_project(
            "lj_class_admin_manager",
            cfg.projects.get("lj_class_admin_manager").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(lj.port, 8147);
        assert_eq!(
            lj.root,
            PathBuf::from(
                "/Users/dev/code/testdaily/03-lj-class/lj-class-admin/lj_class_admin_manager"
            )
        );
        let lj_web = resolve_project(
            "lj_class_admin_web",
            cfg.projects.get("lj_class_admin_web").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(lj_web.port, 8146);
        assert!(lj_web.start.join(" ").contains("fnm"));
        assert_eq!(
            lj_web.root,
            PathBuf::from(
                "/Users/dev/code/testdaily/03-lj-class/lj-class-admin/lj_class_admin_web"
            )
        );

        let ap_blocked = cfg
            .projects
            .get("testdaily_system_ap_java_backend")
            .unwrap();
        assert!(!ap_blocked.enabled);
        assert!(ap_blocked.reason.to_ascii_lowercase().contains("maven"));
        let lj_blocked = cfg.projects.get("ljstudentmanageserver").unwrap();
        assert!(!lj_blocked.enabled);
        assert!(lj_blocked.reason.contains("td-job-sdk"));
    }

    #[test]
    fn log_tail_reads_last_lines() {
        let dir = std::env::temp_dir().join(format!("tongyun-dev-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("td_work_catalina.out");
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

        let fs = resolve_project("fullstack", cfg.projects.get("fullstack").unwrap(), Some(home)).unwrap();
        assert_eq!(fs.node.as_deref(), Some("20"));
        assert_eq!(fs.java.as_deref(), Some("17"));

        let none = resolve_project("none", cfg.projects.get("none").unwrap(), Some(home)).unwrap();
        assert_eq!(none.node, None);
        assert_eq!(none.java, None);

        let ws = resolve_project("whitespace", cfg.projects.get("whitespace").unwrap(), Some(home)).unwrap();
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
        let json_fs: serde_json::Value = serde_json::from_str(&serde_json::to_string(&row_fs).unwrap()).unwrap();
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
        let json_none: serde_json::Value = serde_json::from_str(&serde_json::to_string(&row_none).unwrap()).unwrap();
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
        assert_eq!(assemble_start_cmd(&empty, Some("20"), Some("17")), Vec::<String>::new());
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
        let p_fnm = resolve_project("needs_fnm", cfg.projects.get("needs_fnm").unwrap(), Some(Path::new("/nonexistent"))).unwrap();
        let p_sdk = resolve_project("needs_sdkman", cfg.projects.get("needs_sdkman").unwrap(), Some(Path::new("/nonexistent"))).unwrap();

        if Command::new("fnm").arg("--version").status().is_err() {
            let err = check_toolchain_availability(Some("999"), None, Some(Path::new("/nonexistent"))).unwrap_err().to_string();
            assert!(err.contains("fnm"), "{err}");
            assert!(err.contains("999"), "{err}");

            let start_err = cmd_start(&p_fnm).unwrap_err().to_string();
            assert!(start_err.contains("fnm"), "{start_err}");
            assert!(start_err.contains("999"), "{start_err}");
        }

        let err_sdk = check_sdkman_installed("999", Some(Path::new("/nonexistent/sdkman"))).unwrap_err().to_string();
        assert!(err_sdk.contains("sdkman"), "{err_sdk}");
        assert!(err_sdk.contains("999"), "{err_sdk}");

        let start_err_sdk = cmd_start(&p_sdk).unwrap_err().to_string();
        assert!(start_err_sdk.contains("sdkman"), "{start_err_sdk}");
        assert!(start_err_sdk.contains("999"), "{start_err_sdk}");
    }
}
