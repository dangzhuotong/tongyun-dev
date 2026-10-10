//! tongyun-dev CLI. Project operations live in `tongyun-dev-core`.
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tongyun_dev_core::{Loaded, StatusJson};

#[derive(Parser, Debug)]
#[command(
    name = "tongyun-dev",
    about = "Local project runner (start/stop/status/logs)"
)]
struct Cli {
    /// Config file (default: ./config/projects.toml or $TONGYUN_DEV_CONFIG)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// List registered projects
    List {
        #[arg(long)]
        json: bool,
    },
    /// Start a project and wait until the login page responds
    Start { id: String },
    /// Stop a project and wait until its port is free
    Stop { id: String },
    /// Show status (process + port + login probe)
    Status {
        id: String,
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
    let loaded = tongyun_dev_core::load(cli.config.as_deref())?;
    match cli.cmd {
        Cmd::List { json } => print_list(&loaded, json)?,
        Cmd::Start { id } => match loaded.start(&id) {
            Ok(st) => print_status(&st, false)?,
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
        Cmd::Status { id, json } => print_status(&loaded.status(&id)?, json)?,
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
    }
    Ok(())
}

fn print_list(loaded: &Loaded, json: bool) -> Result<()> {
    let list = loaded.list().context("list projects")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&list.projects)?);
    } else {
        for project in &list.projects {
            if project.enabled {
                println!("{}\t{}\t{}", project.id, project.name, project.root);
            } else {
                println!(
                    "{}\t{}\t{}\tdisabled\t{}",
                    project.id, project.name, project.root, project.reason
                );
            }
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
        assert!(matches!(cli.cmd, Cmd::List { json: true }));

        let cli = Cli::try_parse_from(["tongyun-dev", "start", "legacy_app"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Start { ref id } if id == "legacy_app"));

        let cli = Cli::try_parse_from(["tongyun-dev", "status", "legacy_app", "--json"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Status { ref id, json: true } if id == "legacy_app"));
    }

    #[test]
    fn delegates_command_assembly_to_core() {
        let cmd = vec!["pnpm".to_string(), "dev".to_string()];
        let assembled = tongyun_dev_core::assemble_start_cmd(&cmd, Some("20"), None);
        assert_eq!(
            assembled,
            vec!["fnm", "exec", "--using=20", "--", "pnpm", "dev"]
        );

        let assembled_java = tongyun_dev_core::assemble_start_cmd(&cmd, None, Some("17"));
        assert_eq!(
            assembled_java,
            vec![
                "bash",
                "-lc",
                "sdk use java 17 && exec \"$@\"",
                "--",
                "pnpm",
                "dev"
            ]
        );
    }
}
