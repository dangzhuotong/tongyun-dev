//! tongyun-dev CLI. Project operations live in `tongyun-dev-core`; the web
//! console (`ui`) and the desktop shell call the same core.
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tongyun_dev_cli::server::{start_ui_server, DEFAULT_UI_PORT};
use tongyun_dev_core::{Loaded, NewProject, StatusJson, DEFAULT_ADD_PORT};

#[derive(Parser, Debug)]
#[command(
    name = "tongyun-dev",
    about = "Local project runner (start/stop/status/logs/open/add/remove/ui). No subcommand = web console on :9099"
)]
struct Cli {
    /// Config file (default: ./config/projects.toml or $TONGYUN_DEV_CONFIG)
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
    },
    /// Start a project and wait until the login page / port responds
    Start {
        id: String,
        /// Open the project page in the browser after starting
        #[arg(long)]
        open: bool,
    },
    /// Stop a project and wait until its port is free
    Stop { id: String },
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
        #[arg(long, default_value_t = 80)]
        lines: usize,
    },
    /// Open the project page in the browser (or the project folder with --dir)
    Open {
        id: String,
        #[arg(long)]
        dir: bool,
    },
    /// Register a new project in config/projects.toml (comments are kept)
    Add {
        /// Project id (letters, digits, `_`, `-`; e.g. acme_frontend)
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
    },
    /// Remove a registered project (project files are not touched)
    Remove { id: String },
    /// Start the web console and open it in the browser
    Ui {
        #[arg(long, default_value_t = DEFAULT_UI_PORT)]
        port: u16,
        /// Do not open the browser
        #[arg(long)]
        no_open: bool,
    },
}

fn print_status(st: &StatusJson, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(st)?);
    } else {
        println!(
            "{id}  running={running}  pid={pid}  port={port} open={open}  login={code:?} ok={ok}",
            id = st.id,
            running = st.running,
            pid = st.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
            port = st.port,
            open = st.port_open,
            code = st.login_probe.http_code,
            ok = st.login_probe.ok
        );
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
    let mut loaded = tongyun_dev_core::load(cli.config.as_deref())?;
    let cmd = cli.cmd.unwrap_or(Cmd::Ui {
        port: DEFAULT_UI_PORT,
        no_open: false,
    });
    match cmd {
        Cmd::List { json } => print_list(&loaded, json)?,
        Cmd::Start { id, open } => match loaded.start(&id) {
            Ok(st) => {
                print_status(&st, false)?;
                if open {
                    if let Err(e) = loaded
                        .page_url(&id)
                        .and_then(|u| tongyun_dev_core::open_url(&u))
                    {
                        eprintln!("could not open page: {e:#}");
                    }
                }
            }
            Err(e) => {
                if let Ok(st) = loaded.status(&id) {
                    print_status(&st, false)?;
                }
                return Err(e);
            }
        },
        Cmd::Stop { id } => {
            loaded.stop(&id)?;
            print_status(&loaded.status(&id)?, false)?;
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
        Cmd::Logs { id, follow, lines } => {
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
        Cmd::Open { id, dir } => {
            if dir {
                tongyun_dev_core::open_path(&loaded.root_dir(&id)?)?;
            } else {
                tongyun_dev_core::open_url(&loaded.page_url(&id)?)?;
            }
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
            })?;
            println!(
                "已注册项目 `{}`（{}），写入 {}",
                row.id,
                row.root,
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
    }
    Ok(())
}

fn print_list(loaded: &Loaded, json: bool) -> Result<()> {
    let list = loaded.list().context("list projects")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&list.projects)?);
    } else {
        for project in &list.projects {
            let mut line = format!("{}\t{}\t{}", project.id, project.name, project.root);
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
        assert!(matches!(cli.cmd, Some(Cmd::List { json: true })));

        let cli = Cli::try_parse_from(["tongyun-dev", "start", "legacy_app", "--open"]).unwrap();
        assert!(matches!(cli.cmd, Some(Cmd::Start { ref id, open: true }) if id == "legacy_app"));

        let cli = Cli::try_parse_from(["tongyun-dev", "status", "legacy_app", "--json"]).unwrap();
        assert!(
            matches!(cli.cmd, Some(Cmd::Status { id: Some(ref id), json: true }) if id == "legacy_app")
        );

        let cli = Cli::try_parse_from(["tongyun-dev", "status"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Status {
                id: None,
                json: false
            })
        ));

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
    fn delegates_command_assembly_to_core() {
        let cmd = vec!["pnpm".to_string(), "dev".to_string()];
        let assembled = tongyun_dev_core::assemble_start_cmd(&cmd, Some("20"), None);
        assert_eq!(assembled, vec!["pnpm", "dev"]);

        let assembled_java = tongyun_dev_core::assemble_start_cmd(&cmd, None, Some("17"));
        assert_eq!(assembled_java, vec!["pnpm", "dev"]);
    }
}
