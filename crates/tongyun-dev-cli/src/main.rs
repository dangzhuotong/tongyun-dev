//! tongyun-dev — local project start/stop/status/logs

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use tongyun_dev_cli::{
    apply_env_overrides, load_config_file, logs_project, resolve_config_path, resolve_project,
    start_project, status_of, stop_project, FileConfig, ListEntry, StatusJson, StopOptions,
    SystemProbe,
};

#[derive(Parser, Debug)]
#[command(
    name = "tongyun-dev",
    version,
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
    /// Start a project
    Start { id: String },
    /// Stop a project
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

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn load(cli: &Cli) -> Result<FileConfig> {
    let cwd = std::env::current_dir().context("current directory")?;
    let env_cfg = std::env::var("TONGYUN_DEV_CONFIG").ok();
    let path = resolve_config_path(cli.config.as_deref(), env_cfg.as_deref(), &cwd)?;
    let mut cfg = load_config_file(&path)?;
    let env: Vec<(String, String)> = std::env::vars().collect();
    apply_env_overrides(&mut cfg, &env);
    Ok(cfg)
}

fn project_by_id(
    cfg: &FileConfig,
    id: &str,
    home: &Path,
) -> Result<tongyun_dev_cli::ResolvedProject> {
    let project = cfg
        .projects
        .get(id)
        .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))?;
    Ok(resolve_project(id, project, home))
}

fn print_status(status: &StatusJson, json: bool) -> Result<()> {
    if json {
        println!("{}", status.to_json_pretty()?);
        return Ok(());
    }
    let pid = status
        .pid
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    println!(
        "{}  running={}  pid={pid}  port={} open={}  login={:?} ok={}",
        status.id,
        status.running,
        status.port,
        status.port_open,
        status.login_probe.http_code,
        status.login_probe.ok
    );
    println!("  log {}", status.log_path);
    println!("  pidfile {}", status.pid_path);
    println!("  root {}", status.root);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = load(&cli)?;
    let home = home_dir();
    let probe = SystemProbe;
    match cli.cmd {
        Cmd::List { json } => {
            let entries: Vec<ListEntry> = cfg
                .projects
                .iter()
                .map(|(id, project)| {
                    let resolved = resolve_project(id, project, &home);
                    ListEntry {
                        id: id.clone(),
                        name: project.name.clone(),
                        root: resolved.root.display().to_string(),
                    }
                })
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for entry in &entries {
                    println!("{}\t{}\t{}", entry.id, entry.name, entry.root);
                }
            }
        }
        Cmd::Start { id } => {
            let project = project_by_id(&cfg, &id, &home)?;
            start_project(&project)?;
            print_status(&status_of(&project, &probe), false)?;
        }
        Cmd::Stop { id } => {
            let project = project_by_id(&cfg, &id, &home)?;
            stop_project(&project, &probe, StopOptions::default())?;
            print_status(&status_of(&project, &probe), false)?;
        }
        Cmd::Status { id, json } => {
            let project = project_by_id(&cfg, &id, &home)?;
            print_status(&status_of(&project, &probe), json)?;
        }
        Cmd::Logs { id, follow, lines } => {
            let project = project_by_id(&cfg, &id, &home)?;
            logs_project(&project, follow, lines)?;
        }
    }
    Ok(())
}
