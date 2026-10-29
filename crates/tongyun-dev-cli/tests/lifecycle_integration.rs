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

fn free_service_port(exclude: &[u16]) -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT_PORT: AtomicU16 = AtomicU16::new(24100);
    for _ in 0..8000 {
        let p = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        let port = if (24000..32000).contains(&p) {
            p
        } else {
            NEXT_PORT.store(24101, Ordering::Relaxed);
            24100
        };
        if exclude.contains(&port) {
            continue;
        }
        if let Ok(l) = TcpListener::bind(("127.0.0.1", port)) {
            drop(l);
            return port;
        }
    }
    panic!("could not find an available port in 24000..32000");
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
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let unlistened_port = free_service_port(&[port]);
    let cfg = write_config(&dir, unlistened_port);
    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();

    let st = loaded.status("mock").unwrap();
    assert!(!st.running && !st.port_open && st.pid.is_none());
    assert!(!st.port_conflict);

    // Simulate the service: our own pid in the pid file, listening on the port.
    let cfg = write_config(&dir, port);
    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
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
    let cfg = write_config(&dir, free_service_port(&[]));
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

    // PATCH group using Chinese label: auto creates group with legal key and label="前端"
    let edit_chinese = r#"{"group":"前端"}"#;
    let resp = patch(edit_chinese, &format!("http://127.0.0.1:{ui}"));
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let detail_after_chinese = get("/api/projects/web_app");
    let json_body = detail_after_chinese.split("\r\n\r\n").nth(1).unwrap();
    let parsed_detail: serde_json::Value = serde_json::from_str(json_body).unwrap();
    let assigned_group = parsed_detail["project"]["group"].as_str().unwrap();
    assert!(
        tongyun_dev_core::validate_group_key(assigned_group).is_ok(),
        "assigned group `{assigned_group}` must be a valid key"
    );
    assert_eq!(assigned_group, "g1");
    let groups_resp_chinese = get("/api/groups");
    let groups_body = groups_resp_chinese.split("\r\n\r\n").nth(1).unwrap();
    let parsed_groups: serde_json::Value = serde_json::from_str(groups_body).unwrap();
    let has_chinese_label =
        parsed_groups["groups"].as_array().unwrap().iter().any(|g| {
            g["key"].as_str() == Some(assigned_group) && g["label"].as_str() == Some("前端")
        });
    assert!(
        has_chinese_label,
        "new group must have label='前端' in /api/groups: {groups_resp_chinese}"
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
    let res = tongyun_dev_cli::server::start_ui_server(missing_path, 0, false);
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

#[cfg(target_os = "linux")]
fn controlling_tty_nr(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    let mut fields = rest.split_whitespace();
    fields.next()?;
    fields.next()?;
    fields.next()?;
    fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(unix)]
fn process_pgid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(unix)]
fn process_argv0(pid: u32) -> String {
    let proc_path = format!("/proc/{pid}/cmdline");
    if let Ok(raw) = fs::read(proc_path) {
        let text = String::from_utf8_lossy(&raw);
        return text.split('\0').next().unwrap_or("").to_string();
    }
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output();
    let Ok(out) = output else {
        return String::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

#[cfg(unix)]
fn argv0_base(argv0: &str) -> &str {
    std::path::Path::new(argv0)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(argv0)
}

#[cfg(unix)]
fn descendant_pids(root: u32) -> Vec<u32> {
    let mut result = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        let Ok(out) = std::process::Command::new("pgrep")
            .args(["-P", &parent.to_string()])
            .output()
        else {
            continue;
        };
        if !out.status.success() {
            continue;
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Ok(cpid) = line.trim().parse::<u32>() {
                if cpid > 1 && !result.contains(&cpid) {
                    result.push(cpid);
                    queue.push(cpid);
                }
            }
        }
    }
    result
}

#[cfg(unix)]
fn assert_tree_has_no_script(root: u32) {
    let mut argv0s = vec![process_argv0(root)];
    for pid in descendant_pids(root) {
        argv0s.push(process_argv0(pid));
    }
    for argv0 in &argv0s {
        assert_ne!(
            argv0_base(argv0),
            "script",
            "pty path must not exec system script, argv0s={argv0s:?}"
        );
    }
    assert!(
        argv0s.iter().any(|argv0| argv0_base(argv0) == "sh"),
        "real start command should be running, argv0s={argv0s:?}"
    );
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
    let port = free_service_port(&[]);
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
    let port = free_service_port(&[]);
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
    let port = free_service_port(&[]);
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
    let port = free_service_port(&[]);
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

fn probe_python_cmd() -> Option<&'static str> {
    let candidates: &[&str] = if cfg!(windows) {
        &["py", "python", "python3"]
    } else {
        &["python3", "python"]
    };
    for &cmd in candidates {
        if std::process::Command::new(cmd)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(cmd);
        }
    }
    None
}

#[test]
fn delayed_listen_transitions_from_starting_to_ready() {
    let py = probe_python_cmd().expect(
        "delayed_listen_transitions_from_starting_to_ready requires python or python3 on PATH, but neither was found",
    );

    let dir = temp_dir("py_listen");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");
    let listen_py =
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/listen.py").replace('\\', "/");
    fs::write(
        &cfg,
        format!(
            r#"[projects.py_srv]
name = "Python Server"
root = "{d}"
start = ["{py}", "{listen_py}", "{port}", "--delay-secs", "2"]
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
            free_service_port(&[])
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

#[cfg(unix)]
#[test]
fn pty_stdin_fifo_and_env_lifecycle() {
    let dir = temp_dir("stdin_fifo");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script = "read line; echo got_input=$line; echo CI_VAL=$CI; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.interactive]
name = "Interactive Service"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
port = {port}
log_dir = "{d}/logs"
env = {{ CI = "test_ci_value", CUSTOM = "123" }}
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "interactive", dir.clone());

    let st = loaded.start_detached("interactive").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);

    // 1. Check that FIFO was created at logs/interactive.stdin
    let fifo_path = dir.join("logs/interactive.stdin");
    assert!(
        fifo_path.exists(),
        "FIFO {} should exist",
        fifo_path.display()
    );

    // 2. Send input via send_input
    loaded.send_input("interactive", "hello_dev\n").unwrap();

    // 3. Wait for log to contain got_input=hello_dev, CI_VAL=test_ci_value, and echo › hello_dev
    let log_path = dir.join("logs/interactive.out");
    let start_t = std::time::Instant::now();
    let mut log_content = String::new();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("got_input=hello_dev")
                && log_content.contains("CI_VAL=test_ci_value")
            {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    assert!(
        log_content.contains("got_input=hello_dev"),
        "log: {log_content}"
    );
    assert!(
        log_content.contains("CI_VAL=test_ci_value"),
        "log: {log_content}"
    );
    assert!(
        log_content.contains("› [已发送 10 个字符]"),
        "log: {log_content}"
    );
    assert!(!log_content.contains("› hello_dev"), "log: {log_content}");

    // 4. Stop project and verify cleanup
    let stopped = loaded.stop("interactive").unwrap();
    assert_eq!(stopped.state, tongyun_dev_core::LifeState::Stopped);
    assert!(!fifo_path.exists(), "FIFO must be removed after stop");
    assert!(
        !dir.join("logs/interactive.pid").exists(),
        "pid file must be removed"
    );
}

#[cfg(unix)]
#[test]
fn http_post_input_endpoint_and_security() {
    let dir = temp_dir("http_input");
    let ui_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let ui_port = ui_listener.local_addr().unwrap().port();
    let service_port = free_service_port(&[ui_port]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script = "read line; echo http_got=$line; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.http_test]
name = "HTTP Test"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
port = {service_port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();
    let cfg_clone = cfg.clone();
    thread::spawn(move || {
        let _ = tongyun_dev_cli::server::serve(ui_listener, cfg_clone, ui_port);
    });

    // 1. Before project is running: POST /api/projects/http_test/input should return 409
    let post_req = |body: &str, origin: &str| -> String {
        format!(
            "POST /api/projects/http_test/input HTTP/1.1\r\n\
             Host: 127.0.0.1:{ui_port}\r\n\
             Origin: {origin}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             {}",
            body.len(),
            body
        )
    };

    let resp = http(
        ui_port,
        &post_req(r#"{"text":"ping"}"#, &format!("http://127.0.0.1:{ui_port}")),
    );
    assert!(
        resp.starts_with("HTTP/1.1 409"),
        "expected 409 when not running, got: {resp}"
    );

    // 2. Cross-origin request: rejected with 403
    let evil_resp = http(
        ui_port,
        &post_req(r#"{"text":"ping"}"#, "http://evil.example"),
    );
    assert!(
        evil_resp.starts_with("HTTP/1.1 403"),
        "expected 403 on evil origin, got: {evil_resp}"
    );

    // 3. Start project
    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "http_test", dir.clone());
    loaded.start_detached("http_test").unwrap();

    // Wait until process is alive (pid recorded) and stdin FIFO is ready
    let fifo_path = dir.join("logs/http_test.stdin");
    let start_t = std::time::Instant::now();
    while start_t.elapsed() < Duration::from_secs(5) {
        if fifo_path.exists() {
            if let Ok(st) = loaded.status("http_test") {
                if st.pid.is_some() {
                    break;
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }

    // 4. While running, POST input succeeds (retry briefly if server is still registering process status)
    let mut ok_resp = String::new();
    let start_t = std::time::Instant::now();
    while start_t.elapsed() < Duration::from_secs(5) {
        ok_resp = http(
            ui_port,
            &post_req(
                r#"{"text":"pong","enter":true}"#,
                &format!("http://127.0.0.1:{ui_port}"),
            ),
        );
        if ok_resp.starts_with("HTTP/1.1 200") {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        ok_resp.starts_with("HTTP/1.1 200"),
        "expected 200, got: {ok_resp}"
    );

    // Check log received input
    let log_path = dir.join("logs/http_test.out");
    let start_t = std::time::Instant::now();
    let mut log_content = String::new();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("http_got=pong") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(log_content.contains("http_got=pong"), "log: {log_content}");
    assert!(
        log_content.contains("› [已发送 5 个字符]"),
        "log: {log_content}"
    );
    assert!(!log_content.contains("› pong"), "log: {log_content}");

    loaded.stop("http_test").unwrap();
}

#[cfg(unix)]
#[test]
fn auto_answers_automatically_answers_prompt() {
    let dir = temp_dir("auto_answers_test");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let prompt_sh = dir.join("prompt.sh");
    fs::write(
        &prompt_sh,
        "#!/bin/bash\nread -p \"Select environment [dev/prod]: \" e\necho \"env=$e\"\nexec sleep 30\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&prompt_sh).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&prompt_sh, perms).unwrap();

    let p_path = prompt_sh.to_string_lossy().replace('\\', "/");
    fs::write(
        &cfg,
        format!(
            r#"[projects.prompt_app]
name = "Prompt App"
root = "{d}"
start = ["{p_path}"]
background = true
pty = true
port = {port}
log_dir = "{d}/logs"
auto_answers = [
    {{ expect = "Select environment", send = "dev" }}
]
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "prompt_app", dir.clone());

    let st = loaded.start_detached("prompt_app").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);

    let log_path = dir.join("logs/prompt_app.out");
    let start_t = std::time::Instant::now();
    let mut log_content = String::new();
    while start_t.elapsed() < Duration::from_secs(6) {
        let _ = loaded.status("prompt_app");
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("env=dev") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }

    assert!(log_content.contains("env=dev"), "log: {log_content}");

    let stopped = loaded.stop("prompt_app").unwrap();
    assert_eq!(stopped.state, tongyun_dev_core::LifeState::Stopped);
    assert!(
        !dir.join("logs/prompt_app.stdin").exists(),
        "FIFO must be cleaned up"
    );
    assert!(
        !dir.join("logs/prompt_app.answers.json").exists(),
        "answers state file must be cleaned up"
    );
}

#[cfg(unix)]
#[test]
fn pty_true_outputs_tty_and_cleans_up_on_stop() {
    let dir = temp_dir("pty_tty_test");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script =
        "[ -t 0 ] && echo TTY_DETECTED; echo CI_VAL=$CI; read line; echo got_input=$line; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.pty_srv]
name = "PTY Service"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
pty = true
port = {port}
log_dir = "{d}/logs"
env = {{ CI = "pty_ci" }}
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "pty_srv", dir.clone());

    let st = loaded.start_detached("pty_srv").unwrap();
    assert_eq!(st.state, tongyun_dev_core::LifeState::Starting);
    let pid = st.pid.expect("pid must exist");
    assert!(is_process_alive(pid), "pty relay must stay alive");
    assert_eq!(
        process_pgid(pid),
        Some(pid),
        "pid file must record the relay process group leader"
    );

    let log_path = dir.join("logs/pty_srv.out");
    let fifo_path = dir.join("logs/pty_srv.stdin");
    let answers_path = dir.join("logs/pty_srv.answers.json");
    assert!(fifo_path.exists(), "FIFO should exist for pty project");

    let start_t = std::time::Instant::now();
    let mut log_content = String::new();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("TTY_DETECTED") && log_content.contains("CI_VAL=pty_ci") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    assert!(log_content.contains("TTY_DETECTED"), "log: {log_content}");
    assert!(log_content.contains("CI_VAL=pty_ci"), "log: {log_content}");
    assert!(!log_content.contains("tcgetattr"), "log: {log_content}");
    assert!(
        !log_content.contains("Operation not supported"),
        "log: {log_content}"
    );
    assert!(!log_content.contains("setsid failed"), "log: {log_content}");
    assert!(
        !log_content.contains("set controlling tty failed"),
        "log: {log_content}"
    );
    #[cfg(target_os = "linux")]
    {
        let child = descendant_pids(pid)
            .into_iter()
            .next()
            .expect("start command should already be running");
        let tty_nr = controlling_tty_nr(child).unwrap_or(0);
        assert!(
            tty_nr > 0,
            "child {child} must have a controlling tty, stat tty_nr={tty_nr}"
        );
    }
    assert!(
        is_process_alive(pid),
        "pty relay exited early: {log_content}"
    );
    assert_tree_has_no_script(pid);

    loaded.send_input("pty_srv", "hello_pty\n").unwrap();
    let start_t = std::time::Instant::now();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("got_input=hello_pty") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        log_content.contains("got_input=hello_pty"),
        "log: {log_content}"
    );
    assert!(
        !log_content.contains("› [已发送"),
        "pty mode must not append a plaintext placeholder from our layer: {log_content}"
    );
    assert!(is_process_alive(pid), "pty relay died after input");

    let descendants = descendant_pids(pid);
    assert!(
        !descendants.is_empty(),
        "start command should be a child of the relay"
    );
    let group_before = pgroup_pids(pid);
    assert!(
        group_before.contains(&pid),
        "relay group should contain itself, got {group_before:?}"
    );

    let stopped = loaded.stop("pty_srv").unwrap();
    assert_eq!(stopped.state, tongyun_dev_core::LifeState::Stopped);
    assert!(!is_process_alive(pid), "relay must be terminated");
    for child in descendants {
        assert!(
            !is_process_alive(child),
            "descendant {child} must be terminated"
        );
    }
    let remaining = pgroup_pids(pid);
    assert!(
        remaining.is_empty(),
        "process group must be cleared, got {remaining:?}"
    );
    assert!(!fifo_path.exists(), "FIFO must be removed after stop");
    assert!(
        !answers_path.exists(),
        "answers state must be removed after stop"
    );
    assert!(
        !dir.join("logs/pty_srv.pid").exists(),
        "pid file must be removed"
    );
}

#[cfg(unix)]
#[test]
fn pty_password_prompt_does_not_echo_secret_into_log() {
    let dir = temp_dir("pty_secret");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script = "printf 'Password:'; read -r secret; echo received_len=${#secret}; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.pty_secret]
name = "PTY Secret"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
pty = true
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "pty_secret", dir.clone());
    loaded.start_detached("pty_secret").unwrap();

    let log_path = dir.join("logs/pty_secret.out");
    let start_t = std::time::Instant::now();
    let mut log_content = String::new();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("Password:") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        log_content.contains("Password:"),
        "prompt should be logged: {log_content}"
    );

    loaded.send_input("pty_secret", "secret123\n").unwrap();

    let start_t = std::time::Instant::now();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("received_len=9") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(
        log_content.contains("received_len=9"),
        "input should reach the process: {log_content}"
    );
    assert!(
        !log_content.contains("secret123"),
        "pty log must not contain the password: {log_content}"
    );

    loaded.stop("pty_secret").unwrap();
}

#[cfg(unix)]
#[test]
fn test_send_input_secret_pass_shows_placeholder_not_plaintext() {
    let dir = temp_dir("secret_input_test");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script = "read -r secret; echo received_len=${#secret}; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.secret_app]
name = "Secret App"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "secret_app", dir.clone());

    loaded.start_detached("secret_app").unwrap();

    let secret_val = "secret-pass";
    let input_str = format!("{secret_val}\n");
    loaded.send_input("secret_app", &input_str).unwrap();

    let log_path = dir.join("logs/secret_app.out");
    let start_t = std::time::Instant::now();
    let mut log_content = String::new();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            log_content = c;
            if log_content.contains("received_len=11") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    assert!(
        log_content.contains("received_len=11"),
        "log: {log_content}"
    );
    // 日志中不出现明文 secret-pass
    assert!(
        !log_content.contains("secret-pass"),
        "log should NOT contain secret-pass: {log_content}"
    );
    // 出现占位符
    let char_count = input_str.chars().count();
    assert!(
        log_content.contains(&format!("› [已发送 {char_count} 个字符]")),
        "log should contain placeholder: {log_content}"
    );

    loaded.stop("secret_app").unwrap();
}

#[cfg(unix)]
#[test]
fn test_send_input_limit_4096_bytes_error() {
    let dir = temp_dir("limit_input_test");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script = "read -r line; echo got_len=${#line}; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.limit_app]
name = "Limit App"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
port = {port}
log_dir = "{d}/logs"
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "limit_app", dir.clone());

    loaded.start_detached("limit_app").unwrap();

    // 超过 4096 字节
    let huge = "a".repeat(4097);
    let err = loaded.send_input("limit_app", &huge).unwrap_err();
    assert!(
        err.to_string().contains("单次输入不能超过 4096 字节"),
        "expected limit error, got: {err}"
    );

    // 子进程没有收到
    thread::sleep(Duration::from_millis(300));
    let log_path = dir.join("logs/limit_app.out");
    if let Ok(content) = fs::read_to_string(&log_path) {
        assert!(
            !content.contains("got_len"),
            "child process should not receive input: {content}"
        );
    }

    loaded.stop("limit_app").unwrap();
}

#[cfg(unix)]
#[test]
fn test_auto_answers_skips_echo_lines_once_false() {
    let dir = temp_dir("echo_skip_test");
    let port = free_service_port(&[]);
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    let script = "echo start_marker; exec sleep 30";
    fs::write(
        &cfg,
        format!(
            r#"[projects.echo_skip_app]
name = "Echo Skip App"
root = "{d}"
start = ["sh", "-c", "{script}"]
background = true
port = {port}
log_dir = "{d}/logs"
auto_answers = [
    {{ expect = "› [已发送", send = "recursive_trigger", once = false }}
]
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard = CleanupGuard::new(&loaded, "echo_skip_app", dir.clone());

    loaded.start_detached("echo_skip_app").unwrap();

    // 等待子进程启动输出
    let log_path = dir.join("logs/echo_skip_app.out");
    let start_t = std::time::Instant::now();
    while start_t.elapsed() < Duration::from_secs(5) {
        if let Ok(c) = fs::read_to_string(&log_path) {
            if c.contains("start_marker") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    // 主动发送一次普通输入，产生 › [已发送 6 个字符] 回显
    loaded.send_input("echo_skip_app", "hello\n").unwrap();

    // status 检查 auto_answers
    let _ = loaded.status("echo_skip_app");

    // 检查 auto_answers 状态文件
    let answers_state_path = dir.join("logs/echo_skip_app.answers.json");
    if answers_state_path.exists() {
        let content = fs::read_to_string(&answers_state_path).unwrap();
        assert!(
            !content.contains("recursive_trigger"),
            "state should not trigger on echo line: {content}"
        );
    }

    // 日志中不应包含 recursive_trigger
    let final_log = fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !final_log.contains("recursive_trigger"),
        "echo line should not trigger once=false auto answer: {final_log}"
    );

    loaded.stop("echo_skip_app").unwrap();
}

#[test]
fn test_server_project_add_and_update_validation_failure_leaves_no_empty_group() {
    let dir = temp_dir("srv_grp_valid");
    let cfg = dir.join("projects.toml");
    let d = dir.to_string_lossy().replace('\\', "/");

    fs::write(
        &cfg,
        format!(
            r#"[group.existing_grp]
label = "已有分组"

[projects.app1]
name = "App 1"
root = "{d}"
start = ["sh", "-c", "sleep 30"]
port = 8080
"#
        ),
    )
    .unwrap();

    let ui_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let ui_port = ui_listener.local_addr().unwrap().port();
    let cfg_clone = cfg.clone();
    thread::spawn(move || {
        let _ = tongyun_dev_cli::server::serve(ui_listener, cfg_clone, ui_port);
    });

    let post_req = |path: &str, body: &str| -> String {
        format!(
            "POST {path} HTTP/1.1\r\n\
             Host: 127.0.0.1:{ui_port}\r\n\
             Origin: http://127.0.0.1:{ui_port}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             {}",
            body.len(),
            body
        )
    };
    let patch_req = |path: &str, body: &str| -> String {
        format!(
            "PATCH {path} HTTP/1.1\r\n\
             Host: 127.0.0.1:{ui_port}\r\n\
             Origin: http://127.0.0.1:{ui_port}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n\
             {}",
            body.len(),
            body
        )
    };

    // 1. POST /api/projects with invalid root/start and new group "ghost_grp"
    let bad_add_body = r#"{
        "id": "bad_proj",
        "root": "",
        "start": "",
        "port": 0,
        "group": "ghost_grp"
    }"#;
    let resp = http(ui_port, &post_req("/api/projects", bad_add_body));
    assert!(
        resp.starts_with("HTTP/1.1 400"),
        "expected 400 on invalid add, got: {resp}"
    );

    // 校验 projects.toml 绝对没有留下 ghost_grp
    let toml_content = fs::read_to_string(&cfg).unwrap();
    assert!(
        !toml_content.contains("ghost_grp"),
        "config should NOT contain ghost_grp after failed add: {toml_content}"
    );

    // 2. PATCH /api/projects/app1 with invalid node version and new group "ghost_patch_grp"
    let bad_patch_body = r#"{
        "node": "invalid version with spaces and @@",
        "group": "ghost_patch_grp"
    }"#;
    let patch_resp = http(ui_port, &patch_req("/api/projects/app1", bad_patch_body));
    assert!(
        patch_resp.starts_with("HTTP/1.1 400"),
        "expected 400 on invalid patch, got: {patch_resp}"
    );

    // 校验 projects.toml 绝对没有留下 ghost_patch_grp
    let toml_content_after_patch = fs::read_to_string(&cfg).unwrap();
    assert!(
        !toml_content_after_patch.contains("ghost_patch_grp"),
        "config should NOT contain ghost_patch_grp after failed patch: {toml_content_after_patch}"
    );
}

#[test]
fn group_restart_waits_and_skips_downstream_when_dependency_exits() {
    let dir = temp_dir("grp_restart_skip");
    let d = dir.to_string_lossy().replace('\\', "/");
    let cfg = dir.join("projects.toml");
    let web_hits = dir.join("web.hits");
    let web_s = web_hits.to_string_lossy().replace('\\', "/");
    let api_port = free_service_port(&[]);
    let web_port = free_service_port(&[api_port]);
    fs::write(
        &cfg,
        format!(
            r#"
[group.shop]
members = ["demo_api", "demo_web"]

[projects.demo_api]
name = "Demo API"
group = "shop"
root = "{d}"
port = {api_port}
log_dir = "{d}/logs"
start_timeout_secs = 5
start = ["sh", "-c", "exit 1"]

[projects.demo_web]
name = "Demo Web"
group = "shop"
root = "{d}"
port = {web_port}
log_dir = "{d}/logs"
start_timeout_secs = 5
depends_on = ["demo_api"]
start = ["sh", "-c", "echo ran >> '{web_s}'"]
"#
        ),
    )
    .unwrap();

    let ui_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let ui_port = ui_listener.local_addr().unwrap().port();
    let cfg_clone = cfg.clone();
    thread::spawn(move || {
        let _ = tongyun_dev_cli::server::serve(ui_listener, cfg_clone, ui_port);
    });

    let resp = post_json(ui_port, "/api/groups/shop/restart", "{}", None);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    let value: serde_json::Value = serde_json::from_str(body).expect(body);
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["results"][0]["id"], "demo_api");
    assert_eq!(value["results"][0]["ok"], false);
    assert_eq!(value["results"][1]["id"], "demo_web");
    assert_eq!(value["results"][1]["error"], "依赖服务启动失败，已跳过");
    assert!(
        !web_hits.exists(),
        "group restart started demo_web before demo_api failed"
    );
}

fn detect_python_cmd() -> &'static str {
    let candidates: &[&str] = if cfg!(windows) {
        &["py", "python", "python3"]
    } else {
        &["python3", "python"]
    };
    for &cmd in candidates {
        if std::process::Command::new(cmd)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return cmd;
        }
    }
    if cfg!(windows) {
        "py"
    } else {
        "python3"
    }
}

#[test]
fn group_start_waits_for_ready_and_web_only_starts_after_api_ready() {
    let py = detect_python_cmd();
    let dir = temp_dir("grp_start_order");
    let d = dir.to_string_lossy().replace('\\', "/");
    let cfg = dir.join("projects.toml");
    let api_port = free_service_port(&[]);
    let web_port = free_service_port(&[api_port]);
    let api_ready_file = dir.join("api_ready.flag");
    let api_ready_s = api_ready_file.to_string_lossy().replace('\\', "/");
    let web_record_file = dir.join("web_record.txt");
    let web_record_s = web_record_file.to_string_lossy().replace('\\', "/");

    let api_py = dir.join("api.py");
    let web_py = dir.join("web.py");
    let api_py_s = api_py.to_string_lossy().replace('\\', "/");
    let web_py_s = web_py.to_string_lossy().replace('\\', "/");

    let api_code = format!(
        "import socket, time\ntime.sleep(1)\ns = socket.socket()\ns.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\ns.bind(('127.0.0.1', {api_port}))\ns.listen(16)\nopen('{api_ready_s}', 'w').write('ready')\ns.settimeout(0.5)\nt0 = time.time()\nwhile time.time() - t0 < 15:\n    try:\n        conn, _ = s.accept()\n        conn.close()\n    except Exception:\n        pass\n"
    );
    fs::write(&api_py, api_code).unwrap();

    let web_code = format!(
        "import socket, time, os\nassert os.path.exists('{api_ready_s}')\ns = socket.socket()\ns.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\ns.bind(('127.0.0.1', {web_port}))\ns.listen(16)\nopen('{web_record_s}', 'w').write('started_after_api_ready')\ns.settimeout(0.5)\nt0 = time.time()\nwhile time.time() - t0 < 15:\n    try:\n        conn, _ = s.accept()\n        conn.close()\n    except Exception:\n        pass\n"
    );
    fs::write(&web_py, web_code).unwrap();

    fs::write(
        &cfg,
        format!(
            r#"
[group.shop]
members = ["demo_api", "demo_web"]

[projects.demo_api]
name = "Demo API"
group = "shop"
root = "{d}"
port = {api_port}
background = true
log_dir = "{d}/logs"
start_timeout_secs = 8
start = ["{py}", "{api_py_s}"]

[projects.demo_web]
name = "Demo Web"
group = "shop"
root = "{d}"
port = {web_port}
background = true
log_dir = "{d}/logs"
start_timeout_secs = 8
depends_on = ["demo_api"]
start = ["{py}", "{web_py_s}"]
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    let _guard_api = CleanupGuard::new(&loaded, "demo_api", dir.clone());
    let _guard_web = CleanupGuard::new(&loaded, "demo_web", dir.clone());

    // 1. Calling start_batch_detached for group "shop"
    let out = loaded.start_batch_detached(Some("shop"), None).unwrap();
    assert_eq!(out.total, 2);
    assert_eq!(out.failed, 0);
    assert_eq!(out.succeeded, 2);
    assert!(out.results[0].ok);
    assert_eq!(out.results[0].id, "demo_api");
    assert!(out.results[1].ok);
    assert_eq!(out.results[1].id, "demo_web");

    // demo_web must have verified that demo_api was already ready
    assert!(api_ready_file.exists());
    assert!(web_record_file.exists());
    assert_eq!(
        fs::read_to_string(&web_record_file).unwrap(),
        "started_after_api_ready"
    );

    let _ = loaded.stop_batch(Some("shop"), None);
}

#[test]
fn group_start_web_api_skips_downstream_when_dependency_fails() {
    let py = detect_python_cmd();
    let dir = temp_dir("grp_start_skip");
    let d = dir.to_string_lossy().replace('\\', "/");
    let cfg = dir.join("projects.toml");
    let web_hits = dir.join("web.hits");
    let web_s = web_hits.to_string_lossy().replace('\\', "/");
    let api_port = free_service_port(&[]);
    let web_port = free_service_port(&[api_port]);

    fs::write(
        &cfg,
        format!(
            r#"
[group.shop]
members = ["demo_api", "demo_web"]

[projects.demo_api]
name = "Demo API"
group = "shop"
root = "{d}"
port = {api_port}
log_dir = "{d}/logs"
start_timeout_secs = 5
start = ["{py}", "-c", "import sys; sys.exit(1)"]

[projects.demo_web]
name = "Demo Web"
group = "shop"
root = "{d}"
port = {web_port}
log_dir = "{d}/logs"
start_timeout_secs = 5
depends_on = ["demo_api"]
start = ["{py}", "-c", "open('{web_s}', 'w').write('ran')"]
"#
        ),
    )
    .unwrap();

    let ui_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let ui_port = ui_listener.local_addr().unwrap().port();
    let cfg_clone = cfg.clone();
    thread::spawn(move || {
        let _ = tongyun_dev_cli::server::serve(ui_listener, cfg_clone, ui_port);
    });

    let resp = post_json(ui_port, "/api/groups/shop/start", "{}", None);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    let value: serde_json::Value = serde_json::from_str(body).expect(body);
    assert_eq!(value["ok"], false, "{value}");
    assert_eq!(value["results"][0]["id"], "demo_api");
    assert_eq!(value["results"][0]["ok"], false);
    assert_eq!(value["results"][1]["id"], "demo_web");
    assert_eq!(value["results"][1]["error"], "依赖服务启动失败，已跳过");
    assert!(
        !web_hits.exists(),
        "group start started demo_web before demo_api failed"
    );
    assert!(
        !dir.join("logs/demo_web.pid.start").exists(),
        "demo_web should never have entered starting"
    );
}

#[test]
fn group_start_detects_and_refuses_cycle() {
    let dir = temp_dir("grp_cycle");
    let d = dir.to_string_lossy().replace('\\', "/");
    let cfg = dir.join("projects.toml");
    let port_a = free_service_port(&[]);
    let port_b = free_service_port(&[port_a]);

    fs::write(
        &cfg,
        format!(
            r#"
[group.cycle_grp]
members = ["demo_api", "demo_web"]

[projects.demo_api]
name = "Demo API"
group = "cycle_grp"
root = "{d}"
port = {port_a}
log_dir = "{d}/logs"
depends_on = ["demo_web"]
start = ["echo", "api"]

[projects.demo_web]
name = "Demo Web"
group = "cycle_grp"
root = "{d}"
port = {port_b}
log_dir = "{d}/logs"
depends_on = ["demo_api"]
start = ["echo", "web"]
"#
        ),
    )
    .unwrap();

    let loaded = tongyun_dev_core::load(Some(&cfg)).unwrap();
    // 1. start_batch_detached detects cycle
    let detached_res = loaded
        .start_batch_detached(Some("cycle_grp"), None)
        .unwrap();
    assert_eq!(detached_res.succeeded, 0);
    assert_eq!(detached_res.failed, 2);
    let err_msg = detached_res.results[0].error.as_deref().unwrap_or("");
    assert!(
        err_msg.contains("检测到循环依赖"),
        "expected cycle error, got: {err_msg}"
    );

    // 2. Web API /api/groups/cycle_grp/start detects cycle
    let ui_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let ui_port = ui_listener.local_addr().unwrap().port();
    let cfg_clone = cfg.clone();
    thread::spawn(move || {
        let _ = tongyun_dev_cli::server::serve(ui_listener, cfg_clone, ui_port);
    });

    let resp = post_json(ui_port, "/api/groups/cycle_grp/start", "{}", None);
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    let value: serde_json::Value = serde_json::from_str(body).expect(body);
    assert_eq!(value["ok"], false, "{value}");
    let web_err = value["results"][0]["error"].as_str().unwrap_or("");
    assert!(
        web_err.contains("检测到循环依赖"),
        "expected cycle error in web API, got: {web_err}"
    );
}

#[test]
fn test_start_background_server_and_port_fallback() {
    let dir = temp_dir("bg_srv");
    let cfg = write_config(&dir, 0);

    // 1. Occupy initial_port with a dummy listener
    let blocker = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let initial_port = blocker.local_addr().unwrap().port();

    // 2. Start background server starting from initial_port
    let mut server =
        tongyun_dev_cli::server::start_background_server(Some(cfg.clone()), initial_port).unwrap();

    // The bound port must be > initial_port (顺延)
    assert_eq!(server.port(), initial_port + 1);

    // 3. Verify server responds to HTTP requests
    let resp = http(
        server.port(),
        &format!(
            "GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            server.port()
        ),
    );
    assert!(resp.starts_with("HTTP/1.1 200"), "response: {resp}");
    assert!(resp.contains("\"id\":\"mock\""), "response: {resp}");

    // 4. Stop the server and verify port is freed
    server.stop();
    drop(blocker);

    // Port should be closed and no longer accept connections
    thread::sleep(Duration::from_millis(50));
    assert!(TcpStream::connect(("127.0.0.1", initial_port + 1)).is_err());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_start_background_server_reuses_existing_on_initial_port() {
    let dir = temp_dir("bg_reuse");
    let cfg = write_config(&dir, 0);

    // 1. Start a real tongyun-dev background server on an available port
    let mut server1 =
        tongyun_dev_cli::server::start_background_server(Some(cfg.clone()), 0).unwrap();
    let initial_port = server1.port();
    assert_eq!(server1.port(), initial_port);
    assert!(!server1.is_reused());

    // 2. Start a second server targeting the same port; it should reuse it
    let mut server2 =
        tongyun_dev_cli::server::start_background_server(Some(cfg), initial_port).unwrap();
    assert_eq!(server2.port(), initial_port);
    assert!(server2.is_reused());

    // 3. Stopping server2 does not stop server1
    server2.stop();
    assert!(tongyun_dev_cli::server::is_tongyun_dev_server(initial_port));

    // 4. Stopping server1 cleans up
    server1.stop();
    thread::sleep(Duration::from_millis(50));
    assert!(!tongyun_dev_cli::server::is_tongyun_dev_server(
        initial_port
    ));

    let _ = fs::remove_dir_all(&dir);
}
