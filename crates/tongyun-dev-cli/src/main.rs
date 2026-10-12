//! tongyun-dev CLI. Project operations live in `tongyun-dev-core`; the web
//! console (`ui`) and the desktop shell call the same core.
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;
use tongyun_dev_cli::server::{start_ui_server, DEFAULT_UI_PORT};
use tongyun_dev_core::{
    Loaded, NewProject, ScanReport, StatusJson, DEFAULT_ADD_PORT, DEFAULT_SCAN_DEPTH,
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
        id: String,
        /// Profile name to launch with (e.g. dev, prod)
        #[arg(long)]
        profile: Option<String>,
        /// Open the project page in the browser after starting
        #[arg(long, conflicts_with = "json")]
        open: bool,
        #[arg(long)]
        json: bool,
        /// Wait budget in seconds after launching; timeout leaves the service running
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: Option<u64>,
    },
    /// Stop, start again and wait for readiness
    Restart {
        id: String,
        /// Profile name to launch with (e.g. dev, prod)
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, conflicts_with = "json")]
        open: bool,
        #[arg(long)]
        json: bool,
        /// Wait budget in seconds after launching; timeout leaves the service running
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: Option<u64>,
    },
    /// Stop a project and wait until its port is free
    Stop {
        id: String,
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

// Existing list/status JSON keeps its original shape. Operations use a stable envelope.
fn machine_operation(cmd: &Cmd) -> Option<(&str, &str)> {
    match cmd {
        Cmd::Start { id, json: true, .. } => Some(("start", id)),
        Cmd::Restart { id, json: true, .. } => Some(("restart", id)),
        Cmd::Stop { id, json: true } => Some(("stop", id)),
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
            Cmd::Start { timeout, profile, .. } => {
                loaded.start_with_profile(id, profile.as_deref(), timeout.map(Duration::from_secs))?
            }
            Cmd::Restart { timeout, profile, .. } => {
                loaded.restart_with_profile(id, profile.as_deref(), timeout.map(Duration::from_secs))?
            }
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
            ref profile,
            open,
            timeout,
            ..
        }
        | Cmd::Restart {
            ref id,
            ref profile,
            open,
            timeout,
            ..
        } => {
            match if matches!(cmd, Cmd::Restart { .. }) {
                loaded.restart_with_profile(id, profile.as_deref(), timeout.map(Duration::from_secs))
            } else {
                loaded.start_with_profile(id, profile.as_deref(), timeout.map(Duration::from_secs))
            } {
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
        Cmd::Stop { id, .. } => {
            loaded.stop(&id)?;
            print_status(&loaded.status(&id)?, false)?;
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
            let mut all = Vec::with_capacity(list.projects.len());
            for row in &list.projects {
                all.push(loaded.status(&row.id)?);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&all)?);
            } else {
                for st in &all {
                    print_status(st, false)?;
                    println!();
                }
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
        assert!(matches!(cli.cmd, Some(Cmd::Start { ref id, open: true, .. }) if id == "my_api"));

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
}
