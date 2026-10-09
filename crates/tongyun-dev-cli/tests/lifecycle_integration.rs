use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use tongyun_dev_cli::config::{FileConfig, Project};
use tongyun_dev_cli::runner::{read_last_lines, status_of};
use tongyun_dev_cli::server::start_ui_server;

#[test]
fn test_project_lifecycle_simulation() {
    let temp_dir = std::env::temp_dir().join(format!("tongyun_test_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("create temp dir");

    let pid_file = temp_dir.join("app.pid");
    let log_file = temp_dir.join("app.log");

    // Choose an available port
    let probe_listener = TcpListener::bind("127.0.0.1:0").expect("find free port");
    let test_port = probe_listener.local_addr().expect("port").port();
    drop(probe_listener);

    let proj = Project {
        name: "Mock Service".into(),
        root: temp_dir.display().to_string(),
        start: vec!["echo".into(), "start".into()],
        stop: vec!["echo".into(), "stop".into()],
        tomcat: "".into(),
        log_dir: temp_dir.display().to_string(),
        log_file: log_file.display().to_string(),
        pid_file: pid_file.display().to_string(),
        port: test_port,
        login_url: format!("http://127.0.0.1:{test_port}"),
    };

    // Phase 1: Before service starts
    let st1 = status_of("mock", &proj);
    assert!(!st1.running);
    assert!(!st1.port_open);
    assert!(st1.pid.is_none());

    // Phase 2: Simulate service running
    // 1. Bind port
    let service_listener = TcpListener::bind(format!("127.0.0.1:{test_port}")).expect("bind service");
    // 2. Write current PID
    let current_pid = std::process::id();
    fs::write(&pid_file, current_pid.to_string()).expect("write pid");
    // 3. Write initial log
    fs::write(&log_file, "Service started successfully\nListening on port\n").expect("write log");

    let st2 = status_of("mock", &proj);
    assert!(st2.running);
    assert_eq!(st2.pid, Some(current_pid));
    assert!(st2.port_open);

    // Verify logs
    let logs = read_last_lines(&log_file, 10).expect("read logs");
    assert_eq!(logs.len(), 2);
    assert!(logs[0].contains("started successfully"));

    // Phase 3: Simulate service stopping
    drop(service_listener);
    let _ = fs::remove_file(&pid_file);
    thread::sleep(Duration::from_millis(100));

    let st3 = status_of("mock", &proj);
    assert!(!st3.running);
    assert!(!st3.port_open);
    assert!(st3.pid.is_none());

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_embedded_ui_server_http() {
    let temp_dir = std::env::temp_dir().join(format!("tongyun_ui_test_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).expect("create temp dir");
    let cfg_path = temp_dir.join("projects.toml");

    let mut projects = HashMap::new();
    projects.insert(
        "web_app".into(),
        Project {
            name: "Web Application".into(),
            root: temp_dir.display().to_string(),
            start: vec!["echo".into()],
            stop: vec![],
            tomcat: "".into(),
            log_dir: "".into(),
            log_file: "".into(),
            pid_file: "".into(),
            port: 9876,
            login_url: "http://127.0.0.1:9876".into(),
        },
    );

    let config = FileConfig { projects };

    // Find free port for UI server
    let probe = TcpListener::bind("127.0.0.1:0").expect("probe port");
    let ui_port = probe.local_addr().expect("port").port();
    drop(probe);

    let cfg_path_clone = cfg_path.clone();
    let config_clone = config.clone();

    // Spawn UI server in background thread
    thread::spawn(move || {
        let _ = start_ui_server(cfg_path_clone, config_clone, ui_port, false);
    });

    // Wait up to 2 seconds for server to start
    let mut stream = None;
    for _ in 0..20 {
        thread::sleep(Duration::from_millis(100));
        if let Ok(s) = TcpStream::connect(format!("127.0.0.1:{ui_port}")) {
            stream = Some(s);
            break;
        }
    }

    assert!(stream.is_some(), "UI server failed to accept connections");
    let mut stream = stream.unwrap();

    // Request /api/projects
    let req = format!("GET /api/projects HTTP/1.1\r\nHost: 127.0.0.1:{ui_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).expect("write request");

    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    let resp_str = String::from_utf8_lossy(&response);

    assert!(resp_str.contains("HTTP/1.1 200 OK"));
    assert!(resp_str.contains("application/json"));
    assert!(resp_str.contains("web_app"));
    assert!(resp_str.contains("Web Application"));

    let _ = fs::remove_dir_all(&temp_dir);
}
