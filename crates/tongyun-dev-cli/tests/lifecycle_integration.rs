//! End-to-end checks through the shared core and the web console.
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tongyun_it_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("logs")).unwrap();
    dir
}

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn write_config(dir: &Path, port: u16) -> PathBuf {
    let d = dir.display();
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
    let add = format!(
        r#"{{"id":"web_app","name":"Web App","root":"{}","start":"pnpm dev","port":5173,"node":"22","java":"17"}}"#,
        dir.display()
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
