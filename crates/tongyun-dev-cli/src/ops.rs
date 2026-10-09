//! Start, stop, status, and logs. Status JSON field names are a stable contract.

use crate::config::ResolvedProject;
use crate::probe::{http_code_ok, Probe};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginProbe {
    pub url: String,
    pub http_code: Option<u16>,
    pub ok: bool,
}

/// Stable `--json` object. Keys stay; whitespace from pretty-print does not.
///
/// `running` is true only when the pid file names a live process.
/// `port_open` is an independent localhost TCP check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusJson {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub port: u16,
    pub port_open: bool,
    pub login_probe: LoginProbe,
    pub log_path: String,
    pub pid_path: String,
    pub root: String,
}

impl StatusJson {
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("parse status json")
    }

    pub fn to_json_pretty(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub id: String,
    pub name: String,
    pub root: String,
}

impl ListEntry {
    pub fn from_json_vec(text: &str) -> Result<Vec<Self>> {
        serde_json::from_str(text).context("parse list json")
    }
}

/// First line of a pid file. Missing, blank, non-numeric, and `0` are all "no pid".
pub fn read_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim();
    let pid = line.parse::<u32>().ok()?;
    if pid == 0 {
        None
    } else {
        Some(pid)
    }
}

pub fn status_of(project: &ResolvedProject, probe: &dyn Probe) -> StatusJson {
    let pid = read_pid(&project.pid_file).filter(|pid| probe.pid_alive(*pid));
    let http_code = probe.login_http_code(&project.login_url);
    StatusJson {
        id: project.id.clone(),
        name: project.name.clone(),
        running: pid.is_some(),
        pid,
        port: project.port,
        port_open: probe.port_open(project.port),
        login_probe: LoginProbe {
            url: project.login_url.clone(),
            http_code,
            ok: http_code_ok(http_code),
        },
        log_path: project.log_file.display().to_string(),
        pid_path: project.pid_file.display().to_string(),
        root: project.root.display().to_string(),
    }
}

/// Run argv. Arguments are never formatted into errors.
fn run_argv(argv: &[String], cwd: Option<&Path>) -> Result<()> {
    if argv.is_empty() {
        bail!("empty command");
    }
    let prog = &argv[0];
    let mut cmd = Command::new(prog);
    cmd.args(&argv[1..]).stdin(Stdio::null());
    if let Some(dir) = cwd {
        if dir.is_dir() {
            cmd.current_dir(dir);
        }
    }
    let status = cmd.status().with_context(|| format!("spawn {prog}"))?;
    if !status.success() {
        bail!("{prog} exited with {status}");
    }
    Ok(())
}

pub fn start_project(project: &ResolvedProject) -> Result<()> {
    eprintln!("starting {} …", project.id);
    run_argv(&project.start, Some(&project.root))
}

#[derive(Debug, Clone)]
pub struct StopOptions {
    pub timeout: Duration,
    pub poll: Duration,
}

impl Default for StopOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            poll: Duration::from_millis(500),
        }
    }
}

pub fn stop_project(project: &ResolvedProject, probe: &dyn Probe, opts: StopOptions) -> Result<()> {
    eprintln!("stopping {} …", project.id);
    match run_argv(&project.stop, Some(&project.root)) {
        Ok(()) => {}
        Err(err) => {
            if !still_up(project, probe) {
                eprintln!("stop reported error but process and port are already down: {err}");
            } else {
                return Err(err);
            }
        }
    }
    let deadline = Instant::now() + opts.timeout;
    loop {
        if !still_up(project, probe) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let pid_alive = read_pid(&project.pid_file).is_some_and(|pid| probe.pid_alive(pid));
            let port_open = probe.port_open(project.port);
            bail!(
                "stop timed out for {}: pid_alive={pid_alive} port_open={port_open} port={}",
                project.id,
                project.port
            );
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(opts.poll));
    }
}

fn still_up(project: &ResolvedProject, probe: &dyn Probe) -> bool {
    let pid_alive = read_pid(&project.pid_file).is_some_and(|pid| probe.pid_alive(pid));
    pid_alive || probe.port_open(project.port)
}

pub(crate) fn tail_args(lines: usize, follow: bool, path: &Path) -> Vec<String> {
    let mut args = vec![format!("-n{lines}")];
    if follow {
        args.push("-F".into());
    }
    args.push(path.display().to_string());
    args
}

pub fn logs_project(project: &ResolvedProject, follow: bool, lines: usize) -> Result<()> {
    if !project.log_file.is_file() {
        bail!("log file missing: {}", project.log_file.display());
    }
    let args = tail_args(lines, follow, &project.log_file);
    let status = Command::new("tail")
        .args(&args)
        .status()
        .context("spawn tail")?;
    if !status.success() && !follow {
        bail!("tail failed: {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse_config, resolve_project};
    use crate::testutil::TempDir;
    use std::collections::HashSet;

    struct FakeProbe {
        alive: HashSet<u32>,
        open: HashSet<u16>,
        code: Option<u16>,
    }

    impl Probe for FakeProbe {
        fn pid_alive(&self, pid: u32) -> bool {
            self.alive.contains(&pid)
        }
        fn port_open(&self, port: u16) -> bool {
            self.open.contains(&port)
        }
        fn login_http_code(&self, _url: &str) -> Option<u16> {
            self.code
        }
    }

    fn project_in(tmp: &TempDir, port: u16) -> ResolvedProject {
        let root = tmp.path().join("root");
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        let text = format!(
            r#"
            [projects.demo]
            name = "Demo"
            root = "{root}"
            start = ["sh", "-c", "touch ./started-marker"]
            stop = ["sh", "-c", "rm -f pid"]
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
        );
        let cfg = parse_config(&text).unwrap();
        resolve_project("demo", &cfg.projects["demo"], Path::new("/unused"))
    }

    #[test]
    fn read_pid_file_cases() {
        let tmp = TempDir::new();
        let path = tmp.path().join("p.pid");
        assert_eq!(read_pid(&path), None);
        fs::write(&path, "").unwrap();
        assert_eq!(read_pid(&path), None);
        fs::write(&path, "0\n").unwrap();
        assert_eq!(read_pid(&path), None);
        fs::write(&path, "abc").unwrap();
        assert_eq!(read_pid(&path), None);
        fs::write(&path, " 42 \nrest\n").unwrap();
        assert_eq!(read_pid(&path), Some(42));
        fs::write(&path, "42 extra\n").unwrap();
        assert_eq!(read_pid(&path), None);
    }

    #[test]
    fn status_json_keys_and_running_mean_live_pid() {
        let tmp = TempDir::new();
        let project = project_in(&tmp, 9);
        fs::write(&project.pid_file, "42\n").unwrap();
        let up = FakeProbe {
            alive: HashSet::from([42]),
            open: HashSet::from([9]),
            code: Some(200),
        };
        let status = status_of(&project, &up);
        assert!(status.running);
        assert_eq!(status.pid, Some(42));
        assert!(status.port_open);
        assert!(status.login_probe.ok);
        assert_eq!(status.login_probe.http_code, Some(200));
        assert_eq!(status.port, 9);

        let value = serde_json::to_value(&status).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "id",
                "log_path",
                "login_probe",
                "name",
                "pid",
                "pid_path",
                "port",
                "port_open",
                "root",
                "running",
            ]
        );
        let mut probe_keys: Vec<_> = value["login_probe"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        probe_keys.sort();
        assert_eq!(probe_keys, vec!["http_code", "ok", "url"]);

        let pretty = status.to_json_pretty().unwrap();
        let parsed = StatusJson::from_json(&pretty).unwrap();
        assert_eq!(parsed, status);
        assert!(!pretty.contains("started-marker"));
        assert!(!pretty.contains("catalina.sh"));

        let stale = FakeProbe {
            alive: HashSet::new(),
            open: HashSet::new(),
            code: Some(503),
        };
        let down = status_of(&project, &stale);
        assert!(!down.running);
        assert_eq!(down.pid, None);
        assert!(!down.port_open);
        assert!(!down.login_probe.ok);
        assert_eq!(down.login_probe.http_code, Some(503));
    }

    #[test]
    fn port_open_without_pid_is_not_running() {
        let tmp = TempDir::new();
        let project = project_in(&tmp, 9);
        let probe = FakeProbe {
            alive: HashSet::new(),
            open: HashSet::from([9]),
            code: None,
        };
        let status = status_of(&project, &probe);
        assert!(!status.running);
        assert!(status.port_open);
        assert!(!status.login_probe.ok);
    }

    #[test]
    fn start_runs_in_project_root_and_does_not_echo_args() {
        let tmp = TempDir::new();
        let mut project = project_in(&tmp, 9);
        project.start = vec!["sh".into(), "-c".into(), "touch ./started-marker".into()];
        start_project(&project).unwrap();
        assert!(project.root.join("started-marker").is_file());
    }

    #[test]
    fn stop_error_omits_command_arguments() {
        let tmp = TempDir::new();
        let mut project = project_in(&tmp, 9);
        let secret = "super-secret-token-value";
        project.stop = vec!["false".into(), secret.into()];
        fs::write(&project.pid_file, "7\n").unwrap();
        let probe = FakeProbe {
            alive: HashSet::from([7]),
            open: HashSet::new(),
            code: None,
        };
        let err = stop_project(
            &project,
            &probe,
            StopOptions {
                timeout: Duration::from_millis(0),
                poll: Duration::from_millis(0),
            },
        )
        .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("exited"));
        assert!(!text.contains(secret));
    }

    #[test]
    fn stop_accepts_already_down_and_fails_when_port_stays_open() {
        let tmp = TempDir::new();
        let mut project = project_in(&tmp, 9);
        project.stop = vec!["false".into()];
        let down = FakeProbe {
            alive: HashSet::new(),
            open: HashSet::new(),
            code: None,
        };
        stop_project(
            &project,
            &down,
            StopOptions {
                timeout: Duration::from_secs(5),
                poll: Duration::from_secs(5),
            },
        )
        .unwrap();

        let mut hanging = project.clone();
        hanging.stop = vec!["true".into()];
        let up = FakeProbe {
            alive: HashSet::new(),
            open: HashSet::from([9]),
            code: None,
        };
        let err = stop_project(
            &hanging,
            &up,
            StopOptions {
                timeout: Duration::from_millis(0),
                poll: Duration::from_millis(0),
            },
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("timed out"));
        assert!(text.contains("port_open=true"));
        assert!(text.contains("port=9"));
    }

    #[test]
    fn tail_args_follow_flag() {
        let path = Path::new("/tmp/catalina.out");
        assert_eq!(
            tail_args(80, false, path),
            vec!["-n80".to_string(), "/tmp/catalina.out".to_string()]
        );
        assert_eq!(
            tail_args(10, true, path),
            vec![
                "-n10".to_string(),
                "-F".to_string(),
                "/tmp/catalina.out".to_string()
            ]
        );
    }

    #[test]
    fn logs_missing_file_errors_without_spawning() {
        let tmp = TempDir::new();
        let project = project_in(&tmp, 9);
        let err = logs_project(&project, false, 5).unwrap_err();
        assert!(err.to_string().contains("log file missing"));
    }

    #[test]
    fn empty_argv_errors() {
        let err = run_argv(&[], None).unwrap_err();
        assert!(err.to_string().contains("empty command"));
    }
}
