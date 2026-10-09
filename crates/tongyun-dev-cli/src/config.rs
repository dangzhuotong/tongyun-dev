//! `projects.toml`: parse, environment overrides, `~` and template expansion.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    pub root: String,
    pub start: Vec<String>,
    pub stop: Vec<String>,
    pub tomcat: String,
    pub log_dir: String,
    pub log_file: String,
    pub pid_file: String,
    pub port: u16,
    pub login_url: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FileConfig {
    pub projects: BTreeMap<String, Project>,
}

/// Paths and argv after `~` and `{{root}}` / `{{tomcat}}` / `{{log_dir}}` expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProject {
    pub id: String,
    pub name: String,
    pub root: PathBuf,
    pub tomcat: PathBuf,
    pub log_dir: PathBuf,
    pub log_file: PathBuf,
    pub pid_file: PathBuf,
    pub port: u16,
    pub login_url: String,
    pub start: Vec<String>,
    pub stop: Vec<String>,
}

pub fn parse_config(text: &str) -> Result<FileConfig> {
    let cfg: FileConfig = toml::from_str(text).context("parse projects.toml")?;
    if cfg.projects.is_empty() {
        bail!("projects.toml has no [projects.*] entries");
    }
    for (id, project) in &cfg.projects {
        if id.is_empty() {
            bail!("project id is empty");
        }
        if project.name.is_empty() {
            bail!("project `{id}` is missing name");
        }
        if project.start.is_empty() {
            bail!("project `{id}` has an empty start command");
        }
        if project.stop.is_empty() {
            bail!("project `{id}` has an empty stop command");
        }
        if project.start.iter().any(|part| part.is_empty())
            || project.stop.iter().any(|part| part.is_empty())
        {
            bail!("project `{id}` has an empty command argument");
        }
        if project.login_url.is_empty() {
            bail!("project `{id}` is missing login_url");
        }
    }
    Ok(cfg)
}

pub fn load_config_file(path: &Path) -> Result<FileConfig> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse_config(&text)
}

/// `--config`, then `TONGYUN_DEV_CONFIG`, then walk from `cwd` up to `config/projects.toml`.
pub fn resolve_config_path(
    explicit: Option<&Path>,
    env_value: Option<&str>,
    cwd: &Path,
) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if !path.is_file() {
            bail!("config file not found: {}", path.display());
        }
        return Ok(path.to_path_buf());
    }
    if let Some(raw) = env_value.map(str::trim).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(raw);
        if !path.is_file() {
            bail!("config file not found: {}", path.display());
        }
        return Ok(path);
    }
    let mut cur = cwd.to_path_buf();
    loop {
        let candidate = cur.join("config/projects.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
        if !cur.pop() {
            bail!("config/projects.toml not found; pass --config or set TONGYUN_DEV_CONFIG");
        }
    }
}

/// Override `root` / `tomcat` / `log_dir` from `TONGYUN_DEV_<ID>_ROOT|_TOMCAT|_LOG_DIR`.
///
/// `<ID>` is the project id with `-` turned into `_` and ASCII uppercased.
/// Empty or whitespace-only values are ignored. Values are not logged.
pub fn apply_env_overrides(cfg: &mut FileConfig, env: &[(String, String)]) {
    for (id, project) in &mut cfg.projects {
        let key = env_id(id);
        if let Some(value) = env_get(env, &format!("TONGYUN_DEV_{key}_ROOT")) {
            project.root = value;
        }
        if let Some(value) = env_get(env, &format!("TONGYUN_DEV_{key}_TOMCAT")) {
            project.tomcat = value;
        }
        if let Some(value) = env_get(env, &format!("TONGYUN_DEV_{key}_LOG_DIR")) {
            project.log_dir = value;
        }
    }
}

fn env_id(id: &str) -> String {
    id.replace('-', "_").to_ascii_uppercase()
}

fn env_get(env: &[(String, String)], key: &str) -> Option<String> {
    env.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Expand a leading `~` or `~/` against `home`. `~other` is left unchanged.
pub fn expand_tilde(input: &str, home: &Path) -> PathBuf {
    if input == "~" || input == "~/" {
        home.to_path_buf()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(input)
    }
}

fn expand_tilde_str(input: &str, home: &Path) -> String {
    if input == "~" || input.starts_with("~/") {
        expand_tilde(input, home).display().to_string()
    } else {
        input.to_string()
    }
}

/// Insert template values once. Text that arrives inside a value is not scanned again.
fn substitute(input: &str, root: &str, tomcat: &str, log_dir: Option<&str>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("{{log_dir}}") {
            match log_dir {
                Some(value) => out.push_str(value),
                None => out.push_str("{{log_dir}}"),
            }
            rest = after;
        } else if let Some(after) = rest.strip_prefix("{{root}}") {
            out.push_str(root);
            rest = after;
        } else if let Some(after) = rest.strip_prefix("{{tomcat}}") {
            out.push_str(tomcat);
            rest = after;
        } else {
            let ch = rest.chars().next().expect("rest is non-empty");
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    out
}

pub fn resolve_project(id: &str, project: &Project, home: &Path) -> ResolvedProject {
    let root = expand_tilde_str(&project.root, home);
    let tomcat = expand_tilde_str(&project.tomcat, home);
    // `log_dir` may reference `{{root}}` and `{{tomcat}}`, not itself.
    let log_dir = expand_tilde_str(&substitute(&project.log_dir, &root, &tomcat, None), home);

    let expand_arg = |raw: &str| {
        let rendered = substitute(raw, &root, &tomcat, Some(&log_dir));
        expand_tilde_str(&rendered, home)
    };

    ResolvedProject {
        id: id.to_string(),
        name: project.name.clone(),
        root: PathBuf::from(&root),
        tomcat: PathBuf::from(&tomcat),
        log_dir: PathBuf::from(&log_dir),
        log_file: PathBuf::from(expand_arg(&project.log_file)),
        pid_file: PathBuf::from(expand_arg(&project.pid_file)),
        port: project.port,
        login_url: substitute(&project.login_url, &root, &tomcat, Some(&log_dir)),
        start: project.start.iter().map(|part| expand_arg(part)).collect(),
        stop: project.stop.iter().map(|part| expand_arg(part)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundled_config() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/projects.toml")
    }

    #[test]
    fn bundled_td_work_defaults_expand_and_hold_no_secrets() {
        let text = fs::read_to_string(bundled_config()).expect("read bundled config");
        let lower = text.to_ascii_lowercase();
        for banned in [
            "password",
            "passwd",
            "secret",
            "api_key",
            "token",
            "begin private",
            "begin rsa",
        ] {
            assert!(!lower.contains(banned), "{banned} found in projects.toml");
        }

        let cfg = parse_config(&text).expect("parse");
        let project = cfg.projects.get("td_work").expect("td_work");
        assert_eq!(project.name, "td_work");
        assert_eq!(project.root, "~/code/testdaily/tdprojects");
        assert_eq!(
            project.start,
            vec![
                "bash".to_string(),
                "{{root}}/scripts/start-td-work.sh".to_string()
            ]
        );
        assert_eq!(project.tomcat, "~/opt/apache-tomcat-9.0.98");
        assert_eq!(project.log_dir, "~/code/_index/logs");
        assert_eq!(project.log_file, "{{log_dir}}/td_work_catalina.out");
        assert_eq!(project.pid_file, "{{log_dir}}/td_work_tomcat.pid");
        assert_eq!(project.port, 8080);
        assert_eq!(
            project.login_url,
            "http://127.0.0.1:8080/td_work/admin/login.html"
        );

        let resolved = resolve_project("td_work", project, Path::new("/home/dev"));
        assert_eq!(
            resolved.root,
            PathBuf::from("/home/dev/code/testdaily/tdprojects")
        );
        assert_eq!(
            resolved.tomcat,
            PathBuf::from("/home/dev/opt/apache-tomcat-9.0.98")
        );
        assert_eq!(
            resolved.log_dir,
            PathBuf::from("/home/dev/code/_index/logs")
        );
        assert_eq!(
            resolved.log_file,
            PathBuf::from("/home/dev/code/_index/logs/td_work_catalina.out")
        );
        assert_eq!(
            resolved.pid_file,
            PathBuf::from("/home/dev/code/_index/logs/td_work_tomcat.pid")
        );
        assert_eq!(
            resolved.start,
            vec![
                "bash".to_string(),
                "/home/dev/code/testdaily/tdprojects/scripts/start-td-work.sh".to_string(),
            ]
        );
        let script = resolved.stop.get(2).expect("stop shell");
        assert_eq!(
            script,
            "export CATALINA_PID=\"/home/dev/code/_index/logs/td_work_tomcat.pid\" CATALINA_HOME=\"/home/dev/opt/apache-tomcat-9.0.98\" CATALINA_BASE=\"/home/dev/opt/apache-tomcat-9.0.98\"; \"/home/dev/opt/apache-tomcat-9.0.98/bin/catalina.sh\" stop 15 -force"
        );
        assert!(!script.contains('~'));
        assert!(!script.contains("{{"));
        assert_eq!(resolved.port, 8080);
        assert_eq!(
            resolved.login_url,
            "http://127.0.0.1:8080/td_work/admin/login.html"
        );
    }

    #[test]
    fn tilde_expansion_rules() {
        let home = Path::new("/home/dev");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/dev"));
        assert_eq!(expand_tilde("~/", home), PathBuf::from("/home/dev"));
        assert_eq!(
            expand_tilde("~/code/td", home),
            PathBuf::from("/home/dev/code/td")
        );
        assert_eq!(
            expand_tilde("~/my dir/a", home),
            PathBuf::from("/home/dev/my dir/a")
        );
        assert_eq!(
            expand_tilde("~other/proj", home),
            PathBuf::from("~other/proj")
        );
        assert_eq!(expand_tilde("/abs/path", home), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("rel/path", home), PathBuf::from("rel/path"));
    }

    #[test]
    fn templates_expand_once_and_log_dir_can_use_root() {
        let text = r#"
            [projects.demo]
            name = "demo"
            root = "~/{{tomcat}}"
            start = ["bash", "{{root}}/run.sh"]
            stop = ["bash", "{{tomcat}}/bin/stop.sh"]
            tomcat = "~/opt/tomcat"
            log_dir = "{{root}}/logs"
            log_file = "{{log_dir}}/app.out"
            pid_file = "{{log_dir}}/app.pid"
            port = 9
            login_url = "http://127.0.0.1:9/{{root}}"
        "#;
        let cfg = parse_config(text).unwrap();
        let resolved = resolve_project("demo", &cfg.projects["demo"], Path::new("/h"));
        // `{{tomcat}}` inside the already expanded root is not scanned again.
        assert_eq!(resolved.root, PathBuf::from("/h/{{tomcat}}"));
        assert_eq!(resolved.tomcat, PathBuf::from("/h/opt/tomcat"));
        assert_eq!(resolved.log_dir, PathBuf::from("/h/{{tomcat}}/logs"));
        assert_eq!(
            resolved.log_file,
            PathBuf::from("/h/{{tomcat}}/logs/app.out")
        );
        assert_eq!(resolved.start[1], "/h/{{tomcat}}/run.sh");
        assert_eq!(resolved.stop[1], "/h/opt/tomcat/bin/stop.sh");
        assert_eq!(resolved.login_url, "http://127.0.0.1:9//h/{{tomcat}}");
    }

    #[test]
    fn env_overrides_root_tomcat_and_log_dir() {
        let text = r#"
            [projects.td_work]
            name = "td_work"
            root = "~/code/testdaily/tdprojects"
            start = ["bash", "{{root}}/scripts/start-td-work.sh"]
            stop = ["true"]
            tomcat = "~/opt/apache-tomcat-9.0.98"
            log_dir = "~/code/_index/logs"
            log_file = "{{log_dir}}/td_work_catalina.out"
            pid_file = "{{log_dir}}/td_work_tomcat.pid"
            port = 8080
            login_url = "http://127.0.0.1:8080/td_work/admin/login.html"

            [projects.my-app]
            name = "my-app"
            root = "/tmp/app"
            start = ["true"]
            stop = ["true"]
            tomcat = "/tmp/tomcat"
            log_dir = "/tmp/logs"
            log_file = "{{log_dir}}/a.out"
            pid_file = "{{log_dir}}/a.pid"
            port = 9
            login_url = "http://127.0.0.1:9/"
        "#;
        let mut cfg = parse_config(text).unwrap();
        apply_env_overrides(
            &mut cfg,
            &[
                ("TONGYUN_DEV_TD_WORK_ROOT".into(), "/opt/work".into()),
                ("TONGYUN_DEV_TD_WORK_TOMCAT".into(), "  ".into()),
                ("TONGYUN_DEV_TD_WORK_LOG_DIR".into(), "/var/log/td".into()),
                ("TONGYUN_DEV_MY_APP_ROOT".into(), "/srv/app".into()),
            ],
        );
        let work = resolve_project("td_work", &cfg.projects["td_work"], Path::new("/home/dev"));
        assert_eq!(work.root, PathBuf::from("/opt/work"));
        assert_eq!(
            work.tomcat,
            PathBuf::from("/home/dev/opt/apache-tomcat-9.0.98")
        );
        assert_eq!(work.log_dir, PathBuf::from("/var/log/td"));
        assert_eq!(
            work.log_file,
            PathBuf::from("/var/log/td/td_work_catalina.out")
        );
        assert_eq!(work.start[1], "/opt/work/scripts/start-td-work.sh");
        let app = resolve_project("my-app", &cfg.projects["my-app"], Path::new("/home/dev"));
        assert_eq!(app.root, PathBuf::from("/srv/app"));
    }

    #[test]
    fn parse_rejects_missing_fields_empty_commands_and_garbage() {
        let err = parse_config("not toml {{{").unwrap_err();
        assert!(err.to_string().contains("parse projects.toml"));

        let missing = r#"
            [projects.demo]
            name = "demo"
        "#;
        assert!(parse_config(missing).is_err());

        let empty_start = r#"
            [projects.demo]
            name = "demo"
            root = "/r"
            start = []
            stop = ["true"]
            tomcat = "/t"
            log_dir = "/l"
            log_file = "/l/a"
            pid_file = "/l/p"
            port = 9
            login_url = "http://127.0.0.1:9/"
        "#;
        assert!(parse_config(empty_start)
            .unwrap_err()
            .to_string()
            .contains("empty start"));

        let empty_table = "[projects]\n";
        assert!(parse_config(empty_table).is_err());
    }

    #[test]
    fn unknown_scalar_fields_are_ignored() {
        let text = r#"
            [projects.demo]
            name = "demo"
            note = "kept out of the model"
            root = "/r"
            start = ["true"]
            stop = ["true"]
            tomcat = "/t"
            log_dir = "/l"
            log_file = "/l/a"
            pid_file = "/l/p"
            port = 9
            login_url = "http://127.0.0.1:9/"
        "#;
        let cfg = parse_config(text).unwrap();
        assert_eq!(cfg.projects["demo"].name, "demo");
    }

    #[test]
    fn resolve_config_path_prefers_flag_then_env_then_walk() {
        let tmp = crate::testutil::TempDir::new();
        let repo = tmp.path().join("repo");
        let nested = repo.join("a/b");
        fs::create_dir_all(repo.join("config")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        let cfg = repo.join("config/projects.toml");
        fs::write(&cfg, "x").unwrap();
        let other = tmp.path().join("other.toml");
        fs::write(&other, "y").unwrap();

        let found =
            resolve_config_path(Some(&other), Some(cfg.to_str().unwrap()), &nested).unwrap();
        assert_eq!(found, other);

        let from_env = resolve_config_path(None, Some(cfg.to_str().unwrap()), &nested).unwrap();
        assert_eq!(from_env, cfg);

        let walked = resolve_config_path(None, Some("  "), &nested).unwrap();
        assert_eq!(walked, cfg);

        let missing = tmp.path().join("nope.toml");
        assert!(resolve_config_path(Some(&missing), None, &nested).is_err());
        assert!(resolve_config_path(None, None, tmp.path()).is_err());
    }
}
