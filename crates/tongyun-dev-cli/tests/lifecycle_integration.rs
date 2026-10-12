//! End-to-end checks through the shared core and the web console.
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use std::sync::atomic::{AtomicUsize, Ordering};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(tag: &str) -> PathBuf {
    let count = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir =
        std::env::temp_dir().join(format!("tongyun_it_{tag}_{}_{}", std::process::id(), count));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("logs")).unwrap();
    dir
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn write_config(dir: &Path, port: u16) -> PathBuf {
    let d = dir.to_string_lossy().replace('\\', "/");
    let cfg = dir.join("projects.toml");
    fs::write(
        &cfg,
        format!(
            r#"# test config
[projects.mock]
name = "Mock Service"
root = "{d}"
start = ["echo", "start"]
log_dir = "{d}/logs"
port = {port}

[projects.parked]
name = "Parked"
root = "{d}"
enabled = false
reason = "not checked out"
"#
        ),
    )
    .unwrap();
    cfg
}

#[test]
fn status_follows_pid_file_port_and_log() {
    let dir = temp_dir("lifecycle");
    let port = free_port();
    let cfg = write_config(&dir, port);
    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();

    let st = loaded.status("mock").unwrap();
    assert!(!st.running && !st.port_open && st.pid.is_none());
    assert!(!st.port_conflict);

    // Simulate the service: our own pid in the pid file, listening on the port.
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    fs::write(dir.join("logs/mock.pid"), std::process::id().to_string()).unwrap();
    fs::write(dir.join("logs/mock.out"), "Service started\nListening\n").unwrap();
    let st = loaded.status("mock").unwrap();
    assert!(st.running);
    assert_eq!(st.pid, Some(std::process::id()));
    assert!(st.port_open);

    let tail = loaded.log_tail("mock", 10).unwrap();
    assert!(!tail.missing);
    assert!(tail.text.contains("Service started"));

    drop(listener);
    fs::remove_file(dir.join("logs/mock.pid")).unwrap();
    thread::sleep(Duration::from_millis(100));
    let st = loaded.status("mock").unwrap();
    assert!(!st.running && !st.port_open);

    // Disabled projects refuse to start.
    let err = loaded.start("parked").unwrap_err();
    assert!(format!("{err:#}").contains("not checked out"));
    let _ = fs::remove_dir_all(&dir);
}

fn http(port: u16, req: &str) -> String {
    let mut stream = None;
    for _ in 0..30 {
        if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
            stream = Some(s);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let mut s = stream.expect("UI server did not accept connections");
    s.write_all(req.as_bytes()).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

fn post_json(port: u16, path: &str, body: &str, origin: Option<&str>) -> String {
    let origin = origin
        .map(|o| format!("Origin: {o}\r\n"))
        .unwrap_or_default();
    http(
        port,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{origin}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

#[test]
fn web_console_lists_adds_and_removes_through_core() {
    let dir = temp_dir("ui");
    let cfg = write_config(&dir, free_port());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let ui = listener.local_addr().unwrap().port();
    let cfg_clone = cfg.clone();
    thread::spawn(move || {
        let _ = tongyun_dev_cli::server::serve(listener, cfg_clone, ui);
    });

    let get = |path: &str| {
        http(
            ui,
            &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{ui}\r\nConnection: close\r\n\r\n"),
        )
    };

    let page = get("/");
    assert!(page.starts_with("HTTP/1.1 200"));
    assert!(page.contains("tongyun-dev"));

    let list = get("/api/projects");
    assert!(list.starts_with("HTTP/1.1 200"));
    assert!(list.contains("\"id\":\"mock\""));
    assert!(list.contains("\"enabled\":false"));
    assert!(list.contains("not checked out"));
    assert!(list.contains("\"port_conflict\""));

    // Add with toolchain fields, from our own origin.
    let d_str = dir.to_string_lossy().replace('\\', "/");
    let add = format!(
        r#"{{"id":"web_app","name":"Web App","root":"{d_str}","start":"pnpm dev","port":5173,"node":"22","java":"17"}}"#
    );
    let resp = post_json(
        ui,
        "/api/projects",
        &add,
        Some(&format!("http://127.0.0.1:{ui}")),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("# test config"), "comments kept");
    assert!(text.contains("[projects.web_app]"));
    assert!(text.contains("node = \"22\""));
    assert!(text.contains("java = \"17\""));
    assert!(get("/api/projects").contains("\"id\":\"web_app\""));

    // Read and edit through the same core used by the desktop.
    let detail = get("/api/projects/web_app");
    assert!(detail.contains("\"start_editable\":true"), "{detail}");
    let patch = |body: &str, origin: &str| {
        http(ui, &format!(
        "PATCH /api/projects/web_app HTTP/1.1\r\nHost: 127.0.0.1:{ui}\r\nOrigin: {origin}\r\nContent-Length: {}\r\n\r\n{body}", body.len()))
    };
    let edit =
        r#"{"name":"Renamed","port":5180,"node":"","health_url":"http://127.0.0.1:5180/health"}"#;
    assert!(patch(edit, "http://evil.example").starts_with("HTTP/1.1 403"));
    assert_eq!(fs::read_to_string(&cfg).unwrap(), text);
    let resp = patch(edit, &format!("http://127.0.0.1:{ui}"));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("# test config"));
    let detail = get("/api/projects/web_app");
    assert!(detail.contains("Renamed") && detail.contains("5180"));
    assert!(detail.contains("\"node\":null"));

    // POST /api/groups creates a new group
    let add_group = r#"{"key":"backend","label":"后端服务"}"#;
    let evil_group_resp = post_json(ui, "/api/groups", add_group, Some("http://evil.example"));
    assert!(
        evil_group_resp.starts_with("HTTP/1.1 403"),
        "{evil_group_resp}"
    );
    let resp = post_json(ui, "/api/groups", add_group, None);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("\"key\":\"backend\""));

    let groups_resp = get("/api/groups");
    assert!(groups_resp.contains("\"key\":\"backend\"") && groups_resp.contains("后端服务"));

    // PATCH group updates the group and is reflected in the project list.
    let edit_group = r#"{"group":"frontend"}"#;
    let resp = patch(edit_group, &format!("http://127.0.0.1:{ui}"));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let list_after_group = get("/api/projects");
    assert!(
        list_after_group.contains("\"group\":\"frontend\""),
        "group visible in project list"
    );
    let detail_after_group = get("/api/projects/web_app");
    assert!(
        detail_after_group.contains("\"group\":\"frontend\""),
        "group visible in detail"
    );

    // Clear group by setting empty string
    let clear_group = r#"{"group":""}"#;
    let resp = patch(clear_group, &format!("http://127.0.0.1:{ui}"));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let list_after_clear = get("/api/projects");
    assert!(
        list_after_clear.contains("\"group\":null"),
        "cleared group is null in list"
    );

    let text = fs::read_to_string(&cfg).unwrap();
    let bad = patch(r#"{"port":0}"#, &format!("http://127.0.0.1:{ui}"));
    assert!(bad.starts_with("HTTP/1.1 400"), "{bad}");
    assert_eq!(fs::read_to_string(&cfg).unwrap(), text);

    // Duplicate id is refused and the file is unchanged.
    let resp = post_json(ui, "/api/projects", &add, None);
    assert!(resp.starts_with("HTTP/1.1 400"), "{resp}");
    assert_eq!(fs::read_to_string(&cfg).unwrap(), text);

    // Cross-site requests and foreign Host headers are refused.
    let resp = post_json(
        ui,
        "/api/projects/mock/start",
        "",
        Some("http://evil.example"),
    );
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
    let resp = http(
        ui,
        "GET /api/projects HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n",
    );
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");

    // Disabled project start goes through core and reports the reason.
    let resp = post_json(ui, "/api/projects/parked/start", "", None);
    assert!(
        resp.contains("\"ok\":false") && resp.contains("not checked out"),
        "{resp}"
    );

    let resp = http(
        ui,
        &format!("DELETE /api/projects/web_app HTTP/1.1\r\nHost: 127.0.0.1:{ui}\r\nConnection: close\r\n\r\n"),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(!fs::read_to_string(&cfg).unwrap().contains("web_app"));

    let logs = get("/api/projects/mock/logs");
    assert!(logs.contains("\"missing\":true"), "{logs}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn start_ui_server_missing_config_fails_gracefully() {
    let missing_path = PathBuf::from("/definitely/nonexistent/tongyun_dev_missing_projects.toml");
    let res = tongyun_dev_cli::server::start_ui_server(missing_path, free_port(), false);
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.contains("未找到配置文件 projects.toml") || err.contains("not found"));
}

struct CleanupGuard<'a> {
    loaded: Option<&'a tongyun_dev_core::Loaded>,
    id: &'static str,
    dir: PathBuf,
}

impl<'a> CleanupGuard<'a> {
    fn new(loaded: &'a tongyun_dev_core::Loaded, id: &'static str, dir: PathBuf) -> Self {
        Self {
            loaded: Some(loaded),
            id,
            dir,
        }
    }
}

impl<'a> Drop for CleanupGuard<'a> {
    fn drop(&mut self) {
        if let Some(loaded) = self.loaded {
            let _ = loaded.stop(self.id);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        if pid <= 1 {
            return false;
        }
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.contains(&pid.to_string())
            })
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

#[cfg(unix)]
fn pgroup_pids(pgid: u32) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output();
    let Ok(out) = output else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .collect()
}

#[test]
#[cfg(unix)]
fn process_group_stop_cleans_children_and_markers() {
    let dir = temp_dir("pg_stop");
    let port = free_port();
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    fs::write(
        &cfg,
        format!(
            r#"[projects.worker]
name = "Worker"
root = "{d}"
start = ["sh", "-c", "sleep 300 & sleep 300 & wait"]
background = true
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "worker", dir.clone());

    let st = loaded.start_detached("worker").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);
    let parent_pid = st.pid.expect("parent pid must exist");
    assert!(is_process_alive(parent_pid));

    thread::sleep(Duration::from_millis(300));
    let group_pids = pgroup_pids(parent_pid);
    assert!(
        group_pids.len() >= 3,
        "expected parent sh and two sleep children, got {:?}",
        group_pids
    );

    let pid_file = dir.join("logs/worker.pid");
    let start_marker = dir.join("logs/worker.pid.start");
    assert!(pid_file.is_file(), "pid file must exist");
    assert!(start_marker.is_file(), "start marker must exist");

    let stop_st = loaded.stop("worker").unwrap();
    assert_eq!(stop_st.state, tongyun_dev_core::LifeState::Stopped);

    assert!(
        !is_process_alive(parent_pid),
        "parent pid {parent_pid} must be gone"
    );
    for child in &group_pids {
        assert!(
            !is_process_alive(*child),
            "group process {child} must be gone after stop"
        );
    }
    let remaining = pgroup_pids(parent_pid);
    assert!(
        remaining.is_empty(),
        "no processes should remain in group {parent_pid}, got {:?}",
        remaining
    );

    assert!(!pid_file.is_file(), "pid file must be deleted");
    assert!(!start_marker.is_file(), "start marker must be deleted");
}

#[test]
#[cfg(unix)]
fn trap_term_killed_after_grace_period() {
    let dir = temp_dir("trap_term");
    let port = free_port();
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    fs::write(
        &cfg,
        format!(
            r#"[projects.trapped]
name = "Trapped"
root = "{d}"
start = ["sh", "-c", "trap '' TERM; sleep 300"]
background = true
stop_grace_secs = 1
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "trapped", dir.clone());

    let st = loaded.start_detached("trapped").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);
    let parent_pid = st.pid.expect("parent pid must exist");

    thread::sleep(Duration::from_millis(300));
    let group_pids = pgroup_pids(parent_pid);

    let stop_st = loaded.stop("trapped").unwrap();
    assert_eq!(stop_st.state, tongyun_dev_core::LifeState::Stopped);

    assert!(
        !is_process_alive(parent_pid),
        "parent pid {parent_pid} must be killed"
    );
    for child in &group_pids {
        assert!(
            !is_process_alive(*child),
            "child process {child} must be killed after grace period"
        );
    }
    let remaining = pgroup_pids(parent_pid);
    assert!(remaining.is_empty(), "group must have no alive processes");
}

#[test]
fn stale_pid_file_reported_stopped_and_cleaned() {
    let dir = temp_dir("stale_pid");
    let port = free_port();
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    let (shell_cmd, shell_arg) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    fs::write(
        &cfg,
        format!(
            r#"[projects.mock]
name = "Mock"
root = "{d}"
start = ["{shell_cmd}", "{shell_arg}", "echo start"]
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "mock", dir.clone());

    let pid_file = dir.join("logs/mock.pid");
    fs::write(&pid_file, "999999\n").unwrap();
    assert!(pid_file.is_file());

    let st = loaded.status("mock").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Stopped);
    assert!(!st.running);

    let stop_res = loaded.stop("mock");
    assert!(stop_res.is_ok(), "stop should not fail on stale pid");
    assert!(!pid_file.is_file(), "stale pid file must be removed");
}

#[test]
fn fast_exit_transitions_to_failed_within_four_seconds() {
    let dir = temp_dir("fast_exit");
    let port = free_port();
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    let (shell_cmd, shell_arg) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    fs::write(
        &cfg,
        format!(
            r#"[projects.exiter]
name = "Exiter"
root = "{d}"
start = ["{shell_cmd}", "{shell_arg}", "exit 3"]
background = true
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "exiter", dir.clone());

    let st = loaded.start_detached("exiter").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);

    let start_t = std::time::Instant::now();
    let mut final_status = None;
    while start_t.elapsed() < Duration::from_secs(4) {
        let current = loaded.status("exiter").unwrap();
        if current.state == tongyun_dev_core::LifeState::Failed {
            final_status = Some(current);
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let failed_st = final_status.expect("must transition to Failed within 4s");
    assert_eq!(failed_st.state, tongyun_dev_core::LifeState::Failed);
    let failure_msg = failed_st.failure.unwrap_or_default();
    assert!(
        failure_msg.contains("进程已退出"),
        "expected failure to contain '进程已退出', got: {failure_msg}"
    );
}

#[test]
fn delayed_listen_transitions_from_starting_to_ready() {
    let python3_ok = std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !python3_ok {
        eprintln!("跳过测试 delayed_listen_transitions_from_starting_to_ready: python3 不可用");
        return;
    }

    let dir = temp_dir("py_listen");
    let port = free_port();
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    let (shell_cmd, shell_arg, script) = if cfg!(windows) {
        (
            "cmd",
            "/C",
            format!("python3 -m http.server {port} --bind 127.0.0.1"),
        )
    } else {
        (
            "sh",
            "-c",
            format!("sleep 2; exec python3 -m http.server {port} --bind 127.0.0.1"),
        )
    };
    fs::write(
        &cfg,
        format!(
            r#"[projects.py_srv]
name = "Python Server"
root = "{d}"
start = ["{shell_cmd}", "{shell_arg}", "{script}"]
background = true
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "py_srv", dir.clone());

    let st = loaded.start_detached("py_srv").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);

    let start_t = std::time::Instant::now();
    let mut ready_st = None;
    while start_t.elapsed() < Duration::from_secs(10) {
        let current = loaded.status("py_srv").unwrap();
        if current.state == tongyun_dev_core::LifeState::Ready {
            ready_st = Some(current);
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }

    assert!(ready_st.is_some(), "must transition to Ready within 10s");
    let stop_st = loaded.stop("py_srv").unwrap();
    assert_eq!(stop_st.state, tongyun_dev_core::LifeState::Stopped);
    assert!(!stop_st.port_open);
}

#[test]
fn repeated_start_keeps_pid_and_restart_replaces_it() {
    let dir = temp_dir("restart");
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    let (shell_cmd, shell_arg, script) = if cfg!(windows) {
        ("cmd", "/C", "ping -n 300 127.0.0.1")
    } else {
        ("sh", "-c", "sleep 300 & wait")
    };
    fs::write(
        &cfg,
        format!(
            r#"[projects.worker]
root = "{d}"
start = ["{shell_cmd}", "{shell_arg}", "{script}"]
background = true
port = {}
log_dir = "{d}/logs"
"#,
            free_port()
        ),
    )
    .unwrap();
    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "worker", dir.clone());
    let first = loaded.start_detached("worker").unwrap().pid.unwrap();
    // Dependency metadata can appear while a process is starting. A repeated
    // start must not launch an installer or replace that process's marker.
    fs::write(dir.join("package.json"), "{}").unwrap();
    let second = loaded.start_detached("worker").unwrap().pid.unwrap();
    assert!(!dir.join("logs/worker.pid.install").exists());
    fs::remove_file(dir.join("package.json")).unwrap();
    assert_eq!(first, second, "starting process must not be replaced");
    let next = loaded.restart_detached("worker").unwrap();
    assert_eq!(next.state, tongyun_dev_core::LifeState::Starting);
    assert_ne!(next.pid.unwrap(), first);
    assert!(!is_process_alive(first));
    loaded.stop("worker").unwrap();
    assert_eq!(
        loaded.status("worker").unwrap().state,
        tongyun_dev_core::LifeState::Stopped
    );
}

#[test]
fn foreign_listener_blocks_restart_without_running_start_command() {
    let dir = temp_dir("blocked_restart");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = write_config(&dir, listener.local_addr().unwrap().port());
    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    assert!(
        loaded.stop("mock").is_err(),
        "occupied port is not a successful stop"
    );
    assert!(loaded.restart_detached("mock").is_err());
    assert!(
        !dir.join("logs/mock.pid.start").exists(),
        "must not start after failed stop"
    );
    assert!(
        listener.local_addr().is_ok(),
        "foreign listener must survive"
    );
    drop(listener);
    fs::remove_dir_all(dir).unwrap();
}
