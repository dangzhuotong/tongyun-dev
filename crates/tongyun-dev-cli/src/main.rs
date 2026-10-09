//! tongyun-dev — local project runner (start/stop/status/logs/open/ui)
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use tongyun_dev_cli::config::{expand_user, get_proj, load_config, save_config, Project};
use tongyun_dev_cli::runner::{cmd_logs, cmd_start, cmd_stop, open_path, open_url, status_of, StatusJson};
use tongyun_dev_cli::server::start_ui_server;

#[derive(Parser, Debug)]
#[command(name = "tongyun-dev", about = "Local project runner (start/stop/status/logs/open/ui)")]
struct Cli {
    /// Config file (default: ./config/projects.toml or $TONGYUN_DEV_CONFIG)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List registered projects
    List {
        #[arg(long)]
        json: bool,
    },
    /// Start a project
    Start {
        id: String,
        /// Open login page in browser after starting
        #[arg(long)]
        open: bool,
    },
    /// Stop a project
    Stop { id: String },
    /// Show status (process + port + login probe). If ID is omitted, shows all projects.
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
    /// Open project login/home page in default system browser, or open directory
    Open {
        id: String,
        /// Open project folder in file explorer instead of browser
        #[arg(long)]
        dir: bool,
    },
    /// Register a new project
    Add {
        /// Project ID (e.g. acme_frontend)
        id: String,
        /// Project name (defaults to ID if omitted)
        #[arg(long)]
        name: Option<String>,
        /// Root directory of the project
        #[arg(long)]
        root: String,
        /// Start command (e.g. "npm run dev")
        #[arg(long)]
        start: String,
        /// Stop command
        #[arg(long)]
        stop: Option<String>,
        /// Port number
        #[arg(long, default_value_t = 8080)]
        port: u16,
        /// Login / entry URL
        #[arg(long)]
        login_url: Option<String>,
        /// Log file path
        #[arg(long)]
        log_file: Option<String>,
    },
    /// Remove a registered project
    Remove { id: String },
    /// Start web control panel and open in browser
    Ui {
        /// Port to bind UI server
        #[arg(long, default_value_t = 9099)]
        port: u16,
        /// Do not automatically open browser
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
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let (cfg_path, mut cfg) = load_config(cli.config.as_deref())?;

    let cmd = cli.cmd.unwrap_or(Cmd::Ui {
        port: 9099,
        no_open: false,
    });

    match cmd {
        Cmd::List { json } => {
            let mut ids: Vec<_> = cfg.projects.keys().cloned().collect();
            ids.sort();
            if json {
                let arr: Vec<_> = ids
                    .iter()
                    .map(|id| {
                        let p = &cfg.projects[id];
                        serde_json::json!({"id": id, "name": p.name, "root": expand_user(&p.root)})
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
            } else {
                for id in ids {
                    let p = &cfg.projects[&id];
                    println!("{id}\t{}\t{}", p.name, expand_user(&p.root).display());
                }
            }
        }
        Cmd::Start { id, open } => {
            let p = get_proj(&cfg, &id)?;
            cmd_start(&id, p)?;
            let st = status_of(&id, p);
            print_status(&st, false)?;
            if open {
                let target = if !p.login_url.is_empty() {
                    &p.login_url
                } else {
                    &format!("http://127.0.0.1:{}", p.port)
                };
                let _ = open_url(target);
            }
        }
        Cmd::Stop { id } => {
            let p = get_proj(&cfg, &id)?;
            cmd_stop(&id, p)?;
            let st = status_of(&id, p);
            print_status(&st, false)?;
        }
        Cmd::Status { id, json } => {
            match id {
                Some(id) => {
                    let p = get_proj(&cfg, &id)?;
                    let st = status_of(&id, p);
                    print_status(&st, json)?;
                }
                None => {
                    let mut ids: Vec<_> = cfg.projects.keys().cloned().collect();
                    ids.sort();
                    let list: Vec<StatusJson> = ids
                        .iter()
                        .map(|id| {
                            let p = &cfg.projects[id];
                            status_of(id, p)
                        })
                        .collect();
                    if json {
                        println!("{}", serde_json::to_string_pretty(&list)?);
                    } else {
                        for st in list {
                            print_status(&st, false)?;
                            println!();
                        }
                    }
                }
            }
        }
        Cmd::Logs { id, follow, lines } => {
            let p = get_proj(&cfg, &id)?;
            cmd_logs(p, follow, lines)?;
        }
        Cmd::Open { id, dir } => {
            let p = get_proj(&cfg, &id)?;
            if dir {
                let root_path = expand_user(&p.root);
                open_path(&root_path)?;
            } else {
                let target = if !p.login_url.is_empty() {
                    &p.login_url
                } else {
                    &format!("http://127.0.0.1:{}", p.port)
                };
                open_url(target)?;
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
            log_file,
        } => {
            let name = name.unwrap_or_else(|| id.clone());
            let login_url = login_url.unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
            let log_file = log_file.unwrap_or_default();
            let start_cmd: Vec<String> = if cfg!(windows) {
                vec!["cmd".into(), "/c".into(), start]
            } else {
                vec!["bash".into(), "-lc".into(), start]
            };
            let stop_cmd: Vec<String> = stop
                .map(|s| {
                    if cfg!(windows) {
                        vec!["cmd".into(), "/c".into(), s]
                    } else {
                        vec!["bash".into(), "-lc".into(), s]
                    }
                })
                .unwrap_or_default();

            let new_proj = Project {
                name,
                root,
                start: start_cmd,
                stop: stop_cmd,
                tomcat: String::new(),
                log_dir: String::new(),
                log_file,
                pid_file: String::new(),
                port,
                login_url,
            };
            cfg.projects.insert(id.clone(), new_proj);
            save_config(&cfg_path, &cfg)?;
            println!("已成功注册项目 `{id}`，并保存到 {}", cfg_path.display());
        }
        Cmd::Remove { id } => {
            if cfg.projects.remove(&id).is_some() {
                save_config(&cfg_path, &cfg)?;
                println!("已成功移除项目 `{id}`，并更新 {}", cfg_path.display());
            } else {
                eprintln!("未找到项目 `{id}`");
            }
        }
        Cmd::Ui { port, no_open } => {
            start_ui_server(cfg_path, cfg, port, !no_open)?;
        }
    }
    Ok(())
}
