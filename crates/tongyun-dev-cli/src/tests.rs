#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::TcpListener;
    use std::path::PathBuf;

    use crate::config::{expand_user, get_proj, render, FileConfig, Project};
    use crate::runner::{open_url, pid_alive, port_open, read_last_lines, LoginProbe, StatusJson};

    #[test]
    fn test_expand_user() {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let expanded = expand_user("~/code/test");
        assert_eq!(expanded, home.join("code/test"));

        let abs_path = if cfg!(windows) { "C:\\test\\dir" } else { "/test/dir" };
        assert_eq!(expand_user(abs_path), PathBuf::from(abs_path));
    }

    #[test]
    fn test_render_variables() {
        let proj = Project {
            name: "test_proj".into(),
            root: "/tmp/root".into(),
            start: vec!["echo".into()],
            stop: vec![],
            tomcat: "/opt/tomcat".into(),
            log_dir: "/var/logs".into(),
            log_file: "{{log_dir}}/test.log".into(),
            pid_file: "{{log_dir}}/test.pid".into(),
            port: 8080,
            login_url: "http://127.0.0.1:8080".into(),
        };

        let rendered_log = render(&proj.log_file, &proj);
        assert!(rendered_log.contains("test.log"));
        let rendered_script = render("{{root}}/bin/start.sh", &proj);
        assert!(rendered_script.contains("start.sh"));
    }

    #[test]
    fn test_config_serde() {
        let toml_str = r#"
[projects.demo]
name = "demo_service"
root = "~/code/demo"
start = ["npm", "run", "dev"]
stop = ["kill", "{{pid}}"]
tomcat = ""
log_dir = "~/logs"
log_file = "{{log_dir}}/demo.log"
pid_file = "{{log_dir}}/demo.pid"
port = 3000
login_url = "http://127.0.0.1:3000"
"#;
        let cfg: FileConfig = toml::from_str(toml_str).expect("deserialize config");
        assert!(cfg.projects.contains_key("demo"));
        let proj = &cfg.projects["demo"];
        assert_eq!(proj.name, "demo_service");
        assert_eq!(proj.port, 3000);

        let serialized = toml::to_string_pretty(&cfg).expect("serialize config");
        let re_parsed: FileConfig = toml::from_str(&serialized).expect("re-parse config");
        assert_eq!(re_parsed.projects["demo"].name, "demo_service");
    }

    #[test]
    fn test_pid_alive() {
        // pid 0 must be false
        assert!(!pid_alive(0));

        // Current process PID must be alive
        let my_pid = std::process::id();
        assert!(pid_alive(my_pid));
    }

    #[test]
    fn test_port_open() {
        // Port 0 is invalid
        assert!(!port_open(0));

        // Bind a random ephemeral port
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("get local addr").port();

        // While listener is open, port_open must be true
        assert!(port_open(port));

        // Drop listener to release port
        drop(listener);

        // Allow tiny moment for OS port release
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!port_open(port));
    }

    #[test]
    fn test_read_last_lines() {
        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join(format!("test_log_{}.log", std::process::id()));

        let lines_content = (1..=20)
            .map(|i| format!("log line number {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&test_file, lines_content).expect("write temp log");

        let result = read_last_lines(&test_file, 5).expect("read last lines");
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], "log line number 16");
        assert_eq!(result[4], "log line number 20");

        let _ = fs::remove_file(&test_file);

        // Missing file must error
        let missing_path = temp_dir.join("non_existent_file_abc_123.log");
        assert!(read_last_lines(&missing_path, 10).is_err());
    }

    #[test]
    fn test_status_json_schema() {
        let st = StatusJson {
            id: "legacy_app".into(),
            name: "acme 工作平台".into(),
            running: true,
            pid: Some(12345),
            port: 8080,
            port_open: true,
            login_probe: LoginProbe {
                url: "http://127.0.0.1:8080/legacy_app/admin/login.html".into(),
                http_code: Some(200),
                ok: true,
            },
            log_path: "/tmp/catalina.out".into(),
            pid_path: "/tmp/tomcat.pid".into(),
            root: "/tmp/legacy-app".into(),
        };

        let json_val = serde_json::to_value(&st).expect("serialize StatusJson");
        assert_eq!(json_val["id"], "legacy_app");
        assert_eq!(json_val["running"], true);
        assert_eq!(json_val["pid"], 12345);
        assert_eq!(json_val["port"], 8080);
        assert_eq!(json_val["port_open"], true);
        assert_eq!(json_val["login_probe"]["http_code"], 200);
        assert_eq!(json_val["login_probe"]["ok"], true);
        assert_eq!(json_val["log_path"], "/tmp/catalina.out");
        assert_eq!(json_val["root"], "/tmp/legacy-app");
    }

    #[test]
    fn test_add_and_get_project_in_memory() {
        let mut cfg = FileConfig {
            projects: std::collections::HashMap::new(),
        };

        assert!(get_proj(&cfg, "my_app").is_err());

        cfg.projects.insert(
            "my_app".into(),
            Project {
                name: "My App".into(),
                root: "~/code/app".into(),
                start: vec!["cargo".into(), "run".into()],
                stop: vec![],
                tomcat: "".into(),
                log_dir: "".into(),
                log_file: "".into(),
                pid_file: "".into(),
                port: 8000,
                login_url: "".into(),
            },
        );

        assert!(get_proj(&cfg, "my_app").is_ok());
        let proj = get_proj(&cfg, "my_app").unwrap();
        assert_eq!(proj.name, "My App");
    }

    #[test]
    fn test_open_url_empty_validation() {
        assert!(open_url("").is_err());
        assert!(open_url("   ").is_err());
    }
}
