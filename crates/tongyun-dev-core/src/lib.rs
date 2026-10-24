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
pub mod secret_store;
pub mod toolchain;

pub use scan::{Candidate, Confidence, ScanReport, DEFAULT_SCAN_DEPTH};
pub use toolchain::{
    format_home, resolve_java_home, resolve_node_bin, shell_argv, shell_argv_with_os,
    toolchain_env, HostOs, ToolchainError, ToolchainRoots,
};

const START_READY_DEFAULT_SECS: u64 = 180;
/// 有启动标记、进程却找不到时，先等这么多秒再判定「进程已退出」。
const START_EXIT_GRACE_SECS: u64 = 3;
/// Batch start/restart records this instead of launching a project whose dependency failed.
pub const DEP_START_SKIPPED: &str = "依赖服务启动失败，已跳过";
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
    /// Default `log_dir` for every project that does not set its own.
    /// Empty means `~/.tongyun-dev/logs`. Same resolution rules as a project value.
    #[serde(default)]
    log_dir: String,
    #[serde(default)]
    projects: HashMap<String, Project>,
    #[serde(default)]
    group: HashMap<String, GroupConfig>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct GroupConfig {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub order: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AutoAnswer {
    pub expect: String,
    #[serde(default)]
    pub send: String,
    #[serde(default = "default_true")]
    pub once: bool,
    #[serde(default)]
    pub regex: bool,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct WebLink {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct ProfileConfig {
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Project {
    #[serde(default)]
    name: String,
    #[serde(default)]
    group: String,
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
    /// 端口传递方式："none"（缺省）、"arg"、"env"、"config"
    #[serde(default)]
    pub port_mode: Option<String>,
    /// 端口参数名（缺省 "--port"）
    #[serde(default)]
    pub port_arg: Option<String>,
    /// 端口环境变量名（缺省 "PORT"）
    #[serde(default)]
    pub port_env: Option<String>,
    /// 额外环境变量（可选）
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 伪终端运行（可选，缺省 false）。
    #[serde(default)]
    pub pty: bool,
    /// 自动应答规则（可选）。
    #[serde(default)]
    pub auto_answers: Vec<AutoAnswer>,
    /// 自定义快捷网页链接（可选）。
    #[serde(default)]
    pub links: Vec<WebLink>,
    /// 默认启动环境预设（可选）。
    #[serde(default)]
    pub default_profile: Option<String>,
    /// 启动环境预设表（可选）。
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, ProfileConfig>,
    /// 依赖服务项目 ID 列表（可选）。
    #[serde(default)]
    pub depends_on: Vec<String>,
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
    group: String,
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
    pub port_mode: String,
    pub port_arg: Option<String>,
    pub port_env: Option<String>,
    pub env: std::collections::BTreeMap<String, String>,
    pub pty: bool,
    pub auto_answers: Vec<AutoAnswer>,
    pub links: Vec<WebLink>,
    pub depends_on: Vec<String>,
}

impl Resolved {
    #[cfg(unix)]
    pub fn stdin_fifo_path(&self) -> PathBuf {
        self.log_dir.join(format!("{}.stdin", self.id))
    }

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
        let active = self.active_start();
        if self.port_mode == "arg" {
            append_port_arg(&active, self.port, self.port_arg.as_deref())
        } else {
            active
        }
    }
}

pub fn default_port_mode() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
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
    #[serde(default = "default_port_mode")]
    pub port_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_env: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub pty: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_answers: Vec<AutoAnswer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<WebLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profiles: std::collections::BTreeMap<String, ProfileConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Shell line a person can paste to start the project (assembled start command).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start_command: String,
    /// Assembled start line for each profile name. Copy uses the selected profile.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profile_start_commands: std::collections::BTreeMap<String, String>,
    /// Set when this row could not be resolved (for example a bad `log_dir`).
    /// The rest of the catalog still loads, so the main window can open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct GroupInfo {
    pub key: String,
    pub label: String,
    pub order: Option<i64>,
    /// Project ids in start order (`[group.<key>].members`, then other projects in the group).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub config_path: String,
    pub projects: Vec<ProjectRow>,
    pub groups: Vec<GroupInfo>,
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
    /// 诊断建议
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
    /// 当前生效的启动预设环境（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    /// Set when the project could not be resolved. Absent on a normal status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolve_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct BatchItemResult {
    pub id: String,
    pub success: bool,
    #[serde(default)]
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct BatchOutput {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub results: Vec<BatchItemResult>,
}

/// Status row for a project `list` could not resolve. Other projects are unaffected.
pub fn unresolved_status(row: &ProjectRow, failure: impl Into<String>) -> StatusJson {
    StatusJson {
        id: row.id.clone(),
        name: row.name.clone(),
        running: false,
        state: LifeState::Stopped,
        elapsed_secs: None,
        timeout_secs: 0,
        failure: Some(failure.into()),
        pid: None,
        port: row.port,
        port_open: false,
        login_probe: LoginProbe {
            url: String::new(),
            http_code: None,
            ok: false,
        },
        log_path: String::new(),
        pid_path: String::new(),
        root: row.root.clone(),
        port_conflict: false,
        port_holder: String::new(),
        deps_missing: false,
        install_command: String::new(),
        hints: Vec::new(),
        active_profile: None,
        resolve_error: row.resolve_error.clone(),
    }
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

fn row_from_resolved(
    resolved: &Resolved,
    configured: bool,
    default_profile: Option<String>,
    profiles: std::collections::BTreeMap<String, ProfileConfig>,
    profile_start_commands: std::collections::BTreeMap<String, String>,
) -> ProjectRow {
    ProjectRow {
        id: resolved.id.clone(),
        name: resolved.name.clone(),
        group: if resolved.group.is_empty() {
            None
        } else {
            Some(resolved.group.clone())
        },
        root: path_to_string(&resolved.root),
        enabled: resolved.enabled,
        port: resolved.port,
        reason: resolved.reason.clone(),
        configured,
        start_timeout_secs: resolved.start_timeout_secs,
        stop_grace_secs: resolved.stop_grace_secs,
        node: resolved.node.clone(),
        java: resolved.java.clone(),
        java_home: resolved.java_home.as_ref().map(|p| path_to_string(p)),
        node_home: resolved.node_home.as_ref().map(|p| path_to_string(p)),
        shell: resolved.shell.clone(),
        auto_install: resolved.auto_install,
        port_mode: resolved.port_mode.clone(),
        port_arg: resolved.port_arg.clone(),
        port_env: resolved.port_env.clone(),
        env: resolved.env.clone(),
        pty: resolved.pty,
        auto_answers: resolved.auto_answers.clone(),
        links: resolved.links.clone(),
        default_profile,
        profiles,
        depends_on: resolved.depends_on.clone(),
        start_command: display_command(&resolved.assembled_start()),
        profile_start_commands,
        resolve_error: None,
    }
}

fn unresolved_row(id: &str, raw: Option<&Project>, err: String) -> ProjectRow {
    let name = raw
        .map(|p| p.name.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
        .to_string();
    ProjectRow {
        id: id.to_string(),
        name,
        group: raw.and_then(|p| {
            let g = p.group.trim();
            if g.is_empty() {
                None
            } else {
                Some(g.to_string())
            }
        }),
        root: raw.map(|p| p.root.clone()).unwrap_or_default(),
        enabled: raw.map(|p| p.enabled).unwrap_or(true),
        port: raw.map(|p| p.port).unwrap_or(0),
        reason: raw.map(|p| p.reason.clone()).unwrap_or_default(),
        configured: false,
        start_timeout_secs: raw.and_then(|p| p.start_timeout_secs),
        stop_grace_secs: raw.and_then(|p| p.stop_grace_secs),
        node: raw.and_then(|p| p.node.clone()),
        java: raw.and_then(|p| p.java.clone()),
        java_home: raw.and_then(|p| p.java_home.clone()),
        node_home: raw.and_then(|p| p.node_home.clone()),
        shell: raw.and_then(|p| p.shell.clone()),
        auto_install: raw.map(|p| p.auto_install.unwrap_or(true)).unwrap_or(true),
        port_mode: raw
            .and_then(|p| p.port_mode.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "none".to_string()),
        port_arg: raw.and_then(|p| p.port_arg.clone()),
        port_env: raw.and_then(|p| p.port_env.clone()),
        env: raw.map(|p| p.env.clone()).unwrap_or_default(),
        pty: raw.map(|p| p.pty).unwrap_or(false),
        auto_answers: raw.map(|p| p.auto_answers.clone()).unwrap_or_default(),
        links: raw.map(|p| p.links.clone()).unwrap_or_default(),
        default_profile: raw.and_then(|p| p.default_profile.clone()),
        profiles: raw.map(|p| p.profiles.clone()).unwrap_or_default(),
        depends_on: raw.map(|p| p.depends_on.clone()).unwrap_or_default(),
        start_command: raw.map(|p| display_command(&p.start)).unwrap_or_default(),
        profile_start_commands: std::collections::BTreeMap::new(),
        resolve_error: Some(err),
    }
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
            match self.resolve(&id) {
                Ok(resolved) => {
                    let configured = resolved.port > 0
                        && (!resolved.active_start().is_empty() || resolved.tomcat.is_some());
                    projects.push(self.finish_row(&resolved, configured));
                }
                Err(err) => {
                    let raw = self.config.projects.get(&id);
                    projects.push(unresolved_row(&id, raw, format!("{err:#}")));
                }
            }
        }
        let groups = self.groups();
        Ok(ProjectList {
            config_path: path_to_string(&self.path),
            projects,
            groups,
        })
    }

    pub fn groups(&self) -> Vec<GroupInfo> {
        let mut group_map: HashMap<String, GroupInfo> = HashMap::new();
        for (key, cfg) in &self.config.group {
            let label = cfg
                .label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(key)
                .to_string();
            group_map.insert(
                key.clone(),
                GroupInfo {
                    key: key.clone(),
                    label,
                    order: cfg.order,
                    members: Vec::new(),
                    depends_on: cfg.depends_on.clone(),
                },
            );
        }
        for p in self.config.projects.values() {
            let k = p.group.trim();
            if !k.is_empty() && !group_map.contains_key(k) {
                group_map.insert(
                    k.to_string(),
                    GroupInfo {
                        key: k.to_string(),
                        label: k.to_string(),
                        order: None,
                        members: Vec::new(),
                        depends_on: Vec::new(),
                    },
                );
            }
        }
        let keys: Vec<String> = group_map.keys().cloned().collect();
        for key in keys {
            let members = self.projects_in_group(&key);
            if let Some(info) = group_map.get_mut(&key) {
                info.members = members;
            }
        }
        let mut list: Vec<GroupInfo> = group_map.into_values().collect();
        list.sort_by(|a, b| match (a.order, b.order) {
            (Some(oa), Some(ob)) => {
                if oa != ob {
                    oa.cmp(&ob)
                } else {
                    a.label.cmp(&b.label).then_with(|| a.key.cmp(&b.key))
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.label.cmp(&b.label).then_with(|| a.key.cmp(&b.key)),
        });
        list
    }

    /// Return project IDs that belong to the specified group (either via project.group or group.members).
    pub fn projects_in_group(&self, group_key: &str) -> Vec<String> {
        let key_trim = group_key.trim();
        let mut ids = Vec::new();
        if let Some(cfg) = self.config.group.get(key_trim) {
            for m in &cfg.members {
                if self.config.projects.contains_key(m) && !ids.contains(m) {
                    ids.push(m.clone());
                }
            }
        }
        let mut rest = Vec::new();
        for (id, p) in &self.config.projects {
            if p.group.trim() == key_trim && !ids.contains(id) {
                rest.push(id.clone());
            }
        }
        rest.sort();
        ids.extend(rest);
        ids
    }

    /// True when `id` is a registered project.
    pub fn contains_project(&self, id: &str) -> bool {
        self.config.projects.contains_key(id.trim())
    }

    /// Direct `depends_on` ids for a project. Empty when the id is unknown.
    pub fn depends_on_of(&self, id: &str) -> Vec<String> {
        self.config
            .projects
            .get(id.trim())
            .map(|p| p.depends_on.clone())
            .unwrap_or_default()
    }

    fn profile_start_commands(&self, id: &str) -> std::collections::BTreeMap<String, String> {
        let Some(raw) = self.config.projects.get(id) else {
            return std::collections::BTreeMap::new();
        };
        let mut out = std::collections::BTreeMap::new();
        for name in raw.profiles.keys() {
            if let Ok(resolved) = self.resolve_with_profile(id, Some(name.as_str())) {
                let line = display_command(&resolved.assembled_start());
                if !line.is_empty() {
                    out.insert(name.clone(), line);
                }
            }
        }
        out
    }

    fn finish_row(&self, resolved: &Resolved, configured: bool) -> ProjectRow {
        let raw = self.config.projects.get(&resolved.id);
        row_from_resolved(
            resolved,
            configured,
            raw.and_then(|p| p.default_profile.clone()),
            raw.map(|p| p.profiles.clone()).unwrap_or_default(),
            self.profile_start_commands(&resolved.id),
        )
    }

    /// True when `key` is a declared group or at least one project sets `group = key`.
    pub fn contains_group(&self, key: &str) -> bool {
        let key = key.trim();
        if key.is_empty() {
            return false;
        }
        self.config.group.contains_key(key)
            || self.config.projects.values().any(|p| p.group.trim() == key)
    }

    /// Index inside `[group].members` for this project. Missing entries sort last.
    fn member_index(&self, id: &str) -> usize {
        let group = self
            .config
            .projects
            .get(id)
            .map(|p| p.group.trim())
            .unwrap_or("");
        if !group.is_empty() {
            if let Some(cfg) = self.config.group.get(group) {
                if let Some(i) = cfg.members.iter().position(|m| m == id) {
                    return i;
                }
            }
        }
        for cfg in self.config.group.values() {
            if let Some(i) = cfg.members.iter().position(|m| m == id) {
                return i;
            }
        }
        usize::MAX
    }

    /// Return group keys sorted by dependency order (dependencies first), then by order and label.
    pub fn group_start_order(&self) -> Vec<String> {
        let groups = self.groups();
        let mut in_degrees: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();

        for g in &groups {
            in_degrees.entry(g.key.clone()).or_insert(0);
            adj.entry(g.key.clone()).or_default();
        }

        for (key, cfg) in &self.config.group {
            for dep in &cfg.depends_on {
                if adj.contains_key(dep) && adj.contains_key(key) {
                    adj.get_mut(dep).unwrap().push(key.clone());
                    *in_degrees.get_mut(key).unwrap() += 1;
                }
            }
        }

        let mut ready: Vec<GroupInfo> = groups
            .iter()
            .filter(|g| in_degrees.get(&g.key).copied().unwrap_or(0) == 0)
            .cloned()
            .collect();

        let mut order = Vec::new();
        while !ready.is_empty() {
            ready.sort_by(|a, b| match (a.order, b.order) {
                (Some(oa), Some(ob)) => {
                    if oa != ob {
                        oa.cmp(&ob)
                    } else {
                        a.label.cmp(&b.label).then_with(|| a.key.cmp(&b.key))
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.label.cmp(&b.label).then_with(|| a.key.cmp(&b.key)),
            });
            let next = ready.remove(0);
            order.push(next.key.clone());
            if let Some(neighbors) = adj.get(&next.key) {
                for n in neighbors {
                    if let Some(deg) = in_degrees.get_mut(n) {
                        *deg -= 1;
                        if *deg == 0 {
                            if let Some(info) = groups.iter().find(|g| &g.key == n) {
                                ready.push(info.clone());
                            }
                        }
                    }
                }
            }
        }

        for g in &groups {
            if !order.contains(&g.key) {
                order.push(g.key.clone());
            }
        }
        order
    }

    /// Sort project IDs for startup according to project dependency ordering, group dependency ordering and group order.
    pub fn sorted_projects_for_start<'a>(&self, ids: &[&'a str]) -> Vec<String> {
        let group_order = self.group_start_order();
        let mut group_rank: HashMap<&str, usize> = HashMap::new();
        for (idx, g) in group_order.iter().enumerate() {
            group_rank.insert(g.as_str(), idx);
        }

        let mut in_degrees: HashMap<&'a str, usize> = HashMap::new();
        let mut adj: HashMap<&'a str, Vec<&'a str>> = HashMap::new();

        for &id in ids {
            in_degrees.insert(id, 0);
            adj.insert(id, Vec::new());
        }

        // 1. Direct project dependencies: dep -> id (dep starts before id)
        for &id in ids {
            if let Some(p) = self.config.projects.get(id) {
                for dep in &p.depends_on {
                    if let Some(&dep_in_ids) = ids.iter().find(|&&i| i == dep.as_str()) {
                        if let Some(neighbors) = adj.get_mut(dep_in_ids) {
                            if !neighbors.contains(&id) {
                                neighbors.push(id);
                                if let Some(deg) = in_degrees.get_mut(id) {
                                    *deg += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Ready queue with in_degree == 0
        let mut ready: Vec<&'a str> = ids
            .iter()
            .copied()
            .filter(|&id| in_degrees.get(id).copied().unwrap_or(0) == 0)
            .collect();

        let mut order = Vec::new();
        while !ready.is_empty() {
            ready.sort_by(|&a, &b| {
                let pa = self.config.projects.get(a);
                let pb = self.config.projects.get(b);
                let ga = pa.map(|p| p.group.trim()).unwrap_or("");
                let gb = pb.map(|p| p.group.trim()).unwrap_or("");
                let ra = group_rank.get(ga).copied().unwrap_or(usize::MAX);
                let rb = group_rank.get(gb).copied().unwrap_or(usize::MAX);
                if ra != rb {
                    ra.cmp(&rb)
                } else {
                    let ma = self.member_index(a);
                    let mb = self.member_index(b);
                    if ma != mb {
                        ma.cmp(&mb)
                    } else {
                        let na = pa.map(|p| p.name.as_str()).unwrap_or(a);
                        let nb = pb.map(|p| p.name.as_str()).unwrap_or(b);
                        na.cmp(nb).then_with(|| a.cmp(b))
                    }
                }
            });
            let next = ready.remove(0);
            order.push(next.to_string());
            if let Some(neighbors) = adj.get(next) {
                for &n in neighbors {
                    if let Some(deg) = in_degrees.get_mut(n) {
                        *deg -= 1;
                        if *deg == 0 {
                            ready.push(n);
                        }
                    }
                }
            }
        }

        // Fallback for cycles or any disconnected
        for &id in ids {
            if !order.contains(&id.to_string()) {
                order.push(id.to_string());
            }
        }

        order
    }

    /// Sort project IDs for shutdown in reverse of startup dependency ordering.
    pub fn sorted_projects_for_stop<'a>(&self, ids: &[&'a str]) -> Vec<String> {
        let mut list = self.sorted_projects_for_start(ids);
        list.reverse();
        list
    }
}

/// Validate group key: lowercase letters, digits, underscore, hyphen; 1-40 characters.
pub fn validate_group_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("分组 key 不能为空");
    }
    if key.len() > 40 {
        bail!("分组 key 长度不能超过 40 个字符");
    }
    for c in key.chars() {
        if !matches!(c, 'a'..='z' | '0'..='9' | '_' | '-') {
            bail!("分组 key 只能包含小写字母、数字、下划线和连字符: `{key}`");
        }
    }
    Ok(())
}

/// Generate a valid group key from a label:
/// - ASCII labels: lowercase, replace illegal chars (`[^a-z0-9_-]`) with `-`, trim leading/trailing `-`, truncate to <=40 chars.
///   If non-empty and not already in `existing_keys`, return it.
/// - Non-ASCII labels, empty labels, or if candidate is already in `existing_keys`:
///   Generate `g1`, `g2`, ... avoiding existing keys.
pub fn generate_group_key<S: AsRef<str>, I: IntoIterator<Item = S>>(
    label: &str,
    existing_keys: I,
) -> String {
    let keys: std::collections::HashSet<String> = existing_keys
        .into_iter()
        .map(|s| s.as_ref().to_string())
        .collect();
    let trimmed = label.trim();
    if trimmed.is_ascii() && !trimmed.is_empty() {
        let lower = trimmed.to_ascii_lowercase();
        let mut candidate = String::with_capacity(lower.len());
        let mut prev_hyphen = false;
        for c in lower.chars() {
            if matches!(c, 'a'..='z' | '0'..='9' | '_' | '-') {
                candidate.push(c);
                prev_hyphen = c == '-';
            } else if !prev_hyphen {
                candidate.push('-');
                prev_hyphen = true;
            }
        }
        let trimmed_cand = candidate.trim_matches('-');
        let mut candidate_str = trimmed_cand.to_string();
        if candidate_str.len() > 40 {
            candidate_str.truncate(40);
            candidate_str = candidate_str.trim_end_matches('-').to_string();
        }
        if !candidate_str.is_empty() && !keys.contains(&candidate_str) {
            return candidate_str;
        }
    }
    let mut idx = 1;
    loop {
        let key = format!("g{idx}");
        if !keys.contains(&key) {
            return key;
        }
        idx += 1;
    }
}

#[cfg(unix)]
#[derive(Debug, Default, Serialize, serde::Deserialize)]
struct AutoAnswersState {
    pub last_log_offset: u64,
    #[serde(default)]
    pub triggered_counts: std::collections::BTreeMap<usize, usize>,
}

#[cfg(unix)]
impl Loaded {
    pub(crate) fn check_auto_answers(&self, p: &Resolved) -> Result<()> {
        if p.auto_answers.is_empty() {
            return Ok(());
        }
        if tracked_pids(p).is_empty() {
            return Ok(());
        }
        if !p.log_file.exists() {
            return Ok(());
        }

        let state_path = p.log_dir.join(format!("{}.answers.json", p.id));
        let mut state: AutoAnswersState = if state_path.exists() {
            std::fs::read_to_string(&state_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            AutoAnswersState::default()
        };

        use std::fs::File;
        use std::io::{Read, Seek, SeekFrom};

        let mut file = match File::open(&p.log_file) {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        let file_len = match file.metadata() {
            Ok(m) => m.len(),
            Err(_) => return Ok(()),
        };

        if file_len <= state.last_log_offset {
            return Ok(());
        }

        if file.seek(SeekFrom::Start(state.last_log_offset)).is_err() {
            return Ok(());
        }

        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return Ok(());
        }

        state.last_log_offset = file_len;

        let new_text = String::from_utf8_lossy(&buf);

        let filtered_lines: Vec<&str> = new_text
            .lines()
            .filter(|line| !line.starts_with("› ["))
            .collect();
        if filtered_lines.is_empty() {
            if let Ok(json) = serde_json::to_string(&state) {
                let _ = std::fs::write(&state_path, json);
            }
            return Ok(());
        }
        let scan_text = filtered_lines.join("\n");

        for (idx, rule) in p.auto_answers.iter().enumerate() {
            let triggered_count = state.triggered_counts.get(&idx).copied().unwrap_or(0);
            if rule.once && triggered_count > 0 {
                continue;
            }

            let matched = if rule.regex {
                match regex::Regex::new(&rule.expect) {
                    Ok(re) => re.is_match(&scan_text),
                    Err(_) => false,
                }
            } else {
                scan_text.contains(&rule.expect)
            };

            if matched {
                let to_send = format!("{}\n", rule.send);
                let echo = format!("› [自动应答：第 {} 条规则]", idx + 1);
                if let Err(e) = self.send_input_internal(&p.id, &to_send, Some(echo)) {
                    eprintln!("auto_answer failed to send input for `{}`: {e}", p.id);
                } else {
                    *state.triggered_counts.entry(idx).or_insert(0) += 1;
                }
            }
        }

        if let Ok(json) = serde_json::to_string(&state) {
            let _ = std::fs::write(&state_path, json);
        }

        Ok(())
    }
}

#[cfg(not(unix))]
impl Loaded {
    pub(crate) fn check_auto_answers(&self, _p: &Resolved) -> Result<()> {
        Ok(())
    }
}

impl Loaded {
    pub fn status(&self, id: &str) -> Result<StatusJson> {
        let base = self.resolve(id)?;
        let active_prof = {
            let pid = read_pid(&base.pid_file);
            let alive = pid.is_some_and(process_alive);
            if alive
                || read_start_marker(&base.pid_file).is_some()
                || install_in_progress(&base.pid_file)
            {
                std::fs::read_to_string(profile_marker_path(&base.pid_file))
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        };
        let project = if let Some(ref prof) = active_prof {
            self.resolve_with_profile(id, Some(prof)).unwrap_or(base)
        } else {
            base
        };
        let _ = self.check_auto_answers(&project);
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
        self.start_detached_with_profile(id, None)
    }

    pub fn start_detached_with_profile(
        &self,
        id: &str,
        profile_name: Option<&str>,
    ) -> Result<StatusJson> {
        let project = self.resolve_with_profile(id, profile_name)?;
        if !project.enabled {
            let why = if project.reason.is_empty() {
                "disabled"
            } else {
                project.reason.as_str()
            };
            bail!("project `{}` is disabled: {why}", project.id);
        }

        if !project.depends_on.is_empty() {
            for dep_id in &project.depends_on {
                let dep_proj = self
                    .config
                    .projects
                    .get(dep_id)
                    .with_context(|| format!("项目 `{id}` 的依赖项 `{dep_id}` 不存在"))?;
                if !dep_proj.enabled {
                    bail!("项目 `{id}` 的依赖项 `{dep_id}` 已停用，无法启动");
                }
                let st = self.status(dep_id)?;
                if !matches!(st.state, LifeState::Ready | LifeState::PortOpen) {
                    let _ = self.start_detached_with_profile(dep_id, None)?;
                }
            }
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

        let prof_marker = profile_marker_path(&project.pid_file);
        let effective_prof = profile_name.or(self
            .config
            .projects
            .get(id)
            .and_then(|p| p.default_profile.as_deref()));
        if let Some(prof) = effective_prof {
            let _ = std::fs::write(&prof_marker, prof);
        } else {
            let _ = std::fs::remove_file(&prof_marker);
        }

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
        self.start_with_profile(id, None, timeout)
    }

    pub fn start_with_profile(
        &self,
        id: &str,
        profile_name: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<StatusJson> {
        let mut visited = Vec::new();
        self.start_with_profile_internal(id, profile_name, timeout, &mut visited)
    }

    fn start_with_profile_internal(
        &self,
        id: &str,
        profile_name: Option<&str>,
        timeout: Option<Duration>,
        visited: &mut Vec<String>,
    ) -> Result<StatusJson> {
        if visited.iter().any(|v| v == id) {
            visited.push(id.to_string());
            bail!("检测到循环依赖: {}", visited.join(" -> "));
        }
        visited.push(id.to_string());

        let p = self
            .config
            .projects
            .get(id)
            .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))?;

        if !p.depends_on.is_empty() {
            for dep_id in &p.depends_on {
                let dep_proj = self
                    .config
                    .projects
                    .get(dep_id)
                    .with_context(|| format!("项目 `{id}` 的依赖项 `{dep_id}` 不存在"))?;
                if !dep_proj.enabled {
                    bail!("项目 `{id}` 的依赖项 `{dep_id}` 已停用，无法启动");
                }
                let st = self.status(dep_id)?;
                if !matches!(st.state, LifeState::Ready | LifeState::PortOpen) {
                    self.start_with_profile_internal(dep_id, None, timeout, visited)
                        .with_context(|| format!("启动项目 `{id}` 的依赖项 `{dep_id}` 失败"))?;
                }
            }
        }

        let initial = self.start_detached_with_profile(id, profile_name)?;
        let budget = timeout.unwrap_or_else(|| Duration::from_secs(initial.timeout_secs));
        let res = self.finish_start(id, initial, budget);
        visited.pop();
        res
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
        let project = self.resolve(id)?;
        loop {
            let _ = self.check_auto_answers(&project);
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
        self.restart_detached_with_profile(id, None)
    }

    pub fn restart_detached_with_profile(
        &self,
        id: &str,
        profile_name: Option<&str>,
    ) -> Result<StatusJson> {
        let project = self.resolve_with_profile(id, profile_name)?;
        if !project.enabled {
            bail!("project `{id}` is disabled: {}", project.reason);
        }
        self.stop(id)?;
        self.start_detached_with_profile(id, profile_name)
    }

    /// Restart and wait for readiness, as with `start`.
    pub fn restart(&self, id: &str) -> Result<StatusJson> {
        self.restart_with_timeout(id, None)
    }

    pub fn restart_with_timeout(&self, id: &str, timeout: Option<Duration>) -> Result<StatusJson> {
        self.restart_with_profile(id, None, timeout)
    }

    pub fn restart_with_profile(
        &self,
        id: &str,
        profile_name: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<StatusJson> {
        let initial = self.restart_detached_with_profile(id, profile_name)?;
        let budget = timeout.unwrap_or_else(|| Duration::from_secs(initial.timeout_secs));
        self.finish_start(id, initial, budget)
    }

    pub fn stop(&self, id: &str) -> Result<StatusJson> {
        let project = self.resolve(id)?;
        cancel_install(&project)?;
        let _ = std::fs::remove_file(profile_marker_path(&project.pid_file));
        cmd_stop(&project)?;
        let peers = self.peer_pids(&project.id);
        Ok(status_of(&project, &peers))
    }

    /// 批量启动项目（阻塞等待就绪）
    pub fn start_batch(
        &self,
        group: Option<&str>,
        ids: Option<&[String]>,
        timeout: Option<Duration>,
    ) -> Result<BatchOutput> {
        let candidate_ids: Vec<String> = if let Some(ids) = ids {
            ids.iter()
                .filter(|id| self.config.projects.contains_key(*id))
                .cloned()
                .collect()
        } else if let Some(g) = group {
            self.projects_in_group(g)
        } else {
            self.config.projects.keys().cloned().collect()
        };

        let mut to_run = Vec::new();
        for id in &candidate_ids {
            if let Ok(resolved) = self.resolve(id) {
                let configured = resolved.port > 0
                    && (!resolved.active_start().is_empty() || resolved.tomcat.is_some());
                if resolved.enabled && configured {
                    to_run.push(id.as_str());
                }
            }
        }

        let sorted = self.sorted_projects_for_start(&to_run);
        let mut results = Vec::new();
        let mut failed_ids = std::collections::HashSet::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for id in &sorted {
            let p = match self.config.projects.get(id) {
                Some(p) => p,
                None => continue,
            };
            if p.depends_on.iter().any(|d| failed_ids.contains(d)) {
                failed += 1;
                failed_ids.insert(id.clone());
                results.push(self.skipped_dep_item(id));
                continue;
            }

            match self.start_with_profile(id, None, timeout) {
                Ok(st) => {
                    succeeded += 1;
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: true,
                        ok: true,
                        status: Some(st),
                        error: None,
                    });
                }
                Err(err) => {
                    failed += 1;
                    failed_ids.insert(id.clone());
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: false,
                        ok: false,
                        status: self.status(id).ok(),
                        error: Some(format!("{err:#}")),
                    });
                }
            }
        }

        Ok(BatchOutput {
            total: sorted.len(),
            succeeded,
            failed,
            results,
        })
    }

    /// 批量启动项目（后台脱离运行，立刻返回，供 GUI/网页端调用）
    pub fn start_batch_detached(
        &self,
        group: Option<&str>,
        ids: Option<&[String]>,
    ) -> Result<BatchOutput> {
        let candidate_ids: Vec<String> = if let Some(ids) = ids {
            ids.iter()
                .filter(|id| self.config.projects.contains_key(*id))
                .cloned()
                .collect()
        } else if let Some(g) = group {
            self.projects_in_group(g)
        } else {
            self.config.projects.keys().cloned().collect()
        };

        let mut to_run = Vec::new();
        for id in &candidate_ids {
            if let Ok(resolved) = self.resolve(id) {
                let configured = resolved.port > 0
                    && (!resolved.active_start().is_empty() || resolved.tomcat.is_some());
                if resolved.enabled && configured {
                    to_run.push(id.as_str());
                }
            }
        }

        let sorted = self.sorted_projects_for_start(&to_run);
        let mut results = Vec::new();
        let mut failed_ids = std::collections::HashSet::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for id in &sorted {
            let depends_on = self
                .config
                .projects
                .get(id)
                .map(|p| p.depends_on.clone())
                .unwrap_or_default();
            if depends_on.iter().any(|d| failed_ids.contains(d)) {
                failed += 1;
                failed_ids.insert(id.clone());
                results.push(self.skipped_dep_item(id));
                continue;
            }
            match self.start_detached_with_profile(id, None) {
                Ok(st) if st.state == LifeState::Failed => {
                    failed += 1;
                    failed_ids.insert(id.clone());
                    let error = st.failure.clone().unwrap_or_else(|| "启动失败".to_string());
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: false,
                        ok: false,
                        status: Some(st),
                        error: Some(error),
                    });
                }
                Ok(st) => {
                    succeeded += 1;
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: true,
                        ok: true,
                        status: Some(st),
                        error: None,
                    });
                }
                Err(err) => {
                    failed += 1;
                    failed_ids.insert(id.clone());
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: false,
                        ok: false,
                        status: self.status(id).ok(),
                        error: Some(format!("{err:#}")),
                    });
                }
            }
        }

        Ok(BatchOutput {
            total: sorted.len(),
            succeeded,
            failed,
            results,
        })
    }

    /// 批量停止项目（按依赖逆序停止）
    pub fn stop_batch(&self, group: Option<&str>, ids: Option<&[String]>) -> Result<BatchOutput> {
        let candidate_ids: Vec<String> = if let Some(ids) = ids {
            ids.iter()
                .filter(|id| self.config.projects.contains_key(*id))
                .cloned()
                .collect()
        } else if let Some(g) = group {
            self.projects_in_group(g)
        } else {
            self.config.projects.keys().cloned().collect()
        };

        let mut to_run = Vec::new();
        for id in &candidate_ids {
            if let Ok(resolved) = self.resolve(id) {
                if resolved.enabled {
                    to_run.push(id.as_str());
                }
            }
        }

        let sorted = self.sorted_projects_for_stop(&to_run);
        let mut results = Vec::new();
        let mut succeeded = 0;
        let mut failed = 0;

        for id in &sorted {
            match self.stop(id) {
                Ok(st) => {
                    succeeded += 1;
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: true,
                        ok: true,
                        status: Some(st),
                        error: None,
                    });
                }
                Err(err) => {
                    failed += 1;
                    results.push(BatchItemResult {
                        id: id.clone(),
                        success: false,
                        ok: false,
                        status: self.status(id).ok(),
                        error: Some(format!("{err:#}")),
                    });
                }
            }
        }

        Ok(BatchOutput {
            total: sorted.len(),
            succeeded,
            failed,
            results,
        })
    }

    fn skipped_dep_item(&self, id: &str) -> BatchItemResult {
        BatchItemResult {
            id: id.to_string(),
            success: false,
            ok: false,
            status: self.status(id).ok(),
            error: Some(DEP_START_SKIPPED.to_string()),
        }
    }

    /// 批量重启项目（先逆序全部停止，再正序启动）
    pub fn restart_batch(
        &self,
        group: Option<&str>,
        ids: Option<&[String]>,
        timeout: Option<Duration>,
    ) -> Result<BatchOutput> {
        let _ = self.stop_batch(group, ids)?;
        self.start_batch(group, ids, timeout)
    }

    /// 批量重启项目（后台脱离模式）
    pub fn restart_batch_detached(
        &self,
        group: Option<&str>,
        ids: Option<&[String]>,
    ) -> Result<BatchOutput> {
        let _ = self.stop_batch(group, ids)?;
        self.start_batch_detached(group, ids)
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

    pub const MAX_INPUT_BYTES: usize = MAX_INPUT_BYTES;

    /// 向运行中的项目标准输入管道写入数据。
    pub fn send_input(&self, id: &str, data: &str) -> Result<()> {
        self.send_input_internal(id, data, None)
    }

    pub(crate) fn send_input_internal(
        &self,
        id: &str,
        data: &str,
        custom_echo: Option<String>,
    ) -> Result<()> {
        #[cfg(not(unix))]
        {
            let _ = id;
            let _ = data;
            let _ = custom_echo;
            bail!("Windows 暂不支持向进程发送输入");
        }
        #[cfg(unix)]
        {
            if data.len() > MAX_INPUT_BYTES {
                bail!("单次输入不能超过 4096 字节");
            }
            let p = self.resolve(id)?;
            if tracked_pids(&p).is_empty() {
                bail!("项目 `{id}` 未在运行");
            }
            let fifo_path = p.stdin_fifo_path();
            if !fifo_path.exists() {
                bail!("项目 `{id}` 输入管道不存在");
            }
            use std::ffi::CString;
            use std::io::Write;
            use std::os::unix::ffi::OsStrExt;
            use std::os::unix::io::FromRawFd;

            let c_path = CString::new(fifo_path.as_os_str().as_bytes())
                .with_context(|| format!("convert path to cstring: {}", fifo_path.display()))?;

            let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
            if fd < 0 {
                let err = std::io::Error::last_os_error();
                bail!("无法写入输入管道（打开失败）：{err}");
            }
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            if let Err(e) = file.write_all(data.as_bytes()) {
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.raw_os_error() == Some(libc::EAGAIN)
                    || e.raw_os_error() == Some(libc::EWOULDBLOCK)
                {
                    bail!("子进程没有读取输入（管道已满）");
                } else {
                    bail!("向项目 `{id}` 写入输入失败: {e}");
                }
            }

            if !p.pty {
                let echo_text = if let Some(echo) = custom_echo {
                    echo
                } else {
                    match data {
                        "\x1b[A" => "› [按键 up]".to_string(),
                        "\x1b[B" => "› [按键 down]".to_string(),
                        "\x1b[C" => "› [按键 right]".to_string(),
                        "\x1b[D" => "› [按键 left]".to_string(),
                        "\t" => "› [按键 tab]".to_string(),
                        "\x1b" => "› [按键 esc]".to_string(),
                        "\x03" => "› [按键 ctrl-c]".to_string(),
                        "\n" | "\r" => "› [按键 enter]".to_string(),
                        other => {
                            let count = other.chars().count();
                            format!("› [已发送 {count} 个字符]")
                        }
                    }
                };
                append_log_line(&p.log_file, &echo_text);
            }

            Ok(())
        }
    }

    fn resolve(&self, id: &str) -> Result<Resolved> {
        self.resolve_with_profile(id, None)
    }

    pub(crate) fn resolve_with_profile(
        &self,
        id: &str,
        profile_name: Option<&str>,
    ) -> Result<Resolved> {
        let project = self
            .config
            .projects
            .get(id)
            .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))?;
        resolve_project_with_profile(
            id,
            project,
            current_home().as_deref(),
            profile_name,
            &self.config.log_dir,
        )
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
pub const MAX_INPUT_BYTES: usize = 4096;

pub fn map_key(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().as_str() {
        "up" => Some("\x1b[A"),
        "down" => Some("\x1b[B"),
        "right" => Some("\x1b[C"),
        "left" => Some("\x1b[D"),
        "enter" => Some("\n"),
        "tab" => Some("\t"),
        "esc" | "escape" => Some("\x1b"),
        "ctrl-c" | "ctrl_c" | "ctrlc" => Some("\x03"),
        _ => None,
    }
}

pub fn resolve_input_data(text: Option<&str>, enter: bool, key: Option<&str>) -> Result<String> {
    let mut data = String::new();
    let has_key = key.is_some();
    if let Some(t) = text {
        data.push_str(t);
        if enter && !has_key {
            data.push('\n');
        }
    } else if enter && !has_key {
        data.push('\n');
    }
    if let Some(k) = key {
        let mapped = map_key(k).ok_or_else(|| anyhow::anyhow!("未知按键: {k}"))?;
        data.push_str(mapped);
    }
    if data.is_empty() {
        bail!("empty input: specify text or key");
    }
    Ok(data)
}

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
    p.to_string_lossy().replace('\\', "/")
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
    let out = s
        .replace("{{root}}", &vars.root)
        .replace("{{log_dir}}", &vars.log_dir);
    // An empty tomcat must not erase the placeholder. `{{tomcat}}/bin/catalina.sh`
    // would otherwise become `/bin/catalina.sh`.
    if vars.tomcat.is_empty() {
        out
    } else {
        out.replace("{{tomcat}}", &vars.tomcat)
    }
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
    if !path.is_absolute() && !is_windows_absolute(&expanded) && !expanded.starts_with('/') {
        bail!("{label} must be an absolute path after expansion, got `{expanded}`");
    }
    Ok(path)
}

fn render_token(raw: &str, vars: &Vars, home: Option<&Path>) -> Result<String> {
    if raw.contains("{{tomcat}}") && vars.tomcat.is_empty() {
        bail!("command uses {{{{tomcat}}}} but tomcat is not set");
    }
    let expanded = expand_tilde_with(&subst(raw, vars), home)?;
    if expanded.contains("{{") {
        bail!("unresolved template in `{raw}` -> `{expanded}`");
    }
    Ok(expanded)
}

/// Render a start/stop argv. Enabled projects with `{{tomcat}}` and no tomcat
/// path fail here. Disabled ones keep the placeholder so import can park a WAR
/// until the path is filled, without ever rewriting it to `/bin/catalina.sh`.
fn render_project_argv(
    id: &str,
    label: &str,
    argv: &[String],
    vars: &Vars,
    home: Option<&Path>,
    enabled: bool,
) -> Result<Vec<String>> {
    if argv.is_empty() {
        return Ok(Vec::new());
    }
    let missing_tomcat =
        vars.tomcat.is_empty() && argv.iter().any(|arg| arg.contains("{{tomcat}}"));
    if missing_tomcat {
        if enabled {
            bail!("project `{id}` {label} uses {{{{tomcat}}}} but tomcat is not set");
        }
        return Ok(argv.to_vec());
    }
    render_argv(argv, vars, home)
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

/// Absolute log directory.
///
/// Empty uses the file-level `log_dir` when one was passed in, otherwise
/// `~/.tongyun-dev/logs`. `{{root}}` and `{{tomcat}}` are replaced with the
/// already-expanded paths. A relative value is placed under the project root
/// (`logs` → `<root>/logs`). A relative value that contains a `..` segment is
/// rejected so it cannot leave the project root. `\` is treated as a separator
/// here, the same way `path_to_string` rewrites it to `/`, so `..\x` cannot
/// slip past the check. Absolute paths are kept as written. One project's
/// log_dir cannot fail the whole catalog.
fn resolve_log_dir(
    raw: &str,
    root: &Path,
    tomcat: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf> {
    let raw = if raw.trim().is_empty() {
        DEFAULT_LOG_DIR.to_string()
    } else {
        raw.trim().to_string()
    };
    if raw.contains("{{tomcat}}") && tomcat.is_none() {
        bail!("log_dir uses {{{{tomcat}}}} but tomcat is not set");
    }
    let prelim = Vars {
        root: path_to_string(root),
        tomcat: tomcat.map(path_to_string).unwrap_or_default(),
        log_dir: String::new(),
    };
    let substituted = subst(&raw, &prelim);
    if substituted.contains("{{") {
        bail!("log_dir has an unresolved template: {substituted}");
    }
    let expanded = expand_tilde_with(&substituted, home)?;
    let path = PathBuf::from(&expanded);
    if path.is_absolute() || is_windows_absolute(&expanded) || expanded.starts_with('/') {
        return Ok(path);
    }
    // `path_to_string` rewrites `\` to `/` after this returns. Check that form
    // so a relative `..\x` cannot become `../x` outside the project root.
    let normalized = expanded.replace('\\', "/");
    if normalized.split('/').any(|seg| seg == "..") {
        bail!("relative log_dir cannot contain `..` (got `{expanded}`)");
    }
    Ok(root.join(normalized))
}

#[cfg(test)]
fn resolve_project(id: &str, p: &Project, home: Option<&Path>) -> Result<Resolved> {
    resolve_project_with_log_default(id, p, home, "")
}

fn resolve_project_with_log_default(
    id: &str,
    p: &Project,
    home: Option<&Path>,
    default_log_dir: &str,
) -> Result<Resolved> {
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
        default_log_dir
    } else {
        p.log_dir.as_str()
    };
    let log_dir = resolve_log_dir(log_dir_raw, &root, tomcat.as_deref(), home)?;
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
    let start = render_project_argv(id, "start", &p.start, &vars, home, p.enabled)?;
    let stop = render_project_argv(id, "stop", &p.stop, &vars, home, p.enabled)?;
    let start_windows = render_project_argv(
        id,
        "start_windows",
        &p.start_windows,
        &vars,
        home,
        p.enabled,
    )?;
    let stop_windows =
        render_project_argv(id, "stop_windows", &p.stop_windows, &vars, home, p.enabled)?;
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
    let port_mode = match p
        .port_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => {
            let m = s.to_ascii_lowercase();
            if !matches!(m.as_str(), "none" | "arg" | "env" | "config") {
                bail!("port_mode must be one of none, arg, env, config, got `{s}`");
            }
            m
        }
        None => "none".to_string(),
    };
    let port_arg = p
        .port_arg
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let port_env = p
        .port_env
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
        group: p.group.trim().to_string(),
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
        port_mode,
        port_arg,
        port_env,
        env: p.env.clone(),
        pty: p.pty,
        auto_answers: p.auto_answers.clone(),
        links: p.links.clone(),
        depends_on: p.depends_on.clone(),
    })
}

fn resolve_project_with_profile(
    id: &str,
    p: &Project,
    home: Option<&Path>,
    profile_name: Option<&str>,
    default_log_dir: &str,
) -> Result<Resolved> {
    let mut project = p.clone();
    let effective_profile = profile_name.or(project.default_profile.as_deref());
    if let Some(name) = effective_profile {
        if let Some(prof) = project.profiles.get(name) {
            if let Some(ref start_cmd) = prof.start {
                let trimmed = start_cmd.trim();
                if !trimmed.is_empty() {
                    project.start = crate::toolchain::shell_argv(trimmed, project.shell.as_deref());
                }
            }
            if let Some(port) = prof.port {
                if port > 0 {
                    let old_port_str = format!(":{}", project.port);
                    let new_port_str = format!(":{}", port);
                    if !project.login_url.is_empty() {
                        project.login_url = project.login_url.replace(&old_port_str, &new_port_str);
                    }
                    if !project.health_url.is_empty() {
                        project.health_url =
                            project.health_url.replace(&old_port_str, &new_port_str);
                    }
                    project.port = port;
                }
            }
            for (k, v) in &prof.env {
                project.env.insert(k.clone(), v.clone());
            }
        } else {
            bail!("unknown profile `{name}` for project `{id}`");
        }
    }
    resolve_project_with_log_default(id, &project, home, default_log_dir)
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

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    if pid <= 1 {
        return false;
    }
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map(|o| {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains(&pid.to_string())
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
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

#[cfg(windows)]
fn win_proc_info(target_pid: u32) -> (Option<String>, Option<u32>) {
    use std::ffi::c_void;

    type HANDLE = *mut c_void;
    type BOOL = i32;
    type DWORD = u32;

    const INVALID_HANDLE_VALUE: HANDLE = -1isize as *mut c_void;
    const TH32CS_SNAPPROCESS: DWORD = 0x00000002;

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PROCESSENTRY32W {
        dwSize: DWORD,
        cntUsage: DWORD,
        th32ProcessID: DWORD,
        th32DefaultHeapID: usize,
        th32ModuleID: DWORD,
        cntThreads: DWORD,
        th32ParentProcessID: DWORD,
        pcPriClassBase: i32,
        dwFlags: DWORD,
        szExeFile: [u16; 260],
    }

    extern "system" {
        fn CreateToolhelp32Snapshot(dwFlags: DWORD, th32ProcessID: DWORD) -> HANDLE;
        fn Process32FirstW(hSnapshot: HANDLE, lppe: *mut PROCESSENTRY32W) -> BOOL;
        fn Process32NextW(hSnapshot: HANDLE, lppe: *mut PROCESSENTRY32W) -> BOOL;
        fn CloseHandle(hObject: HANDLE) -> BOOL;
    }

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return (None, None);
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as DWORD,
            cntUsage: 0,
            th32ProcessID: 0,
            th32DefaultHeapID: 0,
            th32ModuleID: 0,
            cntThreads: 0,
            th32ParentProcessID: 0,
            pcPriClassBase: 0,
            dwFlags: 0,
            szExeFile: [0; 260],
        };
        let mut found_name = None;
        let mut found_parent = None;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ProcessID == target_pid {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        found_name = Some(trimmed.to_string());
                    }
                    if entry.th32ParentProcessID > 0 {
                        found_parent = Some(entry.th32ParentProcessID);
                    }
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        (found_name, found_parent)
    }
}

#[cfg(windows)]
fn ps_field(pid: u32, field: &str) -> Option<String> {
    let (name, parent) = win_proc_info(pid);
    if field.starts_with("ppid") {
        return parent.map(|p| p.to_string());
    }
    if let Some(n) = name {
        return Some(n);
    }
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?;
    let name = first.split(',').next()?.trim_matches('"');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(not(windows))]
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

#[cfg(windows)]
fn listener_pids(port: u16) -> Vec<u32> {
    if port == 0 {
        return Vec::new();
    }
    let out = Command::new("netstat")
        .args(["-ano"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(out) = out else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut pids = Vec::new();
    let port_suffix = format!(":{port}");
    for line in text.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5
            && parts[0].eq_ignore_ascii_case("TCP")
            && parts[3].eq_ignore_ascii_case("LISTENING")
        {
            let local_addr = parts[1];
            if local_addr.ends_with(&port_suffix) {
                let local_port_str = local_addr.rsplit(':').next().unwrap_or("");
                if let Ok(p) = local_port_str.parse::<u16>() {
                    if p == port {
                        if let Ok(pid) = parts[4].parse::<u32>() {
                            if pid > 0 && !pids.contains(&pid) {
                                pids.push(pid);
                            }
                        }
                    }
                }
            }
        }
    }
    pids.sort_unstable();
    pids
}

#[cfg(not(windows))]
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
    if let Ok(out) = out {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            let mut pids = Vec::new();
            for line in text.lines() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    if pid > 1 && !pids.contains(&pid) {
                        pids.push(pid);
                    }
                }
            }
            if !pids.is_empty() {
                pids.sort_unstable();
                return pids;
            }
        }
    }
    // Fallback to ss on Linux if lsof is absent or empty
    let out_ss = Command::new("ss")
        .args(["-H", "-t", "-l", "-n", "-p", &format!("sport = :{port}")])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    if let Ok(out_ss) = out_ss {
        let text = String::from_utf8_lossy(&out_ss.stdout);
        let mut pids = Vec::new();
        for line in text.lines() {
            let mut rest = line;
            while let Some(idx) = rest.find("pid=") {
                rest = &rest[idx + 4..];
                let pid_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid > 1 && !pids.contains(&pid) {
                        pids.push(pid);
                    }
                }
            }
        }
        if !pids.is_empty() {
            pids.sort_unstable();
            return pids;
        }
    }
    Vec::new()
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

pub fn profile_marker_path(pid_file: &Path) -> PathBuf {
    let mut p = pid_file.as_os_str().to_os_string();
    p.push(".profile");
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

fn legacy_pid_paths(p: &Resolved) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let root_logs = p.root.join("logs").join(format!("{}.pid", p.id));
    if root_logs != p.pid_file {
        candidates.push(root_logs);
    }
    if let Some(home) = current_home() {
        let cfg_logs = home
            .join(".config")
            .join("tongyun-dev")
            .join("logs")
            .join(format!("{}.pid", p.id));
        if cfg_logs != p.pid_file && !candidates.contains(&cfg_logs) {
            candidates.push(cfg_logs);
        }
    }
    candidates
}

fn status_of(p: &Resolved, peers: &[(String, u32)]) -> StatusJson {
    let own = tracked_pids(p);
    let open = port_open(p.port);
    let (port_conflict, port_holder) = port_conflict_of(p, &own, peers, open);
    let pid = own
        .into_iter()
        .next()
        .or_else(|| read_pid(&p.pid_file).filter(|&pid| process_alive(pid)));
    let is_alive = pid.is_some();
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

    let (mut state, mut failure) = derive_state(StateInput {
        process_alive: is_alive,
        port_open: open,
        port_conflict,
        has_probe,
        probe_ok: probe.ok,
        started_at,
        now,
        timeout_secs,
        start_error,
    });

    let mut hints = Vec::new();
    if let Some(h) = diagnose_start_command(&p.root, &p.start) {
        hints.push(h);
    }
    let has_unknown_port_err = !is_alive && check_log_unknown_port_error(&p.log_file);
    if has_unknown_port_err
        && (state == LifeState::Failed || state == LifeState::Starting || marker.is_some())
    {
        state = LifeState::Failed;
        let port_advice =
            "启动脚本不认 --port 参数；建议去掉 --port，端口传递方式选「不传」".to_string();
        if let Some(ref mut fail_msg) = failure {
            if !fail_msg.contains("建议去掉 --port") {
                fail_msg.push_str(&format!("：{port_advice}"));
            }
        } else {
            failure = Some(format!("进程已退出：{port_advice}"));
        }
        if !hints.contains(&port_advice) {
            hints.push(port_advice);
        }
    }

    if !is_alive {
        for legacy_path in legacy_pid_paths(p) {
            if let Some(lpid) = read_pid(&legacy_path) {
                if process_alive(lpid) {
                    let hint = format!(
                        "检测到旧版 PID 文件 ({}) 包含存活进程 PID {}。可使用 stop 停止，或设置 log_dir 沿用旧路径",
                        path_to_string(&legacy_path),
                        lpid
                    );
                    if !hints.contains(&hint) {
                        hints.push(hint);
                    }
                }
            }
        }
    }

    if state == LifeState::Stopped && !is_alive {
        let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
        let _ = std::fs::remove_file(profile_marker_path(&p.pid_file));
    }

    let mut started_at = started_at;
    if !matches!(state, LifeState::Ready | LifeState::PortOpen) {
        if let Some((st, why, at)) = install_overlay(read_install_marker(&p.pid_file)) {
            state = st;
            failure = why;
            started_at = Some(at);
        }
    }

    if state == LifeState::Failed
        && failure
            .as_deref()
            .map(failure_allows_log_line)
            .unwrap_or(true)
    {
        if let Ok(text) = read_tail(&p.log_file, 120) {
            if let Some(line) = extract_start_failure_line(&text) {
                failure = Some(line);
            }
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

    let active_profile = if state != LifeState::Stopped {
        std::fs::read_to_string(profile_marker_path(&p.pid_file))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

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
        hints,
        active_profile,
        resolve_error: None,
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

fn has_port_flag(toks: &[&str], flag: &str) -> bool {
    toks.iter().any(|t| {
        *t == flag
            || *t == "--port"
            || *t == "-p"
            || t.starts_with(&format!("{flag}="))
            || t.starts_with("--port=")
            || t.starts_with("-p=")
    })
}

fn append_port_to_words(line: &str, port: u16, flag: &str) -> String {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.is_empty() {
        return line.to_string();
    }
    if has_port_flag(&toks, flag) {
        return line.to_string();
    }

    let is_npm_run = toks.windows(2).any(|w| w[0] == "npm" && w[1] == "run");
    let has_double_dash = toks.contains(&"--");

    let mut out = line.trim_end().to_string();
    if is_npm_run {
        if has_double_dash {
            out.push_str(&format!(" {flag} {port}"));
        } else {
            out.push_str(&format!(" -- {flag} {port}"));
        }
    } else {
        out.push_str(&format!(" {flag} {port}"));
    }
    out
}

pub fn append_port_to_script(script: &str, port: u16, flag: &str) -> String {
    let lines: Vec<&str> = script.split('\n').collect();
    if lines.is_empty() {
        return script.to_string();
    }

    let all_toks: Vec<&str> = script.split_whitespace().collect();
    if has_port_flag(&all_toks, flag) {
        return script.to_string();
    }

    let mut target_idx = None;
    for (i, line) in lines.iter().enumerate().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with("exec ") || trimmed.starts_with("exec\t") {
            target_idx = Some(i);
            break;
        }
    }
    if target_idx.is_none() {
        for (i, line) in lines.iter().enumerate().rev() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                target_idx = Some(i);
                break;
            }
        }
    }
    let target_idx = target_idx.unwrap_or(lines.len().saturating_sub(1));

    let mut result = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i == target_idx {
            result.push(append_port_to_words(line, port, flag));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

pub fn append_port_to_argv(argv: &[String], port: u16, flag: &str) -> Vec<String> {
    if argv.is_empty() {
        return Vec::new();
    }
    let toks: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    if has_port_flag(&toks, flag) {
        return argv.to_vec();
    }

    let mut out = argv.to_vec();
    let is_npm_run = argv.len() >= 2 && argv[0] == "npm" && argv[1] == "run";
    let has_double_dash = argv.iter().any(|s| s == "--");

    if is_npm_run {
        if !has_double_dash {
            out.push("--".to_string());
        }
        out.push(flag.to_string());
        out.push(port.to_string());
    } else {
        out.push(flag.to_string());
        out.push(port.to_string());
    }
    out
}

/// 自动在命令末尾追加端口参数。
/// pnpm/yarn → ` --port N`；npm run → ` -- --port N`（命令已有 `--` 就不再加）；其他命令直接追加 ` --port N`。
/// 若命令里已指定 --port / -p / flag 则不重复追加。
pub fn append_port_arg(cmd: &[String], port: u16, port_arg: Option<&str>) -> Vec<String> {
    if cmd.is_empty() {
        return Vec::new();
    }
    let flag = port_arg.unwrap_or("--port");

    if cmd.len() == 3 && (cmd[0] == "bash" || cmd[0] == "sh") && (cmd[1] == "-c" || cmd[1] == "-lc")
    {
        let mut out = cmd.to_vec();
        out[2] = append_port_to_script(&cmd[2], port, flag);
        return out;
    }
    if cmd.len() == 3
        && cmd[0].eq_ignore_ascii_case("cmd")
        && (cmd[1].eq_ignore_ascii_case("/c") || cmd[1].eq_ignore_ascii_case("-c"))
    {
        let mut out = cmd.to_vec();
        out[2] = append_port_to_script(&cmd[2], port, flag);
        return out;
    }
    if cmd.len() == 6
        && cmd[0].eq_ignore_ascii_case("powershell")
        && cmd[4].eq_ignore_ascii_case("-command")
    {
        let mut out = cmd.to_vec();
        out[5] = append_port_to_script(&cmd[5], port, flag);
        return out;
    }
    if cmd.len() == 3
        && cmd[0].eq_ignore_ascii_case("powershell")
        && (cmd[1].eq_ignore_ascii_case("-command") || cmd[1].eq_ignore_ascii_case("-c"))
    {
        let mut out = cmd.to_vec();
        out[2] = append_port_to_script(&cmd[2], port, flag);
        return out;
    }

    append_port_to_argv(cmd, port, flag)
}

fn extract_script_name(cmd: &[String]) -> Option<String> {
    let all_tokens: Vec<&str> = cmd.iter().flat_map(|s| s.split_whitespace()).collect();

    for (i, t) in all_tokens.iter().enumerate() {
        if is_pnpm_token(t) || *t == "yarn" || t.ends_with("/yarn") {
            let mut j = i + 1;
            if all_tokens.get(j) == Some(&"run") {
                j += 1;
            }
            if let Some(script) = all_tokens.get(j) {
                if !script.starts_with('-') && !matches!(*script, "&&" | "||" | ";" | "|" | "exec")
                {
                    return Some(script.to_string());
                }
            }
        } else if *t == "npm" || t.ends_with("/npm") {
            let mut j = i + 1;
            if all_tokens.get(j) == Some(&"run") {
                j += 1;
                if let Some(script) = all_tokens.get(j) {
                    if !script.starts_with('-')
                        && !matches!(*script, "&&" | "||" | ";" | "|" | "exec")
                    {
                        return Some(script.to_string());
                    }
                }
            }
        } else if *t == "node" || t.ends_with("/node") {
            let mut j = i + 1;
            if let Some(entry) = all_tokens.get(j) {
                if entry.ends_with(".js")
                    || entry.ends_with(".ts")
                    || entry.ends_with(".mjs")
                    || entry.ends_with(".cjs")
                {
                    j += 1;
                    if let Some(script) = all_tokens.get(j) {
                        if !script.starts_with('-')
                            && !matches!(*script, "&&" | "||" | ";" | "|" | "exec")
                        {
                            return Some(script.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// 诊断启动命令：若 start 写死了 `--port` 且为 monorepo 启动脚本，给出警告
pub fn diagnose_start_command(root: &Path, start_cmd: &[String]) -> Option<String> {
    let has_port_flag = start_cmd.iter().any(|arg| {
        arg.split_whitespace()
            .any(|tok| tok == "--port" || tok.starts_with("--port="))
    });
    if !has_port_flag {
        return None;
    }

    let script_name = extract_script_name(start_cmd).unwrap_or_else(|| "dev".to_string());

    let is_monorepo_root =
        root.join("turbo.json").is_file() || root.join("pnpm-workspace.yaml").is_file();

    let pkg_path = root.join("package.json");
    let is_monorepo = if let Ok(text) = std::fs::read_to_string(&pkg_path) {
        if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&text) {
            let script_body = pkg
                .get("scripts")
                .and_then(|s| s.as_object())
                .and_then(|m| m.get(&script_name).or_else(|| m.get("dev")))
                .and_then(|v| v.as_str());

            if let Some(body) = script_body {
                let b = body.trim();
                let first_word = b.split_whitespace().next().unwrap_or("");
                let is_direct_framework =
                    matches!(first_word, "vite" | "vue-cli-service" | "next" | "nuxt")
                        || b.starts_with("npx vite")
                        || b.starts_with("npx next")
                        || b.starts_with("npx nuxt");

                if is_direct_framework && !is_monorepo_root {
                    false
                } else {
                    first_word == "turbo"
                        || first_word == "turbo-run"
                        || first_word == "nx"
                        || first_word == "lerna"
                        || first_word == "node"
                        || first_word == "ts-node"
                        || first_word == "tsx"
                        || is_monorepo_root
                }
            } else {
                is_monorepo_root
            }
        } else {
            is_monorepo_root
        }
    } else {
        is_monorepo_root
    };

    if is_monorepo {
        Some(format!(
            "start 命令里写了 --port，但 {script_name} 是 monorepo 启动脚本，可能不认 --port；建议去掉 --port，端口传递方式选「不传」"
        ))
    } else {
        None
    }
}

/// Whether a stored failure is generic enough to replace with a log one-liner.
fn failure_allows_log_line(msg: &str) -> bool {
    if msg.contains("建议去掉 --port")
        || msg.contains("缺 Java")
        || msg.contains("缺 Node")
        || msg.contains("依赖未安装")
        || msg.contains("已停用")
        || msg.contains("disabled")
    {
        return false;
    }
    msg == "进程已退出"
        || msg == "启动失败"
        || msg.starts_with("超过 ")
        || msg.starts_with("进程已退出")
}

/// One-line start-failure reason from a log tail.
/// Prefers port-in-use, then dependency resolution, then the last exception.
pub fn extract_start_failure_line(log: &str) -> Option<String> {
    let lines = log_lines(log);
    if let Some(line) = lines.iter().rev().find(|l| is_port_in_use(l)) {
        return Some(prefix_failure("端口被占用", line));
    }
    if let Some(line) = lines.iter().rev().find_map(|l| maven_auth_failure_line(l)) {
        return Some(line);
    }
    if let Some(line) = lines.iter().rev().find(|l| is_dependency_resolve(l)) {
        return Some(prefix_failure("依赖解析失败", line));
    }
    if let Some(line) = lines.iter().rev().find(|l| is_caused_by(l)) {
        return Some(prefix_failure("异常", line));
    }
    if let Some(line) = lines.iter().rev().find(|l| is_exception_line(l)) {
        return Some(prefix_failure("异常", line));
    }
    None
}

fn log_lines(log: &str) -> Vec<String> {
    log.lines()
        .map(|l| collapse_ws(&strip_ansi_line(l)))
        .filter(|l| !l.is_empty())
        .collect()
}

fn strip_ansi_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b && i + 1 < b.len() && b[i + 1] == b'[' {
            i += 2;
            while i < b.len() && !b[i].is_ascii_alphabetic() && b[i] != b'~' {
                i += 1;
            }
            if i < b.len() {
                i += 1;
            }
            continue;
        }
        let ch = s[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn prefix_failure(label: &str, line: &str) -> String {
    let line = if line.chars().count() > 180 {
        let cut: String = line.chars().take(180).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    };
    if line.starts_with(label) {
        line
    } else {
        format!("{label}：{line}")
    }
}

fn is_port_in_use(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("address already in use")
        || lower.contains("port already in use")
        || lower.contains("was already in use")
        || lower.contains("eaddrinuse")
        || lower.contains("bindexception")
        || lower.contains("failed to bind")
        || lower.contains("web server failed to start")
        || line.contains("端口已被占用")
        || line.contains("地址已在使用")
}

/// Maven/Nexus 401/403 is a fixed failure even when the line is too long to keep the code.
fn maven_auth_failure_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let code = if contains_http_code(&lower, "401") {
        "401"
    } else if contains_http_code(&lower, "403") {
        "403"
    } else {
        return None;
    };
    let auth = lower.contains("unauthorized")
        || lower.contains("forbidden")
        || lower.contains("authentication failed")
        || lower.contains("status:")
        || lower.contains("status code")
        || lower.contains("return code is:");
    if !auth {
        return None;
    }
    Some(format!("认证失败：Maven 仓库返回 {code}"))
}

fn contains_http_code(lower: &str, code: &str) -> bool {
    let mut rest = lower;
    while let Some(i) = rest.find(code) {
        let before_ok = i == 0 || !rest.as_bytes()[i - 1].is_ascii_digit();
        let after = i + code.len();
        let after_ok = after >= rest.len() || !rest.as_bytes()[after].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        let next = i + code.len();
        if next >= rest.len() {
            break;
        }
        rest = &rest[next..];
    }
    false
}

fn is_dependency_resolve(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("could not resolve dependencies")
        || lower.contains("could not resolve dependency")
        || lower.contains("failed to collect dependencies")
        || lower.contains("non-resolvable")
        || lower.contains("could not find artifact")
        || lower.contains("eresolve")
        || lower.contains("unable to resolve dependency")
        || lower.contains("cannot find module")
        || lower.contains("module not found")
        || lower.contains("failed to resolve")
        || lower.contains("dependency resolution failed")
        || line.contains("无法解析依赖")
}

fn is_caused_by(line: &str) -> bool {
    line.to_ascii_lowercase().contains("caused by:")
}

fn is_exception_line(line: &str) -> bool {
    if line.contains("Exception") {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    lower.contains("error:") && !lower.starts_with("error ") && line.chars().count() >= 8
}

pub fn check_log_unknown_port_error(log_path: &Path) -> bool {
    if let Ok(text) = read_tail(log_path, 50) {
        let lower = text.to_ascii_lowercase();
        (lower.contains("unknown option") || lower.contains("unrecognized option"))
            && lower.contains("--port")
    } else {
        false
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
    for (k, v) in &p.env {
        cmd.env(k, v);
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

#[cfg(unix)]
fn prepare_stdin_fifo(fifo_path: &Path) -> Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::FromRawFd;

    if fifo_path.exists() || std::fs::symlink_metadata(fifo_path).is_ok() {
        let _ = std::fs::remove_file(fifo_path);
    }
    if let Some(parent) = fifo_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let c_path = CString::new(fifo_path.as_os_str().as_bytes())
        .with_context(|| format!("convert path to cstring: {}", fifo_path.display()))?;

    let res = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if res != 0 {
        let err = std::io::Error::last_os_error();
        bail!("mkfifo failed for {}: {}", fifo_path.display(), err);
    }

    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR) };
    if fd < 0 {
        let err = std::io::Error::last_os_error();
        let _ = std::fs::remove_file(fifo_path);
        bail!(
            "open FIFO O_RDWR failed for {}: {}",
            fifo_path.display(),
            err
        );
    }

    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    Ok(file)
}

#[cfg(unix)]
struct CloseFd(std::os::unix::io::RawFd);

#[cfg(unix)]
impl Drop for CloseFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            unsafe { libc::close(self.0) };
            self.0 = -1;
        }
    }
}

#[cfg(unix)]
fn last_errno() -> libc::c_int {
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location()
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    unsafe {
        *libc::__error()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
}

#[cfg(unix)]
fn claim_owned_fd(fd: std::os::unix::io::RawFd) -> Result<std::os::unix::io::RawFd> {
    let fd = if fd > 2 {
        fd
    } else {
        let duped = unsafe { libc::fcntl(fd, libc::F_DUPFD, 3) };
        if duped < 0 {
            let err = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            bail!("dup pty fd: {err}");
        }
        unsafe { libc::close(fd) };
        duped
    };
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        bail!("fcntl getfd: {err}");
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(fd) };
        bail!("fcntl cloexec: {err}");
    }
    Ok(fd)
}

#[cfg(unix)]
fn open_pty_pair() -> Result<(std::os::unix::io::RawFd, std::os::unix::io::RawFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut win: libc::winsize = unsafe { std::mem::zeroed() };
    win.ws_row = 24;
    win.ws_col = 80;
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            // Apple takes `*mut winsize`; other Unix takes `*const`.
            #[cfg(target_os = "macos")]
            &mut win,
            #[cfg(not(target_os = "macos"))]
            &win,
        )
    };
    if rc == 0 && master >= 0 && slave >= 0 {
        return Ok((master, slave));
    }
    if master >= 0 {
        unsafe { libc::close(master) };
    }
    if slave >= 0 {
        unsafe { libc::close(slave) };
    }

    let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
    if master < 0 {
        let err = std::io::Error::last_os_error();
        bail!("openpty/posix_openpt failed: {err}");
    }
    if unsafe { libc::grantpt(master) } != 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(master) };
        bail!("grantpt failed: {err}");
    }
    if unsafe { libc::unlockpt(master) } != 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(master) };
        bail!("unlockpt failed: {err}");
    }
    let name_ptr = unsafe { libc::ptsname(master) };
    if name_ptr.is_null() {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(master) };
        bail!("ptsname failed: {err}");
    }
    let slave = unsafe { libc::open(name_ptr, libc::O_RDWR | libc::O_NOCTTY) };
    if slave < 0 {
        let err = std::io::Error::last_os_error();
        unsafe { libc::close(master) };
        bail!("open pty slave failed: {err}");
    }
    unsafe {
        libc::ioctl(slave, libc::TIOCSWINSZ, &win);
    }
    Ok((master, slave))
}

#[cfg(unix)]
fn configure_pty_slave(slave: std::os::unix::io::RawFd) -> Result<()> {
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(slave, &mut term) } != 0 {
        let err = std::io::Error::last_os_error();
        bail!("tcgetattr on pty slave: {err}");
    }
    term.c_iflag |= libc::ICRNL;
    term.c_oflag |= libc::OPOST | libc::ONLCR;
    // Keep cooked input (ICANON) and signals (ISIG), but never echo keystrokes.
    // ECHO would copy passwords from the master back into the project log.
    term.c_lflag |= libc::ICANON | libc::ISIG;
    term.c_lflag &= !(libc::ECHO | libc::ECHOE | libc::ECHOK | libc::ECHONL);
    if unsafe { libc::tcsetattr(slave, libc::TCSANOW, &term) } != 0 {
        let err = std::io::Error::last_os_error();
        bail!("tcsetattr on pty slave: {err}");
    }
    Ok(())
}

#[cfg(unix)]
fn upsert_env(map: &mut Vec<(Vec<u8>, Vec<u8>)>, key: &[u8], value: &[u8]) {
    if let Some(slot) = map.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value.to_vec();
    } else {
        map.push((key.to_vec(), value.to_vec()));
    }
}

#[cfg(unix)]
fn build_relay_env(
    toolchain: &[(String, String)],
    project: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<std::ffi::CString>> {
    use std::os::unix::ffi::OsStrExt;
    let mut map: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for (k, v) in std::env::vars_os() {
        map.push((k.as_bytes().to_vec(), v.as_bytes().to_vec()));
    }
    for (k, v) in toolchain {
        if !v.is_empty() {
            upsert_env(&mut map, k.as_bytes(), v.as_bytes());
        }
    }
    for (k, v) in project {
        upsert_env(&mut map, k.as_bytes(), v.as_bytes());
    }
    let mut out = Vec::with_capacity(map.len());
    for (k, v) in map {
        let mut item = Vec::with_capacity(k.len() + 1 + v.len());
        item.extend_from_slice(&k);
        item.push(b'=');
        item.extend_from_slice(&v);
        if let Ok(c) = std::ffi::CString::new(item) {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn env_bytes<'a>(env: &'a [std::ffi::CString], key: &str) -> Option<&'a [u8]> {
    let prefix = format!("{key}=");
    let prefix = prefix.as_bytes();
    for item in env {
        let bytes = item.as_bytes();
        if let Some(rest) = bytes.strip_prefix(prefix) {
            return Some(rest);
        }
    }
    None
}

#[cfg(unix)]
fn resolve_program(cmd: &str, path_env: Option<&[u8]>, root: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    if cmd.contains('/') {
        let candidate = if Path::new(cmd).is_absolute() {
            PathBuf::from(cmd)
        } else {
            root.join(cmd)
        };
        let c_path = std::ffi::CString::new(candidate.as_os_str().as_bytes())
            .with_context(|| format!("spawn {cmd}: path contains nul"))?;
        if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } != 0 {
            let err = std::io::Error::last_os_error();
            bail!("spawn {cmd}: {err}");
        }
        return std::ffi::CString::new(cmd.as_bytes())
            .with_context(|| format!("spawn {cmd}: path contains nul"));
    }
    let owned;
    let path = if let Some(p) = path_env {
        owned = String::from_utf8_lossy(p).into_owned();
        owned.as_str()
    } else {
        "/usr/bin:/bin"
    };
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(cmd);
        let Ok(c_path) = std::ffi::CString::new(candidate.as_os_str().as_bytes()) else {
            continue;
        };
        if unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0 {
            return Ok(c_path);
        }
    }
    bail!("spawn {cmd}: No such file or directory");
}

#[cfg(unix)]
#[derive(Clone, Copy)]
struct PtyRelayArgs {
    master: std::os::unix::io::RawFd,
    slave: std::os::unix::io::RawFd,
    fifo: std::os::unix::io::RawFd,
    log_fd: std::os::unix::io::RawFd,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
    exe: *const libc::c_char,
    cwd: *const libc::c_char,
    fifo_path: *const libc::c_char,
}

#[cfg(unix)]
static PTY_RELAY_CHILD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

#[cfg(unix)]
extern "C" fn pty_relay_on_signal(_sig: libc::c_int) {
    let child = PTY_RELAY_CHILD.load(std::sync::atomic::Ordering::Relaxed);
    unsafe {
        if child > 1 {
            libc::kill(-child, libc::SIGTERM);
            libc::kill(child, libc::SIGTERM);
        }
        libc::_exit(0);
    }
}

#[cfg(unix)]
fn write_all_raw(fd: std::os::unix::io::RawFd, mut data: &[u8]) {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast::<libc::c_void>(), data.len()) };
        if n < 0 {
            let err = last_errno();
            if err == libc::EINTR {
                continue;
            }
            return;
        }
        let Some(count) = usize::try_from(n).ok().filter(|count| *count > 0) else {
            return;
        };
        data = &data[count..];
    }
}

#[cfg(unix)]
unsafe fn set_nonblock(fd: std::os::unix::io::RawFd) {
    let flags = libc::fcntl(fd, libc::F_GETFL);
    if flags >= 0 {
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

#[cfg(unix)]
unsafe fn close_extra_fds(keep: &[std::os::unix::io::RawFd; 4]) {
    let open_max = libc::sysconf(libc::_SC_OPEN_MAX);
    let max_fd = if (1..=4096).contains(&open_max) {
        i32::try_from(open_max).unwrap_or(1024)
    } else {
        1024
    };
    for fd in 0..max_fd {
        if keep.contains(&fd) {
            continue;
        }
        libc::close(fd);
    }
}

#[cfg(unix)]
unsafe fn unblock_all_signals() {
    let mut set: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut set);
    libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
}

#[cfg(unix)]
unsafe fn block_stop_signals() {
    let mut set: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut set);
    libc::sigaddset(&mut set, libc::SIGTERM);
    libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
}

#[cfg(unix)]
unsafe fn install_relay_term_handler() {
    let mut action: libc::sigaction = std::mem::zeroed();
    action.sa_sigaction = pty_relay_on_signal as *const () as libc::sighandler_t;
    libc::sigemptyset(&mut action.sa_mask);
    action.sa_flags = 0;
    libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
}

#[cfg(unix)]
unsafe fn reset_command_signals() {
    let mut action: libc::sigaction = std::mem::zeroed();
    action.sa_sigaction = libc::SIG_DFL;
    libc::sigemptyset(&mut action.sa_mask);
    action.sa_flags = 0;
    for sig in [
        libc::SIGPIPE,
        libc::SIGINT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGQUIT,
        libc::SIGTTIN,
        libc::SIGTTOU,
        libc::SIGCHLD,
    ] {
        libc::sigaction(sig, &action, std::ptr::null_mut());
    }
}

/// Long-lived PTY relay. Runs only in the forked child and never returns.
///
/// The parent (possibly multi-threaded) has already created the PTY, FIFO, log,
/// and argument vectors. This process only uses libc so it does not touch locks
/// that other threads may have held across `fork`.
#[cfg(unix)]
unsafe fn pty_relay_entry(args: PtyRelayArgs) -> ! {
    block_stop_signals();
    if libc::setsid() < 0 {
        libc::setpgid(0, 0);
    }
    close_extra_fds(&[args.master, args.slave, args.fifo, args.log_fd]);
    set_nonblock(args.master);
    set_nonblock(args.fifo);

    let child = libc::fork();
    if child < 0 {
        write_all_raw(args.log_fd, b"pty relay: fork command failed\n");
        libc::_exit(1);
    }
    if child == 0 {
        unblock_all_signals();
        reset_command_signals();
        let sid = libc::setsid();
        // macOS `TIOCSCTTY` is `c_uint`; `ioctl` request is `c_ulong`.
        let ctty = libc::ioctl(
            args.slave,
            #[cfg(target_os = "macos")]
            libc::TIOCSCTTY.into(),
            #[cfg(not(target_os = "macos"))]
            libc::TIOCSCTTY,
            0,
        );
        let pgid = libc::getpid();
        libc::ioctl(args.slave, libc::TIOCSPGRP, &pgid);
        if args.slave != 0 {
            libc::dup2(args.slave, 0);
        }
        if args.slave != 1 {
            libc::dup2(args.slave, 1);
        }
        if args.slave != 2 {
            libc::dup2(args.slave, 2);
        }
        if args.slave > 2 {
            libc::close(args.slave);
        }
        libc::close(args.master);
        libc::close(args.fifo);
        libc::close(args.log_fd);
        if sid < 0 {
            write_all_raw(2, b"pty relay: setsid failed\n");
        }
        if ctty < 0 {
            write_all_raw(2, b"pty relay: set controlling tty failed\n");
        }
        if libc::chdir(args.cwd) != 0 {
            write_all_raw(2, b"pty relay: chdir failed\n");
            libc::_exit(1);
        }
        libc::execve(args.exe, args.argv, args.envp);
        write_all_raw(2, b"pty relay: exec failed\n");
        libc::_exit(127);
    }

    libc::close(args.slave);
    PTY_RELAY_CHILD.store(child, std::sync::atomic::Ordering::Relaxed);
    install_relay_term_handler();
    unblock_all_signals();

    const BUF: usize = 8192;
    let mut inbuf = [0u8; BUF];
    let mut in_len: usize = 0;
    let mut in_off: usize = 0;
    let mut outbuf = [0u8; BUF];
    let mut master_open = true;

    loop {
        let mut status: libc::c_int = 0;
        let waited = libc::waitpid(child, &mut status, libc::WNOHANG);
        if waited == child || (waited < 0 && last_errno() == libc::ECHILD) {
            if master_open {
                loop {
                    let n = libc::read(
                        args.master,
                        outbuf.as_mut_ptr().cast::<libc::c_void>(),
                        outbuf.len(),
                    );
                    if let Some(count) = usize::try_from(n).ok().filter(|count| *count > 0) {
                        write_all_raw(args.log_fd, &outbuf[..count]);
                        continue;
                    }
                    break;
                }
            }
            libc::close(args.master);
            libc::close(args.fifo);
            libc::close(args.log_fd);
            libc::unlink(args.fifo_path);
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                1
            };
            libc::_exit(code);
        }

        if in_off > 0 {
            let rest = in_len - in_off;
            if rest > 0 {
                std::ptr::copy(inbuf.as_ptr().add(in_off), inbuf.as_mut_ptr(), rest);
            }
            in_len = rest;
            in_off = 0;
        }

        let mut fds = [
            libc::pollfd {
                fd: -1,
                events: 0,
                revents: 0,
            },
            libc::pollfd {
                fd: -1,
                events: 0,
                revents: 0,
            },
        ];
        let mut nfd: usize = 0;
        if master_open || in_len > in_off {
            let mut events: libc::c_short = 0;
            if master_open {
                events |= libc::POLLIN as libc::c_short;
            }
            if in_len > in_off {
                events |= libc::POLLOUT as libc::c_short;
            }
            fds[nfd] = libc::pollfd {
                fd: args.master,
                events,
                revents: 0,
            };
            nfd += 1;
        }
        if in_len < BUF {
            fds[nfd] = libc::pollfd {
                fd: args.fifo,
                events: libc::POLLIN as libc::c_short,
                revents: 0,
            };
            nfd += 1;
        }
        if nfd == 0 {
            let pause = libc::timespec {
                tv_sec: 0,
                tv_nsec: 200_000_000,
            };
            libc::nanosleep(&pause, std::ptr::null_mut());
            continue;
        }
        let pr = libc::poll(fds.as_mut_ptr(), nfd as libc::nfds_t, 200);
        if pr < 0 {
            if last_errno() == libc::EINTR {
                continue;
            }
            break;
        }
        let mut progress = false;
        for slot in fds.iter().take(nfd) {
            if slot.revents == 0 {
                continue;
            }
            if slot.fd == args.master
                && master_open
                && (slot.revents & libc::POLLIN as libc::c_short) != 0
            {
                let n = libc::read(
                    args.master,
                    outbuf.as_mut_ptr().cast::<libc::c_void>(),
                    outbuf.len(),
                );
                if let Some(count) = usize::try_from(n).ok().filter(|count| *count > 0) {
                    write_all_raw(args.log_fd, &outbuf[..count]);
                    progress = true;
                } else if n < 0 {
                    let err = last_errno();
                    if err == libc::EIO
                        || (err != libc::EAGAIN && err != libc::EWOULDBLOCK && err != libc::EINTR)
                    {
                        master_open = false;
                    }
                } else {
                    master_open = false;
                }
            }
            if slot.fd == args.master
                && (slot.revents & libc::POLLOUT as libc::c_short) != 0
                && in_off < in_len
            {
                let n = libc::write(
                    args.master,
                    inbuf.as_ptr().add(in_off).cast::<libc::c_void>(),
                    in_len - in_off,
                );
                if let Some(count) = usize::try_from(n).ok().filter(|count| *count > 0) {
                    in_off += count;
                    progress = true;
                } else if n < 0 {
                    let err = last_errno();
                    if err == libc::EIO || err == libc::EPIPE {
                        in_off = 0;
                        in_len = 0;
                        master_open = false;
                    }
                }
            }
            if slot.fd == args.fifo
                && (slot.revents & libc::POLLIN as libc::c_short) != 0
                && in_len < BUF
            {
                let n = libc::read(
                    args.fifo,
                    inbuf.as_mut_ptr().add(in_len).cast::<libc::c_void>(),
                    BUF - in_len,
                );
                if let Some(count) = usize::try_from(n).ok().filter(|count| *count > 0) {
                    in_len += count;
                    progress = true;
                }
            }
        }
        if pr > 0 && !progress {
            let pause = libc::timespec {
                tv_sec: 0,
                tv_nsec: 20_000_000,
            };
            libc::nanosleep(&pause, std::ptr::null_mut());
        }
    }

    libc::close(args.master);
    libc::close(args.fifo);
    libc::close(args.log_fd);
    libc::unlink(args.fifo_path);
    libc::_exit(1);
}

#[cfg(unix)]
fn reap_relay(pid: libc::pid_t) {
    let mut status: libc::c_int = 0;
    loop {
        let r = unsafe { libc::waitpid(pid, &mut status, 0) };
        if r == pid {
            break;
        }
        if r < 0 {
            let err = std::io::Error::last_os_error().raw_os_error();
            if err == Some(libc::EINTR) {
                continue;
            }
            break;
        }
    }
}

/// Unix `pty = true`: fork a process-group leader that owns the PTY.
///
/// The recorded pid is the relay. It holds `<log_dir>/<id>.stdin`, copies FIFO
/// bytes to the PTY master, and appends master output to the log. The real
/// start command is a grandchild with the slave as its controlling terminal.
/// macOS and Linux share this path; system `script` is not used.
#[cfg(unix)]
fn spawn_pty_relay(p: &Resolved, start_cmd: &[String], env: &[(String, String)]) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::IntoRawFd;

    if start_cmd.is_empty() {
        bail!("empty start command");
    }
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p.log_file)
        .with_context(|| format!("open log {}", p.log_file.display()))?;
    let log_fd = claim_owned_fd(log_file.into_raw_fd())?;
    let _log_guard = CloseFd(log_fd);

    let (raw_master, raw_slave) = open_pty_pair()?;
    let master = match claim_owned_fd(raw_master) {
        Ok(fd) => fd,
        Err(err) => {
            unsafe { libc::close(raw_slave) };
            return Err(err);
        }
    };
    let slave = match claim_owned_fd(raw_slave) {
        Ok(fd) => fd,
        Err(err) => {
            unsafe { libc::close(master) };
            return Err(err);
        }
    };
    let _master_guard = CloseFd(master);
    let _slave_guard = CloseFd(slave);
    configure_pty_slave(slave)?;

    let fifo_path = p.stdin_fifo_path();
    let fifo_file = prepare_stdin_fifo(&fifo_path)?;
    let fifo_fd = match claim_owned_fd(fifo_file.into_raw_fd()) {
        Ok(fd) => fd,
        Err(err) => {
            let _ = std::fs::remove_file(&fifo_path);
            return Err(err);
        }
    };
    let _fifo_guard = CloseFd(fifo_fd);

    let env_owned = build_relay_env(env, &p.env)?;
    let path_env = env_bytes(&env_owned, "PATH");
    let exe = resolve_program(&start_cmd[0], path_env, &p.root)?;
    let argv_owned = start_cmd
        .iter()
        .map(|s| {
            std::ffi::CString::new(s.as_str())
                .with_context(|| format!("start command contains nul: {s}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let cwd = std::ffi::CString::new(p.root.as_os_str().as_bytes())
        .with_context(|| format!("project root contains nul: {}", p.root.display()))?;
    let fifo_c = std::ffi::CString::new(fifo_path.as_os_str().as_bytes())
        .with_context(|| format!("fifo path contains nul: {}", fifo_path.display()))?;

    let mut argv_ptrs: Vec<*const libc::c_char> = argv_owned.iter().map(|s| s.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    let mut env_ptrs: Vec<*const libc::c_char> = env_owned.iter().map(|s| s.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());

    let relay_args = PtyRelayArgs {
        master,
        slave,
        fifo: fifo_fd,
        log_fd,
        argv: argv_ptrs.as_ptr(),
        envp: env_ptrs.as_ptr(),
        exe: exe.as_ptr(),
        cwd: cwd.as_ptr(),
        fifo_path: fifo_c.as_ptr(),
    };

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let err = std::io::Error::last_os_error();
        let _ = std::fs::remove_file(&fifo_path);
        bail!("fork pty relay: {err}");
    }
    if pid == 0 {
        // Keep the pointed-to allocations alive until exec/_exit. Dropping them
        // before fork would leave the child with dangling argv/env pointers.
        std::mem::forget(argv_owned);
        std::mem::forget(argv_ptrs);
        std::mem::forget(env_owned);
        std::mem::forget(env_ptrs);
        std::mem::forget(exe);
        std::mem::forget(cwd);
        std::mem::forget(fifo_c);
        unsafe { pty_relay_entry(relay_args) }
    }

    let pid_u = u32::try_from(pid).unwrap_or(0);
    if pid_u <= 1 {
        let _ = std::fs::remove_file(&fifo_path);
        bail!("fork pty relay returned pid {pid}");
    }
    if let Err(err) = std::fs::write(&p.pid_file, format!("{pid_u}\n")) {
        signal_group(pid_u, "-KILL");
        signal_pid(pid_u, "-KILL");
        let _ = std::fs::remove_file(&fifo_path);
        let _ = std::fs::remove_file(p.log_dir.join(format!("{}.answers.json", p.id)));
        return Err(err).with_context(|| format!("write pid {}", p.pid_file.display()));
    }
    thread::spawn(move || reap_relay(pid));
    eprintln!("  pid {pid_u}");
    Ok(())
}

fn spawn_background(p: &Resolved, start_cmd: &[String], env: &[(String, String)]) -> Result<()> {
    if start_cmd.is_empty() {
        bail!("empty start command");
    }
    #[cfg(unix)]
    if p.pty {
        return spawn_pty_relay(p, start_cmd, env);
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
    cmd.args(&start_cmd[1..]);
    cmd.stdout(log).stderr(log_err).current_dir(&p.root);

    #[cfg(unix)]
    {
        let fifo_file = prepare_stdin_fifo(&p.stdin_fifo_path())?;
        cmd.stdin(Stdio::from(fifo_file));
    }
    #[cfg(not(unix))]
    {
        cmd.stdin(Stdio::null());
    }

    for (k, v) in env {
        if !v.is_empty() {
            cmd.env(k, v);
        }
    }
    for (k, v) in &p.env {
        cmd.env(k, v);
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
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(p.stdin_fifo_path());
            let _ = std::fs::remove_file(p.log_dir.join(format!("{}.answers.json", p.id)));
        }
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
    let mut tc_env = toolchain_env(
        p.java.as_deref(),
        p.java_home.as_deref(),
        p.node.as_deref(),
        p.node_home.as_deref(),
        &roots,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    if p.port_mode == "env" && p.port > 0 {
        let env_key = p.port_env.as_deref().unwrap_or("PORT");
        tc_env.push((env_key.to_string(), p.port.to_string()));
    }

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
    if let Some(parent) = p
        .log_file
        .parent()
        .filter(|d| !d.as_os_str().is_empty() && *d != p.log_dir.as_path())
    {
        ensure_log_dir(parent, current_home().as_deref())?;
    }
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

    #[cfg(unix)]
    {
        let initial_offset = std::fs::metadata(&p.log_file).map(|m| m.len()).unwrap_or(0);
        let init_state = AutoAnswersState {
            last_log_offset: initial_offset,
            triggered_counts: std::collections::BTreeMap::new(),
        };
        if let Ok(json) = serde_json::to_string(&init_state) {
            let _ = std::fs::write(p.log_dir.join(format!("{}.answers.json", p.id)), json);
        }
    }

    if p.background {
        spawn_background(p, &start_cmd, &tc_env)?;
    } else {
        let mut full_env = tc_env;
        full_env.extend(p.catalina_env());
        for (k, v) in &p.env {
            full_env.push((k.clone(), v.clone()));
        }
        run_cmd(&start_cmd, Some(&p.root), &full_env)?;
    }
    Ok(())
}

#[allow(dead_code)]
fn cmd_start(p: &Resolved) -> Result<()> {
    cmd_start_detached(p)
}

#[cfg(windows)]
fn signal_pid(pid: u32, _sig: &str) -> bool {
    Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
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

#[cfg(windows)]
fn signal_group(pid: u32, _sig: &str) -> bool {
    if pid <= 1 {
        return false;
    }
    Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
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

#[cfg(windows)]
fn group_alive(pid: u32) -> bool {
    process_alive(pid)
}

#[cfg(not(windows))]
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
        return format!("port {port} is open; no listener found");
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

#[cfg(unix)]
fn descendant_pids(root: u32) -> Vec<u32> {
    if root <= 1 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        if let Ok(out) = Command::new("pgrep")
            .args(["-P", &parent.to_string()])
            .output()
        {
            if out.status.success() {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    if let Ok(cpid) = line.trim().parse::<u32>() {
                        if cpid > 1 && !result.contains(&cpid) {
                            result.push(cpid);
                            queue.push(cpid);
                        }
                    }
                }
            }
        }
    }
    result
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
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(p.stdin_fifo_path());
            let _ = std::fs::remove_file(p.log_dir.join(format!("{}.answers.json", p.id)));
        }
    }

    if is_down(p) {
        eprintln!("{} already stopped", p.id);
        let _ = std::fs::remove_file(&p.pid_file);
        let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(p.stdin_fifo_path());
            let _ = std::fs::remove_file(p.log_dir.join(format!("{}.answers.json", p.id)));
        }
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
    let mut roots: Vec<u32> = pid_in_file
        .filter(|&pid| process_alive(pid))
        .into_iter()
        .collect();

    let mut legacy_to_clean = Vec::new();
    if roots.is_empty() {
        for lpath in legacy_pid_paths(p) {
            if let Some(lpid) = read_pid(&lpath) {
                if process_alive(lpid) {
                    if !roots.contains(&lpid) {
                        roots.push(lpid);
                    }
                    legacy_to_clean.push(lpath);
                } else if lpath.is_file() {
                    let _ = std::fs::remove_file(&lpath);
                }
            }
        }
    }

    #[cfg(unix)]
    let mut all_descendants = Vec::new();
    #[cfg(unix)]
    for &r in &roots {
        all_descendants.extend(descendant_pids(r));
    }

    // 2. 然后对 pid 文件（或旧版存活根）里的进程按进程组发 TERM，宽限期后仍在则对进程组 KILL
    for &pid in &roots {
        if process_alive(pid) {
            let _ = terminate_pid_with_grace(pid, true, grace_secs);
        }
    }

    #[cfg(unix)]
    for dpid in all_descendants {
        if process_alive(dpid) {
            let _ = terminate_pid_with_grace(dpid, true, 2);
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

    // 5. 结束后删除 pid 文件和启动标记，清理旧版 PID 文件
    let _ = std::fs::remove_file(&p.pid_file);
    let _ = std::fs::remove_file(start_marker_path(&p.pid_file));
    for lpath in legacy_to_clean {
        let _ = std::fs::remove_file(lpath);
    }
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(p.stdin_fifo_path());
        let _ = std::fs::remove_file(p.log_dir.join(format!("{}.answers.json", p.id)));
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
    /// Log directory. Empty uses `~/.tongyun-dev/logs`.
    #[serde(default)]
    pub log_dir: Option<String>,
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
    #[serde(default)]
    pub port_mode: Option<String>,
    #[serde(default)]
    pub port_arg: Option<String>,
    #[serde(default)]
    pub port_env: Option<String>,
    /// 所属分组（可选）。
    #[serde(default)]
    pub group: Option<String>,
    /// 额外环境变量（可选）。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 伪终端运行（可选，缺省 false）。
    #[serde(default)]
    pub pty: bool,
    /// 自动应答规则（可选）。
    #[serde(default)]
    pub auto_answers: Vec<AutoAnswer>,
    /// 快捷网页链接（可选）。
    #[serde(default)]
    pub links: Vec<WebLink>,
    /// 默认启动环境预设（可选）。
    #[serde(default)]
    pub default_profile: Option<String>,
    /// 启动环境预设表（可选）。
    #[serde(default)]
    pub profiles: std::collections::BTreeMap<String, ProfileConfig>,
    /// 依赖服务项目 ID 列表（可选）。
    #[serde(default)]
    pub depends_on: Vec<String>,
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
        let port_mode = match self
            .port_mode
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(s) => {
                let m = s.to_ascii_lowercase();
                if !matches!(m.as_str(), "none" | "arg" | "env" | "config") {
                    bail!("port_mode must be one of none, arg, env, config, got `{s}`");
                }
                Some(m)
            }
            None => None,
        };
        let port_arg = non_empty(&self.port_arg);
        let port_env = non_empty(&self.port_env);
        let java_home = non_empty(&self.java_home);
        let node_home = non_empty(&self.node_home);
        let name = non_empty(&self.name).unwrap_or_else(|| id.clone());
        let enabled = self.enabled.unwrap_or(true);
        let reason = non_empty(&self.reason).unwrap_or_default();
        if !enabled && reason.is_empty() {
            bail!("disabled project `{id}` needs a reason");
        }
        for (idx, link) in self.links.iter().enumerate() {
            let u = link.url.trim();
            if u.is_empty() {
                bail!("links[{}]: url cannot be empty", idx);
            }
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                bail!(
                    "links[{}]: url `{u}` must start with http:// or https://",
                    idx
                );
            }
        }
        for (key, prof) in &self.profiles {
            if key.trim().is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                bail!("profile key `{key}` may only contain letters, digits, `_`, `-`");
            }
            if let Some(p) = prof.port {
                if p == 0 {
                    bail!("profile `{key}`: port cannot be 0");
                }
            }
        }
        if let Some(ref dp) = self.default_profile {
            let trimmed = dp.trim();
            if !trimmed.is_empty() && !self.profiles.contains_key(trimmed) {
                bail!("default_profile `{trimmed}` does not exist in profiles");
            }
        }
        let group = non_empty(&self.group).unwrap_or_default();
        let project = Project {
            name,
            group,
            root,
            enabled,
            reason,
            start: shell_argv(&start, shell.as_deref()),
            stop: non_empty(&self.stop)
                .map(|s| shell_argv(&s, shell.as_deref()))
                .unwrap_or_default(),
            start_windows: Vec::new(),
            stop_windows: Vec::new(),
            tomcat: non_empty(&self.tomcat).unwrap_or_default(),
            log_dir: non_empty(&self.log_dir).unwrap_or_default(),
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
            port_mode,
            port_arg,
            port_env,
            env: self.env.clone(),
            pty: self.pty,
            auto_answers: self.auto_answers,
            links: self.links,
            default_profile: non_empty(&self.default_profile),
            profiles: self.profiles,
            depends_on: self.depends_on,
        };
        Ok((id, project))
    }
}

fn project_table(p: &Project) -> toml_edit::Table {
    use toml_edit::{value, Array, Table};
    let mut t = Table::new();
    t["name"] = value(p.name.as_str());
    if !p.group.is_empty() {
        t["group"] = value(p.group.as_str());
    }
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
    if !p.log_dir.is_empty() {
        t["log_dir"] = value(p.log_dir.as_str());
    }
    if p.auto_install == Some(false) {
        t["auto_install"] = value(false);
    }
    if let Some(v) = &p.port_mode {
        if !v.is_empty() {
            t["port_mode"] = value(v.as_str());
        }
    }
    if let Some(v) = &p.port_arg {
        if !v.is_empty() {
            t["port_arg"] = value(v.as_str());
        }
    }
    if let Some(v) = &p.port_env {
        if !v.is_empty() {
            t["port_env"] = value(v.as_str());
        }
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
    if !p.env.is_empty() {
        let mut inline = toml_edit::InlineTable::new();
        for (k, v) in &p.env {
            inline.insert(k, toml_edit::Value::from(v.as_str()));
        }
        t["env"] = toml_edit::Item::Value(toml_edit::Value::InlineTable(inline));
    }
    if p.pty {
        t["pty"] = value(true);
    }
    if !p.auto_answers.is_empty() {
        t["auto_answers"] = toml_edit::Item::Value(toml_edit::Value::Array(
            auto_answers_to_toml_array(&p.auto_answers),
        ));
    }
    if !p.links.is_empty() {
        t["links"] = toml_edit::Item::Value(toml_edit::Value::Array(links_to_toml_array(&p.links)));
    }
    if let Some(ref dp) = p.default_profile {
        if !dp.is_empty() {
            t["default_profile"] = value(dp.as_str());
        }
    }
    if !p.profiles.is_empty() {
        t["profiles"] = toml_edit::Item::Table(profiles_to_toml_table(&p.profiles));
    }
    if !p.depends_on.is_empty() {
        let mut arr = toml_edit::Array::new();
        for d in &p.depends_on {
            arr.push(d.as_str());
        }
        t["depends_on"] = toml_edit::Item::Value(toml_edit::Value::Array(arr));
    }
    t
}

fn auto_answers_to_toml_array(answers: &[AutoAnswer]) -> toml_edit::Array {
    let mut arr = toml_edit::Array::new();
    for a in answers {
        let mut inline = toml_edit::InlineTable::new();
        inline.insert("expect", toml_edit::Value::from(a.expect.as_str()));
        inline.insert("send", toml_edit::Value::from(a.send.as_str()));
        if !a.once {
            inline.insert("once", toml_edit::Value::from(false));
        }
        if a.regex {
            inline.insert("regex", toml_edit::Value::from(true));
        }
        arr.push(toml_edit::Value::InlineTable(inline));
    }
    arr
}

fn links_to_toml_array(links: &[WebLink]) -> toml_edit::Array {
    let mut arr = toml_edit::Array::new();
    for l in links {
        let mut inline = toml_edit::InlineTable::new();
        inline.insert("title", toml_edit::Value::from(l.title.as_str()));
        inline.insert("url", toml_edit::Value::from(l.url.as_str()));
        arr.push(toml_edit::Value::InlineTable(inline));
    }
    arr
}

fn profiles_to_toml_table(
    profiles: &std::collections::BTreeMap<String, ProfileConfig>,
) -> toml_edit::Table {
    let mut table = toml_edit::Table::new();
    for (name, prof) in profiles {
        let mut prof_tab = toml_edit::Table::new();
        if !prof.label.is_empty() {
            prof_tab.insert(
                "label",
                toml_edit::Item::Value(toml_edit::Value::from(prof.label.as_str())),
            );
        }
        if let Some(ref start) = prof.start {
            prof_tab.insert(
                "start",
                toml_edit::Item::Value(toml_edit::Value::from(start.as_str())),
            );
        }
        if let Some(port) = prof.port {
            prof_tab.insert(
                "port",
                toml_edit::Item::Value(toml_edit::Value::from(port as i64)),
            );
        }
        if !prof.env.is_empty() {
            let mut inline = toml_edit::InlineTable::new();
            for (k, v) in &prof.env {
                inline.insert(k, toml_edit::Value::from(v.as_str()));
            }
            prof_tab.insert(
                "env",
                toml_edit::Item::Value(toml_edit::Value::InlineTable(inline)),
            );
        }
        table.insert(name, toml_edit::Item::Table(prof_tab));
    }
    table
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
    pub group: Option<String>,
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
    pub log_dir: Option<String>,
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
    #[serde(default)]
    pub port_mode: Option<String>,
    #[serde(default)]
    pub port_arg: Option<String>,
    #[serde(default)]
    pub port_env: Option<String>,
    /// 额外环境变量（可选）。
    #[serde(default)]
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// 伪终端运行（可选）。
    #[serde(default)]
    pub pty: Option<bool>,
    /// 自动应答规则（可选）。
    #[serde(default)]
    pub auto_answers: Option<Vec<AutoAnswer>>,
    /// 快捷网页链接（可选）。
    #[serde(default)]
    pub links: Option<Vec<WebLink>>,
    /// 默认启动环境预设（可选）。
    #[serde(default)]
    pub default_profile: Option<String>,
    /// 启动环境预设表（可选）。
    #[serde(default)]
    pub profiles: Option<std::collections::BTreeMap<String, ProfileConfig>>,
    /// 依赖服务项目 ID 列表（可选）。
    #[serde(default)]
    pub depends_on: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ProjectDetail {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub root: String,
    pub start: String,
    pub start_editable: bool,
    pub stop: String,
    pub stop_editable: bool,
    pub port: u16,
    pub login_url: String,
    pub health_url: String,
    pub log_dir: String,
    pub log_file: String,
    pub node: Option<String>,
    pub java: Option<String>,
    pub enabled: bool,
    pub reason: String,
    pub start_timeout_secs: Option<u64>,
    pub stop_grace_secs: Option<u64>,
    #[serde(default = "default_true")]
    pub auto_install: bool,
    #[serde(default = "default_port_mode")]
    pub port_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_arg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_env: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub pty: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_answers: Vec<AutoAnswer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<WebLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub profiles: std::collections::BTreeMap<String, ProfileConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

#[allow(clippy::if_same_then_else)]
/// One shell line for display / copy. `bash -c <script>` becomes the script.
pub fn display_command(argv: &[String]) -> String {
    if argv.is_empty() {
        return String::new();
    }
    if argv.len() >= 3
        && (argv[0] == "bash" || argv[0] == "sh")
        && (argv[1] == "-c" || argv[1] == "-lc")
    {
        let mut line = argv[2].clone();
        if argv.len() > 3 {
            line.push(' ');
            line.push_str(&shell_join(&argv[3..]));
        }
        return line;
    }
    shell_join(argv)
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.is_empty()
                || arg
                    .chars()
                    .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'))
            {
                format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn unpack_command(argv: &[String]) -> (String, bool) {
    if argv.is_empty() {
        (String::new(), true)
    } else if (argv.len() == 3
        && (argv[0] == "bash" || argv[0] == "sh")
        && (argv[1] == "-c" || argv[1] == "-lc"))
        || (argv.len() == 3
            && argv[0].eq_ignore_ascii_case("cmd")
            && (argv[1].eq_ignore_ascii_case("/c") || argv[1].eq_ignore_ascii_case("-c")))
        || (argv.len() == 3
            && argv[0].eq_ignore_ascii_case("powershell")
            && (argv[1].eq_ignore_ascii_case("-command") || argv[1].eq_ignore_ascii_case("-c")))
    {
        (argv[2].clone(), true)
    } else if argv.len() == 6
        && argv[0].eq_ignore_ascii_case("powershell")
        && argv[4].eq_ignore_ascii_case("-command")
    {
        (argv[5].clone(), true)
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
    /// Validate project data for a new project before modifying any config file or groups.
    pub fn validate_new_project(&self, req: &NewProject) -> Result<()> {
        let (id, project) = req.clone().into_project()?;
        if self.config.projects.contains_key(&id) {
            bail!("project `{id}` already exists; remove it first or pick another id");
        }
        resolve_project_with_log_default(
            &id,
            &project,
            current_home().as_deref(),
            &self.config.log_dir,
        )?;
        Ok(())
    }

    /// Register a project and write it to the config file. Existing ids are refused.
    pub fn add_project(&mut self, req: NewProject) -> Result<ProjectRow> {
        let (id, project) = req.into_project()?;
        if self.config.projects.contains_key(&id) {
            bail!("project `{id}` already exists; remove it first or pick another id");
        }
        // Same validation as loading: absolute root, templates resolve, etc.
        resolve_project_with_log_default(
            &id,
            &project,
            current_home().as_deref(),
            &self.config.log_dir,
        )?;
        let mut doc = read_doc(&self.path)?;
        if !project.group.is_empty() {
            validate_group_key(&project.group)?;
            let groups = doc
                .entry("group")
                .or_insert_with(|| {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    toml_edit::Item::Table(t)
                })
                .as_table_mut()
                .context("`group` in config is not a table")?;
            if !groups.contains_key(&project.group) {
                let t = toml_edit::Table::new();
                groups.insert(&project.group, toml_edit::Item::Table(t));
                self.config
                    .group
                    .insert(project.group.clone(), GroupConfig::default());
            }
        }
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
        Ok(self.finish_row(&r, configured))
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
        let r = self.resolve(id)?;
        let mut hints = Vec::new();
        if let Some(h) = diagnose_start_command(&r.root, &r.start) {
            hints.push(h);
        }
        if check_log_unknown_port_error(&r.log_file) {
            let port_advice =
                "启动脚本不认 --port 参数；建议去掉 --port，端口传递方式选「不传」".to_string();
            if !hints.contains(&port_advice) {
                hints.push(port_advice);
            }
        }
        Ok(ProjectDetail {
            id: id.to_string(),
            name: if p.name.trim().is_empty() {
                id.to_string()
            } else {
                p.name.clone()
            },
            group: if p.group.trim().is_empty() {
                None
            } else {
                Some(p.group.trim().to_string())
            },
            root: p.root.clone(),
            start,
            start_editable,
            stop,
            stop_editable,
            port: p.port,
            login_url: p.login_url.clone(),
            health_url: p.health_url.clone(),
            log_dir: p.log_dir.clone(),
            log_file: p.log_file.clone(),
            node: p.node.clone(),
            java: p.java.clone(),
            enabled: p.enabled,
            reason: p.reason.clone(),
            start_timeout_secs: p.start_timeout_secs,
            stop_grace_secs: p.stop_grace_secs,
            auto_install: p.auto_install.unwrap_or(true),
            port_mode: r.port_mode,
            port_arg: r.port_arg,
            port_env: r.port_env,
            env: p.env.clone(),
            pty: p.pty,
            auto_answers: p.auto_answers.clone(),
            hints,
            links: p.links.clone(),
            default_profile: p.default_profile.clone(),
            profiles: p.profiles.clone(),
            depends_on: p.depends_on.clone(),
        })
    }

    /// Check for dependency cycles in project depends_on configurations.
    pub fn check_depends_on_cycle(&self, target_id: &str, candidate_deps: &[String]) -> Result<()> {
        let mut visited = std::collections::HashSet::new();
        let mut stack = Vec::new();

        fn dfs(
            loaded: &Loaded,
            current: &str,
            target_id: &str,
            candidate_deps: &[String],
            visited: &mut std::collections::HashSet<String>,
            stack: &mut Vec<String>,
        ) -> Result<()> {
            stack.push(current.to_string());
            visited.insert(current.to_string());

            let deps: Vec<String> = if current == target_id {
                candidate_deps
                    .iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            } else if let Some(p) = loaded.config.projects.get(current) {
                p.depends_on.clone()
            } else {
                Vec::new()
            };

            for dep in deps {
                if stack.contains(&dep) {
                    stack.push(dep.clone());
                    bail!("检测到循环依赖: {}", stack.join(" -> "));
                }
                if !visited.contains(&dep) {
                    dfs(loaded, &dep, target_id, candidate_deps, visited, stack)?;
                }
            }

            stack.pop();
            Ok(())
        }

        dfs(
            self,
            target_id,
            target_id,
            candidate_deps,
            &mut visited,
            &mut stack,
        )
    }

    /// Validate project edit data before modifying any config file or groups.
    pub fn validate_update_project(&self, id: &str, edit: &ProjectEdit) -> Result<()> {
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

        if let Some(ref port_mode) = edit.port_mode {
            let trimmed = port_mode.trim().to_ascii_lowercase();
            if !trimmed.is_empty() && !matches!(trimmed.as_str(), "none" | "arg" | "env" | "config")
            {
                bail!("port_mode must be one of none, arg, env, config, got `{trimmed}`");
            }
        }

        if let Some(ref links) = edit.links {
            for (idx, link) in links.iter().enumerate() {
                let u = link.url.trim();
                if u.is_empty() {
                    bail!("links[{}]: url cannot be empty", idx);
                }
                if !(u.starts_with("http://") || u.starts_with("https://")) {
                    bail!(
                        "links[{}]: url `{u}` must start with http:// or https://",
                        idx
                    );
                }
            }
        }

        let effective_profiles = edit.profiles.as_ref().unwrap_or(&existing.profiles);
        for (key, prof) in effective_profiles {
            if key.trim().is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                bail!("profile key `{key}` may only contain letters, digits, `_`, `-`");
            }
            if let Some(p) = prof.port {
                if p == 0 {
                    bail!("profile `{key}`: port cannot be 0");
                }
            }
        }

        if let Some(ref dp) = edit.default_profile {
            let trimmed = dp.trim();
            if !trimmed.is_empty() && !effective_profiles.contains_key(trimmed) {
                bail!("default_profile `{trimmed}` does not exist in profiles");
            }
        }

        if let Some(ref deps) = edit.depends_on {
            for dep in deps {
                let dep_trim = dep.trim();
                if dep_trim.is_empty() {
                    continue;
                }
                if dep_trim == id {
                    bail!("项目 `{id}` 不能将自身作为依赖项");
                }
                if !self.config.projects.contains_key(dep_trim) {
                    bail!("依赖的项目 `{dep_trim}` 不存在");
                }
            }
            self.check_depends_on_cycle(id, deps)?;
        }

        let mut test_project = existing.clone();
        if let Some(ref name) = edit.name {
            test_project.name = name.trim().to_string();
        }
        if let Some(ref root) = edit.root {
            test_project.root = root.trim().to_string();
        }
        if let Some(port) = edit.port {
            test_project.port = port;
        }
        if let Some(ref start) = edit.start {
            test_project.start = shell_argv(start.trim(), existing.shell.as_deref());
        }
        if let Some(ref stop) = edit.stop {
            test_project.stop = shell_argv(stop.trim(), existing.shell.as_deref());
        }
        if let Some(enabled) = edit.enabled {
            test_project.enabled = enabled;
        }
        if let Some(ref reason) = edit.reason {
            test_project.reason = reason.trim().to_string();
        }
        if let Some(ref log_dir) = edit.log_dir {
            test_project.log_dir = log_dir.trim().to_string();
        }
        if let Some(ref log_file) = edit.log_file {
            test_project.log_file = log_file.trim().to_string();
        }
        resolve_project_with_log_default(
            id,
            &test_project,
            current_home().as_deref(),
            &self.config.log_dir,
        )?;
        Ok(())
    }

    /// Update an existing project in the config file. Preserves other keys, key order, comments and multiline strings.
    pub fn update_project(&mut self, id: &str, edit: ProjectEdit) -> Result<ProjectRow> {
        self.validate_update_project(id, &edit)?;
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

        let mut group_to_ensure: Option<String> = None;
        if let Some(ref group) = edit.group {
            let trimmed = group.trim();
            if trimmed.is_empty() {
                proj_table.remove("group");
            } else {
                validate_group_key(trimmed)?;
                set_table_val(proj_table, "group", toml_edit::Value::from(trimmed));
                group_to_ensure = Some(trimmed.to_string());
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
                let argv = shell_argv(trimmed, existing.shell.as_deref());
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

        if let Some(ref log_dir) = edit.log_dir {
            let trimmed = log_dir.trim();
            if trimmed.is_empty() {
                proj_table.remove("log_dir");
            } else {
                set_table_val(proj_table, "log_dir", toml_edit::Value::from(trimmed));
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
        if let Some(ref port_mode) = edit.port_mode {
            let trimmed = port_mode.trim().to_ascii_lowercase();
            if trimmed.is_empty() {
                proj_table.remove("port_mode");
            } else {
                if !matches!(trimmed.as_str(), "none" | "arg" | "env" | "config") {
                    bail!("port_mode must be one of none, arg, env, config, got `{trimmed}`");
                }
                set_table_val(proj_table, "port_mode", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref port_arg) = edit.port_arg {
            let trimmed = port_arg.trim();
            if trimmed.is_empty() {
                proj_table.remove("port_arg");
            } else {
                set_table_val(proj_table, "port_arg", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref port_env) = edit.port_env {
            let trimmed = port_env.trim();
            if trimmed.is_empty() {
                proj_table.remove("port_env");
            } else {
                set_table_val(proj_table, "port_env", toml_edit::Value::from(trimmed));
            }
        }

        if let Some(ref env) = edit.env {
            if env.is_empty() {
                proj_table.remove("env");
            } else {
                let mut inline = toml_edit::InlineTable::new();
                for (k, v) in env {
                    inline.insert(k, toml_edit::Value::from(v.as_str()));
                }
                set_table_val(proj_table, "env", toml_edit::Value::InlineTable(inline));
            }
        }

        if let Some(pty) = edit.pty {
            if !pty {
                proj_table.remove("pty");
            } else {
                set_table_val(proj_table, "pty", toml_edit::Value::from(true));
            }
        }

        if let Some(ref answers) = edit.auto_answers {
            if answers.is_empty() {
                proj_table.remove("auto_answers");
            } else {
                set_table_val(
                    proj_table,
                    "auto_answers",
                    toml_edit::Value::Array(auto_answers_to_toml_array(answers)),
                );
            }
        }

        if let Some(ref links) = edit.links {
            if links.is_empty() {
                proj_table.remove("links");
            } else {
                set_table_val(
                    proj_table,
                    "links",
                    toml_edit::Value::Array(links_to_toml_array(links)),
                );
            }
        }

        if let Some(ref default_profile) = edit.default_profile {
            let trimmed = default_profile.trim();
            if trimmed.is_empty() {
                proj_table.remove("default_profile");
            } else {
                set_table_val(
                    proj_table,
                    "default_profile",
                    toml_edit::Value::from(trimmed),
                );
            }
        }

        if let Some(ref profiles) = edit.profiles {
            if profiles.is_empty() {
                proj_table.remove("profiles");
            } else {
                proj_table.insert(
                    "profiles",
                    toml_edit::Item::Table(profiles_to_toml_table(profiles)),
                );
            }
        }

        if let Some(ref depends_on) = edit.depends_on {
            if depends_on.is_empty() {
                proj_table.remove("depends_on");
            } else {
                let mut arr = toml_edit::Array::new();
                for d in depends_on {
                    let trimmed = d.trim();
                    if !trimmed.is_empty() {
                        arr.push(trimmed);
                    }
                }
                set_table_val(proj_table, "depends_on", toml_edit::Value::Array(arr));
            }
        }

        if let Some(ref grp) = group_to_ensure {
            let groups = doc
                .entry("group")
                .or_insert_with(|| {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    toml_edit::Item::Table(t)
                })
                .as_table_mut()
                .context("`group` in config is not a table")?;
            if !groups.contains_key(grp) {
                let t = toml_edit::Table::new();
                groups.insert(grp, toml_edit::Item::Table(t));
                self.config
                    .group
                    .insert(grp.to_string(), GroupConfig::default());
            }
        }

        let temp_config = parse_config(&doc.to_string())?;
        let updated_project = temp_config
            .projects
            .get(id)
            .context(format!("project `{id}` missing after update"))?;
        resolve_project_with_log_default(
            id,
            updated_project,
            current_home().as_deref(),
            &temp_config.log_dir,
        )?;

        write_doc(&self.path, &doc)?;
        self.config = temp_config;

        let r = self.resolve(id)?;
        let configured = r.port > 0 && (!r.active_start().is_empty() || r.tomcat.is_some());
        Ok(self.finish_row(&r, configured))
    }

    /// Create a new group, writing `[group.<key>]` (with optional label) to config preserving comments.
    pub fn create_group(&mut self, key: &str, label: Option<&str>) -> Result<GroupInfo> {
        let key = key.trim();
        validate_group_key(key)?;
        if self.config.group.contains_key(key) {
            bail!("分组 `{key}` 已存在");
        }
        let label = label.map(str::trim).filter(|s| !s.is_empty());
        let mut doc = read_doc(&self.path)?;
        let groups = doc
            .entry("group")
            .or_insert_with(|| {
                let mut t = toml_edit::Table::new();
                t.set_implicit(true);
                toml_edit::Item::Table(t)
            })
            .as_table_mut()
            .context("`group` in config is not a table")?;
        if groups.contains_key(key) {
            bail!("分组 `{key}` 已存在");
        }
        let mut t = toml_edit::Table::new();
        if let Some(l) = label {
            t["label"] = toml_edit::value(l);
        }
        groups.insert(key, toml_edit::Item::Table(t));
        write_doc(&self.path, &doc)?;
        self.config.group.insert(
            key.to_string(),
            GroupConfig {
                label: label.map(|s| s.to_string()),
                order: None,
                members: Vec::new(),
                depends_on: Vec::new(),
            },
        );
        Ok(GroupInfo {
            key: key.to_string(),
            label: label.unwrap_or(key).to_string(),
            order: None,
            members: Vec::new(),
            depends_on: Vec::new(),
        })
    }

    /// Rename group label, writing to config preserving comments.
    pub fn rename_group_label(&mut self, key: &str, label: &str) -> Result<GroupInfo> {
        let key = key.trim();
        validate_group_key(key)?;
        let label = label.trim();
        if label.is_empty() {
            bail!("分组 label 不能为空");
        }
        let mut doc = read_doc(&self.path)?;
        let groups = doc
            .entry("group")
            .or_insert_with(|| {
                let mut t = toml_edit::Table::new();
                t.set_implicit(true);
                toml_edit::Item::Table(t)
            })
            .as_table_mut()
            .context("`group` in config is not a table")?;
        let target = groups
            .entry(key)
            .or_insert_with(|| {
                let t = toml_edit::Table::new();
                toml_edit::Item::Table(t)
            })
            .as_table_mut()
            .context("group entry is not a table")?;
        set_table_val(target, "label", toml_edit::Value::from(label));
        write_doc(&self.path, &doc)?;
        let entry = self.config.group.entry(key.to_string()).or_default();
        entry.label = Some(label.to_string());
        Ok(GroupInfo {
            key: key.to_string(),
            label: label.to_string(),
            order: entry.order,
            members: entry.members.clone(),
            depends_on: entry.depends_on.clone(),
        })
    }

    /// Resolve a group input (key or label) for a project:
    /// - If input is empty/whitespace: returns `Ok(None)` (clear group)
    /// - If matches an existing group's label or key (exact, or case-insensitive):
    ///   returns `Ok(Some(existing_group_key))`
    /// - Otherwise (new group):
    ///   generates a valid key via `generate_group_key`,
    ///   creates the group via `create_group(key, Some(input))`,
    ///   and returns `Ok(Some(key))`.
    pub fn resolve_or_create_group(&mut self, input: &str) -> Result<Option<String>> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let groups = self.groups();
        // 1. Exact match on label or key
        if let Some(g) = groups
            .iter()
            .find(|g| g.label == trimmed || g.key == trimmed)
        {
            return Ok(Some(g.key.clone()));
        }

        // 2. Case-insensitive match on label or key
        if let Some(g) = groups
            .iter()
            .find(|g| g.label.eq_ignore_ascii_case(trimmed) || g.key.eq_ignore_ascii_case(trimmed))
        {
            return Ok(Some(g.key.clone()));
        }

        // 3. New group: generate key and create group
        let existing_keys: Vec<String> = groups.iter().map(|g| g.key.clone()).collect();
        let key = generate_group_key(trimmed, &existing_keys);
        self.create_group(&key, Some(trimmed))?;
        Ok(Some(key))
    }

    /// Remove a group by key. Does not touch projects belonging to it.
    pub fn remove_group(&mut self, key: &str) -> Result<()> {
        let key = key.trim();
        validate_group_key(key)?;
        let mut doc = read_doc(&self.path)?;
        if let Some(groups) = doc.get_mut("group").and_then(|g| g.as_table_mut()) {
            groups.remove(key);
        }
        write_doc(&self.path, &doc)?;
        self.config.group.remove(key);
        Ok(())
    }

    /// Update or clear project group, writing back to config with toml_edit preserving comments.
    pub fn set_project_group(&mut self, id: &str, group: Option<&str>) -> Result<ProjectRow> {
        let val = group.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
        self.update_project(
            id,
            ProjectEdit {
                group: Some(val.to_string()),
                ..Default::default()
            },
        )
    }

    /// Page to open in the browser: first link if configured, else `login_url`, else `health_url`, else `http://127.0.0.1:<port>/`.
    pub fn page_url(&self, id: &str) -> Result<String> {
        let p = self.resolve(id)?;
        if let Some(first) = p.links.first() {
            if !first.url.trim().is_empty() {
                return Ok(first.url.trim().to_string());
            }
        }
        if !p.login_url.is_empty() {
            return Ok(p.login_url);
        }
        if p.port == 0 {
            bail!("project `{id}` has no port or page url");
        }
        Ok(format!("http://127.0.0.1:{}/", p.port))
    }

    /// All configured links, falling back to a single link from `page_url`.
    pub fn page_urls(&self, id: &str) -> Result<Vec<WebLink>> {
        let p = self.resolve(id)?;
        if !p.links.is_empty() {
            return Ok(p.links);
        }
        let fallback = self.page_url(id)?;
        Ok(vec![WebLink {
            title: "主页".to_string(),
            url: fallback,
        }])
    }

    /// Open project in interactive native terminal.
    pub fn open_in_terminal(&self, id: &str, profile_name: Option<&str>) -> Result<()> {
        let p = self.resolve_with_profile(id, profile_name)?;
        let start_cmd = p.active_start().join(" ");
        if start_cmd.trim().is_empty() {
            bail!("project `{id}` has no start command to run in terminal");
        }
        open_terminal_command(&p.root, &start_cmd, &p.name)
    }

    /// Absolute project root.
    pub fn log_file(&self, id: &str) -> Result<PathBuf> {
        Ok(self.resolve(id)?.log_file)
    }

    pub fn root_dir(&self, id: &str) -> Result<PathBuf> {
        Ok(self.resolve(id)?.root)
    }
}

pub fn open_terminal_command(dir: &Path, cmd_str: &str, title: &str) -> Result<()> {
    if !dir.is_dir() {
        bail!("directory does not exist: {}", dir.display());
    }
    let cmd_trimmed = cmd_str.trim();
    if cmd_trimmed.is_empty() {
        bail!("command string cannot be empty");
    }
    let dir_str = path_to_string(dir);

    #[cfg(target_os = "windows")]
    {
        let safe_title = title.replace(['"', '&', '|', '<', '>', '^'], "");
        let window_title = if safe_title.trim().is_empty() {
            "tongyun-dev"
        } else {
            safe_title.trim()
        };
        let mut c = Command::new("cmd");
        let full_cmd = format!("title {window_title} && cd /d \"{dir_str}\" && {cmd_trimmed}");
        c.args(["/C", "start", "", "cmd.exe", "/K", &full_cmd]);
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to open terminal for {title}"))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        let escaped_dir = dir_str.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_cmd = cmd_trimmed.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "tell application \"Terminal\" to do script \"cd \\\"{}\\\"; {}\"",
            escaped_dir, escaped_cmd
        );
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            &script,
            "-e",
            "tell application \"Terminal\" to activate",
        ]);
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to open terminal for {title}"))?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let terms = [
            ("x-terminal-emulator", vec!["-e", "bash", "-c", cmd_trimmed]),
            ("gnome-terminal", vec!["--", "bash", "-c", cmd_trimmed]),
            ("konsole", vec!["-e", "bash", "-c", cmd_trimmed]),
            ("xterm", vec!["-e", "bash", "-c", cmd_trimmed]),
        ];
        for (term, args) in terms {
            if let Ok(mut c) = Command::new(term)
                .current_dir(dir)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                return Ok(());
            }
        }
        bail!("no supported terminal emulator found (tried: x-terminal-emulator, gnome-terminal, konsole, xterm)");
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

/// Open a project directory in Cursor (`cursor <dir>` on PATH).
pub fn open_in_cursor(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!("不是目录：{}", path.display());
    }
    let bin = if cfg!(windows) {
        "cursor.cmd"
    } else {
        "cursor"
    };
    match Command::new(bin)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "未找到 {bin} 命令。安装 Cursor 后，在命令面板执行 Install 'cursor' command in PATH"
            );
        }
        Err(e) => bail!("无法用 Cursor 打开：{e}"),
    }
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
        let home = Path::new("/home/demo");
        assert_eq!(
            expand_tilde_with("~/code/acme/legacy-app", Some(home)).unwrap(),
            "/home/demo/code/acme/legacy-app"
        );
        assert_eq!(
            expand_tilde_with(
                "$HOME/.tongyun-dev/logs/legacy_app_catalina.out",
                Some(home)
            )
            .unwrap(),
            "/home/demo/.tongyun-dev/logs/legacy_app_catalina.out"
        );
        assert_eq!(
            expand_tilde_with("${HOME}/opt/tomcat-9", Some(home)).unwrap(),
            "/home/demo/opt/tomcat-9"
        );
        let err = expand_tilde_with("~/code/acme/legacy-app", None).unwrap_err();
        assert!(err.to_string().contains("HOME is not set"));
    }

    #[test]
    fn legacy_paths_resolve_without_tilde() {
        let p = legacy_app(Path::new("/home/demo"));
        assert_eq!(p.root, PathBuf::from("/home/demo/code/acme/legacy-app"));
        assert_eq!(
            p.tomcat.as_deref(),
            Some(Path::new("/home/demo/opt/tomcat-9"))
        );
        assert!(p.enabled);
        assert!(!p.background);
        assert!(p.reason.is_empty());
        assert_eq!(
            p.log_file,
            PathBuf::from("/home/demo/.tongyun-dev/logs/legacy_app_catalina.out")
        );
        assert_eq!(
            p.pid_file,
            PathBuf::from("/home/demo/.tongyun-dev/logs/legacy_app_tomcat.pid")
        );
        assert_eq!(
            p.start,
            vec![
                "bash".to_string(),
                "/home/demo/code/acme/legacy-app/scripts/start-legacy.sh".to_string(),
            ]
        );
        assert_eq!(
            p.stop,
            vec![
                "bash".to_string(),
                "/home/demo/opt/tomcat-9/bin/catalina.sh".to_string(),
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
            log_path: "/home/demo/.tongyun-dev/logs/legacy_app_catalina.out".into(),
            pid_path: "/home/demo/.tongyun-dev/logs/legacy_app_tomcat.pid".into(),
            root: "/home/demo/code/acme/legacy-app".into(),
            port_conflict: false,
            port_holder: String::new(),
            deps_missing: false,
            install_command: String::new(),
            hints: Vec::new(),
            active_profile: None,
            resolve_error: None,
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
            "/home/demo/.tongyun-dev/logs/legacy_app_catalina.out"
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
            Some(Path::new("/home/demo")),
        )
        .unwrap();
        assert!(p.enabled);
        assert!(!p.background);
        assert!(p.tomcat.is_none());
        assert!(p.login_url.is_empty());
        assert_eq!(
            p.log_file,
            PathBuf::from("/home/demo/.tongyun-dev/logs/demo.out")
        );
        assert_eq!(
            p.pid_file,
            PathBuf::from("/home/demo/.tongyun-dev/logs/demo.pid")
        );
    }

    #[test]
    fn log_dir_accepts_root_template_and_relative_path() {
        let home = Path::new("/home/demo");
        let cfg: FileConfig = toml::from_str(
            r#"
[projects.templated]
name = "templated"
root = "/tmp/demo-ok"
start = ["true"]
port = 9
log_dir = "{{root}}/logs"

[projects.relative]
name = "relative"
root = "/tmp/demo-rel"
start = ["true"]
port = 9
log_dir = "logs"

[projects.tomcat_logs]
name = "tomcat_logs"
root = "/tmp/demo-tc"
tomcat = "/opt/tomcat"
start = ["true"]
stop = ["true"]
port = 9
enabled = false
reason = "off"
log_dir = "{{tomcat}}/logs"
"#,
        )
        .unwrap();
        let templated = resolve_project(
            "templated",
            cfg.projects.get("templated").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(templated.log_dir, PathBuf::from("/tmp/demo-ok/logs"));
        assert_eq!(
            templated.log_file,
            PathBuf::from("/tmp/demo-ok/logs/templated.out")
        );
        let relative = resolve_project(
            "relative",
            cfg.projects.get("relative").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(relative.log_dir, PathBuf::from("/tmp/demo-rel/logs"));
        let tomcat_logs = resolve_project(
            "tomcat_logs",
            cfg.projects.get("tomcat_logs").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(tomcat_logs.log_dir, PathBuf::from("/opt/tomcat/logs"));

        let bad: FileConfig = toml::from_str(
            r#"
[projects.bad]
name = "bad"
root = "/tmp/demo-bad"
start = ["true"]
port = 9
log_dir = "{{not_a_var}}/logs"
"#,
        )
        .unwrap();
        let err = resolve_project("bad", bad.projects.get("bad").unwrap(), Some(home)).unwrap_err();
        assert!(err.to_string().contains("unresolved template"), "{err}");

        for raw in ["../../escaped", r"..\x", r"a\..\..\x"] {
            let escaped: FileConfig = toml::from_str(&format!(
                r#"
[projects.escaped]
name = "escaped"
root = "/tmp/demo-esc"
start = ["true"]
port = 9
log_dir = '{raw}'
"#
            ))
            .unwrap();
            let err = resolve_project(
                "escaped",
                escaped.projects.get("escaped").unwrap(),
                Some(home),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains(".."),
                "log_dir `{raw}` should be rejected, got {err}"
            );
        }

        let nested: FileConfig = toml::from_str(
            r#"
[projects.nested]
name = "nested"
root = "/tmp/demo-nested"
start = ["true"]
port = 9
log_dir = 'logs\sub'
"#,
        )
        .unwrap();
        let nested =
            resolve_project("nested", nested.projects.get("nested").unwrap(), Some(home)).unwrap();
        assert_eq!(nested.log_dir, PathBuf::from("/tmp/demo-nested/logs/sub"));

        let absolute: FileConfig = toml::from_str(
            r#"
[projects.absolute]
name = "absolute"
root = "/tmp/demo-abs"
start = ["true"]
port = 9
log_dir = "/tmp/../tongyun-logs"
"#,
        )
        .unwrap();
        let absolute = resolve_project(
            "absolute",
            absolute.projects.get("absolute").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(absolute.log_dir, PathBuf::from("/tmp/../tongyun-logs"));
    }

    #[test]
    fn top_level_log_dir_is_the_default_and_a_project_can_override_it() {
        let home = Path::new("/home/demo");
        let cfg: FileConfig = toml::from_str(
            r#"
log_dir = "logs"

[projects.inherit]
name = "inherit"
root = "/tmp/demo-inherit"
start = ["true"]
port = 9

[projects.own]
name = "own"
root = "/tmp/demo-own"
start = ["true"]
port = 9
log_dir = "/tmp/own-logs"
"#,
        )
        .unwrap();
        let inherit = resolve_project_with_log_default(
            "inherit",
            cfg.projects.get("inherit").unwrap(),
            Some(home),
            &cfg.log_dir,
        )
        .unwrap();
        assert_eq!(inherit.log_dir, PathBuf::from("/tmp/demo-inherit/logs"));
        assert_eq!(
            inherit.log_file,
            PathBuf::from("/tmp/demo-inherit/logs/inherit.out")
        );
        let own = resolve_project_with_log_default(
            "own",
            cfg.projects.get("own").unwrap(),
            Some(home),
            &cfg.log_dir,
        )
        .unwrap();
        assert_eq!(own.log_dir, PathBuf::from("/tmp/own-logs"));

        let escaped: FileConfig = toml::from_str(
            r#"
log_dir = '..\x'

[projects.inherits_bad]
name = "inherits_bad"
root = "/tmp/demo-bad-top"
start = ["true"]
port = 9

[projects.overrides]
name = "overrides"
root = "/tmp/demo-ok-top"
start = ["true"]
port = 9
log_dir = "logs"
"#,
        )
        .unwrap();
        let err = resolve_project_with_log_default(
            "inherits_bad",
            escaped.projects.get("inherits_bad").unwrap(),
            Some(home),
            &escaped.log_dir,
        )
        .unwrap_err();
        assert!(err.to_string().contains(".."), "{err}");
        let overrides = resolve_project_with_log_default(
            "overrides",
            escaped.projects.get("overrides").unwrap(),
            Some(home),
            &escaped.log_dir,
        )
        .unwrap();
        assert_eq!(overrides.log_dir, PathBuf::from("/tmp/demo-ok-top/logs"));

        let nested: FileConfig = toml::from_str(
            r#"
log_dir = 'a\..\..\x'

[projects.nested_bad]
name = "nested_bad"
root = "/tmp/demo-nested-bad"
start = ["true"]
port = 9
"#,
        )
        .unwrap();
        let err = resolve_project_with_log_default(
            "nested_bad",
            nested.projects.get("nested_bad").unwrap(),
            Some(home),
            &nested.log_dir,
        )
        .unwrap_err();
        assert!(err.to_string().contains(".."), "{err}");
    }

    #[test]
    fn list_keeps_other_projects_when_one_log_dir_fails() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_listdir_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("app");
        std::fs::create_dir_all(&root).unwrap();
        let path = dir.join("projects.toml");
        let root_s = path_to_string(&root);
        std::fs::write(
            &path,
            format!(
                r#"
[projects.good]
name = "good"
root = "{root_s}"
start = ["true"]
port = 9
log_dir = "{root_s}/logs"

[projects.bad]
name = "bad"
root = "{root_s}"
start = ["true"]
port = 9
log_dir = "{{{{not_a_var}}}}/logs"
"#
            ),
        )
        .unwrap();
        let loaded = load(Some(&path)).unwrap();
        let list = loaded.list().unwrap();
        assert_eq!(list.projects.len(), 2);
        let good = list.projects.iter().find(|p| p.id == "good").unwrap();
        assert!(good.resolve_error.is_none(), "{:?}", good.resolve_error);
        assert!(good.configured);
        let bad = list.projects.iter().find(|p| p.id == "bad").unwrap();
        assert!(!bad.configured);
        let err = bad.resolve_error.as_deref().unwrap_or("");
        assert!(err.contains("unresolved template"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn example_config_resolves() {
        let home = Path::new("/home/demo");
        let cfg = parse_config(EXAMPLE_TOML).unwrap();
        let mut ids: Vec<_> = cfg.projects.keys().cloned().collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["demo_api", "demo_legacy", "demo_monorepo", "demo_web"]
        );
        for id in ["demo_api", "demo_web"] {
            let p = resolve_project(id, cfg.projects.get(id).unwrap(), Some(home)).unwrap();
            assert!(p.enabled, "{id}");
            assert!(p.background, "{id}");
            assert!(!p.start.is_empty(), "{id}");
            assert_ne!(p.port, 0, "{id}");
            assert!(
                p.root.is_absolute() || path_to_string(&p.root).starts_with('/'),
                "{id}"
            );
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
        assert_eq!(api.root, PathBuf::from("/home/demo/code/demo/demo-api"));
        assert_eq!(
            api.log_file,
            PathBuf::from("/home/demo/.tongyun-dev/logs/demo_api.out")
        );
        let web = resolve_project(
            "demo_web",
            cfg.projects.get("demo_web").unwrap(),
            Some(home),
        )
        .unwrap();
        assert_eq!(web.node.as_deref(), Some("20"));
        assert_eq!(web.login_url, "http://127.0.0.1:5173/");

        let mono = resolve_project(
            "demo_monorepo",
            cfg.projects.get("demo_monorepo").unwrap(),
            Some(home),
        )
        .unwrap();
        assert!(!mono.enabled);
        assert_eq!(mono.port_mode, "none");
        assert_eq!(mono.port, 3000);

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
            Some(Path::new("/home/demo/opt/tomcat-9"))
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
        let home = Path::new("/home/demo");

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
            group: Some("Demo".into()),
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
            port_mode: "none".to_string(),
            port_arg: None,
            port_env: None,
            env: Default::default(),
            pty: false,
            auto_answers: Vec::new(),
            links: Vec::new(),
            default_profile: None,
            profiles: Default::default(),
            depends_on: Vec::new(),
            start_command: String::new(),
            profile_start_commands: Default::default(),
            resolve_error: None,
        };
        let json_fs: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&row_fs).unwrap()).unwrap();
        assert_eq!(json_fs["node"], "20");
        assert_eq!(json_fs["java"], "17");
        assert_eq!(json_fs["group"], "Demo");

        let row_none = ProjectRow {
            id: none.id.clone(),
            name: none.name.clone(),
            group: None,
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
            port_mode: "none".to_string(),
            port_arg: None,
            port_env: None,
            env: Default::default(),
            pty: false,
            auto_answers: Vec::new(),
            links: Vec::new(),
            default_profile: None,
            profiles: Default::default(),
            depends_on: Vec::new(),
            start_command: String::new(),
            profile_start_commands: Default::default(),
            resolve_error: None,
        };
        let json_none: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&row_none).unwrap()).unwrap();
        assert!(json_none.get("node").is_none());
        assert!(json_none.get("java").is_none());
        assert_eq!(json_none["group"], serde_json::Value::Null);
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
    fn add_persists_log_dir_and_resolves_it_under_root() {
        let path = temp_config("logdir", BASE);
        let root = path.parent().unwrap().join("app");
        let mut loaded = load(Some(&path)).unwrap();
        let mut created = req("web_app", &root);
        created.log_dir = Some("{{root}}/logs".into());
        loaded.add_project(created).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("log_dir = \"{{root}}/logs\""));
        let reloaded = load(Some(&path)).unwrap();
        let resolved = reloaded.resolve("web_app").unwrap();
        assert_eq!(resolved.log_dir, root.join("logs"));
        assert_eq!(resolved.log_file, root.join("logs").join("web_app.out"));
        let detail = reloaded.project_detail("web_app").unwrap();
        assert_eq!(detail.log_dir, "{{root}}/logs");
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
        let expected_start = if cfg!(windows) {
            vec!["cmd", "/C", "pnpm dev"]
        } else {
            vec!["bash", "-c", "pnpm dev"]
        };
        assert_eq!(p.start, expected_start);
        let resolved = reloaded.resolve("web_app").unwrap();
        assert_eq!(resolved.assembled_start(), expected_start);

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
        let expected_start = if cfg!(windows) {
            vec!["cmd", "/C", "pnpm dev"]
        } else {
            vec!["bash", "-c", "pnpm dev"]
        };
        let expected_stop = if cfg!(windows) {
            vec!["cmd", "/C", "pkill -f vite"]
        } else {
            vec!["bash", "-c", "pkill -f vite"]
        };
        assert_eq!(p.start, expected_start);
        assert_eq!(p.stop, expected_stop);
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

        let expected_shell = if cfg!(windows) { "cmd" } else { "bash" };
        let expected_flag = if cfg!(windows) { "/C" } else { "-c" };
        let expected_snippet =
            format!(r#"start = ["{expected_shell}", "{expected_flag}", "pnpm dev --port 8080"]"#);
        let text_after = std::fs::read_to_string(&path).unwrap();
        assert!(text_after.contains(&expected_snippet));

        let reloaded = load(Some(&path)).unwrap();
        assert_eq!(
            reloaded.config.projects.get("proj_a").unwrap().start,
            vec![expected_shell, expected_flag, "pnpm dev --port 8080"]
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

    #[test]
    fn test_10_group_read_write_clear_and_preserves_comments() {
        let toml_content = r#"# top comment
[group.demo]
label = "Demo 业务线"
order = 1
# reserved fields ignored during deserialization
members = ["proj_a"]
depends_on = ["backend"]

[group.backend]
label = "后端服务"
order = 2

[group.empty_grp]
label = "空分组"
order = 3

# comment before proj_a
[projects.proj_a]
name = "proj_a"
group = "demo"
root = "@ROOT@/proj_a"
start = ["bash", "-c", "echo start"]
port = 8080

# comment before proj_b
[projects.proj_b]
name = "proj_b"
group = "unregistered"
root = "@ROOT@/proj_b"
start = ["bash", "-c", "echo start"]
port = 9000
"#;
        let path = temp_config("t10_grp", toml_content);
        let mut loaded = load(Some(&path)).unwrap();

        let groups = loaded.groups();
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].key, "demo");
        assert_eq!(groups[0].label, "Demo 业务线");
        assert_eq!(groups[0].order, Some(1));

        assert_eq!(groups[1].key, "backend");
        assert_eq!(groups[1].label, "后端服务");
        assert_eq!(groups[1].order, Some(2));

        assert_eq!(groups[2].key, "empty_grp");
        assert_eq!(groups[2].label, "空分组");
        assert_eq!(groups[2].order, Some(3));

        // unregistered referenced by proj_b
        assert_eq!(groups[3].key, "unregistered");
        assert_eq!(groups[3].label, "unregistered");
        assert_eq!(groups[3].order, None);

        let list = loaded.list().unwrap();
        assert_eq!(list.groups.len(), 4);

        let p_a = list.projects.iter().find(|p| p.id == "proj_a").unwrap();
        assert_eq!(p_a.group.as_deref(), Some("demo"));
        let p_b = list.projects.iter().find(|p| p.id == "proj_b").unwrap();
        assert_eq!(p_b.group.as_deref(), Some("unregistered"));

        // Test create_group
        let created = loaded.create_group("frontend", Some("前端项目")).unwrap();
        assert_eq!(created.key, "frontend");
        assert_eq!(created.label, "前端项目");

        // Test rename_group_label
        let renamed = loaded.rename_group_label("frontend", "前端展示").unwrap();
        assert_eq!(renamed.label, "前端展示");

        // Edit proj_b group using set_project_group to newly created "frontend"
        let row_b = loaded
            .set_project_group("proj_b", Some("frontend"))
            .unwrap();
        assert_eq!(row_b.group.as_deref(), Some("frontend"));

        // Setting project group to an auto-created new group "auto_grp"
        let row_a_auto = loaded
            .set_project_group("proj_a", Some("auto_grp"))
            .unwrap();
        assert_eq!(row_a_auto.group.as_deref(), Some("auto_grp"));

        // Clear proj_b group
        let row_b_cleared = loaded.set_project_group("proj_b", None).unwrap();
        assert_eq!(row_b_cleared.group, None);

        // Check file content preserves comments and writes [group.auto_grp]
        let text_after = std::fs::read_to_string(&path).unwrap();
        assert!(text_after.contains("# top comment"));
        assert!(text_after.contains("# comment before proj_a"));
        assert!(text_after.contains("# comment before proj_b"));
        assert!(text_after.contains("[group.auto_grp]"));
        assert!(text_after.contains("group = \"auto_grp\""));

        // Key validation tests
        assert!(validate_group_key("valid-key_123").is_ok());
        assert!(validate_group_key("").is_err());
        assert!(validate_group_key("Invalid_Upper").is_err());
        assert!(validate_group_key("has space").is_err());
        assert!(validate_group_key(&"a".repeat(41)).is_err());

        // generate_group_key tests
        assert_eq!(generate_group_key("Demo", &[] as &[&str]), "demo");
        assert_eq!(generate_group_key("API_v2", &[] as &[&str]), "api_v2");
        assert_eq!(generate_group_key("web-app", &[] as &[&str]), "web-app");
        assert_eq!(
            generate_group_key("Demo Service", &[] as &[&str]),
            "demo-service"
        );
        assert_eq!(
            generate_group_key("Foo@Bar#123", &[] as &[&str]),
            "foo-bar-123"
        );
        assert_eq!(
            generate_group_key("  --hello..world-- ", &[] as &[&str]),
            "hello-world"
        );
        assert_eq!(generate_group_key("前端项目", &[] as &[&str]), "g1");
        assert_eq!(generate_group_key("后端项目", &["g1"]), "g2");
        assert_eq!(generate_group_key("微服务", &["g1", "g2"]), "g3");
        assert_eq!(generate_group_key("demo", &["demo"]), "g1");
        assert_eq!(generate_group_key("demo", &["demo", "g1"]), "g2");
        assert_eq!(generate_group_key("", &[] as &[&str]), "g1");

        // resolve_or_create_group tests
        assert_eq!(loaded.resolve_or_create_group("").unwrap(), None);
        assert_eq!(loaded.resolve_or_create_group("   ").unwrap(), None);
        let res_fe = loaded.resolve_or_create_group("前端").unwrap();
        assert_eq!(res_fe.as_deref(), Some("g1"));
        let groups = loaded.groups();
        let g1_info = groups.iter().find(|g| g.key == "g1").unwrap();
        assert_eq!(g1_info.label, "前端");
        assert_eq!(
            loaded.resolve_or_create_group("前端").unwrap().as_deref(),
            Some("g1")
        );
        assert_eq!(
            loaded.resolve_or_create_group("g1").unwrap().as_deref(),
            Some("g1")
        );

        // Test remove_group
        loaded.remove_group("g1").unwrap();
        assert!(!loaded.groups().iter().any(|g| g.key == "g1"));
        let text_after_remove = std::fs::read_to_string(&path).unwrap();
        assert!(!text_after_remove.contains("[group.g1]"));

        // Reload and check
        let reloaded = load(Some(&path)).unwrap();
        let list2 = reloaded.list().unwrap();
        let p_a2 = list2.projects.iter().find(|p| p.id == "proj_a").unwrap();
        assert_eq!(p_a2.group.as_deref(), Some("auto_grp"));
        let p_b2 = list2.projects.iter().find(|p| p.id == "proj_b").unwrap();
        assert_eq!(p_b2.group, None);
    }

    #[test]
    fn test_11_env_read_write_inline_table_and_preserves_comments() {
        let toml_content = r#"# project comment
[projects.demo]
name = "Demo"
root = "@ROOT@/demo"
start = ["echo", "start"]
port = 3000
env = { CI = "1", FOO = "bar" }
"#;
        let path = temp_config("t11_env", toml_content);
        let mut loaded = load(Some(&path)).unwrap();

        let detail = loaded.project_detail("demo").unwrap();
        assert_eq!(detail.env.get("CI").map(String::as_str), Some("1"));
        assert_eq!(detail.env.get("FOO").map(String::as_str), Some("bar"));

        // Update env
        let mut new_env = std::collections::BTreeMap::new();
        new_env.insert("CI".into(), "true".into());
        new_env.insert("NEW_VAR".into(), "hello".into());

        let edit = ProjectEdit {
            env: Some(new_env),
            ..Default::default()
        };
        let updated = loaded.update_project("demo", edit).unwrap();
        assert_eq!(updated.env.get("CI").map(String::as_str), Some("true"));
        assert_eq!(
            updated.env.get("NEW_VAR").map(String::as_str),
            Some("hello")
        );
        assert!(!updated.env.contains_key("FOO"));

        let file_text = std::fs::read_to_string(&path).unwrap();
        assert!(file_text.contains("# project comment"));
        assert!(file_text.contains("env = {"));

        let reloaded = load(Some(&path)).unwrap();
        let reloaded_detail = reloaded.project_detail("demo").unwrap();
        assert_eq!(
            reloaded_detail.env.get("CI").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            reloaded_detail.env.get("NEW_VAR").map(String::as_str),
            Some("hello")
        );
    }

    #[test]
    fn test_12_pty_and_auto_answers_read_write_inline_table_array() {
        let toml_content = r#"# project comment
[projects.interactive]
name = "Interactive"
root = "@ROOT@/demo"
start = ["echo", "start"]
port = 3000
pty = true
auto_answers = [
    { expect = "Select environment", send = "dev" }
]
"#;
        let path = temp_config("t12_pty_answers", toml_content);
        let mut loaded = load(Some(&path)).unwrap();

        let detail = loaded.project_detail("interactive").unwrap();
        assert!(detail.pty);
        assert_eq!(detail.auto_answers.len(), 1);
        assert_eq!(detail.auto_answers[0].expect, "Select environment");
        assert_eq!(detail.auto_answers[0].send, "dev");
        assert!(detail.auto_answers[0].once);

        // Update auto_answers and pty
        let edit = ProjectEdit {
            pty: Some(false),
            auto_answers: Some(vec![
                AutoAnswer {
                    expect: "(y/N)".into(),
                    send: "y".into(),
                    once: true,
                    regex: false,
                },
                AutoAnswer {
                    expect: "Continue?".into(),
                    send: "yes".into(),
                    once: false,
                    regex: true,
                },
            ]),
            ..Default::default()
        };
        let updated = loaded.update_project("interactive", edit).unwrap();
        assert!(!updated.pty);
        assert_eq!(updated.auto_answers.len(), 2);
        assert_eq!(updated.auto_answers[0].expect, "(y/N)");
        assert_eq!(updated.auto_answers[0].send, "y");
        assert_eq!(updated.auto_answers[1].expect, "Continue?");
        assert!(!updated.auto_answers[1].once);
        assert!(updated.auto_answers[1].regex);

        let file_text = std::fs::read_to_string(&path).unwrap();
        assert!(file_text.contains("# project comment"));
        assert!(!file_text.contains("pty = true"));
        assert!(file_text.contains("auto_answers = ["));

        let reloaded = load(Some(&path)).unwrap();
        let reloaded_detail = reloaded.project_detail("interactive").unwrap();
        assert!(!reloaded_detail.pty);
        assert_eq!(reloaded_detail.auto_answers.len(), 2);
        assert_eq!(reloaded_detail.auto_answers[0].expect, "(y/N)");
        assert_eq!(reloaded_detail.auto_answers[1].expect, "Continue?");
        assert!(!reloaded_detail.auto_answers[1].once);
        assert!(reloaded_detail.auto_answers[1].regex);
    }

    #[test]
    fn test_13_links_and_profiles_read_write_and_resolution() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_prof_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = r#"
# multi-profile project
[projects.web_app]
name = "Web App"
root = "/tmp"
port = 3000
default_profile = "dev"
start = ["bash", "-c", "exec pnpm dev"]
links = [
    { title = "前端主页", url = "http://127.0.0.1:3000" },
    { title = "接口文档", url = "http://127.0.0.1:3000/docs" }
]

[projects.web_app.profiles.dev]
label = "开发环境"
start = "pnpm dev"
port = 3000

[projects.web_app.profiles.mock]
label = "Mock 模式"
start = "pnpm dev:mock"
port = 3001
[projects.web_app.profiles.mock.env]
USE_MOCK = "true"
"#;
        std::fs::write(&path, toml).unwrap();
        let mut loaded = load(Some(&path)).unwrap();

        let detail = loaded.project_detail("web_app").unwrap();
        assert_eq!(detail.links.len(), 2);
        assert_eq!(detail.links[0].title, "前端主页");
        assert_eq!(detail.links[1].url, "http://127.0.0.1:3000/docs");
        assert_eq!(detail.default_profile.as_deref(), Some("dev"));
        assert_eq!(detail.profiles.len(), 2);
        assert_eq!(detail.profiles.get("mock").unwrap().port, Some(3001));

        // Resolution with default profile
        let res_default = loaded.resolve("web_app").unwrap();
        assert_eq!(res_default.port, 3000);

        // Resolution with mock profile
        let res_mock = loaded
            .resolve_with_profile("web_app", Some("mock"))
            .unwrap();
        assert_eq!(res_mock.port, 3001);
        assert_eq!(
            res_mock.env.get("USE_MOCK").map(|s| s.as_str()),
            Some("true")
        );

        // Update links and profiles
        let mut new_profiles = std::collections::BTreeMap::new();
        new_profiles.insert(
            "prod".to_string(),
            ProfileConfig {
                label: "生产预览".to_string(),
                start: Some("pnpm preview".to_string()),
                port: Some(4000),
                env: Default::default(),
            },
        );
        let updated = loaded
            .update_project(
                "web_app",
                ProjectEdit {
                    links: Some(vec![WebLink {
                        title: "主页".to_string(),
                        url: "http://127.0.0.1:4000".to_string(),
                    }]),
                    default_profile: Some("prod".to_string()),
                    profiles: Some(new_profiles),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(updated.links.len(), 1);
        assert_eq!(updated.default_profile.as_deref(), Some("prod"));
        assert_eq!(updated.profiles.len(), 1);

        let reloaded = load(Some(&path)).unwrap();
        let reloaded_detail = reloaded.project_detail("web_app").unwrap();
        assert_eq!(reloaded_detail.links.len(), 1);
        assert_eq!(reloaded_detail.default_profile.as_deref(), Some("prod"));
        assert_eq!(reloaded_detail.profiles.len(), 1);
        assert_eq!(
            reloaded_detail.profiles.get("prod").unwrap().port,
            Some(4000)
        );
    }

    #[test]
    fn test_14_validation_of_links_and_profiles() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_val_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = r#"
[projects.app]
name = "App"
root = "/tmp"
port = 3000
start = ["bash", "-c", "exec pnpm dev"]
"#;
        std::fs::write(&path, toml).unwrap();
        let mut loaded = load(Some(&path)).unwrap();

        // Invalid link URL
        let bad_link = loaded.update_project(
            "app",
            ProjectEdit {
                links: Some(vec![WebLink {
                    title: "Bad".to_string(),
                    url: "ftp://example.com".to_string(),
                }]),
                ..Default::default()
            },
        );
        assert!(bad_link.is_err());
        assert!(bad_link
            .unwrap_err()
            .to_string()
            .contains("must start with http:// or https://"));

        // Invalid profile key
        let mut bad_profiles = std::collections::BTreeMap::new();
        bad_profiles.insert(
            "bad key!".to_string(),
            ProfileConfig {
                label: "Bad".to_string(),
                start: Some("pnpm dev".to_string()),
                port: Some(3001),
                env: Default::default(),
            },
        );
        let bad_prof = loaded.update_project(
            "app",
            ProjectEdit {
                profiles: Some(bad_profiles),
                ..Default::default()
            },
        );
        assert!(bad_prof.is_err());
        assert!(bad_prof
            .unwrap_err()
            .to_string()
            .contains("profile key `bad key!` may only contain"));

        // default_profile not in profiles
        let bad_dp = loaded.update_project(
            "app",
            ProjectEdit {
                default_profile: Some("nonexistent".to_string()),
                ..Default::default()
            },
        );
        assert!(bad_dp.is_err());
        assert!(bad_dp
            .unwrap_err()
            .to_string()
            .contains("default_profile `nonexistent` does not exist in profiles"));
    }

    #[test]
    fn test_15_open_terminal_command_validations() {
        let missing_dir = Path::new("/definitely/nonexistent/terminal/dir");
        let err = open_terminal_command(missing_dir, "echo hi", "test").unwrap_err();
        assert!(err.to_string().contains("directory does not exist"));

        let temp_dir = std::env::temp_dir();
        let err2 = open_terminal_command(&temp_dir, "   ", "test").unwrap_err();
        assert!(err2.to_string().contains("command string cannot be empty"));
    }

    #[test]
    fn test_16_status_reflects_active_profile_and_custom_port() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_actprof_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = format!(
            r#"
[projects.app]
name = "App"
root = "{}"
port = 3000
log_dir = "{}/logs"
start = ["echo", "hi"]
health_url = "http://127.0.0.1:3000/health"

[projects.app.profiles.mock]
label = "Mock"
start = "echo mock"
port = 3001
"#,
            dir.display().to_string().replace('\\', "/"),
            dir.display().to_string().replace('\\', "/")
        );
        std::fs::write(&path, toml).unwrap();
        let loaded = load(Some(&path)).unwrap();

        // 1. Stopped status: port is base port 3000, active_profile is None
        let st_stopped = loaded.status("app").unwrap();
        assert_eq!(st_stopped.port, 3000);
        assert_eq!(st_stopped.active_profile, None);

        // 2. Simulate running process with active profile "mock"
        let pid_file = dir.join("logs/app.pid");
        let _ = std::fs::create_dir_all(dir.join("logs"));
        let my_pid = std::process::id();
        std::fs::write(&pid_file, my_pid.to_string()).unwrap();
        let prof_marker = profile_marker_path(&pid_file);
        std::fs::write(&prof_marker, "mock").unwrap();

        let st_active = loaded.status("app").unwrap();
        assert_eq!(st_active.port, 3001);
        assert_eq!(st_active.active_profile.as_deref(), Some("mock"));
        assert_eq!(st_active.login_probe.url, "http://127.0.0.1:3001/health");

        // 3. Clean up active profile marker
        let _ = std::fs::remove_file(&prof_marker);
        let st_no_prof = loaded.status("app").unwrap();
        assert_eq!(st_no_prof.port, 3000);
        assert_eq!(st_no_prof.active_profile, None);
        let _ = std::fs::remove_file(&pid_file);
    }

    #[test]
    fn test_17_listener_pids_detects_local_tcp_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let pids = listener_pids(port);
        assert!(
            pids.contains(&std::process::id()),
            "listener_pids({port}) = {:?} must contain current pid {}",
            pids,
            std::process::id()
        );
    }

    #[test]
    fn test_18_legacy_pid_migration_hints() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_legacy_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = format!(
            r#"
[projects.app]
name = "App"
root = "{}"
port = 39999
start = ["echo", "hi"]
"#,
            dir.display().to_string().replace('\\', "/")
        );
        std::fs::write(&path, toml).unwrap();

        // Write legacy PID file in <root>/logs/app.pid
        let legacy_logs = dir.join("logs");
        let _ = std::fs::create_dir_all(&legacy_logs);
        let legacy_pid = legacy_logs.join("app.pid");
        let my_pid = std::process::id();
        std::fs::write(&legacy_pid, my_pid.to_string()).unwrap();

        let loaded = load(Some(&path)).unwrap();
        let st = loaded.status("app").unwrap();
        assert!(
            st.hints
                .iter()
                .any(|h| h.contains("旧版 PID 文件") && h.contains(&my_pid.to_string())),
            "hints {:?} should mention legacy pid file migration",
            st.hints
        );

        let _ = std::fs::remove_file(&legacy_pid);
    }

    #[test]
    fn test_19_depends_on_topological_sorting_and_cycles() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_dep_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = r#"
[projects.api]
name = "API"
root = "/tmp"
port = 8080
start = ["echo", "api"]

[projects.web]
name = "Web"
root = "/tmp"
port = 3000
start = ["echo", "web"]
depends_on = ["api"]

[projects.admin]
name = "Admin"
root = "/tmp"
port = 3001
start = ["echo", "admin"]
depends_on = ["web"]
"#;
        std::fs::write(&path, toml).unwrap();
        let mut loaded = load(Some(&path)).unwrap();

        // 1. Verify sorted_projects_for_start orders api before web, web before admin
        let all_ids = ["admin", "web", "api"];
        let start_order = loaded.sorted_projects_for_start(&all_ids);
        assert_eq!(start_order, vec!["api", "web", "admin"]);

        // 2. Verify sorted_projects_for_stop reverses it: admin, web, api
        let stop_order = loaded.sorted_projects_for_stop(&all_ids);
        assert_eq!(stop_order, vec!["admin", "web", "api"]);

        // 3. Cycle detection when updating: api depends_on admin (forming admin -> web -> api -> admin cycle)
        let cycle_edit = loaded.update_project(
            "api",
            ProjectEdit {
                depends_on: Some(vec!["admin".to_string()]),
                ..Default::default()
            },
        );
        assert!(cycle_edit.is_err());
        assert!(cycle_edit
            .unwrap_err()
            .to_string()
            .contains("检测到循环依赖"));

        // 4. Self dependency error
        let self_dep = loaded.update_project(
            "api",
            ProjectEdit {
                depends_on: Some(vec!["api".to_string()]),
                ..Default::default()
            },
        );
        assert!(self_dep.is_err());
        assert!(self_dep
            .unwrap_err()
            .to_string()
            .contains("不能将自身作为依赖项"));

        // 5. Non-existent dependency error
        let non_exist = loaded.update_project(
            "api",
            ProjectEdit {
                depends_on: Some(vec!["ghost".to_string()]),
                ..Default::default()
            },
        );
        assert!(non_exist.is_err());
        assert!(non_exist
            .unwrap_err()
            .to_string()
            .contains("依赖的项目 `ghost` 不存在"));
    }

    #[test]
    fn group_members_order_starts_before_name_and_after_depends_on() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_members_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = r#"
[group.shop]
label = "Shop"
members = ["zeta_api", "alpha_web"]

[projects.alpha_web]
name = "Alpha Web"
group = "shop"
root = "/tmp"
port = 5173
start = ["echo", "web"]

[projects.zeta_api]
name = "Zeta API"
group = "shop"
root = "/tmp"
port = 8080
start = ["echo", "api"]
"#;
        std::fs::write(&path, toml).unwrap();
        let loaded = load(Some(&path)).unwrap();
        assert_eq!(
            loaded.projects_in_group("shop"),
            vec!["zeta_api".to_string(), "alpha_web".to_string()]
        );
        let order = loaded.sorted_projects_for_start(&["alpha_web", "zeta_api"]);
        assert_eq!(order, vec!["zeta_api".to_string(), "alpha_web".to_string()]);
        let info = loaded
            .groups()
            .into_iter()
            .find(|g| g.key == "shop")
            .unwrap();
        assert_eq!(
            info.members,
            vec!["zeta_api".to_string(), "alpha_web".to_string()]
        );

        let toml = r#"
[group.shop]
label = "Shop"
members = ["alpha_web", "zeta_api"]

[projects.alpha_web]
name = "Alpha Web"
group = "shop"
root = "/tmp"
port = 5173
start = ["echo", "web"]
depends_on = ["zeta_api"]

[projects.zeta_api]
name = "Zeta API"
group = "shop"
root = "/tmp"
port = 8080
start = ["echo", "api"]
"#;
        std::fs::write(&path, toml).unwrap();
        let loaded = load(Some(&path)).unwrap();
        let order = loaded.sorted_projects_for_start(&["alpha_web", "zeta_api"]);
        assert_eq!(
            order,
            vec!["zeta_api".to_string(), "alpha_web".to_string()],
            "depends_on still starts the backend before the frontend"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_start_failure_line_prefers_port_then_deps_then_exception() {
        let port = extract_start_failure_line(
            "Web server failed to start. Port 8080 was already in use.\n",
        )
        .unwrap();
        assert!(port.starts_with("端口被占用"));
        assert!(port.contains("8080"));
        assert!(!port.contains('\n'));

        let dep = extract_start_failure_line(
            "[ERROR] Failed to execute goal\n[ERROR] Could not resolve dependencies for project com.example:demo:jar:1.0\n",
        )
        .unwrap();
        assert!(dep.starts_with("依赖解析失败"));
        assert!(dep.contains("Could not resolve dependencies"));

        let npm = extract_start_failure_line(
            "npm ERR! code ERESOLVE\nnpm ERR! ERESOLVE unable to resolve dependency tree\n",
        )
        .unwrap();
        assert!(npm.starts_with("依赖解析失败"));

        let ex = extract_start_failure_line(
            "INFO started\nException in thread \"main\" java.lang.RuntimeException: wrapper\nCaused by: java.lang.IllegalStateException: boom\n\tat com.example.Demo.main(Demo.java:1)\n",
        )
        .unwrap();
        assert!(ex.starts_with("异常"));
        assert!(ex.contains("IllegalStateException"));
        assert!(!ex.contains("RuntimeException"));

        assert!(extract_start_failure_line("ready on port 8080\n").is_none());
        assert_eq!(
            display_command(&[
                "bash".into(),
                "-c".into(),
                "exec mvn spring-boot:run".into()
            ]),
            "exec mvn spring-boot:run"
        );
    }

    #[test]
    fn test_20_batch_start_and_stop_detached() {
        let dir = std::env::temp_dir().join(format!("tongyun_dev_batch_{}", current_time_secs()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        let toml = format!(
            r#"
[group.demo]
label = "Demo"

[projects.api]
name = "API"
group = "demo"
root = "{}"
port = 8080
start = ["echo", "api"]

[projects.web]
name = "Web"
group = "demo"
root = "{}"
port = 3000
start = ["echo", "web"]
depends_on = ["api"]

[projects.disabled_app]
name = "Disabled"
group = "demo"
root = "{}"
port = 9000
enabled = false
reason = "test disabled"
start = ["echo", "disabled"]
"#,
            dir.display().to_string().replace('\\', "/"),
            dir.display().to_string().replace('\\', "/"),
            dir.display().to_string().replace('\\', "/")
        );
        std::fs::write(&path, toml).unwrap();
        let loaded = load(Some(&path)).unwrap();

        // 1. Batch start detached for group "demo": only enabled projects (api, web) run
        let batch_res = loaded.start_batch_detached(Some("demo"), None).unwrap();
        assert_eq!(batch_res.total, 2);
        assert_eq!(batch_res.results[0].id, "api");
        assert_eq!(batch_res.results[1].id, "web");

        // 2. Batch stop: web stops before api
        let stop_res = loaded.stop_batch(Some("demo"), None).unwrap();
        assert_eq!(stop_res.total, 2);
        assert_eq!(stop_res.results[0].id, "web");
        assert_eq!(stop_res.results[1].id, "api");
    }

    #[test]
    fn batch_start_skips_downstream_when_dependency_fails() {
        let dir = std::env::temp_dir().join(format!(
            "tongyun_dev_skip_{}_{}",
            std::process::id(),
            current_time_secs()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let root = dir.display().to_string().replace('\\', "/");
        let path = dir.join("projects.toml");
        let toml = format!(
            r#"
[group.shop]
label = "Shop"
members = ["demo_api", "demo_web", "demo_ui"]

[projects.demo_api]
name = "Demo API"
group = "shop"
root = "{root}"
port = 8080
log_dir = "{root}/logs"
start = ["{root}/missing-api.sh"]

[projects.demo_web]
name = "Demo Web"
group = "shop"
root = "{root}"
port = 5173
log_dir = "{root}/logs"
start = ["{root}/missing-web.sh"]
depends_on = ["demo_api"]

[projects.demo_ui]
name = "Demo UI"
group = "shop"
root = "{root}"
port = 5174
log_dir = "{root}/logs"
start = ["{root}/missing-ui.sh"]
depends_on = ["demo_web"]
"#
        );
        std::fs::write(&path, toml).unwrap();
        let loaded = load(Some(&path)).unwrap();

        let batch = loaded.start_batch(Some("shop"), None, None).unwrap();
        assert_eq!(
            batch
                .results
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["demo_api", "demo_web", "demo_ui"]
        );
        assert!(!batch.results[0].ok);
        assert_eq!(batch.results[1].error.as_deref(), Some(DEP_START_SKIPPED));
        assert_eq!(batch.results[2].error.as_deref(), Some(DEP_START_SKIPPED));
        assert!(start_marker_path(&dir.join("logs/demo_api.pid")).is_file());
        assert!(!start_marker_path(&dir.join("logs/demo_web.pid")).exists());
        assert!(!start_marker_path(&dir.join("logs/demo_ui.pid")).exists());

        let detached = loaded.start_batch_detached(Some("shop"), None).unwrap();
        assert_eq!(
            detached.results[1].error.as_deref(),
            Some(DEP_START_SKIPPED)
        );
        assert!(!detached.results[1].ok);
        assert!(!start_marker_path(&dir.join("logs/demo_web.pid")).exists());

        let restarted = loaded.restart_batch(Some("shop"), None, None).unwrap();
        assert_eq!(
            restarted.results[1].error.as_deref(),
            Some(DEP_START_SKIPPED)
        );
        assert!(!start_marker_path(&dir.join("logs/demo_web.pid")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_start_commands_use_that_profiles_start_and_port() {
        let dir = std::env::temp_dir().join(format!(
            "tongyun_dev_pcmd_{}_{}",
            std::process::id(),
            current_time_secs()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let root = dir.display().to_string().replace('\\', "/");
        let path = dir.join("projects.toml");
        let toml = format!(
            r#"
[projects.demo_web]
name = "Demo Web"
root = "{root}"
port = 5173
port_mode = "arg"
log_dir = "{root}/logs"
start = ["bash", "-c", "exec pnpm dev"]
default_profile = "dev"

[projects.demo_web.profiles.dev]
label = "Dev"
start = "pnpm dev --host 127.0.0.1"

[projects.demo_web.profiles.mock]
label = "Mock"
start = "pnpm dev:mock"
port = 5174
"#
        );
        std::fs::write(&path, toml).unwrap();
        let loaded = load(Some(&path)).unwrap();
        let row = loaded
            .list()
            .unwrap()
            .projects
            .into_iter()
            .find(|p| p.id == "demo_web")
            .unwrap();
        assert!(
            row.start_command.contains("pnpm dev --host 127.0.0.1"),
            "{}",
            row.start_command
        );
        assert!(row.start_command.contains("5173"), "{}", row.start_command);
        let dev = row.profile_start_commands.get("dev").unwrap();
        let mock = row.profile_start_commands.get("mock").unwrap();
        assert!(dev.contains("pnpm dev --host 127.0.0.1"), "{dev}");
        assert!(dev.contains("5173"), "{dev}");
        assert!(mock.contains("pnpm dev:mock"), "{mock}");
        assert!(mock.contains("5174"), "{mock}");
        assert!(!mock.contains("5173"), "{mock}");
        let _ = std::fs::remove_dir_all(&dir);
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
        let home = Path::new("/home/demo");
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
        assert!(start_of("demo_web").contains("pnpm dev --host 127.0.0.1 --port 5173"));
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
        let home = Some(Path::new("/home/demo"));
        let web = resolve_project("web", cfg.projects.get("web").unwrap(), home).unwrap();
        // 手写 start 命令原样执行，不再改写
        assert_eq!(
            web.assembled_start(),
            vec!["bash", "-c", "exec pnpm dev -- --port 5173"]
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
        assert!(!r
            .active_start()
            .iter()
            .any(|a| a.contains("/bin/catalina.sh") && !a.contains("/opt/tomcat")));
        let text = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            text.contains("tongyun-dev 项目配置"),
            "template header kept"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unresolved_tomcat_is_not_rewritten_to_root_catalina() {
        let home = Path::new("/home/demo");
        let enabled = r#"
[projects.legacy]
name = "legacy"
root = "/opt/demo-legacy"
enabled = true
port = 8080
start = ["bash", "-c", "exec \"{{tomcat}}/bin/catalina.sh\" run"]
stop = ["bash", "{{tomcat}}/bin/catalina.sh", "stop", "15", "-force"]
"#;
        let cfg = parse_config(enabled).unwrap();
        let project = cfg.projects.get("legacy").unwrap();
        let err = resolve_project("legacy", project, Some(home)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("tomcat is not set"), "{msg}");
        assert!(!msg.contains("/bin/catalina.sh"), "{msg}");

        let disabled = enabled.replace(
            "enabled = true",
            "enabled = false\nreason = \"先填 Tomcat 安装目录\"",
        );
        let cfg = parse_config(&disabled).unwrap();
        let project = cfg.projects.get("legacy").unwrap();
        let resolved = resolve_project("legacy", project, Some(home)).unwrap();
        for arg in resolved.start.iter().chain(resolved.stop.iter()) {
            assert!(
                !arg.contains("/bin/catalina.sh") || arg.contains("{{tomcat}}"),
                "empty tomcat rewrote the command: {arg}"
            );
        }
        assert!(resolved.start.iter().any(|a| a.contains("{{tomcat}}")));
        assert!(resolved
            .stop
            .iter()
            .any(|a| a.contains("{{tomcat}}/bin/catalina.sh")));

        let d = tmp("notomcat");
        let cfg_path = d.join("projects.toml");
        std::fs::write(&cfg_path, enabled).unwrap();
        let loaded = load(Some(&cfg_path)).unwrap();
        let err = loaded.start("legacy").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("tomcat is not set"), "{msg}");
        assert!(!msg.contains("/bin/catalina.sh"), "{msg}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn scan_spring_boot_imports_with_profiles_and_default_profile() {
        let d = tmp("boot_profiles");
        let cfg = d.join("projects.toml");
        std::fs::write(&cfg, DEFAULT_CONFIG_TEMPLATE).unwrap();
        let mut loaded = load(Some(&cfg)).unwrap();

        let app_dir = d.join("demo-svc");
        write(
            &app_dir.join("pom.xml"),
            "<project><parent><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-parent</artifactId><version>3.2.0</version></parent><artifactId>demo-svc</artifactId><dependencies><dependency><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-web</artifactId></dependency></dependencies></project>",
        );
        write(&app_dir.join("src/main/resources/application.yml"), "server:\n  port: 8080\n");
        write(&app_dir.join("src/main/resources/application-dev.yml"), "server:\n  port: 8081\n");
        write(&app_dir.join("src/main/resources/application-prod.yml"), "server:\n  port: 8082\n");

        let report = loaded.scan(&path_to_string(&d), 2).unwrap();
        assert_eq!(report.candidates.len(), 1);
        let cand = &report.candidates[0];
        assert_eq!(cand.default_profile.as_deref(), Some("dev"));
        assert_eq!(cand.profiles.len(), 2);
        assert!(cand.profiles.contains_key("dev"));
        assert!(cand.profiles.contains_key("prod"));

        // User confirms and imports candidate
        let req = cand.to_new_project();
        assert_eq!(req.default_profile.as_deref(), Some("dev"));
        let out = loaded.import_scanned(vec![req]).unwrap();
        assert_eq!(out.added.len(), 1);
        assert_eq!(out.added[0].id, "demo-svc");

        // Verify config on disk
        let saved_toml = std::fs::read_to_string(&cfg).unwrap();
        assert!(saved_toml.contains("default_profile = \"dev\""));
        assert!(saved_toml.contains("[projects.demo-svc.profiles.dev]"));
        assert!(saved_toml.contains("[projects.demo-svc.profiles.prod]"));

        // Verify loaded resolution
        let reloaded = load(Some(&cfg)).unwrap();
        let resolved = reloaded.resolve("demo-svc").unwrap();
        let detail = reloaded.project_detail("demo-svc").unwrap();
        assert_eq!(detail.default_profile.as_deref(), Some("dev"));
        assert_eq!(resolved.port, 8081);
        let start_str = resolved.active_start().join(" ");
        assert!(start_str.contains("-Dspring-boot.run.profiles=dev"));

        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod port_mode_tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "tongyun_port_mode_{tag}_{}_{}",
            std::process::id(),
            current_time_secs()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn has_node() -> bool {
        std::process::Command::new("node")
            .arg("-v")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn pure_unit_test_append_port_arg() {
        // pnpm
        let pnpm_cmd = vec!["pnpm".to_string(), "dev".to_string()];
        assert_eq!(
            append_port_arg(&pnpm_cmd, 5173, None),
            vec!["pnpm", "dev", "--port", "5173"]
        );

        // yarn
        let yarn_cmd = vec!["yarn".to_string(), "dev".to_string()];
        assert_eq!(
            append_port_arg(&yarn_cmd, 5173, None),
            vec!["yarn", "dev", "--port", "5173"]
        );

        // npm run without double dash
        let npm_cmd = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];
        assert_eq!(
            append_port_arg(&npm_cmd, 5173, None),
            vec!["npm", "run", "dev", "--", "--port", "5173"]
        );

        // npm run with existing double dash
        let npm_dash_cmd = vec![
            "npm".to_string(),
            "run".to_string(),
            "dev".to_string(),
            "--".to_string(),
            "--host".to_string(),
        ];
        assert_eq!(
            append_port_arg(&npm_dash_cmd, 5173, None),
            vec!["npm", "run", "dev", "--", "--host", "--port", "5173"]
        );

        // other command
        let other_cmd = vec!["demo_api".to_string()];
        assert_eq!(
            append_port_arg(&other_cmd, 8080, None),
            vec!["demo_api", "--port", "8080"]
        );

        // custom port_arg flag
        assert_eq!(
            append_port_arg(&other_cmd, 8080, Some("-p")),
            vec!["demo_api", "-p", "8080"]
        );

        // already has --port or -p: do not duplicate
        let with_port = vec![
            "pnpm".to_string(),
            "dev".to_string(),
            "--port".to_string(),
            "3000".to_string(),
        ];
        assert_eq!(append_port_arg(&with_port, 5173, None), with_port);

        let with_p = vec![
            "pnpm".to_string(),
            "dev".to_string(),
            "-p".to_string(),
            "3000".to_string(),
        ];
        assert_eq!(append_port_arg(&with_p, 5173, None), with_p);

        let with_eq = vec![
            "pnpm".to_string(),
            "dev".to_string(),
            "--port=3000".to_string(),
        ];
        assert_eq!(append_port_arg(&with_eq, 5173, None), with_eq);

        // single line bash -c
        let bash_pnpm = vec![
            "bash".to_string(),
            "-c".to_string(),
            "exec pnpm dev --host 127.0.0.1".to_string(),
        ];
        assert_eq!(
            append_port_arg(&bash_pnpm, 5173, None),
            vec!["bash", "-c", "exec pnpm dev --host 127.0.0.1 --port 5173"]
        );

        let bash_npm = vec![
            "bash".to_string(),
            "-c".to_string(),
            "exec npm run dev".to_string(),
        ];
        assert_eq!(
            append_port_arg(&bash_npm, 5173, None),
            vec!["bash", "-c", "exec npm run dev -- --port 5173"]
        );

        let bash_npm_with_dash = vec![
            "bash".to_string(),
            "-c".to_string(),
            "exec npm run dev -- --host 127.0.0.1".to_string(),
        ];
        assert_eq!(
            append_port_arg(&bash_npm_with_dash, 5173, None),
            vec![
                "bash",
                "-c",
                "exec npm run dev -- --host 127.0.0.1 --port 5173"
            ]
        );

        // multi-line bash -c
        let bash_multi = vec![
            "bash".to_string(),
            "-c".to_string(),
            "# setup\ncd app\nexec pnpm dev".to_string(),
        ];
        assert_eq!(
            append_port_arg(&bash_multi, 5173, None),
            vec!["bash", "-c", "# setup\ncd app\nexec pnpm dev --port 5173"]
        );
    }

    #[test]
    fn manual_start_with_double_dash_is_not_rewritten() {
        let text = r#"
[projects.demo_app]
name = "Demo App"
root = "/tmp/demo_app"
port = 5555
start = ["bash", "-c", "exec pnpm dev -- --port 5555 --host 127.0.0.1"]
port_mode = "none"
"#;
        let cfg = parse_config(text).unwrap();
        let proj = resolve_project(
            "demo_app",
            cfg.projects.get("demo_app").unwrap(),
            Some(Path::new("/tmp")),
        )
        .unwrap();
        assert_eq!(
            proj.assembled_start(),
            vec![
                "bash",
                "-c",
                "exec pnpm dev -- --port 5555 --host 127.0.0.1"
            ]
        );
    }

    #[test]
    fn diagnose_warns_on_fixture_and_silent_on_vite() {
        let d = tmp("diagnose");

        // 1. Monorepo fixture with turbo.json + pnpm-workspace.yaml + node turbo-run script
        let mono_root = d.join("acme-monorepo");
        write(
            &mono_root.join("package.json"),
            r#"{"name":"acme-monorepo","private":true,"scripts":{"dev":"node scripts/turbo-run/index.js dev"}}"#,
        );
        write(&mono_root.join("turbo.json"), "{}");
        write(
            &mono_root.join("pnpm-workspace.yaml"),
            "packages: ['packages/*']",
        );

        let mono_start_with_port = vec![
            "bash".to_string(),
            "-c".to_string(),
            "exec pnpm dev -- --port 5555 --host 127.0.0.1".to_string(),
        ];
        let warn = diagnose_start_command(&mono_root, &mono_start_with_port);
        assert!(
            warn.is_some(),
            "should warn on monorepo with --port in start"
        );
        assert!(
            warn.unwrap()
                .contains("建议去掉 --port，端口传递方式选「不传」"),
            "should contain actionable suggestion"
        );

        let mono_start_without_port = vec![
            "bash".to_string(),
            "-c".to_string(),
            "exec pnpm dev --host 127.0.0.1".to_string(),
        ];
        assert_eq!(
            diagnose_start_command(&mono_root, &mono_start_without_port),
            None,
            "no warning when --port is not present"
        );

        // 2. Single package vite project: no warning even with --port
        let vite_root = d.join("acme-vite");
        write(
            &vite_root.join("package.json"),
            r#"{"name":"acme-vite","scripts":{"dev":"vite"}}"#,
        );
        let vite_start = vec![
            "bash".to_string(),
            "-c".to_string(),
            "exec pnpm dev -- --port 5173".to_string(),
        ];
        assert_eq!(
            diagnose_start_command(&vite_root, &vite_start),
            None,
            "single package vite should not trigger monorepo warning"
        );

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn turbo_monorepo_node_fixture_lifecycle() {
        if !has_node() {
            eprintln!("skipping turbo_monorepo_node_fixture_lifecycle: node not installed");
            return;
        }

        let d = tmp("turbo_fixture");
        let fixture_root = d.join("acme-monorepo");
        write(
            &fixture_root.join("package.json"),
            r#"{"name":"acme-monorepo","private":true,"scripts":{"dev":"node scripts/turbo-run/index.js dev"}}"#,
        );
        write(&fixture_root.join("turbo.json"), "{}");
        write(
            &fixture_root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n",
        );
        let _ = std::fs::create_dir_all(fixture_root.join("node_modules"));

        let node_script = r#"
const http = require('http');
const args = process.argv.slice(2);
if (args.includes('--port') || args.some(a => a.startsWith('--port='))) {
  console.error("ERROR Unknown option `--port`");
  process.exit(1);
}
const port = parseInt(process.env.PORT || '4777', 10);
const server = http.createServer((req, res) => {
  res.writeHead(200, { 'Content-Type': 'text/plain' });
  res.end('ok');
});
server.listen(port, '127.0.0.1', () => {
  console.log('Server ready on ' + port);
});
"#;
        write(
            &fixture_root.join("scripts/turbo-run/index.js"),
            node_script,
        );

        let log_dir = d.join("logs");
        let cfg_path = d.join("projects.toml");

        // 1. port_mode = "none": start command executes as-is, probe port 4777 succeeds
        let toml_none = format!(
            r#"
[projects.mono_none]
name = "Mono None"
root = "{root}"
port = 4777
start = ["node", "scripts/turbo-run/index.js", "dev"]
log_dir = "{log}"
port_mode = "none"
auto_install = false
background = true
"#,
            root = path_to_string(&fixture_root),
            log = path_to_string(&log_dir)
        );
        std::fs::write(&cfg_path, &toml_none).unwrap();

        let loaded = load(Some(&cfg_path)).unwrap();
        let _ = loaded.start_with_timeout("mono_none", Some(Duration::from_secs(5)));
        let mut ready = false;
        for _ in 0..30 {
            if let Ok(st) = loaded.status("mono_none") {
                if st.running && st.port_open {
                    ready = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            ready,
            "port_mode=none should start and port probe 4777 succeed"
        );
        let _ = loaded.stop("mono_none");

        // 2. port_mode = "arg": tongyun-dev automatically appends --port, reproduces Unknown option failure
        let toml_arg = format!(
            r#"
[projects.mono_arg]
name = "Mono Arg"
root = "{root}"
port = 4778
start = ["node", "scripts/turbo-run/index.js", "dev"]
log_dir = "{log}"
port_mode = "arg"
auto_install = false
background = true
"#,
            root = path_to_string(&fixture_root),
            log = path_to_string(&log_dir)
        );
        std::fs::write(&cfg_path, &toml_arg).unwrap();

        let loaded = load(Some(&cfg_path)).unwrap();
        let _ = loaded.start_with_timeout("mono_arg", Some(Duration::from_secs(2)));
        let mut failed_with_hint = false;
        for _ in 0..30 {
            if let Ok(st) = loaded.status("mono_arg") {
                if st.state == LifeState::Failed {
                    let fail_msg = st.failure.as_deref().unwrap_or("");
                    let has_advice = fail_msg.contains("建议去掉 --port，端口传递方式选「不传」")
                        || st
                            .hints
                            .iter()
                            .any(|h| h.contains("建议去掉 --port，端口传递方式选「不传」"));
                    if has_advice {
                        failed_with_hint = true;
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            failed_with_hint,
            "port_mode=arg should fail and contain actionable advice in failure/hints"
        );
        let _ = loaded.stop("mono_arg");

        // 3. port_mode = "env": child process receives PORT environment variable and starts on port 4779
        let toml_env = format!(
            r#"
[projects.mono_env]
name = "Mono Env"
root = "{root}"
port = 4779
start = ["node", "scripts/turbo-run/index.js", "dev"]
log_dir = "{log}"
port_mode = "env"
port_env = "PORT"
auto_install = false
background = true
"#,
            root = path_to_string(&fixture_root),
            log = path_to_string(&log_dir)
        );
        std::fs::write(&cfg_path, &toml_env).unwrap();

        let loaded = load(Some(&cfg_path)).unwrap();
        let _ = loaded.start_with_timeout("mono_env", Some(Duration::from_secs(5)));
        let mut env_ready = false;
        for _ in 0..30 {
            if let Ok(st) = loaded.status("mono_env") {
                if st.running && st.port_open {
                    env_ready = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            env_ready,
            "port_mode=env should start with PORT=4779 and probe succeed"
        );
        let _ = loaded.stop("mono_env");

        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod input_helpers_tests {
    use super::*;

    #[test]
    fn pure_unit_test_map_key_and_resolve_input_data() {
        // map_key tests for arrows
        assert_eq!(map_key("up"), Some("\x1b[A"));
        assert_eq!(map_key("UP"), Some("\x1b[A"));
        assert_eq!(map_key("down"), Some("\x1b[B"));
        assert_eq!(map_key("Down"), Some("\x1b[B"));
        assert_eq!(map_key("right"), Some("\x1b[C"));
        assert_eq!(map_key("RIGHT"), Some("\x1b[C"));
        assert_eq!(map_key("left"), Some("\x1b[D"));
        assert_eq!(map_key("Left"), Some("\x1b[D"));

        // Whitespace and escape keys
        assert_eq!(map_key("enter"), Some("\n"));
        assert_eq!(map_key("Enter"), Some("\n"));
        assert_eq!(map_key("tab"), Some("\t"));
        assert_eq!(map_key("TAB"), Some("\t"));
        assert_eq!(map_key("esc"), Some("\x1b"));
        assert_eq!(map_key("escape"), Some("\x1b"));
        assert_eq!(map_key("ESCAPE"), Some("\x1b"));

        // Control-C variants
        assert_eq!(map_key("ctrl-c"), Some("\x03"));
        assert_eq!(map_key("ctrl_c"), Some("\x03"));
        assert_eq!(map_key("ctrlc"), Some("\x03"));
        assert_eq!(map_key("CTRL-C"), Some("\x03"));

        // Unknown keys
        assert_eq!(map_key("unknown"), None);
        assert_eq!(map_key(""), None);
        assert_eq!(map_key("backspace"), None);
        assert_eq!(map_key("ctrl-d"), None);

        // resolve_input_data: text only
        assert_eq!(
            resolve_input_data(Some("hello"), false, None).unwrap(),
            "hello"
        );
        assert_eq!(
            resolve_input_data(Some("hello"), true, None).unwrap(),
            "hello\n"
        );

        // resolve_input_data: enter only (no text, no key)
        assert_eq!(resolve_input_data(None, true, None).unwrap(), "\n");

        // resolve_input_data: key only
        assert_eq!(
            resolve_input_data(None, false, Some("up")).unwrap(),
            "\x1b[A"
        );
        // when key is specified, enter = true does not add extra \n (has_key is true)
        assert_eq!(
            resolve_input_data(None, true, Some("ctrl-c")).unwrap(),
            "\x03"
        );

        // resolve_input_data: text and key combined
        assert_eq!(
            resolve_input_data(Some("select option"), false, Some("enter")).unwrap(),
            "select option\n"
        );
        assert_eq!(
            resolve_input_data(Some("item"), true, Some("tab")).unwrap(),
            "item\t"
        );

        // resolve_input_data: unknown key error
        let err_bad_key = resolve_input_data(None, false, Some("foo_bar")).unwrap_err();
        assert!(err_bad_key.to_string().contains("未知按键: foo_bar"));

        // resolve_input_data: empty input error
        let err_empty1 = resolve_input_data(None, false, None).unwrap_err();
        assert!(err_empty1.to_string().contains("empty input"));

        let err_empty2 = resolve_input_data(Some(""), false, None).unwrap_err();
        assert!(err_empty2.to_string().contains("empty input"));
    }
}

#[cfg(test)]
mod ordering_and_status_edge_cases_tests {
    use super::*;

    fn temp_test_config(tag: &str, toml_str: &str) -> (PathBuf, Loaded) {
        let dir = std::env::temp_dir().join(format!(
            "tongyun_test_edge_{tag}_{}_{}",
            std::process::id(),
            current_time_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("projects.toml");
        std::fs::write(&path, toml_str).unwrap();
        let loaded = load(Some(&path)).unwrap();
        (dir, loaded)
    }

    #[test]
    fn test_group_start_order_multi_tier_and_cycle_fallback() {
        let toml_str = r#"
[group.frontend]
label = "Frontend Layer"
order = 1
depends_on = ["gateway"]

[group.gateway]
label = "API Gateway"
order = 5
depends_on = ["backend"]

[group.backend]
label = "Backend Services"
order = 10
depends_on = ["database"]

[group.database]
label = "Database Layer"
order = 20

[group.cycle_b]
label = "Cycle B"
order = 50
depends_on = ["cycle_a"]

[group.cycle_a]
label = "Cycle A"
order = 40
depends_on = ["cycle_b"]

[group.orphan_dep]
label = "Orphan Dep Group"
order = 30
depends_on = ["non_existent_group"]

[projects.p_fe]
name = "Frontend Project"
group = "frontend"
root = "/tmp"
port = 3000
start = ["echo", "fe"]

[projects.p_db]
name = "DB Project"
group = "database"
root = "/tmp"
port = 5432
start = ["echo", "db"]
"#;
        let (dir, loaded) = temp_test_config("grp_order", toml_str);
        let order = loaded.group_start_order();
        assert_eq!(
            order,
            vec![
                "database",
                "backend",
                "gateway",
                "frontend",
                "orphan_dep",
                "cycle_a",
                "cycle_b"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_group_tie_breaking_order_and_inferred_groups() {
        let toml_str = r#"
[group.g_same_b]
label = "Beta Label"
order = 10

[group.g_same_a]
label = "Alpha Label"
order = 10

[group.g_same_a2]
label = "Alpha Label"
order = 10

[group.g_no_order_b]
label = "Beta None"

[group.g_no_order_a]
label = "Alpha None"

[projects.p_implicit]
name = "Implicit Project"
group = "inferred_group"
root = "/tmp"
port = 8000
start = ["echo", "hi"]
"#;
        let (dir, loaded) = temp_test_config("tie_break", toml_str);
        let groups = loaded.groups();
        let group_keys: Vec<String> = groups.into_iter().map(|g| g.key).collect();
        assert_eq!(
            group_keys,
            vec![
                "g_same_a",
                "g_same_a2",
                "g_same_b",
                "g_no_order_a",
                "g_no_order_b",
                "inferred_group"
            ]
        );

        let start_order = loaded.group_start_order();
        assert_eq!(
            start_order,
            vec![
                "g_same_a",
                "g_same_a2",
                "g_same_b",
                "g_no_order_a",
                "g_no_order_b",
                "inferred_group"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_sorted_projects_for_start_and_stop_edge_cases() {
        let toml_str = r#"
[group.g_early]
label = "Early Group"
order = 1

[group.g_late]
label = "Late Group"
order = 2

[group.grp_ordered]
label = "Ordered Members"
order = 3
members = ["proj_second_in_name", "proj_first_in_name"]

# Cross group dependency overriding group order
[projects.p_high]
name = "P High"
group = "g_early"
root = "/tmp"
port = 3000
start = ["echo", "high"]
depends_on = ["p_dep"]

[projects.p_dep]
name = "P Dep"
group = "g_late"
root = "/tmp"
port = 3001
start = ["echo", "dep"]

# Diamond dependency: base -> (mid1, mid2) -> top
[projects.p_base]
name = "Base"
root = "/tmp"
port = 3002
start = ["echo", "base"]

[projects.p_mid1]
name = "Mid 1"
root = "/tmp"
port = 3003
start = ["echo", "mid1"]
depends_on = ["p_base"]

[projects.p_mid2]
name = "Mid 2"
root = "/tmp"
port = 3004
start = ["echo", "mid2"]
depends_on = ["p_base"]

[projects.p_top]
name = "Top"
root = "/tmp"
port = 3005
start = ["echo", "top"]
depends_on = ["p_mid1", "p_mid2"]

# Members order in group
[projects.proj_first_in_name]
name = "Alpha Member"
group = "grp_ordered"
root = "/tmp"
port = 3006
start = ["echo", "alpha"]

[projects.proj_second_in_name]
name = "Beta Member"
group = "grp_ordered"
root = "/tmp"
port = 3007
start = ["echo", "beta"]

[projects.proj_unlisted]
name = "Gamma Unlisted"
group = "grp_ordered"
root = "/tmp"
port = 3008
start = ["echo", "gamma"]

# Project cycle
[projects.p_c1]
name = "Cycle 1"
root = "/tmp"
port = 3009
start = ["echo", "c1"]
depends_on = ["p_c2"]

[projects.p_c2]
name = "Cycle 2"
root = "/tmp"
port = 3010
start = ["echo", "c2"]
depends_on = ["p_c1"]

[projects.p_solo]
name = "Solo"
root = "/tmp"
port = 3011
start = ["echo", "solo"]
"#;
        let (dir, loaded) = temp_test_config("proj_order", toml_str);

        // 1. Cross-group dependency: p_dep (in late group) must start before p_high (in early group)
        let cross_order = loaded.sorted_projects_for_start(&["p_high", "p_dep"]);
        assert_eq!(cross_order, vec!["p_dep", "p_high"]);
        let cross_stop = loaded.sorted_projects_for_stop(&["p_high", "p_dep"]);
        assert_eq!(cross_stop, vec!["p_high", "p_dep"]);

        // 2. Diamond dependency
        let diamond_order =
            loaded.sorted_projects_for_start(&["p_top", "p_mid2", "p_mid1", "p_base"]);
        assert_eq!(diamond_order, vec!["p_base", "p_mid1", "p_mid2", "p_top"]);
        let diamond_stop =
            loaded.sorted_projects_for_stop(&["p_top", "p_mid2", "p_mid1", "p_base"]);
        assert_eq!(diamond_stop, vec!["p_top", "p_mid2", "p_mid1", "p_base"]);

        // 3. Member index priority in group members
        let mem_order = loaded.sorted_projects_for_start(&[
            "proj_first_in_name",
            "proj_unlisted",
            "proj_second_in_name",
        ]);
        assert_eq!(
            mem_order,
            vec!["proj_second_in_name", "proj_first_in_name", "proj_unlisted"]
        );

        // 4. Dependency on a project not in the ids slice
        let partial_order = loaded.sorted_projects_for_start(&["p_top"]);
        assert_eq!(partial_order, vec!["p_top"]);

        // 5. Project cycle fallback preserves slice input order for cyclic nodes
        let cycle_order = loaded.sorted_projects_for_start(&["p_c1", "p_solo", "p_c2"]);
        assert_eq!(cycle_order, vec!["p_solo", "p_c1", "p_c2"]);
        let cycle_stop = loaded.sorted_projects_for_stop(&["p_c1", "p_solo", "p_c2"]);
        assert_eq!(cycle_stop, vec!["p_c2", "p_c1", "p_solo"]);

        // 6. Ungrouped vs grouped project
        let grp_vs_none = loaded.sorted_projects_for_start(&["p_base", "p_dep"]);
        assert_eq!(grp_vs_none, vec!["p_dep", "p_base"]);

        // 7. Unknown project ID in ids slice
        let unknown_order = loaded.sorted_projects_for_start(&["p_dep", "non_existent_project"]);
        assert_eq!(unknown_order, vec!["p_dep", "non_existent_project"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_status_json_serialization_all_lifestates_and_fields() {
        // 1. LifeState serialization and deserialization
        let states = [
            (LifeState::Stopped, "stopped"),
            (LifeState::Installing, "installing"),
            (LifeState::Starting, "starting"),
            (LifeState::PortOpen, "port_open"),
            (LifeState::Ready, "ready"),
            (LifeState::Failed, "failed"),
        ];
        for (state, expected) in states {
            let serialized = serde_json::to_string(&state).unwrap();
            assert_eq!(serialized, format!("\"{expected}\""));
            let deserialized: LifeState = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, state);
            assert_eq!(format!("{state}"), expected);
        }

        // 2. StatusJson field omission (skip_serializing_if)
        let minimal_st = StatusJson {
            id: "demo_app".into(),
            name: "Demo App".into(),
            running: false,
            state: LifeState::Stopped,
            elapsed_secs: None,
            timeout_secs: 180,
            failure: None,
            pid: None,
            port: 8080,
            port_open: false,
            login_probe: LoginProbe {
                url: String::new(),
                http_code: None,
                ok: false,
            },
            log_path: "/tmp/logs/demo.out".into(),
            pid_path: "/tmp/logs/demo.pid".into(),
            root: "/tmp/demo".into(),
            port_conflict: false,
            port_holder: String::new(),
            deps_missing: false,
            install_command: String::new(),
            hints: Vec::new(),
            active_profile: None,
            resolve_error: None,
        };
        let min_json = serde_json::to_string(&minimal_st).unwrap();
        assert!(
            !min_json.contains("\"hints\""),
            "empty hints must be omitted"
        );
        assert!(
            !min_json.contains("\"active_profile\""),
            "None active_profile must be omitted"
        );
        assert!(
            !min_json.contains("\"resolve_error\""),
            "None resolve_error must be omitted"
        );
        assert!(min_json.contains("\"elapsed_secs\":null"));
        assert!(min_json.contains("\"failure\":null"));
        assert!(min_json.contains("\"pid\":null"));
        assert!(min_json.contains("\"http_code\":null"));

        // 3. StatusJson populated fields inclusion
        let mut populated_st = minimal_st.clone();
        populated_st.running = true;
        populated_st.state = LifeState::Ready;
        populated_st.elapsed_secs = Some(42);
        populated_st.failure = Some("warning test".into());
        populated_st.pid = Some(12345);
        populated_st.login_probe = LoginProbe {
            url: "http://127.0.0.1:8080/health".into(),
            http_code: Some(200),
            ok: true,
        };
        populated_st.hints = vec!["Check firewall".into()];
        populated_st.active_profile = Some("dev".into());
        populated_st.resolve_error = Some("test warning".into());

        let pop_json = serde_json::to_string(&populated_st).unwrap();
        assert!(pop_json.contains("\"hints\":[\"Check firewall\"]"));
        assert!(pop_json.contains("\"active_profile\":\"dev\""));
        assert!(pop_json.contains("\"resolve_error\":\"test warning\""));
        assert!(pop_json.contains("\"elapsed_secs\":42"));
        assert!(pop_json.contains("\"pid\":12345"));
        assert!(pop_json.contains("\"http_code\":200"));
        assert!(pop_json.contains("\"ok\":true"));

        // 4. Backward-compatible deserialization of legacy JSON missing newer fields
        let legacy_json = r#"{
            "id": "legacy_service",
            "name": "Legacy Service",
            "running": false,
            "state": "stopped",
            "elapsed_secs": null,
            "timeout_secs": 60,
            "failure": null,
            "pid": null,
            "port": 3000,
            "port_open": false,
            "login_probe": {
                "url": "",
                "http_code": null,
                "ok": false
            },
            "log_path": "/tmp/legacy.log",
            "pid_path": "/tmp/legacy.pid",
            "root": "/tmp/legacy",
            "port_conflict": false,
            "port_holder": ""
        }"#;
        let parsed_legacy: StatusJson = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed_legacy.id, "legacy_service");
        assert_eq!(parsed_legacy.deps_missing, false);
        assert_eq!(parsed_legacy.install_command, "");
        assert!(parsed_legacy.hints.is_empty());
        assert_eq!(parsed_legacy.active_profile, None);
        assert_eq!(parsed_legacy.resolve_error, None);
    }

    #[test]
    fn test_unresolved_status_and_batch_serialization() {
        // 1. unresolved_status helper
        let row = ProjectRow {
            id: "broken_app".into(),
            name: "Broken App".into(),
            group: None,
            root: "/tmp/broken".into(),
            enabled: false,
            port: 9090,
            reason: "syntax error".into(),
            configured: false,
            start_timeout_secs: None,
            stop_grace_secs: None,
            node: None,
            java: None,
            java_home: None,
            node_home: None,
            shell: None,
            auto_install: true,
            port_mode: "none".into(),
            port_arg: None,
            port_env: None,
            env: Default::default(),
            pty: false,
            auto_answers: Vec::new(),
            links: Vec::new(),
            default_profile: None,
            profiles: Default::default(),
            depends_on: Vec::new(),
            start_command: String::new(),
            profile_start_commands: Default::default(),
            resolve_error: Some("failed to parse log_dir".into()),
        };
        let st = unresolved_status(&row, "Cannot resolve log dir");
        assert_eq!(st.id, "broken_app");
        assert_eq!(st.state, LifeState::Stopped);
        assert!(!st.running);
        assert_eq!(st.failure.as_deref(), Some("Cannot resolve log dir"));
        assert_eq!(st.resolve_error.as_deref(), Some("failed to parse log_dir"));
        assert_eq!(st.port, 9090);
        assert!(!st.port_conflict);
        assert!(st.hints.is_empty());

        let st_json = serde_json::to_string(&st).unwrap();
        assert!(st_json.contains("\"failure\":\"Cannot resolve log dir\""));
        assert!(st_json.contains("\"resolve_error\":\"failed to parse log_dir\""));
        assert!(!st_json.contains("\"hints\""));

        // 2. BatchItemResult serialization & deserialization
        let item_empty = BatchItemResult {
            id: "svc_a".into(),
            success: true,
            ok: true,
            status: None,
            error: None,
        };
        let item_empty_json = serde_json::to_string(&item_empty).unwrap();
        assert!(!item_empty_json.contains("\"status\""));
        assert!(!item_empty_json.contains("\"error\""));

        let item_with_status = BatchItemResult {
            id: "svc_b".into(),
            success: false,
            ok: false,
            status: Some(st.clone()),
            error: Some("failed execution".into()),
        };
        let item_with_status_json = serde_json::to_string(&item_with_status).unwrap();
        assert!(item_with_status_json.contains("\"status\":{"));
        assert!(item_with_status_json.contains("\"error\":\"failed execution\""));

        // 3. BatchOutput serialization round-trip
        let batch_out = BatchOutput {
            total: 2,
            succeeded: 1,
            failed: 1,
            results: vec![item_empty, item_with_status],
        };
        let batch_json = serde_json::to_string(&batch_out).unwrap();
        let parsed_batch: BatchOutput = serde_json::from_str(&batch_json).unwrap();
        assert_eq!(parsed_batch.total, 2);
        assert_eq!(parsed_batch.succeeded, 1);
        assert_eq!(parsed_batch.failed, 1);
        assert_eq!(parsed_batch.results.len(), 2);
        assert_eq!(parsed_batch.results[0].id, "svc_a");
        assert_eq!(parsed_batch.results[1].id, "svc_b");

        // 4. GroupInfo serialization omission of empty members/depends_on
        let group_info = GroupInfo {
            key: "test_grp".into(),
            label: "Test Group".into(),
            order: None,
            members: Vec::new(),
            depends_on: Vec::new(),
        };
        let grp_json = serde_json::to_string(&group_info).unwrap();
        assert!(!grp_json.contains("\"members\""));
        assert!(!grp_json.contains("\"depends_on\""));
        assert!(grp_json.contains("\"order\":null"));
    }

    #[test]
    fn derive_state_failure_edge_cases() {
        let base = StateInput {
            process_alive: false,
            port_open: false,
            port_conflict: false,
            has_probe: false,
            probe_ok: false,
            started_at: None,
            now: 100,
            timeout_secs: 10,
            start_error: None,
        };

        // Empty start_error still wins and is surfaced verbatim.
        let empty = derive_state(StateInput {
            start_error: Some(String::new()),
            ..base.clone()
        });
        assert_eq!(empty, (LifeState::Failed, Some(String::new())));

        // Whitespace-only start_error is not trimmed.
        let blank = derive_state(StateInput {
            start_error: Some("   ".to_string()),
            ..base.clone()
        });
        assert_eq!(blank, (LifeState::Failed, Some("   ".to_string())));

        // start_error beats an otherwise-Ready input.
        let err_over_ready = derive_state(StateInput {
            process_alive: true,
            port_open: true,
            probe_ok: true,
            has_probe: true,
            started_at: Some(100),
            start_error: Some("启动失败".to_string()),
            ..base.clone()
        });
        assert_eq!(
            err_over_ready,
            (LifeState::Failed, Some("启动失败".to_string()))
        );

        // Process exit inside the grace window is not yet a failure.
        let in_grace = derive_state(StateInput {
            started_at: Some(100),
            now: 100 + START_EXIT_GRACE_SECS - 1,
            ..base.clone()
        });
        assert_eq!(in_grace, (LifeState::Starting, None));

        // Process exit exactly at the grace boundary fails.
        let at_grace = derive_state(StateInput {
            started_at: Some(100),
            now: 100 + START_EXIT_GRACE_SECS,
            ..base.clone()
        });
        assert_eq!(
            at_grace,
            (LifeState::Failed, Some("进程已退出".to_string()))
        );

        // Timeout wording carries the configured budget.
        let timeout = derive_state(StateInput {
            process_alive: true,
            started_at: Some(100),
            now: 100 + 45,
            timeout_secs: 45,
            ..base.clone()
        });
        assert_eq!(
            timeout,
            (LifeState::Failed, Some("超过 45 秒仍未就绪".to_string()))
        );

        // timeout_secs = 0 fails immediately with the zero wording.
        let zero_timeout = derive_state(StateInput {
            process_alive: true,
            started_at: Some(100),
            now: 100,
            timeout_secs: 0,
            ..base.clone()
        });
        assert_eq!(
            zero_timeout,
            (LifeState::Failed, Some("超过 0 秒仍未就绪".to_string()))
        );

        // Port conflict suppresses PortOpen; alive process stays Starting.
        let conflict_no_marker = derive_state(StateInput {
            process_alive: true,
            port_open: true,
            port_conflict: true,
            has_probe: true,
            probe_ok: true,
            ..base.clone()
        });
        assert_eq!(conflict_no_marker, (LifeState::Starting, None));

        // Port open, no marker, probe configured but failing -> PortOpen.
        let no_marker_probe_fail = derive_state(StateInput {
            port_open: true,
            has_probe: true,
            probe_ok: false,
            ..base.clone()
        });
        assert_eq!(no_marker_probe_fail, (LifeState::PortOpen, None));

        // Port open, no marker, no probe -> Ready.
        let no_marker_no_probe = derive_state(StateInput {
            port_open: true,
            ..base.clone()
        });
        assert_eq!(no_marker_no_probe, (LifeState::Ready, None));
    }

    #[test]
    fn extract_start_failure_line_edge_cases() {
        // Empty / whitespace-only / non-failure logs produce no reason.
        assert_eq!(extract_start_failure_line(""), None);
        assert_eq!(extract_start_failure_line("\n  \n\t\n"), None);
        assert_eq!(extract_start_failure_line("ready on port 8080\n"), None);

        // Last port-in-use line wins and whitespace is collapsed.
        let port = extract_start_failure_line(
            "Address already in use: 3000\nPort   8080   was already in use\n",
        )
        .unwrap();
        assert!(port.starts_with("端口被占用"));
        assert!(port.contains("Port 8080 was already in use"));
        assert!(!port.contains("3000"));

        // Chinese port phrase is recognised and prefixed once.
        let zh = extract_start_failure_line("端口已被占用\n").unwrap();
        assert_eq!(zh, "端口被占用：端口已被占用");

        // Dependency resolution outranks an exception even when it appears earlier.
        let dep_beats_ex = extract_start_failure_line(
            "Exception in thread \"main\"\nCould not resolve dependencies for project a:b:1\n",
        )
        .unwrap();
        assert!(dep_beats_ex.starts_with("依赖解析失败"));

        // "failed to resolve" is treated as a dependency problem.
        let resolve = extract_start_failure_line("failed to resolve module foo\n").unwrap();
        assert!(resolve.starts_with("依赖解析失败"));

        // ANSI escapes are stripped before matching.
        let ansi =
            extract_start_failure_line("\u{1b}[31mAddress already in use\u{1b}[0m\n").unwrap();
        assert!(ansi.starts_with("端口被占用"));
        assert!(!ansi.contains('\u{1b}'));

        // A bare "Caused by:" line is surfaced as 异常.
        let caused = extract_start_failure_line("Caused by: java.io.IOException\n").unwrap();
        assert_eq!(caused, "异常：Caused by: java.io.IOException");

        // Over-long reasons are truncated to 180 chars plus an ellipsis.
        let long = format!("Exception {}", "x".repeat(240));
        let truncated = extract_start_failure_line(&long).unwrap();
        assert!(truncated.starts_with("异常："));
        assert!(truncated.ends_with('…'));
        assert_eq!(truncated.chars().count(), "异常：".chars().count() + 181);

        // "error:" style lines need at least 8 chars.
        assert_eq!(extract_start_failure_line("error:a\n"), None);
        assert!(extract_start_failure_line("error: boom\n").is_some());

        // Maven 401/403 stays visible even when the transfer line is huge.
        let auth = format!(
            "[ERROR] Could not resolve dependencies for project demo: authentication failed, status: 401 Unauthorized {}\n",
            "z".repeat(400)
        );
        assert_eq!(
            extract_start_failure_line(&auth).as_deref(),
            Some("认证失败：Maven 仓库返回 401")
        );
        assert_eq!(
            extract_start_failure_line("Return code is: 403, ReasonPhrase: Forbidden.\n")
                .as_deref(),
            Some("认证失败：Maven 仓库返回 403")
        );
        assert_eq!(extract_start_failure_line("status: 4010 extra\n"), None);
        assert_eq!(extract_start_failure_line("elapsed 401 ms\n"), None);
        let port_wins =
            extract_start_failure_line("status: 401 Unauthorized\nAddress already in use\n")
                .unwrap();
        assert!(port_wins.starts_with("端口被占用"));
    }

    #[test]
    fn prefix_failure_and_log_helpers_edge_cases() {
        assert_eq!(
            prefix_failure("异常", "java.lang.Exception"),
            "异常：java.lang.Exception"
        );
        // Already-prefixed content is returned unchanged.
        assert_eq!(prefix_failure("端口被占用", "端口被占用"), "端口被占用");
        // Truncation applies before prefixing.
        let long = "y".repeat(200);
        let out = prefix_failure("异常", &long);
        assert!(out.starts_with("异常："));
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), "异常：".chars().count() + 181);

        assert_eq!(collapse_ws("  a \t b\n c "), "a b c");
        assert_eq!(collapse_ws(""), "");
        assert_eq!(strip_ansi_line("\u{1b}[32mok\u{1b}[0m"), "ok");
        assert_eq!(strip_ansi_line("plain"), "plain");
        assert_eq!(
            log_lines(" a \n\n b \n"),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn failure_allows_log_line_edge_cases() {
        assert!(failure_allows_log_line("进程已退出"));
        assert!(failure_allows_log_line("进程已退出：some tail"));
        assert!(failure_allows_log_line("启动失败"));
        assert!(failure_allows_log_line("超过 10 秒仍未就绪"));
        assert!(failure_allows_log_line("超过 0 秒仍未就绪"));

        assert!(!failure_allows_log_line(
            "端口被占用：Address already in use"
        ));
        assert!(!failure_allows_log_line("依赖解析失败：Could not resolve"));
        assert!(!failure_allows_log_line(
            "缺 Java 8：未在 ~/.sdkman 找到 8.*"
        ));
        assert!(!failure_allows_log_line("缺 Node 18：未找到 node"));
        assert!(!failure_allows_log_line("依赖未安装：node_modules missing"));
        assert!(!failure_allows_log_line("已停用：disabled"));
        assert!(!failure_allows_log_line("project disabled"));
        assert!(!failure_allows_log_line(
            "建议去掉 --port，端口传递方式选「不传」"
        ));
        assert!(!failure_allows_log_line(""));
    }

    #[test]
    fn unresolved_status_empty_failure() {
        let row = ProjectRow {
            id: "demo_api".into(),
            name: "Demo API".into(),
            group: None,
            root: "~/code/demo/api".into(),
            enabled: true,
            port: 3000,
            reason: String::new(),
            configured: true,
            start_timeout_secs: None,
            stop_grace_secs: None,
            node: None,
            java: None,
            java_home: None,
            node_home: None,
            shell: None,
            auto_install: true,
            port_mode: "none".into(),
            port_arg: None,
            port_env: None,
            env: Default::default(),
            pty: false,
            auto_answers: Vec::new(),
            links: Vec::new(),
            default_profile: None,
            profiles: Default::default(),
            depends_on: Vec::new(),
            start_command: String::new(),
            profile_start_commands: Default::default(),
            resolve_error: None,
        };
        let st = unresolved_status(&row, String::new());
        assert_eq!(st.state, LifeState::Stopped);
        assert!(!st.running);
        assert_eq!(st.failure.as_deref(), Some(""));
        assert_eq!(st.port, 3000);
        let json = serde_json::to_string(&st).unwrap();
        assert!(json.contains("\"failure\":\"\""));
    }
}

#[cfg(test)]
mod core7_group_key_ordering_and_failure_tests {
    use super::*;

    fn temp_loaded(tag: &str, toml_str: &str) -> (PathBuf, Loaded) {
        let dir = std::env::temp_dir().join(format!(
            "tongyun_dev_core7_{tag}_{}_{}",
            std::process::id(),
            current_time_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("projects.toml");
        std::fs::write(&path, toml_str).unwrap();
        let loaded = load(Some(&path)).unwrap();
        (dir, loaded)
    }

    // ---- group key validation ----

    #[test]
    fn validate_group_key_boundaries_and_messages() {
        assert!(
            validate_group_key(&"a".repeat(40)).is_ok(),
            "40 chars is the max and must pass"
        );
        assert!(
            validate_group_key(&"z".repeat(41)).is_err(),
            "41 chars is too long"
        );
        assert!(validate_group_key("0").is_ok());
        assert!(validate_group_key("1234567890").is_ok());
        assert!(validate_group_key("_").is_ok());
        assert!(validate_group_key("-").is_ok());
        assert!(validate_group_key("a_b-c9").is_ok());

        // no trimming here: surrounding whitespace is part of the key and rejected.
        assert!(validate_group_key(" demo").is_err());
        assert!(validate_group_key("demo ").is_err());
        assert!(validate_group_key("de mo").is_err());
        assert!(validate_group_key("de\tmo").is_err());
        assert!(validate_group_key("de\nmo").is_err());
        assert!(validate_group_key("demo.api").is_err());
        assert!(validate_group_key("demo/api").is_err());
        assert!(validate_group_key("Demo").is_err());
        assert!(validate_group_key("DEMO").is_err());
        assert!(validate_group_key("分组").is_err());

        // exact error wording for each branch.
        assert_eq!(
            validate_group_key("").unwrap_err().to_string(),
            "分组 key 不能为空"
        );
        assert_eq!(
            validate_group_key(&"a".repeat(41)).unwrap_err().to_string(),
            "分组 key 长度不能超过 40 个字符"
        );
        assert_eq!(
            validate_group_key("Bad Key").unwrap_err().to_string(),
            "分组 key 只能包含小写字母、数字、下划线和连字符: `Bad Key`"
        );
    }

    #[test]
    fn generate_group_key_uncovered_shapes() {
        // labels that collapse to nothing fall back to a generated key.
        assert_eq!(generate_group_key("@@@", &[] as &[&str]), "g1");
        assert_eq!(generate_group_key("-", &[] as &[&str]), "g1");
        assert_eq!(generate_group_key("---", &[] as &[&str]), "g1");
        assert_eq!(generate_group_key("   ", &[] as &[&str]), "g1");

        // consecutive illegal runs collapse to a single hyphen; legal chars kept.
        assert_eq!(generate_group_key("a@@@b", &[] as &[&str]), "a-b");
        assert_eq!(generate_group_key("V1_2-3", &[] as &[&str]), "v1_2-3");

        // surrounding whitespace ignored, uppercase lowered.
        assert_eq!(generate_group_key("  Demo  ", &[] as &[&str]), "demo");

        // longer than 40 chars: truncate, then drop a trailing hyphen.
        assert_eq!(
            generate_group_key(&"a".repeat(50), &[] as &[&str]),
            "a".repeat(40)
        );
        let trailing = format!("{}-zzzzzzzzzzzzzzzzzz", "a".repeat(39));
        assert_eq!(
            generate_group_key(&trailing, &[] as &[&str]),
            "a".repeat(39)
        );

        // a truncated candidate that already exists falls back to g{n}.
        let existing = vec!["a".repeat(40)];
        assert_eq!(generate_group_key(&"a".repeat(50), &existing), "g1");

        // existing keys may be owned Strings, and g{n} probes skip taken keys.
        let owned = vec!["g1".to_string(), "g3".to_string()];
        assert_eq!(generate_group_key("中文", owned), "g2");
    }

    #[test]
    fn create_and_rename_group_enforce_key_validation() {
        let toml_str = r#"
[projects.demo]
name = "Demo"
root = "/tmp"
port = 3000
start = ["echo", "hi"]
"#;
        let (dir, mut loaded) = temp_loaded("grp_crud", toml_str);

        assert!(loaded.create_group("Bad Key", Some("x")).is_err());
        assert_eq!(
            loaded
                .create_group("bad.key", None)
                .unwrap_err()
                .to_string(),
            "分组 key 只能包含小写字母、数字、下划线和连字符: `bad.key`"
        );

        // the key is trimmed before validation and storage.
        let created = loaded
            .create_group("  demo_grp  ", Some("Demo Group"))
            .unwrap();
        assert_eq!(created.key, "demo_grp");
        assert_eq!(created.label, "Demo Group");
        // duplicate detection uses the trimmed key.
        assert_eq!(
            loaded
                .create_group("demo_grp", None)
                .unwrap_err()
                .to_string(),
            "分组 `demo_grp` 已存在"
        );

        // empty label is rejected before touching the file.
        assert_eq!(
            loaded
                .rename_group_label("demo_grp", "   ")
                .unwrap_err()
                .to_string(),
            "分组 label 不能为空"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_or_create_group_matches_case_insensitively_and_generates_keys() {
        let toml_str = r#"
[group.shop]
label = "Shop"

[projects.p]
name = "P"
root = "/tmp"
port = 1
start = ["echo", "x"]
"#;
        let (dir, mut loaded) = temp_loaded("resolve_grp", toml_str);
        assert_eq!(
            loaded.resolve_or_create_group("shop").unwrap().as_deref(),
            Some("shop")
        );
        assert_eq!(
            loaded.resolve_or_create_group("SHOP").unwrap().as_deref(),
            Some("shop")
        );
        assert_eq!(
            loaded
                .resolve_or_create_group("  Shop  ")
                .unwrap()
                .as_deref(),
            Some("shop")
        );
        let key = loaded.resolve_or_create_group("New Team").unwrap().unwrap();
        assert_eq!(key, "new-team");
        // the freshly generated key is found case-insensitively too.
        assert_eq!(
            loaded
                .resolve_or_create_group("NEW-TEAM")
                .unwrap()
                .as_deref(),
            Some("new-team")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- depends_on ordering helpers ----

    #[test]
    fn depends_on_of_returns_direct_dependencies() {
        let toml_str = r#"
[projects.a]
name = "A"
root = "/tmp"
port = 1
start = ["echo", "a"]
depends_on = ["b", "c"]

[projects.b]
name = "B"
root = "/tmp"
port = 2
start = ["echo", "b"]
"#;
        let (dir, loaded) = temp_loaded("dep_of", toml_str);
        assert_eq!(
            loaded.depends_on_of("a"),
            vec!["b".to_string(), "c".to_string()]
        );
        assert!(loaded.depends_on_of("b").is_empty());
        assert!(loaded.depends_on_of("ghost").is_empty());
        // the id is trimmed before lookup.
        assert_eq!(
            loaded.depends_on_of("  a  "),
            vec!["b".to_string(), "c".to_string()]
        );
        // returns an owned copy: mutating it does not change the config.
        let mut copy = loaded.depends_on_of("a");
        copy.push("zzz".into());
        assert_eq!(
            loaded.depends_on_of("a"),
            vec!["b".to_string(), "c".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn contains_group_and_projects_in_group_semantics() {
        let toml_str = r#"
[group.declared_empty]
label = "Declared Empty"

[group.members_only]
members = ["ghost", "p_member", "p_member"]

[group.other]

[projects.p_member]
name = "Member"
group = "members_only"
root = "/tmp"
port = 1
start = ["echo", "m"]

[projects.p_extra]
name = "Extra"
group = "members_only"
root = "/tmp"
port = 2
start = ["echo", "e"]

[projects.p_other]
name = "Other"
group = "other"
root = "/tmp"
port = 3
start = ["echo", "o"]

[projects.p_inferred]
name = "Inferred"
group = "inferred"
root = "/tmp"
port = 4
start = ["echo", "i"]
"#;
        let (dir, loaded) = temp_loaded("group_membership", toml_str);

        assert!(loaded.contains_group("declared_empty"));
        assert!(loaded.contains_group("members_only"));
        assert!(loaded.contains_group("inferred"));
        assert!(!loaded.contains_group("nope"));
        assert!(!loaded.contains_group(""));
        assert!(!loaded.contains_group("   "));

        // declared members come first, then the remaining members sorted by id.
        assert_eq!(
            loaded.projects_in_group("members_only"),
            vec!["p_member".to_string(), "p_extra".to_string()]
        );
        assert_eq!(
            loaded.projects_in_group("declared_empty"),
            Vec::<String>::new()
        );
        assert_eq!(loaded.projects_in_group("nope"), Vec::<String>::new());
        // an inferred, undeclared group is sorted by id.
        assert_eq!(
            loaded.projects_in_group("inferred"),
            vec!["p_inferred".to_string()]
        );
        // group keys are trimmed.
        assert_eq!(
            loaded.projects_in_group("  other  "),
            vec!["p_other".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn group_member_listed_across_groups_appears_in_both() {
        let toml_str = r#"
[group.a]
members = ["p_cross"]

[group.b]

[projects.p_cross]
name = "Cross"
group = "b"
root = "/tmp"
port = 1
start = ["echo", "x"]
"#;
        let (dir, loaded) = temp_loaded("cross_group", toml_str);
        assert_eq!(loaded.projects_in_group("a"), vec!["p_cross".to_string()]);
        assert_eq!(loaded.projects_in_group("b"), vec!["p_cross".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn group_start_order_self_dependency_and_duplicate_deps() {
        let toml_str = r#"
[group.self_dep]
label = "Self"
order = 1
depends_on = ["self_dep"]

[group.a]
label = "A"
order = 2
depends_on = ["b", "b"]

[group.b]
label = "B"
order = 3
"#;
        let (dir, loaded) = temp_loaded("grp_self_dep", toml_str);
        // duplicate depends_on entries do not break the topological sort; a group
        // that depends on itself is left to the cycle fallback (groups() order).
        let order = loaded.group_start_order();
        assert_eq!(
            order,
            vec!["b".to_string(), "a".to_string(), "self_dep".to_string()]
        );
        // every group appears exactly once even with duplicated dependencies.
        let mut sorted = order.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), order.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sorted_projects_self_dependency_duplicates_and_group_tiebreaks() {
        let toml_str = r#"
[group.gb]
order = 1

[group.ga]
order = 2

[projects.p_self]
name = "Self"
root = "/tmp"
port = 1
start = ["echo", "x"]
depends_on = ["p_self"]

[projects.p_z]
name = "ZZZ"
group = "gb"
root = "/tmp"
port = 2
start = ["echo", "z"]

[projects.p_a]
name = "AAA"
group = "ga"
root = "/tmp"
port = 3
start = ["echo", "a"]

[projects.p_alpha_in]
name = "Alpha"
group = "ga"
root = "/tmp"
port = 4
start = ["echo", "ai"]

[projects.p_beta_in]
name = "Beta"
group = "ga"
root = "/tmp"
port = 5
start = ["echo", "bi"]
"#;
        let (dir, loaded) = temp_loaded("sorted_proj_edge", toml_str);

        // a self dependency cannot be ordered, so input order is kept.
        assert_eq!(
            loaded.sorted_projects_for_start(&["p_self"]),
            vec!["p_self".to_string()]
        );

        // duplicate ids are preserved verbatim (no de-duplication).
        assert_eq!(
            loaded.sorted_projects_for_start(&["p_a", "p_a"]),
            vec!["p_a".to_string(), "p_a".to_string()]
        );

        // group order (gb before ga) beats project name ordering.
        assert_eq!(
            loaded.sorted_projects_for_start(&["p_a", "p_z"]),
            vec!["p_z".to_string(), "p_a".to_string()]
        );

        // within one group with no members, name then id decide.
        assert_eq!(
            loaded.sorted_projects_for_start(&["p_beta_in", "p_alpha_in"]),
            vec!["p_alpha_in".to_string(), "p_beta_in".to_string()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sorted_projects_uses_inferred_group_order() {
        let toml_str = r#"
[projects.p_zeta]
name = "Zeta Project"
group = "zeta"
root = "/tmp"
port = 1
start = ["echo", "z"]

[projects.p_alpha]
name = "Alpha Project"
group = "alpha"
root = "/tmp"
port = 2
start = ["echo", "a"]

[projects.p_none]
name = "Aaa Ungrouped"
root = "/tmp"
port = 3
start = ["echo", "n"]
"#;
        let (dir, loaded) = temp_loaded("inferred_order", toml_str);
        // undeclared groups order by label/key; ungrouped projects sort last.
        assert_eq!(
            loaded.group_start_order(),
            vec!["alpha".to_string(), "zeta".to_string()]
        );
        assert_eq!(
            loaded.sorted_projects_for_start(&["p_none", "p_zeta", "p_alpha"]),
            vec![
                "p_alpha".to_string(),
                "p_zeta".to_string(),
                "p_none".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- derive_state / failure messages ----

    #[test]
    fn derive_state_uncovered_branches() {
        let base = StateInput {
            process_alive: false,
            port_open: false,
            port_conflict: false,
            has_probe: false,
            probe_ok: false,
            started_at: None,
            now: 100,
            timeout_secs: 10,
            start_error: None,
        };

        // no start marker: open port + passing probe -> Ready.
        assert_eq!(
            derive_state(StateInput {
                port_open: true,
                has_probe: true,
                probe_ok: true,
                ..base.clone()
            }),
            (LifeState::Ready, None)
        );

        // no start marker: closed port with a dead process -> Stopped, probe ignored.
        assert_eq!(
            derive_state(StateInput {
                has_probe: true,
                probe_ok: true,
                ..base.clone()
            }),
            (LifeState::Stopped, None)
        );

        // no start marker: closed port with a live process -> Starting.
        assert_eq!(
            derive_state(StateInput {
                process_alive: true,
                has_probe: true,
                probe_ok: true,
                ..base.clone()
            }),
            (LifeState::Starting, None)
        );

        // no start marker + port conflict + live process -> Starting, never Ready.
        assert_eq!(
            derive_state(StateInput {
                process_alive: true,
                port_open: true,
                port_conflict: true,
                ..base.clone()
            }),
            (LifeState::Starting, None)
        );

        // with a start marker, elapsed exactly at the timeout is already a failure.
        assert_eq!(
            derive_state(StateInput {
                process_alive: true,
                started_at: Some(100),
                now: 110,
                ..base.clone()
            }),
            (LifeState::Failed, Some("超过 10 秒仍未就绪".to_string()))
        );

        // "process exited" is reported ahead of the timeout wording.
        assert_eq!(
            derive_state(StateInput {
                started_at: Some(100),
                now: 100 + START_EXIT_GRACE_SECS + 500,
                timeout_secs: 10,
                ..base.clone()
            }),
            (LifeState::Failed, Some("进程已退出".to_string()))
        );

        // port conflict + dead process past the grace window -> 进程已退出.
        assert_eq!(
            derive_state(StateInput {
                port_open: true,
                port_conflict: true,
                started_at: Some(100),
                now: 110,
                ..base.clone()
            }),
            (LifeState::Failed, Some("进程已退出".to_string()))
        );

        // closed port + live process, probe irrelevant, still within timeout -> Starting.
        assert_eq!(
            derive_state(StateInput {
                process_alive: true,
                has_probe: true,
                probe_ok: true,
                started_at: Some(100),
                now: 105,
                ..base.clone()
            }),
            (LifeState::Starting, None)
        );
    }

    #[test]
    fn extract_start_failure_line_uncovered_patterns() {
        // port-in-use synonyms.
        for line in [
            "java.net.BindException: Address already in use\n",
            "Failed to bind to port 8080\n",
            "EADDRINUSE: address already in use :::3000\n",
            "地址已在使用\n",
        ] {
            let got = extract_start_failure_line(line).unwrap();
            assert!(got.starts_with("端口被占用"), "line `{line}` -> {got}");
        }

        // dependency-resolution synonyms.
        for line in [
            "Could not find artifact com.example:demo:jar:1.0\n",
            "module not found: Error: Can't resolve './missing'\n",
            "non-resolvable parent POM for demo\n",
            "无法解析依赖：foo\n",
            "failed to collect dependencies\n",
        ] {
            let got = extract_start_failure_line(line).unwrap();
            assert!(got.starts_with("依赖解析失败"), "line `{line}` -> {got}");
        }

        // a capital-E "Exception" is required; lowercase alone is not a failure.
        assert_eq!(
            extract_start_failure_line("java.lang.runtimeexception: boom\n"),
            None
        );
        assert_eq!(
            extract_start_failure_line("exception in thread main\n"),
            None
        );
        // lowercase "caused by:" is still recognised.
        assert_eq!(
            extract_start_failure_line("caused by: java.io.IOException\n").as_deref(),
            Some("异常：caused by: java.io.IOException")
        );

        // Maven auth: a bare `status:` keyword plus 401/403 is enough.
        assert_eq!(
            extract_start_failure_line("status: 401\n").as_deref(),
            Some("认证失败：Maven 仓库返回 401")
        );
        assert_eq!(
            extract_start_failure_line("status: 403\n").as_deref(),
            Some("认证失败：Maven 仓库返回 403")
        );
        // digit boundaries are respected: 4010 / x1401 are not a 401.
        assert_eq!(extract_start_failure_line("status: 4010\n"), None);
        assert_eq!(
            extract_start_failure_line("code x1401 unauthorized\n"),
            None
        );

        // auth is matched before dependency lines.
        let both = extract_start_failure_line(
            "authentication failed status: 401\nCould not resolve dependencies for demo\n",
        )
        .unwrap();
        assert!(both.starts_with("认证失败"), "{both}");
    }

    #[test]
    fn failure_message_helper_uncovered_cases() {
        // generic reasons are replaceable by a log line.
        assert!(failure_allows_log_line("进程已退出但仍有子进程"));
        // non-generic or warned reasons are not.
        assert!(!failure_allows_log_line("超过"));
        assert!(!failure_allows_log_line("超 过 10 秒"));
        assert!(!failure_allows_log_line("process exited"));
        assert!(!failure_allows_log_line("Disabled"));
        assert!(!failure_allows_log_line("启动失败: boom"));

        // prefix_failure: an already-labelled line is returned as-is.
        assert_eq!(prefix_failure("异常", "异常处理失败"), "异常处理失败");

        // exactly 180 chars is not truncated.
        let exactly180 = "x".repeat(180);
        let out180 = prefix_failure("异常", &exactly180);
        assert_eq!(out180, format!("异常：{exactly180}"));
        assert!(!out180.ends_with('…'));

        // 181 chars is truncated to 180 plus an ellipsis.
        let out181 = prefix_failure("异常", &"x".repeat(181));
        assert!(out181.ends_with('…'));
        assert_eq!(out181.chars().count(), "异常：".chars().count() + 181);
    }
}
