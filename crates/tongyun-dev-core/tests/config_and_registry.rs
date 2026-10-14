use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;
use tongyun_dev_core::{
    default_port_mode, derive_state, generate_group_key, load, map_key, normalize_start_arg,
    resolve_input_data, unresolved_status, validate_group_key, LifeState, NewProject,
    ProjectRow, StateInput, WebLink,
};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("ty_test_{tag}_{pid}_{id}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------------
// 1. Config Parsing Tests
// ---------------------------------------------------------------------------

#[test]
fn test_config_parsing_valid_full_featured() {
    let temp = TempDir::new("cfg_full");
    let cfg_file = temp.path().join("projects.toml");
    let app_root = temp.path().join("demo_app");
    std::fs::create_dir_all(&app_root).unwrap();

    let toml = format!(
        r#"
log_dir = "{log_dir}"

[group.backend]
label = "Backend Services"
order = 1
members = ["api_srv"]
depends_on = []

[group.frontend]
label = "Web Frontends"
order = 2
depends_on = ["backend"]

[projects.api_srv]
name = "API Service"
group = "backend"
root = "{root}"
port = 8080
start = ["bash", "-c", "echo start api"]
stop = ["bash", "-c", "echo stop api"]
health_url = "http://127.0.0.1:8080/health"
login_url = "http://127.0.0.1:8080/login"
background = true
start_timeout_secs = 90
stop_grace_secs = 12
auto_install = false
port_mode = "arg"
port_arg = "--port"
port_env = "PORT"
pty = true
env = {{ ENV_MODE = "production", DB_NAME = "demo_db" }}

[[projects.api_srv.auto_answers]]
expect = "Ready?"
send = "yes"
once = true
regex = false

[[projects.api_srv.auto_answers]]
expect = "Pattern.*"
send = "ack"
once = false
regex = true

[[projects.api_srv.links]]
title = "API Docs"
url = "http://127.0.0.1:8080/docs"

[projects.api_srv.profiles.dev]
label = "Development Profile"
start = "pnpm dev"
port = 8081
env = {{ PROFILE = "dev" }}

[projects.api_srv.profiles.prod]
label = "Production Profile"
port = 8080

[projects.web_ui]
name = "Web UI"
group = "frontend"
root = "{root}"
port = 3000
start = ["pnpm", "dev"]
default_profile = "local"
depends_on = ["api_srv"]

[projects.web_ui.profiles.local]
label = "Local Dev"
start = "pnpm dev --local"
port = 3000
"#,
        log_dir = path_str(&temp.path().join("logs")),
        root = path_str(&app_root),
    );
    std::fs::write(&cfg_file, &toml).unwrap();

    let loaded = load(Some(&cfg_file)).expect("valid toml should parse successfully");
    let list = loaded.list().unwrap();
    assert_eq!(list.projects.len(), 2);
    assert_eq!(list.groups.len(), 2);

    // Groups check
    let grp_be = list.groups.iter().find(|g| g.key == "backend").unwrap();
    assert_eq!(grp_be.label, "Backend Services");
    assert_eq!(grp_be.order, Some(1));

    let grp_fe = list.groups.iter().find(|g| g.key == "frontend").unwrap();
    assert_eq!(grp_fe.label, "Web Frontends");
    assert_eq!(grp_fe.order, Some(2));
    assert_eq!(grp_fe.depends_on, vec!["backend"]);

    // Projects check: api_srv
    let api = list.projects.iter().find(|p| p.id == "api_srv").unwrap();
    assert_eq!(api.name, "API Service");
    assert_eq!(api.group.as_deref(), Some("backend"));
    assert_eq!(api.port, 8080);
    assert_eq!(api.start_timeout_secs, Some(90));
    assert_eq!(api.stop_grace_secs, Some(12));
    assert!(!api.auto_install);
    assert_eq!(api.port_mode, "arg");
    assert_eq!(api.port_arg.as_deref(), Some("--port"));
    assert_eq!(api.port_env.as_deref(), Some("PORT"));
    assert!(api.pty);
    assert_eq!(api.env.get("ENV_MODE").map(String::as_str), Some("production"));
    assert_eq!(api.auto_answers.len(), 2);
    assert_eq!(api.auto_answers[0].expect, "Ready?");
    assert_eq!(api.auto_answers[0].send, "yes");
    assert!(api.auto_answers[0].once);
    assert!(!api.auto_answers[0].regex);
    assert_eq!(api.auto_answers[1].expect, "Pattern.*");
    assert!(!api.auto_answers[1].once);
    assert!(api.auto_answers[1].regex);
    assert_eq!(api.links.len(), 1);
    assert_eq!(api.links[0].title, "API Docs");
    assert_eq!(api.profiles.len(), 2);

    // Projects check: web_ui
    let web = list.projects.iter().find(|p| p.id == "web_ui").unwrap();
    assert_eq!(web.default_profile.as_deref(), Some("local"));
    assert_eq!(web.depends_on, vec!["api_srv"]);

    // Group memberships
    let be_members = loaded.projects_in_group("backend");
    assert!(be_members.contains(&"api_srv".to_string()));
    let fe_members = loaded.projects_in_group("frontend");
    assert!(fe_members.contains(&"web_ui".to_string()));
}

#[test]
fn test_config_parsing_defaults() {
    let temp = TempDir::new("cfg_defaults");
    let cfg_file = temp.path().join("projects.toml");
    let app_root = temp.path().join("min_app");
    std::fs::create_dir_all(&app_root).unwrap();

    let toml = format!(
        r#"
[projects.minimal]
root = "{root}"
port = 5000
start = ["echo", "minimal"]
"#,
        root = path_str(&app_root)
    );
    std::fs::write(&cfg_file, &toml).unwrap();

    let loaded = load(Some(&cfg_file)).unwrap();
    let detail = loaded.project_detail("minimal").unwrap();
    assert_eq!(detail.name, "minimal", "name defaults to id when omitted");
    assert_eq!(detail.group, None);
    assert!(detail.enabled, "enabled defaults to true");
    assert_eq!(detail.reason, "", "reason defaults to empty string");
    assert!(detail.auto_install, "auto_install defaults to true");
    assert_eq!(detail.port_mode, "none", "port_mode defaults to none");
    assert_eq!(detail.port_arg, None);
    assert_eq!(detail.port_env, None);
    assert!(!detail.pty, "pty defaults to false");
    assert!(detail.env.is_empty());
    assert!(detail.auto_answers.is_empty());
    assert!(detail.links.is_empty());
    assert_eq!(detail.default_profile, None);
    assert!(detail.profiles.is_empty());
    assert!(detail.depends_on.is_empty());
}

#[test]
fn test_config_parsing_syntax_error_and_invalid_types() {
    let temp = TempDir::new("cfg_err");

    // 1. Invalid TOML syntax
    let bad_syntax = temp.path().join("bad_syntax.toml");
    std::fs::write(&bad_syntax, "projects = [invalid syntax").unwrap();
    let err = load(Some(&bad_syntax)).unwrap_err();
    assert!(err.to_string().contains("parse projects.toml"));

    // 2. Missing required root field
    let missing_root = temp.path().join("missing_root.toml");
    std::fs::write(
        &missing_root,
        r#"
[projects.no_root]
port = 8080
start = ["echo"]
"#,
    )
    .unwrap();
    let err2 = load(Some(&missing_root)).unwrap_err();
    assert!(err2.to_string().contains("parse projects.toml"));

    // 3. Invalid port type (string instead of integer)
    let bad_port_type = temp.path().join("bad_port.toml");
    std::fs::write(
        &bad_port_type,
        r#"
[projects.bad_port]
root = "/tmp"
port = "eighty-eighty"
start = ["echo"]
"#,
    )
    .unwrap();
    let err3 = load(Some(&bad_port_type)).unwrap_err();
    assert!(err3.to_string().contains("parse projects.toml"));
}

// ---------------------------------------------------------------------------
// 2. add_project & remove_project with toml_edit on temp dirs
// ---------------------------------------------------------------------------

#[test]
fn test_add_project_preserves_comments_and_creates_group_with_toml_edit() {
    let temp = TempDir::new("add_toml_edit");
    let cfg_file = temp.path().join("projects.toml");
    let base_root = temp.path().join("base_proj");
    let new_root = temp.path().join("added_proj");
    std::fs::create_dir_all(&base_root).unwrap();
    std::fs::create_dir_all(&new_root).unwrap();

    let initial_toml = format!(
        r#"# Header comment preserving test
# Author notes

[projects.base_app]
name = "Base Application"
root = "{base_root}"
port = 7000
start = ["bash", "-c", "echo base"]
"#,
        base_root = path_str(&base_root)
    );
    std::fs::write(&cfg_file, &initial_toml).unwrap();

    let mut loaded = load(Some(&cfg_file)).unwrap();
    assert_eq!(loaded.list().unwrap().projects.len(), 1);

    // Add project with a new group
    let req = NewProject {
        id: "added_app".to_string(),
        name: Some("Added App".to_string()),
        group: Some("services".to_string()),
        root: path_str(&new_root),
        start: "pnpm dev".to_string(),
        port: Some(7001),
        ..Default::default()
    };
    let row = loaded.add_project(req).unwrap();
    assert_eq!(row.id, "added_app");
    assert_eq!(row.port, 7001);
    assert_eq!(row.group.as_deref(), Some("services"));

    // Verify toml_edit preserved comments and formatting
    let content = std::fs::read_to_string(&cfg_file).unwrap();
    assert!(content.contains("# Header comment preserving test"));
    assert!(content.contains("# Author notes"));
    assert!(content.contains("[projects.base_app]"));
    assert!(content.contains("[projects.added_app]"));
    assert!(content.contains("[group.services]"), "group table should be created in toml");

    let doc: DocumentMut = content.parse().expect("toml_edit document must be valid");
    assert!(doc.as_table().contains_key("projects"));
    assert!(doc.as_table().contains_key("group"));

    // Reload from disk to verify full persistence
    let reloaded = load(Some(&cfg_file)).unwrap();
    let projects = reloaded.list().unwrap().projects;
    assert_eq!(projects.len(), 2);
    let added = projects.iter().find(|p| p.id == "added_app").unwrap();
    assert_eq!(added.port, 7001);
    assert_eq!(added.group.as_deref(), Some("services"));
}

#[test]
#[test]
fn test_add_project_validation_rejects_without_modifying_file() {
    let temp = TempDir::new("add_validation");
    let cfg_file = temp.path().join("projects.toml");
    let valid_root = temp.path().join("valid_app");
    std::fs::create_dir_all(&valid_root).unwrap();

    let initial_toml = format!(
        r#"[projects.first]
name = "First"
root = "{root}"
port = 8000
start = ["echo", "1"]
"#,
        root = path_str(&valid_root)
    );
    std::fs::write(&cfg_file, &initial_toml).unwrap();
    let mut loaded = load(Some(&cfg_file)).unwrap();

    // 1. Duplicate ID
    let dup_req = NewProject {
        id: "first".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        port: Some(8001),
        ..Default::default()
    };
    let err_dup = loaded.add_project(dup_req).unwrap_err();
    assert!(err_dup.to_string().contains("already exists"));

    // 2. Invalid ID with spaces
    let bad_id = NewProject {
        id: "bad id with space".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        port: Some(8002),
        ..Default::default()
    };
    assert!(loaded.add_project(bad_id).is_err());

    // 3. Empty root
    let empty_root = NewProject {
        id: "no_root".to_string(),
        root: "".to_string(),
        start: "echo".to_string(),
        port: Some(8003),
        ..Default::default()
    };
    assert!(loaded.add_project(empty_root).is_err());

    // 4. Empty start command
    let empty_start = NewProject {
        id: "no_start".to_string(),
        root: path_str(&valid_root),
        start: "".to_string(),
        port: Some(8004),
        ..Default::default()
    };
    assert!(loaded.add_project(empty_start).is_err());

    // 5. Port 0
    let zero_port = NewProject {
        id: "zero_port".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        port: Some(0),
        ..Default::default()
    };
    assert!(loaded.add_project(zero_port).is_err());

    // 6. Disabled without reason
    let disabled_no_reason = NewProject {
        id: "disabled_no_reason".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        port: Some(8005),
        enabled: Some(false),
        reason: None,
        ..Default::default()
    };
    assert!(loaded.add_project(disabled_no_reason).is_err());

    // 7. Invalid link URL (not http/https)
    let bad_link = NewProject {
        id: "bad_link".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        port: Some(8006),
        links: vec![WebLink {
            title: "Bad".to_string(),
            url: "ftp://example.com".to_string(),
        }],
        ..Default::default()
    };
    assert!(loaded.add_project(bad_link).is_err());

    // 8. Invalid shell name
    let bad_shell = NewProject {
        id: "bad_shell".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        shell: Some("zsh_unsupported".to_string()),
        ..Default::default()
    };
    assert!(loaded.add_project(bad_shell).is_err());

    // 9. Invalid port_mode
    let bad_port_mode = NewProject {
        id: "bad_mode".to_string(),
        root: path_str(&valid_root),
        start: "echo".to_string(),
        port_mode: Some("invalid_mode".to_string()),
        ..Default::default()
    };
    assert!(loaded.add_project(bad_port_mode).is_err());

    // Verify file content is completely unchanged
    let current_content = std::fs::read_to_string(&cfg_file).unwrap();
    assert_eq!(current_content, initial_toml);
}

#[test]
#[test]
fn test_validate_new_project_method() {
    let temp = TempDir::new("validate_method");
    let cfg_file = temp.path().join("projects.toml");
    let root = temp.path().join("app");
    std::fs::create_dir_all(&root).unwrap();

    std::fs::write(
        &cfg_file,
        format!(
            r#"[projects.app_1]
root = "{}"
port = 8080
start = ["echo"]
"#,
            path_str(&root)
        ),
    )
    .unwrap();
    let loaded = load(Some(&cfg_file)).unwrap();

    // Valid project validation passes
    let valid_req = NewProject {
        id: "app_2".to_string(),
        root: path_str(&root),
        start: "echo hi".to_string(),
        port: Some(8081),
        ..Default::default()
    };
    assert!(loaded.validate_new_project(&valid_req).is_ok());

    // Duplicate project validation fails
    let dup_req = NewProject {
        id: "app_1".to_string(),
        root: path_str(&root),
        start: "echo hi".to_string(),
        port: Some(8082),
        ..Default::default()
    };
    let err = loaded.validate_new_project(&dup_req).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

#[test]
fn test_check_depends_on_cycle() {
    let temp = TempDir::new("dep_cycle");
    let cfg_file = temp.path().join("projects.toml");
    let root = temp.path().join("app");
    std::fs::create_dir_all(&root).unwrap();

    let toml = format!(
        r#"
[projects.srv_a]
root = "{r}"
port = 8001
start = ["echo"]
depends_on = ["srv_b"]

[projects.srv_b]
root = "{r}"
port = 8002
start = ["echo"]
depends_on = ["srv_c"]

[projects.srv_c]
root = "{r}"
port = 8003
start = ["echo"]
depends_on = []
"#,
        r = path_str(&root)
    );
    std::fs::write(&cfg_file, &toml).unwrap();
    let loaded = load(Some(&cfg_file)).unwrap();

    // 1. Self dependency
    let err_self = loaded
        .check_depends_on_cycle("srv_c", &["srv_c".to_string()])
        .unwrap_err();
    assert!(err_self.to_string().contains("检测到循环依赖"));

    // 2. Transitive cycle: srv_c depending on srv_a creates srv_a -> srv_b -> srv_c -> srv_a
    let err_cycle = loaded
        .check_depends_on_cycle("srv_c", &["srv_a".to_string()])
        .unwrap_err();
    assert!(err_cycle.to_string().contains("检测到循环依赖"));

    // 3. Valid dependency (no cycle)
    let ok = loaded.check_depends_on_cycle("srv_c", &[]);
    assert!(ok.is_ok());
}

// ---------------------------------------------------------------------------
// 3. Pure Functions: derive_state
// ---------------------------------------------------------------------------

#[test]
fn test_derive_state_all_branches() {
    // 1. start_error takes precedence -> Failed
    let (st, err) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: true,
        started_at: Some(100),
        now: 105,
        timeout_secs: 60,
        start_error: Some("command not found".to_string()),
    });
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("command not found"));

    // 2. Port conflict masks port open
    let (st, _) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: true, // conflict!
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Starting);

    // 3. Effective port open + no probe -> Ready
    let (st, _) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Ready);

    // 4. Effective port open + probe ok -> Ready
    let (st, _) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: true,
        started_at: Some(100),
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Ready);

    // 5. Port open + probe not ok (has started_at) -> PortOpen
    let (st, _) = derive_state(StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: false,
        started_at: Some(100),
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::PortOpen);

    // 6. Port open + probe not ok (no started_at, manual running) -> PortOpen
    let (st, _) = derive_state(StateInput {
        process_alive: false,
        port_open: true,
        port_conflict: false,
        has_probe: true,
        probe_ok: false,
        started_at: None,
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::PortOpen);

    // 7. Process exited after grace period (>= 3s) -> Failed
    let (st, err) = derive_state(StateInput {
        process_alive: false,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 104, // 4s >= 3s grace
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("进程已退出"));

    // 8. Process not alive within grace period (< 3s) -> Starting (grace period)
    let (st, _) = derive_state(StateInput {
        process_alive: false,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 101, // 1s < 3s grace
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Starting);

    // 9. Timeout exceeded -> Failed
    let (st, err) = derive_state(StateInput {
        process_alive: true,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: Some(100),
        now: 200, // elapsed 100 >= timeout 60
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Failed);
    assert!(err.unwrap().contains("超过 60 秒仍未就绪"));

    // 10. Manual run without started_at, process alive -> Starting
    let (st, _) = derive_state(StateInput {
        process_alive: true,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: None,
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Starting);

    // 11. Fully stopped
    let (st, _) = derive_state(StateInput {
        process_alive: false,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: None,
        now: 105,
        timeout_secs: 60,
        start_error: None,
    });
    assert_eq!(st, LifeState::Stopped);
}

// ---------------------------------------------------------------------------
// 4. Pure Functions: resolve_input_data & map_key
// ---------------------------------------------------------------------------

#[test]
fn test_map_key_all_cases() {
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

    // Case-insensitivity
    assert_eq!(map_key("UP"), Some("\x1b[A"));
    assert_eq!(map_key("Enter"), Some("\n"));
    assert_eq!(map_key("Ctrl-C"), Some("\x03"));

    // Unmapped keys
    assert_eq!(map_key("backspace"), None);
    assert_eq!(map_key("a"), None);
    assert_eq!(map_key(""), None);
}

#[test]
fn test_resolve_input_data() {
    // Text with enter
    assert_eq!(
        resolve_input_data(Some("pnpm dev"), true, None).unwrap(),
        "pnpm dev\n"
    );

    // Text without enter
    assert_eq!(
        resolve_input_data(Some("pnpm dev"), false, None).unwrap(),
        "pnpm dev"
    );

    // Enter only
    assert_eq!(resolve_input_data(None, true, None).unwrap(), "\n");

    // Key only
    assert_eq!(
        resolve_input_data(None, false, Some("up")).unwrap(),
        "\x1b[A"
    );

    // Text + key (enter is ignored when key is provided)
    assert_eq!(
        resolve_input_data(Some("select"), true, Some("enter")).unwrap(),
        "select\n"
    );

    // Empty input error
    let err_empty = resolve_input_data(None, false, None).unwrap_err();
    assert!(err_empty.to_string().contains("empty input"));

    // Unknown key error
    let err_key = resolve_input_data(None, false, Some("invalid_key")).unwrap_err();
    assert!(err_key.to_string().contains("未知按键"));
}

// ---------------------------------------------------------------------------
// 5. Pure Functions: validate_group_key & generate_group_key
// ---------------------------------------------------------------------------

#[test]
fn test_validate_group_key() {
    assert!(validate_group_key("api").is_ok());
    assert!(validate_group_key("team-frontend").is_ok());
    assert!(validate_group_key("web_v2-prod").is_ok());
    assert!(validate_group_key("123").is_ok());

    // Empty
    assert!(validate_group_key("").is_err());

    // >40 characters
    let long_key = "a".repeat(41);
    assert!(validate_group_key(&long_key).is_err());
    let max_len_key = "a".repeat(40);
    assert!(validate_group_key(&max_len_key).is_ok());

    // Invalid characters
    assert!(validate_group_key("has space").is_err());
    assert!(validate_group_key("UpperCase").is_err());
    assert!(validate_group_key("special@key").is_err());
    assert!(validate_group_key("中文").is_err());
}

#[test]
fn test_generate_group_key() {
    // ASCII labels
    assert_eq!(generate_group_key("Backend Team", &[] as &[&str]), "backend-team");
    assert_eq!(generate_group_key("API_v2", &[] as &[&str]), "api_v2");
    assert_eq!(
        generate_group_key("   --hello..world--   ", &[] as &[&str]),
        "hello-world"
    );

    // Non-ASCII labels
    assert_eq!(generate_group_key("移动端开发", &[] as &[&str]), "g1");
    assert_eq!(generate_group_key("移动端开发", &["g1"]), "g2");
    assert_eq!(generate_group_key("移动端开发", &["g1", "g2"]), "g3");

    // Collision avoidance for ASCII
    assert_eq!(generate_group_key("demo", &["demo"]), "g1");
    assert_eq!(generate_group_key("demo", &["demo", "g1"]), "g2");

    // Empty label fallback
    assert_eq!(generate_group_key("", &[] as &[&str]), "g1");
}

// ---------------------------------------------------------------------------
// 6. Helpers & Pure Types: default_port_mode, LifeState, unresolved_status
// ---------------------------------------------------------------------------

#[test]
fn test_default_port_mode_and_lifestate_display() {
    assert_eq!(default_port_mode(), "none");

    assert_eq!(format!("{}", LifeState::Stopped), "stopped");
    assert_eq!(format!("{}", LifeState::Installing), "installing");
    assert_eq!(format!("{}", LifeState::Starting), "starting");
    assert_eq!(format!("{}", LifeState::PortOpen), "port_open");
    assert_eq!(format!("{}", LifeState::Ready), "ready");
    assert_eq!(format!("{}", LifeState::Failed), "failed");
}

#[test]
fn test_unresolved_status_construction() {
    let row = ProjectRow {
        id: "broken".to_string(),
        name: "Broken App".to_string(),
        group: None,
        root: "/tmp/broken".to_string(),
        enabled: true,
        port: 9090,
        reason: "".to_string(),
        configured: false,
        start_timeout_secs: None,
        stop_grace_secs: None,
        node: None,
        java: None,
        java_home: None,
        node_home: None,
        shell: None,
        auto_install: true,
        port_mode: "none".to_string(),
        port_arg: None,
        port_env: None,
        env: BTreeMap::new(),
        pty: false,
        auto_answers: Vec::new(),
        links: Vec::new(),
        default_profile: None,
        profiles: BTreeMap::new(),
        depends_on: Vec::new(),
        resolve_error: Some("log_dir resolution failed".to_string()),
    };

    let st = unresolved_status(&row, "bad config");
    assert_eq!(st.id, "broken");
    assert_eq!(st.name, "Broken App");
    assert!(!st.running);
    assert_eq!(st.state, LifeState::Stopped);
    assert_eq!(st.port, 9090);
    assert_eq!(st.failure.as_deref(), Some("bad config"));
    assert_eq!(st.resolve_error.as_deref(), Some("log_dir resolution failed"));
}

#[test]
fn test_normalize_start_arg_pnpm_rules() {
    // pnpm with extra double dash is cleaned
    assert_eq!(
        normalize_start_arg("pnpm dev -- --port 3000"),
        "pnpm dev --port 3000"
    );
    assert_eq!(
        normalize_start_arg("pnpm run dev -- --host 0.0.0.0"),
        "pnpm run dev --host 0.0.0.0"
    );

    // npm double dash is left untouched
    assert_eq!(
        normalize_start_arg("npm run dev -- --port 3000"),
        "npm run dev -- --port 3000"
    );

    // Plain commands left untouched
    assert_eq!(normalize_start_arg("mvn spring-boot:run"), "mvn spring-boot:run");
}
