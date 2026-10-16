use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Mac,
    Linux,
    Windows,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            HostOs::Mac
        } else if cfg!(target_os = "windows") {
            HostOs::Windows
        } else {
            HostOs::Linux
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolchainRoots {
    pub os: HostOs,
    pub home: Option<PathBuf>,
    pub sdkman_dir: Option<PathBuf>,
    pub fnm_dir: Option<PathBuf>,
    pub nvm_dir: Option<PathBuf>,
    pub program_files: Vec<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub nvm_home: Option<PathBuf>,
    pub base_path: String,
    pub common_dirs: Option<Vec<PathBuf>>,
    pub mac_jvm_dir: Option<PathBuf>,
}

impl ToolchainRoots {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let sdkman_dir = std::env::var("SDKMAN_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let fnm_dir = std::env::var("FNM_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let nvm_dir = std::env::var("NVM_DIR")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let mut program_files = Vec::new();
        for key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Ok(v) = std::env::var(key) {
                let trimmed = v.trim();
                if !trimmed.is_empty() {
                    let pb = PathBuf::from(trimmed);
                    if !program_files.contains(&pb) {
                        program_files.push(pb);
                    }
                }
            }
        }

        let appdata = std::env::var("APPDATA")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let nvm_home = std::env::var("NVM_HOME")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let base_path = std::env::var("PATH").unwrap_or_default();

        Self {
            os: HostOs::current(),
            home,
            sdkman_dir,
            fnm_dir,
            nvm_dir,
            program_files,
            appdata,
            nvm_home,
            base_path,
            common_dirs: None,
            mac_jvm_dir: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolchainError {
    MissingJava { version: String, searched: String },
    InvalidJavaHome { path: PathBuf, reason: String },
    MissingNode { version: String, searched: String },
    InvalidNodeHome { path: PathBuf, reason: String },
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolchainError::MissingJava { version, searched } => {
                let target = if version == "8" || version == "1.8" {
                    "8.*"
                } else {
                    &format!("{version}.*")
                };
                write!(f, "缺 Java {version}：未在 {searched} 找到 {target}")
            }
            ToolchainError::InvalidJavaHome { path, reason } => {
                write!(f, "指定的 java_home 无效（{}）：{reason}", path.display())
            }
            ToolchainError::MissingNode { version, searched } => {
                let clean_ver = version.strip_prefix('v').unwrap_or(version);
                write!(
                    f,
                    "缺 Node {clean_ver}：未在 {searched} 找到 v{clean_ver}.*"
                )
            }
            ToolchainError::InvalidNodeHome { path, reason } => {
                write!(f, "指定的 node_home 无效（{}）：{reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ToolchainError {}

pub fn format_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(h) = home {
        if let Ok(rel) = path.strip_prefix(h) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if rel_str.is_empty() {
                return "~".to_string();
            } else {
                return format!("~/{rel_str}");
            }
        }
    }
    path.to_string_lossy().replace('\\', "/")
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
enum VersionPart {
    Numeric(u64),
    Text(String),
}

fn parse_version_key(s: &str) -> Vec<VersionPart> {
    let mut parts = Vec::new();
    let mut current_digits = String::new();
    let mut current_chars = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            if !current_chars.is_empty() {
                parts.push(VersionPart::Text(std::mem::take(&mut current_chars)));
            }
            current_digits.push(c);
        } else if c.is_ascii_alphabetic() {
            if !current_digits.is_empty() {
                if let Ok(n) = current_digits.parse::<u64>() {
                    parts.push(VersionPart::Numeric(n));
                }
                current_digits.clear();
            }
            current_chars.push(c.to_ascii_lowercase());
        } else {
            if !current_digits.is_empty() {
                if let Ok(n) = current_digits.parse::<u64>() {
                    parts.push(VersionPart::Numeric(n));
                }
                current_digits.clear();
            }
            if !current_chars.is_empty() {
                parts.push(VersionPart::Text(std::mem::take(&mut current_chars)));
            }
        }
    }
    if !current_digits.is_empty() {
        if let Ok(n) = current_digits.parse::<u64>() {
            parts.push(VersionPart::Numeric(n));
        }
    }
    if !current_chars.is_empty() {
        parts.push(VersionPart::Text(current_chars));
    }
    parts
}

fn java_version_matches(dir_name: &str, ver: &str) -> bool {
    if dir_name == "current" || dir_name.starts_with('.') {
        return false;
    }
    let norm_ver = if ver == "1.8" { "8" } else { ver };
    if dir_name == ver || dir_name == norm_ver {
        return true;
    }
    if norm_ver == "8" {
        dir_name.starts_with("8.")
            || dir_name.starts_with("8-")
            || dir_name.starts_with("8_")
            || dir_name.starts_with("8+")
            || dir_name.starts_with("1.8.")
            || dir_name.starts_with("1.8-")
            || dir_name.starts_with("1.8_")
            || dir_name.starts_with("1.8+")
            || dir_name == "1.8"
    } else {
        dir_name.starts_with(&format!("{norm_ver}."))
            || dir_name.starts_with(&format!("{norm_ver}-"))
            || dir_name.starts_with(&format!("{norm_ver}_"))
            || dir_name.starts_with(&format!("{norm_ver}+"))
    }
}

fn mac_jvm_matches(dir_name: &str, ver: &str) -> bool {
    let lower = dir_name.to_ascii_lowercase();
    let norm_ver = if ver == "1.8" { "8" } else { ver };
    if norm_ver == "8" {
        lower.contains("-8.")
            || lower.contains("-8-")
            || lower.contains("-8_")
            || lower.contains("jdk-8")
            || lower.contains("jdk8")
            || lower.contains("zulu-8")
            || lower.contains("1.8")
    } else {
        lower.contains(&format!("-{norm_ver}."))
            || lower.contains(&format!("-{norm_ver}-"))
            || lower.contains(&format!("-{norm_ver}_"))
            || lower.contains(&format!("jdk-{norm_ver}"))
            || lower.contains(&format!("jdk{norm_ver}"))
    }
}

fn win_java_matches(vendor: &str, dir_name: &str, ver: &str) -> bool {
    let lower = dir_name.to_ascii_lowercase();
    let norm_ver = if ver == "1.8" { "8" } else { ver };
    match vendor {
        "Zulu" => {
            if norm_ver == "8" {
                lower.starts_with("zulu-8") || lower.starts_with("zulu8")
            } else {
                lower.starts_with(&format!("zulu-{norm_ver}"))
                    || lower.starts_with(&format!("zulu{norm_ver}"))
            }
        }
        "Eclipse Adoptium" => {
            if norm_ver == "8" {
                lower.starts_with("jdk-8") || lower.starts_with("jdk8")
            } else {
                lower.starts_with(&format!("jdk-{norm_ver}"))
                    || lower.starts_with(&format!("jdk{norm_ver}"))
            }
        }
        "Java" => {
            if norm_ver == "8" {
                lower.starts_with("jdk1.8")
                    || lower.starts_with("jdk-8")
                    || lower.starts_with("jdk8")
            } else {
                lower.starts_with(&format!("jdk-{norm_ver}"))
                    || lower.starts_with(&format!("jdk{norm_ver}"))
            }
        }
        _ => false,
    }
}

fn check_java_bin(home: &Path) -> bool {
    let bin = home.join("bin");
    bin.join("java").is_file() || bin.join("java.exe").is_file()
}

pub fn resolve_java_home(
    ver: &str,
    explicit_java_home: Option<&Path>,
    roots: &ToolchainRoots,
) -> Result<PathBuf, ToolchainError> {
    if let Some(explicit) = explicit_java_home {
        if check_java_bin(explicit) {
            return Ok(explicit.to_path_buf());
        }
        return Err(ToolchainError::InvalidJavaHome {
            path: explicit.to_path_buf(),
            reason: "bin/java 未找到".into(),
        });
    }

    match roots.os {
        HostOs::Mac | HostOs::Linux => {
            let sdkman_java = if let Some(ref d) = roots.sdkman_dir {
                d.join("candidates/java")
            } else if let Some(ref h) = roots.home {
                h.join(".sdkman/candidates/java")
            } else {
                PathBuf::from(".sdkman/candidates/java")
            };

            let mut candidates = Vec::new();
            if sdkman_java.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&sdkman_java) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            if java_version_matches(&name, ver) {
                                candidates.push((parse_version_key(&name), path));
                            }
                        }
                    }
                }
            }

            if !candidates.is_empty() {
                candidates.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                let (_, best_path) = candidates.pop().unwrap();
                let contents_home = best_path.join("Contents/Home");
                if contents_home.is_dir() {
                    return Ok(contents_home);
                }
                return Ok(best_path);
            }

            if roots.os == HostOs::Mac {
                let mac_jvm = roots
                    .mac_jvm_dir
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("/Library/Java/JavaVirtualMachines"));
                let mut mac_candidates = Vec::new();
                if mac_jvm.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&mac_jvm) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                if mac_jvm_matches(&name, ver) {
                                    let home_dir = path.join("Contents/Home");
                                    if home_dir.is_dir() {
                                        mac_candidates.push((parse_version_key(&name), home_dir));
                                    }
                                }
                            }
                        }
                    }
                }
                if !mac_candidates.is_empty() {
                    mac_candidates.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                    let (_, best_path) = mac_candidates.pop().unwrap();
                    return Ok(best_path);
                }
            }

            let searched = format_home(&sdkman_java, roots.home.as_deref());
            Err(ToolchainError::MissingJava {
                version: ver.to_string(),
                searched,
            })
        }
        HostOs::Windows => {
            let mut candidates = Vec::new();
            let mut searched_dirs = Vec::new();

            for pf in &roots.program_files {
                for vendor in ["Zulu", "Eclipse Adoptium", "Java"] {
                    let vendor_dir = pf.join(vendor);
                    searched_dirs.push(vendor_dir.to_string_lossy().into_owned());
                    if vendor_dir.is_dir() {
                        if let Ok(entries) = std::fs::read_dir(&vendor_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_dir() {
                                    let name = entry.file_name().to_string_lossy().into_owned();
                                    if win_java_matches(vendor, &name, ver)
                                        && (check_java_bin(&path) || path.join("bin").is_dir())
                                    {
                                        candidates.push((parse_version_key(&name), path.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !candidates.is_empty() {
                candidates.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                let (_, best_path) = candidates.pop().unwrap();
                return Ok(best_path);
            }

            let searched = if searched_dirs.is_empty() {
                "Program Files (Zulu, Eclipse Adoptium, Java)".to_string()
            } else {
                searched_dirs.join(", ")
            };
            Err(ToolchainError::MissingJava {
                version: ver.to_string(),
                searched,
            })
        }
    }
}

fn node_version_matches(dir_name: &str, ver: &str) -> bool {
    let clean_dir = dir_name.strip_prefix('v').unwrap_or(dir_name);
    let clean_ver = ver.strip_prefix('v').unwrap_or(ver);
    if clean_dir == clean_ver {
        return true;
    }
    if let Some(rest) = clean_dir.strip_prefix(clean_ver) {
        if rest.starts_with('.') || rest.starts_with('-') || rest.starts_with('+') {
            return true;
        }
    }
    false
}

fn check_node_bin(dir: &Path) -> bool {
    dir.join("node").is_file() || dir.join("node.exe").is_file()
}

pub fn resolve_node_bin(
    ver: &str,
    explicit_node_home: Option<&Path>,
    roots: &ToolchainRoots,
) -> Result<PathBuf, ToolchainError> {
    if let Some(explicit) = explicit_node_home {
        if check_node_bin(explicit) {
            return Ok(explicit.to_path_buf());
        }
        let bin = explicit.join("bin");
        if check_node_bin(&bin) || bin.is_dir() {
            return Ok(bin);
        }
        if explicit.is_dir() {
            return Ok(explicit.to_path_buf());
        }
        return Err(ToolchainError::InvalidNodeHome {
            path: explicit.to_path_buf(),
            reason: "node 未找到".into(),
        });
    }

    match roots.os {
        HostOs::Mac | HostOs::Linux => {
            let mut fnm_candidates_dirs = Vec::new();
            if let Some(ref d) = roots.fnm_dir {
                fnm_candidates_dirs.push(d.clone());
            }
            if let Some(ref h) = roots.home {
                fnm_candidates_dirs.push(h.join(".local/share/fnm"));
                fnm_candidates_dirs.push(h.join("Library/Application Support/fnm"));
                fnm_candidates_dirs.push(h.join(".fnm"));
            }

            let mut fnm_matches = Vec::new();
            for base in &fnm_candidates_dirs {
                let node_versions = base.join("node-versions");
                if node_versions.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&node_versions) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                if node_version_matches(&name, ver) {
                                    let bin_dir = path.join("installation/bin");
                                    if check_node_bin(&bin_dir) || bin_dir.is_dir() {
                                        fnm_matches.push((parse_version_key(&name), bin_dir));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !fnm_matches.is_empty() {
                fnm_matches.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                let (_, best_bin) = fnm_matches.pop().unwrap();
                return Ok(best_bin);
            }

            let mut nvm_candidates_dirs = Vec::new();
            if let Some(ref d) = roots.nvm_dir {
                nvm_candidates_dirs.push(d.clone());
            }
            if let Some(ref h) = roots.home {
                nvm_candidates_dirs.push(h.join(".nvm"));
            }

            let mut nvm_matches = Vec::new();
            for base in &nvm_candidates_dirs {
                let versions_node = base.join("versions/node");
                if versions_node.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&versions_node) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                if node_version_matches(&name, ver) {
                                    let bin_dir = path.join("bin");
                                    if check_node_bin(&bin_dir) || bin_dir.is_dir() {
                                        nvm_matches.push((parse_version_key(&name), bin_dir));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !nvm_matches.is_empty() {
                nvm_matches.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                let (_, best_bin) = nvm_matches.pop().unwrap();
                return Ok(best_bin);
            }

            let fnm_hint = if let Some(ref d) = roots.fnm_dir {
                format_home(&d.join("node-versions"), roots.home.as_deref())
            } else {
                "~/.local/share/fnm/node-versions".to_string()
            };
            let searched = format!("fnm（{fnm_hint}）或 nvm");
            Err(ToolchainError::MissingNode {
                version: ver.to_string(),
                searched,
            })
        }
        HostOs::Windows => {
            let mut fnm_candidates_dirs = Vec::new();
            if let Some(ref d) = roots.fnm_dir {
                fnm_candidates_dirs.push(d.clone());
            }
            if let Some(ref a) = roots.appdata {
                fnm_candidates_dirs.push(a.join("fnm"));
            }

            let mut fnm_matches = Vec::new();
            for base in &fnm_candidates_dirs {
                let node_versions = base.join("node-versions");
                if node_versions.is_dir() {
                    if let Ok(entries) = std::fs::read_dir(&node_versions) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                if node_version_matches(&name, ver) {
                                    let inst = path.join("installation");
                                    if check_node_bin(&inst) || inst.is_dir() {
                                        fnm_matches.push((parse_version_key(&name), inst));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !fnm_matches.is_empty() {
                fnm_matches.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                let (_, best_bin) = fnm_matches.pop().unwrap();
                return Ok(best_bin);
            }

            if let Some(ref nvm_home) = roots.nvm_home {
                if nvm_home.is_dir() {
                    let mut nvm_matches = Vec::new();
                    if let Ok(entries) = std::fs::read_dir(nvm_home) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                let name = entry.file_name().to_string_lossy().into_owned();
                                if node_version_matches(&name, ver)
                                    && (check_node_bin(&path) || path.is_dir())
                                {
                                    nvm_matches.push((parse_version_key(&name), path));
                                }
                            }
                        }
                    }
                    if !nvm_matches.is_empty() {
                        nvm_matches.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
                        let (_, best_bin) = nvm_matches.pop().unwrap();
                        return Ok(best_bin);
                    }
                }
            }

            let searched = "fnm（%APPDATA%\\fnm\\node-versions）或 nvm";
            Err(ToolchainError::MissingNode {
                version: ver.to_string(),
                searched: searched.into(),
            })
        }
    }
}

pub fn toolchain_env(
    java: Option<&str>,
    java_home: Option<&Path>,
    node: Option<&str>,
    node_home: Option<&Path>,
    roots: &ToolchainRoots,
) -> Result<Vec<(String, String)>, ToolchainError> {
    let mut env = Vec::new();
    let mut prepended = Vec::new();

    let resolved_java = if java.is_some() || java_home.is_some() {
        let jh = resolve_java_home(java.unwrap_or(""), java_home, roots)?;
        let bin = jh.join("bin");
        prepended.push(bin);
        Some(jh)
    } else if roots.os != HostOs::Windows {
        // No version declared: fall back to sdkman's default JDK (what `source sdkman-init.sh`
        // used to give), so a GUI-launched app does not pick up macOS's /usr/bin/java stub.
        let sdk = roots
            .sdkman_dir
            .clone()
            .or_else(|| roots.home.as_ref().map(|h| h.join(".sdkman")));
        match sdk.map(|s| s.join("candidates/java/current")) {
            Some(cur) if check_java_bin(&cur) => {
                prepended.push(cur.join("bin"));
                Some(cur)
            }
            _ => None,
        }
    } else {
        None
    };

    if node.is_some() || node_home.is_some() {
        let nb = resolve_node_bin(node.unwrap_or(""), node_home, roots)?;
        prepended.push(nb);
    }

    let sep = if roots.os == HostOs::Windows {
        ";"
    } else {
        ":"
    };

    let mut orig_items = Vec::new();
    for part in roots.base_path.split(sep) {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            orig_items.push(PathBuf::from(trimmed));
        }
    }

    let mut common_candidates = Vec::new();
    if roots.os != HostOs::Windows {
        if let Some(ref dirs) = roots.common_dirs {
            common_candidates.extend(dirs.clone());
        } else {
            common_candidates.push(PathBuf::from("/opt/homebrew/bin"));
            common_candidates.push(PathBuf::from("/usr/local/bin"));
            let sdk = if let Some(ref s) = roots.sdkman_dir {
                s.clone()
            } else if let Some(ref h) = roots.home {
                h.join(".sdkman")
            } else {
                PathBuf::from(".sdkman")
            };
            common_candidates.push(sdk.join("candidates/maven/current/bin"));
            common_candidates.push(sdk.join("candidates/gradle/current/bin"));
            if let Some(ref h) = roots.home {
                common_candidates.push(h.join(".local/bin"));
                if roots.os == HostOs::Mac {
                    common_candidates.push(h.join("Library/pnpm"));
                }
            }
        }
    }

    let mut all_paths = Vec::new();
    for p in prepended {
        all_paths.push(p);
    }
    for p in orig_items {
        all_paths.push(p);
    }
    for p in common_candidates {
        if p.is_dir() {
            all_paths.push(p);
        }
    }

    let mut seen = HashSet::new();
    let mut final_strings = Vec::new();
    for p in all_paths {
        let s = p.to_string_lossy().into_owned();
        if !s.is_empty() && seen.insert(s.clone()) {
            final_strings.push(s);
        }
    }

    let new_path = final_strings.join(sep);

    if let Some(jh) = resolved_java {
        env.push(("JAVA_HOME".to_string(), jh.to_string_lossy().into_owned()));
    }
    env.push(("PATH".to_string(), new_path));

    Ok(env)
}

pub fn shell_argv_with_os(cmd: &str, shell: Option<&str>, os: HostOs) -> Vec<String> {
    match os {
        HostOs::Windows => {
            let s = shell.unwrap_or("cmd").trim();
            if s.eq_ignore_ascii_case("powershell") {
                vec![
                    "powershell".into(),
                    "-NoProfile".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-Command".into(),
                    cmd.to_string(),
                ]
            } else {
                vec!["cmd".into(), "/C".into(), cmd.to_string()]
            }
        }
        _ => {
            let s = shell.unwrap_or("bash").trim();
            if s.eq_ignore_ascii_case("sh") {
                vec!["sh".into(), "-c".into(), cmd.to_string()]
            } else {
                vec!["bash".into(), "-c".into(), cmd.to_string()]
            }
        }
    }
}

pub fn shell_argv(cmd: &str, shell: Option<&str>) -> Vec<String> {
    shell_argv_with_os(cmd, shell, HostOs::current())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("ty_dev_test_{}_{}", pid, id));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn mac_sdkman_version_resolution() {
        let temp = TestDir::new();
        let home = temp.path().join("home");
        let sdkman_java = home.join(".sdkman/candidates/java");
        std::fs::create_dir_all(&sdkman_java).unwrap();

        for dir in ["8.0.392-zulu", "8.0.504+1-zulu", "17.0.9-tem", "current"] {
            std::fs::create_dir_all(sdkman_java.join(dir).join("bin")).unwrap();
            std::fs::write(sdkman_java.join(dir).join("bin/java"), "").unwrap();
        }

        // None means the real /Library/Java/JavaVirtualMachines. GitHub macOS
        // runners have a Temurin JDK there, so pin an empty dir and keep the
        // missing-Java assertion independent of the host.
        let empty_jvm = temp.path().join("empty-jvms");
        std::fs::create_dir_all(&empty_jvm).unwrap();
        let roots = ToolchainRoots {
            os: HostOs::Mac,
            home: Some(home.clone()),
            sdkman_dir: None,
            fnm_dir: None,
            nvm_dir: None,
            program_files: Vec::new(),
            appdata: None,
            nvm_home: None,
            base_path: "/bin".into(),
            common_dirs: None,
            mac_jvm_dir: Some(empty_jvm),
        };

        // "8" picks 8.0.504+1-zulu
        let res8 = resolve_java_home("8", None, &roots).unwrap();
        assert_eq!(res8, sdkman_java.join("8.0.504+1-zulu"));

        // "1.8" same as "8"
        let res18 = resolve_java_home("1.8", None, &roots).unwrap();
        assert_eq!(res18, sdkman_java.join("8.0.504+1-zulu"));

        // "17" picks 17.0.9-tem
        let res17 = resolve_java_home("17", None, &roots).unwrap();
        assert_eq!(res17, sdkman_java.join("17.0.9-tem"));

        // "11" errors with readable message
        let err11 = resolve_java_home("11", None, &roots)
            .unwrap_err()
            .to_string();
        assert!(err11.contains("缺 Java 11"), "{err11}");
        assert!(err11.contains("~/.sdkman/candidates/java"), "{err11}");

        // SDKMAN_DIR overrides default location
        let custom_sdkman = temp.path().join("custom_sdkman");
        let custom_java = custom_sdkman.join("candidates/java");
        std::fs::create_dir_all(custom_java.join("11.0.2-tem/bin")).unwrap();
        std::fs::write(custom_java.join("11.0.2-tem/bin/java"), "").unwrap();

        let mut roots_custom = roots.clone();
        roots_custom.sdkman_dir = Some(custom_sdkman);
        let res11 = resolve_java_home("11", None, &roots_custom).unwrap();
        assert_eq!(res11, custom_java.join("11.0.2-tem"));

        // Contents/Home returns Contents/Home
        let mac_jdk = sdkman_java.join("21.0.1-apple");
        let ch = mac_jdk.join("Contents/Home");
        std::fs::create_dir_all(ch.join("bin")).unwrap();
        std::fs::write(ch.join("bin/java"), "").unwrap();
        let res21 = resolve_java_home("21", None, &roots).unwrap();
        assert_eq!(res21, ch);
    }

    #[test]
    fn fnm_and_nvm_node_resolution() {
        let temp = TestDir::new();
        let home = temp.path().join("home");

        // fnm under ~/.local/share/fnm
        let fnm_dir1 = home.join(".local/share/fnm/node-versions/v22.11.0/installation/bin");
        std::fs::create_dir_all(&fnm_dir1).unwrap();
        std::fs::write(fnm_dir1.join("node"), "").unwrap();

        let roots = ToolchainRoots {
            os: HostOs::Mac,
            home: Some(home.clone()),
            sdkman_dir: None,
            fnm_dir: None,
            nvm_dir: None,
            program_files: Vec::new(),
            appdata: None,
            nvm_home: None,
            base_path: "/bin".into(),
            common_dirs: None,
            mac_jvm_dir: None,
        };

        let res22 = resolve_node_bin("22", None, &roots).unwrap();
        assert_eq!(res22, fnm_dir1);

        let res2211 = resolve_node_bin("22.11", None, &roots).unwrap();
        assert_eq!(res2211, fnm_dir1);

        // ~/Library/Application Support/fnm works when ~/.local/share/fnm not matching
        let home2 = temp.path().join("home2");
        let fnm_dir2 =
            home2.join("Library/Application Support/fnm/node-versions/v22.11.0/installation/bin");
        std::fs::create_dir_all(&fnm_dir2).unwrap();
        std::fs::write(fnm_dir2.join("node"), "").unwrap();
        let roots2 = ToolchainRoots {
            os: HostOs::Mac,
            home: Some(home2.clone()),
            sdkman_dir: None,
            fnm_dir: None,
            nvm_dir: None,
            program_files: Vec::new(),
            appdata: None,
            nvm_home: None,
            base_path: "/bin".into(),
            common_dirs: None,
            mac_jvm_dir: None,
        };
        let res_app_support = resolve_node_bin("22", None, &roots2).unwrap();
        assert_eq!(res_app_support, fnm_dir2);

        // FNM_DIR overrides
        let custom_fnm = temp.path().join("custom_fnm");
        let custom_bin = custom_fnm.join("node-versions/v22.12.0/installation/bin");
        std::fs::create_dir_all(&custom_bin).unwrap();
        std::fs::write(custom_bin.join("node"), "").unwrap();
        let mut roots_fnm_dir = roots.clone();
        roots_fnm_dir.fnm_dir = Some(custom_fnm);
        let res_custom = resolve_node_bin("22", None, &roots_fnm_dir).unwrap();
        assert_eq!(res_custom, custom_bin);

        // NVM fallback when fnm has nothing
        let home_nvm = temp.path().join("home_nvm");
        let nvm_bin = home_nvm.join(".nvm/versions/node/v20.10.0/bin");
        std::fs::create_dir_all(&nvm_bin).unwrap();
        std::fs::write(nvm_bin.join("node"), "").unwrap();
        let roots_nvm = ToolchainRoots {
            os: HostOs::Linux,
            home: Some(home_nvm),
            sdkman_dir: None,
            fnm_dir: None,
            nvm_dir: None,
            program_files: Vec::new(),
            appdata: None,
            nvm_home: None,
            base_path: "/bin".into(),
            common_dirs: None,
            mac_jvm_dir: None,
        };
        let res_nvm = resolve_node_bin("20", None, &roots_nvm).unwrap();
        assert_eq!(res_nvm, nvm_bin);

        // Missing reports readable Chinese error
        let err_missing = resolve_node_bin("99", None, &roots_nvm)
            .unwrap_err()
            .to_string();
        assert!(err_missing.contains("缺 Node 99"), "{err_missing}");
    }

    #[test]
    fn windows_java_and_node_resolution() {
        let temp = TestDir::new();
        let pf = temp.path().join("Program Files");
        let appdata = temp.path().join("AppData");
        let nvm_home = temp.path().join("nvm");

        // Windows Java layouts:
        // Zulu\zulu-8
        let zulu8 = pf.join("Zulu/zulu-8.0.392-win_x64");
        std::fs::create_dir_all(zulu8.join("bin")).unwrap();
        std::fs::write(zulu8.join("bin/java.exe"), "").unwrap();

        // Eclipse Adoptium\jdk-17.0.9.9-hotspot
        let tem17 = pf.join("Eclipse Adoptium/jdk-17.0.9.9-hotspot");
        std::fs::create_dir_all(tem17.join("bin")).unwrap();
        std::fs::write(tem17.join("bin/java.exe"), "").unwrap();

        // Java\jdk1.8.0_392
        let oracle8 = pf.join("Java/jdk1.8.0_392");
        std::fs::create_dir_all(oracle8.join("bin")).unwrap();
        std::fs::write(oracle8.join("bin/java.exe"), "").unwrap();

        // fnm node on windows
        let fnm_inst = appdata.join("fnm/node-versions/v22.11.0/installation");
        std::fs::create_dir_all(&fnm_inst).unwrap();
        std::fs::write(fnm_inst.join("node.exe"), "").unwrap();

        // nvm-windows
        let nvm_win_dir = nvm_home.join("v20.9.0");
        std::fs::create_dir_all(&nvm_win_dir).unwrap();
        std::fs::write(nvm_win_dir.join("node.exe"), "").unwrap();

        let roots = ToolchainRoots {
            os: HostOs::Windows,
            home: None,
            sdkman_dir: None,
            fnm_dir: None,
            nvm_dir: None,
            program_files: vec![pf],
            appdata: Some(appdata),
            nvm_home: Some(nvm_home),
            base_path: "C:\\Windows\\System32;C:\\Windows".into(),
            common_dirs: None,
            mac_jvm_dir: None,
        };

        // Java 17 hits Adoptium
        let res17 = resolve_java_home("17", None, &roots).unwrap();
        assert_eq!(res17, tem17);

        // Java 8 hits Zulu or Java
        let res8 = resolve_java_home("8", None, &roots).unwrap();
        assert!(res8 == zulu8 || res8 == oracle8);

        // Explicit java_home takes precedence
        let explicit = temp.path().join("my_jdk");
        std::fs::create_dir_all(explicit.join("bin")).unwrap();
        std::fs::write(explicit.join("bin/java.exe"), "").unwrap();
        let res_exp = resolve_java_home("8", Some(&explicit), &roots).unwrap();
        assert_eq!(res_exp, explicit);

        // fnm node on Windows returns installation directory
        let res_fnm = resolve_node_bin("22.11", None, &roots).unwrap();
        assert_eq!(res_fnm, fnm_inst);

        // nvm-windows returns version directory
        let res_nvm_win = resolve_node_bin("20.9", None, &roots).unwrap();
        assert_eq!(res_nvm_win, nvm_win_dir);

        // Windows PATH uses semicolon separator
        let env = toolchain_env(Some("17"), None, Some("22.11"), None, &roots).unwrap();
        let path_val = env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        let parts: Vec<&str> = path_val.split(';').collect();
        assert!(parts.len() >= 4, "{path_val}");
        assert_eq!(Path::new(parts[0]), tem17.join("bin"));
        assert_eq!(Path::new(parts[1]), fnm_inst);
        assert_eq!(parts[2], "C:\\Windows\\System32");
        assert_eq!(parts[3], "C:\\Windows");
    }

    #[test]
    fn toolchain_env_path_ordering_and_deduplication() {
        let temp = TestDir::new();
        let home = temp.path().join("home");
        let brew_bin = temp.path().join("brew/bin");
        std::fs::create_dir_all(&brew_bin).unwrap();

        let roots = ToolchainRoots {
            os: HostOs::Mac,
            home: Some(home),
            sdkman_dir: None,
            fnm_dir: None,
            nvm_dir: None,
            program_files: Vec::new(),
            appdata: None,
            nvm_home: None,
            base_path: "/usr/bin:/bin:/usr/bin".into(),
            common_dirs: Some(vec![brew_bin.clone(), temp.path().join("nonexistent_bin")]),
            mac_jvm_dir: None,
        };

        let explicit_jdk = temp.path().join("my_jdk");
        std::fs::create_dir_all(explicit_jdk.join("bin")).unwrap();
        std::fs::write(explicit_jdk.join("bin/java"), "").unwrap();

        let explicit_node = temp.path().join("my_node");
        std::fs::create_dir_all(explicit_node.join("bin")).unwrap();
        std::fs::write(explicit_node.join("bin/node"), "").unwrap();

        let env = toolchain_env(
            Some("17"),
            Some(&explicit_jdk),
            Some("22"),
            Some(&explicit_node),
            &roots,
        )
        .unwrap();

        let java_home = env
            .iter()
            .find(|(k, _)| k == "JAVA_HOME")
            .unwrap()
            .1
            .clone();
        assert_eq!(java_home, explicit_jdk.to_string_lossy());

        let path = env.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        let parts: Vec<String> = if cfg!(windows) {
            let raw: Vec<&str> = path.split(':').collect();
            let mut res = Vec::new();
            let mut i = 0;
            while i < raw.len() {
                if raw[i].len() == 1
                    && raw[i].chars().all(|c| c.is_ascii_alphabetic())
                    && i + 1 < raw.len()
                    && (raw[i + 1].starts_with('\\') || raw[i + 1].starts_with('/'))
                {
                    res.push(format!("{}:{}", raw[i], raw[i + 1]));
                    i += 2;
                } else {
                    res.push(raw[i].to_string());
                    i += 1;
                }
            }
            res
        } else {
            path.split(':').map(|s| s.to_string()).collect()
        };

        let jdk_bin = explicit_jdk.join("bin");
        let node_bin = explicit_node.join("bin");

        assert_eq!(parts[0], jdk_bin.to_string_lossy());
        assert_eq!(parts[1], node_bin.to_string_lossy());
        assert_eq!(parts[2], "/usr/bin");
        assert_eq!(parts[3], "/bin");
        assert_eq!(parts[4], brew_bin.to_string_lossy());

        // Deduplication worked
        assert_eq!(parts.iter().filter(|p| *p == "/usr/bin").count(), 1);
        // Nonexistent common dir was not included
        assert!(!parts.iter().any(|p| p.contains("nonexistent_bin")));
    }

    #[test]
    fn shell_argv_platform_cases() {
        let mac_cmd = shell_argv_with_os("npm run dev", None, HostOs::Mac);
        assert_eq!(mac_cmd, vec!["bash", "-c", "npm run dev"]);

        let linux_cmd = shell_argv_with_os("mvn clean", Some("sh"), HostOs::Linux);
        assert_eq!(linux_cmd, vec!["sh", "-c", "mvn clean"]);

        let win_cmd_default = shell_argv_with_os("dir", None, HostOs::Windows);
        assert_eq!(win_cmd_default, vec!["cmd", "/C", "dir"]);

        let win_cmd_explicit = shell_argv_with_os("dir", Some("cmd"), HostOs::Windows);
        assert_eq!(win_cmd_explicit, vec!["cmd", "/C", "dir"]);

        let win_ps = shell_argv_with_os("Get-Process", Some("powershell"), HostOs::Windows);
        assert_eq!(
            win_ps,
            vec![
                "powershell",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "Get-Process"
            ]
        );
    }

    #[test]
    fn toolchain_version_matcher_helpers() {
        // java_version_matches
        assert!(!java_version_matches("current", "8"));
        assert!(!java_version_matches(".hidden", "8"));
        assert!(java_version_matches("8", "8"));
        assert!(java_version_matches("8", "1.8"));
        assert!(java_version_matches("1.8", "8"));
        assert!(java_version_matches("8.0.392-zulu", "8"));
        assert!(java_version_matches("8.0.504+1-zulu", "1.8"));
        assert!(java_version_matches("1.8.0_392", "8"));
        assert!(java_version_matches("1.8.0-tem", "1.8"));
        assert!(java_version_matches("17.0.9-tem", "17"));
        assert!(java_version_matches("17+35", "17"));
        assert!(java_version_matches("17_35", "17"));
        assert!(!java_version_matches("170", "17"));
        assert!(!java_version_matches("7.0.1", "17"));
        assert!(!java_version_matches("11.0.2", "17"));

        // node_version_matches
        assert!(node_version_matches("v20.10.0", "20"));
        assert!(node_version_matches("v20.10.0", "v20"));
        assert!(node_version_matches("20.10.0", "20.10"));
        assert!(node_version_matches("v22.11.0", "22"));
        assert!(node_version_matches("v20-nightly", "20"));
        assert!(node_version_matches("v20+build", "20"));
        assert!(!node_version_matches("v200.0.0", "20"));
        assert!(!node_version_matches("v18.0.0", "20"));

        // mac_jvm_matches & win_java_matches
        assert!(mac_jvm_matches("jdk-8.jdk", "8"));
        assert!(mac_jvm_matches("zulu-8.jdk", "1.8"));
        assert!(mac_jvm_matches("jdk-17.jdk", "17"));
        assert!(mac_jvm_matches("jdk17.0.9.jdk", "17"));
        assert!(!mac_jvm_matches("jdk-11.jdk", "17"));

        assert!(win_java_matches("Zulu", "zulu-8.0.392-win_x64", "8"));
        assert!(win_java_matches("Zulu", "zulu8", "1.8"));
        assert!(win_java_matches("Zulu", "zulu-17.0.9-win_x64", "17"));
        assert!(!win_java_matches("Zulu", "zulu-21", "17"));
        assert!(win_java_matches(
            "Eclipse Adoptium",
            "jdk-17.0.9.9-hotspot",
            "17"
        ));
        assert!(win_java_matches("Eclipse Adoptium", "jdk-8.0.392", "8"));
        assert!(win_java_matches("Java", "jdk1.8.0_392", "8"));
        assert!(!win_java_matches("OtherVendor", "jdk-17", "17"));

        // parse_version_key ordering
        let k1 = parse_version_key("8.0.392");
        let k2 = parse_version_key("8.0.504");
        assert!(k2 > k1);
        let k_ver9 = parse_version_key("17.0.9");
        let k_ver10 = parse_version_key("17.0.10");
        assert!(
            k_ver10 > k_ver9,
            "numeric components must sort numerically, not lexicographically"
        );

        // format_home
        assert_eq!(
            format_home(Path::new("/home/user/code"), Some(Path::new("/home/user"))),
            "~/code"
        );
        assert_eq!(
            format_home(Path::new("/home/user"), Some(Path::new("/home/user"))),
            "~"
        );
        assert_eq!(
            format_home(Path::new("/opt/bin"), Some(Path::new("/home/user"))),
            "/opt/bin"
        );
        assert_eq!(format_home(Path::new("/opt/bin"), None), "/opt/bin");
    }
}
