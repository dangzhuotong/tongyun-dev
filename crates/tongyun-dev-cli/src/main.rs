//! tongyun-dev CLI. Project operations live in `tongyun-dev-core`; the web
//! console (`ui`) and the desktop shell call the same core.
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;
use tongyun_dev_cli::server::{start_ui_server, DEFAULT_UI_PORT};
use tongyun_dev_core::{
    unresolved_status, Loaded, NewProject, ScanReport, StatusJson, DEFAULT_ADD_PORT,
    DEFAULT_SCAN_DEPTH,
};

#[derive(Parser, Debug)]
#[command(
    name = "tongyun-dev",
    about = "Local project runner (start/stop/restart/wait/status/logs/install/scan/ui). No subcommand = web console on :9099"
)]
struct Cli {
    /// Config file path (lookup order: --config -> $TONGYUN_DEV_CONFIG -> ~/.config/tongyun-dev/settings.json -> ~/.config/tongyun-dev/projects.toml -> ./config/projects.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Cmd {
    /// List registered projects
    List {
        #[arg(long)]
        json: bool,
        /// Include groups in JSON output: {"projects":[...],"groups":[{key,label,order}]}
        #[arg(long)]
        with_groups: bool,
    },
    /// Start a project and wait until the login page / port responds
    Start {
        /// Project id to start (optional when using --all or --group)
        #[arg(conflicts_with = "all", conflicts_with = "group")]
        id: Option<String>,
        /// Start all registered projects
        #[arg(long, conflicts_with = "group")]
        all: bool,
        /// Start all projects in the specified group
        #[arg(long)]
        group: Option<String>,
        /// Profile name to launch with (e.g. dev, prod)
        #[arg(long)]
        profile: Option<String>,
        /// Open the project page in the browser after starting
        #[arg(long, conflicts_with = "json", conflicts_with = "all", conflicts_with = "group")]
        open: bool,
        #[arg(long)]
        json: bool,
        /// Wait budget in seconds after launching; timeout leaves the service running
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: Option<u64>,
    },
    /// Stop, start again and wait for readiness
    Restart {
        /// Project id to restart (optional when using --all or --group)
        #[arg(conflicts_with = "all", conflicts_with = "group")]
        id: Option<String>,
        /// Restart all registered projects
        #[arg(long, conflicts_with = "group")]
        all: bool,
        /// Restart all projects in the specified group
        #[arg(long)]
        group: Option<String>,
        /// Profile name to launch with (e.g. dev, prod)
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, conflicts_with = "json", conflicts_with = "all", conflicts_with = "group")]
        open: bool,
        #[arg(long)]
        json: bool,
        /// Wait budget in seconds after launching; timeout leaves the service running
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: Option<u64>,
    },
    /// Stop a project and wait until its port is free
    Stop {
        /// Project id to stop (optional when using --all or --group)
        #[arg(conflicts_with = "all", conflicts_with = "group")]
        id: Option<String>,
        /// Stop all registered projects
        #[arg(long, conflicts_with = "group")]
        all: bool,
        /// Stop all projects in the specified group
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Install frontend dependencies (pnpm / yarn / npm picked by lockfile); output goes to the project log
    Install {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Wait for an existing service; never starts or stops it
    Wait {
        id: String,
        #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
        #[arg(long)]
        json: bool,
    },
    /// Show status (process + port + login probe). Without ID, shows all projects.
    Status {
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Tail logs
    Logs {
        id: String,
        #[arg(long)]
        follow: bool,
        #[arg(long, conflicts_with = "follow")]
        json: bool,
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    /// Open the project page in the browser (or the project folder with --dir, or all links with --all)
    Open {
        id: String,
        #[arg(long)]
        dir: bool,
        /// Open all configured web links
        #[arg(long, conflicts_with = "dir")]
        all: bool,
    },
    /// Launch the project in an interactive external system terminal
    Terminal {
        id: String,
        /// Profile name to launch with
        #[arg(long)]
        profile: Option<String>,
    },
    /// Register a new project in your config file (comments are kept)
    Add {
        /// Project id (letters, digits, `_`, `-`; e.g. my_web)
        id: String,
        /// Display name (defaults to the id)
        #[arg(long)]
        name: Option<String>,
        /// Project root (`~` is fine)
        #[arg(long)]
        root: String,
        /// Start command line, e.g. "pnpm dev"
        #[arg(long)]
        start: String,
        /// Stop command line (optional; otherwise the process group and port listener are stopped)
        #[arg(long)]
        stop: Option<String>,
        #[arg(long, default_value_t = DEFAULT_ADD_PORT)]
        port: u16,
        /// Ready probe: HTTP 200/301/302 here means started (optional; otherwise port open)
        #[arg(long)]
        login_url: Option<String>,
        /// Ready probe used when login_url is empty
        #[arg(long)]
        health_url: Option<String>,
        /// Log directory (default ~/.tongyun-dev/logs). `~`, `{{root}}`, or a path relative to the project root (`..` is not allowed).
        #[arg(long)]
        log_dir: Option<String>,
        /// Log file (default ~/.tongyun-dev/logs/<id>.out)
        #[arg(long)]
        log_file: Option<String>,
        /// Node version, e.g. 22
        #[arg(long)]
        node: Option<String>,
        /// Java version, e.g. 17
        #[arg(long)]
        java: Option<String>,
        /// Explicit JDK home directory
        #[arg(long)]
        java_home: Option<String>,
        /// Explicit Node home or bin directory
        #[arg(long)]
        node_home: Option<String>,
        /// Shell to run commands with (bash, sh, cmd, powershell)
        #[arg(long)]
        shell: Option<String>,
        /// Run the start command in the foreground (only for scripts that exit by themselves)
        #[arg(long)]
        foreground: bool,
        /// Do not install node_modules automatically before the first start
        #[arg(long)]
        no_auto_install: bool,
        /// Port delivery mode (none, arg, env, config)
        #[arg(long)]
        port_mode: Option<String>,
        /// Port argument flag for port_mode=arg (default --port)
        #[arg(long, allow_hyphen_values = true)]
        port_arg: Option<String>,
        /// Port env var name for port_mode=env (default PORT)
        #[arg(long)]
        port_env: Option<String>,
        /// Project group name (optional)
        #[arg(long)]
        group: Option<String>,
        /// Extra environment variables (KEY=VALUE, repeatable)
        #[arg(long = "env", value_parser = parse_env_pair)]
        env: Vec<(String, String)>,
        /// Run in pseudo-terminal (PTY)
        #[arg(long)]
        pty: bool,
        /// Auto answer rules ("MATCH=>SEND", repeatable)
        #[arg(long = "auto-answer", value_parser = parse_auto_answer_pair)]
        auto_answers: Vec<tongyun_dev_core::AutoAnswer>,
        /// Shortcut web link ("TITLE=URL" or "URL", repeatable)
        #[arg(long = "link", value_parser = parse_web_link_pair)]
        links: Vec<tongyun_dev_core::WebLink>,
        /// Default profile to run with
        #[arg(long)]
        default_profile: Option<String>,
        /// Dependent project IDs (repeatable)
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
    },
    /// Edit an existing registered project in your config file
    Edit {
        /// Project id
        id: String,
        /// Extra environment variables (KEY=VALUE, repeatable)
        #[arg(long = "env", value_parser = parse_env_pair)]
        env: Vec<(String, String)>,
        /// Enable pseudo-terminal (PTY)
        #[arg(long, conflicts_with = "no_pty")]
        pty: bool,
        /// Disable pseudo-terminal (PTY)
        #[arg(long, conflicts_with = "pty")]
        no_pty: bool,
        /// Auto answer rules ("MATCH=>SEND", repeatable)
        #[arg(long = "auto-answer", value_parser = parse_auto_answer_pair)]
        auto_answers: Vec<tongyun_dev_core::AutoAnswer>,
        /// Clear all auto answer rules
        #[arg(long)]
        clear_auto_answers: bool,
        /// Shortcut web link ("TITLE=URL" or "URL", repeatable)
        #[arg(long = "link", value_parser = parse_web_link_pair)]
        links: Vec<tongyun_dev_core::WebLink>,
        /// Clear all shortcut web links
        #[arg(long)]
        clear_links: bool,
        /// Default profile to run with
        #[arg(long)]
        default_profile: Option<String>,
        /// Clear default profile setting
        #[arg(long)]
        clear_default_profile: bool,
        /// Dependent project IDs (repeatable)
        #[arg(long = "depends-on")]
        depends_on: Vec<String>,
        /// Clear all dependent project IDs
        #[arg(long)]
        clear_depends_on: bool,
    },
    /// Send input to a running interactive process
    Input {
        /// Project id
        id: String,
        /// Input text to send
        #[arg(default_value = "")]
        text: String,
        /// Do not append newline to text
        #[arg(long)]
        no_enter: bool,
        /// Send special key (up, down, left, right, enter, tab, esc, ctrl-c)
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set or clear the group for a registered project
    Group {
        /// Project id
        id: String,
        /// Group key (omit when using --clear)
        key: Option<String>,
        /// Clear the group
        #[arg(long, conflicts_with = "key")]
        clear: bool,
    },
    /// Add a new group
    #[command(name = "group-add")]
    GroupAdd {
        /// Group key
        key: String,
        /// Group display label (optional)
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove a registered project (project files are not touched)
    Remove { id: String },
    /// Find projects under a folder (Spring Boot, Tomcat WAR, Vite/Vue/React, vue-cli,
    /// Next, Nuxt, npm start, Django/FastAPI/Flask). Read-only unless --apply.
    Scan {
        /// Folder to scan, e.g. ~/code
        dir: String,
        /// How many folder levels below DIR to look at
        #[arg(long, default_value_t = DEFAULT_SCAN_DEPTH)]
        depth: usize,
        /// Add the new candidates to the config (existing ids / roots are skipped)
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Start the web console and open it in the browser
    Ui {
        #[arg(long, default_value_t = DEFAULT_UI_PORT)]
        port: u16,
        /// Do not open the browser
        #[arg(long)]
        no_open: bool,
    },
}

fn parse_env_pair(s: &str) -> Result<(String, String), String> {
    let Some((k, v)) = s.split_once('=') else {
        return Err(format!("env must be in KEY=VALUE format, got `{s}`"));
    };
    let k = k.trim();
    if k.is_empty() {
        return Err(format!("env variable key cannot be empty in `{s}`"));
    }
    Ok((k.to_string(), v.to_string()))
}

fn parse_auto_answer_pair(s: &str) -> Result<tongyun_dev_core::AutoAnswer, String> {
    let Some((exp, send)) = s.split_once("=>") else {
        return Err(format!(
            "auto-answer must be in 'MATCH=>SEND' format, got `{s}`"
        ));
    };
    let exp = exp.trim();
    if exp.is_empty() {
        return Err(format!(
            "auto-answer expect pattern cannot be empty in `{s}`"
        ));
    }
    Ok(tongyun_dev_core::AutoAnswer {
        expect: exp.to_string(),
        send: send.trim().to_string(),
        once: true,
        regex: false,
    })
}

fn parse_web_link_pair(s: &str) -> Result<tongyun_dev_core::WebLink, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("web link cannot be empty".to_string());
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return Ok(tongyun_dev_core::WebLink {
            title: s.to_string(),
            url: s.to_string(),
        });
    }
    if let Some((title, url)) = s.split_once('=') {
        let title = title.trim();
        let url = url.trim();
        if url.is_empty() {
            return Err(format!("link url cannot be empty in `{s}`"));
        }
        let title = if title.is_empty() { url } else { title };
        Ok(tongyun_dev_core::WebLink {
            title: title.to_string(),
            url: url.to_string(),
        })
    } else {
        Ok(tongyun_dev_core::WebLink {
            title: s.to_string(),
            url: s.to_string(),
        })
    }
}

// Existing list/status JSON keeps its original shape. Operations use a stable envelope.
fn machine_operation(cmd: &Cmd) -> Option<(&str, &str)> {
    match cmd {
        Cmd::Start { id: Some(id), json: true, .. } => Some(("start", id)),
        Cmd::Restart { id: Some(id), json: true, .. } => Some(("restart", id)),
        Cmd::Stop { id: Some(id), json: true, .. } => Some(("stop", id)),
        Cmd::Install { id, json: true } => Some(("install", id)),
        Cmd::Wait { id, json: true, .. } => Some(("wait", id)),
        Cmd::Logs { id, json: true, .. } => Some(("logs", id)),
        Cmd::Input { id, json: true, .. } => Some(("input", id)),
        _ => None,
    }
}

fn run_machine_operation(config: Option<&std::path::Path>, cmd: &Cmd) -> Result<()> {
    let (action, id) = machine_operation(cmd).expect("JSON operation");
    let loaded = tongyun_dev_core::load(config);
    let mut status = None;
    let mut logs = None;
    let result: Result<()> = (|| {
        let loaded = loaded.as_ref().map_err(|e| anyhow::anyhow!("{e:#}"))?;
        let next = match cmd {
            Cmd::Start {
                timeout, profile, ..
            } => loaded.start_with_profile(
                id,
                profile.as_deref(),
                timeout.map(Duration::from_secs),
            )?,
            Cmd::Restart {
                timeout, profile, ..
            } => loaded.restart_with_profile(
                id,
                profile.as_deref(),
                timeout.map(Duration::from_secs),
            )?,
            Cmd::Stop { .. } => loaded.stop(id)?,
            Cmd::Install { .. } => loaded.install_deps_blocking(id)?,
            Cmd::Wait { timeout, .. } => loaded.wait_ready(id, Duration::from_secs(*timeout))?,
            Cmd::Logs { lines, .. } => {
                logs = Some(loaded.log_tail(id, *lines)?);
                return Ok(());
            }
            Cmd::Input {
                text,
                no_enter,
                key,
                ..
            } => {
                let data = tongyun_dev_core::resolve_input_data(
                    Some(text.as_str()),
                    !no_enter,
                    key.as_deref(),
                )?;
                loaded.send_input(id, &data)?;
                return Ok(());
            }
            _ => unreachable!(),
        };
        status = Some(next);
        Ok(())
    })();
    if status.is_none() {
        status = loaded.as_ref().ok().and_then(|l| l.status(id).ok());
    }
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "ok": result.is_ok(), "action": action, "id": id,
            "status": status, "logs": logs,
            "error": result.as_ref().err().map(|e| format!("{e:#}")),
        }))?
    );
    result
}

fn run_batch_start_restart(
    loaded: &Loaded,
    action: &str,
    all: bool,
    group: Option<&str>,
    profile: Option<&str>,
    timeout: Option<u64>,
    json: bool,
) -> Result<()> {
    let target_label = if all {
        "all".to_string()
    } else {
        format!("group:{}", group.unwrap_or_default())
    };

    let target_ids: Vec<String> = if all {
        loaded.list()?.projects.into_iter().map(|p| p.id).collect()
    } else if let Some(g) = group {
        let members = loaded.projects_in_group(g);
        if members.is_empty() {
            anyhow::bail!("分组 `{g}` 为空或不存在");
        }
        members
    } else {
        vec![]
    };

    let target_refs: Vec<&str> = target_ids.iter().map(|s| s.as_str()).collect();
    let ordered_ids = loaded.sorted_projects_for_start(&target_refs);
    let total = ordered_ids.len();
    if total == 0 {
        if json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "ok": true, "action": action, "target": target_label, "total": 0, "results": [], "error": null
                }))?
            );
        } else {
            println!("没有需要执行的项目");
        }
        return Ok(());
    }

    let mut results = Vec::with_capacity(total);
    let mut any_err = false;

    for (idx, id) in ordered_ids.iter().enumerate() {
        if !json {
            println!(
                "[{}/{}] {} `{id}` ...",
                idx + 1,
                total,
                if action == "start" { "启动" } else { "重启" }
            );
        }
        let res = if action == "restart" {
            loaded.restart_with_profile(id, profile, timeout.map(Duration::from_secs))
        } else {
            loaded.start_with_profile(id, profile, timeout.map(Duration::from_secs))
        };
        match res {
            Ok(st) => {
                if !json {
                    print_status(&st, false)?;
                }
                results.push(serde_json::json!({
                    "id": id,
                    "ok": true,
                    "status": st,
                    "error": null,
                }));
            }
            Err(e) => {
                any_err = true;
                let err_msg = format!("{e:#}");
                if !json {
                    eprintln!("  失败: {err_msg}");
                    if let Ok(st) = loaded.status(id) {
                        let _ = print_status(&st, false);
                    }
                }
                results.push(serde_json::json!({
                    "id": id,
                    "ok": false,
                    "status": loaded.status(id).ok(),
                    "error": err_msg,
                }));
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": !any_err,
                "action": action,
                "target": target_label,
                "total": total,
                "results": results,
                "error": if any_err { Some("部分项目执行失败") } else { None }
            }))?
        );
    } else {
        if any_err {
            anyhow::bail!("{total} 个项目中存在执行失败项");
        }
        println!("已完成全部 {total} 个项目的 {action} 操作");
    }

    if any_err {
        anyhow::bail!("批量 {action} 中部分项目失败");
    }
    Ok(())
}

fn run_batch_stop(
    loaded: &Loaded,
    all: bool,
    group: Option<&str>,
    json: bool,
) -> Result<()> {
    let target_label = if all {
        "all".to_string()
    } else {
        format!("group:{}", group.unwrap_or_default())
    };

    let target_ids: Vec<String> = if all {
        loaded.list()?.projects.into_iter().map(|p| p.id).collect()
    } else if let Some(g) = group {
        let members = loaded.projects_in_group(g);
        if members.is_empty() {
            anyhow::bail!("分组 `{g}` 为空或不存在");
        }
        members
    } else {
        vec![]
    };

    let target_refs: Vec<&str> = target_ids.iter().map(|s| s.as_str()).collect();
    let ordered_ids = loaded.sorted_projects_for_stop(&target_refs);
    let total = ordered_ids.len();
    if total == 0 {
        if json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "ok": true, "action": "stop", "target": target_label, "total": 0, "results": [], "error": null
                }))?
            );
        } else {
            println!("没有需要停止的项目");
        }
        return Ok(());
    }

    let mut results = Vec::with_capacity(total);
    let mut any_err = false;

    for (idx, id) in ordered_ids.iter().enumerate() {
        if !json {
            println!("[{}/{}] 停止 `{id}` ...", idx + 1, total);
        }
        match loaded.stop(id) {
            Ok(st) => {
                if !json {
                    print_status(&st, false)?;
                }
                results.push(serde_json::json!({
                    "id": id,
                    "ok": true,
                    "status": st,
                    "error": null,
                }));
            }
            Err(e) => {
                any_err = true;
                let err_msg = format!("{e:#}");
                if !json {
                    eprintln!("  失败: {err_msg}");
                }
                results.push(serde_json::json!({
                    "id": id,
                    "ok": false,
                    "status": loaded.status(id).ok(),
                    "error": err_msg,
                }));
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "ok": !any_err,
                "action": "stop",
                "target": target_label,
                "total": total,
                "results": results,
                "error": if any_err { Some("部分项目停止失败") } else { None }
            }))?
        );
    } else {
        if any_err {
            anyhow::bail!("{total} 个项目中存在停止失败项");
        }
        println!("已完成全部 {total} 个项目的停止操作");
    }

    if any_err {
        anyhow::bail!("批量停止中部分项目失败");
    }
    Ok(())
}

fn print_status(st: &StatusJson, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(st)?);
    } else {
        println!(
            "{id}  state={state}  running={running}  pid={pid}  port={port} open={open}  login={code:?} ok={ok}",
            id = st.id,
            state = st.state,
            running = st.running,
            pid = st.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            port = st.port,
            open = st.port_open,
            code = st.login_probe.http_code,
            ok = st.login_probe.ok
        );
        if let Some(why) = &st.failure {
            println!("  failure {why}");
        }
        if let Some(err) = &st.resolve_error {
            if st.failure.as_deref() != Some(err.as_str()) {
                println!("  resolve_error {err}");
            }
        }
        for hint in &st.hints {
            println!("  hint {hint}");
        }
        if st.deps_missing {
            println!(
                "  deps missing (node_modules); run `tongyun-dev install {}` ({})",
                st.id, st.install_command
            );
        }
        println!("  log {}", st.log_path);
        println!("  root {}", st.root);
        if st.port_conflict {
            if st.port_holder.is_empty() {
                println!("  port {} held by another process", st.port);
            } else {
                println!("  port {} held by {}", st.port, st.port_holder);
            }
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cmd = cli.cmd.unwrap_or(Cmd::Ui {
        port: DEFAULT_UI_PORT,
        no_open: false,
    });
    if machine_operation(&cmd).is_some() {
        return run_machine_operation(cli.config.as_deref(), &cmd);
    }
    if let Cmd::Scan {
        dir,
        depth,
        apply,
        json,
    } = &cmd
    {
        return run_scan(cli.config.as_deref(), dir, *depth, *apply, *json);
    }
    let mut loaded = match tongyun_dev_core::load(cli.config.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            if matches!(cmd, Cmd::Ui { .. }) {
                eprintln!("{e:#}");
                std::process::exit(1);
            }
            return Err(e);
        }
    };
    match cmd {
        Cmd::List { json, with_groups } => print_list(&loaded, json, with_groups)?,
        Cmd::Start {
            ref id,
            all,
            ref group,
            ref profile,
            open,
            timeout,
            json,
        } => {
            if let Some(id) = id {
                match loaded.start_with_profile(id, profile.as_deref(), timeout.map(Duration::from_secs)) {
                    Ok(st) => {
                        print_status(&st, false)?;
                        if open {
                            if let Err(e) = loaded
                                .page_url(id)
                                .and_then(|u| tongyun_dev_core::open_url(&u))
                            {
                                eprintln!("could not open page: {e:#}");
                            }
                        }
                    }
                    Err(e) => {
                        if let Ok(st) = loaded.status(id) {
                            print_status(&st, false)?;
                        }
                        return Err(e);
                    }
                }
            } else if all || group.is_some() {
                run_batch_start_restart(&loaded, "start", all, group.as_deref(), profile.as_deref(), timeout, json)?;
            } else {
                anyhow::bail!("请指定项目 ID，或使用 --all / --group <组名>");
            }
        }
        Cmd::Restart {
            ref id,
            all,
            ref group,
            ref profile,
            open,
            timeout,
            json,
        } => {
            if let Some(id) = id {
                match loaded.restart_with_profile(id, profile.as_deref(), timeout.map(Duration::from_secs)) {
                    Ok(st) => {
                        print_status(&st, false)?;
                        if open {
                            if let Err(e) = loaded
                                .page_url(id)
                                .and_then(|u| tongyun_dev_core::open_url(&u))
                            {
                                eprintln!("could not open page: {e:#}");
                            }
                        }
                    }
                    Err(e) => {
                        if let Ok(st) = loaded.status(id) {
                            print_status(&st, false)?;
                        }
                        return Err(e);
                    }
                }
            } else if all || group.is_some() {
                run_batch_start_restart(&loaded, "restart", all, group.as_deref(), profile.as_deref(), timeout, json)?;
            } else {
                anyhow::bail!("请指定项目 ID，或使用 --all / --group <组名>");
            }
        }
        Cmd::Install { id, .. } => {
            let st = loaded.status(&id)?;
            eprintln!(
                "安装依赖：{}（输出写到 {}）",
                if st.install_command.is_empty() {
                    "?"
                } else {
                    &st.install_command
                },
                st.log_path
            );
            match loaded.install_deps_blocking(&id) {
                Ok(st) => {
                    eprintln!("依赖装好了");
                    print_status(&st, false)?;
                }
                Err(e) => {
                    if let Ok(tail) = loaded.log_tail(&id, 20) {
                        eprint!("{}", tail.text);
                    }
                    return Err(e);
                }
            }
        }
        Cmd::Stop {
            ref id,
            all,
            ref group,
            json,
        } => {
            if let Some(id) = id {
                loaded.stop(id)?;
                print_status(&loaded.status(id)?, false)?;
            } else if all || group.is_some() {
                run_batch_stop(&loaded, all, group.as_deref(), json)?;
            } else {
                anyhow::bail!("请指定项目 ID，或使用 --all / --group <组名>");
            }
        }
        Cmd::Wait { id, timeout, .. } => {
            print_status(
                &loaded.wait_ready(&id, Duration::from_secs(timeout))?,
                false,
            )?;
        }
        Cmd::Status { id: Some(id), json } => print_status(&loaded.status(&id)?, json)?,
        Cmd::Status { id: None, json } => {
            let list = loaded.list()?;
            let mut failed = 0usize;
            let mut all = Vec::with_capacity(list.projects.len());
            for row in &list.projects {
                match loaded.status(&row.id) {
                    Ok(st) => all.push(st),
                    Err(e) => {
                        failed += 1;
                        let msg = row
                            .resolve_error
                            .clone()
                            .unwrap_or_else(|| format!("{e:#}"));
                        all.push(unresolved_status(row, msg));
                    }
                }
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&all)?);
            } else {
                for st in &all {
                    print_status(st, false)?;
                    println!();
                }
            }
            if failed > 0 {
                anyhow::bail!("{failed} 个项目无法解析状态");
            }
        }
        Cmd::Logs {
            id, follow, lines, ..
        } => {
            if follow {
                loaded.follow_log(&id, lines)?;
            } else {
                let tail = loaded.log_tail(&id, lines)?;
                if tail.missing {
                    anyhow::bail!("log file missing: {}", tail.path);
                }
                print!("{}", tail.text);
                if !tail.text.is_empty() && !tail.text.ends_with('\n') {
                    println!();
                }
            }
        }
        Cmd::Open { id, dir, all } => {
            if dir {
                tongyun_dev_core::open_path(&loaded.root_dir(&id)?)?;
            } else if all {
                for link in loaded.page_urls(&id)? {
                    tongyun_dev_core::open_url(&link.url)?;
                }
            } else {
                tongyun_dev_core::open_url(&loaded.page_url(&id)?)?;
            }
        }
        Cmd::Terminal { id, profile } => {
            loaded.open_in_terminal(&id, profile.as_deref())?;
            println!("已在外部终端打开 `{id}`");
        }
        Cmd::Add {
            id,
            name,
            root,
            start,
            stop,
            port,
            login_url,
            health_url,
            log_dir,
            log_file,
            node,
            java,
            java_home,
            node_home,
            shell,
            foreground,
            no_auto_install,
            port_mode,
            port_arg,
            port_env,
            group,
            env,
            pty,
            auto_answers,
            links,
            default_profile,
            depends_on,
        } => {
            let row = loaded.add_project(NewProject {
                id,
                name,
                root,
                start,
                stop,
                port: Some(port),
                login_url,
                health_url,
                log_dir,
                log_file,
                node,
                java,
                java_home,
                node_home,
                shell,
                background: Some(!foreground),
                auto_install: no_auto_install.then_some(false),
                port_mode,
                port_arg,
                port_env,
                group,
                env: env.into_iter().collect(),
                pty,
                auto_answers,
                links,
                default_profile,
                depends_on,
                ..Default::default()
            })?;
            println!(
                "已注册项目 `{}`（{}），写入 {}",
                row.id,
                row.root,
                loaded.path().display()
            );
        }
        Cmd::Edit {
            id,
            env,
            pty,
            no_pty,
            auto_answers,
            clear_auto_answers,
            links,
            clear_links,
            default_profile,
            clear_default_profile,
            depends_on,
            clear_depends_on,
        } => {
            let mut edit = tongyun_dev_core::ProjectEdit::default();
            if !env.is_empty() {
                edit.env = Some(env.into_iter().collect());
            }
            if pty {
                edit.pty = Some(true);
            } else if no_pty {
                edit.pty = Some(false);
            }
            if clear_auto_answers {
                edit.auto_answers = Some(Vec::new());
            } else if !auto_answers.is_empty() {
                edit.auto_answers = Some(auto_answers);
            }
            if clear_links {
                edit.links = Some(Vec::new());
            } else if !links.is_empty() {
                edit.links = Some(links);
            }
            if clear_default_profile {
                edit.default_profile = Some(String::new());
            } else if let Some(dp) = default_profile {
                edit.default_profile = Some(dp);
            }
            if clear_depends_on {
                edit.depends_on = Some(Vec::new());
            } else if !depends_on.is_empty() {
                edit.depends_on = Some(depends_on);
            }
            let row = loaded.update_project(&id, edit)?;
            println!("已更新项目 `{}`，写入 {}", row.id, loaded.path().display());
        }
        Cmd::Input {
            id,
            text,
            no_enter,
            key,
            json: _,
        } => {
            let data = tongyun_dev_core::resolve_input_data(
                Some(text.as_str()),
                !no_enter,
                key.as_deref(),
            )?;
            loaded.send_input(&id, &data)?;
        }
        Cmd::Group { id, key, clear } => {
            let row = if clear {
                loaded.set_project_group(&id, None)?
            } else if let Some(ref g) = key {
                let trimmed = g.trim();
                if trimmed.is_empty() {
                    loaded.set_project_group(&id, None)?
                } else {
                    let existing_groups = loaded.groups();
                    let matched_key = existing_groups
                        .iter()
                        .find(|info| info.label == trimmed || info.key == trimmed)
                        .or_else(|| {
                            existing_groups.iter().find(|info| {
                                info.label.eq_ignore_ascii_case(trimmed)
                                    || info.key.eq_ignore_ascii_case(trimmed)
                            })
                        })
                        .map(|info| info.key.clone());

                    if let Some(ref k) = matched_key {
                        loaded.set_project_group(&id, Some(k))?
                    } else if tongyun_dev_core::validate_group_key(trimmed).is_ok() {
                        loaded.set_project_group(&id, Some(trimmed))?
                    } else {
                        anyhow::bail!(
                            "未找到分组 `{trimmed}`，请先使用 `tongyun-dev group-add <key> --label {trimmed}` 创建分组"
                        );
                    }
                }
            } else {
                anyhow::bail!("请指定分组 key，或使用 --clear 清空分组");
            };
            match row.group {
                Some(ref g) => println!(
                    "已将项目 `{id}` 分组设置为 `{g}`，写入 {}",
                    loaded.path().display()
                ),
                None => println!("已清空项目 `{id}` 的分组，写入 {}", loaded.path().display()),
            }
        }
        Cmd::GroupAdd { key, label } => {
            let group = loaded.create_group(&key, label.as_deref())?;
            println!(
                "已创建分组 `{}`（显示名：{}），写入 {}",
                group.key,
                group.label,
                loaded.path().display()
            );
        }
        Cmd::Remove { id } => {
            loaded.remove_project(&id)?;
            println!("已移除项目 `{id}`，更新 {}", loaded.path().display());
        }
        Cmd::Ui { port, no_open } => {
            let path = loaded.path().to_path_buf();
            drop(loaded);
            start_ui_server(path, port, !no_open)?;
        }
        Cmd::Scan { .. } => unreachable!("handled above"),
    }
    Ok(())
}

/// `scan`: works before any config exists. With `--apply` and no config yet, the
/// default config (~/.config/tongyun-dev/projects.toml) is created first.
fn run_scan(
    config: Option<&std::path::Path>,
    dir: &str,
    depth: usize,
    apply: bool,
    json: bool,
) -> Result<()> {
    let loaded = match tongyun_dev_core::load(config) {
        Ok(l) => l,
        Err(e) if config.is_none() => {
            let path =
                tongyun_dev_core::create_default_config().with_context(|| format!("{e:#}"))?;
            if !json {
                eprintln!("还没有配置文件，已新建 {}", path.display());
            }
            tongyun_dev_core::load(Some(&path))?
        }
        Err(e) => return Err(e),
    };
    let mut loaded = loaded;
    let report = loaded.scan(dir, depth)?;
    if !apply {
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_scan(&report);
            let new = report
                .candidates
                .iter()
                .filter(|c| c.existing_id.is_none())
                .count();
            if new > 0 {
                println!(
                    "\n加 --apply 把 {new} 个新项目写进 {}",
                    loaded.path().display()
                );
            }
        }
        return Ok(());
    }
    let items: Vec<NewProject> = report
        .candidates
        .iter()
        .filter(|c| c.existing_id.is_none())
        .map(|c| c.to_new_project())
        .collect();
    let out = loaded.import_scanned(items)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print_scan(&report);
        println!();
        for row in &out.added {
            println!("已添加 `{}`（{}）", row.id, row.root);
        }
        for s in &out.skipped {
            println!("跳过 `{}`：{}", s.id, s.reason);
        }
        println!("写入 {}", loaded.path().display());
    }
    Ok(())
}

fn print_scan(report: &ScanReport) {
    println!(
        "扫描 {}（深度 {}，看了 {} 个目录{}）：找到 {} 个项目",
        report.root,
        report.depth,
        report.dirs_visited,
        if report.truncated {
            "，目录太多已截断"
        } else {
            ""
        },
        report.candidates.len()
    );
    for c in &report.candidates {
        let port = match (c.port, c.port_guessed) {
            (0, _) => "端口?".to_string(),
            (p, true) => format!(":{p}(猜)"),
            (p, false) => format!(":{p}"),
        };
        let mut tool = String::new();
        if let Some(v) = &c.java {
            tool.push_str(&format!(" java {v}"));
        }
        if let Some(v) = &c.node {
            tool.push_str(&format!(" node {v}"));
        }
        let state = match &c.existing_id {
            Some(id) => format!("  [已注册为 {id}]"),
            None => String::new(),
        };
        println!(
            "\n  {}  {}  {:?}  {}{}{}\n    {}\n    启动：{}",
            c.id, c.label, c.confidence, port, tool, state, c.root, c.start
        );
        for w in &c.why {
            println!("    · {w}");
        }
    }
}

fn print_list(loaded: &Loaded, json: bool, with_groups: bool) -> Result<()> {
    let list = loaded.list().context("list projects")?;
    if json {
        if with_groups {
            #[derive(serde::Serialize)]
            struct ListWithGroups<'a> {
                projects: &'a [tongyun_dev_core::ProjectRow],
                groups: &'a [tongyun_dev_core::GroupInfo],
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&ListWithGroups {
                    projects: &list.projects,
                    groups: &list.groups,
                })?
            );
        } else {
            println!("{}", serde_json::to_string_pretty(&list.projects)?);
        }
    } else {
        for project in &list.projects {
            let mut line = format!("{}\t{}\t{}", project.id, project.name, project.root);
            if let Some(v) = &project.group {
                line.push_str(&format!("\tgroup {v}"));
            }
            if let Some(v) = &project.node {
                line.push_str(&format!("\tnode {v}"));
            }
            if let Some(v) = &project.java {
                line.push_str(&format!("\tjava {v}"));
            }
            if !project.enabled {
                line.push_str(&format!("\tdisabled\t{}", project.reason));
            }
            if let Some(err) = &project.resolve_error {
                line.push_str(&format!("\t!解析失败: {err}"));
            }
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_cli_arguments() {
        let cli = Cli::try_parse_from(["tongyun-dev", "list", "--json"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::List {
                json: true,
                with_groups: false
            })
        ));

        let cli = Cli::try_parse_from(["tongyun-dev", "list", "--json", "--with-groups"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::List {
                json: true,
                with_groups: true
            })
        ));

        let cli = Cli::try_parse_from(["tongyun-dev", "start", "my_api", "--open"]).unwrap();
        assert!(matches!(cli.cmd, Some(Cmd::Start { ref id, open: true, .. }) if id.as_deref() == Some("my_api")));

        let cli = Cli::try_parse_from(["tongyun-dev", "status", "my_api", "--json"]).unwrap();
        assert!(
            matches!(cli.cmd, Some(Cmd::Status { id: Some(ref id), json: true }) if id == "my_api")
        );

        let cli = Cli::try_parse_from(["tongyun-dev", "status"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Status {
                id: None,
                json: false
            })
        ));

        let cli = Cli::try_parse_from(["tongyun-dev", "install", "my_web"]).unwrap();
        assert!(matches!(cli.cmd, Some(Cmd::Install { ref id, .. }) if id == "my_web"));

        let cli = Cli::try_parse_from(["tongyun-dev"]).unwrap();
        assert!(
            cli.cmd.is_none(),
            "no subcommand falls back to the web console"
        );

        let cli =
            Cli::try_parse_from(["tongyun-dev", "ui", "--port", "9100", "--no-open"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Ui {
                port: 9100,
                no_open: true
            })
        ));
    }

    #[test]
    fn parses_scan_arguments() {
        let cli = Cli::try_parse_from(["tongyun-dev", "scan", "~/code"]).unwrap();
        assert!(
            matches!(cli.cmd, Some(Cmd::Scan { ref dir, depth: 3, apply: false, json: false }) if dir == "~/code")
        );
        let cli = Cli::try_parse_from([
            "tongyun-dev",
            "scan",
            ".",
            "--depth",
            "5",
            "--apply",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Scan {
                depth: 5,
                apply: true,
                json: true,
                ..
            })
        ));
    }

    #[test]
    fn add_accepts_toolchain_flags() {
        let cli = Cli::try_parse_from([
            "tongyun-dev",
            "add",
            "web",
            "--root",
            "~/code/web",
            "--start",
            "pnpm dev",
            "--port",
            "5173",
            "--node",
            "22",
            "--java",
            "17",
        ])
        .unwrap();
        match cli.cmd {
            Some(Cmd::Add {
                node,
                java,
                port,
                foreground,
                ..
            }) => {
                assert_eq!(node.as_deref(), Some("22"));
                assert_eq!(java.as_deref(), Some("17"));
                assert_eq!(port, 5173);
                assert!(!foreground);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn add_accepts_port_mode_flags() {
        let cli = Cli::try_parse_from([
            "tongyun-dev",
            "add",
            "web",
            "--root",
            "~/code/web",
            "--start",
            "pnpm dev",
            "--port",
            "5173",
            "--port-mode",
            "arg",
            "--port-arg",
            "-p",
            "--port-env",
            "VITE_PORT",
        ])
        .unwrap();
        match cli.cmd {
            Some(Cmd::Add {
                port_mode,
                port_arg,
                port_env,
                ..
            }) => {
                assert_eq!(port_mode.as_deref(), Some("arg"));
                assert_eq!(port_arg.as_deref(), Some("-p"));
                assert_eq!(port_env.as_deref(), Some("VITE_PORT"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn delegates_command_assembly_to_core() {
        let cmd = vec!["pnpm".to_string(), "dev".to_string()];
        let assembled = tongyun_dev_core::assemble_start_cmd(&cmd, Some("20"), None);
        assert_eq!(assembled, vec!["pnpm", "dev"]);

        let assembled_java = tongyun_dev_core::assemble_start_cmd(&cmd, None, Some("17"));
        assert_eq!(assembled_java, vec!["pnpm", "dev"]);
    }

    #[test]
    fn parses_group_arguments() {
        let cli = Cli::try_parse_from(["tongyun-dev", "group", "web", "demo"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Group {
                ref id,
                key: Some(ref g),
                clear: false
            }) if id == "web" && g == "demo"
        ));

        let cli = Cli::try_parse_from(["tongyun-dev", "group", "web", "--clear"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Group {
                ref id,
                key: None,
                clear: true
            }) if id == "web"
        ));

        let cli =
            Cli::try_parse_from(["tongyun-dev", "group-add", "demo", "--label", "Demo 业务线"])
                .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::GroupAdd {
                ref key,
                label: Some(ref l),
            }) if key == "demo" && l == "Demo 业务线"
        ));

        let cli = Cli::try_parse_from(["tongyun-dev", "group-add", "demo"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::GroupAdd {
                ref key,
                label: None,
            }) if key == "demo"
        ));

        let cli = Cli::try_parse_from([
            "tongyun-dev",
            "add",
            "web",
            "--root",
            "~/code/web",
            "--start",
            "pnpm dev",
            "--group",
            "frontend",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Add {
                ref id,
                group: Some(ref g),
                ..
            }) if id == "web" && g == "frontend"
        ));
    }

    #[test]
    fn parses_input_and_env_arguments() {
        let cli = Cli::try_parse_from([
            "tongyun-dev",
            "add",
            "web",
            "--root",
            "~/code/web",
            "--start",
            "pnpm dev",
            "--env",
            "CI=1",
            "--env",
            "APP_ENV=test",
        ])
        .unwrap();
        match cli.cmd {
            Some(Cmd::Add { env, .. }) => {
                assert_eq!(
                    env,
                    vec![
                        ("CI".to_string(), "1".to_string()),
                        ("APP_ENV".to_string(), "test".to_string())
                    ]
                );
            }
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from(["tongyun-dev", "input", "web", "hello"]).unwrap();
        match cli.cmd {
            Some(Cmd::Input {
                id,
                text,
                no_enter,
                key,
                json,
            }) => {
                assert_eq!(id, "web");
                assert_eq!(text, "hello");
                assert!(!no_enter);
                assert_eq!(key, None);
                assert!(!json);
            }
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "tongyun-dev",
            "input",
            "web",
            "--key",
            "up",
            "--no-enter",
            "--json",
        ])
        .unwrap();
        match cli.cmd {
            Some(Cmd::Input {
                id,
                text,
                no_enter,
                key,
                json,
            }) => {
                assert_eq!(id, "web");
                assert_eq!(text, "");
                assert!(no_enter);
                assert_eq!(key.as_deref(), Some("up"));
                assert!(json);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_profile_open_all_and_terminal_arguments() {
        // start with profile
        let cli = Cli::try_parse_from(["tongyun-dev", "start", "web", "--profile", "dev"]).unwrap();
        match cli.cmd {
            Some(Cmd::Start { id, profile, .. }) => {
                assert_eq!(id.as_deref(), Some("web"));
                assert_eq!(profile.as_deref(), Some("dev"));
            }
            other => panic!("unexpected {other:?}"),
        }

        // restart with profile
        let cli = Cli::try_parse_from(["tongyun-dev", "restart", "web", "--profile", "mock"]).unwrap();
        match cli.cmd {
            Some(Cmd::Restart { id, profile, .. }) => {
                assert_eq!(id.as_deref(), Some("web"));
                assert_eq!(profile.as_deref(), Some("mock"));
            }
            other => panic!("unexpected {other:?}"),
        }

        // open --all
        let cli = Cli::try_parse_from(["tongyun-dev", "open", "web", "--all"]).unwrap();
        match cli.cmd {
            Some(Cmd::Open { id, dir, all }) => {
                assert_eq!(id, "web");
                assert!(!dir);
                assert!(all);
            }
            other => panic!("unexpected {other:?}"),
        }

        // open --dir and --all conflict
        assert!(Cli::try_parse_from(["tongyun-dev", "open", "web", "--dir", "--all"]).is_err());

        // terminal with profile
        let cli = Cli::try_parse_from(["tongyun-dev", "terminal", "web", "--profile", "dev"]).unwrap();
        match cli.cmd {
            Some(Cmd::Terminal { id, profile }) => {
                assert_eq!(id, "web");
                assert_eq!(profile.as_deref(), Some("dev"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_batch_start_stop_restart_arguments() {
        // start --all
        let cli = Cli::try_parse_from(["tongyun-dev", "start", "--all"]).unwrap();
        match cli.cmd {
            Some(Cmd::Start { id, all, group, .. }) => {
                assert_eq!(id, None);
                assert!(all);
                assert_eq!(group, None);
            }
            other => panic!("unexpected {other:?}"),
        }

        // start --group
        let cli = Cli::try_parse_from(["tongyun-dev", "start", "--group", "backend"]).unwrap();
        match cli.cmd {
            Some(Cmd::Start { id, all, group, .. }) => {
                assert_eq!(id, None);
                assert!(!all);
                assert_eq!(group.as_deref(), Some("backend"));
            }
            other => panic!("unexpected {other:?}"),
        }

        // restart --all
        let cli = Cli::try_parse_from(["tongyun-dev", "restart", "--all"]).unwrap();
        match cli.cmd {
            Some(Cmd::Restart { id, all, group, .. }) => {
                assert_eq!(id, None);
                assert!(all);
                assert_eq!(group, None);
            }
            other => panic!("unexpected {other:?}"),
        }

        // stop --group
        let cli = Cli::try_parse_from(["tongyun-dev", "stop", "--group", "backend", "--json"]).unwrap();
        match cli.cmd {
            Some(Cmd::Stop { id, all, group, json }) => {
                assert_eq!(id, None);
                assert!(!all);
                assert_eq!(group.as_deref(), Some("backend"));
                assert!(json);
            }
            other => panic!("unexpected {other:?}"),
        }

        // conflict: id and --all
        assert!(Cli::try_parse_from(["tongyun-dev", "start", "web", "--all"]).is_err());

        // conflict: --all and --group
        assert!(Cli::try_parse_from(["tongyun-dev", "stop", "--all", "--group", "backend"]).is_err());
    }

    #[test]
    fn parses_add_and_edit_links_profiles_depends_on() {
        let add_cli = Cli::try_parse_from([
            "tongyun-dev",
            "add",
            "demo_web",
            "--root",
            "~/code/demo_web",
            "--start",
            "npm run dev",
            "--link",
            "Docs=http://localhost:3000/docs",
            "--link",
            "http://localhost:3000/api",
            "--default-profile",
            "dev",
            "--depends-on",
            "demo_api",
            "--depends-on",
            "demo_db",
        ])
        .unwrap();

        match add_cli.cmd {
            Some(Cmd::Add {
                id,
                links,
                default_profile,
                depends_on,
                ..
            }) => {
                assert_eq!(id, "demo_web");
                assert_eq!(links.len(), 2);
                assert_eq!(links[0].title, "Docs");
                assert_eq!(links[0].url, "http://localhost:3000/docs");
                assert_eq!(links[1].title, "http://localhost:3000/api");
                assert_eq!(links[1].url, "http://localhost:3000/api");
                assert_eq!(default_profile.as_deref(), Some("dev"));
                assert_eq!(depends_on, vec!["demo_api", "demo_db"]);
            }
            other => panic!("unexpected {other:?}"),
        }

        let edit_cli = Cli::try_parse_from([
            "tongyun-dev",
            "edit",
            "demo_web",
            "--link",
            "http://localhost:3000",
            "--default-profile",
            "mock",
            "--depends-on",
            "demo_svc",
        ])
        .unwrap();

        match edit_cli.cmd {
            Some(Cmd::Edit {
                id,
                links,
                clear_links,
                default_profile,
                clear_default_profile,
                depends_on,
                clear_depends_on,
                ..
            }) => {
                assert_eq!(id, "demo_web");
                assert_eq!(links.len(), 1);
                assert!(!clear_links);
                assert_eq!(default_profile.as_deref(), Some("mock"));
                assert!(!clear_default_profile);
                assert_eq!(depends_on, vec!["demo_svc"]);
                assert!(!clear_depends_on);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
