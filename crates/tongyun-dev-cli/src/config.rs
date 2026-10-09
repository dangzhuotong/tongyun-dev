use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileConfig {
    pub projects: HashMap<String, Project>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub root: String,
    pub start: Vec<String>,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(default)]
    pub tomcat: String,
    #[serde(default)]
    pub log_dir: String,
    #[serde(default)]
    pub log_file: String,
    #[serde(default)]
    pub pid_file: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub login_url: String,
}

fn default_port() -> u16 {
    8080
}

pub fn expand_user(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/"))
            .join(rest)
    } else if p == "~" {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    } else {
        PathBuf::from(p)
    }
}

pub fn render(s: &str, proj: &Project) -> String {
    let root = expand_user(&proj.root).display().to_string();
    let tomcat = expand_user(&proj.tomcat).display().to_string();
    let log_dir = expand_user(&proj.log_dir).display().to_string();
    s.replace("{{root}}", &root)
        .replace("{{tomcat}}", &tomcat)
        .replace("{{log_dir}}", &log_dir)
}

pub fn find_default_config_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("TONGYUN_DEV_CONFIG") {
        return Ok(PathBuf::from(p));
    }
    let mut cur = std::env::current_dir()?;
    loop {
        let cand = cur.join("config/projects.toml");
        if cand.is_file() {
            return Ok(cand);
        }
        if !cur.pop() {
            bail!("config/projects.toml not found; pass --config or set TONGYUN_DEV_CONFIG");
        }
    }
}

pub fn load_config(explicit_path: Option<&Path>) -> Result<(PathBuf, FileConfig)> {
    let path = match explicit_path {
        Some(p) => p.to_path_buf(),
        None => find_default_config_path()?,
    };
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let cfg: FileConfig = toml::from_str(&text).context("parse projects.toml")?;
    Ok((path, cfg))
}

pub fn save_config(path: &Path, cfg: &FileConfig) -> Result<()> {
    let text = toml::to_string_pretty(cfg).context("serialize projects.toml")?;
    fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn get_proj<'a>(cfg: &'a FileConfig, id: &str) -> Result<&'a Project> {
    cfg.projects
        .get(id)
        .with_context(|| format!("unknown project id `{id}`; try `tongyun-dev list`"))
}
