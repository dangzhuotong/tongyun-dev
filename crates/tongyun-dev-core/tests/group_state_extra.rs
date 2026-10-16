use std::path::{Path, PathBuf};

use tongyun_dev_core::{
    derive_state, generate_group_key, load, validate_group_key, LifeState, StateInput,
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
        let path = std::env::temp_dir().join(format!("ty_group_state_{tag}_{pid}_{id}"));
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
// validate_group_key: failure message cases and accepted boundary shapes
// ---------------------------------------------------------------------------

#[test]
fn test_validate_group_key_error_messages() {
    // Empty key message.
    let err = validate_group_key("").unwrap_err();
    assert_eq!(err.to_string(), "分组 key 不能为空");

    // Too long (41 ASCII bytes) message.
    let too_long = "a".repeat(41);
    let err = validate_group_key(&too_long).unwrap_err();
    assert_eq!(err.to_string(), "分组 key 长度不能超过 40 个字符");

    // Invalid character message echoes the offending key.
    let err = validate_group_key("has space").unwrap_err();
    assert_eq!(
        err.to_string(),
        "分组 key 只能包含小写字母、数字、下划线和连字符: `has space`"
    );

    let err = validate_group_key("UPPER").unwrap_err();
    assert!(err.to_string().contains("`UPPER`"));

    // Length is measured in bytes, and the length check runs before the
    // character check: 14 CJK chars = 42 bytes -> length error.
    let multibyte_long = "中".repeat(14);
    assert_eq!(multibyte_long.chars().count(), 14);
    assert!(multibyte_long.len() > 40);
    let err = validate_group_key(&multibyte_long).unwrap_err();
    assert_eq!(err.to_string(), "分组 key 长度不能超过 40 个字符");

    // 13 CJK chars = 39 bytes pass the length check but fail the char check.
    let multibyte_short = "中".repeat(13);
    assert!(multibyte_short.len() <= 40);
    let err = validate_group_key(&multibyte_short).unwrap_err();
    assert!(err.to_string().contains("只能包含小写字母"));

    // 40 ASCII bytes is the accepted maximum.
    assert!(validate_group_key(&"z".repeat(40)).is_ok());
}

#[test]
fn test_validate_group_key_accepted_shapes() {
    // A key consisting only of separators is still accepted by the current rules.
    assert!(validate_group_key("-").is_ok());
    assert!(validate_group_key("_").is_ok());
    assert!(validate_group_key("--").is_ok());
    assert!(validate_group_key("-_-").is_ok());
    // Digits only.
    assert!(validate_group_key("007").is_ok());
    // Mixed accepted characters.
    assert!(validate_group_key("a-1_b").is_ok());
}

// ---------------------------------------------------------------------------
// generate_group_key: sanitization / truncation / fallback edges
// ---------------------------------------------------------------------------

#[test]
fn test_generate_group_key_truncation_and_collapse() {
    // Consecutive illegal characters collapse into a single hyphen.
    assert_eq!(generate_group_key("a!!!b", &[] as &[&str]), "a-b");
    assert_eq!(generate_group_key("A...B", &[] as &[&str]), "a-b");

    // Over-length input is truncated to 40 chars.
    assert_eq!(
        generate_group_key(&"a".repeat(45), &[] as &[&str]),
        "a".repeat(40)
    );

    // Truncation can leave a trailing hyphen, which is then trimmed:
    // 39 * "a" + "-" + "b" = 41 chars -> truncate to 40 = 39 * "a" + "-".
    let label = format!("{}b", format!("{}-", "a".repeat(39)));
    assert_eq!(generate_group_key(&label, &[] as &[&str]), "a".repeat(39));

    // 40-char candidate is kept as-is.
    assert_eq!(
        generate_group_key(&"b".repeat(40), &[] as &[&str]),
        "b".repeat(40)
    );

    // Mixed-case input is lowercased while keeping digits/underscore/hyphen.
    assert_eq!(
        generate_group_key("UPPER_case-99", &[] as &[&str]),
        "upper_case-99"
    );

    // Dots and other punctuation become hyphens (collapsed / trimmed).
    assert_eq!(generate_group_key("Foo.Bar", &[] as &[&str]), "foo-bar");
}

#[test]
fn test_generate_group_key_fallbacks() {
    // Separator-only and whitespace-only labels fall back to gN.
    assert_eq!(generate_group_key("---", &[] as &[&str]), "g1");
    assert_eq!(generate_group_key("...", &[] as &[&str]), "g1");
    assert_eq!(generate_group_key("   ", &[] as &[&str]), "g1");
    assert_eq!(generate_group_key("\t\n", &[] as &[&str]), "g1");

    // Fallback skips existing keys.
    assert_eq!(generate_group_key("---", &["g1"]), "g2");
    assert_eq!(generate_group_key("", &["g1"]), "g2");

    // Non-ASCII labels always take the fallback path.
    assert_eq!(generate_group_key("中文", &[] as &[&str]), "g1");
    assert_eq!(generate_group_key("中文", &["g1"]), "g2");
    assert_eq!(generate_group_key("中文", &["g1", "g2"]), "g3");

    // A sanitized ASCII label that collides with an existing key falls back.
    assert_eq!(generate_group_key("Foo.Bar", &["foo-bar"]), "g1");
    assert_eq!(generate_group_key("Foo.Bar", &["foo-bar", "g1"]), "g2");

    // Duplicate keys in the existing set do not change the result.
    assert_eq!(generate_group_key("demo", &["demo", "demo"]), "g1");
}

// ---------------------------------------------------------------------------
// Loaded::groups() listing
// ---------------------------------------------------------------------------

#[test]
fn test_groups_listing_declared_implicit_and_order() {
    let temp = TempDir::new("groups");
    let cfg_file = temp.path().join("projects.toml");
    let root = temp.path().join("app");
    std::fs::create_dir_all(&root).unwrap();

    let toml = format!(
        r#"
log_dir = "{log_dir}"

[group.alpha]
label = "   "
order = 2
members = ["p_declared", "p_missing"]
depends_on = ["beta"]

[group.beta]
label = "Beta Team"
order = 1

[group.ordered]
order = 3
members = ["z_proj"]

[group.gamma]

[projects.p_declared]
root = "{root}"
port = 1001
start = ["echo"]
group = "alpha"

[projects.p_beta]
root = "{root}"
port = 1002
start = ["echo"]
group = "beta"

[projects.z_proj]
root = "{root}"
port = 1003
start = ["echo"]
group = "ordered"

[projects.a_proj]
root = "{root}"
port = 1004
start = ["echo"]
group = "ordered"

[projects.p_implicit]
root = "{root}"
port = 1005
start = ["echo"]
group = "implicit"
"#,
        log_dir = path_str(&temp.path().join("logs")),
        root = path_str(&root),
    );
    std::fs::write(&cfg_file, &toml).unwrap();

    let loaded = load(Some(&cfg_file)).expect("config should parse");
    let groups = loaded.groups();

    // Ordering: Some(order) ascending first, then None by label/key.
    let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
    assert_eq!(keys, vec!["beta", "alpha", "ordered", "gamma", "implicit"]);

    // Whitspace-only label falls back to the key.
    let alpha = groups.iter().find(|g| g.key == "alpha").unwrap();
    assert_eq!(alpha.label, "alpha");
    assert_eq!(alpha.order, Some(2));
    assert_eq!(alpha.depends_on, vec!["beta"]);
    // Declared members that are not registered projects are dropped.
    assert_eq!(alpha.members, vec!["p_declared"]);

    // A declared label is used verbatim; members come from `project.group`.
    let beta = groups.iter().find(|g| g.key == "beta").unwrap();
    assert_eq!(beta.label, "Beta Team");
    assert_eq!(beta.order, Some(1));
    assert!(beta.depends_on.is_empty());
    assert_eq!(beta.members, vec!["p_beta"]);

    // Explicit member order is preserved, remaining members are sorted after.
    let ordered = groups.iter().find(|g| g.key == "ordered").unwrap();
    assert_eq!(ordered.label, "ordered");
    assert_eq!(ordered.order, Some(3));
    assert_eq!(ordered.members, vec!["z_proj", "a_proj"]);

    // Declared group without a label/order/members.
    let gamma = groups.iter().find(|g| g.key == "gamma").unwrap();
    assert_eq!(gamma.label, "gamma");
    assert_eq!(gamma.order, None);
    assert!(gamma.members.is_empty());
    assert!(gamma.depends_on.is_empty());

    // Implicit group: only referenced by `project.group`, never declared.
    let implicit = groups.iter().find(|g| g.key == "implicit").unwrap();
    assert_eq!(implicit.label, "implicit");
    assert_eq!(implicit.order, None);
    assert_eq!(implicit.members, vec!["p_implicit"]);
    assert!(implicit.depends_on.is_empty());

    // Cross-checks on membership helpers.
    assert!(loaded.contains_group("implicit"));
    assert!(loaded.contains_group("beta"));
    assert!(!loaded.contains_group("does_not_exist"));
    assert!(loaded.contains_project("p_declared"));
    assert_eq!(
        loaded.projects_in_group("ordered"),
        vec!["z_proj".to_string(), "a_proj".to_string()]
    );
    // A declared group with no members resolves to an empty list.
    assert!(loaded.projects_in_group("gamma").is_empty());
    // Unknown group resolves to an empty list.
    assert!(loaded.projects_in_group("nope").is_empty());
}

#[test]
fn test_groups_listing_sorted_by_label_when_no_order() {
    let temp = TempDir::new("groups_label");
    let cfg_file = temp.path().join("projects.toml");
    let root = temp.path().join("app");
    std::fs::create_dir_all(&root).unwrap();

    let toml = format!(
        r#"
[group.zzz]
label = "Alpha Label"

[group.aaa]
label = "Zeta Label"

[projects.p1]
root = "{root}"
port = 2001
start = ["echo"]
group = "zzz"
"#,
        root = path_str(&root),
    );
    std::fs::write(&cfg_file, &toml).unwrap();

    let loaded = load(Some(&cfg_file)).expect("config should parse");
    let groups = loaded.groups();
    let keys: Vec<&str> = groups.iter().map(|g| g.key.as_str()).collect();
    // Both have order None: sorted by label, not key.
    assert_eq!(keys, vec!["zzz", "aaa"]);
}

// ---------------------------------------------------------------------------
// derive_state: failure message cases
// ---------------------------------------------------------------------------

fn state_input() -> StateInput {
    StateInput {
        process_alive: true,
        port_open: false,
        port_conflict: false,
        has_probe: false,
        probe_ok: false,
        started_at: None,
        now: 0,
        timeout_secs: 60,
        start_error: None,
    }
}

#[test]
fn test_derive_state_exact_failure_messages() {
    // Exact timeout message.
    let input = StateInput {
        started_at: Some(100),
        now: 105,
        timeout_secs: 5,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("超过 5 秒仍未就绪"));

    // Timeout of 0 fails immediately.
    let input = StateInput {
        started_at: Some(100),
        now: 100,
        timeout_secs: 0,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("超过 0 秒仍未就绪"));

    // `now` before `started_at` saturates elapsed to 0, so timeout 0 still fails.
    let input = StateInput {
        started_at: Some(100),
        now: 50,
        timeout_secs: 0,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("超过 0 秒仍未就绪"));

    // An empty start_error is still surfaced verbatim.
    let input = StateInput {
        start_error: Some(String::new()),
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some(""));
}

#[test]
fn test_derive_state_process_exit_grace_boundary() {
    // Exactly at the grace boundary (3s) the process is considered exited.
    let input = StateInput {
        process_alive: false,
        started_at: Some(100),
        now: 103,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("进程已退出"));

    // One second before the boundary -> still Starting.
    let input = StateInput {
        process_alive: false,
        started_at: Some(100),
        now: 102,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Starting);
    assert_eq!(err, None);

    // Process-exit failure wins over the timeout message when both apply.
    let input = StateInput {
        process_alive: false,
        started_at: Some(100),
        now: 200,
        timeout_secs: 5,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Failed);
    assert_eq!(err.as_deref(), Some("进程已退出"));
}

#[test]
fn test_derive_state_port_conflict_without_start_marker() {
    // Port open but owned by another process, no start marker -> Starting
    // because the effective port is not considered open.
    let input = StateInput {
        process_alive: true,
        port_open: true,
        port_conflict: true,
        started_at: None,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Starting);
    assert_eq!(err, None);

    // Same conflict with the process not alive and no start marker -> Stopped.
    let input = StateInput {
        process_alive: false,
        port_open: true,
        port_conflict: true,
        started_at: None,
        ..state_input()
    };
    let (st, err) = derive_state(input);
    assert_eq!(st, LifeState::Stopped);
    assert_eq!(err, None);
}
