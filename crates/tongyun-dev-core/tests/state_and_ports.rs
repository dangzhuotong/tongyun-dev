use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tongyun_dev_core::{
    append_port_arg, append_port_to_argv, append_port_to_script, assemble_start_cmd,
    default_port_mode, derive_state, format_home, generate_group_key, install_marker_path,
    map_key, normalize_start_arg, profile_marker_path, resolve_input_data,
    shell_argv_with_os, start_marker_path, unresolved_status, validate_group_key,
    Candidate, Confidence, HostOs, LifeState, PackageManager, ProjectRow, StateInput,
    ToolchainError, DEFAULT_ADD_PORT, DEFAULT_LOG_DIR, DEPS_MISSING_MSG, MAX_INPUT_BYTES,
};

#[test]
fn test_lifestate_display_and_serde() {
    let cases = [
        (LifeState::Stopped, "stopped"),
        (LifeState::Installing, "installing"),
        (LifeState::Starting, "starting"),
        (LifeState::PortOpen, "port_open"),
        (LifeState::Ready, "ready"),
        (LifeState::Failed, "failed"),
    ];
    for (state, s) in cases {
        assert_eq!(state.to_string(), s);
        let json = serde_json::to_string(&state).expect("serialize LifeState");
        assert_eq!(json, format!("\"{s}\""));
        let parsed: LifeState = serde_json::from_str(&json).expect("deserialize LifeState");
        assert_eq!(parsed, state);
    }
}

#[test]
fn test_derive_state_comprehensive() {
    // 1. Startup command error takes precedence
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 30,
        start_error: Some("command not found".into()),
    });
    assert_eq!(state, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("command not found"));

    // 2. Port open without probe -> Ready
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Ready);
    assert!(err.is_none());

    // 3. Port open with probe ok -> Ready
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: true,
        started_at: Some(100),
        now: 105,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Ready);
    assert!(err.is_none());

    // 4. Port open with probe failing and not timed out -> PortOpen
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::PortOpen);
    assert!(err.is_none());

    // 5. Port open with probe failing and timed out -> Failed
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: false,
        started_at: Some(100),
        now: 135,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("超过 30 秒仍未就绪"));

    // 6. Port conflict prevents Ready/PortOpen even if port is open
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: true,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Starting);
    assert!(err.is_none());

    // 7. Port closed, process alive and not timed out -> Starting
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Starting);
    assert!(err.is_none());

    // 8. Process exited during startup grace period (< 3s) -> remains Starting
    let (state, err) = derive_state(StateInput {
        process_alive: false,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 102, // elapsed = 2 < 3s grace
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Starting);
    assert!(err.is_none());

    // 9. Process exited after grace period (>= 3s) -> Failed
    let (state, err) = derive_state(StateInput {
        process_alive: false,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 103, // elapsed = 3 >= 3s grace
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("进程已退出"));

    // 10. Startup timed out without port open -> Failed
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 130, // elapsed = 30 >= timeout 30
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("超过 30 秒仍未就绪"));

    // 11. Without started_at: process alive and port closed -> Starting
    let (state, err) = derive_state(StateInput {
        process_alive: true,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: None,
        now: 100,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Starting);
    assert!(err.is_none());

    // 12. Without started_at: port open with probe pending -> PortOpen
    let (state, err) = derive_state(StateInput {
        process_alive: false,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: false,
        started_at: None,
        now: 100,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::PortOpen);
    assert!(err.is_none());

    // 13. Without started_at: nothing alive and port closed -> Stopped
    let (state, err) = derive_state(StateInput {
        process_alive: false,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: None,
        now: 100,
        timeout_secs: 30,
        start_error: None,
    });
    assert_eq!(state, LifeState::Stopped);
    assert!(err.is_none());
}

#[test]
fn test_default_port_mode() {
    assert_eq!(default_port_mode(), "none");
}

#[test]
fn test_append_port_arg_empty() {
    assert_eq!(append_port_arg(&[], 8080, None), Vec::<String>::new());
    assert_eq!(append_port_arg(&[], 8080, Some("-p")), Vec::<String>::new());
}

#[test]
fn test_append_port_arg_plain_argv() {
    // pnpm
    let pnpm = vec!["pnpm".to_string(), "dev".to_string()];
    assert_eq!(
        append_port_arg(&pnpm, 5173, None),
        vec!["pnpm", "dev", "--port", "5173"]
    );

    // yarn
    let yarn = vec!["yarn".to_string(), "start".to_string()];
    assert_eq!(
        append_port_arg(&yarn, 3000, None),
        vec!["yarn", "start", "--port", "3000"]
    );

    // npm run without double dash
    let npm = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];
    assert_eq!(
        append_port_arg(&npm, 5173, None),
        vec!["npm", "run", "dev", "--", "--port", "5173"]
    );

    // npm run with existing double dash
    let npm_dash = vec![
        "npm".to_string(),
        "run".to_string(),
        "dev".to_string(),
        "--".to_string(),
        "--host".to_string(),
    ];
    assert_eq!(
        append_port_arg(&npm_dash, 5173, None),
        vec!["npm", "run", "dev", "--", "--host", "--port", "5173"]
    );

    // custom port flag
    let custom = vec!["python".to_string(), "main.py".to_string()];
    assert_eq!(
        append_port_arg(&custom, 8000, Some("-p")),
        vec!["python", "main.py", "-p", "8000"]
    );

    // already present flags do not duplicate
    let with_port = vec!["pnpm".to_string(), "dev".to_string(), "--port".to_string(), "3000".to_string()];
    assert_eq!(append_port_arg(&with_port, 5173, None), with_port);

    let with_p = vec!["pnpm".to_string(), "dev".to_string(), "-p".to_string(), "3000".to_string()];
    assert_eq!(append_port_arg(&with_p, 5173, None), with_p);

    let with_port_eq = vec!["pnpm".to_string(), "dev".to_string(), "--port=3000".to_string()];
    assert_eq!(append_port_arg(&with_port_eq, 5173, None), with_port_eq);

    let with_p_eq = vec!["pnpm".to_string(), "dev".to_string(), "-p=3000".to_string()];
    assert_eq!(append_port_arg(&with_p_eq, 5173, None), with_p_eq);
}

#[test]
fn test_append_port_arg_shell_scripts() {
    // bash -c single line
    let bash = vec![
        "bash".to_string(),
        "-c".to_string(),
        "exec pnpm dev --host 0.0.0.0".to_string(),
    ];
    assert_eq!(
        append_port_arg(&bash, 5173, None),
        vec!["bash", "-c", "exec pnpm dev --host 0.0.0.0 --port 5173"]
    );

    // sh -c single line npm
    let sh = vec![
        "sh".to_string(),
        "-c".to_string(),
        "exec npm run dev".to_string(),
    ];
    assert_eq!(
        append_port_arg(&sh, 5173, None),
        vec!["sh", "-c", "exec npm run dev -- --port 5173"]
    );

    // cmd /C
    let cmd = vec![
        "cmd".to_string(),
        "/C".to_string(),
        "pnpm dev".to_string(),
    ];
    assert_eq!(
        append_port_arg(&cmd, 5173, None),
        vec!["cmd", "/C", "pnpm dev --port 5173"]
    );

    // powershell -c
    let ps = vec![
        "powershell".to_string(),
        "-c".to_string(),
        "pnpm dev".to_string(),
    ];
    assert_eq!(
        append_port_arg(&ps, 5173, None),
        vec!["powershell", "-c", "pnpm dev --port 5173"]
    );

    // powershell 6-arg form
    let ps6 = vec![
        "powershell".to_string(),
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-Command".to_string(),
        "pnpm dev".to_string(),
    ];
    assert_eq!(
        append_port_arg(&ps6, 5173, None),
        vec![
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "pnpm dev --port 5173"
        ]
    );

    // multi-line bash script
    let multi = vec![
        "bash".to_string(),
        "-c".to_string(),
        "# setup env\ncd app\nexec pnpm dev".to_string(),
    ];
    assert_eq!(
        append_port_arg(&multi, 5173, None),
        vec!["bash", "-c", "# setup env\ncd app\nexec pnpm dev --port 5173"]
    );
}

#[test]
fn test_append_port_to_script() {
    assert_eq!(append_port_to_script("", 5173, "--port"), "");
    assert_eq!(
        append_port_to_script("exec pnpm dev", 5173, "--port"),
        "exec pnpm dev --port 5173"
    );
    assert_eq!(
        append_port_to_script("pnpm dev --port 3000", 5173, "--port"),
        "pnpm dev --port 3000"
    );
    assert_eq!(
        append_port_to_script("echo step1\nexec npm run dev", 8080, "--port"),
        "echo step1\nexec npm run dev -- --port 8080"
    );
}

#[test]
fn test_append_port_to_argv() {
    assert_eq!(append_port_to_argv(&[], 8080, "--port"), Vec::<String>::new());
    let base = vec!["node".to_string(), "server.js".to_string()];
    assert_eq!(
        append_port_to_argv(&base, 8080, "--port"),
        vec!["node", "server.js", "--port", "8080"]
    );
    let npm = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];
    assert_eq!(
        append_port_to_argv(&npm, 8080, "-p"),
        vec!["npm", "run", "dev", "--", "-p", "8080"]
    );
    let with_flag = vec!["node".to_string(), "server.js".to_string(), "--port".to_string(), "8080".to_string()];
    assert_eq!(append_port_to_argv(&with_flag, 9000, "--port"), with_flag);
}

#[test]
fn test_normalize_start_arg() {
    assert_eq!(
        normalize_start_arg("pnpm dev -- --port 4001"),
        "pnpm dev --port 4001"
    );
    assert_eq!(
        normalize_start_arg("exec pnpm dev -- --port 4004"),
        "exec pnpm dev --port 4004"
    );
    assert_eq!(
        normalize_start_arg("pnpm run dev:antd -- --port 4002"),
        "pnpm run dev:antd --port 4002"
    );
    // npm double dash must be kept
    assert_eq!(
        normalize_start_arg("npm run dev -- --port 4001"),
        "npm run dev -- --port 4001"
    );
    // yarn remains untouched
    assert_eq!(
        normalize_start_arg("yarn dev --port 3000"),
        "yarn dev --port 3000"
    );
}

#[test]
fn test_assemble_start_cmd() {
    let cmd = vec!["mvn".to_string(), "spring-boot:run".to_string()];
    assert_eq!(assemble_start_cmd(&cmd, Some("20"), Some("17")), cmd);
    assert_eq!(assemble_start_cmd(&cmd, None, None), cmd);
}

#[test]
fn test_marker_paths() {
    let pid = Path::new("logs/web.pid");
    assert_eq!(start_marker_path(pid), PathBuf::from("logs/web.pid.start"));
    assert_eq!(profile_marker_path(pid), PathBuf::from("logs/web.pid.profile"));
    assert_eq!(install_marker_path(pid), PathBuf::from("logs/web.pid.install"));
}

#[test]
fn test_package_manager_install_command() {
    assert_eq!(PackageManager::Pnpm.install_command(), "pnpm install");
    assert_eq!(PackageManager::Yarn.install_command(), "yarn install");
    assert_eq!(PackageManager::Npm.install_command(), "npm install");
}

#[test]
fn test_global_constants() {
    assert_eq!(DEFAULT_LOG_DIR, "~/.tongyun-dev/logs");
    assert_eq!(DEFAULT_ADD_PORT, 8080);
    assert_eq!(MAX_INPUT_BYTES, 4096);
    assert_eq!(DEPS_MISSING_MSG, "依赖未安装（缺 node_modules）");
}

#[test]
fn test_map_key() {
    assert_eq!(map_key("up"), Some("\x1b[A"));
    assert_eq!(map_key("down"), Some("\x1b[B"));
    assert_eq!(map_key("right"), Some("\x1b[C"));
    assert_eq!(map_key("left"), Some("\x1b[D"));
    assert_eq!(map_key("enter"), Some("\n"));
    assert_eq!(map_key("tab"), Some("\t"));
    assert_eq!(map_key("esc"), Some("\x1b"));
    assert_eq!(map_key("escape"), Some("\x1b"));
    assert_eq!(map_key("ctrl-c"), Some("\x03"));
    assert_eq!(map_key("ctrl_c"), Some("\x03"));
    assert_eq!(map_key("ctrlc"), Some("\x03"));
    assert_eq!(map_key("UP"), Some("\x1b[A"));
    assert_eq!(map_key("Enter"), Some("\n"));
    assert_eq!(map_key("Ctrl-C"), Some("\x03"));
    assert_eq!(map_key("backspace"), None);
    assert_eq!(map_key("f1"), None);
}

#[test]
fn test_resolve_input_data() {
    assert_eq!(
        resolve_input_data(Some("line"), true, None).unwrap(),
        "line\n"
    );
    assert_eq!(
        resolve_input_data(Some("line"), false, None).unwrap(),
        "line"
    );
    assert_eq!(resolve_input_data(None, true, None).unwrap(), "\n");
    assert_eq!(
        resolve_input_data(None, false, Some("up")).unwrap(),
        "\x1b[A"
    );
    assert_eq!(
        resolve_input_data(Some("input"), false, Some("tab")).unwrap(),
        "input\t"
    );
    // Enter is ignored when key is explicitly given
    assert_eq!(
        resolve_input_data(None, true, Some("ctrl-c")).unwrap(),
        "\x03"
    );

    // Empty input fails
    assert!(resolve_input_data(None, false, None).is_err());
    // Unknown key fails
    assert!(resolve_input_data(None, false, Some("invalid_key")).is_err());
}

#[test]
fn test_validate_group_key() {
    assert!(validate_group_key("").is_err());
    assert!(validate_group_key(&"a".repeat(41)).is_err());
    assert!(validate_group_key("InvalidCapital").is_err());
    assert!(validate_group_key("space in key").is_err());
    assert!(validate_group_key("group@bad").is_err());

    assert!(validate_group_key("g1").is_ok());
    assert!(validate_group_key("my-backend-group_01").is_ok());
    assert!(validate_group_key(&"a".repeat(40)).is_ok());
}

#[test]
fn test_generate_group_key() {
    assert_eq!(
        generate_group_key("Backend Services", &[] as &[&str]),
        "backend-services"
    );
    assert_eq!(
        generate_group_key("My App (v2)!", &[] as &[&str]),
        "my-app-v2"
    );
    // When generated key exists, falls back to g1, g2...
    assert_eq!(
        generate_group_key("Frontend", &["frontend"]),
        "g1"
    );
    assert_eq!(
        generate_group_key("Frontend", &["frontend", "g1"]),
        "g2"
    );
    // Non-ASCII labels generate g1, g2...
    assert_eq!(generate_group_key("后台服务", &[] as &[&str]), "g1");
    assert_eq!(generate_group_key("后台服务", &["g1"]), "g2");
    // Empty label generates g1
    assert_eq!(generate_group_key("", &[] as &[&str]), "g1");
    // Truncates long label to <= 40 chars
    let long_label = "a".repeat(50);
    let gen = generate_group_key(&long_label, &[] as &[&str]);
    assert!(gen.len() <= 40);
}

#[test]
fn test_unresolved_status() {
    let row = ProjectRow {
        id: "demo_api".to_string(),
        name: "Demo API".to_string(),
        group: Some("backend".to_string()),
        root: "/demo/api".to_string(),
        enabled: true,
        port: 8080,
        reason: String::new(),
        configured: true,
        start_timeout_secs: Some(60),
        stop_grace_secs: Some(10),
        node: None,
        java: Some("17".to_string()),
        java_home: None,
        node_home: None,
        shell: None,
        auto_install: true,
        port_mode: default_port_mode(),
        port_arg: None,
        port_env: None,
        env: BTreeMap::new(),
        pty: false,
        auto_answers: Vec::new(),
        links: Vec::new(),
        default_profile: None,
        profiles: BTreeMap::new(),
        depends_on: Vec::new(),
        resolve_error: Some("test error".to_string()),
    };

    let st = unresolved_status(&row, "bad config");
    assert_eq!(st.id, "demo_api");
    assert_eq!(st.name, "Demo API");
    assert!(!st.running);
    assert_eq!(st.state, LifeState::Stopped);
    assert_eq!(st.port, 8080);
    assert_eq!(st.failure.as_deref(), Some("bad config"));
    assert_eq!(st.root, "/demo/api");
    assert_eq!(st.resolve_error.as_deref(), Some("test error"));
    assert!(!st.port_conflict);
    assert_eq!(st.timeout_secs, 0);
}

#[test]
fn test_format_home() {
    let home = Path::new("/home/demo");
    assert_eq!(format_home(Path::new("/home/demo/repo"), Some(home)), "~/repo");
    assert_eq!(format_home(Path::new("/home/demo"), Some(home)), "~");
    assert_eq!(format_home(Path::new("/var/log"), Some(home)), "/var/log");
    assert_eq!(format_home(Path::new("/var/log"), None), "/var/log");
}

#[test]
fn test_shell_argv_with_os() {
    assert_eq!(
        shell_argv_with_os("dir", None, HostOs::Windows),
        vec!["cmd", "/C", "dir"]
    );
    assert_eq!(
        shell_argv_with_os("dir", Some("cmd"), HostOs::Windows),
        vec!["cmd", "/C", "dir"]
    );
    assert_eq!(
        shell_argv_with_os("Get-Process", Some("powershell"), HostOs::Windows),
        vec![
            "powershell",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "Get-Process"
        ]
    );

    assert_eq!(
        shell_argv_with_os("ls", None, HostOs::Linux),
        vec!["bash", "-c", "ls"]
    );
    assert_eq!(
        shell_argv_with_os("ls", Some("sh"), HostOs::Linux),
        vec!["sh", "-c", "ls"]
    );
    assert_eq!(
        shell_argv_with_os("ls", Some("bash"), HostOs::Mac),
        vec!["bash", "-c", "ls"]
    );
}

#[test]
fn test_toolchain_error_display() {
    let err_java = ToolchainError::MissingJava {
        version: "17".into(),
        searched: "~/.sdkman".into(),
    };
    assert!(err_java.to_string().contains("缺 Java 17"));
    assert!(err_java.to_string().contains("17.*"));

    let err_java8 = ToolchainError::MissingJava {
        version: "8".into(),
        searched: "~/.sdkman".into(),
    };
    assert!(err_java8.to_string().contains("8.*"));

    let err_node = ToolchainError::MissingNode {
        version: "20".into(),
        searched: "fnm".into(),
    };
    assert!(err_node.to_string().contains("缺 Node 20"));
    assert!(err_node.to_string().contains("v20.*"));

    let err_jh = ToolchainError::InvalidJavaHome {
        path: PathBuf::from("/nonexistent"),
        reason: "bin/java not found".into(),
    };
    assert!(err_jh.to_string().contains("指定的 java_home 无效"));

    let err_nh = ToolchainError::InvalidNodeHome {
        path: PathBuf::from("/nonexistent"),
        reason: "node not found".into(),
    };
    assert!(err_nh.to_string().contains("指定的 node_home 无效"));
}

#[test]
fn test_candidate_to_new_project() {
    let cand = Candidate {
        id: "web".to_string(),
        name: "Web App".to_string(),
        root: "~/code/web".to_string(),
        kind: "vite".to_string(),
        label: "Vite".to_string(),
        confidence: Confidence::High,
        start: "pnpm dev".to_string(),
        stop: None,
        port: 5173,
        port_guessed: false,
        health_url: None,
        login_url: None,
        java: None,
        node: Some("20".to_string()),
        background: true,
        needs_tomcat: false,
        why: vec!["vite config".to_string()],
        existing_id: None,
        port_mode: Some("none".to_string()),
    };
    let np = cand.to_new_project();
    assert_eq!(np.id, "web");
    assert_eq!(np.name.as_deref(), Some("Web App"));
    assert_eq!(np.port, Some(5173));
    assert_eq!(np.node.as_deref(), Some("20"));
    assert_eq!(np.background, Some(true));
    assert_eq!(np.enabled, None);
    assert!(np.reason.is_none());

    let mut tc_cand = cand.clone();
    tc_cand.needs_tomcat = true;
    let np_tc = tc_cand.to_new_project();
    assert_eq!(np_tc.enabled, Some(false));
    assert!(np_tc.reason.unwrap().contains("Tomcat WAR"));
}
