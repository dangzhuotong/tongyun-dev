//! Binary tests. They use temp dirs, a local TCP listener, and shell scripts.
//! Nothing here starts Tomcat or binds port 8080.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tongyun_dev_cli::{ListEntry, StatusJson};

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("tongyun-dev-cli-{n}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct HttpOk {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl HttpOk {
    fn spawn() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 8080, "test listener must not use the td_work port");
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
                        let mut buf = [0_u8; 1024];
                        let _ = stream.read(&mut buf);
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                        );
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for HttpOk {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn bin() -> PathBuf {
    // Clippy does not always define CARGO_BIN_EXE_*; `cargo test` does.
    if let Some(path) = option_env!("CARGO_BIN_EXE_tongyun_dev") {
        PathBuf::from(path)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/tongyun-dev")
    }
}

fn isolate(cmd: &mut Command) -> &mut Command {
    for key in [
        "TONGYUN_DEV_CONFIG",
        "TONGYUN_DEV_TD_WORK_ROOT",
        "TONGYUN_DEV_TD_WORK_TOMCAT",
        "TONGYUN_DEV_TD_WORK_LOG_DIR",
        "TONGYUN_DEV_DEMO_ROOT",
        "TONGYUN_DEV_DEMO_TOMCAT",
        "TONGYUN_DEV_DEMO_LOG_DIR",
        "TONGYUN_DEV_A_ROOT",
        "TONGYUN_DEV_B_ROOT",
    ] {
        cmd.env_remove(key);
    }
    cmd
}

fn closed_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert_ne!(port, 8080);
    port
}

fn write_project(dir: &Path) -> PathBuf {
    let root = dir.join("root");
    let logs = dir.join("logs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&logs).unwrap();
    let port = closed_port();
    let path = dir.join("projects.toml");
    let body = format!(
        r#"
        [projects.demo]
        name = "demo"
        root = "{root}"
        start = ["sh", "-c", "touch ./started-marker"]
        stop = ["sh", "-c", "rm -f \"{pid}\""]
        tomcat = "{tomcat}"
        log_dir = "{logs}"
        log_file = "{{{{log_dir}}}}/app.out"
        pid_file = "{{{{log_dir}}}}/app.pid"
        port = {port}
        login_url = "http://127.0.0.1:{port}/login"
        "#,
        root = root.display(),
        tomcat = dir.join("tomcat").display(),
        logs = logs.display(),
        pid = logs.join("app.pid").display(),
    );
    std::fs::write(&path, body).unwrap();
    path
}

fn run(dir: &Path, config: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(bin());
    isolate(&mut cmd);
    cmd.current_dir(dir)
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .expect("spawn tongyun-dev")
}

#[test]
fn help_lists_subcommands() {
    let mut cmd = Command::new(bin());
    let output = cmd.arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    for word in ["list", "start", "stop", "status", "logs"] {
        assert!(text.contains(word), "help missing {word}: {text}");
    }
}

#[test]
fn list_walks_config_sorts_ids_and_expands_tilde() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    let nested = repo.join("src/nested");
    std::fs::create_dir_all(repo.join("config")).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        repo.join("config/projects.toml"),
        r#"
        [projects.b_proj]
        name = "B"
        root = "~/b"
        start = ["true"]
        stop = ["true"]
        tomcat = "~/tomcat"
        log_dir = "~/logs"
        log_file = "{{log_dir}}/b.out"
        pid_file = "{{log_dir}}/b.pid"
        port = 9
        login_url = "http://127.0.0.1:9/b"

        [projects.a_proj]
        name = "A"
        root = "~/a"
        start = ["true"]
        stop = ["true"]
        tomcat = "~/tomcat"
        log_dir = "~/logs"
        log_file = "{{log_dir}}/a.out"
        pid_file = "{{log_dir}}/a.pid"
        port = 9
        login_url = "http://127.0.0.1:9/a"
        "#,
    )
    .unwrap();

    let mut cmd = Command::new(bin());
    isolate(&mut cmd);
    let output = cmd
        .current_dir(&nested)
        .env("HOME", &home)
        .args(["list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with("a_proj\tA\t"));
    assert!(lines[0].contains(&format!("{}/a", home.display())));
    assert!(lines[1].starts_with("b_proj\t"));
    assert!(!text.contains("login"));
    assert!(!text.contains("catalina"));

    let mut json_cmd = Command::new(bin());
    isolate(&mut json_cmd);
    let json_out = json_cmd
        .current_dir(&nested)
        .env("HOME", &home)
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(json_out.status.success());
    let entries = ListEntry::from_json_vec(&String::from_utf8_lossy(&json_out.stdout)).unwrap();
    assert_eq!(entries[0].id, "a_proj");
    assert_eq!(entries[1].id, "b_proj");
    assert_eq!(entries[0].root, home.join("a").display().to_string());
}

#[test]
fn explicit_config_beats_env_and_unknown_project_fails() {
    let tmp = TempDir::new();
    let good = write_project(tmp.path());
    let bad = tmp.path().join("missing.toml");
    let mut cmd = Command::new(bin());
    isolate(&mut cmd);
    let output = cmd
        .current_dir(tmp.path())
        .env("TONGYUN_DEV_CONFIG", &bad)
        .args(["--config", good.to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let status = run(tmp.path(), &good, &["status", "nope", "--json"]);
    assert!(!status.status.success());
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(err.contains("unknown project"));
    assert!(!err.contains("start-td-work"));
}

#[test]
fn status_json_live_pid_closed_port_and_env_root_override() {
    let tmp = TempDir::new();
    let config = write_project(tmp.path());
    let pid_file = tmp.path().join("logs/app.pid");
    std::fs::write(&pid_file, format!("{}\n", std::process::id())).unwrap();
    let overridden = tmp.path().join("override-root");
    std::fs::create_dir_all(&overridden).unwrap();

    let mut cmd = Command::new(bin());
    isolate(&mut cmd);
    let output = cmd
        .current_dir(tmp.path())
        .env("TONGYUN_DEV_DEMO_ROOT", &overridden)
        .args([
            "--config",
            config.to_str().unwrap(),
            "status",
            "demo",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = StatusJson::from_json(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert!(status.running);
    assert_eq!(status.pid, Some(std::process::id()));
    assert_ne!(status.port, 8080);
    assert!(!status.port_open);
    assert!(!status.login_probe.ok);
    assert_eq!(status.login_probe.http_code, None);
    assert_eq!(status.root, overridden.display().to_string());
    assert_eq!(
        status.log_path,
        tmp.path().join("logs/app.out").display().to_string()
    );
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(!rendered.contains("super-secret"));
    assert!(!rendered.contains("started-marker"));
}

#[test]
fn status_json_login_ok_when_local_http_returns_200() {
    let tmp = TempDir::new();
    let server = HttpOk::spawn();
    let root = tmp.path().join("root");
    let logs = tmp.path().join("logs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&logs).unwrap();
    let config = tmp.path().join("projects.toml");
    std::fs::write(
        &config,
        format!(
            r#"
            [projects.demo]
            name = "demo"
            root = "{root}"
            start = ["true"]
            stop = ["true"]
            tomcat = "{tomcat}"
            log_dir = "{logs}"
            log_file = "{{{{log_dir}}}}/app.out"
            pid_file = "{{{{log_dir}}}}/app.pid"
            port = {port}
            login_url = "http://127.0.0.1:{port}/login"
            "#,
            root = root.display(),
            tomcat = tmp.path().join("tomcat").display(),
            logs = logs.display(),
            port = server.port,
        ),
    )
    .unwrap();

    let output = run(tmp.path(), &config, &["status", "demo", "--json"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = StatusJson::from_json(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert!(!status.running);
    assert!(status.port_open);
    assert_eq!(status.login_probe.http_code, Some(200));
    assert!(status.login_probe.ok);
    assert_eq!(status.port, server.port);
}

#[test]
fn logs_print_tail_and_follow_sees_a_new_line() {
    let tmp = TempDir::new();
    let config = write_project(tmp.path());
    let log_path = tmp.path().join("logs/app.out");
    let mut body = String::new();
    for i in 0..5 {
        body.push_str(&format!("line-{i}\n"));
    }
    std::fs::write(&log_path, &body).unwrap();

    let output = run(tmp.path(), &config, &["logs", "demo", "--lines", "1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("line-4"));
    assert!(!text.contains("line-0"));

    let missing = tmp.path().join("logs/absent.out");
    let _ = missing;
    let bad = run(tmp.path(), &config, &["logs", "missing"]);
    assert!(!bad.status.success());

    let mut child = isolate(&mut Command::new(bin()))
        .current_dir(tmp.path())
        .args([
            "--config",
            config.to_str().unwrap(),
            "logs",
            "demo",
            "--follow",
            "--lines",
            "1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let guard = ChildGuard(&mut child);
    let stdout = guard.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
        let mut first = String::new();
        let _ = std::io::BufRead::read_line(&mut reader, &mut first);
        let _ = tx.send(first);
        let mut second = String::new();
        let _ = std::io::BufRead::read_line(&mut reader, &mut second);
        let _ = tx.send(second);
    });
    let first = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("first log line");
    assert!(first.contains("line-4"), "first={first:?}");
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .unwrap();
    writeln!(file, "line-new").unwrap();
    file.flush().unwrap();
    drop(file);
    let second = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("followed log line");
    assert!(second.contains("line-new"), "second={second:?}");
    drop(guard);
    let _ = reader.join();
}

struct ChildGuard<'a>(&'a mut Child);

impl Drop for ChildGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn start_then_stop_and_failed_stop_hides_arguments() {
    let tmp = TempDir::new();
    let config = write_project(tmp.path());
    let started = run(tmp.path(), &config, &["start", "demo"]);
    assert!(
        started.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(tmp.path().join("root/started-marker").is_file());
    let stdout = String::from_utf8_lossy(&started.stdout);
    assert!(stdout.contains("running="));
    assert!(!stdout.contains("started-marker"));

    let stopped = run(tmp.path(), &config, &["stop", "demo"]);
    assert!(
        stopped.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&stopped.stderr)
    );

    let secret = "super-secret-token-value";
    let root = tmp.path().join("root2");
    let logs = tmp.path().join("logs2");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&logs).unwrap();
    let pid = logs.join("app.pid");
    std::fs::write(&pid, format!("{}\n", std::process::id())).unwrap();
    let secret_cfg = tmp.path().join("secret.toml");
    std::fs::write(
        &secret_cfg,
        format!(
            r#"
            [projects.demo]
            name = "demo"
            root = "{root}"
            start = ["true"]
            stop = ["false", "{secret}"]
            tomcat = "{tomcat}"
            log_dir = "{logs}"
            log_file = "{{{{log_dir}}}}/app.out"
            pid_file = "{{{{log_dir}}}}/app.pid"
            port = {port}
            login_url = "http://127.0.0.1:{port}/login"
            "#,
            root = root.display(),
            tomcat = tmp.path().join("tomcat").display(),
            logs = logs.display(),
            port = closed_port(),
        ),
    )
    .unwrap();
    let failed = run(tmp.path(), &secret_cfg, &["stop", "demo"]);
    assert!(!failed.status.success());
    let err = String::from_utf8_lossy(&failed.stderr);
    assert!(err.contains("exited"));
    assert!(
        !err.contains(secret),
        "stop error leaked an argument: {err}"
    );
}

#[test]
fn missing_log_and_missing_config_fail() {
    let tmp = TempDir::new();
    let config = write_project(tmp.path());
    let output = run(tmp.path(), &config, &["logs", "demo"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("log file missing"));

    let missing = tmp.path().join("nope.toml");
    let mut cmd = Command::new(bin());
    let output = isolate(&mut cmd)
        .args(["--config", missing.to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("config file not found"));
}
