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

pub mod scan;
pub mod toolchain;

pub use scan::{Candidate, Confidence, ScanReport, DEFAULT_SCAN_DEPTH};
pub use toolchain::{
    format_home, resolve_java_home, resolve_node_bin, shell_argv, shell_argv_with_os,
    toolchain_env, HostOs, ToolchainError, ToolchainRoots,
};

const START_READY_DEFAULT_SECS: u64 = 180;
/// 有启动标记、进程却找不到时，先等这么多秒再判定「进程已退出」。
const START_EXIT_GRACE_SECS: u64 = 3;
/// Where logs / pid files go when a project sets no `log_dir`.
pub const DEFAULT_LOG_DIR: &str = "~/.tongyun-dev/logs";
const LOG_TAIL_MAX_LINES: usize = 500;
const LOG_TAIL_WINDOW: u64 = 512 * 1024;

#[derive(Debug)]
pub struct Loaded {
    path: PathBuf,
    config: FileConfig,
}

#[derive(Debug, serde::Deserialize)]
struct FileConfig {
    #[serde(default)]
    projects: HashMap<String, Project>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Project {
    #[serde(default)]
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
    start_windows: Vec<String>,
    #[serde(default)]
    stop_windows: Vec<String>,
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
    pub start_timeout_secs: Option<u64>,
    #[serde(default)]
    pub stop_grace_secs: Option<u64>,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    java: Option<String>,
    #[serde(default)]
    java_home: Option<String>,
    #[serde(default)]
    node_home: Option<String>,
    #[serde(default)]
    shell: Option<String>,
    /// 缺 node_modules 时启动前自动装依赖。缺省 true。
    #[serde(default)]
    auto_install: Option<bool>,
}

struct Vars {
    root: String,
    tomcat: String,
    log_dir: String,
}

#[derive(Debug, Clone)]
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
    start_timeout_secs: Option<u64>,
    stop_grace_secs: Option<u64>,
    start: Vec<String>,
    stop: Vec<String>,
    start_windows: Vec<String>,
    stop_windows: Vec<String>,
    node: Option<String>,
    java: Option<String>,
    java_home: Option<PathBuf>,
    node_home: Option<PathBuf>,
    shell: Option<String>,
    auto_install: bool,
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

    pub fn active_start(&self) -> Vec<String> {
        if cfg!(windows) && !self.start_windows.is_empty() {
            return self.start_windows.clone();
        }
        self.start.clone()
    }

    pub fn active_stop(&self) -> Vec<String> {
        if cfg!(windows) && !self.stop_windows.is_empty() {
            return self.stop_windows.clone();
        }
        self.stop.clone()
    }

    pub fn assembled_start(&self) -> Vec<String> {
        // 旧配置里常见 `pnpm dev -- --port X`：pnpm 会把 `--` 原样交给 vite / vue-cli，端口参数失效。
        self.active_start()
            .iter()
            .map(|a| normalize_start_arg(a))
            .collect()
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
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_grace_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_home: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_home: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// 缺 node_modules 时启动前是否自动装依赖（缺省 true）。
    pub auto_install: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub config_path: String,
    pub projects: Vec<ProjectRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifeState {
    Stopped,
    /// 正在装前端依赖（pnpm / yarn / npm install）。
    Installing,
    Starting,
    PortOpen,
    Ready,
    Failed,
}

impl std::fmt::Display for LifeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifeState::Stopped => write!(f, "stopped"),
            LifeState::Installing => write!(f, "installing"),
            LifeState::Starting => write!(f, "starting"),
            LifeState::PortOpen => write!(f, "port_open"),
            LifeState::Ready => write!(f, "ready"),
            LifeState::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct StartMarker {
    pub started_at: u64,
    pub timeout_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateInput {
    pub process_alive: bool,
    pub port_open: bool,
    pub port_conflict: bool,
    pub has_probe: bool,
    pub probe_ok: bool,
    pub started_at: Option<u64>,
    pub now: u64,
    pub timeout_secs: u64,
    pub start_error: Option<String>,
}

pub fn derive_state(input: StateInput) -> (LifeState, Option<String>) {
    // 启动命令报错
    if let Some(err) = input.start_error {
        return (LifeState::Failed, Some(err));
    }

    // 端口被别的程序占着（现有 port_conflict=true）不算本项目 Ready/PortOpen
    let effective_port_open = input.port_open && !input.port_conflict;

    // 端口通 + 探针 ok（或没配探针）→ Ready
    if effective_port_open && (!input.has_probe || input.probe_ok) {
        return (LifeState::Ready, None);
    }

    // 未就绪情况：
    // 有启动标记 + 未就绪 + （进程已退出 或 已超时）→ Failed
    if let Some(started_at) = input.started_at {
        let elapsed = input.now.saturating_sub(started_at);
        // catalina.sh start 返回后 JVM 写 pid 文件要一点时间：头几秒进程没找到不算失败
        if !input.process_alive && elapsed >= START_EXIT_GRACE_SECS {
            return (LifeState::Failed, Some("进程已退出".to_string()));
        }
        if elapsed >= input.timeout_secs {
            return (
                LifeState::Failed,
                Some(format!("超过 {} 秒仍未就绪", input.timeout_secs)),
            );
        }
        // 端口通 + 配了探针但没 ok → PortOpen
        if effective_port_open {
            return (LifeState::PortOpen, None);
        }
        // 端口不通 + 进程活着 + 未超时 → Starting
        return (LifeState::Starting, None);
    }

    // 无启动标记的情况（例如在终端里手动起的）
    if effective_port_open && input.has_probe && !input.probe_ok {
        return (LifeState::PortOpen, None);
    }
    if input.process_alive && !effective_port_open {
        // 进程在但端口没通：算启动中，不算 running
        return (LifeState::Starting, None);
    }

    // 其它 → Stopped
    (LifeState::Stopped, None)
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct StatusJson {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub state: LifeState,
    pub elapsed_secs: Option<u64>,
    pub timeout_secs: u64,
    pub failure: Option<String>,
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
    /// 根目录有 package.json 但没有 node_modules。
    #[serde(default)]
    pub deps_missing: bool,
    /// 装依赖用的命令（例如 `pnpm install`）；不是前端项目时为空。
    #[serde(default)]
    pub install_command: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
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
            let configured = resolved.port > 0
                && (!resolved.active_start().is_empty() || resolved.tomcat.is_some());
            projects.push(ProjectRow {
                id: resolved.id,
                name: resolved.name,
                root: path_to_string(&resolved.root),
                enabled: resolved.enabled,
                port: resolved.port,
                reason: resolved.reason,
                configured,
                start_timeout_secs: resolved.start_timeout_secs,
                stop_grace_secs: resolved.stop_grace_secs,
                node: resolved.node,
                java: resolved.java,
                java_home: resolved.java_home.as_ref().map(|p| path_to_string(p)),
                node_home: resolved.node_home.as_ref().map(|p| path_to_string(p)),
                shell: resolved.shell,
                auto_install: resolved.auto_install,
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

    pub fn start_detached(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        if !project.enabled {
            let why = if project.reason.is_empty() {
                "disabled"
            } else {
                project.reason.as_str()
            };
            bail!("project `{}` is disabled: {why}", project.id);
        }
        // Do not reinstall dependencies or replace markers for an existing process.
        if !tracked_pids(&project).is_empty() {
            return self.status(id);
        }
        let current = self.status(id)?;
        if current.port_conflict {
            bail!(
                "端口 {} 被其他进程占用：{}",
                current.port,
                current.port_holder
            );
        }
        // 正在装依赖：装完会自己接着启动，不重复起。
        if install_in_progress(&project.pid_file) {
            return self.status(id);
        }
        if deps_missing(&project.root) && !(project.background && is_ready(&project)) {
            if project.auto_install {
                let _ = std::fs::remove_file(start_marker_path(&project.pid_file));
                begin_install(&project, true);
                return self.status(id);
            }
            let _ = clear_install_marker(&project.pid_file);
            let marker = StartMarker {
                started_at: current_time_secs(),
                timeout_secs: start_ready_timeout_secs(&project),
                error: Some(DEPS_MISSING_MSG.to_string()),
            };
            let _ = write_start_marker(&project.pid_file, &marker);
            return self.status(id);
        }
        let _ = clear_install_marker(&project.pid_file);
        if let Err(e) = cmd_start_detached(&project) {
            let err_msg = format!("{e:#}");
            let marker = StartMarker {
                started_at: current_time_secs(),
                timeout_secs: start_ready_timeout_secs(&project),
                error: Some(err_msg.clone()),
            };
            let _ = write_start_marker(&project.pid_file, &marker);
            let mut st = self.status(id)?;
            if st.state != LifeState::Failed {
                st.state = LifeState::Failed;
                st.failure = Some(err_msg);
            }
            return Ok(st);
        }
        self.status(id)
    }

    pub fn start(&self, id: &str) -> Result<StatusJson> {
        self.start_with_timeout(id, None)
    }

    /// The wait budget begins after the start command returns. It does not cancel the service.
    pub fn start_with_timeout(&self, id: &str, timeout: Option<Duration>) -> Result<StatusJson> {
        let initial = self.start_detached(id)?;
        let budget = timeout.unwrap_or_else(|| Duration::from_secs(initial.timeout_secs));
        self.finish_start(id, initial, budget)
    }

    fn finish_start(&self, id: &str, initial: StatusJson, budget: Duration) -> Result<StatusJson> {
        let result = self.wait_for_ready(id, initial, budget);
        let project = self.resolve(id)?;
        // Automatic installation is supervised by this process. Do not leave an
        // installer orphaned when a blocking CLI call exits after a timeout.
        if let Err(ref error) = result {
            let own_install = read_install_marker(&project.pid_file).is_some_and(|m| {
                m.supervisor == std::process::id() && m.then_start && m.error.is_none()
            });
            if own_install && install_in_progress(&project.pid_file) {
                cancel_install(&project)?;
                let message = format!("{error:#}；自动依赖安装已取消");
                write_start_marker(
                    &project.pid_file,
                    &StartMarker {
                        started_at: current_time_secs(),
                        timeout_secs: start_ready_timeout_secs(&project),
                        error: Some(message.clone()),
                    },
                )?;
                bail!("{message}");
            }
        }
        result
    }

    /// Observe an existing service without starting, stopping or resetting its startup timer.
    pub fn wait_ready(&self, id: &str, timeout: Duration) -> Result<StatusJson> {
        self.wait_for_ready(id, self.status(id)?, timeout)
    }

    fn wait_for_ready(
        &self,
        id: &str,
        mut st: StatusJson,
        timeout: Duration,
    ) -> Result<StatusJson> {
        let began = Instant::now();
        let mut last_state = None;
        loop {
            if st.port_conflict {
                bail!("端口 {} 被其他进程占用：{}", st.port, st.port_holder);
            }
            match st.state {
                LifeState::Ready => return Ok(st),
                LifeState::Failed => bail!("{}", st.failure.unwrap_or_else(|| "启动失败".into())),
                LifeState::Stopped => bail!("服务已停止"),
                _ => {}
            }
            if began.elapsed() >= timeout {
                bail!(
                    "等待就绪超过 {} 秒（当前 {}）；服务未被停止，请查看状态和日志",
                    timeout.as_secs(),
                    st.state
                );
            }
            if last_state != Some(st.state) {
                eprintln!("{}：{}（日志 {}）", st.id, st.state, st.log_path);
                last_state = Some(st.state);
            }
            thread::sleep(Duration::from_millis(200).min(timeout.saturating_sub(began.elapsed())));
            st = self.status(id)?;
        }
    }

    /// Restart only after a verified successful stop. GUI callers return while starting.
    pub fn restart_detached(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        if !project.enabled {
            bail!("project `{id}` is disabled: {}", project.reason);
        }
        self.stop(id)?;
        self.start_detached(id)
    }

    /// Restart and wait for readiness, as with `start`.
    pub fn restart(&self, id: &str) -> Result<StatusJson> {
        self.restart_with_timeout(id, None)
    }

    pub fn restart_with_timeout(&self, id: &str, timeout: Option<Duration>) -> Result<StatusJson> {
        let initial = self.restart_detached(id)?;
        let budget = timeout.unwrap_or_else(|| Duration::from_secs(initial.timeout_secs));
        self.finish_start(id, initial, budget)
    }

    pub fn stop(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        cancel_install(&project)?;
        cmd_stop(&project)?;
        let peers = self.peer_pids(&project.id);
        Ok(status_of(&project, &peers))
    }

    /// 后台装前端依赖（按锁文件选 pnpm / yarn / npm），日志追加到项目日志，立刻返回（状态 installing）。
    pub fn install_deps(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        if detect_package_manager(&project.root).is_none() {
            bail!(
                "不是前端项目：{} 下没有 package.json",
                project.root.display()
            );
        }
        if !install_in_progress(&project.pid_file) {
            begin_install(&project, false);
        }
        self.status(id)
    }

    /// CLI 用：装完才返回。失败时返回错误（原因同 App 里显示的）。
    pub fn install_deps_blocking(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        if detect_package_manager(&project.root).is_none() {
            bail!(
                "不是前端项目：{} 下没有 package.json",
                project.root.display()
            );
        }
        if install_in_progress(&project.pid_file) {
            bail!("{} 正在装依赖，稍后再试", project.id);
        }
        match spawn_install(&project, false) {
            Ok(handle) => {
                let _ = handle.join();
            }
            Err(e) => {
                write_install_failure(&project, &format!("{e:#}"));
            }
        }
        let st = self.status(id)?;
        if st.state == LifeState::Failed {
            bail!("{}", st.failure.unwrap_or_else(|| "依赖安装失败".into()));
        }
        Ok(st)
    }

    /// Last lines of the catalina log. `missing` is true when the file is not there yet.
    pub fn log_tail(&self, id: &str, lines: usize) -> Result<LogTail> {
        let project = self.resolve(id)?;
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

pub const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../../config/projects.template.toml");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ConfigInfo {
    pub path: Option<String>,
    pub source: String,
    pub default_path: String,
}

pub fn config_dir(home: Option<&Path>) -> PathBuf {
    match home {
        Some(h) => h.join(".config").join("tongyun-dev"),
        None => current_home()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("tongyun-dev"),
    }
}

pub fn default_config_path(home: Option<&Path>) -> PathBuf {
    config_dir(home).join("projects.toml")
}

pub fn create_default_config() -> Result<PathBuf> {
    create_default_config_with(None)
}

pub fn create_default_config_with(home: Option<&Path>) -> Result<PathBuf> {
    let target = default_config_path(home);
    if target.is_file() {
        return Ok(target);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config directory {}", parent.display()))?;
    }
    std::fs::write(&target, DEFAULT_CONFIG_TEMPLATE)
        .with_context(|| format!("write default config template to {}", target.display()))?;
    Ok(target)
}

pub fn remember_config_path(path: &Path) -> Result<()> {
    remember_config_path_with(path, None)
}

pub fn remember_config_path_with(path: &Path, home: Option<&Path>) -> Result<()> {
    let expanded = expand_config_arg_with(path, home).unwrap_or_else(|_| path.to_path_buf());
    let abs_path = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    };
    let text = std::fs::read_to_string(&abs_path)
        .with_context(|| format!("read config file {}", abs_path.display()))?;
    parse_config(&text)?;

    let dir = config_dir(home);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create config directory {}", dir.display()))?;
    let settings_path = dir.join("settings.json");

    let mut map: serde_json::Map<String, serde_json::Value> = if settings_path.is_file() {
        let content = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("read {}", settings_path.display()))?;
        let parsed: serde_json::Value = serde_json::from_str(&content)
            .with_context(|| format!("parse settings JSON from {}", settings_path.display()))?;
        parsed
            .as_object()
            .cloned()
            .with_context(|| format!("expected JSON object in {}", settings_path.display()))?
    } else {
        serde_json::Map::new()
    };

    map.insert(
        "config_path".to_string(),
        serde_json::Value::String(path_to_string(&abs_path)),
    );

    let json_bytes = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .context("serialize settings.json")?;
    std::fs::write(&settings_path, format!("{json_bytes}\n"))
        .with_context(|| format!("write {}", settings_path.display()))?;
    Ok(())
}

fn read_remembered_config(home: Option<&Path>) -> Option<PathBuf> {
    let settings_file = config_dir(home).join("settings.json");
    if !settings_file.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(&settings_file).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let path_str = value.get("config_path")?.as_str()?.trim();
    if path_str.is_empty() {
        return None;
    }
    let expanded = expand_tilde_with(path_str, home).ok()?;
    let path = PathBuf::from(expanded);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

fn resolve_config_source_with(
    explicit: Option<&Path>,
    env_value: Option<&str>,
    home: Option<&Path>,
    cwd: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Result<(PathBuf, &'static str)> {
    if let Some(p) = explicit {
        let expanded = expand_config_arg_with(p, home)?;
        return Ok((expanded, "arg"));
    }
    if let Some(env_str) = env_value.map(str::trim).filter(|s| !s.is_empty()) {
        let expanded = expand_config_arg_with(Path::new(env_str), home)?;
        return Ok((expanded, "env"));
    }
    if let Some(remembered) = read_remembered_config(home) {
        return Ok((remembered, "remembered"));
    }
    let user_default = default_config_path(home);
    if user_default.is_file() {
        return Ok((user_default, "user_default"));
    }
    if let Some(path) = cwd.and_then(|d| walk_for_config(d.to_path_buf())) {
        return Ok((path, "repo"));
    }
    if let Some(path) = exe_dir.and_then(|d| walk_for_config(d.to_path_buf())) {
        return Ok((path, "repo"));
    }

    let default_hint = path_to_string(&user_default);
    let mut searched = Vec::new();
    searched.push("--config explicit argument".to_string());
    searched.push("$TONGYUN_DEV_CONFIG environment variable".to_string());
    searched.push(format!(
        "{}/settings.json (remembered path)",
        path_to_string(&config_dir(home))
    ));
    searched.push(default_hint.clone());
    if let Some(d) = cwd {
        searched.push(format!(
            "{}/config/projects.toml (and ancestor directories)",
            path_to_string(d)
        ));
    }
    if let Some(d) = exe_dir {
        searched.push(format!(
            "{}/config/projects.toml (and ancestor directories)",
            path_to_string(d)
        ));
    }

    bail!(
        "未找到配置文件 projects.toml (not found).\n已查找候选位置 (searched candidate locations):\n  - {}\n提示：可在 `{default_hint}` 创建默认配置，或通过 --config 参数 / TONGYUN_DEV_CONFIG 环境变量指定。",
        searched.join("\n  - ")
    );
}

pub fn resolve_config_path_with(
    explicit: Option<&Path>,
    env_value: Option<&str>,
    home: Option<&Path>,
    cwd: Option<&Path>,
    exe_dir: Option<&Path>,
) -> Result<PathBuf> {
    resolve_config_source_with(explicit, env_value, home, cwd, exe_dir).map(|(p, _)| p)
}

pub fn resolve_config_path(explicit: Option<&Path>) -> Result<PathBuf> {
    let env_val = std::env::var("TONGYUN_DEV_CONFIG").ok();
    let home = current_home();
    let cwd = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    resolve_config_path_with(
        explicit,
        env_val.as_deref(),
        home.as_deref(),
        cwd.as_deref(),
        exe_dir.as_deref(),
    )
}

pub fn config_info() -> ConfigInfo {
    let env_val = std::env::var("TONGYUN_DEV_CONFIG").ok();
    let home = current_home();
    let cwd = std::env::current_dir().ok();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    config_info_with(
        None,
        env_val.as_deref(),
        home.as_deref(),
        cwd.as_deref(),
        exe_dir.as_deref(),
    )
}

pub fn config_info_with(
    explicit: Option<&Path>,
    env_value: Option<&str>,
    home: Option<&Path>,
    cwd: Option<&Path>,
    exe_dir: Option<&Path>,
) -> ConfigInfo {
    let default_path = path_to_string(&default_config_path(home));
    match resolve_config_source_with(explicit, env_value, home, cwd, exe_dir) {
        Ok((path, source)) => ConfigInfo {
            path: Some(path_to_string(&path)),
            source: source.to_string(),
            default_path,
        },
        Err(_) => ConfigInfo {
            path: None,
            source: "none".to_string(),
            default_path,
        },
    }
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

fn is_windows_absolute(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn expand_path(label: &str, raw: &str, home: Option<&Path>) -> Result<PathBuf> {
    let expanded = expand_tilde_with(raw, home)?;
    if expanded.contains("{{") {
        bail!("{label} has an unresolved template: {expanded}");
    }
    let path = PathBuf::from(&expanded);
    if !path.is_absolute() && !is_windows_absolute(&expanded) {
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
        DEFAULT_LOG_DIR.to_string()
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
    if p.enabled && p.start.is_empty() && p.start_windows.is_empty() {
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
    let start_windows = if p.start_windows.is_empty() {
        Vec::new()
    } else {
        render_argv(&p.start_windows, &vars, home)?
    };
    let stop_windows = if p.stop_windows.is_empty() {
        Vec::new()
    } else {
        render_argv(&p.stop_windows, &vars, home)?
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
    let java_home = if let Some(jh) = p
        .java_home
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(expand_path("java_home", jh, home)?)
    } else {
        None
    };
    let node_home = if let Some(nh) = p
        .node_home
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(expand_path("node_home", nh, home)?)
    } else {
        None
    };
    let shell = p
        .shell
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(Resolved {
        id: id.to_string(),
        name: if p.name.trim().is_empty() {
            id.to_string()
        } else {
            p.name.clone()
        },
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
        start_timeout_secs: p.start_timeout_secs.filter(|&t| t > 0),
        stop_grace_secs: p.stop_grace_secs.filter(|&g| g > 0),
        start,
        stop,
        start_windows,
        stop_windows,
        node,
        java,
        java_home,
        node_home,
        shell,
        auto_install: p.auto_install.unwrap_or(true),
    })
}

fn parse_config(text: &str) -> Result<FileConfig> {
    toml::from_str(text).context("parse projects.toml")
}

fn expand_config_arg_with(p: &Path, home: Option<&Path>) -> Result<PathBuf> {
    let s = p.to_str().context("config path is not utf-8")?;
    Ok(PathBuf::from(expand_tilde_with(s, home)?))
}

#[allow(dead_code)]
fn expand_config_arg(p: &Path) -> Result<PathBuf> {
    expand_config_arg_with(p, current_home().as_deref())
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

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn start_marker_path(pid_file: &Path) -> PathBuf {
    let mut p = pid_file.as_os_str().to_os_string();
    p.push(".start");
    PathBuf::from(p)
}

pub fn read_start_marker(pid_file: &Path) -> Option<StartMarker> {
    let path = start_marker_path(pid_file);
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_start_marker(pid_file: &Path, marker: &StartMarker) -> Result<()> {
    let path = start_marker_path(pid_file);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string(marker).context("serialize start marker")?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn status_of(p: &Resolved, peers: &[(String, u32)]) -> StatusJson {
    let own = tracked_pids(p);
    let open = port_open(p.port);
    let (port_conflict, port_holder) = port_conflict_of(p, &own, peers, open);
    let pid = own
        .into_iter()
        .next()
        .or_else(|| read_pid(&p.pid_file).filter(|&pid| process_alive(pid)));
    let process_alive = pid.is_some();
    let probe = if open && !port_conflict {
        login_probe(&p.login_url)
    } else {
        LoginProbe {
            url: p.login_url.clone(),
            http_code: None,
            ok: false,
        }
    };
    let has_probe = !p.login_url.is_empty();

    let marker = read_start_marker(&p.pid_file);
    let timeout_secs = marker
        .as_ref()
        .map(|m| m.timeout_secs)
        .unwrap_or_else(|| start_ready_timeout_secs(p));
    let started_at = marker.as_ref().map(|m| m.started_at);
    let start_error = marker.as_ref().and_then(|m| m.error.clone());
    let now = current_time_secs();

    let (state, failure) = derive_state(StateInput {
        process_alive,
        port_open: open,
        port_conflict,
        has_probe,
        probe_ok: probe.ok,
        started_at,
        now,
        timeout_secs,
        start_error,
    });

    if state == LifeState::Stopped && !process_alive {
        let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
    }

    let mut state = state;
    let mut failure = failure;
    let mut started_at = started_at;
    if !matches!(state, LifeState::Ready | LifeState::PortOpen) {
        if let Some((st, why, at)) = install_overlay(read_install_marker(&p.pid_file)) {
            state = st;
            failure = why;
            started_at = Some(at);
        }
    }

    let elapsed_secs = match state {
        LifeState::Installing
        | LifeState::Starting
        | LifeState::PortOpen
        | LifeState::Failed
        | LifeState::Ready => started_at.map(|st| now.saturating_sub(st)),
        LifeState::Stopped => None,
    };
    let running = matches!(state, LifeState::PortOpen | LifeState::Ready);

    StatusJson {
        id: p.id.clone(),
        name: p.name.clone(),
        running,
        state,
        elapsed_secs,
        timeout_secs,
        failure,
        pid,
        port: p.port,
        port_open: open,
        login_probe: probe,
        log_path: path_to_string(&p.log_file),
        pid_path: path_to_string(&p.pid_file),
        root: path_to_string(&p.root),
        port_conflict,
        port_holder,
        deps_missing: deps_missing(&p.root),
        install_command: detect_package_manager(&p.root)
            .map(|pm| pm.install_command().to_string())
            .unwrap_or_default(),
    }
}

// ---------- 前端依赖（node_modules） ----------

pub const DEPS_MISSING_MSG: &str = "依赖未安装（缺 node_modules）";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    pub fn install_command(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm install",
            PackageManager::Yarn => "yarn install",
            PackageManager::Npm => "npm install",
        }
    }
}

/// 按锁文件选包管理器：pnpm-lock.yaml → pnpm，yarn.lock → yarn，其它（package-lock.json 或没有锁文件）→ npm。
/// 根目录没有 package.json 时返回 None（不是前端项目）。
pub fn detect_package_manager(root: &Path) -> Option<PackageManager> {
    if !root.join("package.json").is_file() {
        return None;
    }
    if root.join("pnpm-lock.yaml").is_file() {
        Some(PackageManager::Pnpm)
    } else if root.join("yarn.lock").is_file() {
        Some(PackageManager::Yarn)
    } else {
        Some(PackageManager::Npm)
    }
}

/// 根目录有 package.json 但没有 node_modules 目录。
pub fn deps_missing(root: &Path) -> bool {
    root.join("package.json").is_file() && !root.join("node_modules").is_dir()
}

fn is_pnpm_token(t: &str) -> bool {
    t == "pnpm" || t.ends_with("/pnpm") || t.eq_ignore_ascii_case("pnpm.cmd")
}

/// 去掉 `pnpm [run] <script> -- …` 里多余的 `--`。
/// pnpm 会把脚本名后面的参数原样交给脚本，`--` 也会被传过去，vite / vue-cli 看到 `--` 就不再解析
/// 后面的 `--port`。npm 的 `npm run dev -- --port` 是对的，不动。
pub fn normalize_start_arg(arg: &str) -> String {
    let mut changed = false;
    let lines: Vec<String> = arg
        .split('\n')
        .map(|line| {
            let toks: Vec<&str> = line.split_whitespace().collect();
            let mut drop = Vec::new();
            for (i, t) in toks.iter().enumerate() {
                if !is_pnpm_token(t) {
                    continue;
                }
                let mut j = i + 1;
                if toks.get(j) == Some(&"run") {
                    j += 1;
                }
                let Some(script) = toks.get(j) else { continue };
                if script.starts_with('-') || matches!(*script, "&&" | "||" | ";" | "|") {
                    continue;
                }
                if toks.get(j + 1) == Some(&"--") {
                    drop.push(j + 1);
                }
            }
            if drop.is_empty() {
                return line.to_string();
            }
            changed = true;
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let kept: Vec<&str> = toks
                .iter()
                .enumerate()
                .filter(|(i, _)| !drop.contains(i))
                .map(|(_, t)| *t)
                .collect();
            format!("{indent}{}", kept.join(" "))
        })
        .collect();
    if changed {
        lines.join("\n")
    } else {
        arg.to_string()
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct InstallMarker {
    pub started_at: u64,
    /// 等安装进程结束并接着启动的那个进程（App / CLI / 网页控制台）。
    pub supervisor: u32,
    #[serde(default)]
    pub pid: Option<u32>,
    pub command: String,
    #[serde(default)]
    pub then_start: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn install_marker_path(pid_file: &Path) -> PathBuf {
    let mut p = pid_file.as_os_str().to_os_string();
    p.push(".install");
    PathBuf::from(p)
}

pub fn read_install_marker(pid_file: &Path) -> Option<InstallMarker> {
    let text = std::fs::read_to_string(install_marker_path(pid_file)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_install_marker(pid_file: &Path, m: &InstallMarker) -> Result<()> {
    let path = install_marker_path(pid_file);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string(m).context("serialize install marker")?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))
}

fn clear_install_marker(pid_file: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(install_marker_path(pid_file)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// 安装标记 → 覆盖到状态上：(状态, 失败原因, 开始时间)。
fn install_overlay(m: Option<InstallMarker>) -> Option<(LifeState, Option<String>, u64)> {
    let m = m?;
    if let Some(err) = m.error {
        return Some((LifeState::Failed, Some(err), m.started_at));
    }
    let child_alive = m.pid.is_some_and(process_alive);
    if child_alive || process_alive(m.supervisor) {
        return Some((LifeState::Installing, None, m.started_at));
    }
    Some((
        LifeState::Failed,
        Some(format!(
            "依赖安装中断（{} 没跑完），点「安装依赖」重试",
            m.command
        )),
        m.started_at,
    ))
}

fn install_in_progress(pid_file: &Path) -> bool {
    matches!(
        install_overlay(read_install_marker(pid_file)),
        Some((LifeState::Installing, _, _))
    )
}

fn write_install_failure(p: &Resolved, msg: &str) {
    let command = detect_package_manager(&p.root)
        .map(|pm| pm.install_command().to_string())
        .unwrap_or_default();
    let m = InstallMarker {
        started_at: current_time_secs(),
        supervisor: std::process::id(),
        pid: None,
        command,
        then_start: false,
        error: Some(format!("依赖安装失败：{msg}")),
    };
    let _ = write_install_marker(&p.pid_file, &m);
}

fn append_log_line(path: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// 起安装；起不来就把原因写进安装标记（状态显示「失败」）。
fn begin_install(p: &Resolved, then_start: bool) {
    if let Err(e) = spawn_install(p, then_start) {
        write_install_failure(p, &format!("{e:#}"));
    }
}

/// 在项目根目录后台跑 `pnpm install` / `yarn install` / `npm install`，Node 环境和启动时一样。
/// 返回等待线程：装好且 `then_start` 时接着启动项目；失败时把原因写进安装标记。
fn spawn_install(p: &Resolved, then_start: bool) -> Result<thread::JoinHandle<()>> {
    let pm = detect_package_manager(&p.root)
        .with_context(|| format!("{} 下没有 package.json", p.root.display()))?;
    let command = pm.install_command().to_string();
    let tc_env = toolchain_env(
        p.java.as_deref(),
        p.java_home.as_deref(),
        p.node.as_deref(),
        p.node_home.as_deref(),
        &ToolchainRoots::from_env(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    ensure_log_dir(&p.log_dir, current_home().as_deref())?;
    if let Some(parent) = p.log_file.parent().filter(|d| !d.as_os_str().is_empty()) {
        if !parent.is_dir() {
            ensure_log_dir(parent, current_home().as_deref())?;
        }
    }
    append_log_line(
        &p.log_file,
        &format!(
            "[tongyun-dev] 安装依赖：{command}（目录 {}）",
            p.root.display()
        ),
    );
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.log_file)
        .with_context(|| format!("open log {}", p.log_file.display()))?;
    let log_err = log.try_clone().context("clone log handle")?;
    let argv = shell_argv(&command, p.shell.as_deref());
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .current_dir(&p.root);
    for (k, v) in &tc_env {
        if !v.is_empty() {
            cmd.env(k, v);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("运行 {command} 失败"))?;
    let marker = InstallMarker {
        started_at: current_time_secs(),
        supervisor: std::process::id(),
        pid: Some(child.id()),
        command: command.clone(),
        then_start,
        error: None,
    };
    if let Err(e) = write_install_marker(&p.pid_file, &marker) {
        let _ = child.kill();
        return Err(e);
    }
    // 上一次「依赖未安装」之类的启动失败不再算数。
    if read_start_marker(&p.pid_file).is_some_and(|m| m.error.is_some()) {
        let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
    }
    let p = p.clone();
    Ok(thread::spawn(move || {
        let result = child.wait();
        // 被「停止」取消了：标记已删或换成了新的一次安装。
        let still_ours = read_install_marker(&p.pid_file)
            .is_some_and(|m| m.pid == marker.pid && m.started_at == marker.started_at);
        if !still_ours {
            append_log_line(&p.log_file, "[tongyun-dev] 安装依赖已取消");
            return;
        }
        match result {
            Ok(status) if status.success() => {
                append_log_line(&p.log_file, &format!("[tongyun-dev] {command} 完成"));
                if then_start {
                    if let Err(e) = cmd_start_detached(&p) {
                        let marker = StartMarker {
                            started_at: current_time_secs(),
                            timeout_secs: start_ready_timeout_secs(&p),
                            error: Some(format!("{e:#}")),
                        };
                        let _ = write_start_marker(&p.pid_file, &marker);
                    }
                }
                let _ = clear_install_marker(&p.pid_file);
            }
            other => {
                let why = match other {
                    Ok(status) => match status.code() {
                        Some(code) => format!("{command} 退出码 {code}，详见日志"),
                        None => format!("{command} 被中断，详见日志"),
                    },
                    Err(e) => format!("{command}：{e}"),
                };
                append_log_line(&p.log_file, &format!("[tongyun-dev] 依赖安装失败：{why}"));
                let mut m = marker;
                m.error = Some(format!("依赖安装失败：{why}"));
                let _ = write_install_marker(&p.pid_file, &m);
            }
        }
    }))
}

/// 停止时顺手取消正在跑的安装（整个进程组），并清掉安装标记。
fn cancel_install(p: &Resolved) -> Result<()> {
    let Some(mut marker) = read_install_marker(&p.pid_file) else {
        return Ok(());
    };
    clear_install_marker(&p.pid_file)?;
    if let Some(pid) = marker
        .pid
        .filter(|&pid| process_alive(pid) || group_alive(pid))
    {
        if let Err(error) = terminate_pid_with_grace(pid, true, 3) {
            marker.error = Some(format!("取消依赖安装失败：{error:#}"));
            write_install_marker(&p.pid_file, &marker)?;
            return Err(error);
        }
    }
    Ok(())
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
    // Child diagnostics must not contaminate the CLI's machine-readable stdout.
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()));
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

fn start_ready_timeout_secs(p: &Resolved) -> u64 {
    if let Some(t) = p.start_timeout_secs {
        if t > 0 {
            return t;
        }
    }
    if let Ok(s) = std::env::var("TONGYUN_DEV_START_TIMEOUT") {
        if let Ok(secs) = s.trim().parse::<u64>() {
            if secs > 0 {
                return secs;
            }
        }
    }
    START_READY_DEFAULT_SECS
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
    _node: Option<&str>,
    _java: Option<&str>,
) -> Vec<String> {
    start.to_vec()
}

fn spawn_background(p: &Resolved, start_cmd: &[String], env: &[(String, String)]) -> Result<()> {
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
    for (k, v) in env {
        if !v.is_empty() {
            cmd.env(k, v);
        }
    }
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

/// Env for stop commands: same JAVA_HOME / PATH as start (catalina.sh stop needs java),
/// but a missing toolchain must not block stopping.
fn stop_env(p: &Resolved, mut base: Vec<(String, String)>) -> Vec<(String, String)> {
    if let Ok(tc) = toolchain_env(
        p.java.as_deref(),
        p.java_home.as_deref(),
        p.node.as_deref(),
        p.node_home.as_deref(),
        &ToolchainRoots::from_env(),
    ) {
        base.extend(tc);
    }
    base
}

fn cmd_start_detached(p: &Resolved) -> Result<()> {
    if !p.enabled {
        let why = if p.reason.is_empty() {
            "disabled"
        } else {
            p.reason.as_str()
        };
        bail!("project `{}` is disabled: {why}", p.id);
    }
    // A live tracked process may still be opening its port. Never overwrite its PID.
    if !tracked_pids(p).is_empty() {
        return Ok(());
    }
    let timeout_secs = start_ready_timeout_secs(p);
    let roots = ToolchainRoots::from_env();
    let tc_env = toolchain_env(
        p.java.as_deref(),
        p.java_home.as_deref(),
        p.node.as_deref(),
        p.node_home.as_deref(),
        &roots,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

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

    let marker = StartMarker {
        started_at: current_time_secs(),
        timeout_secs,
        error: None,
    };
    write_start_marker(&p.pid_file, &marker)?;

    if p.background {
        spawn_background(p, &start_cmd, &tc_env)?;
    } else {
        let mut full_env = tc_env;
        full_env.extend(p.catalina_env());
        run_cmd(&start_cmd, Some(&p.root), &full_env)?;
    }
    Ok(())
}

#[allow(dead_code)]
fn cmd_start(p: &Resolved) -> Result<()> {
    cmd_start_detached(p)
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

fn group_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    Command::new("kill")
        .args(["-0", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn terminate_pid_with_grace(pid: u32, group: bool, grace_secs: u64) -> Result<()> {
    if pid <= 1 {
        bail!("refusing to signal pid {pid}");
    }
    let comm = ps_field(pid, "comm=").unwrap_or_else(|| "process".into());
    eprintln!("sending TERM to {pid} ({comm})");
    if group {
        signal_group(pid, "-TERM");
    }
    signal_pid(pid, "-TERM");
    if wait_until(Duration::from_secs(grace_secs), || {
        if group {
            !process_alive(pid) && !group_alive(pid)
        } else {
            !process_alive(pid)
        }
    }) {
        return Ok(());
    }
    eprintln!("sending KILL to {pid} ({comm})");
    if group {
        signal_group(pid, "-KILL");
    }
    signal_pid(pid, "-KILL");
    if wait_until(Duration::from_secs(3), || {
        if group {
            !process_alive(pid) && !group_alive(pid)
        } else {
            !process_alive(pid)
        }
    }) {
        Ok(())
    } else {
        bail!("pid {pid} still alive after KILL");
    }
}

#[allow(dead_code)]
fn terminate_pid(pid: u32, group: bool) -> Result<()> {
    terminate_pid_with_grace(pid, group, 8)
}

#[allow(dead_code)]
fn terminate_server(pid: u32) -> Result<()> {
    terminate_pid(pid, false)
}

#[allow(dead_code)]
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
    eprintln!("  pid file {}", p.pid_file.display());
    eprintln!("  port {}", p.port);

    let pid_in_file = read_pid(&p.pid_file);
    let stale_pid = match pid_in_file {
        Some(pid) => !process_alive(pid),
        None => p.pid_file.is_file(),
    };

    if stale_pid {
        let _ = std::fs::remove_file(&p.pid_file);
        let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
    }

    if is_down(p) {
        eprintln!("{} already stopped", p.id);
        let _ = std::fs::remove_file(&p.pid_file);
        let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
        return Ok(());
    }

    let grace_secs = p.stop_grace_secs.unwrap_or(8);

    // 1. 先跑配置的 stop 命令（有的话，失败不致命）
    let stop_cmd = p.active_stop();
    if !stop_cmd.is_empty() {
        if let Some(missing) = first_missing_script(&stop_cmd) {
            eprintln!("stop script not found: {missing}");
        } else {
            let (dir, env) = if let Some(tomcat) = &p.tomcat {
                (Some(tomcat.as_path()), stop_env(p, p.catalina_env()))
            } else {
                (Some(p.root.as_path()), stop_env(p, Vec::new()))
            };
            if let Err(e) = run_cmd(&stop_cmd, dir, &env) {
                eprintln!("stop command reported an error: {e}");
            }
        }
    }

    // 本项目自己的进程树根（发信号之前先记下，信号之后父进程可能已经没了）
    let roots: Vec<u32> = pid_in_file
        .filter(|&pid| process_alive(pid))
        .into_iter()
        .collect();

    // 2. 然后对 pid 文件里的进程按进程组发 TERM，宽限期后仍在则对进程组 KILL
    if let Some(pid) = pid_in_file {
        if process_alive(pid) {
            let _ = terminate_pid_with_grace(pid, true, grace_secs);
        }
    }

    // 3. 再清理端口上属于本项目的监听进程（沿用现有 belongs_to / is_server_process 判断，不杀陌生进程）
    if p.tomcat.is_some() {
        for pid in server_pids(p) {
            if process_alive(pid) {
                let _ = terminate_pid_with_grace(pid, false, grace_secs);
            }
        }
    } else {
        // 只清理属于本项目进程树的监听进程，端口上的陌生进程不动
        for pid in listener_pids(p.port) {
            if process_alive(pid) && belongs_to(pid, &roots) {
                let _ = terminate_pid_with_grace(pid, false, grace_secs);
            }
        }
    }

    // Keep tracking information if shutdown did not actually succeed.
    if !wait_until(Duration::from_secs(3), || is_down(p)) {
        bail!("{} 未完全停止：{}", p.id, listener_hint(p.port));
    }

    // 5. 结束后删除 pid 文件和启动标记
    let _ = std::fs::remove_file(&p.pid_file);
    let _ = std::fs::remove_file(start_marker_path(&p.pid_file));

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
    pub java_home: Option<String>,
    #[serde(default)]
    pub node_home: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub background: Option<bool>,
    #[serde(default)]
    pub start_timeout_secs: Option<u64>,
    #[serde(default)]
    pub stop_grace_secs: Option<u64>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub reason: Option<String>,
    /// 缺 node_modules 时启动前自动装依赖。None / true = 缺省（自动装），false = 不自动装。
    #[serde(default)]
    pub auto_install: Option<bool>,
    /// Tomcat 安装目录（WAR 项目用；写了就必须有 stop）。
    #[serde(default)]
    pub tomcat: Option<String>,
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
        let shell = non_empty(&self.shell);
        if let Some(ref sh) = shell {
            let lower = sh.to_ascii_lowercase();
            if !matches!(lower.as_str(), "bash" | "sh" | "cmd" | "powershell") {
                bail!("shell must be one of bash, sh, cmd, powershell, got `{sh}`");
            }
        }
        let java_home = non_empty(&self.java_home);
        let node_home = non_empty(&self.node_home);
        let name = non_empty(&self.name).unwrap_or_else(|| id.clone());
        let enabled = self.enabled.unwrap_or(true);
        let reason = non_empty(&self.reason).unwrap_or_default();
        if !enabled && reason.is_empty() {
            bail!("disabled project `{id}` needs a reason");
        }
        let project = Project {
            name,
            root,
            enabled,
            reason,
            start: shell_argv(&normalize_start_arg(&start), shell.as_deref()),
            stop: non_empty(&self.stop)
                .map(|s| shell_argv(&s, shell.as_deref()))
                .unwrap_or_default(),
            start_windows: Vec::new(),
            stop_windows: Vec::new(),
            tomcat: non_empty(&self.tomcat).unwrap_or_default(),
            log_dir: String::new(),
            log_file: non_empty(&self.log_file).unwrap_or_default(),
            pid_file: String::new(),
            port,
            login_url: non_empty(&self.login_url).unwrap_or_default(),
            health_url: non_empty(&self.health_url).unwrap_or_default(),
            background: self.background.unwrap_or(true),
            start_timeout_secs: self.start_timeout_secs,
            stop_grace_secs: self.stop_grace_secs,
            node,
            java,
            java_home,
            node_home,
            shell,
            auto_install: self.auto_install.filter(|v| !v),
        };
        Ok((id, project))
    }
}

fn project_table(p: &Project) -> toml_edit::Table {
    use toml_edit::{value, Array, Table};
    let mut t = Table::new();
    t["name"] = value(p.name.as_str());
    t["root"] = value(p.root.as_str());
    if !p.enabled {
        t["enabled"] = value(false);
    }
    if !p.reason.is_empty() {
        t["reason"] = value(p.reason.as_str());
    }
    t["port"] = value(i64::from(p.port));
    if p.background {
        t["background"] = value(true);
    }
    if let Some(v) = p.start_timeout_secs {
        t["start_timeout_secs"] = value(v as i64);
    }
    if let Some(v) = p.stop_grace_secs {
        t["stop_grace_secs"] = value(v as i64);
    }
    if let Some(v) = &p.node {
        t["node"] = value(v.as_str());
    }
    if let Some(v) = &p.java {
        t["java"] = value(v.as_str());
    }
    if let Some(v) = &p.java_home {
        t["java_home"] = value(v.as_str());
    }
    if let Some(v) = &p.node_home {
        t["node_home"] = value(v.as_str());
    }
    if let Some(v) = &p.shell {
        t["shell"] = value(v.as_str());
    }
    if !p.tomcat.is_empty() {
        t["tomcat"] = value(p.tomcat.as_str());
    }
    if p.auto_install == Some(false) {
        t["auto_install"] = value(false);
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

/// Result of [`Loaded::import_scanned`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImportReport {
    pub added: Vec<ProjectRow>,
    pub skipped: Vec<SkippedImport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedImport {
    pub id: String,
    pub reason: String,
}

fn canonical_or_self(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Scan without a config file (first run): nothing is registered yet.
pub fn scan_without_config(root: &str, depth: usize) -> Result<ScanReport> {
    scan::scan(&scan_root_path(root)?, depth, &Default::default())
}

/// `~/code`, `$HOME/code`, `/abs/path` or a path relative to the current dir.
fn scan_root_path(raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("scan directory is required");
    }
    let expanded = expand_tilde_with(raw, current_home().as_deref())?;
    let mut p = PathBuf::from(expanded);
    if !p.is_absolute() && !is_windows_absolute(&path_to_string(&p)) {
        p = std::env::current_dir()?.join(p);
    }
    if !p.is_dir() {
        bail!("not a directory: {}", p.display());
    }
    Ok(canonical_or_self(&p))
}

/// Project updates applied via `Loaded::update_project`.
/// Semantics: `None` = do not modify; `Some("")` (for strings) = remove key; otherwise set.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProjectEdit {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub start: Option<String>,
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
    pub enabled: Option<bool>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub start_timeout_secs: Option<u64>,
    #[serde(default)]
    pub stop_grace_secs: Option<u64>,
    /// Some(false) 写 `auto_install = false`；Some(true) 删掉这个键（缺省就是自动装）。
    #[serde(default)]
    pub auto_install: Option<bool>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProjectDetail {
    pub id: String,
    pub name: String,
    pub root: String,
    pub start: String,
    pub start_editable: bool,
    pub stop: String,
    pub stop_editable: bool,
    pub port: u16,
    pub login_url: String,
    pub health_url: String,
    pub log_file: String,
    pub node: Option<String>,
    pub java: Option<String>,
    pub enabled: bool,
    pub reason: String,
    pub start_timeout_secs: Option<u64>,
    pub stop_grace_secs: Option<u64>,
    #[serde(default = "default_true")]
    pub auto_install: bool,
}

fn unpack_command(argv: &[String]) -> (String, bool) {
    if argv.is_empty() {
        (String::new(), true)
    } else if argv.len() == 3 && argv[0] == "bash" && (argv[1] == "-c" || argv[1] == "-lc") {
        (argv[2].clone(), true)
    } else {
        (argv.join(" "), false)
    }
}

fn set_table_val(table: &mut toml_edit::Table, key: &str, new_val: toml_edit::Value) {
    if let Some(item) = table.get_mut(key) {
        if let Some(val) = item.as_value_mut() {
            let decor = val.decor().clone();
            *val = new_val;
            *val.decor_mut() = decor;
            return;
        }
    }
    table.insert(key, toml_edit::Item::Value(new_val));
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
        let configured = r.port > 0 && (!r.active_start().is_empty() || r.tomcat.is_some());
        Ok(ProjectRow {
            id: r.id,
            name: r.name,
            root: path_to_string(&r.root),
            enabled: r.enabled,
            port: r.port,
            reason: r.reason,
            configured,
            start_timeout_secs: r.start_timeout_secs,
            stop_grace_secs: r.stop_grace_secs,
            node: r.node,
            java: r.java,
            java_home: r.java_home.as_ref().map(|p| path_to_string(p)),
            node_home: r.node_home.as_ref().map(|p| path_to_string(p)),
            shell: r.shell,
            auto_install: r.auto_install,
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

    /// Absolute, canonical roots of registered projects -> id (for scan dedupe).
    fn registered_roots(&self) -> HashMap<PathBuf, String> {
        let home = current_home();
        let mut out = HashMap::new();
        for (id, p) in &self.config.projects {
            if let Ok(root) = expand_path("root", &p.root, home.as_deref()) {
                out.insert(canonical_or_self(&root), id.clone());
            }
        }
        out
    }

    /// Scan a folder for projects (read-only). Candidates whose root is already
    /// registered carry `existing_id`; suggested ids never clash with the config.
    pub fn scan(&self, root: &str, depth: usize) -> Result<scan::ScanReport> {
        let dir = scan_root_path(root)?;
        let taken: std::collections::HashSet<String> =
            self.config.projects.keys().cloned().collect();
        let mut report = scan::scan(&dir, depth, &taken)?;
        let roots = self.registered_roots();
        let home = current_home();
        for c in &mut report.candidates {
            if let Ok(p) = expand_path("root", &c.root, home.as_deref()) {
                c.existing_id = roots.get(&canonical_or_self(&p)).cloned();
                if let Some(id) = &c.existing_id {
                    c.id = id.clone();
                }
            }
        }
        Ok(report)
    }

    /// Add scanned (and possibly user-edited) projects. Never overwrites: an id that
    /// already exists or a root that is already registered is skipped with a reason.
    /// Each project is validated and written on its own, so one bad row does not
    /// block the rest.
    pub fn import_scanned(&mut self, items: Vec<NewProject>) -> Result<ImportReport> {
        let home = current_home();
        let mut report = ImportReport::default();
        for item in items {
            let id = item.id.trim().to_string();
            if self.config.projects.contains_key(&id) {
                report.skipped.push(SkippedImport {
                    reason: format!("id `{id}` 已存在"),
                    id,
                });
                continue;
            }
            if let Ok(p) = expand_path("root", item.root.trim(), home.as_deref()) {
                if let Some(other) = self.registered_roots().get(&canonical_or_self(&p)) {
                    report.skipped.push(SkippedImport {
                        id,
                        reason: format!("这个目录已经注册为 `{other}`"),
                    });
                    continue;
                }
            }
            match self.add_project(item) {
                Ok(row) => report.added.push(row),
                Err(e) => report.skipped.push(SkippedImport {
                    id,
                    reason: format!("{e:#}"),
                }),
            }
        }
        Ok(report)
    }

    /// Detailed information about a registered project, suitable for filling the edit form.
    pub fn project_detail(&self, id: &str) -> Result<ProjectDetail> {
        let p = self
            .config
            .projects
            .get(id)
            .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))?;
        let (start, start_editable) = unpack_command(&p.start);
        let (stop, stop_editable) = unpack_command(&p.stop);
        Ok(ProjectDetail {
            id: id.to_string(),
            name: if p.name.trim().is_empty() {
                id.to_string()
            } else {
                p.name.clone()
            },
            root: p.root.clone(),
            start,
            start_editable,
            stop,
            stop_editable,
            port: p.port,
            login_url: p.login_url.clone(),
            health_url: p.health_url.clone(),
            log_file: p.log_file.clone(),
            node: p.node.clone(),
            java: p.java.clone(),
            enabled: p.enabled,
            reason: p.reason.clone(),
            start_timeout_secs: p.start_timeout_secs,
            stop_grace_secs: p.stop_grace_secs,
            auto_install: p.auto_install.unwrap_or(true),
        })
    }

    /// Update an existing project in the config file. Preserves other keys, key order, comments and multiline strings.
    pub fn update_project(&mut self, id: &str, edit: ProjectEdit) -> Result<ProjectRow> {
        let existing = self
            .config
            .projects
            .get(id)
            .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))?;

        for (label, v) in [("node", &edit.node), ("java", &edit.java)] {
            if let Some(v) = v {
                let trimmed = v.trim();
                if !trimmed.is_empty() && !valid_version(trimmed) {
                    bail!("{label} version `{trimmed}` may only contain letters, digits, `.`, `-`, `_`");
                }
            }
        }

        let mut doc = read_doc(&self.path)?;
        let projects = doc
            .get_mut("projects")
            .and_then(|p| p.as_table_mut())
            .context("`projects` in config is not a table")?;
        let proj_table = projects
            .get_mut(id)
            .and_then(|p| p.as_table_mut())
            .context(format!(
                "project `{id}` not found in {}",
                self.path.display()
            ))?;

        if let Some(ref name) = edit.name {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                proj_table.remove("name");
            } else {
                set_table_val(proj_table, "name", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref root) = edit.root {
            let trimmed = root.trim();
            if trimmed.is_empty() {
                proj_table.remove("root");
            } else {
                set_table_val(proj_table, "root", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(port) = edit.port {
            set_table_val(proj_table, "port", toml_edit::Value::from(i64::from(port)));
        }

        if let Some(ref start) = edit.start {
            let trimmed = start.trim();
            if trimmed.is_empty() {
                proj_table.remove("start");
            } else {
                let argv = shell_argv(&normalize_start_arg(trimmed), existing.shell.as_deref());
                let mut a = toml_edit::Array::new();
                for s in argv {
                    a.push(s.as_str());
                }
                set_table_val(proj_table, "start", toml_edit::Value::Array(a));
            }
        }

        if let Some(ref stop) = edit.stop {
            let trimmed = stop.trim();
            if trimmed.is_empty() {
                proj_table.remove("stop");
            } else {
                let argv = shell_argv(trimmed, existing.shell.as_deref());
                let mut a = toml_edit::Array::new();
                for s in argv {
                    a.push(s.as_str());
                }
                set_table_val(proj_table, "stop", toml_edit::Value::Array(a));
            }
        }

        if let Some(ref login_url) = edit.login_url {
            let trimmed = login_url.trim();
            if trimmed.is_empty() {
                proj_table.remove("login_url");
            } else {
                set_table_val(proj_table, "login_url", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref health_url) = edit.health_url {
            let trimmed = health_url.trim();
            if trimmed.is_empty() {
                proj_table.remove("health_url");
            } else {
                set_table_val(proj_table, "health_url", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref log_file) = edit.log_file {
            let trimmed = log_file.trim();
            if trimmed.is_empty() {
                proj_table.remove("log_file");
            } else {
                set_table_val(proj_table, "log_file", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref node) = edit.node {
            let trimmed = node.trim();
            if trimmed.is_empty() {
                proj_table.remove("node");
            } else {
                set_table_val(proj_table, "node", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref java) = edit.java {
            let trimmed = java.trim();
            if trimmed.is_empty() {
                proj_table.remove("java");
            } else {
                set_table_val(proj_table, "java", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(enabled) = edit.enabled {
            set_table_val(proj_table, "enabled", toml_edit::Value::from(enabled));
        }

        if let Some(ref reason) = edit.reason {
            let trimmed = reason.trim();
            if trimmed.is_empty() {
                proj_table.remove("reason");
            } else {
                set_table_val(proj_table, "reason", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(timeout) = edit.start_timeout_secs {
            if timeout == 0 {
                proj_table.remove("start_timeout_secs");
            } else {
                set_table_val(
                    proj_table,
                    "start_timeout_secs",
                    toml_edit::Value::from(timeout as i64),
                );
            }
        }

        if let Some(auto) = edit.auto_install {
            if auto {
                proj_table.remove("auto_install");
            } else {
                set_table_val(proj_table, "auto_install", toml_edit::Value::from(false));
            }
        }

        if let Some(grace) = edit.stop_grace_secs {
            if grace == 0 {
                proj_table.remove("stop_grace_secs");
            } else {
                set_table_val(
                    proj_table,
                    "stop_grace_secs",
                    toml_edit::Value::from(grace as i64),
                );
            }
        }

        let temp_config = parse_config(&doc.to_string())?;
        let updated_project = temp_config
            .projects
            .get(id)
            .context(format!("project `{id}` missing after update"))?;
        resolve_project(id, updated_project, current_home().as_deref())?;

        write_doc(&self.path, &doc)?;
        self.config
            .projects
            .insert(id.to_string(), updated_project.clone());

        let r = self.resolve(id)?;
        let configured = r.port > 0 && (!r.active_start().is_empty() || r.tomcat.is_some());
        Ok(ProjectRow {
            id: r.id,
            name: r.name,
            root: path_to_string(&r.root),
            enabled: r.enabled,
            port: r.port,
            reason: r.reason,
            configured,
            start_timeout_secs: r.start_timeout_secs,
            stop_grace_secs: r.stop_grace_secs,
            node: r.node,
            java: r.java,
            java_home: r.java_home.as_ref().map(|p| path_to_string(p)),
            node_home: r.node_home.as_ref().map(|p| path_to_string(p)),
            shell: r.shell,
            auto_install: r.auto_install,
        })
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

    const EXAMPLE_TOML: &str = include_str!("../../../config/projects.example.toml");
    const TEMPLATE_TOML: &str = include_str!("../../../config/projects.template.toml");
    const LEGACY_TOML: &str = r#"
[projects.legacy_app]
name = "legacy_app"
root = "~/code/acme/legacy-app"
java = "8"
start = ["bash", "{{root}}/scripts/start-legacy.sh"]
stop = ["bash", "{{tomcat}}/bin/catalina.sh", "stop", "15", "-force"]
tomcat = "~/opt/tomcat-9"
log_dir = "~/.tongyun-dev/logs"
log_file = "{{log_dir}}/legacy_app_catalina.out"
pid_file = "{{log_dir}}/legacy_app_tomcat.pid"
port = 8080
login_url = "http://127.0.0.1:8080/legacy_app/admin/login.html"
"#;

    fn legacy_app(home: &Path) -> Resolved {
        let cfg = parse_config(LEGACY_TOML).unwrap();
        resolve_project(
            "legacy_app",
            cfg.projects.get("legacy_app").unwrap(),
            Some(home),
        )
        .unwrap()
    }

    #[test]
    fn expands_tilde_and_home_forms() {
        let home = Path::new("/Users/dev");
        assert_eq!(
            expand_tilde_with("~/code/acme/legacy-app", Some(home)).unwrap(),
            "/Users/dev/code/acme/legacy-app"
        );
        assert_eq!(
            expand_tilde_with(
                "$HOME/.tongyun-dev/logs/legacy_app_catalina.out",
                Some(home)
            )
            .unwrap(),
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
    fn legacy_paths_resolve_without_tilde() {
        let p = legacy_app(Path::new("/Users/dev"));
        assert_eq!(p.root, PathBuf::from("/Users/dev/code/acme/legacy-app"));
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
        for text in [EXAMPLE_TOML, TEMPLATE_TOML] {
            let lower = text.to_ascii_lowercase();
            for bad in ["password", "secret", "token", "jdbc", "api_key", "apikey"] {
                assert!(!lower.contains(bad), "{bad} appeared in a shipped config");
            }
            assert!(!text.contains("sdkman-init"));
            assert!(!text.contains("sdk use"));
            assert!(!text.contains("fnm env"));
            assert!(!text.contains("fnm exec"));
        }
    }

    #[test]
    fn status_json_fields_stay_stable() {
        let st = StatusJson {
            id: "legacy_app".into(),
            name: "legacy_app".into(),
            running: false,
            state: LifeState::Stopped,
            elapsed_secs: None,
            timeout_secs: 180,
            failure: None,
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
            deps_missing: false,
            install_command: String::new(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        for key in [
            "running",
            "state",
            "elapsed_secs",
            "timeout_secs",
            "failure",
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
        assert_eq!(v["state"], "stopped");
        assert_eq!(v["timeout_secs"], 180);
        assert!(v["failure"].is_null());
        assert!(v["elapsed_secs"].is_null());
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
    fn test_derive_state_table() {
        struct Case {
            desc: &'static str,
            input: StateInput,
            expected_state: LifeState,
            expected_failure: Option<&'static str>,
        }

        let cases = vec![
            Case {
                desc: "1. 端口通 + 探针 ok -> Ready",
                input: StateInput {
                    process_alive: true,
                    port_open: true,
                    port_conflict: false,
                    has_probe: true,
                    probe_ok: true,
                    started_at: Some(100),
                    now: 105,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Ready,
                expected_failure: None,
            },
            Case {
                desc: "2. 端口通 + 无探针 -> Ready",
                input: StateInput {
                    process_alive: true,
                    port_open: true,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 105,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Ready,
                expected_failure: None,
            },
            Case {
                desc: "3. 端口通 + 配了探针但没 ok + 未超时 -> PortOpen",
                input: StateInput {
                    process_alive: true,
                    port_open: true,
                    port_conflict: false,
                    has_probe: true,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 105,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::PortOpen,
                expected_failure: None,
            },
            Case {
                desc: "4. 端口不通 + 进程活着 + 未超时 -> Starting",
                input: StateInput {
                    process_alive: true,
                    port_open: false,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 105,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Starting,
                expected_failure: None,
            },
            Case {
                desc: "5. 有启动标记 + 进程已退出 -> Failed (进程已退出)",
                input: StateInput {
                    process_alive: false,
                    port_open: false,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 105,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Failed,
                expected_failure: Some("进程已退出"),
            },
            Case {
                desc: "6. 有启动标记 + 超过 N 秒未就绪 -> Failed (超时)",
                input: StateInput {
                    process_alive: true,
                    port_open: false,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 115,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Failed,
                expected_failure: Some("超过 10 秒仍未就绪"),
            },
            Case {
                desc: "7. 端口冲突 (别的程序占着端口) -> 不算 Ready，进程活着未超时仍是 Starting",
                input: StateInput {
                    process_alive: true,
                    port_open: true,
                    port_conflict: true,
                    has_probe: false,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 105,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Starting,
                expected_failure: None,
            },
            Case {
                desc: "8. 启动命令报错 -> Failed",
                input: StateInput {
                    process_alive: false,
                    port_open: false,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 100,
                    timeout_secs: 10,
                    start_error: Some("缺 Java 8：未在 ~/.sdkman 找到 8.*".to_string()),
                },
                expected_state: LifeState::Failed,
                expected_failure: Some("缺 Java 8：未在 ~/.sdkman 找到 8.*"),
            },
            Case {
                desc: "9. 其它 (无启动标记，进程不活，端口不通) -> Stopped",
                input: StateInput {
                    process_alive: false,
                    port_open: false,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: None,
                    now: 100,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Stopped,
                expected_failure: None,
            },
            Case {
                desc: "10. 端口冲突且无启动标记 -> Stopped",
                input: StateInput {
                    process_alive: false,
                    port_open: true,
                    port_conflict: true,
                    has_probe: false,
                    probe_ok: false,
                    started_at: None,
                    now: 100,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Stopped,
                expected_failure: None,
            },
            Case {
                desc: "11. 端口通但探针未 ok 且超时 -> Failed",
                input: StateInput {
                    process_alive: true,
                    port_open: true,
                    port_conflict: false,
                    has_probe: true,
                    probe_ok: false,
                    started_at: Some(100),
                    now: 115,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Failed,
                expected_failure: Some("超过 10 秒仍未就绪"),
            },
            Case {
                desc: "12. 无启动标记 + 进程活着 + 端口不通 -> Starting",
                input: StateInput {
                    process_alive: true,
                    port_open: false,
                    port_conflict: false,
                    has_probe: false,
                    probe_ok: false,
                    started_at: None,
                    now: 100,
                    timeout_secs: 10,
                    start_error: None,
                },
                expected_state: LifeState::Starting,
                expected_failure: None,
            },
        ];

        for c in cases {
            let (state, failure) = derive_state(c.input);
            assert_eq!(state, c.expected_state, "Case '{}' failed on state", c.desc);
            assert_eq!(
                failure.as_deref(),
                c.expected_failure,
                "Case '{}' failed on failure",
                c.desc
            );
            let running = matches!(state, LifeState::PortOpen | LifeState::Ready);
            if state == LifeState::Starting {
                assert!(!running, "Starting state must have running = false");
            }
        }
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
    fn example_config_resolves() {
        let home = Path::new("/Users/dev");
        let cfg = parse_config(EXAMPLE_TOML).unwrap();
        let mut ids: Vec<_> = cfg.projects.keys().cloned().collect();
        ids.sort();
        assert_eq!(ids, vec!["demo_api", "demo_legacy", "demo_web"]);
        for id in ["demo_api", "demo_web"] {
            let p = resolve_project(id, cfg.projects.get(id).unwrap(), Some(home)).unwrap();
            assert!(p.enabled, "{id}");
            assert!(p.background, "{id}");
            assert!(!p.start.is_empty(), "{id}");
            assert_ne!(p.port, 0, "{id}");
            assert!(p.root.is_absolute(), "{id}");
            assert!(!path_to_string(&p.root).contains('~'), "{id}");
            assert!(!path_to_string(&p.log_file).contains("{{"), "{id}");
        }
        let api = resolve_project(
            "demo_api",
            cfg.projects.get("demo_api").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(api.java.as_deref(), Some("17"));
        assert_eq!(api.port, 8081);
        assert!(api.start.join(" ").contains("spring-boot:run"));
        assert_eq!(api.root, PathBuf::from("/Users/dev/code/demo/demo-api"));
        assert_eq!(
            api.log_file,
            PathBuf::from("/Users/dev/.tongyun-dev/logs/demo_api.out")
        );
        let web = resolve_project(
            "demo_web",
            cfg.projects.get("demo_web").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(web.node.as_deref(), Some("20"));
        assert_eq!(web.login_url, "http://127.0.0.1:5173/");

        let legacy = resolve_project(
            "demo_legacy",
            cfg.projects.get("demo_legacy").unwrap(),
            Some(home),
        )
        .unwrap();
        assert!(!legacy.enabled);
        assert!(!legacy.reason.is_empty());
        assert_eq!(
            legacy.tomcat.as_deref(),
            Some(Path::new("/Users/dev/opt/tomcat-9"))
        );
        let err = cmd_start(&legacy).unwrap_err().to_string();
        assert!(err.contains("disabled"), "{err}");
        assert!(err.contains(&legacy.reason), "{err}");
    }

    #[test]
    fn default_template_has_no_projects() {
        let cfg = parse_config(TEMPLATE_TOML).unwrap();
        assert!(cfg.projects.is_empty());
        assert!(TEMPLATE_TOML.contains("scan"));
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
            configured: true,
            start_timeout_secs: fs.start_timeout_secs,
            stop_grace_secs: fs.stop_grace_secs,
            node: fs.node.clone(),
            java: fs.java.clone(),
            java_home: fs.java_home.as_ref().map(|p| path_to_string(p)),
            node_home: fs.node_home.as_ref().map(|p| path_to_string(p)),
            shell: fs.shell.clone(),
            auto_install: fs.auto_install,
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
            configured: true,
            start_timeout_secs: none.start_timeout_secs,
            stop_grace_secs: none.stop_grace_secs,
            node: none.node.clone(),
            java: none.java.clone(),
            java_home: none.java_home.as_ref().map(|p| path_to_string(p)),
            node_home: none.node_home.as_ref().map(|p| path_to_string(p)),
            shell: none.shell.clone(),
            auto_install: none.auto_install,
        };
        let json_none: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&row_none).unwrap()).unwrap();
        assert!(json_none.get("node").is_none());
        assert!(json_none.get("java").is_none());
    }

    #[test]
    fn assemble_start_cmd_cases() {
        let base = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];

        let with_node = assemble_start_cmd(&base, Some("20"), None);
        assert_eq!(with_node, base);

        let with_java = assemble_start_cmd(&base, None, Some("17"));
        assert_eq!(with_java, base);

        let with_both = assemble_start_cmd(&base, Some("20"), Some("17"));
        assert_eq!(with_both, base);

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

        let start_err = cmd_start(&p_fnm).unwrap_err().to_string();
        assert!(start_err.contains("缺 Node 999"), "{start_err}");

        let start_err_sdk = cmd_start(&p_sdk).unwrap_err().to_string();
        assert!(start_err_sdk.contains("缺 Java 999"), "{start_err_sdk}");
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
        assert_eq!(resolved.assembled_start(), vec!["bash", "-c", "pnpm dev"]);

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
        assert_eq!(p.start, vec!["bash", "-c", "pnpm dev"]);
        assert_eq!(p.stop, vec!["bash", "-c", "pkill -f vite"]);
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

#[cfg(test)]
mod config_resolution_tests {
    use super::*;

    fn make_temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tongyun_dev_cfg_test_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_valid_config(path: &Path) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(
            path,
            r#"[projects.app]
name = "app"
root = "/tmp"
port = 8080
start = ["echo", "hi"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn test_1_explicit_over_env() {
        let temp = make_temp("t1");
        let arg_cfg = temp.join("arg.toml");
        let env_cfg = temp.join("env.toml");
        write_valid_config(&arg_cfg);
        write_valid_config(&env_cfg);

        let (resolved, source) = resolve_config_source_with(
            Some(&arg_cfg),
            Some(env_cfg.to_str().unwrap()),
            Some(&temp),
            None,
            None,
        )
        .unwrap();
        assert_eq!(source, "arg");
        assert_eq!(resolved, arg_cfg);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_2_env_over_remembered() {
        let temp = make_temp("t2");
        let env_cfg = temp.join("env.toml");
        let rem_cfg = temp.join("rem.toml");
        write_valid_config(&env_cfg);
        write_valid_config(&rem_cfg);
        remember_config_path_with(&rem_cfg, Some(&temp)).unwrap();

        let (resolved, source) = resolve_config_source_with(
            None,
            Some(env_cfg.to_str().unwrap()),
            Some(&temp),
            None,
            None,
        )
        .unwrap();
        assert_eq!(source, "env");
        assert_eq!(resolved, env_cfg);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_3_remembered_over_user_default() {
        let temp = make_temp("t3");
        let rem_cfg = temp.join("rem.toml");
        write_valid_config(&rem_cfg);
        remember_config_path_with(&rem_cfg, Some(&temp)).unwrap();

        let def_cfg = default_config_path(Some(&temp));
        write_valid_config(&def_cfg);

        let (resolved, source) =
            resolve_config_source_with(None, None, Some(&temp), None, None).unwrap();
        assert_eq!(source, "remembered");
        assert_eq!(resolved, rem_cfg);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_4_user_default_over_repo() {
        let temp = make_temp("t4");
        let def_cfg = default_config_path(Some(&temp));
        write_valid_config(&def_cfg);

        let repo_dir = temp.join("repo");
        let repo_cfg = repo_dir.join("config/projects.toml");
        write_valid_config(&repo_cfg);

        let (resolved, source) =
            resolve_config_source_with(None, None, Some(&temp), Some(&repo_dir), None).unwrap();
        assert_eq!(source, "user_default");
        assert_eq!(resolved, def_cfg);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_5_none_returns_candidate_error() {
        let temp = make_temp("t5");
        let empty_cwd = temp.join("empty_repo");
        std::fs::create_dir_all(&empty_cwd).unwrap();

        let err = resolve_config_source_with(None, None, Some(&temp), Some(&empty_cwd), None)
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("projects.toml") || msg.contains("not found"));
        assert!(msg.contains("--config"));
        assert!(msg.contains("TONGYUN_DEV_CONFIG"));
        assert!(msg.contains("settings.json"));
        assert!(msg.contains(&path_to_string(&default_config_path(Some(&temp)))));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_6_deleted_remembered_file_skipped() {
        let temp = make_temp("t6");
        let rem_cfg = temp.join("rem.toml");
        write_valid_config(&rem_cfg);
        remember_config_path_with(&rem_cfg, Some(&temp)).unwrap();

        std::fs::remove_file(&rem_cfg).unwrap();

        let def_cfg = default_config_path(Some(&temp));
        write_valid_config(&def_cfg);

        let (resolved, source) =
            resolve_config_source_with(None, None, Some(&temp), None, None).unwrap();
        assert_eq!(source, "user_default");
        assert_eq!(resolved, def_cfg);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_7_remember_rejects_unparseable_file() {
        let temp = make_temp("t7");
        let bad_cfg = temp.join("bad.toml");
        std::fs::write(&bad_cfg, "not a valid toml [[{").unwrap();

        let res = remember_config_path_with(&bad_cfg, Some(&temp));
        assert!(res.is_err());

        let settings = config_dir(Some(&temp)).join("settings.json");
        assert!(!settings.exists());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_8_create_default_does_not_overwrite_and_template_parses() {
        let parsed = parse_config(DEFAULT_CONFIG_TEMPLATE);
        assert!(
            parsed.is_ok(),
            "template should parse successfully: {:?}",
            parsed.err()
        );

        let temp = make_temp("t8");
        let created = create_default_config_with(Some(&temp)).unwrap();
        assert!(created.is_file());
        let initial_content = std::fs::read_to_string(&created).unwrap();
        assert_eq!(initial_content, DEFAULT_CONFIG_TEMPLATE);

        std::fs::write(&created, "# custom user modification\n").unwrap();
        let second_call = create_default_config_with(Some(&temp)).unwrap();
        assert_eq!(second_call, created);
        let current_content = std::fs::read_to_string(&created).unwrap();
        assert_eq!(current_content, "# custom user modification\n");
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_9_remember_preserves_existing_keys() {
        let temp = make_temp("t9");
        let settings_dir = config_dir(Some(&temp));
        std::fs::create_dir_all(&settings_dir).unwrap();
        let settings_path = settings_dir.join("settings.json");
        std::fs::write(
            &settings_path,
            r#"{"existing_key": "preserved", "num": 42}"#,
        )
        .unwrap();

        let cfg = temp.join("cfg.toml");
        write_valid_config(&cfg);
        remember_config_path_with(&cfg, Some(&temp)).unwrap();

        let read_back = std::fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&read_back).unwrap();
        assert_eq!(
            val.get("existing_key").and_then(|v| v.as_str()),
            Some("preserved")
        );
        assert_eq!(val.get("num").and_then(|v| v.as_i64()), Some(42));
        assert_eq!(
            val.get("config_path").and_then(|v| v.as_str()),
            Some(path_to_string(&cfg).as_str())
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_10_remember_fails_on_corrupt_settings_json_without_wiping() {
        let temp = make_temp("t10");
        let settings_dir = config_dir(Some(&temp));
        std::fs::create_dir_all(&settings_dir).unwrap();
        let settings_path = settings_dir.join("settings.json");
        let corrupt_content = "{corrupted json not valid";
        std::fs::write(&settings_path, corrupt_content).unwrap();

        let cfg = temp.join("cfg.toml");
        write_valid_config(&cfg);
        let res = remember_config_path_with(&cfg, Some(&temp));
        assert!(res.is_err());

        let after = std::fs::read_to_string(&settings_path).unwrap();
        assert_eq!(after, corrupt_content);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_11_remember_absolutizes_relative_path() {
        let temp = make_temp("t11");
        let rel_cfg = temp.join("rel.toml");
        write_valid_config(&rel_cfg);

        remember_config_path_with(&rel_cfg, Some(&temp)).unwrap();

        let settings_path = config_dir(Some(&temp)).join("settings.json");
        let read_back = std::fs::read_to_string(&settings_path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&read_back).unwrap();
        let saved = val.get("config_path").unwrap().as_str().unwrap();
        assert!(Path::new(saved).is_absolute());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_12_config_info_reporting() {
        let temp = make_temp("t12");
        let info_none = config_info_with(None, None, Some(&temp), None, None);
        assert_eq!(info_none.source, "none");
        assert!(info_none.path.is_none());
        assert_eq!(
            info_none.default_path,
            path_to_string(&default_config_path(Some(&temp)))
        );

        let def_cfg = default_config_path(Some(&temp));
        write_valid_config(&def_cfg);
        let info_found = config_info_with(None, None, Some(&temp), None, None);
        assert_eq!(info_found.source, "user_default");
        assert_eq!(
            info_found.path.as_deref(),
            Some(path_to_string(&def_cfg).as_str())
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}

#[cfg(test)]
mod edit_project_tests {
    use super::*;

    fn temp_config(tag: &str, body: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tongyun_dev_edit_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj_a")).unwrap();
        std::fs::create_dir_all(dir.join("proj_b")).unwrap();
        let path = dir.join("projects.toml");
        std::fs::write(&path, body.replace("@ROOT@", &path_to_string(&dir))).unwrap();
        path
    }

    const BASE_EDIT: &str = r#"# keep this header comment
[projects.proj_a]
name = "proj_a"
root = "@ROOT@/proj_a"
# keep this inline note before start
start = ["bash", "{{root}}/start.sh"]
port = 8080
login_url = "http://127.0.0.1:8080/login"
enabled = true

# keep comment before proj_b
[projects.proj_b]
name = "proj_b"
root = "@ROOT@/proj_b"
port = 9000
start = ["bash", "-c", '''
set -euo pipefail
echo starting proj_b
''']
"#;

    #[test]
    fn test_1_change_port_preserves_comments_multiline_and_other_projects_verbatim() {
        let path = temp_config("t1", BASE_EDIT);
        let text_before = std::fs::read_to_string(&path).unwrap();
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            port: Some(8888),
            ..Default::default()
        };
        let row = loaded.update_project("proj_a", edit).unwrap();
        assert_eq!(row.port, 8888);

        let text_after = std::fs::read_to_string(&path).unwrap();
        let lines_before: Vec<&str> = text_before.lines().collect();
        let lines_after: Vec<&str> = text_after.lines().collect();

        assert_eq!(
            lines_before.len(),
            lines_after.len(),
            "number of lines must match"
        );
        let mut diff_indices = Vec::new();
        for (idx, (b, a)) in lines_before.iter().zip(lines_after.iter()).enumerate() {
            if b != a {
                diff_indices.push((idx, *b, *a));
            }
        }
        assert_eq!(
            diff_indices.len(),
            1,
            "only exactly one line should change, got: {:?}",
            diff_indices
        );
        assert_eq!(diff_indices[0].1, "port = 8080");
        assert_eq!(diff_indices[0].2, "port = 8888");
    }

    #[test]
    fn test_2_clear_login_url_removes_key() {
        let path = temp_config("t2", BASE_EDIT);
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            login_url: Some("".into()),
            ..Default::default()
        };
        let row = loaded.update_project("proj_a", edit).unwrap();
        assert_eq!(row.id, "proj_a");

        let text_after = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text_after.contains("login_url"),
            "login_url key must be removed from toml"
        );

        let reloaded = load(Some(&path)).unwrap();
        assert_eq!(
            reloaded.config.projects.get("proj_a").unwrap().login_url,
            ""
        );
    }

    #[test]
    fn test_3_change_start_generates_bash_c_argv() {
        let path = temp_config("t3", BASE_EDIT);
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            start: Some("pnpm dev --port 8080".into()),
            ..Default::default()
        };
        loaded.update_project("proj_a", edit).unwrap();

        let text_after = std::fs::read_to_string(&path).unwrap();
        assert!(text_after.contains(r#"start = ["bash", "-c", "pnpm dev --port 8080"]"#));

        let reloaded = load(Some(&path)).unwrap();
        assert_eq!(
            reloaded.config.projects.get("proj_a").unwrap().start,
            vec!["bash", "-c", "pnpm dev --port 8080"]
        );
    }

    #[test]
    fn test_4_start_non_editable_preserves_start_when_changing_port() {
        let path = temp_config("t4", BASE_EDIT);
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            port: Some(9999),
            ..Default::default()
        };
        let row = loaded.update_project("proj_a", edit).unwrap();
        assert_eq!(row.port, 9999);

        let reloaded = load(Some(&path)).unwrap();
        let p = reloaded.config.projects.get("proj_a").unwrap();
        assert_eq!(p.port, 9999);
        assert_eq!(p.start, vec!["bash", "{{root}}/start.sh"]);

        let text_after = std::fs::read_to_string(&path).unwrap();
        assert!(text_after.contains(r#"start = ["bash", "{{root}}/start.sh"]"#));
    }

    #[test]
    fn test_5_enabled_false_without_reason_rejected() {
        let path = temp_config("t5", BASE_EDIT);
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            enabled: Some(false),
            reason: None,
            ..Default::default()
        };
        let err = loaded
            .update_project("proj_a", edit)
            .unwrap_err()
            .to_string();
        assert!(err.contains("needs a reason"), "got: {err}");

        let edit_empty_reason = ProjectEdit {
            enabled: Some(false),
            reason: Some("   ".into()),
            ..Default::default()
        };
        let err2 = loaded
            .update_project("proj_a", edit_empty_reason)
            .unwrap_err()
            .to_string();
        assert!(err2.contains("needs a reason"), "got: {err2}");
    }

    #[test]
    fn test_6_unknown_id_errors() {
        let path = temp_config("t6", BASE_EDIT);
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            port: Some(9090),
            ..Default::default()
        };
        let err = loaded
            .update_project("nonexistent_id", edit)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("unknown project id `nonexistent_id`"),
            "got: {err}"
        );
    }

    #[test]
    fn test_7_write_can_be_read_back_by_load() {
        let path = temp_config("t7", BASE_EDIT);
        let mut loaded = load(Some(&path)).unwrap();

        let edit = ProjectEdit {
            name: Some("proj_a_renamed".into()),
            port: Some(8181),
            node: Some("20".into()),
            start_timeout_secs: Some(120),
            ..Default::default()
        };
        loaded.update_project("proj_a", edit).unwrap();

        let reloaded = load(Some(&path)).unwrap();
        let p = reloaded.config.projects.get("proj_a").unwrap();
        assert_eq!(p.name, "proj_a_renamed");
        assert_eq!(p.port, 8181);
        assert_eq!(p.node.as_deref(), Some("20"));
        assert_eq!(p.start_timeout_secs, Some(120));
    }

    #[test]
    fn test_9_rejected_edit_leaves_file_untouched() {
        let path = temp_config("t9", BASE_EDIT);
        let before = std::fs::read_to_string(&path).unwrap();
        let mut loaded = load(Some(&path)).unwrap();
        let bad_java = ProjectEdit {
            java: Some("8; rm -rf /".into()),
            ..Default::default()
        };
        assert!(loaded.update_project("proj_a", bad_java).is_err());
        let zero_port = ProjectEdit {
            port: Some(0),
            ..Default::default()
        };
        assert!(loaded.update_project("proj_a", zero_port).is_err());
        let no_reason = ProjectEdit {
            enabled: Some(false),
            ..Default::default()
        };
        assert!(loaded.update_project("proj_b", no_reason).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn test_8_project_detail_for_both_start_forms() {
        let path = temp_config("t8", BASE_EDIT);
        let loaded = load(Some(&path)).unwrap();

        let detail_a = loaded.project_detail("proj_a").unwrap();
        assert_eq!(detail_a.id, "proj_a");
        assert_eq!(detail_a.start_editable, false);
        assert_eq!(detail_a.start, "bash {{root}}/start.sh");
        assert_eq!(detail_a.port, 8080);
        assert_eq!(detail_a.login_url, "http://127.0.0.1:8080/login");
        assert!(detail_a.enabled);

        let detail_b = loaded.project_detail("proj_b").unwrap();
        assert_eq!(detail_b.id, "proj_b");
        assert_eq!(detail_b.start_editable, true);
        assert!(detail_b.start.contains("echo starting proj_b"));
        assert_eq!(detail_b.port, 9000);
    }
}

#[cfg(test)]
mod deps_tests {
    use super::*;

    const EXAMPLE_TOML: &str = include_str!("../../../config/projects.example.toml");
    const TEMPLATE_TOML: &str = include_str!("../../../config/projects.template.toml");

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tongyun_dev_deps_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lockfile_picks_package_manager() {
        let dir = temp_dir("lock");
        assert_eq!(
            detect_package_manager(&dir),
            None,
            "no package.json = not a frontend project"
        );
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        assert_eq!(detect_package_manager(&dir), Some(PackageManager::Npm));
        std::fs::write(dir.join("package-lock.json"), "{}").unwrap();
        assert_eq!(detect_package_manager(&dir), Some(PackageManager::Npm));
        std::fs::write(dir.join("yarn.lock"), "").unwrap();
        assert_eq!(detect_package_manager(&dir), Some(PackageManager::Yarn));
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_package_manager(&dir), Some(PackageManager::Pnpm));
        assert_eq!(PackageManager::Pnpm.install_command(), "pnpm install");
        assert_eq!(PackageManager::Yarn.install_command(), "yarn install");
        assert_eq!(PackageManager::Npm.install_command(), "npm install");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_missing_node_modules() {
        let dir = temp_dir("nm");
        assert!(
            !deps_missing(&dir),
            "backend without package.json never needs node_modules"
        );
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        assert!(deps_missing(&dir));
        // A file called node_modules is not an install.
        std::fs::write(dir.join("node_modules"), "").unwrap();
        assert!(deps_missing(&dir));
        std::fs::remove_file(dir.join("node_modules")).unwrap();
        std::fs::create_dir(dir.join("node_modules")).unwrap();
        assert!(!deps_missing(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strips_pnpm_double_dash() {
        let cases = [
            (
                "pnpm dev -- --port 4001 --host 127.0.0.1",
                "pnpm dev --port 4001 --host 127.0.0.1",
            ),
            ("exec pnpm dev -- --port 4004", "exec pnpm dev --port 4004"),
            (
                "pnpm run dev:antd -- --port 4002",
                "pnpm run dev:antd --port 4002",
            ),
            (
                "/opt/homebrew/bin/pnpm serve -- --port 1",
                "/opt/homebrew/bin/pnpm serve --port 1",
            ),
            (
                "cd web && pnpm dev -- --port 3000",
                "cd web && pnpm dev --port 3000",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_start_arg(input), want, "input {input}");
        }
        // Left alone: npm needs its `--`, pnpm flags before a script, plain commands.
        for keep in [
            "npm run dev -- --port 4001 --host 127.0.0.1",
            "pnpm dev --port 5173",
            "pnpm -- dev",
            "yarn dev --port 3000",
            "mvn spring-boot:run -Dspring-boot.run.profiles=dev",
            "pnpm run dev:antd",
        ] {
            assert_eq!(normalize_start_arg(keep), keep);
        }
        // Multi-line `bash -c` bodies keep their other lines byte-for-byte.
        let script = "set -euo pipefail\n  exec pnpm dev -- --port 4003  --host 127.0.0.1\n# pnpm dev -- note";
        assert_eq!(
            normalize_start_arg(script),
            "set -euo pipefail\n  exec pnpm dev --port 4003 --host 127.0.0.1\n# pnpm dev note"
        );
    }

    #[test]
    fn shipped_configs_have_no_pnpm_double_dash() {
        let home = Path::new("/Users/dev");
        for text in [EXAMPLE_TOML, TEMPLATE_TOML] {
            let cfg = parse_config(text).unwrap();
            for (id, p) in &cfg.projects {
                let r = resolve_project(id, p, Some(home)).unwrap();
                for arg in r.active_start() {
                    assert_eq!(
                        normalize_start_arg(&arg),
                        arg,
                        "{id} start still has `pnpm <script> --`"
                    );
                }
                assert!(r.auto_install, "{id}: auto_install defaults to true");
            }
        }
        let cfg = parse_config(EXAMPLE_TOML).unwrap();
        let start_of = |id: &str| {
            resolve_project(id, cfg.projects.get(id).unwrap(), Some(home))
                .unwrap()
                .assembled_start()
                .join(" ")
        };
        assert!(start_of("demo_web").contains("pnpm dev --port 5173 --host 127.0.0.1"));
        assert!(start_of("demo_api").contains("mvn spring-boot:run"));
    }

    #[test]
    fn old_configs_are_rewritten_at_start_and_auto_install_opt_out() {
        let text = r#"
[projects.web]
root = "/tmp/web"
port = 5173
background = true
start = ["bash", "-c", "exec pnpm dev -- --port 5173"]

[projects.web_manual]
root = "/tmp/web"
port = 5174
auto_install = false
start = ["pnpm", "dev", "--", "--port", "5174"]
"#;
        let cfg = parse_config(text).unwrap();
        let home = Some(Path::new("/Users/dev"));
        let web = resolve_project("web", cfg.projects.get("web").unwrap(), home).unwrap();
        assert_eq!(
            web.assembled_start(),
            vec!["bash", "-c", "exec pnpm dev --port 5173"]
        );
        assert!(web.auto_install);
        let manual =
            resolve_project("web_manual", cfg.projects.get("web_manual").unwrap(), home).unwrap();
        assert!(!manual.auto_install);
        // argv form: each token is checked on its own, so `--` as its own element is left
        // (pnpm only gets it from a shell line in practice).
        assert_eq!(
            manual.assembled_start(),
            vec!["pnpm", "dev", "--", "--port", "5174"]
        );
    }

    #[test]
    fn install_marker_states() {
        let me = std::process::id();
        let base = InstallMarker {
            started_at: 100,
            supervisor: me,
            pid: None,
            command: "pnpm install".into(),
            then_start: true,
            error: None,
        };
        assert_eq!(install_overlay(None), None);
        let (st, why, at) = install_overlay(Some(base.clone())).unwrap();
        assert_eq!((st, why, at), (LifeState::Installing, None, 100));

        let mut failed = base.clone();
        failed.error = Some("依赖安装失败：pnpm install 退出码 1，详见日志".into());
        let (st, why, _) = install_overlay(Some(failed)).unwrap();
        assert_eq!(st, LifeState::Failed);
        assert!(why.unwrap().contains("退出码 1"));

        // Supervisor and installer both gone without a result: interrupted.
        let mut orphan = base;
        orphan.supervisor = 99_999_999;
        orphan.pid = Some(99_999_998);
        let (st, why, _) = install_overlay(Some(orphan)).unwrap();
        assert_eq!(st, LifeState::Failed);
        assert!(why.unwrap().contains("中断"));
    }

    #[cfg(unix)]
    fn fake_toolchain(dir: &Path, npm_body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("nodebin");
        std::fs::create_dir_all(&bin).unwrap();
        for (name, body) in [("node", "#!/bin/sh\nexit 0\n"), ("npm", npm_body)] {
            let p = bin.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        bin
    }

    #[cfg(unix)]
    fn frontend_config(dir: &Path, bin: &Path, extra: &str) -> PathBuf {
        let app = dir.join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("package.json"), "{}").unwrap();
        let cfg = format!(
            r#"[projects.web]
root = "{root}"
port = 1
background = true
node_home = "{bin}"
log_dir = "{logs}"
log_file = "{logs}/web.out"
pid_file = "{logs}/web.pid"
start = ["bash", "-c", "echo started > started.txt; exec sleep 30"]
{extra}
"#,
            root = path_to_string(&app),
            bin = path_to_string(bin),
            logs = path_to_string(&dir.join("logs")),
        );
        let path = dir.join("projects.toml");
        std::fs::write(&path, cfg).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn start_installs_missing_deps_then_starts() {
        let dir = temp_dir("auto");
        let bin = fake_toolchain(
            &dir,
            "#!/bin/sh\necho fake npm \"$@\"\nsleep 1\nmkdir node_modules\n",
        );
        let path = frontend_config(&dir, &bin, "");
        let loaded = load(Some(&path)).unwrap();

        let st = loaded.start_detached("web").unwrap();
        assert_eq!(st.state, LifeState::Installing, "{st:?}");
        assert!(st.deps_missing);
        assert_eq!(st.install_command, "npm install");

        let app = dir.join("app");
        assert!(wait_until(Duration::from_secs(10), || app
            .join("started.txt")
            .is_file()));
        assert!(app.join("node_modules").is_dir());
        assert!(wait_until(Duration::from_secs(5), || {
            read_install_marker(&dir.join("logs/web.pid")).is_none()
        }));
        let st = loaded.status("web").unwrap();
        assert!(!st.deps_missing);
        assert_ne!(st.state, LifeState::Installing);
        let log = std::fs::read_to_string(dir.join("logs/web.out")).unwrap();
        assert!(log.contains("[tongyun-dev] 安装依赖：npm install"), "{log}");
        assert!(log.contains("fake npm install"), "{log}");
        loaded.stop("web").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn failed_install_is_reported_and_start_skipped() {
        let dir = temp_dir("fail");
        let bin = fake_toolchain(&dir, "#!/bin/sh\necho 'ERR_PNPM boom' >&2\nexit 3\n");
        let path = frontend_config(&dir, &bin, "");
        let loaded = load(Some(&path)).unwrap();
        loaded.start_detached("web").unwrap();
        assert!(wait_until(Duration::from_secs(10), || {
            loaded.status("web").unwrap().state == LifeState::Failed
        }));
        let st = loaded.status("web").unwrap();
        let why = st.failure.unwrap();
        assert!(
            why.contains("依赖安装失败") && why.contains("退出码 3"),
            "{why}"
        );
        assert!(
            !dir.join("app/started.txt").exists(),
            "start must not run after a failed install"
        );
        // Stop clears the failure.
        loaded.stop("web").unwrap();
        assert!(read_install_marker(&dir.join("logs/web.pid")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn auto_install_false_reports_missing_deps() {
        let dir = temp_dir("manual");
        let bin = fake_toolchain(&dir, "#!/bin/sh\nmkdir node_modules\n");
        let path = frontend_config(&dir, &bin, "auto_install = false");
        let loaded = load(Some(&path)).unwrap();
        let st = loaded.start_detached("web").unwrap();
        assert_eq!(st.state, LifeState::Failed);
        assert_eq!(st.failure.as_deref(), Some(DEPS_MISSING_MSG));
        assert!(!dir.join("app/started.txt").exists());

        // Manual install (the 「安装依赖」 button / `tongyun-dev install`).
        let st = loaded.install_deps_blocking("web").unwrap();
        assert!(!st.deps_missing);
        assert!(dir.join("app/node_modules").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod scan_import_tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("tongyun_scan_import_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn scan_marks_registered_roots_and_import_never_overwrites() {
        let d = tmp("dedupe");
        let code = d.join("code");
        let vite =
            r#"{"name":"web","scripts":{"dev":"vite --port 5174"},"devDependencies":{"vite":"5"}}"#;
        write(&code.join("web/package.json"), vite);
        write(
            &code.join("shop/package.json"),
            &vite.replace("\"web\"", "\"shop\"").replace("5174", "5175"),
        );
        let cfg = d.join("projects.toml");
        write(
            &cfg,
            &format!(
                "# my projects\n[projects.web]\nroot = \"{}\"\nport = 5174\nstart = [\"pnpm\", \"dev\"]\n",
                path_to_string(&code.join("web"))
            ),
        );
        let mut loaded = load(Some(&cfg)).unwrap();
        let report = loaded.scan(&path_to_string(&code), 3).unwrap();
        assert_eq!(report.candidates.len(), 2);
        let web = report
            .candidates
            .iter()
            .find(|c| c.root.ends_with("/web"))
            .unwrap();
        assert_eq!(web.existing_id.as_deref(), Some("web"));
        assert_eq!(
            web.id, "web",
            "already-registered rows show their registered id"
        );
        let shop = report
            .candidates
            .iter()
            .find(|c| c.root.ends_with("/shop"))
            .unwrap();
        assert!(shop.existing_id.is_none());

        let items: Vec<NewProject> = report
            .candidates
            .iter()
            .map(|c| c.to_new_project())
            .collect();
        let out = loaded.import_scanned(items).unwrap();
        assert_eq!(out.added.len(), 1);
        assert_eq!(out.added[0].id, "shop");
        assert_eq!(out.added[0].port, 5175);
        assert_eq!(out.skipped.len(), 1);
        assert!(out.skipped[0].reason.contains("web"), "{:?}", out.skipped);
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(text.starts_with("# my projects"), "comments kept: {text}");
        assert!(text.contains("[projects.shop]"));
        // Re-import is a no-op.
        let again = loaded.scan(&path_to_string(&code), 3).unwrap();
        assert!(again.candidates.iter().all(|c| c.existing_id.is_some()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn import_into_empty_default_template_and_tomcat_row() {
        let d = tmp("empty");
        let cfg = d.join("projects.toml");
        std::fs::write(&cfg, DEFAULT_CONFIG_TEMPLATE).unwrap();
        let mut loaded = load(Some(&cfg)).unwrap();
        assert!(loaded.list().unwrap().projects.is_empty());
        write(
            &d.join("legacy/pom.xml"),
            "<project><artifactId>legacy</artifactId><packaging>war</packaging></project>",
        );
        let report = loaded.scan(&path_to_string(&d), 2).unwrap();
        assert_eq!(report.candidates.len(), 1);
        let mut req = report.candidates[0].to_new_project();
        // User filled in Tomcat in the scan list -> enabled.
        req.tomcat = Some("/opt/tomcat".into());
        req.enabled = Some(true);
        req.reason = None;
        let out = loaded.import_scanned(vec![req]).unwrap();
        assert_eq!(out.added.len(), 1, "{:?}", out.skipped);
        let r = loaded.resolve("legacy").unwrap();
        assert_eq!(r.tomcat.as_deref(), Some(Path::new("/opt/tomcat")));
        assert!(r.active_start()[2].contains("/opt/tomcat/webapps/legacy.war"));
        assert!(!r.stop.is_empty());
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            text.contains("tongyun-dev 项目配置"),
            "template header kept"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
