//! Project scanner: walk a directory the user picked and guess which folders are
//! runnable dev projects (Spring Boot, Tomcat WAR, Vite / Vue / React, vue-cli,
//! Next.js, Nuxt, plain `npm start`, Django / FastAPI / Flask).
//!
//! Everything here is a read-only guess. The result is a list of [`Candidate`]s
//! with a suggested id, start command, port and toolchain plus a `why` note; the
//! user reviews them before anything is written (`Loaded::import_scanned`).

use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{detect_package_manager, NewProject, PackageManager, ProfileConfig};

/// Default walk depth below the chosen root (root itself = depth 0).
pub const DEFAULT_SCAN_DEPTH: usize = 3;
/// Hard cap so a mistyped `--depth 99` on `~` does not walk the whole disk.
pub const MAX_SCAN_DEPTH: usize = 8;
const MAX_DIRS: usize = 20_000;

/// Folder names never descended into (besides hidden `.xxx` folders).
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    "out",
    "bin",
    "obj",
    "__pycache__",
    "venv",
    "env",
    "site-packages",
    "coverage",
    "tmp",
    "logs",
];

/// Inside a detected project, these sub-folders are source / asset folders, not projects.
const SOURCE_DIRS: &[&str] = &[
    "src",
    "public",
    "static",
    "assets",
    "resources",
    "test",
    "tests",
    "docs",
    "doc",
    "scripts",
    "config",
    "lib",
    "mock",
    "mocks",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// One suggested project. Field names mirror [`NewProject`] so the UI can edit
/// them in place and send them back.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub id: String,
    pub name: String,
    /// `~/...` when under the home folder, otherwise absolute.
    pub root: String,
    /// Machine-readable type, e.g. `spring-boot-maven`, `vite`, `next`.
    pub kind: String,
    /// Human label, e.g. `Spring Boot（Maven）`.
    pub label: String,
    pub confidence: Confidence,
    /// Start command line (run through bash -c / cmd /C).
    pub start: String,
    pub stop: Option<String>,
    /// 0 = unknown.
    pub port: u16,
    /// true when `port` is the framework default, not read from the project.
    pub port_guessed: bool,
    pub health_url: Option<String>,
    pub login_url: Option<String>,
    pub java: Option<String>,
    pub node: Option<String>,
    pub background: bool,
    /// Tomcat WAR: needs a Tomcat install path before it can start.
    pub needs_tomcat: bool,
    /// Why we think this is a project and where each value came from.
    pub why: Vec<String>,
    /// Set when a project with the same root is already registered.
    pub existing_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_mode: Option<String>,
    /// Suggested Spring profiles, one per existing `application-<name>.*` file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileConfig>,
    /// Extra env for the project itself (scanner leaves this empty unless a file says so).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Non-Spring-Boot Maven: packaging / mainClass / port candidates. Not a disabled row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maven_guide: Option<MavenGuide>,
}

/// Candidates read from a non-Spring-Boot `pom.xml`. The project stays enabled.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MavenGuide {
    pub packaging: String,
    pub main_classes: Vec<String>,
    pub ports: Vec<u16>,
}

impl Candidate {
    /// Turn a (possibly user-edited) candidate into an add request.
    pub fn to_new_project(&self) -> NewProject {
        NewProject {
            id: self.id.clone(),
            name: Some(self.name.clone()),
            root: self.root.clone(),
            start: self.start.clone(),
            stop: self.stop.clone(),
            port: Some(self.port),
            login_url: self.login_url.clone(),
            health_url: self.health_url.clone(),
            node: self.node.clone(),
            java: self.java.clone(),
            background: Some(self.background),
            enabled: if self.needs_tomcat { Some(false) } else { None },
            reason: if self.needs_tomcat {
                Some("Tomcat WAR：先填 Tomcat 安装目录（tomcat 字段）再启用".into())
            } else {
                None
            },
            port_mode: self.port_mode.clone(),
            env: self.env.clone(),
            profiles: self.profiles.clone(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub root: String,
    pub depth: usize,
    pub dirs_visited: usize,
    /// true when the walk stopped early at the directory cap.
    pub truncated: bool,
    pub candidates: Vec<Candidate>,
}

/// Walk `root` (depth 0) down to `depth` levels and return detected projects.
/// `taken_ids` are ids already in the user's config; suggestions avoid them.
pub fn scan(root: &Path, depth: usize, taken_ids: &HashSet<String>) -> Result<ScanReport> {
    if !root.is_dir() {
        bail!("not a directory: {}", root.display());
    }
    let depth = depth.min(MAX_SCAN_DEPTH);
    let home = dirs::home_dir();
    let mut out = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    let mut stack: Vec<(PathBuf, usize, bool)> = vec![(root.to_path_buf(), 0, false)];
    while let Some((dir, level, inside_project)) = stack.pop() {
        if visited >= MAX_DIRS {
            truncated = true;
            break;
        }
        visited += 1;
        let found = detect_dir(&dir, home.as_deref());
        let is_project = found.iter().any(|d| !d.container);
        let is_container = found.iter().any(|d| d.container);
        out.extend(found.into_iter().filter_map(|d| d.candidate));
        if level >= depth {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.starts_with('.') && !SKIP_DIRS.contains(&name)
            })
            .filter(|p| {
                // Inside a detected (non-container) project only look at sub-apps,
                // not its source folders.
                if !(is_project || inside_project) || is_container {
                    return true;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !SOURCE_DIRS.contains(&name)
            })
            .collect();
        children.sort();
        for c in children.into_iter().rev() {
            stack.push((c, level + 1, is_project || inside_project));
        }
    }
    out.sort_by(|a, b| a.root.cmp(&b.root).then(a.kind.cmp(&b.kind)));
    assign_ids(&mut out, taken_ids);
    Ok(ScanReport {
        root: display_path(root, home.as_deref()),
        depth,
        dirs_visited: visited,
        truncated,
        candidates: out,
    })
}

struct Detection {
    /// Aggregator (Maven parent with <modules>, npm workspaces, Gradle multi-project):
    /// not a runnable project itself, keep walking its children.
    container: bool,
    candidate: Option<Candidate>,
}

fn detect_dir(dir: &Path, home: Option<&Path>) -> Vec<Detection> {
    let mut v = Vec::new();
    if let Some(d) = detect_maven(dir, home) {
        v.push(d);
    }
    if let Some(d) = detect_gradle(dir, home) {
        v.push(d);
    }
    if let Some(d) = detect_node(dir, home) {
        v.push(d);
    }
    if let Some(d) = detect_python(dir, home) {
        v.push(d);
    }
    v
}

fn read(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string()
}

pub(crate) fn display_path(p: &Path, home: Option<&Path>) -> String {
    if let Some(h) = home {
        if let Ok(rest) = p.strip_prefix(h) {
            let rest = rest.to_string_lossy().replace('\\', "/");
            return if rest.is_empty() {
                "~".into()
            } else {
                format!("~/{rest}")
            };
        }
    }
    p.to_string_lossy().replace('\\', "/")
}

fn base_candidate(dir: &Path, home: Option<&Path>, kind: &str, label: &str) -> Candidate {
    Candidate {
        id: String::new(),
        name: dir_name(dir),
        root: display_path(dir, home),
        kind: kind.into(),
        label: label.into(),
        confidence: Confidence::Medium,
        start: String::new(),
        stop: None,
        port: 0,
        port_guessed: false,
        health_url: None,
        login_url: None,
        java: None,
        node: None,
        background: true,
        needs_tomcat: false,
        why: Vec::new(),
        existing_id: None,
        port_mode: None,
        profiles: BTreeMap::new(),
        env: BTreeMap::new(),
        maven_guide: None,
    }
}

// ---------------------------------------------------------------------------
// Java
// ---------------------------------------------------------------------------

/// Text between `<tag>` and `</tag>` (first match), trimmed.
fn xml_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

/// Drop the `<parent>…</parent>` and `<dependencies>…` blocks so tags like
/// `<artifactId>` / `<packaging>` read from the project itself.
fn pom_own_section(pom: &str) -> String {
    let mut s = pom.to_string();
    for tag in [
        "parent",
        "dependencies",
        "dependencyManagement",
        "build",
        "profiles",
    ] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        if let (Some(a), Some(b)) = (s.find(&open), s.find(&close)) {
            if b > a {
                s.replace_range(a..b + close.len(), "");
            }
        }
    }
    s
}

/// "1.8" -> "8", "17" -> "17", "${java.version}" -> None, "17.0.2-tem" -> "17".
pub fn normalize_java_version(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_matches('"').trim_matches('\'');
    let raw = raw
        .strip_prefix("1.")
        .filter(|r| !r.is_empty())
        .unwrap_or(raw);
    let major: String = raw.chars().take_while(|c| c.is_ascii_digit()).collect();
    if major.is_empty() {
        return None;
    }
    let n: u32 = major.parse().ok()?;
    (5..=40).contains(&n).then(|| n.to_string())
}

/// Java version from `.sdkmanrc` / `.java-version`, then the build file.
fn java_version_files(dir: &Path) -> Option<(String, String)> {
    if let Some(t) = read(&dir.join(".sdkmanrc")) {
        for line in t.lines() {
            if let Some(v) = line.trim().strip_prefix("java=") {
                if let Some(n) = normalize_java_version(v) {
                    return Some((n, ".sdkmanrc".into()));
                }
            }
        }
    }
    if let Some(t) = read(&dir.join(".java-version")) {
        if let Some(n) = normalize_java_version(t.lines().next().unwrap_or("")) {
            return Some((n, ".java-version".into()));
        }
    }
    None
}

fn pom_java_version(pom: &str) -> Option<(String, String)> {
    for tag in [
        "java.version",
        "maven.compiler.release",
        "maven.compiler.source",
        "maven.compiler.target",
        "release",
        "source",
    ] {
        if let Some(v) = xml_tag(pom, tag).and_then(normalize_java_version) {
            return Some((v, format!("pom.xml <{tag}>")));
        }
    }
    None
}

/// `server.port` from application*.properties / application*.yml under
/// src/main/resources (plain file first, then profiles like application-dev).
fn spring_port(dir: &Path) -> Option<(u16, String)> {
    let res = dir.join("src/main/resources");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&res)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (n.starts_with("application") || n.starts_with("bootstrap"))
                && (n.ends_with(".properties") || n.ends_with(".yml") || n.ends_with(".yaml"))
        })
        .collect();
    // application.* before application-dev.* before the rest.
    files.sort_by_key(|p| {
        let n = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let rank = if n.starts_with("application.") {
            0
        } else if n.contains("-dev") || n.contains("-local") {
            1
        } else {
            2
        };
        (rank, n)
    });
    for f in files {
        let Some(text) = read(&f) else { continue };
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let port = if name.ends_with(".properties") {
            properties_port(&text)
        } else {
            yaml_server_port(&text)
        };
        if let Some(p) = port {
            return Some((p, format!("src/main/resources/{name} server.port")));
        }
    }
    None
}

/// Port number from a value like `8080`, `${PORT:8080}`, `"8080"`.
fn port_value(raw: &str) -> Option<u16> {
    let raw = raw.trim().trim_matches('"').trim_matches('\'');
    let raw = raw.split('#').next().unwrap_or("").trim();
    let digits = if let Some(inner) = raw.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
        inner.rsplit(':').next().unwrap_or("")
    } else {
        raw
    };
    let p: u16 = digits.trim().parse().ok()?;
    (p > 0).then_some(p)
}

fn properties_port(text: &str) -> Option<u16> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("server.port")?.trim_start();
        let rest = rest.strip_prefix('=').or_else(|| rest.strip_prefix(':'))?;
        port_value(rest)
    })
}

/// `server:\n  port: 8080` or `server.port: 8080` in YAML (first document wins).
fn yaml_server_port(text: &str) -> Option<u16> {
    let mut in_server = false;
    let mut server_indent = 0usize;
    for line in text.lines() {
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if line.trim() == "---" {
            in_server = false;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("server.port:") {
            if let Some(p) = port_value(rest) {
                return Some(p);
            }
        }
        if in_server {
            if indent <= server_indent {
                in_server = false;
            } else if let Some(rest) = t.strip_prefix("port:") {
                if indent > server_indent {
                    if let Some(p) = port_value(rest) {
                        return Some(p);
                    }
                }
            }
        }
        if t == "server:" {
            in_server = true;
            server_indent = indent;
        }
    }
    None
}

/// Profile name from `application-<name>.yml` / `.yaml` / `.properties`.
/// `application.yml` itself is not a profile.
pub fn spring_profile_from_filename(name: &str) -> Option<String> {
    let rest = name.strip_prefix("application-")?;
    let stem = rest
        .strip_suffix(".yml")
        .or_else(|| rest.strip_suffix(".yaml"))
        .or_else(|| rest.strip_suffix(".properties"))?;
    if stem.is_empty()
        || stem.contains('.')
        || stem.contains('/')
        || stem.contains('\\')
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(stem.to_string())
}

/// Suggest `[profiles.<name>]` only for `application-*` files that exist.
/// Each profile sets `SPRING_PROFILES_ACTIVE` and copies `server.port` when present.
pub fn suggest_spring_profiles(dir: &Path) -> BTreeMap<String, ProfileConfig> {
    let res = dir.join("src/main/resources");
    let Ok(rd) = std::fs::read_dir(&res) else {
        return BTreeMap::new();
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut out = BTreeMap::new();
    for name in names {
        let Some(profile) = spring_profile_from_filename(&name) else {
            continue;
        };
        let text = read(&res.join(&name)).unwrap_or_default();
        let port = if name.ends_with(".properties") {
            properties_port(&text)
        } else {
            yaml_server_port(&text)
        };
        let mut env = BTreeMap::new();
        env.insert("SPRING_PROFILES_ACTIVE".into(), profile.clone());
        out.insert(
            profile.clone(),
            ProfileConfig {
                label: profile,
                start: None,
                port,
                env,
            },
        );
    }
    out
}

fn attach_spring_profiles(c: &mut Candidate, dir: &Path) {
    let profiles = suggest_spring_profiles(dir);
    if profiles.is_empty() {
        return;
    }
    let names: Vec<&str> = profiles.keys().map(String::as_str).collect();
    c.why.push(format!(
        "环境预设来自已有的 application-*.yml / .yaml / .properties：{}",
        names.join("、")
    ));
    c.profiles = profiles;
}

fn pom_main_classes(pom: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = pom;
    while let Some(i) = rest.find("<mainClass>") {
        let after = &rest[i + "<mainClass>".len()..];
        let Some(end) = after.find("</mainClass>") else {
            break;
        };
        let v = after[..end].trim();
        let ok = !v.is_empty()
            && !v.contains('<')
            && !v.contains('>')
            && !v.contains('$')
            && v.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_'));
        if ok && !out.iter().any(|e| e == v) {
            out.push(v.to_string());
        }
        rest = &after[end + "</mainClass>".len()..];
    }
    out
}

fn spring_ports(dir: &Path) -> Vec<u16> {
    let res = dir.join("src/main/resources");
    let Ok(rd) = std::fs::read_dir(&res) else {
        return Vec::new();
    };
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            (n.starts_with("application") || n.starts_with("bootstrap"))
                && (n.ends_with(".properties") || n.ends_with(".yml") || n.ends_with(".yaml"))
        })
        .collect();
    files.sort();
    let mut ports = Vec::new();
    for f in files {
        let Some(text) = read(&f) else { continue };
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let port = if name.ends_with(".properties") {
            properties_port(&text)
        } else {
            yaml_server_port(&text)
        };
        if let Some(p) = port {
            if !ports.contains(&p) {
                ports.push(p);
            }
        }
    }
    ports
}

fn fill_maven_guide(c: &mut Candidate, pom: &str, dir: &Path, packaging: &str, mvn: &str) {
    let mains = pom_main_classes(pom);
    let ports = spring_ports(dir);
    if let Some(p) = ports.first().copied() {
        c.port = p;
        c.why.push(format!(
            "端口候选：{}",
            ports
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("、")
        ));
    } else {
        c.port = 8080;
        c.port_guessed = true;
        c.why
            .push("没找到 server.port，端口候选按 8080（请按应用实际端口改）".into());
    }
    c.why.push(format!("packaging：{packaging}"));
    if mains.is_empty() {
        c.why.push(
            "pom 里没有 <mainClass>。可在 maven-jar-plugin 或 exec-maven-plugin 里补上，或改启动命令"
                .into(),
        );
        c.start = format!("{mvn} -q -DskipTests package");
    } else {
        c.confidence = Confidence::Medium;
        c.why.push(format!("mainClass 候选：{}", mains.join("、")));
        c.start = format!("{mvn} -q exec:java -Dexec.mainClass={}", mains[0]);
    }
    if packaging != "war" {
        c.why.push(
            "这不是 Spring Boot：按 mainClass / 端口 / packaging 候选确认即可启动，不会默认停用"
                .into(),
        );
    }
    c.maven_guide = Some(MavenGuide {
        packaging: packaging.to_string(),
        main_classes: mains,
        ports: if ports.is_empty() {
            vec![c.port]
        } else {
            ports
        },
    });
}

fn detect_maven(dir: &Path, home: Option<&Path>) -> Option<Detection> {
    let pom = read(&dir.join("pom.xml"))?;
    let own = pom_own_section(&pom);
    if own.contains("<modules>") || pom.contains("<modules>") {
        return Some(Detection {
            container: true,
            candidate: None,
        });
    }
    let artifact = xml_tag(&own, "artifactId").map(str::to_string);
    let packaging = xml_tag(&own, "packaging").unwrap_or("jar").to_string();
    if packaging == "pom" {
        return Some(Detection {
            container: true,
            candidate: None,
        });
    }
    let is_boot = pom.contains("spring-boot");
    let has_wrapper = dir.join("mvnw").is_file();
    let mvn = if has_wrapper { "./mvnw" } else { "mvn" };
    let java = java_version_files(dir).or_else(|| pom_java_version(&pom));

    let mut c = if is_boot {
        let mut c = base_candidate(dir, home, "spring-boot-maven", "Spring Boot（Maven）");
        c.confidence = if pom.contains("spring-boot-maven-plugin")
            || pom.contains("spring-boot-starter-parent")
        {
            Confidence::High
        } else {
            Confidence::Medium
        };
        c.why.push("pom.xml 里有 spring-boot".into());
        c.start = format!("{mvn} spring-boot:run");
        if has_wrapper {
            c.why.push("有 mvnw，用 ./mvnw".into());
        }
        match spring_port(dir) {
            Some((p, src)) => {
                c.port = p;
                c.why.push(format!("端口来自 {src}"));
            }
            None => {
                c.port = 8080;
                c.port_guessed = true;
                c.why
                    .push("没找到 server.port，按 Spring Boot 默认 8080".into());
            }
        }
        if pom.contains("spring-boot-starter-actuator") {
            c.health_url = Some(format!("http://127.0.0.1:{}/actuator/health", c.port));
            c.why.push("有 actuator，用 /actuator/health 探活".into());
        }
        c
    } else {
        let mut c = base_candidate(dir, home, "maven-app", "Maven（非 Spring Boot）");
        c.confidence = Confidence::Low;
        fill_maven_guide(&mut c, &pom, dir, &packaging, mvn);
        if packaging == "war" {
            c.confidence = Confidence::Medium;
            c.label = "Maven WAR（非 Spring Boot）".into();
            c.needs_tomcat = true;
            let war_name = artifact.clone().unwrap_or_else(|| dir_name(dir));
            c.start = format!(
                "{mvn} -q -DskipTests package && cp target/*.war \"{{{{tomcat}}}}/webapps/{war_name}.war\" && exec \"{{{{tomcat}}}}/bin/catalina.sh\" run"
            );
            c.stop = Some("\"{{tomcat}}/bin/catalina.sh\" stop 15 -force".into());
            c.login_url = Some(format!("http://127.0.0.1:{}/{war_name}/", c.port));
            c.why.push(
                "非 Spring Boot 的 WAR 需要外部 Tomcat，未填 tomcat 时先停用。未配置的 {{tomcat}} 在启动时会报错，不会空替换成 /bin/catalina.sh。也可改成 mainClass 候选里的 exec:java。"
                    .into(),
            );
        }
        c
    };
    attach_spring_profiles(&mut c, dir);
    if let Some(a) = artifact.filter(|a| !a.contains('$')) {
        c.name = a;
    }
    if let Some((v, src)) = java {
        c.why.push(format!("Java {v} 来自 {src}"));
        c.java = Some(v);
    }
    Some(Detection {
        container: false,
        candidate: Some(c),
    })
}

fn gradle_java_version(text: &str) -> Option<String> {
    for key in [
        "JavaLanguageVersion.of(",
        "JavaVersion.VERSION_",
        "sourceCompatibility",
    ] {
        if let Some(i) = text.find(key) {
            let rest = &text[i + key.len()..];
            let rest = rest.trim_start_matches([' ', '=', '\'', '"', '(']);
            let rest = rest.replace('_', ".");
            if let Some(v) = normalize_java_version(&rest) {
                return Some(v);
            }
        }
    }
    None
}

fn detect_gradle(dir: &Path, home: Option<&Path>) -> Option<Detection> {
    let (file, text) = ["build.gradle.kts", "build.gradle"]
        .iter()
        .find_map(|f| read(&dir.join(f)).map(|t| (*f, t)))?;
    if !text.contains("org.springframework.boot") {
        let settings = read(&dir.join("settings.gradle"))
            .or_else(|| read(&dir.join("settings.gradle.kts")))
            .unwrap_or_default();
        if settings.contains("include") {
            return Some(Detection {
                container: true,
                candidate: None,
            });
        }
        return None;
    }
    let mut c = base_candidate(dir, home, "spring-boot-gradle", "Spring Boot（Gradle）");
    c.confidence = Confidence::High;
    c.why
        .push(format!("{file} 用了 org.springframework.boot 插件"));
    c.start = if dir.join("gradlew").is_file() {
        "./gradlew bootRun".into()
    } else {
        "gradle bootRun".into()
    };
    match spring_port(dir) {
        Some((p, src)) => {
            c.port = p;
            c.why.push(format!("端口来自 {src}"));
        }
        None => {
            c.port = 8080;
            c.port_guessed = true;
            c.why
                .push("没找到 server.port，按 Spring Boot 默认 8080".into());
        }
    }
    if text.contains("spring-boot-starter-actuator") {
        c.health_url = Some(format!("http://127.0.0.1:{}/actuator/health", c.port));
    }
    attach_spring_profiles(&mut c, dir);
    let java = java_version_files(dir)
        .or_else(|| gradle_java_version(&text).map(|v| (v, file.to_string())));
    if let Some((v, src)) = java {
        c.why.push(format!("Java {v} 来自 {src}"));
        c.java = Some(v);
    }
    Some(Detection {
        container: false,
        candidate: Some(c),
    })
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// Port from a script / command line: `--port 5173`, `--port=5173`, `-p 3000`,
/// `PORT=3000 …`.
pub fn port_from_command(cmd: &str) -> Option<u16> {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        if let Some(v) = t.strip_prefix("--port=") {
            if let Some(p) = port_value(v) {
                return Some(p);
            }
        }
        if let Some(v) = t.strip_prefix("PORT=") {
            if let Some(p) = port_value(v) {
                return Some(p);
            }
        }
        if (*t == "--port" || *t == "-p") && i + 1 < toks.len() {
            if let Some(p) = port_value(toks[i + 1]) {
                return Some(p);
            }
        }
    }
    None
}

/// First `port: 1234` / `port = 1234` in a JS/TS config file (vite / vue / nuxt).
fn port_from_js_config(text: &str) -> Option<u16> {
    // Prefer a port inside a `server` / `devServer` block when there is one.
    let start = ["devServer", "server"]
        .iter()
        .filter_map(|k| text.find(k))
        .min()
        .unwrap_or(0);
    let hay = &text[start..];
    let i = hay.find("port")?;
    let rest = hay[i + 4..].trim_start();
    let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('='))?;
    let digits: String = rest
        .trim_start()
        .trim_start_matches(['\'', '"'])
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let p: u16 = digits.parse().ok()?;
    (p > 0).then_some(p)
}

/// "v18.17.0" -> "18", "lts/*" -> None, ">=16 <21" -> "16", "^20.1" -> "20".
pub fn normalize_node_version(raw: &str) -> Option<String> {
    let s = raw.trim();
    let start = s.find(|c: char| c.is_ascii_digit())?;
    // Ignore lts aliases like "lts/hydrogen".
    if s.starts_with("lts") || s.starts_with("node") && start > 4 {
        return None;
    }
    let major: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let n: u32 = major.parse().ok()?;
    (4..=40).contains(&n).then(|| n.to_string())
}

fn node_version(dir: &Path, pkg: &serde_json::Value) -> Option<(String, String)> {
    for f in [".nvmrc", ".node-version"] {
        if let Some(t) = read(&dir.join(f)) {
            if let Some(v) = normalize_node_version(t.lines().next().unwrap_or("")) {
                return Some((v, f.to_string()));
            }
        }
    }
    let engines = pkg.get("engines")?.get("node")?.as_str()?;
    normalize_node_version(engines).map(|v| (v, "package.json engines.node".into()))
}

fn has_dep(pkg: &serde_json::Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .any(|k| pkg.get(k).and_then(|d| d.get(name)).is_some())
}

fn run_script(pm: PackageManager, script: &str) -> String {
    match (pm, script) {
        (PackageManager::Npm, "start") => "npm start".into(),
        (PackageManager::Npm, s) => format!("npm run {s}"),
        (PackageManager::Pnpm, s) => format!("pnpm {s}"),
        (PackageManager::Yarn, s) => format!("yarn {s}"),
    }
}

fn detect_node(dir: &Path, home: Option<&Path>) -> Option<Detection> {
    let text = read(&dir.join("package.json"))?;
    let pkg: serde_json::Value = serde_json::from_str(&text).ok()?;
    let scripts: BTreeMap<String, String> = pkg
        .get("scripts")
        .and_then(|s| s.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let has_workspaces =
        pkg.get("workspaces").is_some() || dir.join("pnpm-workspace.yaml").is_file();

    // (kind, label, preferred scripts, default port, port flag)
    let (kind, label, prefer, default_port): (&str, &str, &[&str], u16) = if has_dep(&pkg, "next") {
        ("next", "Next.js", &["dev", "start"], 3000)
    } else if has_dep(&pkg, "nuxt") || has_dep(&pkg, "nuxt3") {
        ("nuxt", "Nuxt", &["dev", "start"], 3000)
    } else if has_dep(&pkg, "@vue/cli-service") {
        ("vue-cli", "Vue CLI", &["serve", "dev", "start"], 8080)
    } else if has_dep(&pkg, "vite") {
        let label = if has_dep(&pkg, "vue") {
            "Vite + Vue"
        } else if has_dep(&pkg, "react") {
            "Vite + React"
        } else {
            "Vite"
        };
        ("vite", label, &["dev", "serve", "start"], 5173)
    } else if has_dep(&pkg, "react-scripts") {
        ("create-react-app", "React（CRA）", &["start", "dev"], 3000)
    } else {
        ("node", "Node（npm start）", &["dev", "start", "serve"], 0)
    };
    let Some(script) = prefer.iter().find(|s| scripts.contains_key(**s)) else {
        if has_workspaces {
            return Some(Detection {
                container: true,
                candidate: None,
            });
        }
        return None;
    };
    let script_body = scripts.get(*script).cloned().unwrap_or_default();
    let pm = detect_package_manager(dir).unwrap_or(PackageManager::Npm);

    let mut c = base_candidate(dir, home, kind, label);
    if let Some(n) = pkg
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
    {
        c.name = n.trim_start_matches('@').replace('/', "-");
    }
    c.start = run_script(pm, script);
    c.why.push(format!(
        "package.json scripts.{script} = `{script_body}`（{} 按锁文件选）",
        match pm {
            PackageManager::Pnpm => "pnpm",
            PackageManager::Yarn => "yarn",
            PackageManager::Npm => "npm",
        }
    ));
    c.confidence = if kind == "node" {
        Confidence::Low
    } else {
        Confidence::High
    };

    let config_port = || {
        let files: &[&str] = match kind {
            "vite" => &[
                "vite.config.ts",
                "vite.config.js",
                "vite.config.mts",
                "vite.config.mjs",
            ],
            "vue-cli" => &["vue.config.js", "vue.config.ts"],
            "nuxt" => &["nuxt.config.ts", "nuxt.config.js"],
            _ => &[],
        };
        files.iter().find_map(|f| {
            read(&dir.join(f))
                .and_then(|t| port_from_js_config(&t))
                .map(|p| (p, f.to_string()))
        })
    };
    let is_monorepo = has_workspaces || dir.join("turbo.json").is_file();
    if let Some(p) = port_from_command(&script_body) {
        c.port = p;
        c.why.push(format!("端口来自 scripts.{script}"));
        c.port_mode = None;
    } else if let Some((p, f)) = config_port() {
        c.port = p;
        c.why.push(format!("端口来自 {f}"));
        c.port_mode = Some("config".to_string());
    } else if default_port > 0 {
        c.port = default_port;
        c.port_guessed = true;
        c.why
            .push(format!("没写端口，按 {label} 默认 {default_port}"));
        c.port_mode = Some("none".to_string());
    } else {
        c.why.push("没找到端口，添加前请手动填".into());
        c.port_mode = Some("none".to_string());
    }
    if is_monorepo {
        c.port_mode = Some("none".to_string());
    }
    if c.port > 0 && kind != "node" {
        c.health_url = Some(format!("http://127.0.0.1:{}/", c.port));
    }
    if let Some((v, src)) = node_version(dir, &pkg) {
        c.why.push(format!("Node {v} 来自 {src}"));
        c.node = Some(v);
    }
    Some(Detection {
        container: false,
        candidate: Some(c),
    })
}

// ---------------------------------------------------------------------------
// Python (best effort)
// ---------------------------------------------------------------------------

fn detect_python(dir: &Path, home: Option<&Path>) -> Option<Detection> {
    let deps = ["requirements.txt", "pyproject.toml", "Pipfile"]
        .iter()
        .filter_map(|f| read(&dir.join(f)))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let py = if dir.join(".venv/bin/python").is_file() {
        ".venv/bin/python"
    } else if dir.join("venv/bin/python").is_file() {
        "venv/bin/python"
    } else {
        "python3"
    };
    let venv_note = |c: &mut Candidate| {
        if py != "python3" {
            c.why.push(format!("用项目里的虚拟环境 {py}"));
        }
    };
    if dir.join("manage.py").is_file() {
        let mut c = base_candidate(dir, home, "django", "Django");
        c.confidence = if deps.contains("django") {
            Confidence::High
        } else {
            Confidence::Medium
        };
        c.port = 8000;
        c.port_guessed = true;
        c.start = format!("{py} manage.py runserver 127.0.0.1:8000");
        c.health_url = Some("http://127.0.0.1:8000/".into());
        c.why
            .push("有 manage.py（Django），端口用 runserver 默认 8000".into());
        venv_note(&mut c);
        return Some(Detection {
            container: false,
            candidate: Some(c),
        });
    }
    if deps.is_empty() {
        return None;
    }
    let app_file = [
        "main.py",
        "app.py",
        "app/main.py",
        "src/main.py",
        "server.py",
    ]
    .iter()
    .find_map(|f| read(&dir.join(f)).map(|t| (*f, t)));
    if deps.contains("fastapi") || deps.contains("uvicorn") {
        let module = app_file
            .as_ref()
            .filter(|(_, t)| t.contains("FastAPI("))
            .map(|(f, _)| f.trim_end_matches(".py").replace('/', "."))
            .unwrap_or_else(|| "main".into());
        let mut c = base_candidate(dir, home, "fastapi", "FastAPI（uvicorn）");
        c.confidence = Confidence::Medium;
        c.port = 8000;
        c.port_guessed = true;
        c.start = format!("{py} -m uvicorn {module}:app --reload --host 127.0.0.1 --port 8000");
        c.health_url = Some("http://127.0.0.1:8000/docs".into());
        c.why
            .push(format!("依赖里有 fastapi/uvicorn，入口猜 {module}:app"));
        venv_note(&mut c);
        return Some(Detection {
            container: false,
            candidate: Some(c),
        });
    }
    if deps.contains("flask") {
        let mut c = base_candidate(dir, home, "flask", "Flask");
        c.confidence = Confidence::Low;
        c.port = 5000;
        c.port_guessed = true;
        let app = app_file
            .as_ref()
            .map(|(f, _)| f.trim_end_matches(".py").replace('/', "."))
            .unwrap_or_else(|| "app".into());
        c.start = format!("{py} -m flask --app {app} run --host 127.0.0.1 --port 5000");
        c.health_url = Some("http://127.0.0.1:5000/".into());
        c.why.push(format!("依赖里有 flask，入口猜 {app}"));
        venv_note(&mut c);
        return Some(Detection {
            container: false,
            candidate: Some(c),
        });
    }
    None
}

// ---------------------------------------------------------------------------
// ids
// ---------------------------------------------------------------------------

/// Lowercase ASCII slug usable as a project id (`a-z0-9`, `_`, `-`).
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    out.chars()
        .take(48)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn assign_ids(cands: &mut [Candidate], taken: &HashSet<String>) {
    let mut used: HashSet<String> = taken.clone();
    for c in cands.iter_mut() {
        let mut base = slugify(&c.name);
        if base.is_empty() {
            let root_name = c.root.rsplit('/').next().unwrap_or("");
            base = slugify(root_name);
        }
        if base.is_empty() {
            base = "project".into();
        }
        let mut id = base.clone();
        if used.contains(&id) {
            // Same folder with two stacks (e.g. pom.xml + package.json): suffix the kind.
            id = format!("{base}-{}", slugify(&c.kind));
        }
        let mut n = 2;
        while used.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        used.insert(id.clone());
        c.id = id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let p = std::env::temp_dir().join(format!(
                "tongyun_scan_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn write(&self, rel: &str, body: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(t: &Tmp) -> Vec<Candidate> {
        scan(&t.0, DEFAULT_SCAN_DEPTH, &HashSet::new())
            .unwrap()
            .candidates
    }

    fn one<'a>(cs: &'a [Candidate], kind: &str) -> &'a Candidate {
        cs.iter()
            .find(|c| c.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} in {cs:#?}"))
    }

    const BOOT_POM: &str = r#"<project>
  <parent><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-parent</artifactId></parent>
  <artifactId>orders-api</artifactId>
  <properties><java.version>17</java.version></properties>
  <dependencies><dependency><artifactId>spring-boot-starter-web</artifactId></dependency></dependencies>
  <build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build>
</project>"#;

    #[test]
    fn maven_spring_boot_with_yaml_port() {
        let t = Tmp::new("boot");
        t.write("orders/pom.xml", BOOT_POM);
        t.write(
            "orders/src/main/resources/application.yml",
            "spring:\n  application:\n    name: x\nserver:\n  # note\n  port: ${PORT:8085}\n",
        );
        let cs = run(&t);
        assert_eq!(cs.len(), 1, "{cs:#?}");
        let c = one(&cs, "spring-boot-maven");
        assert_eq!(c.name, "orders-api");
        assert_eq!(c.id, "orders-api");
        assert_eq!(c.start, "mvn spring-boot:run");
        assert_eq!(c.port, 8085);
        assert!(!c.port_guessed);
        assert_eq!(c.java.as_deref(), Some("17"));
        assert_eq!(c.confidence, Confidence::High);
        assert!(c.background);
        assert!(c.health_url.is_none());
    }

    #[test]
    fn maven_properties_port_wrapper_and_legacy_java() {
        let t = Tmp::new("bootprops");
        t.write(
            "svc/pom.xml",
            "<project><artifactId>svc</artifactId><properties><maven.compiler.source>1.8</maven.compiler.source></properties>\
             <dependencies><dependency><artifactId>spring-boot-starter-actuator</artifactId></dependency></dependencies></project>",
        );
        t.write("svc/mvnw", "#!/bin/sh\n");
        t.write(
            "svc/src/main/resources/application-dev.properties",
            "server.port = 9001\n",
        );
        let cs = run(&t);
        let c = one(&cs, "spring-boot-maven");
        assert_eq!(c.start, "./mvnw spring-boot:run");
        assert_eq!(c.port, 9001);
        assert_eq!(c.java.as_deref(), Some("8"));
        assert_eq!(
            c.health_url.as_deref(),
            Some("http://127.0.0.1:9001/actuator/health")
        );
    }

    #[test]
    fn sdkmanrc_beats_pom_and_default_port_is_flagged() {
        let t = Tmp::new("sdkman");
        t.write("a/pom.xml", BOOT_POM);
        t.write("a/.sdkmanrc", "java=21.0.2-tem\n");
        let c = run(&t).remove(0);
        assert_eq!(c.java.as_deref(), Some("21"));
        assert_eq!(c.port, 8080);
        assert!(c.port_guessed);
    }

    #[test]
    fn maven_parent_with_modules_descends_into_modules() {
        let t = Tmp::new("multi");
        t.write(
            "shop/pom.xml",
            "<project><artifactId>shop</artifactId><packaging>pom</packaging><modules><module>shop-web</module><module>shop-core</module></modules></project>",
        );
        t.write(
            "shop/shop-web/pom.xml",
            BOOT_POM.replace("orders-api", "shop-web").as_str(),
        );
        t.write(
            "shop/shop-core/pom.xml",
            "<project><artifactId>shop-core</artifactId></project>",
        );
        let cs = run(&t);
        assert_eq!(cs.len(), 2, "{cs:#?}");
        assert!(cs
            .iter()
            .any(|c| c.id == "shop-web" && c.kind == "spring-boot-maven"));
        let core = cs
            .iter()
            .find(|c| c.id == "shop-core")
            .expect("plain module");
        assert_eq!(core.kind, "maven-app");
        assert!(core.maven_guide.is_some());
        assert_ne!(core.to_new_project().enabled, Some(false));
    }

    #[test]
    fn plain_maven_guides_main_class_port_and_packaging_without_disabling() {
        let t = Tmp::new("plainjar");
        t.write(
            "worker/pom.xml",
            r#"<project>
              <artifactId>demo-worker</artifactId>
              <packaging>jar</packaging>
              <build><plugins><plugin><artifactId>exec-maven-plugin</artifactId>
                <configuration><mainClass>com.example.DemoApp</mainClass></configuration>
              </plugin></plugins></build>
            </project>"#,
        );
        t.write(
            "worker/src/main/resources/application.yml",
            "server:\n  port: 8090\n",
        );
        t.write(
            "worker/src/main/resources/application-dev.yml",
            "server:\n  port: 8091\n",
        );
        t.write(
            "worker/src/main/resources/application-local.yaml",
            "server.port: 8092\n",
        );
        let cs = run(&t);
        let c = one(&cs, "maven-app");
        assert_eq!(c.confidence, Confidence::Medium);
        assert!(!c.needs_tomcat);
        let guide = c.maven_guide.as_ref().unwrap();
        assert_eq!(guide.packaging, "jar");
        assert_eq!(guide.main_classes, vec!["com.example.DemoApp".to_string()]);
        assert!(guide.ports.contains(&8090));
        assert!(c.start.contains("exec:java"));
        assert!(c.start.contains("com.example.DemoApp"));
        assert_eq!(c.profiles.len(), 2);
        assert!(c.profiles.contains_key("dev"));
        assert!(c.profiles.contains_key("local"));
        assert!(!c.profiles.contains_key("prod"));
        assert_eq!(
            c.profiles["dev"]
                .env
                .get("SPRING_PROFILES_ACTIVE")
                .map(String::as_str),
            Some("dev")
        );
        assert_eq!(c.profiles["dev"].port, Some(8091));
        let req = c.to_new_project();
        assert_ne!(req.enabled, Some(false));
        assert!(req.reason.is_none());
        assert_eq!(req.profiles.len(), 2);
    }

    #[test]
    fn war_pom_is_guided_and_needs_tomcat() {
        let t = Tmp::new("war");
        t.write(
            "legacy/pom.xml",
            "<project><artifactId>legacy-admin</artifactId><packaging>war</packaging>\
             <properties><java.version>1.8</java.version></properties>\
             <build><plugins><plugin><configuration><mainClass>com.example.LegacyMain</mainClass></configuration></plugin></plugins></build>\
             </project>",
        );
        let cs = run(&t);
        let c = one(&cs, "maven-app");
        assert!(c.needs_tomcat);
        assert!(c.start.contains("{{tomcat}}"));
        assert!(c.stop.as_deref().unwrap_or("").contains("{{tomcat}}"));
        assert_eq!(c.java.as_deref(), Some("8"));
        assert_eq!(c.label, "Maven WAR（非 Spring Boot）");
        let guide = c.maven_guide.as_ref().unwrap();
        assert_eq!(guide.packaging, "war");
        assert_eq!(
            guide.main_classes,
            vec!["com.example.LegacyMain".to_string()]
        );
        assert!(guide.ports.contains(&8080));
        let req = c.to_new_project();
        assert_eq!(req.enabled, Some(false));
        assert!(req.reason.as_deref().unwrap_or("").contains("Tomcat"));
    }

    #[test]
    fn plain_maven_pure_jar_without_main_class() {
        let t = Tmp::new("purejar");
        t.write(
            "lib/pom.xml",
            r#"<project>
              <modelVersion>4.0.0</modelVersion>
              <groupId>com.example.fake</groupId>
              <artifactId>demo-pure-lib</artifactId>
              <version>1.0.0</version>
              <packaging>jar</packaging>
            </project>"#,
        );
        let cs = run(&t);
        let c = one(&cs, "maven-app");
        assert_eq!(c.id, "demo-pure-lib");
        assert_eq!(c.kind, "maven-app");
        assert_eq!(c.confidence, Confidence::Low);
        assert!(!c.needs_tomcat);
        let guide = c.maven_guide.as_ref().expect("maven_guide present");
        assert_eq!(guide.packaging, "jar");
        assert!(guide.main_classes.is_empty());
        assert_eq!(guide.ports, vec![8080]);
        assert_eq!(c.start, "mvn -q -DskipTests package");
        assert!(c.why.iter().any(|w| w.contains("没有 <mainClass>")));
        assert!(c.why.iter().any(|w| w.contains("这不是 Spring Boot")));
        let req = c.to_new_project();
        assert_ne!(req.enabled, Some(false));
        assert!(req.reason.is_none());
    }

    #[test]
    fn plain_maven_packaging_pom_container_descends_without_candidate() {
        let t = Tmp::new("pomcontainer");
        t.write(
            "parent/pom.xml",
            r#"<project>
              <modelVersion>4.0.0</modelVersion>
              <groupId>com.example.fake</groupId>
              <artifactId>demo-parent</artifactId>
              <version>1.0.0</version>
              <packaging>pom</packaging>
            </project>"#,
        );
        t.write(
            "parent/sub-pure/pom.xml",
            r#"<project>
              <modelVersion>4.0.0</modelVersion>
              <groupId>com.example.fake</groupId>
              <artifactId>demo-sub-pure</artifactId>
              <version>1.0.0</version>
              <packaging>jar</packaging>
            </project>"#,
        );
        t.write(
            "parent/sub-exec/pom.xml",
            r#"<project>
              <modelVersion>4.0.0</modelVersion>
              <groupId>com.example.fake</groupId>
              <artifactId>demo-sub-exec</artifactId>
              <version>1.0.0</version>
              <packaging>jar</packaging>
              <build>
                <plugins>
                  <plugin>
                    <artifactId>exec-maven-plugin</artifactId>
                    <configuration>
                      <mainClass>com.example.fake.Runner</mainClass>
                    </configuration>
                  </plugin>
                </plugins>
              </build>
            </project>"#,
        );
        let cs = run(&t);
        assert!(!cs.iter().any(|c| c.id == "demo-parent"));
        let sub_pure = cs.iter().find(|c| c.id == "demo-sub-pure").expect("sub-pure found");
        assert_eq!(sub_pure.confidence, Confidence::Low);
        assert!(sub_pure.maven_guide.as_ref().unwrap().main_classes.is_empty());

        let sub_exec = cs.iter().find(|c| c.id == "demo-sub-exec").expect("sub-exec found");
        assert_eq!(sub_exec.confidence, Confidence::Medium);
        assert_eq!(
            sub_exec.maven_guide.as_ref().unwrap().main_classes,
            vec!["com.example.fake.Runner".to_string()]
        );
    }

    #[test]
    fn spring_boot_profiles_come_only_from_existing_application_files() {
        let t = Tmp::new("bootprof");
        t.write("api/pom.xml", BOOT_POM);
        t.write(
            "api/src/main/resources/application.yml",
            "server:\n  port: 8080\n",
        );
        t.write(
            "api/src/main/resources/application-dev.yml",
            "server:\n  port: 8088\n",
        );
        let cs = run(&t);
        let c = one(&cs, "spring-boot-maven");
        assert_eq!(c.profiles.len(), 1);
        assert!(c.profiles.contains_key("dev"));
        assert!(!c.profiles.contains_key("test"));
        assert_eq!(c.profiles["dev"].port, Some(8088));
        assert!(c
            .why
            .iter()
            .any(|w| w.contains("application-*.yml") && w.contains("dev")));
    }

    #[test]
    fn gradle_spring_boot() {
        let t = Tmp::new("gradle");
        t.write(
            "inventory/build.gradle.kts",
            "plugins { id(\"org.springframework.boot\") version \"3.2.0\" }\njava { toolchain { languageVersion.set(JavaLanguageVersion.of(21)) } }\n",
        );
        t.write("inventory/gradlew", "");
        t.write(
            "inventory/src/main/resources/application.properties",
            "server.port=8090\n",
        );
        let c = run(&t).remove(0);
        assert_eq!(c.kind, "spring-boot-gradle");
        assert_eq!(c.start, "./gradlew bootRun");
        assert_eq!(c.port, 8090);
        assert_eq!(c.java.as_deref(), Some("21"));
    }

    #[test]
    fn vite_vue_port_from_config_pnpm_and_nvmrc() {
        let t = Tmp::new("vite");
        t.write(
            "web/package.json",
            r#"{"name":"@acme/web","scripts":{"dev":"vite","build":"vite build"},"devDependencies":{"vite":"^5","vue":"^3"}}"#,
        );
        t.write("web/pnpm-lock.yaml", "");
        t.write("web/.nvmrc", "v20.11.1\n");
        t.write(
            "web/vite.config.ts",
            "export default defineConfig({ plugins: [vue()], server: { host: true, port: 5180 } })",
        );
        // src/ must not be scanned as a project of its own.
        t.write("web/src/package.json", r#"{"scripts":{"dev":"x"}}"#);
        let cs = run(&t);
        assert_eq!(cs.len(), 1, "{cs:#?}");
        let c = &cs[0];
        assert_eq!(c.kind, "vite");
        assert_eq!(c.label, "Vite + Vue");
        assert_eq!(c.name, "acme-web");
        assert_eq!(c.start, "pnpm dev");
        assert_eq!(c.port, 5180);
        assert_eq!(c.node.as_deref(), Some("20"));
        assert_eq!(c.health_url.as_deref(), Some("http://127.0.0.1:5180/"));
    }

    #[test]
    fn vite_react_port_from_script_and_engines() {
        let t = Tmp::new("react");
        t.write(
            "app/package.json",
            r#"{"name":"dash","engines":{"node":">=18.0.0"},"scripts":{"dev":"vite --port 4100 --host"},"dependencies":{"react":"18"},"devDependencies":{"vite":"5"}}"#,
        );
        t.write("app/yarn.lock", "");
        let c = run(&t).remove(0);
        assert_eq!(c.label, "Vite + React");
        assert_eq!(c.start, "yarn dev");
        assert_eq!(c.port, 4100);
        assert_eq!(c.node.as_deref(), Some("18"));
    }

    #[test]
    fn vue_cli_prefers_serve_and_reads_vue_config() {
        let t = Tmp::new("vuecli");
        t.write(
            "admin/package.json",
            r#"{"name":"admin","scripts":{"serve":"vue-cli-service serve","dev":"vue-cli-service serve --mode dev"},"devDependencies":{"@vue/cli-service":"4"}}"#,
        );
        t.write("admin/package-lock.json", "{}");
        t.write(
            "admin/vue.config.js",
            "module.exports = { devServer: { port: 8146 } }",
        );
        let c = run(&t).remove(0);
        assert_eq!(c.kind, "vue-cli");
        assert_eq!(c.start, "npm run serve");
        assert_eq!(c.port, 8146);
    }

    #[test]
    fn next_and_nuxt_and_plain_node() {
        let t = Tmp::new("next");
        t.write(
            "site/package.json",
            r#"{"name":"site","scripts":{"dev":"next dev -p 3100"},"dependencies":{"next":"14"}}"#,
        );
        t.write(
            "blog/package.json",
            r#"{"name":"blog","scripts":{"dev":"nuxt dev"},"devDependencies":{"nuxt":"3"}}"#,
        );
        t.write(
            "bot/package.json",
            r#"{"name":"bot","scripts":{"start":"node index.js"}}"#,
        );
        t.write(
            "lib/package.json",
            r#"{"name":"lib","scripts":{"test":"jest"}}"#,
        );
        let cs = run(&t);
        assert_eq!(cs.len(), 3, "{cs:#?}");
        let next = one(&cs, "next");
        assert_eq!(next.start, "npm run dev");
        assert_eq!(next.port, 3100);
        let nuxt = one(&cs, "nuxt");
        assert_eq!(nuxt.port, 3000);
        assert!(nuxt.port_guessed);
        let node = one(&cs, "node");
        assert_eq!(node.start, "npm start");
        assert_eq!(node.port, 0);
        assert_eq!(node.confidence, Confidence::Low);
    }

    #[test]
    fn python_django_fastapi_flask() {
        let t = Tmp::new("py");
        t.write("dj/manage.py", "");
        t.write("dj/requirements.txt", "Django==4.2\n");
        t.write("api/requirements.txt", "fastapi\nuvicorn[standard]\n");
        t.write(
            "api/app/main.py",
            "from fastapi import FastAPI\napp = FastAPI()\n",
        );
        t.write("api/.venv/bin/python", "");
        t.write(
            "fl/pyproject.toml",
            "[project]\ndependencies = [\"flask>=3\"]\n",
        );
        t.write("fl/app.py", "from flask import Flask\n");
        let cs = run(&t);
        let dj = one(&cs, "django");
        assert_eq!(dj.start, "python3 manage.py runserver 127.0.0.1:8000");
        assert_eq!(dj.confidence, Confidence::High);
        let api = one(&cs, "fastapi");
        assert_eq!(
            api.start,
            ".venv/bin/python -m uvicorn app.main:app --reload --host 127.0.0.1 --port 8000"
        );
        let fl = one(&cs, "flask");
        assert!(fl.start.contains("flask --app app run"), "{}", fl.start);
        assert_eq!(fl.port, 5000);
    }

    #[test]
    fn skips_vendor_dirs_hidden_dirs_and_respects_depth() {
        let t = Tmp::new("skip");
        let pkg = r#"{"scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#;
        t.write("node_modules/x/package.json", pkg);
        t.write(".cache/y/package.json", pkg);
        t.write("dist/package.json", pkg);
        t.write("a/b/c/d/package.json", pkg);
        t.write("a/b/c/package.json", pkg);
        let cs = run(&t);
        assert_eq!(cs.len(), 1, "{cs:#?}");
        assert!(cs[0].root.ends_with("a/b/c"), "{}", cs[0].root);
        let shallow = scan(&t.0, 1, &HashSet::new()).unwrap();
        assert!(shallow.candidates.is_empty());
    }

    #[test]
    fn workspaces_root_is_container_and_ids_stay_unique() {
        let t = Tmp::new("ws");
        t.write(
            "mono/package.json",
            r#"{"name":"mono","private":true,"workspaces":["apps/*"]}"#,
        );
        let pkg = r#"{"name":"web","scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#;
        t.write("mono/apps/a/package.json", pkg);
        t.write("mono/apps/b/package.json", pkg);
        let mut taken = HashSet::new();
        taken.insert("web".to_string());
        let cs = scan(&t.0, 4, &taken).unwrap().candidates;
        let ids: Vec<_> = cs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["web-vite", "web-2"]);
    }

    #[test]
    fn fullstack_folder_yields_both_with_distinct_ids() {
        let t = Tmp::new("full");
        t.write("both/pom.xml", BOOT_POM);
        t.write(
            "both/package.json",
            r#"{"name":"orders-api","scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
        );
        let cs = run(&t);
        assert_eq!(cs.len(), 2);
        let mut ids: Vec<_> = cs.iter().map(|c| c.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["orders-api", "orders-api-vite"]);
    }

    #[test]
    fn version_and_port_helpers() {
        assert_eq!(normalize_java_version("1.8").as_deref(), Some("8"));
        assert_eq!(normalize_java_version("11").as_deref(), Some("11"));
        assert_eq!(normalize_java_version("${java.version}"), None);
        assert_eq!(normalize_node_version("lts/iron"), None);
        assert_eq!(normalize_node_version("^20.1.0").as_deref(), Some("20"));
        assert_eq!(port_from_command("vite --port=4000"), Some(4000));
        assert_eq!(port_from_command("PORT=3005 node server.js"), Some(3005));
        assert_eq!(port_from_command("next dev -p 3100"), Some(3100));
        assert_eq!(port_from_command("vite"), None);
        assert_eq!(slugify("My Cool_App!"), "my-cool_app");
        assert_eq!(slugify("中文"), "");
        assert_eq!(
            display_path(Path::new("/home/u/code/x"), Some(Path::new("/home/u"))),
            "~/code/x"
        );
    }

    #[test]
    fn node_scan_port_mode_rules_and_start_cmd_has_no_port() {
        let t = Tmp::new("port_mode_scan");

        t.write(
            "from_script/package.json",
            r#"{"name":"from-script","scripts":{"dev":"vite --port 5200"},"devDependencies":{"vite":"5"}}"#,
        );

        t.write(
            "from_config/package.json",
            r#"{"name":"from-config","scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
        );
        t.write(
            "from_config/vite.config.ts",
            "export default { server: { port: 5300 } };",
        );

        t.write(
            "from_default/package.json",
            r#"{"name":"from-default","scripts":{"dev":"vite"},"devDependencies":{"vite":"5"}}"#,
        );

        t.write(
            "mono_root/package.json",
            r#"{"name":"mono-root","scripts":{"dev":"node scripts/turbo-run/index.js dev"}}"#,
        );
        t.write("mono_root/turbo.json", "{}");

        let cs = run(&t);
        assert_eq!(cs.len(), 4, "{cs:#?}");

        let by_id = |id: &str| cs.iter().find(|c| c.id == id).unwrap();

        let s1 = by_id("from-script");
        assert_eq!(s1.start, "npm run dev");
        assert!(!s1.start.contains("--port"));
        assert_eq!(s1.port, 5200);
        assert_eq!(s1.port_mode, None);

        let s2 = by_id("from-config");
        assert_eq!(s2.start, "npm run dev");
        assert!(!s2.start.contains("--port"));
        assert_eq!(s2.port, 5300);
        assert_eq!(s2.port_mode.as_deref(), Some("config"));

        let s3 = by_id("from-default");
        assert_eq!(s3.start, "npm run dev");
        assert!(!s3.start.contains("--port"));
        assert_eq!(s3.port, 5173);
        assert!(s3.port_guessed);
        assert_eq!(s3.port_mode.as_deref(), Some("none"));

        let s4 = by_id("mono-root");
        assert_eq!(s4.start, "npm run dev");
        assert!(!s4.start.contains("--port"));
        assert_eq!(s4.port_mode.as_deref(), Some("none"));
    }

    #[test]
    fn spring_profile_pure_helpers_and_detection() {
        // Pure helper: spring_profile_from_filename
        assert_eq!(
            spring_profile_from_filename("application-dev.yml").as_deref(),
            Some("dev")
        );
        assert_eq!(
            spring_profile_from_filename("application-prod.yaml").as_deref(),
            Some("prod")
        );
        assert_eq!(
            spring_profile_from_filename("application-local_test-1.properties").as_deref(),
            Some("local_test-1")
        );
        assert_eq!(spring_profile_from_filename("application.yml"), None);
        assert_eq!(spring_profile_from_filename("application.properties"), None);
        assert_eq!(spring_profile_from_filename("application-.yml"), None);
        assert_eq!(
            spring_profile_from_filename("application-dev.test.yml"),
            None
        );
        assert_eq!(
            spring_profile_from_filename("application-dev/test.yml"),
            None
        );
        assert_eq!(
            spring_profile_from_filename("application-dev\\test.yml"),
            None
        );
        assert_eq!(
            spring_profile_from_filename("application-dev@special.yml"),
            None
        );
        assert_eq!(spring_profile_from_filename("bootstrap-dev.yml"), None);
        assert_eq!(spring_profile_from_filename("other-dev.yml"), None);

        // Helper: suggest_spring_profiles with temporary resources
        let t = Tmp::new("prof_helpers");
        let root = &t.0;
        assert!(suggest_spring_profiles(root).is_empty());

        t.write(
            "src/main/resources/application.yml",
            "server:\n  port: 8080\n",
        );
        t.write(
            "src/main/resources/application-dev.yml",
            "server:\n  port: 8081\n",
        );
        t.write(
            "src/main/resources/application-test.properties",
            "server.port = 8082\n",
        );
        t.write(
            "src/main/resources/application-prod.yaml",
            "# no port specified\n",
        );
        t.write(
            "src/main/resources/application-invalid.dot.yml",
            "server.port: 8083\n",
        );
        t.write("src/main/resources/ignored.txt", "nothing\n");

        let profiles = suggest_spring_profiles(root);
        assert_eq!(profiles.len(), 3);

        let dev = profiles.get("dev").unwrap();
        assert_eq!(dev.label, "dev");
        assert_eq!(dev.port, Some(8081));
        assert_eq!(
            dev.env.get("SPRING_PROFILES_ACTIVE").map(String::as_str),
            Some("dev")
        );

        let test = profiles.get("test").unwrap();
        assert_eq!(test.label, "test");
        assert_eq!(test.port, Some(8082));
        assert_eq!(
            test.env.get("SPRING_PROFILES_ACTIVE").map(String::as_str),
            Some("test")
        );

        let prod = profiles.get("prod").unwrap();
        assert_eq!(prod.label, "prod");
        assert_eq!(prod.port, None);
        assert_eq!(
            prod.env.get("SPRING_PROFILES_ACTIVE").map(String::as_str),
            Some("prod")
        );

        // Helper: attach_spring_profiles
        let mut candidate = base_candidate(root, None, "spring-boot-maven", "Spring Boot");
        attach_spring_profiles(&mut candidate, root);
        assert_eq!(candidate.profiles.len(), 3);
        assert!(candidate
            .why
            .iter()
            .any(|w| w.contains("环境预设来自已有的 application-*.yml")
                && w.contains("dev")
                && w.contains("prod")
                && w.contains("test")));
    }
}
