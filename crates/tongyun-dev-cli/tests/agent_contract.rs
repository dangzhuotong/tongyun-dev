//! Exercise the real CLI process: stdout is one JSON value and exit codes agree.
#![cfg(unix)]
use serde_json::Value;
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tongyun_dev_core::{LifeState, Loaded};

static SEQ: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    config: PathBuf,
    loaded: Loaded,
}
impl Fixture {
    fn new(start: &str, extra: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tongyun_agent_{}_{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = root.join("projects.toml");
        let background = if extra.contains("background = false") {
            ""
        } else {
            "background = true"
        };
        fs::write(
            &config,
            format!(
                r#"[projects.demo]
root = "{}"
port = {port}
{background}
start_timeout_secs = 30
start = {start}
log_dir = "{}/logs"
{extra}
"#,
                root.display(),
                root.display()
            ),
        )
        .unwrap();
        let content = fs::read_to_string(&config)
            .unwrap()
            .replace("DEMO_PORT", &port.to_string());
        fs::write(&config, content).unwrap();
        let loaded = tongyun_dev_core::load(Some(&config)).unwrap();
        Self {
            root,
            config,
            loaded,
        }
    }
    fn cli(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tongyun-dev"))
            .arg("--config")
            .arg(&self.config)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.loaded.stop("demo");
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn json(output: &Output, ok: bool) -> Value {
    assert_eq!(
        output.status.success(),
        ok,
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("stdout must contain exactly one JSON value");
    assert_eq!(value["ok"], ok);
    value
}

#[test]
fn failed_script_and_unknown_project_are_structured_and_nonzero() {
    // Foreground script prints to stdout; its diagnostics must not corrupt JSON.
    let f = Fixture::new(
        r#"["sh", "-c", "echo script-output; exit 7"]"#,
        "background = false",
    );
    let output = f.cli(&["start", "demo", "--json"]);
    let result = json(&output, false);
    assert_eq!(result["action"], "start");
    assert_eq!(result["status"]["state"], "failed");
    assert!(result["error"].as_str().unwrap().contains("exited"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("script-output"));
    let unknown = json(&f.cli(&["stop", "unknown", "--json"]), false);
    assert!(unknown["status"].is_null());
    assert!(unknown["error"]
        .as_str()
        .unwrap()
        .contains("unknown project"));
}

#[test]
fn missing_config_and_missing_logs_have_predictable_results() {
    let f = Fixture::new(r#"["sleep", "30"]"#, "");
    let install = json(&f.cli(&["install", "demo", "--json"]), false);
    assert_eq!(install["action"], "install");
    assert!(install["error"].as_str().unwrap().contains("package.json"));
    let missing = json(&f.cli(&["logs", "demo", "--json"]), true);
    assert_eq!(missing["logs"]["missing"], true);
    assert_eq!(missing["logs"]["text"], "");
    fs::create_dir_all(f.root.join("logs")).unwrap();
    fs::write(f.root.join("logs/demo.out"), "first\nERROR demo\nlast\n").unwrap();
    let tail = json(&f.cli(&["logs", "demo", "--lines", "2", "--json"]), true);
    assert_eq!(tail["logs"]["text"], "ERROR demo\nlast\n");
    let output = Command::new(env!("CARGO_BIN_EXE_tongyun-dev"))
        .args([
            "--config",
            "/definitely/missing/projects.toml",
            "start",
            "demo",
            "--json",
        ])
        .output()
        .unwrap();
    let failed = json(&output, false);
    assert!(failed["status"].is_null());
    assert!(failed["error"].is_string());
}

#[test]
fn wait_is_bounded_read_only_and_start_timeout_keeps_service_tracked() {
    let f = Fixture::new(r#"["sh", "-c", "exec sleep 30"]"#, "");
    let before = Instant::now();
    let result = json(
        &f.cli(&["start", "demo", "--timeout", "1", "--json"]),
        false,
    );
    assert!(before.elapsed() < Duration::from_secs(5));
    assert_eq!(result["status"]["state"], "starting");
    let pid = result["status"]["pid"].as_u64().unwrap();
    let marker = fs::read(f.root.join("logs/demo.pid.start")).unwrap();
    let waited = json(&f.cli(&["wait", "demo", "--timeout", "1", "--json"]), false);
    assert_eq!(waited["status"]["pid"], pid);
    assert_eq!(
        fs::read(f.root.join("logs/demo.pid.start")).unwrap(),
        marker
    );
    let stopped = json(&f.cli(&["stop", "demo", "--json"]), true);
    assert_eq!(stopped["status"]["state"], "stopped");
    let wait_stopped = json(&f.cli(&["wait", "demo", "--timeout", "1", "--json"]), false);
    assert!(wait_stopped["error"].as_str().unwrap().contains("已停止"));
    assert_eq!(f.loaded.status("demo").unwrap().state, LifeState::Stopped);
}

#[test]
fn ready_start_wait_restart_and_stop_return_consistent_json() {
    let f = Fixture::new(
        r#"["python3", "-m", "http.server", "DEMO_PORT", "--bind", "127.0.0.1"]"#,
        r#"stop = ["sh", "-c", "echo stop-diagnostic"]"#,
    );
    let started = json(
        &f.cli(&["start", "demo", "--timeout", "10", "--json"]),
        true,
    );
    assert_eq!(started["status"]["state"], "ready");
    let first_pid = started["status"]["pid"].as_u64().unwrap();
    assert_eq!(
        json(&f.cli(&["wait", "demo", "--timeout", "1", "--json"]), true)["status"]["state"],
        "ready"
    );
    let again = json(&f.cli(&["start", "demo", "--timeout", "1", "--json"]), true);
    assert_eq!(again["status"]["pid"], first_pid);
    let restarted = json(
        &f.cli(&["restart", "demo", "--timeout", "10", "--json"]),
        true,
    );
    assert_eq!(restarted["status"]["state"], "ready");
    assert_ne!(restarted["status"]["pid"], first_pid);
    let output = f.cli(&["stop", "demo", "--json"]);
    assert_eq!(json(&output, true)["status"]["state"], "stopped");
    assert!(String::from_utf8_lossy(&output.stderr).contains("stop-diagnostic"));
}

#[test]
fn conflicting_flags_and_zero_timeout_are_usage_errors() {
    let f = Fixture::new(r#"["sleep", "30"]"#, "");
    for args in [
        vec!["wait", "demo", "--timeout", "0"],
        vec!["logs", "demo", "--follow", "--json"],
        vec!["start", "demo", "--open", "--json"],
    ] {
        assert_eq!(f.cli(&args).status.code(), Some(2));
    }
}

#[test]
fn automatic_installer_is_cancelled_when_start_wait_times_out() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new(
        r#"["sh", "-c", "echo started > started.flag; exec sleep 30"]"#,
        "",
    );
    fs::write(f.root.join("package.json"), "{}").unwrap();
    let bin = f.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, text) in [
        ("node", "#!/bin/sh\nexit 0\n"),
        ("npm", "#!/bin/sh\necho $$ > install.pid\nexec sleep 30\n"),
    ] {
        let file = bin.join(name);
        fs::write(&file, text).unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut content = fs::read_to_string(&f.config).unwrap();
    content.push_str(&format!("node_home = \"{}\"\n", bin.display()));
    fs::write(&f.config, content).unwrap();
    f.loaded = tongyun_dev_core::load(Some(&f.config)).unwrap();
    let output = f.cli(&["start", "demo", "--timeout", "1", "--json"]);
    let result = json(&output, false);
    assert!(
        result["error"].as_str().unwrap().contains("安装已取消"),
        "{result}"
    );
    assert_eq!(result["status"]["state"], "failed");
    assert!(!f.root.join("logs/demo.pid.install").exists());
    assert!(!f.root.join("started.flag").exists());
    let pid = fs::read_to_string(f.root.join("install.pid")).unwrap();
    assert!(!Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .unwrap()
        .status
        .success());
}

#[test]
fn foreign_port_blocks_start_without_creating_a_process() {
    let f = Fixture::new(
        r#"["sh", "-c", "echo spawned > spawned.flag; exec sleep 30"]"#,
        "",
    );
    let port = f.loaded.status("demo").unwrap().port;
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let result = json(
        &f.cli(&["start", "demo", "--timeout", "1", "--json"]),
        false,
    );
    assert_eq!(result["status"]["port_conflict"], true);
    assert!(!f.root.join("spawned.flag").exists());
    assert!(!f.root.join("logs/demo.pid").exists());
    drop(listener);
}

#[test]
fn list_json_with_groups_contract() {
    let f = Fixture::new(
        r#"["echo", "ok"]"#,
        "[group.demo]\nlabel = \"Demo 业务线\"\norder = 1\n",
    );
    // Standard list --json is an array of projects
    let output = f.cli(&["list", "--json"]);
    assert!(output.status.success());
    let arr: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(arr.is_array());
    assert_eq!(arr[0]["id"], "demo");

    // list --json --with-groups returns { projects: [...], groups: [...] }
    let output_wg = f.cli(&["list", "--json", "--with-groups"]);
    assert!(output_wg.status.success());
    let obj: Value = serde_json::from_slice(&output_wg.stdout).unwrap();
    assert!(obj.is_object());
    assert!(obj["projects"].is_array());
    assert_eq!(obj["projects"][0]["id"], "demo");
    assert!(obj["groups"].is_array());
    assert_eq!(obj["groups"][0]["key"], "demo");
    assert_eq!(obj["groups"][0]["label"], "Demo 业务线");
    assert_eq!(obj["groups"][0]["order"], 1);
}

#[test]
fn group_cli_resolves_existing_label_and_suggests_group_add() {
    let f = Fixture::new(r#"["echo", "ok"]"#, "[group.fe]\nlabel = \"前端服务\"\n");
    // Setting group by label "前端服务" matches group key "fe"
    let out = f.cli(&["group", "demo", "前端服务"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("已将项目 `demo` 分组设置为 `fe`"));

    // Setting group by nonexistent label errors and suggests group-add
    let out_err = f.cli(&["group", "demo", "未知分组"]);
    assert!(!out_err.status.success());
    let stderr = String::from_utf8_lossy(&out_err.stderr);
    assert!(stderr.contains("未找到分组 `未知分组`"));
    assert!(stderr.contains("tongyun-dev group-add <key> --label 未知分组"));
}

#[test]
fn status_without_id_continues_past_bad_log_dir() {
    let root = std::env::temp_dir().join(format!(
        "tongyun_status_badlog_{}_{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let app = root.join("app");
    fs::create_dir_all(&app).unwrap();
    let config = root.join("projects.toml");
    let app_s = app.display();
    fs::write(
        &config,
        format!(
            "[projects.good]\nname = \"good\"\nroot = \"{app_s}\"\nport = 9\nstart = [\"true\"]\nlog_dir = \"{app_s}/logs\"\n\n[projects.bad]\nname = \"bad\"\nroot = \"{app_s}\"\nport = 9\nstart = [\"true\"]\nlog_dir = \"{{{{not_a_var}}}}/logs\"\n"
        ),
    )
    .unwrap();

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_tongyun-dev"))
            .arg("--config")
            .arg(&config)
            .args(args)
            .output()
            .unwrap()
    };

    let text = run(&["status"]);
    assert!(
        !text.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("good"), "{stdout}");
    assert!(stdout.contains("bad"), "{stdout}");
    assert!(stdout.contains("failure"), "{stdout}");
    assert!(stdout.contains("unresolved template"), "{stdout}");
    let stderr = String::from_utf8_lossy(&text.stderr);
    assert!(stderr.contains("无法解析状态"), "{stderr}");

    let json_out = run(&["status", "--json"]);
    assert!(!json_out.status.success());
    let arr: Value = serde_json::from_slice(&json_out.stdout).expect("status --json is one array");
    let rows = arr.as_array().expect("array");
    let bad = rows.iter().find(|v| v["id"] == "bad").unwrap();
    let good = rows.iter().find(|v| v["id"] == "good").unwrap();
    assert!(bad["failure"]
        .as_str()
        .unwrap()
        .contains("unresolved template"));
    assert!(bad["resolve_error"]
        .as_str()
        .unwrap()
        .contains("unresolved template"));
    assert!(good.get("resolve_error").is_none());
    assert_eq!(good["state"], "stopped");

    let list = run(&["list"]);
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_out = String::from_utf8_lossy(&list.stdout);
    assert!(list_out.contains("good"), "{list_out}");
    assert!(list_out.contains("!解析失败:"), "{list_out}");
    assert!(list_out.contains("unresolved template"), "{list_out}");

    let one = run(&["status", "bad"]);
    assert!(!one.status.success());
    assert!(String::from_utf8_lossy(&one.stderr).contains("unresolved template"));

    let _ = fs::remove_dir_all(&root);
}
