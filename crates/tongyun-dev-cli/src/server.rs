//! Web console for tongyun-dev (`tongyun-dev ui`, default http://127.0.0.1:9099).
//!
//! A tiny std-only HTTP server bound to 127.0.0.1. Every request re-reads
//! config/projects.toml through `tongyun-dev-core`, so the console, the CLI and the
//! desktop shell always see the same projects and the same start/stop/status logic.
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tongyun_dev_core::{Loaded, NewProject, ProjectEdit};

pub const DEFAULT_UI_PORT: u16 = 9099;
const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 64 * 1024;
const LOG_LINES: usize = 120;

struct AppState {
    config_path: PathBuf,
    port: u16,
    /// Serializes config writes (add / remove).
    write_lock: Mutex<()>,
}

/// Bind 127.0.0.1:`initial_port` (or the next free port up to +20) and serve forever.
pub fn start_ui_server(config_path: PathBuf, initial_port: u16, auto_open: bool) -> Result<()> {
    if !config_path.is_file() {
        let default_path = tongyun_dev_core::default_config_path(None);
        let default_hint = default_path.to_string_lossy();
        eprintln!("未找到配置文件 projects.toml：{}", config_path.display());
        eprintln!("默认配置路径：{default_hint}");
        eprintln!("提示：可在 `{default_hint}` 创建默认配置，或通过 --config 参数 / TONGYUN_DEV_CONFIG 环境变量指定配置路径。");
        bail!("未找到配置文件 projects.toml：{}", config_path.display());
    }
    let (listener, port) = bind_from(initial_port)?;
    let url = format!("http://127.0.0.1:{port}");
    println!();
    println!("tongyun-dev 网页控制台已就绪：{url}");
    println!("  配置 {}", config_path.display());
    println!("  一键启停、看状态和日志、注册/移除项目；Ctrl+C 停止");
    println!();
    if auto_open {
        if let Err(e) = tongyun_dev_core::open_url(&url) {
            eprintln!("could not open browser: {e:#}");
        }
    }
    serve(listener, config_path, port)
}

fn bind_from(initial_port: u16) -> Result<(TcpListener, u16)> {
    let last = initial_port.saturating_add(20);
    let mut port = initial_port;
    loop {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => {
                let actual = l.local_addr()?.port();
                return Ok((l, actual));
            }
            Err(e) if port < last => {
                eprintln!("port {port} busy ({e}), trying {}", port + 1);
                port += 1;
            }
            Err(e) => bail!("could not bind UI server on 127.0.0.1:{initial_port}..={last}: {e}"),
        }
    }
}

/// Serve on an already-bound listener (used by tests with an ephemeral port).
pub fn serve(listener: TcpListener, config_path: PathBuf, port: u16) -> Result<()> {
    let state = Arc::new(AppState {
        config_path,
        port,
        write_lock: Mutex::new(()),
    });
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    let _ = handle_connection(stream, &state);
                });
            }
            Err(e) => eprintln!("UI server connection error: {e}"),
        }
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    host: String,
    origin: Option<String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEADER {
            bail!("header too large");
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut parts = lines.next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut host = String::new();
    let mut origin = None;
    let mut len = 0usize;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "host" => host = v.to_string(),
            "origin" => origin = Some(v.to_string()),
            "content-length" => len = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    if len > MAX_BODY {
        bail!("body too large");
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Ok(Some(Request {
        method,
        path,
        host,
        origin,
        body,
    }))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Only our own page may talk to the API: Host must be loopback (blocks DNS
/// rebinding) and state-changing requests from another Origin are refused (CSRF).
fn trusted(req: &Request, port: u16) -> bool {
    let allowed = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !allowed.iter().any(|a| req.host.eq_ignore_ascii_case(a)) {
        return false;
    }
    if req.method == "GET" {
        return true;
    }
    match &req.origin {
        None => true,
        Some(o) => allowed
            .iter()
            .any(|a| o.eq_ignore_ascii_case(&format!("http://{a}"))),
    }
}

fn handle_connection(mut stream: TcpStream, state: &AppState) -> Result<()> {
    let Some(req) = read_request(&mut stream)? else {
        return Ok(());
    };
    if !trusted(&req, state.port) {
        return send_json(
            &mut stream,
            403,
            &json!({"ok": false, "error": "forbidden origin"}),
        );
    }
    let (code, ctype, body) = route(&req, state);
    send_response(&mut stream, code, ctype, &body)
}

fn project_route<'a>(path: &'a str, action: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/api/projects/")?;
    let id = if action.is_empty() {
        rest
    } else {
        rest.strip_suffix(action)?.strip_suffix('/')?
    };
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

fn group_route<'a>(path: &'a str, action: &str) -> Option<&'a str> {
    let rest = path.strip_prefix("/api/groups/")?;
    let key = if action.is_empty() {
        rest
    } else {
        rest.strip_suffix(action)?.strip_suffix('/')?
    };
    (!key.is_empty() && !key.contains('/')).then_some(key)
}

fn url_decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn json_body(code: u16, v: Value) -> (u16, &'static str, Vec<u8>) {
    (
        code,
        "application/json; charset=utf-8",
        v.to_string().into_bytes(),
    )
}

fn err_json(code: u16, e: impl std::fmt::Display) -> (u16, &'static str, Vec<u8>) {
    json_body(code, json!({"ok": false, "error": e.to_string()}))
}

fn load(state: &AppState) -> Result<Loaded> {
    tongyun_dev_core::load(Some(&state.config_path))
}

/// Status plus list fields (enabled / reason / node / java) for every project.
pub fn overview(loaded: &Loaded) -> Result<Vec<Value>> {
    let list = loaded.list()?;
    let mut out = Vec::with_capacity(list.projects.len());
    for row in list.projects {
        let mut v = match loaded.status(&row.id) {
            Ok(st) => serde_json::to_value(st)?,
            Err(e) => json!({
                "id": row.id.clone(),
                "name": row.name.clone(),
                "running": false,
                "state": "stopped",
                "elapsed_secs": null,
                "timeout_secs": 0,
                "failure": format!("{e:#}"),
                "pid": null,
                "port": row.port,
                "port_open": false,
                "login_probe": {"url": "", "http_code": null, "ok": false},
                "log_path": "",
                "pid_path": "",
                "root": row.root.clone(),
                "port_conflict": false,
                "port_holder": "",
                "deps_missing": false,
                "install_command": "",
            }),
        };
        if let Value::Object(m) = &mut v {
            m.insert("enabled".into(), json!(row.enabled));
            m.insert("configured".into(), json!(row.configured));
            m.insert("reason".into(), json!(row.reason));
            m.insert("group".into(), json!(row.group));
            m.insert("node".into(), json!(row.node));
            m.insert("java".into(), json!(row.java));
            m.insert("java_home".into(), json!(row.java_home));
            m.insert("node_home".into(), json!(row.node_home));
            m.insert("shell".into(), json!(row.shell));
            m.insert("links".into(), json!(row.links));
            m.insert("default_profile".into(), json!(row.default_profile));
            m.insert("profiles".into(), json!(row.profiles));
            m.insert("depends_on".into(), json!(row.depends_on));
            m.insert("start_command".into(), json!(row.start_command));
            m.insert(
                "profile_start_commands".into(),
                json!(row.profile_start_commands),
            );
            if let Some(err) = &row.resolve_error {
                m.insert("resolve_error".into(), json!(err));
                m.insert("configured".into(), json!(false));
            }
        }
        out.push(v);
    }
    Ok(out)
}

fn route(req: &Request, state: &AppState) -> (u16, &'static str, Vec<u8>) {
    let path = req.path.split('?').next().unwrap_or("");
    let method = req.method.as_str();
    if method == "GET" && path == "/" {
        return (200, "text/html; charset=utf-8", UI_HTML.as_bytes().to_vec());
    }
    let loaded = match load(state) {
        Ok(l) => l,
        Err(e) => return err_json(500, format!("{e:#}")),
    };
    match method {
        "GET" if path == "/api/projects" => match overview(&loaded) {
            Ok(list) => json_body(200, Value::Array(list)),
            Err(e) => err_json(500, format!("{e:#}")),
        },
        "GET" if path == "/api/groups" => {
            json_body(200, json!({"ok": true, "groups": loaded.groups()}))
        }
        "GET" => {
            if let Some(id) = project_route(path, "logs") {
                return match loaded.log_tail(id, LOG_LINES) {
                    Ok(t) => json_body(
                        200,
                        json!({"ok": true, "path": t.path, "missing": t.missing,
                               "lines": t.text.lines().collect::<Vec<_>>()}),
                    ),
                    Err(e) => err_json(404, format!("{e:#}")),
                };
            }
            if let Some(id) = project_route(path, "") {
                return match loaded.project_detail(id) {
                    Ok(detail) => json_body(200, json!({"ok": true, "project": detail})),
                    Err(e) => err_json(404, format!("{e:#}")),
                };
            }
            err_json(404, "not found")
        }
        "PATCH" => {
            let Some(id) = project_route(path, "") else {
                return err_json(404, "not found");
            };
            let mut edit: ProjectEdit = match serde_json::from_slice(&req.body) {
                Ok(edit) => edit,
                Err(e) => return err_json(400, format!("bad payload: {e}")),
            };
            let _guard = state.write_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut loaded = match load(state) {
                Ok(l) => l,
                Err(e) => return err_json(500, format!("{e:#}")),
            };

            // 先校验项目更新数据，避免校验失败留下空分组
            if let Err(e) = loaded.validate_update_project(id, &edit) {
                return err_json(400, format!("{e:#}"));
            }

            let mut created_group_key = None;
            if let Some(ref g) = edit.group {
                let trimmed = g.trim();
                if !trimmed.is_empty() {
                    let groups_before = loaded.groups();
                    let already_exists = groups_before.iter().any(|info| {
                        info.key == trimmed
                            || info.label == trimmed
                            || info.key.eq_ignore_ascii_case(trimmed)
                            || info.label.eq_ignore_ascii_case(trimmed)
                    });
                    match loaded.resolve_or_create_group(g) {
                        Ok(resolved) => {
                            if !already_exists {
                                created_group_key = resolved.clone();
                            }
                            edit.group = Some(resolved.unwrap_or_default());
                        }
                        Err(e) => return err_json(400, format!("{e:#}")),
                    }
                }
            }
            match loaded.update_project(id, edit) {
                Ok(row) => json_body(200, json!({"ok": true, "project": row})),
                Err(e) => {
                    if let Some(ref k) = created_group_key {
                        let _ = loaded.remove_group(k);
                    }
                    err_json(400, format!("{e:#}"))
                }
            }
        }
        "POST" if path == "/api/scan" => {
            #[derive(serde::Deserialize)]
            struct ScanReq {
                dir: String,
                #[serde(default)]
                depth: Option<usize>,
            }
            let r: ScanReq = match serde_json::from_slice(&req.body) {
                Ok(r) => r,
                Err(e) => return err_json(400, format!("bad payload: {e}")),
            };
            match loaded.scan(
                &r.dir,
                r.depth.unwrap_or(tongyun_dev_core::DEFAULT_SCAN_DEPTH),
            ) {
                Ok(report) => json_body(200, json!({"ok": true, "report": report})),
                Err(e) => err_json(400, format!("{e:#}")),
            }
        }
        "POST" if path == "/api/scan/import" => {
            #[derive(serde::Deserialize)]
            struct ImportReq {
                items: Vec<NewProject>,
            }
            let r: ImportReq = match serde_json::from_slice(&req.body) {
                Ok(r) => r,
                Err(e) => return err_json(400, format!("bad payload: {e}")),
            };
            let _guard = state.write_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut loaded = match load(state) {
                Ok(l) => l,
                Err(e) => return err_json(500, format!("{e:#}")),
            };
            match loaded.import_scanned(r.items) {
                Ok(out) => json_body(
                    200,
                    json!({"ok": true, "added": out.added, "skipped": out.skipped}),
                ),
                Err(e) => err_json(400, format!("{e:#}")),
            }
        }
        "POST" if path == "/api/projects" => {
            let mut new: NewProject = match serde_json::from_slice(&req.body) {
                Ok(n) => n,
                Err(e) => return err_json(400, format!("bad payload: {e}")),
            };
            let _guard = state.write_lock.lock().unwrap_or_else(|p| p.into_inner());
            // Reload under the lock so concurrent adds see each other.
            let mut loaded = match load(state) {
                Ok(l) => l,
                Err(e) => return err_json(500, format!("{e:#}")),
            };

            // 先校验项目数据，避免校验失败留下空分组
            if let Err(e) = loaded.validate_new_project(&new) {
                return err_json(400, format!("{e:#}"));
            }

            let mut created_group_key = None;
            if let Some(ref g) = new.group {
                let trimmed = g.trim();
                if !trimmed.is_empty() {
                    let groups_before = loaded.groups();
                    let already_exists = groups_before.iter().any(|info| {
                        info.key == trimmed
                            || info.label == trimmed
                            || info.key.eq_ignore_ascii_case(trimmed)
                            || info.label.eq_ignore_ascii_case(trimmed)
                    });
                    match loaded.resolve_or_create_group(g) {
                        Ok(resolved) => {
                            if !already_exists {
                                created_group_key = resolved.clone();
                            }
                            new.group = resolved;
                        }
                        Err(e) => return err_json(400, format!("{e:#}")),
                    }
                }
            }
            match loaded.add_project(new) {
                Ok(row) => json_body(200, json!({"ok": true, "project": row})),
                Err(e) => {
                    if let Some(ref k) = created_group_key {
                        let _ = loaded.remove_group(k);
                    }
                    err_json(400, format!("{e:#}"))
                }
            }
        }
        "POST" if path == "/api/groups" => {
            #[derive(serde::Deserialize)]
            struct CreateGroupBody {
                key: String,
                #[serde(default)]
                label: Option<String>,
            }
            let new: CreateGroupBody = match serde_json::from_slice(&req.body) {
                Ok(n) => n,
                Err(e) => return err_json(400, format!("bad payload: {e}")),
            };
            let _guard = state.write_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut loaded = match load(state) {
                Ok(l) => l,
                Err(e) => return err_json(500, format!("{e:#}")),
            };
            match loaded.create_group(&new.key, new.label.as_deref()) {
                Ok(group) => json_body(200, json!({"ok": true, "group": group})),
                Err(e) => err_json(400, format!("{e:#}")),
            }
        }
        "POST" => {
            if let Some(id) = project_route(path, "input") {
                #[derive(serde::Deserialize)]
                struct InputBody {
                    #[serde(default)]
                    text: Option<String>,
                    #[serde(default = "default_input_enter")]
                    enter: bool,
                    #[serde(default)]
                    key: Option<String>,
                }
                fn default_input_enter() -> bool {
                    true
                }
                let body: InputBody = match serde_json::from_slice(&req.body) {
                    Ok(b) => b,
                    Err(e) => return err_json(400, format!("bad payload: {e}")),
                };
                let data = match tongyun_dev_core::resolve_input_data(
                    body.text.as_deref(),
                    body.enter,
                    body.key.as_deref(),
                ) {
                    Ok(d) => d,
                    Err(e) => return err_json(400, format!("{e:#}")),
                };
                return match loaded.send_input(id, &data) {
                    Ok(()) => json_body(200, json!({"ok": true})),
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if msg.contains("未在运行") || msg.contains("not running") {
                            err_json(409, msg)
                        } else if msg.contains("管道已满") {
                            // 子进程未读取输入，管道缓冲区已满
                            err_json(409, msg)
                        } else if msg.contains("不能超过 4096 字节") {
                            // 单次输入超出 4096 字节上限
                            err_json(400, msg)
                        } else {
                            err_json(400, msg)
                        }
                    }
                };
            }
            if let Some(raw_key) = group_route(path, "start") {
                let key = url_decode(raw_key);
                // Block until each project is ready so backends finish before frontends.
                let out = match loaded.start_batch(Some(&key), None, None) {
                    Ok(out) => out,
                    Err(e) => return err_json(500, format!("{e:#}")),
                };
                return json_body(
                    200,
                    json!({
                        "ok": out.failed == 0,
                        "group": key,
                        "total": out.total,
                        "succeeded": out.succeeded,
                        "failed": out.failed,
                        "results": out.results
                    }),
                );
            }
            if let Some(raw_key) = group_route(path, "stop") {
                let key = url_decode(raw_key);
                let out = match loaded.stop_batch(Some(&key), None) {
                    Ok(out) => out,
                    Err(e) => return err_json(500, format!("{e:#}")),
                };
                return json_body(
                    200,
                    json!({
                        "ok": out.failed == 0,
                        "group": key,
                        "total": out.total,
                        "succeeded": out.succeeded,
                        "failed": out.failed,
                        "results": out.results
                    }),
                );
            }
            if let Some(raw_key) = group_route(path, "restart") {
                let key = url_decode(raw_key);
                // Same as group start: wait until each project is ready, and skip
                // downstream when a dependency failed. Detached restart launched
                // the next project before the previous one had failed or become ready.
                let out = match loaded.restart_batch(Some(&key), None, None) {
                    Ok(out) => out,
                    Err(e) => return err_json(500, format!("{e:#}")),
                };
                return json_body(
                    200,
                    json!({
                        "ok": out.failed == 0,
                        "group": key,
                        "total": out.total,
                        "succeeded": out.succeeded,
                        "failed": out.failed,
                        "results": out.results
                    }),
                );
            }
            if path == "/api/projects/start-all" {
                let out = match loaded.start_batch_detached(None, None) {
                    Ok(out) => out,
                    Err(e) => return err_json(500, format!("{e:#}")),
                };
                return json_body(
                    200,
                    json!({
                        "ok": out.failed == 0,
                        "total": out.total,
                        "succeeded": out.succeeded,
                        "failed": out.failed,
                        "results": out.results
                    }),
                );
            }
            if path == "/api/projects/stop-all" {
                let out = match loaded.stop_batch(None, None) {
                    Ok(out) => out,
                    Err(e) => return err_json(500, format!("{e:#}")),
                };
                return json_body(
                    200,
                    json!({
                        "ok": out.failed == 0,
                        "total": out.total,
                        "succeeded": out.succeeded,
                        "failed": out.failed,
                        "results": out.results
                    }),
                );
            }
            if path == "/api/projects/restart-all" {
                let out = match loaded.restart_batch_detached(None, None) {
                    Ok(out) => out,
                    Err(e) => return err_json(500, format!("{e:#}")),
                };
                return json_body(
                    200,
                    json!({
                        "ok": out.failed == 0,
                        "total": out.total,
                        "succeeded": out.succeeded,
                        "failed": out.failed,
                        "results": out.results
                    }),
                );
            }
            let action = [
                "start", "stop", "restart", "install", "open", "open-all", "open-dir", "open-log",
                "cursor", "terminal",
            ]
            .into_iter()
            .find_map(|a| project_route(path, a).map(|id| (a, id)));
            let Some((action, id)) = action else {
                return err_json(404, "not found");
            };
            #[derive(serde::Deserialize, Default)]
            struct ActionBody {
                #[serde(default)]
                profile: Option<String>,
                #[serde(default)]
                url: Option<String>,
            }
            let body_action: ActionBody = serde_json::from_slice(&req.body).unwrap_or_default();
            let res: Result<Value> = match action {
                "start" => loaded
                    .start_detached_with_profile(id, body_action.profile.as_deref())
                    .map(|st| json!({"status": st})),
                "stop" => loaded.stop(id).map(|st| json!({"status": st})),
                "restart" => loaded
                    .restart_detached_with_profile(id, body_action.profile.as_deref())
                    .map(|st| json!({"status": st})),
                "terminal" => loaded
                    .open_in_terminal(id, body_action.profile.as_deref())
                    .map(|_| json!({"terminal": true})),
                "install" => loaded.install_deps(id).map(|st| json!({"status": st})),
                "open" => match body_action.url.as_deref() {
                    Some(u) => tongyun_dev_core::open_url(u).map(|_| json!({"url": u})),
                    None => loaded.page_url(id).and_then(|url| {
                        tongyun_dev_core::open_url(&url).map(|_| json!({"url": url}))
                    }),
                },
                "open-all" => loaded.page_urls(id).and_then(|urls| {
                    for link in &urls {
                        tongyun_dev_core::open_url(&link.url)?;
                    }
                    Ok(json!({"opened": urls.len()}))
                }),
                "open-dir" => loaded
                    .root_dir(id)
                    .and_then(|p| tongyun_dev_core::open_path(&p).map(|_| json!({"path": p}))),
                "open-log" => loaded.log_file(id).and_then(|p| {
                    let shown = p.display().to_string();
                    if !p.exists() {
                        anyhow::bail!("日志文件尚未生成：{shown}（项目启动后才会产生日志）");
                    }
                    tongyun_dev_core::open_path(&p).map(|_| json!({"path": shown}))
                }),
                "cursor" => loaded.root_dir(id).and_then(|p| {
                    let shown = p.display().to_string();
                    match tongyun_dev_core::open_in_cursor(&p) {
                        Ok(()) => Ok(json!({"path": shown})),
                        Err(e) => match tongyun_dev_core::open_path(&p) {
                            Ok(()) => Ok(json!({
                                "path": shown,
                                "degraded": true,
                                "warning": format!("{e:#}，已降级为在系统文件管理器中打开项目目录")
                            })),
                            Err(e2) => Err(anyhow::anyhow!("{e:#}；且降级打开目录也失败：{e2:#}")),
                        },
                    }
                }),
                _ => Err(anyhow::anyhow!("unknown action")),
            };
            match res {
                Ok(mut v) => {
                    v["ok"] = json!(true);
                    json_body(200, v)
                }
                Err(e) => {
                    let status = loaded.status(id).ok();
                    json_body(
                        200,
                        json!({"ok": false, "error": format!("{e:#}"), "status": status}),
                    )
                }
            }
        }
        "DELETE" => {
            let Some(id) = project_route(path, "") else {
                return err_json(404, "not found");
            };
            let _guard = state.write_lock.lock().unwrap_or_else(|p| p.into_inner());
            let mut loaded = match load(state) {
                Ok(l) => l,
                Err(e) => return err_json(500, format!("{e:#}")),
            };
            match loaded.remove_project(id) {
                Ok(()) => json_body(200, json!({"ok": true})),
                Err(e) => err_json(404, format!("{e:#}")),
            }
        }
        _ => err_json(405, "method not allowed"),
    }
}

fn send_json(stream: &mut TcpStream, code: u16, v: &Value) -> Result<()> {
    send_response(
        stream,
        code,
        "application/json; charset=utf-8",
        v.to_string().as_bytes(),
    )
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    }
}

fn send_response(stream: &mut TcpStream, code: u16, content_type: &str, body: &[u8]) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {code} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        reason(code),
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    Ok(())
}

const UI_HTML: &str = r##"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>tongyun-dev · 本机项目控制台</title>
  <style>
    /* 橦云设计令牌（与桌面端 apps/desktop/src/tokens.css 同源） */
    :root {
      /* 表面 */
      --ty-bg: #f6f1e9;            /* 奶茶底：窗口/页面背景 */
      --ty-surface: #fffdf9;       /* 卡片、面板 */
      --ty-surface-raised: #f0e9de;/* 工具条、悬停、分段控件底 */
      --ty-surface-sunken: #efe7da;/* 输入框底、代码/日志区浅色版 */
      --ty-border: #e6dccb;
      --ty-border-strong: #d6c8b1;

      /* 文字 */
      --ty-text: #2d323a;          /* 暖碳灰正文 */
      --ty-text-muted: #6b6257;    /* 次要文字（在 --ty-bg 上 ≈5.6:1） */
      --ty-text-subtle: #978b7c;   /* 占位、元信息，只用于非关键文字 */

      /* 抹茶绿：主操作、选中、成功 */
      --ty-matcha: #4d7c5d;
      --ty-matcha-hover: #3f684c;
      --ty-matcha-soft: #c4d7b2;
      --ty-matcha-tint: #edf3e7;
      --ty-on-matcha: #ffffff;

      /* 蜜桃红：强调、危险、删除 */
      --ty-peach: #c2593f;
      --ty-peach-text: #a34e36;
      --ty-peach-soft: #f5dfdb;
      --ty-peach-tint: #fcf2f0;

      /* 辅助色 */
      --ty-honey: #8b6e3c;         /* 警告/待处理文字 */
      --ty-honey-tint: #faf1dd;
      --ty-sky: #3e6f9e;           /* 信息/修改 */
      --ty-sky-tint: #eaf1f7;

      /* 形状 */
      --ty-radius-xs: 6px;
      --ty-radius-sm: 8px;
      --ty-radius-md: 12px;        /* 卡片、窗口圆角 */
      --ty-radius-lg: 16px;
      --ty-radius-pill: 999px;

      /* 阴影：暖灰，弱 */
      --ty-shadow-card: 0 1px 2px rgba(120, 110, 95, 0.06), 0 6px 16px -8px rgba(154, 142, 128, 0.18);
      --ty-shadow-pop: 0 14px 34px -12px rgba(120, 110, 95, 0.28), 0 2px 6px rgba(120, 110, 95, 0.08);
      --ty-focus-ring: 0 0 0 3px rgba(77, 124, 93, 0.28);

      /* 间距 */
      --ty-space-1: 4px;
      --ty-space-2: 8px;
      --ty-space-3: 12px;
      --ty-space-4: 16px;
      --ty-space-5: 24px;
      --ty-space-6: 32px;

      /* 字体 */
      --ty-font-sans: "Plus Jakarta Sans", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, -apple-system, "Segoe UI", sans-serif;
      --ty-font-mono: "JetBrains Mono", "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
      --ty-font-size-xs: 11px;
      --ty-font-size-sm: 12px;
      --ty-font-size-md: 13px;
      --ty-font-size-lg: 15px;
      --ty-font-size-xl: 20px;
    }

    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--ty-bg);
      color: var(--ty-text);
      font-family: var(--ty-font-sans);
      font-size: var(--ty-font-size-md);
      line-height: 1.5;
      -webkit-font-smoothing: antialiased;
    }
    button, input { font: inherit; color: inherit; }
    code { font-family: var(--ty-font-mono); }
    :focus-visible { outline: none; box-shadow: var(--ty-focus-ring); }
    .container { max-width: 1180px; margin: 0 auto; padding: 24px 28px 40px; }

    header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 18px; }
    .brand { display: flex; align-items: center; gap: 10px; }
    .brand h1 { font-size: 17px; font-weight: 700; letter-spacing: -0.01em; }
    .brand p { font-size: var(--ty-font-size-sm); color: var(--ty-text-muted); }
    .header-actions { display: flex; gap: 8px; }

    button {
      display: inline-flex; align-items: center; justify-content: center; gap: 6px;
      height: 30px; padding: 0 12px;
      border-radius: var(--ty-radius-sm); border: 1px solid transparent; background: transparent;
      font-size: var(--ty-font-size-md); font-weight: 500; cursor: pointer; white-space: nowrap;
      transition: background .15s, border-color .15s, color .15s;
    }
    button:disabled { opacity: .45; cursor: not-allowed; }
    button svg { width: 14px; height: 14px; flex-shrink: 0; }
    .btn-primary { background: var(--ty-matcha); color: var(--ty-on-matcha); }
    .btn-primary:hover:not(:disabled) { background: var(--ty-matcha-hover); }
    .btn-secondary { background: var(--ty-surface); border-color: var(--ty-border); }
    .btn-secondary:hover:not(:disabled) { background: var(--ty-surface-raised); border-color: var(--ty-border-strong); }
    .btn-start { background: var(--ty-matcha); color: var(--ty-on-matcha); }
    .btn-start:hover:not(:disabled) { background: var(--ty-matcha-hover); }
    .btn-stop { background: var(--ty-surface); border-color: var(--ty-peach-soft); color: var(--ty-peach-text); }
    .btn-stop:hover:not(:disabled) { background: var(--ty-peach-tint); }
    .btn-outline { background: var(--ty-surface); border-color: var(--ty-border); color: var(--ty-text); }
    .btn-outline:hover:not(:disabled) { background: var(--ty-surface-raised); }
    .btn-danger-text { color: var(--ty-peach-text); padding: 0 8px; margin-left: auto; }
    .btn-danger-text:hover:not(:disabled) { background: var(--ty-peach-tint); }
    .btn-icon { width: 28px; padding: 0; font-size: 20px; color: var(--ty-text-muted); }
    .btn-icon:hover { background: var(--ty-surface-raised); color: var(--ty-text); }

    .toolbar { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; margin-bottom: 18px; }
    .search {
      position: relative; display: flex; align-items: center; width: 320px; height: 34px;
      padding: 0 8px 0 32px; background: var(--ty-surface); border: 1px solid var(--ty-border);
      border-radius: var(--ty-radius-sm); transition: border-color .15s, box-shadow .15s;
    }
    .search:focus-within { border-color: var(--ty-matcha); box-shadow: var(--ty-focus-ring); }
    .search > svg { position: absolute; left: 10px; color: var(--ty-text-subtle); }
    .search input { flex: 1; min-width: 0; border: 0; outline: none; background: transparent; }
    .search input:focus-visible { box-shadow: none; }
    .search input::placeholder { color: var(--ty-text-subtle); }
    .search input::-webkit-search-cancel-button { display: none; }
    .kbd {
      display: inline-grid; place-items: center; min-width: 20px; height: 20px; padding: 0 5px;
      border: 1px solid var(--ty-border); border-radius: 5px; background: var(--ty-surface-raised);
      color: var(--ty-text-muted); font-size: var(--ty-font-size-xs);
    }
    .search-clear { width: 22px; height: 22px; padding: 0; font-size: 16px; color: var(--ty-text-muted); }
    .search-clear:hover { background: var(--ty-surface-raised); }
    .filters { display: flex; gap: 6px; flex-wrap: wrap; }
    .filter {
      height: 28px; padding: 0 11px; border-radius: var(--ty-radius-pill);
      border: 1px solid var(--ty-border); background: var(--ty-surface); color: var(--ty-text-muted);
      font-size: var(--ty-font-size-sm);
    }
    .filter:hover { border-color: var(--ty-border-strong); color: var(--ty-text); }
    .filter .n { font-size: var(--ty-font-size-xs); color: var(--ty-text-subtle); font-variant-numeric: tabular-nums; }
    .filter.on { background: var(--ty-matcha-tint); border-color: var(--ty-matcha-soft); color: var(--ty-matcha); font-weight: 600; }
    .filter.on .n { color: var(--ty-matcha); }
    .toolbar-right { margin-left: auto; display: flex; align-items: center; gap: 8px; }
    .btn-stop-all { height: 26px; padding: 0 9px; font-size: var(--ty-font-size-xs); background: var(--ty-surface); border: 1px solid var(--ty-peach-soft); color: var(--ty-peach-text); border-radius: var(--ty-radius-sm); cursor: pointer; }
    .btn-stop-all:hover { background: var(--ty-peach-tint); }
    .summary { color: var(--ty-text-muted); font-size: var(--ty-font-size-sm); }

    .group-card { grid-column: 1 / -1; display: flex; flex-direction: column; gap: 8px; margin-top: 12px; padding: 12px 14px; background: var(--ty-surface); border: 1px solid var(--ty-border); border-radius: var(--ty-radius-md); }
    .group-card:first-child { margin-top: 0; }
    .group-card-head { display: flex; align-items: center; gap: 8px; font-weight: 600; font-size: 15px; color: var(--ty-text); }
    .group-card .count { font-size: var(--ty-font-size-xs); color: var(--ty-text-muted); font-weight: normal; }
    .group-card .group-actions { margin-left: auto; display: flex; gap: 6px; }
    .group-card .group-actions button { height: 24px; padding: 0 8px; font-size: var(--ty-font-size-xs); }
    .group-order { font-size: var(--ty-font-size-xs); color: var(--ty-text-muted); line-height: 1.5; }
    .group-order code { font-family: var(--ty-font-mono); color: var(--ty-text); }

    .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(340px, 1fr)); gap: 14px; }
    .card {
      background: var(--ty-surface); border: 1px solid var(--ty-border); border-radius: var(--ty-radius-md);
      box-shadow: var(--ty-shadow-card); padding: 16px; display: flex; flex-direction: column; gap: 12px;
      transition: border-color .15s;
    }
    .card:hover { border-color: var(--ty-border-strong); }
    .card.disabled { opacity: .62; }
    .card-head { display: flex; justify-content: space-between; align-items: flex-start; gap: 10px; }
    .card-title { min-width: 0; }
    .card-title h3 { font-size: 15px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .card-title code { font-size: var(--ty-font-size-sm); color: var(--ty-text-muted); }
    .badge {
      display: inline-flex; align-items: center; gap: 6px; flex-shrink: 0; max-width: 55%;
      height: 22px; padding: 0 9px; border-radius: var(--ty-radius-pill);
      font-size: var(--ty-font-size-sm); font-weight: 500;
      background: var(--ty-surface-raised); color: var(--ty-text-muted);
    }
    .badge span:last-child { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; flex-shrink: 0; }
    .badge-running, .badge-ready, .badge-port_open { background: var(--ty-matcha-tint); color: var(--ty-matcha); }
    .badge-starting { background: var(--ty-sky-tint); color: var(--ty-sky); }
    .badge-starting .dot { animation: pulse 1.2s ease-in-out infinite; }
    .badge-failed { background: var(--ty-peach-tint); color: var(--ty-peach-text); }
    .badge-unconfigured { background: var(--ty-honey-tint); color: var(--ty-honey); }
    @keyframes pulse { 50% { opacity: .35; transform: scale(.75); } }

    .chips { display: flex; gap: 6px; flex-wrap: wrap; }
    .chip { font-size: var(--ty-font-size-xs); font-weight: 500; background: var(--ty-surface-sunken); color: var(--ty-text-muted); padding: 1px 7px; border-radius: 5px; }
    .meta-list { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
    .meta-item { background: var(--ty-bg); border-radius: var(--ty-radius-sm); padding: 7px 10px; min-width: 0; }
    .meta-label { display: block; font-size: var(--ty-font-size-xs); color: var(--ty-text-subtle); }
    .meta-val { display: block; font-family: var(--ty-font-mono); font-size: var(--ty-font-size-sm); font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .val-ok { color: var(--ty-matcha); }
    .val-down { color: var(--ty-text-subtle); }
    .val-warn { color: var(--ty-honey); }
    .root { display: flex; align-items: center; gap: 6px; color: var(--ty-text-muted); font-family: var(--ty-font-mono); font-size: var(--ty-font-size-xs); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
    .root svg { width: 13px; height: 13px; flex-shrink: 0; color: var(--ty-text-subtle); }
    .reason, .warn, .note { font-size: var(--ty-font-size-sm); padding: 7px 10px; border-radius: var(--ty-radius-sm); }
    .reason { background: var(--ty-surface-raised); color: var(--ty-text-muted); }
    .warn { background: var(--ty-honey-tint); color: var(--ty-honey); }
    .note { background: var(--ty-honey-tint); color: var(--ty-honey); }
    .fail { font-size: var(--ty-font-size-sm); padding: 7px 10px; border-radius: var(--ty-radius-sm); background: var(--ty-peach-tint); color: var(--ty-peach-text); }
    .card-actions { display: flex; flex-wrap: wrap; gap: 6px; padding-top: 12px; border-top: 1px solid var(--ty-border); }
    .card-actions button { height: 28px; padding: 0 10px; font-size: var(--ty-font-size-sm); }
    .card-actions .btn-danger-text { width: 28px; padding: 0; }
    mark { background: #f6dd9a; color: inherit; border-radius: 2px; }

    .empty {
      grid-column: 1 / -1; display: flex; flex-direction: column; align-items: center; gap: 10px;
      padding: 48px 16px; color: var(--ty-text-muted); background: var(--ty-surface);
      border: 1px dashed var(--ty-border-strong); border-radius: var(--ty-radius-md);
    }
    .empty strong { color: var(--ty-text); font-size: 15px; }

    .modal-backdrop {
      display: none; position: fixed; inset: 0; z-index: 100; align-items: center; justify-content: center;
      background: rgba(45, 50, 58, .32); backdrop-filter: blur(2px); padding: 24px;
    }
    .modal {
      background: var(--ty-surface); border: 1px solid var(--ty-border); border-radius: var(--ty-radius-lg);
      box-shadow: var(--ty-shadow-pop); width: 100%; max-width: 580px; max-height: calc(100vh - 48px);
      overflow: auto; padding: 20px 22px;
    }
    .modal-wide { max-width: 860px; }
    .modal-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px; }
    .modal-head h3 { font-size: 15px; font-weight: 700; }
    .form-group { margin-bottom: 12px; }
    .form-group label { display: block; font-size: var(--ty-font-size-sm); font-weight: 600; color: var(--ty-text-muted); margin-bottom: 5px; }
    .form-group input {
      width: 100%; height: 34px; padding: 0 10px; border-radius: var(--ty-radius-sm);
      border: 1px solid var(--ty-border); background: var(--ty-surface-sunken); outline: none;
    }
    .form-group input:focus { border-color: var(--ty-matcha); background: var(--ty-surface); box-shadow: var(--ty-focus-ring); }
    .modal-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 14px; border-top: 1px solid var(--ty-border); }
    .log-box {
      background: #2b2620; border: 1px solid #3a332b; border-radius: var(--ty-radius-md);
      padding: 10px 0; height: 420px; overflow-y: auto;
      font-family: var(--ty-font-mono); font-size: 12px; line-height: 1.65; color: #e9dfd0; white-space: pre-wrap;
    }
    .log-box .ll { padding: 0 14px; word-break: break-all; }
    .log-box .ll.err { color: #f2a48f; background: rgba(224,109,83,.12); box-shadow: inset 2px 0 0 #e06d53; }
    .log-box .ll.warn { color: #e8c983; background: rgba(217,180,106,.08); box-shadow: inset 2px 0 0 #d9b46a; }
    @media (max-width: 720px) { .search { width: 100%; } .summary { display: none; } }
    .empty-actions { display: flex; gap: 8px; justify-content: center; flex-wrap: wrap; }
    .form-group input[type="checkbox"] { width: auto; }
    .scan-bar { display: flex; gap: 8px; align-items: center; }
    .scan-bar #scanDir { flex: 1; }
    .scan-depth { display: flex; align-items: center; gap: 6px; font-size: 13px; color: var(--ty-text-muted); white-space: nowrap; }
    .scan-depth input { width: 56px; }
    .scan-hint { margin: 10px 0 8px; font-size: 13px; color: var(--ty-text-muted); }
    .scan-list { max-height: 52vh; overflow: auto; display: flex; flex-direction: column; gap: 8px; }
    .scan-empty { padding: 24px; text-align: center; color: var(--ty-text-muted); }
    .scan-row { display: flex; gap: 10px; padding: 12px; border: 1px solid var(--ty-border); border-radius: var(--ty-radius-md); background: var(--ty-surface); }
    .scan-row.done { opacity: 0.6; }
    .scan-pick { padding-top: 2px; }
    .scan-main { flex: 1; min-width: 0; }
    .scan-title { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
    .scan-root { font-family: var(--ty-font-mono); font-size: 12px; color: var(--ty-text-muted); margin: 4px 0 8px; word-break: break-all; }
    .scan-fields { display: grid; grid-template-columns: 1fr 1fr 2fr 90px; gap: 8px; }
    .scan-fields label { display: flex; flex-direction: column; gap: 3px; font-size: 12px; color: var(--ty-text-muted); }
    .scan-fields input, .scan-fields select { width: 100%; box-sizing: border-box; }
    .scan-why { margin-top: 6px; font-size: 12px; color: var(--ty-text-muted); }
    .scan-why ul { margin: 4px 0 0 18px; padding: 0; }
    .chip.conf-high { background: var(--ty-matcha-tint); color: var(--ty-matcha); }
    .chip.conf-medium { background: var(--ty-sky-tint); color: var(--ty-sky); }
    .chip.conf-low { background: var(--ty-honey-tint); color: var(--ty-honey); }
    .tag-profile {
      display: inline-flex; align-items: center; gap: 4px;
      font-size: var(--ty-font-size-xs); font-weight: 600;
      padding: 1px 7px; border-radius: var(--ty-radius-pill);
      background: var(--ty-matcha-tint); color: var(--ty-matcha);
      border: 1px solid var(--ty-matcha-soft);
    }
    .profile-select-bar { display: flex; align-items: center; gap: 6px; font-size: var(--ty-font-size-sm); color: var(--ty-text-muted); margin-top: 2px; }
    .profile-select {
      height: 26px; padding: 0 6px; border-radius: var(--ty-radius-sm);
      border: 1px solid var(--ty-border); background: var(--ty-surface-raised);
      font-size: var(--ty-font-size-sm); color: var(--ty-text); outline: none;
    }
    .profile-select:focus { border-color: var(--ty-matcha); box-shadow: var(--ty-focus-ring); }
    .split-btn { display: inline-flex; position: relative; }
    .split-btn > .btn-outline { border-top-right-radius: 0; border-bottom-right-radius: 0; }
    .split-btn > .split-toggle {
      border-top-left-radius: 0; border-bottom-left-radius: 0;
      padding: 0 6px; margin-left: -1px; min-width: 22px;
    }
    .split-menu {
      display: none; position: absolute; bottom: 100%; right: 0; margin-bottom: 4px;
      background: var(--ty-surface); border: 1px solid var(--ty-border-strong);
      border-radius: var(--ty-radius-sm); box-shadow: var(--ty-shadow-pop);
      min-width: 140px; z-index: 100; flex-direction: column; padding: 4px 0;
    }
    .split-menu.show { display: flex; }
    .split-menu button {
      justify-content: flex-start; height: 28px; padding: 0 10px; border-radius: 0;
      font-size: var(--ty-font-size-sm); text-align: left; width: 100%;
    }
    .split-menu button:hover { background: var(--ty-surface-raised); }
    .split-menu-sep { height: 1px; background: var(--ty-border); margin: 4px 0; }
    .form-group textarea {
      width: 100%; padding: 6px 10px; border-radius: var(--ty-radius-sm);
      border: 1px solid var(--ty-border); background: var(--ty-surface-sunken); outline: none;
      font-family: var(--ty-font-mono); font-size: 12px; line-height: 1.5; resize: vertical;
    }
    .form-group textarea:focus { border-color: var(--ty-matcha); background: var(--ty-surface); box-shadow: var(--ty-focus-ring); }
    @media (max-width: 720px) { .scan-fields { grid-template-columns: 1fr 1fr; } }
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="brand">
        <svg xmlns="http://www.w3.org/2000/svg" width="26" height="30" aria-hidden="true" viewBox="228 200 560 640"><path d="M410,260 Q410,222 444,242 L690,392 Q722,414 690,436 L444,586 Q410,606 410,568 Z" fill="#6E9A5A" stroke="#6E9A5A" stroke-width="44" stroke-linejoin="round"/><rect x="287.0" y="654.0" width="450.0" height="135.0" rx="67.5" fill="#F4B09F"/><circle cx="377.0" cy="699.0" r="90.0" fill="#F4B09F"/><circle cx="485.0" cy="636.0" r="126.0" fill="#F4B09F"/><circle cx="629.0" cy="672.0" r="103.5" fill="#F4B09F"/><circle cx="323.0" cy="726.0" r="63.0" fill="#F4B09F"/><circle cx="710.0" cy="730.5" r="58.5" fill="#F4B09F"/></svg>
        <div>
          <h1>tongyun-dev</h1>
          <p>本机项目控制台</p>
        </div>
      </div>
      <div class="header-actions">
        <button class="btn-secondary" onclick="fetchProjects()"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 0 1-15.5 6.2M3 12A9 9 0 0 1 18.5 5.8"/><path d="M21 4v5h-5M3 20v-5h5"/></svg>刷新</button>
        <button class="btn-secondary" onclick="openScanModal()"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/><path d="M8 11h6M11 8v6"/></svg>扫描目录…</button>
        <button class="btn-primary" onclick="openAddModal()"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>新增项目</button>
      </div>
    </header>

    <div class="toolbar">
      <label class="search">
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/></svg>
        <input id="searchInput" type="search" placeholder="搜索名称、id、端口、目录" aria-label="搜索项目" autocomplete="off" />
        <span class="kbd" id="searchKbd" title="按 / 或 Ctrl/⌘+K 聚焦">/</span>
        <button class="search-clear" id="searchClear" style="display:none" aria-label="清空搜索" title="清空 (Esc)">×</button>
      </label>
      <div class="filters" id="filters" role="tablist" aria-label="按状态筛选"></div>
      <div class="toolbar-right">
        <span class="summary" id="summary"></span>
        <button id="startAllBtn" class="btn-primary" style="display:none;padding:4px 10px;font-size:12px;" onclick="startAllProjects()" title="启动全部未运行的已配置项目">全部启动</button>
        <button id="stopAllBtn" class="btn-stop-all" style="display:none;" onclick="stopAllProjects()" title="停止所有正在运行的服务">全部停止</button>
      </div>
    </div>

    <div class="grid" id="projectGrid">
      <div class="empty">正在扫描本地项目…</div>
    </div>
  </div>

  <!-- Add Project Modal -->
  <div class="modal-backdrop" id="addModal">
    <div class="modal">
      <div class="modal-head">
        <h3 id="projectFormTitle">新增项目</h3>
        <button class="btn-icon" onclick="closeAddModal()" aria-label="关闭">×</button>
      </div>
      <div class="form-group">
        <label>项目 ID（英文，如 student_web）</label>
        <input id="addId" placeholder="例如：my_web" required />
      </div>
      <div class="form-group">
        <label>显示名称（可选，默认同 ID）</label>
        <input id="addName" placeholder="例如：前端开发服务" />
      </div>
      <div class="form-group">
        <label>所属分组（可选）</label>
        <input id="addGroup" list="groupList" placeholder="选择已有分组或输入新分组名" />
        <datalist id="groupList"></datalist>
      </div>
      <div class="form-group">
        <label>项目根路径</label>
        <input id="addRoot" placeholder="例如：~/code/my-web" required />
      </div>
      <div class="form-group">
        <label>启动命令</label>
        <input id="addStart" placeholder="例如：npm run dev" required />
      </div>
      <div class="form-group">
        <label>停止命令（可选）</label>
        <input id="addStop" placeholder="不填则按 pid 进程组停，并清掉本端口监听" />
      </div>
      <div class="form-group">
        <label>端口号</label>
        <input id="addPort" type="number" value="8080" />
      </div>
      <div class="form-group">
        <label>就绪探针 URL（可选；返回 200/301/302 才算启动完成，不填则端口能连上即可）</label>
        <input id="addUrl" placeholder="例如：http://127.0.0.1:3000/login" />
      </div>
      <div class="form-group">
        <label>Node 版本（可选，如 22；自动找 fnm / nvm 装的版本）</label>
        <input id="addNode" placeholder="例如：22" />
      </div>
      <div class="form-group">
        <label>Java 版本（可选，如 17；自动找 sdkman / Adoptium / Zulu 装的 JDK）</label>
        <input id="addJava" placeholder="例如：17" />
      </div>
      <div id="editOptions" hidden>
        <div class="form-group"><label>备用健康探针 URL</label><input id="editHealth" /></div>
        <div class="form-group"><label>日志目录</label><input id="editLogDir" placeholder="默认 ~/.tongyun-dev/logs；可写 ~/logs、{{root}}/logs，或相对项目根的 logs（不能含 ..）" /></div>
        <div class="form-group"><label>日志文件</label><input id="editLog" /></div>
        <div class="form-group"><label>启动超时（秒；留空保持原值）</label><input id="editTimeout" type="number" min="1" /></div>
        <div class="form-group"><label>停止宽限（秒；留空保持原值）</label><input id="editGrace" type="number" min="1" /></div>
        <div class="form-group"><label><input id="editEnabled" type="checkbox" /> 启用项目</label></div>
        <div class="form-group"><label>停用原因</label><input id="editReason" /></div>
        <div class="form-group"><label><input id="editAutoInstall" type="checkbox" /> 启动前自动安装缺失依赖</label></div>
        <div class="form-group"><label><input id="editPty" type="checkbox" /> 伪终端运行（交互脚本用）</label></div>
        <div class="form-group">
          <label>端口传递方式</label>
          <select id="editPortMode">
            <option value="none">none（不传参不设环境变量，仅探活）</option>
            <option value="arg">arg（自动追加命令行参数）</option>
            <option value="env">env（注入环境变量）</option>
            <option value="config">config（自有配置，同 none）</option>
          </select>
        </div>
        <div class="form-group"><label>端口参数名（port_mode=arg 时使用）</label><input id="editPortArg" placeholder="--port" /></div>
        <div class="form-group"><label>端口环境变量名（port_mode=env 时使用）</label><input id="editPortEnv" placeholder="PORT" /></div>
        <div class="form-group"><label>额外环境变量（每行 KEY=VALUE）</label><textarea id="editEnv" rows="3" placeholder="例如：&#10;CI=1&#10;APP_ENV=dev"></textarea></div>
        <div class="form-group"><label>自动应答（每行 匹配文字 => 发送内容）</label><textarea id="editAutoAnswers" rows="3" placeholder="例如：&#10;Select environment => dev&#10;(y/N) => y"></textarea></div>
        <div class="form-group"><label>快捷网页链接（每行一个：标题 => URL 或直接写 URL）</label><textarea id="editLinks" rows="2" placeholder="例如：&#10;前端主页 => http://127.0.0.1:5173/&#10;管理后台 => http://127.0.0.1:5173/admin"></textarea></div>
        <div class="form-group"><label>默认启动环境</label><input id="editDefaultProfile" placeholder="例如：dev" /></div>
        <div class="form-group"><label>环境预设 (Profiles，每行：标识: 启动命令 或 标识: 标签 | 启动命令 [| 端口])</label><textarea id="editProfiles" rows="3" placeholder="例如：&#10;dev: 开发环境 | pnpm dev&#10;mock: Mock 模式 | pnpm dev:mock | 5174"></textarea></div>
        <div class="form-group"><label>依赖服务（可选，多个项目 ID 用逗号或换行分隔）</label><input id="editDependsOn" placeholder="例如：demo_api, demo_db" /></div>
      </div>
      <div class="modal-actions">
        <button class="btn-secondary" onclick="closeAddModal()">取消</button>
        <button class="btn-primary" id="projectSave" onclick="submitAddProject()">保存</button>
      </div>
    </div>
  </div>

  <!-- Scan Modal -->
  <div class="modal-backdrop" id="scanModal">
    <div class="modal modal-wide">
      <div class="modal-head">
        <h3>扫描目录</h3>
        <button class="btn-icon" onclick="closeScanModal()" aria-label="关闭">×</button>
      </div>
      <div class="scan-bar">
        <input id="scanDir" placeholder="要扫描的目录，例如 ~/code" />
        <label class="scan-depth">深度 <input id="scanDepth" type="number" min="1" max="8" value="3" /></label>
        <button class="btn-primary" id="scanGo" onclick="runScan()">扫描</button>
      </div>
      <div class="scan-hint" id="scanHint">只读不改：先列出找到的项目，勾选、改好再添加。已注册过的目录不会重复添加。</div>
      <div class="scan-list" id="scanList"></div>
      <div class="modal-actions">
        <button class="btn-secondary" onclick="closeScanModal()">取消</button>
        <button class="btn-primary" id="scanAdd" onclick="importScan()" disabled>添加选中</button>
      </div>
    </div>
  </div>

  <!-- Log Modal -->
  <div class="modal-backdrop" id="logModal">
    <div class="modal modal-wide">
      <div class="modal-head">
        <h3 id="logTitle">项目日志</h3>
        <button class="btn-icon" onclick="closeLogModal()" aria-label="关闭">×</button>
      </div>
      <div class="log-box" id="logContent">加载中...</div>
      <div class="modal-actions">
        <button class="btn-secondary" onclick="refreshCurrentLogs()">刷新日志</button>
        <button class="btn-primary" onclick="closeLogModal()">关闭</button>
      </div>
    </div>
  </div>

  <script>
    let scanRows = [];
    function openScanModal() {
      document.getElementById('scanModal').classList.add('show');
      const d = document.getElementById('scanDir');
      if (!d.value) d.value = '~/code';
      d.focus();
    }
    function closeScanModal() { document.getElementById('scanModal').classList.remove('show'); }
    async function runScan() {
      const dir = document.getElementById('scanDir').value.trim();
      const depth = parseInt(document.getElementById('scanDepth').value, 10) || 3;
      const list = document.getElementById('scanList');
      const hint = document.getElementById('scanHint');
      if (!dir) { hint.textContent = '先填一个目录'; return; }
      list.innerHTML = '<div class="scan-empty">扫描中…</div>';
      try {
        const data = await postJson('/api/scan', { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ dir, depth }) });
        const r = data.report;
        scanRows = r.candidates.map(c => Object.assign({}, c, { picked: !c.existing_id }));
        const fresh = scanRows.filter(c => !c.existing_id).length;
        hint.textContent = `在 ${r.root} 下看了 ${r.dirs_visited} 个目录，找到 ${r.candidates.length} 个项目，其中 ${fresh} 个是新的` + (r.truncated ? '（目录太多，已截断，可以换个更小的目录）' : '');
        renderScan();
      } catch (e) {
        list.innerHTML = '';
        hint.textContent = '扫描失败：' + e.message;
      }
    }
    function scanField(i, key, value) {
      const row = scanRows[i];
      row[key] = key === 'port' ? (parseInt(value, 10) || 0) : value;
      if (key === 'picked') row.picked = value;
      syncScanButton();
    }
    function syncScanButton() {
      const n = scanRows.filter(r => r.picked && !r.existing_id).length;
      const b = document.getElementById('scanAdd');
      b.disabled = n === 0;
      b.textContent = n ? `添加选中（${n}）` : '添加选中';
    }
    function renderScan() {
      const list = document.getElementById('scanList');
      if (!scanRows.length) {
        list.innerHTML = '<div class="scan-empty">没找到项目。试试上一级目录，或者把深度调大。</div>';
        syncScanButton();
        return;
      }
      const conf = { high: '把握大', medium: '大概率', low: '猜的' };
      list.innerHTML = scanRows.map((c, i) => {
        const done = !!c.existing_id;
        const tools = [c.java ? 'Java ' + escapeHtml(c.java) : '', c.node ? 'Node ' + escapeHtml(c.node) : ''].filter(Boolean).map(t => `<span class="chip">${t}</span>`).join('');
        const profileKeys = (c.profiles && typeof c.profiles === 'object') ? Object.keys(c.profiles) : [];
        const profileChips = profileKeys.length ? `<span class="chip">Profile: ${profileKeys.map(escapeHtml).join(', ')}</span>` : '';
        const profileField = profileKeys.length ? `
              <label class="full" style="grid-column: 1 / -1;">环境预设（Profile 候选：${profileKeys.map(escapeHtml).join('、')}）· 默认生效 Profile
                <select ${done ? 'disabled' : ''} onchange="scanField(${i}, 'default_profile', this.value)">
                  <option value="">（不设默认，直接使用基础启动命令）</option>
                  ${profileKeys.map(k => {
                    const pConf = c.profiles[k] || {};
                    const portText = pConf.port ? ' · 端口 :' + pConf.port : '';
                    const sel = (c.default_profile === k) ? 'selected' : '';
                    const rec = (k === 'dev' || c.default_profile === k) ? '（推断推荐）' : '';
                    return `<option value="${escapeHtml(k)}" ${sel}>${escapeHtml(k)}${portText}${rec}</option>`;
                  }).join('')}
                </select>
              </label>` : '';
        return `<div class="scan-row ${done ? 'done' : ''}">
          <label class="scan-pick"><input type="checkbox" ${c.picked && !done ? 'checked' : ''} ${done ? 'disabled' : ''} onchange="scanField(${i}, 'picked', this.checked)" /></label>
          <div class="scan-main">
            <div class="scan-title"><strong>${escapeHtml(c.label)}</strong><span class="chip conf-${c.confidence}">${conf[c.confidence] || c.confidence}</span>${tools}${profileChips}${done ? `<span class="chip">已注册为 ${escapeHtml(c.existing_id)}</span>` : ''}${c.needs_tomcat ? '<span class="chip">需要 Tomcat</span>' : ''}</div>
            <div class="scan-root">${escapeHtml(c.root)}</div>
            <div class="scan-fields">
              <label>ID<input value="${escapeHtml(c.id)}" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'id', this.value)" /></label>
              <label>名称<input value="${escapeHtml(c.name)}" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'name', this.value)" /></label>
              <label class="wide">启动命令<input value="${escapeHtml(c.start)}" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'start', this.value)" /></label>
              <label class="narrow">端口<input type="number" value="${c.port || ''}" placeholder="?" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'port', this.value)" /></label>
              ${profileField}
            </div>
            <details class="scan-why"><summary>为什么这么猜</summary><ul>${c.why.map(w => `<li>${escapeHtml(w)}</li>`).join('')}</ul></details>
          </div>
        </div>`;
      }).join('');
      syncScanButton();
    }
    async function importScan() {
      const picked = scanRows.filter(r => r.picked && !r.existing_id);
      if (!picked.length) return;
      const items = picked.map(c => ({
        id: c.id, name: c.name, root: c.root, start: c.start, stop: c.stop || null,
        port: c.port || null, health_url: c.health_url || null, login_url: c.login_url || null,
        node: c.node || null, java: c.java || null, background: c.background,
        port_mode: c.port_mode || null,
        env: c.env || {},
        default_profile: (c.default_profile && c.default_profile.trim()) ? c.default_profile.trim() : null,
        profiles: c.profiles || {},
        enabled: c.needs_tomcat ? false : null,
        reason: c.needs_tomcat ? 'Tomcat WAR：先填 Tomcat 安装目录（tomcat 字段）再启用' : null,
      }));
      try {
        const data = await postJson('/api/scan/import', { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ items }) });
        const msg = [`已添加 ${data.added.length} 个项目`].concat(data.skipped.map(s => `跳过 ${s.id}：${s.reason}`));
        closeScanModal();
        if (data.skipped.length) alert(msg.join('\n'));
        fetchProjects();
      } catch (e) {
        alert('保存失败：' + e.message);
      } finally { save.disabled = false; }
    }
    let currentLogId = null;
    let lastProjects = [];
    let lastGroups = [];
    const busyIds = {};
    let query = '';
    let statusFilter = 'all';
    const FILTERS = [['all', '全部'], ['running', '运行中'], ['starting', '启动中'], ['failed', '失败'], ['unconfigured', '未配置']];
    const ICON_FOLDER = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';

    async function postJson(url, opts) {
      const res = await fetch(url, Object.assign({ method: 'POST' }, opts || {}));
      let data = {};
      try { data = await res.json(); } catch (e) {}
      if (!res.ok || data.ok === false) throw new Error(data.error || ('HTTP ' + res.status));
      return data;
    }

    function formatEnvLines(env) {
      if (!env) return '';
      return Object.entries(env).map(([k, v]) => `${k}=${v}`).join('\n');
    }
    function parseEnvLines(text) {
      const env = {};
      if (!text || !text.trim()) return env;
      for (const raw of text.split('\n')) {
        const line = raw.trim();
        if (!line) continue;
        const eq = line.indexOf('=');
        if (eq === -1) throw new Error('环境变量格式必须为 KEY=VALUE：' + line);
        const k = line.slice(0, eq).trim();
        if (!k) throw new Error('环境变量键名不能为空：' + line);
        env[k] = line.slice(eq + 1).trim();
      }
      return env;
    }
    function formatAutoAnswersLines(answers) {
      if (!answers || !answers.length) return '';
      return answers.map(a => `${a.expect} => ${a.send}`).join('\n');
    }
    function parseAutoAnswersLines(text) {
      const answers = [];
      if (!text || !text.trim()) return answers;
      for (const raw of text.split('\n')) {
        const line = raw.trim();
        if (!line) continue;
        const arrow = line.indexOf('=>');
        if (arrow === -1) throw new Error('自动应答格式必须为 匹配文字 => 发送内容：' + line);
        const expect = line.slice(0, arrow).trim();
        if (!expect) throw new Error('自动应答匹配文字不能为空：' + line);
        const send = line.slice(arrow + 2).trim();
        answers.push({ expect, send, once: true, regex: false });
      }
      return answers;
    }

    function formatLinksLines(links) {
      if (!links || !links.length) return '';
      return links.map(l => (l.title && l.title !== l.url ? `${l.title} => ${l.url}` : l.url)).join('\n');
    }
    function parseLinksLines(text) {
      const links = [];
      if (!text || !text.trim()) return links;
      for (const raw of text.split('\n')) {
        const line = raw.trim();
        if (!line) continue;
        if (line.includes('=>')) {
          const idx = line.indexOf('=>');
          const title = line.slice(0, idx).trim();
          const url = line.slice(idx + 2).trim();
          if (url) links.push({ title: title || url, url });
        } else {
          links.push({ title: line, url: line });
        }
      }
      return links;
    }
    function formatProfilesLines(profiles) {
      if (!profiles) return '';
      return Object.entries(profiles).map(([k, p]) => {
        if (p.label && p.port) return `${k}: ${p.label} | ${p.start || ''} | ${p.port}`;
        if (p.label) return `${k}: ${p.label} | ${p.start || ''}`;
        if (p.port) return `${k}: ${p.start || ''} | ${p.port}`;
        return `${k}: ${p.start || ''}`;
      }).join('\n');
    }
    function parseProfilesLines(text) {
      const profiles = {};
      if (!text || !text.trim()) return profiles;
      for (const raw of text.split('\n')) {
        const line = raw.trim();
        if (!line) continue;
        const colon = line.indexOf(':');
        if (colon === -1) continue;
        const key = line.slice(0, colon).trim();
        if (!key) continue;
        const rest = line.slice(colon + 1).trim();
        const parts = rest.split('|').map(s => s.trim());
        if (parts.length === 1) {
          profiles[key] = { start: parts[0] || null };
        } else if (parts.length === 2) {
          const num = Number(parts[1]);
          if (!isNaN(num) && num > 0 && num <= 65535) {
            profiles[key] = { start: parts[0] || null, port: num };
          } else {
            profiles[key] = { label: parts[0] || '', start: parts[1] || null };
          }
        } else if (parts.length >= 3) {
          const num = Number(parts[2]);
          const port = (!isNaN(num) && num > 0 && num <= 65535) ? num : null;
          profiles[key] = { label: parts[0] || '', start: parts[1] || null, port };
        }
      }
      return profiles;
    }

    function onProfileChange(id, value) {
      selectedProfiles[id] = value;
    }

    function getSelectedProfile(id) {
      const p = lastProjects.find(x => x.id === id);
      if (selectedProfiles[id] !== undefined) return selectedProfiles[id] || null;
      return (p && (p.active_profile || p.default_profile)) || null;
    }

    function toggleSplitMenu(e, id) {
      e.stopPropagation();
      const menu = document.getElementById(`splitMenu_${id}`);
      if (!menu) return;
      const wasShow = menu.classList.contains('show');
      closeAllSplitMenus();
      if (!wasShow) menu.classList.add('show');
    }

    function closeAllSplitMenus() {
      document.querySelectorAll('.split-menu.show').forEach(m => m.classList.remove('show'));
    }

    async function runAction(id, label, path, failText, body) {
      busyIds[id] = label;
      renderProjects(lastProjects);
      try {
        const opts = body ? { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) } : {};
        await postJson(`/api/projects/${id}/${path}`, opts);
      } catch (e) {
        alert(failText + '：' + e.message);
      } finally {
        delete busyIds[id];
        fetchProjects();
      }
    }

    async function fetchProjects() {
      try {
        const [res, gRes] = await Promise.all([
          fetch('/api/projects'),
          fetch('/api/groups').catch(() => null)
        ]);
        const data = await res.json();
        if (!Array.isArray(data)) throw new Error(data.error || '加载失败');
        lastProjects = data;
        if (gRes && gRes.ok) {
          try {
            const gData = await gRes.json();
            if (gData && Array.isArray(gData.groups)) {
              lastGroups = gData.groups;
              renderGroupDatalist();
            }
          } catch (_) {}
        }
        renderProjects(data);
      } catch (e) {
        console.error(e);
      }
    }

    // ── 搜索与筛选（语义同桌面端 projectFilter.ts）──
    function tokens(q) { return q.trim().toLowerCase().split(/\s+/).filter(Boolean); }
    function matchesQuery(p, q) {
      const ts = tokens(q);
      if (!ts.length) return true;
      const hay = [p.name, p.id, p.port > 0 ? String(p.port) : '', p.root].join('\n').toLowerCase();
      return ts.every(t => hay.includes(t));
    }
    function isConfigured(p) { return p.configured !== false; }
    function matchesStatus(p, f) {
      if (f === 'all') return true;
      if (f === 'unconfigured') return !isConfigured(p);
      if (!isConfigured(p) || p.enabled === false) return false;
      const st = p.state || 'stopped';
      if (f === 'running') return st === 'ready' || st === 'port_open';
      if (f === 'starting') return st === 'starting' || st === 'installing';
      if (f === 'failed') return st === 'failed';
      return true;
    }
    function highlight(text) {
      const safe = escapeHtml(text);
      const ts = tokens(query);
      if (!ts.length || !text) return safe;
      const lower = String(text).toLowerCase();
      const marks = new Array(lower.length).fill(false);
      ts.forEach(t => { let i = 0; while ((i = lower.indexOf(t, i)) !== -1) { for (let k = i; k < i + t.length; k++) marks[k] = true; i += Math.max(1, t.length); } });
      let out = '', on = false, buf = '';
      const flush = () => { if (buf) { out += on ? `<mark>${escapeHtml(buf)}</mark>` : escapeHtml(buf); buf = ''; } };
      for (let i = 0; i < text.length; i++) { if (marks[i] !== on) { flush(); on = marks[i]; } buf += text[i]; }
      flush();
      return out;
    }
    function renderFilters(projects) {
      const counts = {};
      FILTERS.forEach(([k]) => counts[k] = 0);
      projects.forEach(p => { if (!matchesQuery(p, query)) return; FILTERS.forEach(([k]) => { if (matchesStatus(p, k)) counts[k]++; }); });
      document.getElementById('filters').innerHTML = FILTERS.map(([k, label]) =>
        `<button class="filter ${statusFilter === k ? 'on' : ''}" role="tab" aria-selected="${statusFilter === k}" onclick="setFilter('${k}')">${label} <span class="n">${counts[k]}</span></button>`
      ).join('');
      const running = projects.filter(p => p.enabled !== false && (p.state === 'ready' || p.state === 'port_open')).length;
      const canStartAny = projects.some(p => p.enabled !== false && isConfigured(p) && (p.state === 'stopped' || p.state === 'failed'));
      document.getElementById('summary').textContent = `${projects.length} 个项目 · ${running} 个运行中`;
      const stopAllBtn = document.getElementById('stopAllBtn');
      if (stopAllBtn) {
        stopAllBtn.style.display = running > 0 ? '' : 'none';
      }
      const startAllBtn = document.getElementById('startAllBtn');
      if (startAllBtn) {
        startAllBtn.style.display = canStartAny ? '' : 'none';
      }
    }
    function setFilter(k) { statusFilter = k; renderProjects(lastProjects); }
    function clearFilters() {
      query = ''; statusFilter = 'all';
      const input = document.getElementById('searchInput');
      input.value = '';
      syncSearchUi();
      renderProjects(lastProjects);
    }
    function syncSearchUi() {
      document.getElementById('searchClear').style.display = query ? '' : 'none';
      document.getElementById('searchKbd').style.display = query ? 'none' : '';
    }

    function renderProjectCard(p) {
      const isRunning = p.running;
      const enabled = p.enabled !== false;
      const configured = isConfigured(p);
      const portOpen = p.port_open;
      const probeOk = p.login_probe.ok;
      const code = p.login_probe.http_code;
      const busy = busyIds[p.id];
      const chips = [p.node ? `Node ${escapeHtml(p.node)}` : '', p.java ? `Java ${escapeHtml(p.java)}` : '']
        .filter(Boolean).map(c => `<span class="chip">${c}</span>`).join('');
      const conflict = p.port_conflict
        ? `<div class="warn">端口 ${p.port} 被占用${p.port_holder ? '：' + escapeHtml(p.port_holder) : '（非本项目进程）'}</div>`
        : '';

      const state = p.state || (isRunning ? 'ready' : 'stopped');
      let badgeClass = 'badge-stopped';
      let stateText = '已停止';
      if (!configured) {
        stateText = '未配置';
        badgeClass = 'badge-unconfigured';
      } else if (!enabled) {
        stateText = '已停用';
        badgeClass = 'badge-stopped';
      } else if (state === 'installing') {
        badgeClass = 'badge-starting';
        const n = p.elapsed_secs != null ? p.elapsed_secs : 0;
        stateText = `安装依赖中 ${n} 秒`;
      } else if (state === 'starting') {
        badgeClass = 'badge-starting';
        const n = p.elapsed_secs != null ? p.elapsed_secs : 0;
        stateText = `启动中 ${n} 秒`;
      } else if (state === 'port_open') {
        badgeClass = 'badge-port_open';
        stateText = '端口已通（等登录页）';
      } else if (state === 'ready') {
        badgeClass = 'badge-ready';
        stateText = '就绪';
      } else if (state === 'failed') {
        badgeClass = 'badge-failed';
        stateText = p.failure && p.failure.indexOf('依赖') !== -1 ? '依赖问题' : (p.failure ? '启动失败' : '失败：未知原因');
      } else {
        badgeClass = 'badge-stopped';
        stateText = '已停止';
      }

      const canStart = enabled && configured && (state === 'stopped' || state === 'failed');
      const canStop = enabled && (state === 'installing' || state === 'starting' || state === 'port_open' || state === 'ready' || state === 'failed');
      const canOpenPage = enabled && (state === 'port_open' || state === 'ready');

      let actionBtns = '';
      if (!enabled) {
        actionBtns = `<button class="btn-start" disabled title="${escapeHtml(p.reason)}">启动</button>`;
      } else if (!configured) {
        actionBtns = `<button class="btn-start" disabled title="缺端口或启动命令">启动</button>`;
      } else if (busy) {
        actionBtns = `<button class="btn-secondary" disabled>${busy}</button>`;
      } else {
        if (canStart) {
          actionBtns += `<button class="btn-start" onclick="startProject('${p.id}')"><svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15l12-7.5z"/></svg>启动</button>`;
        }
        if (p.deps_missing && state !== 'installing') {
          actionBtns += `<button class="btn-secondary" onclick="installDeps('${p.id}')" title="${escapeHtml(p.install_command || '')}">安装依赖</button>`;
        }
        if (canStop) {
          actionBtns += `<button class="btn-stop" onclick="stopProject('${p.id}')"><svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"/></svg>停止</button>`;
        }
      }

      const portText = !configured || !p.port ? '未配置' : `${p.port} · ${portOpen ? '已通' : '未通'}`;
      const portCls = !configured || !p.port ? 'val-warn' : portOpen ? 'val-ok' : 'val-down';
      const probeText = !configured ? '—' : code ? (probeOk ? `正常 ${code}` : `异常 ${code}`) : (isRunning ? '未就绪' : '未响应');

      const hasProfiles = p.profiles && Object.keys(p.profiles).length > 0;
      let profileBar = '';
      if (hasProfiles) {
        const curProf = getSelectedProfile(p.id) || '';
        const opts = [
          `<option value="">默认配置${p.default_profile ? ' (' + escapeHtml(p.default_profile) + ')' : ''}</option>`
        ];
        for (const [k, prof] of Object.entries(p.profiles)) {
          const label = prof.label ? `${k} (${prof.label})` : k;
          const sel = curProf === k ? 'selected' : '';
          opts.push(`<option value="${escapeHtml(k)}" ${sel}>${escapeHtml(label)}</option>`);
        }
        profileBar = `<div class="profile-select-bar"><span class="profile-label">环境</span><select class="profile-select" onchange="onProfileChange('${p.id}', this.value)">${opts.join('')}</select></div>`;
      }

      let webBtn = '';
      if (p.links && p.links.length > 1) {
        const menuItems = p.links.map(l =>
          `<div class="split-item" onclick="openProjectLink('${p.id}', '${escapeHtml(l.url)}')">${escapeHtml(l.title || l.url)}</div>`
        ).join('');
        webBtn = `
          <div class="split-btn">
            <button class="btn-outline" ${canOpenPage ? `onclick="openProjectInBrowser('${p.id}')"` : 'disabled'} title="打开主网页">网页</button>
            <button class="btn-outline btn-arrow" ${canOpenPage ? `onclick="toggleSplitMenu(event, '${p.id}')"` : 'disabled'} title="更多网页链接">▾</button>
            <div id="splitMenu_${p.id}" class="split-menu">
              ${menuItems}
              <div class="split-divider"></div>
              <div class="split-item" onclick="openAllProjectLinks('${p.id}')">全部打开</div>
            </div>
          </div>
        `;
      } else {
        webBtn = `<button class="btn-outline" ${canOpenPage ? `onclick="openProjectInBrowser('${p.id}')"` : 'disabled'} title="在系统浏览器打开登录/访问页面">网页</button>`;
      }

      return `
        <div class="card ${enabled ? '' : 'disabled'}">
          <div class="card-head">
            <div class="card-title">
              <h3>${highlight(p.name)}</h3>
              <code>${highlight(p.id)}</code>
            </div>
            <div style="display:flex;align-items:center;gap:6px;">
              ${p.active_profile ? `<span class="tag-profile" title="当前运行环境：${escapeHtml(p.active_profile)}">⚡ ${escapeHtml(p.active_profile)}</span>` : ''}
              <div class="badge ${badgeClass}" title="${stateText}">
                <span class="dot"></span>
                <span>${stateText}</span>
              </div>
            </div>
          </div>
          ${!enabled ? `<div class="reason">停用原因：${escapeHtml(p.reason)}</div>` : ''}
          ${!configured && p.resolve_error ? `<div class="fail">${escapeHtml(p.resolve_error)}</div>` : ''}
          ${!configured && !p.resolve_error ? `<div class="note">还没配置好：缺端口或启动命令。打开这个项目的「设置」填好就能启动。</div>` : ''}
          ${conflict}
          ${configured && enabled && state === 'failed' && p.failure ? `<div class="fail">${escapeHtml(p.failure)}</div>` : ''}
          ${configured && enabled && p.hints && p.hints.length ? p.hints.map(h => `<div class="note">💡 ${escapeHtml(h)}</div>`).join('') : ''}
          ${state === 'failed' && p.pid ? `<div class="note">进程 PID ${p.pid} 仍在，停止可清理，重启可重新尝试。</div>` : ''}
          ${profileBar}
          <div class="meta-list">
            <div class="meta-item"><span class="meta-label">端口</span><span class="meta-val ${portCls}">${portText}</span></div>
            <div class="meta-item"><span class="meta-label">进程</span><span class="meta-val ${p.pid ? '' : 'val-down'}">${p.pid ? 'PID ' + p.pid : '未运行'}</span></div>
            <div class="meta-item"><span class="meta-label">登录页</span><span class="meta-val ${probeOk ? 'val-ok' : 'val-down'}">${probeText}</span></div>
          </div>
          <div class="root" title="${escapeHtml(p.root)}">${ICON_FOLDER}<span>${highlight(p.root)}</span></div>
          ${chips ? `<div class="chips">${chips}</div>` : ''}
          ${p.depends_on && p.depends_on.length > 0 ? `<div style="font-size:11px;color:var(--ty-text-muted);margin:3px 0 6px;">依赖：${escapeHtml(p.depends_on.join(', '))}</div>` : ''}
          <div class="card-actions">
            ${actionBtns}
            <button class="btn-outline" ${enabled && configured && !busy ? `onclick="restartProject('${p.id}')"` : 'disabled'}>重启</button>
            <button class="btn-outline" onclick="openTerminal('${p.id}')" title="在系统终端打开">终端</button>
            <button class="btn-outline" onclick="editProject('${p.id}')">设置</button>
            ${webBtn}
            <button class="btn-outline" onclick="openProjectDir('${p.id}')" title="在系统文件管理器中打开源码目录">目录</button>
            <button class="btn-outline" onclick="openInCursor('${p.id}')" title="用 Cursor 打开项目目录">Cursor</button>
            <button class="btn-outline" onclick="openLogFile('${p.id}')" title="用系统默认程序打开日志文件">日志文件</button>
            <button class="btn-outline" onclick="copyStartCommand('${p.id}')" title="复制启动命令">复制命令</button>
            <button class="btn-outline" data-id="${escapeHtml(p.id)}" data-name="${escapeHtml(p.name)}" onclick="openLogModal(this.dataset.id, this.dataset.name)">日志</button>
            <button class="btn-danger-text" onclick="deleteProject('${p.id}')" title="从配置里删除这个项目" aria-label="删除"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg></button>
          </div>
        </div>
      `;
    }

    function renderProjects(projects) {
      const grid = document.getElementById('projectGrid');
      renderFilters(projects);
      if (!projects.length) {
        grid.innerHTML = '<div class="empty"><strong>还没有项目</strong><span>选一个放代码的目录（比如 ~/code），自动认出里面的 Spring Boot、Vite、Vue、Next 等项目；也可以手动新增。</span><div class="empty-actions"><button class="btn-primary" onclick="openScanModal()">扫描一个目录</button><button class="btn-secondary" onclick="openAddModal()">手动新增</button></div></div>';
        return;
      }
      const visible = projects.filter(p => matchesQuery(p, query) && matchesStatus(p, statusFilter));
      if (!visible.length) {
        grid.innerHTML = '<div class="empty"><strong>没有匹配的项目</strong><span>换个关键词，或者清除筛选看全部项目。</span><button class="btn-secondary" onclick="clearFilters()">清除筛选</button></div>';
        return;
      }
      const hasAnyGroup = projects.some(p => p.group && p.group.trim());
      if (!hasAnyGroup) {
        grid.innerHTML = visible.map(renderProjectCard).join('');
        return;
      }
      const groupMap = {};
      lastGroups.forEach(g => {
        if (g && g.key) groupMap[g.key] = g;
      });
      const groups = {};
      visible.forEach(p => {
        const key = (p.group && p.group.trim()) || '';
        const info = key ? groupMap[key] : null;
        const gKey = key || '__ungrouped__';
        const label = info ? (info.label || key) : (key || '未分组');
        const order = info && info.order != null ? info.order : null;
        if (!groups[gKey]) {
          groups[gKey] = { key: gKey, label, order, projects: [] };
        }
        groups[gKey].projects.push(p);
      });
      Object.values(groups).forEach(g => {
        const members = (groupMap[g.key] && groupMap[g.key].members) || [];
        g.members = members;
        g.projects.sort((a, b) => {
          const ia = members.indexOf(a.id);
          const ib = members.indexOf(b.id);
          const ra = ia === -1 ? 100000 : ia;
          const rb = ib === -1 ? 100000 : ib;
          if (ra !== rb) return ra - rb;
          return String(a.name || a.id).localeCompare(String(b.name || b.id), 'zh-CN');
        });
      });
      const groupList = Object.values(groups).sort((a, b) => {
        if (a.key === '__ungrouped__') return 1;
        if (b.key === '__ungrouped__') return -1;
        if (a.order != null && b.order != null) return a.order - b.order;
        if (a.order != null) return -1;
        if (b.order != null) return 1;
        return a.label.localeCompare(b.label, 'zh-CN');
      });
      grid.innerHTML = groupList.map(g => {
        let groupActionBtns = '';
        if (g.key !== '__ungrouped__') {
          groupActionBtns = `
            <div class="group-actions">
              <button class="btn-start" onclick="startGroup('${escapeHtml(g.key)}')">全部启动</button>
              <button class="btn-outline" onclick="stopGroup('${escapeHtml(g.key)}')">停止本组</button>
            </div>
          `;
        }
        const order = (g.members || []).length
          ? `<div class="group-order">启动顺序（前一个就绪后再启下一个）：${g.members.map(id => `<code>${escapeHtml(id)}</code>`).join(' → ')}</div>`
          : '';
        const heading = `<div class="group-card"><div class="group-card-head"><span>${escapeHtml(g.label)}</span><span class="count">${g.projects.length} 个项目</span>${groupActionBtns}</div>${order}</div>`;
        return heading + g.projects.map(renderProjectCard).join('');
      }).join('');
    }

    async function startGroup(key) {
      if (!confirm(`确定启动分组【${key}】下的所有项目？`)) return;
      try {
        await postJson(`/api/groups/${encodeURIComponent(key)}/start`);
      } catch (e) {
        alert('启动分组失败：' + e.message);
      } finally {
        fetchProjects();
      }
    }

    async function stopGroup(key) {
      if (!confirm(`确定停止分组【${key}】下的所有项目？`)) return;
      try {
        await postJson(`/api/groups/${encodeURIComponent(key)}/stop`);
      } catch (e) {
        alert('停止分组失败：' + e.message);
      } finally {
        fetchProjects();
      }
    }

    async function startAllProjects() {
      if (!confirm('确定启动所有未运行的已配置项目？')) return;
      try {
        await postJson('/api/projects/start-all');
      } catch (e) {
        alert('全部启动失败：' + e.message);
      } finally {
        fetchProjects();
      }
    }

    async function stopAllProjects() {
      if (!confirm('确定停止所有正在运行的项目？')) return;
      try {
        await postJson('/api/projects/stop-all');
      } catch (e) {
        alert('停止全部失败：' + e.message);
      } finally {
        fetchProjects();
      }
    }

    async function openInCursor(id) {
      try {
        const res = await postJson(`/api/projects/${id}/cursor`);
        if (res && res.degraded) {
          alert(res.warning || '未找到 Cursor，已降级为在系统文件管理器中打开项目目录');
        }
      } catch (e) {
        alert('用 Cursor 打开失败：' + e.message);
      }
    }

    async function openLogFile(id) {
      try {
        await postJson(`/api/projects/${id}/open-log`);
      } catch (e) {
        alert('打开日志文件失败：' + e.message);
      }
    }

    async function copyStartCommand(id) {
      const p = lastProjects.find(x => x.id === id);
      const prof = getSelectedProfile(id);
      const fromProfile = prof && p && p.profile_start_commands && p.profile_start_commands[prof];
      const cmd = fromProfile ? String(fromProfile) : ((p && p.start_command) ? String(p.start_command) : '');
      if (!cmd) {
        alert('没有可复制的启动命令');
        return;
      }
      try {
        await navigator.clipboard.writeText(cmd);
        alert('已复制启动命令');
      } catch (e) {
        prompt('复制启动命令', cmd);
      }
    }

    async function openProjectDir(id) {
      try {
        await postJson(`/api/projects/${id}/open-dir`);
      } catch (e) {
        alert('打开目录失败：' + e.message);
      }
    }

    function startProject(id) {
      const prof = getSelectedProfile(id);
      return runAction(id, '启动中…', 'start', '启动失败', prof ? { profile: prof } : undefined);
    }

    function installDeps(id) {
      return runAction(id, '安装依赖中…', 'install', '安装依赖失败');
    }

    function stopProject(id) {
      return runAction(id, '停止中…', 'stop', '停止失败');
    }

    function restartProject(id) {
      const prof = getSelectedProfile(id);
      return runAction(id, '重启中…', 'restart', '重启失败', prof ? { profile: prof } : undefined);
    }

    async function openTerminal(id) {
      const prof = getSelectedProfile(id);
      try {
        await postJson(`/api/projects/${id}/terminal`, {
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(prof ? { profile: prof } : {})
        });
      } catch (e) {
        alert('打开终端失败：' + e.message);
      }
    }

    async function openProjectInBrowser(id) {
      try {
        await postJson(`/api/projects/${id}/open`);
      } catch (e) {
        alert('打开页面失败：' + e.message);
      }
    }

    async function openProjectLink(id, url) {
      try {
        await postJson(`/api/projects/${id}/open`, {
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ url })
        });
        closeAllSplitMenus();
      } catch (e) {
        alert('打开页面失败：' + e.message);
      }
    }

    async function openAllProjectLinks(id) {
      try {
        await postJson(`/api/projects/${id}/open-all`);
        closeAllSplitMenus();
      } catch (e) {
        alert('全部打开失败：' + e.message);
      }
    }

    async function deleteProject(id) {
      if (!confirm(`确定要删除项目 ${id} 吗？只从配置里移除，不删项目文件。`)) return;
      try {
        await postJson(`/api/projects/${id}`, { method: 'DELETE' });
        fetchProjects();
      } catch (e) {
        alert('删除失败：' + e.message);
      }
    }

    let editingProject = null;
    function renderGroupDatalist() {
      const dl = document.getElementById('groupList');
      if (!dl) return;
      dl.innerHTML = (lastGroups || []).map(g => `<option value="${escapeHtml(g.label || g.key)}">${escapeHtml(g.key !== (g.label || g.key) ? g.key : '')}</option>`).join('');
    }
    function getInitialGroupLabel(key) {
      const trimmed = (key || '').trim();
      if (!trimmed) return '';
      const found = (lastGroups || []).find(g => g.key === trimmed);
      return (found && found.label) ? found.label : trimmed;
    }
    function generateGroupKey(label, existingKeys = []) {
      const trimmed = (label || '').trim();
      const isAscii = /^[\x00-\x7F]+$/.test(trimmed);
      if (isAscii && trimmed.length > 0) {
        let candidate = trimmed
          .toLowerCase()
          .replace(/[^a-z0-9_-]+/g, '-')
          .replace(/^-+|-+$/g, '');
        if (candidate.length > 40) {
          candidate = candidate.slice(0, 40).replace(/-+$/, '');
        }
        if (candidate.length > 0 && !existingKeys.includes(candidate)) {
          return candidate;
        }
      }
      let idx = 1;
      while (existingKeys.includes(`g${idx}`)) {
        idx++;
      }
      return `g${idx}`;
    }
    function resolveFormGroup(rawInput, groups = []) {
      const trimmed = (rawInput || '').trim();
      if (!trimmed) return { key: '' };
      const allExistingKeys = Array.from(new Set(groups.map(g => g.key).filter(Boolean)));
      const exact = groups.find(g => g.label === trimmed || g.key === trimmed);
      if (exact) return { key: exact.key };
      const lower = trimmed.toLowerCase();
      const ci = groups.find(g => (g.label || '').toLowerCase() === lower || (g.key || '').toLowerCase() === lower);
      if (ci) return { key: ci.key };
      const direct = allExistingKeys.find(k => k.toLowerCase() === lower);
      if (direct) return { key: direct };
      const genKey = generateGroupKey(trimmed, allExistingKeys);
      return { key: genKey, newGroup: { key: genKey, label: trimmed } };
    }
    function openAddModal() {
      editingProject = null;
      document.getElementById('editOptions').hidden = true;
      document.getElementById('projectFormTitle').textContent = '新增项目';
      for (const key of ['Id', 'Name', 'Group', 'Root', 'Start', 'Stop', 'Url', 'Node', 'Java']) {
        const input = document.getElementById('add' + key);
        input.value = ''; input.disabled = false;
      }
      document.getElementById('editLinks').value = '';
      document.getElementById('editDefaultProfile').value = '';
      document.getElementById('editProfiles').value = '';
      document.getElementById('editDependsOn').value = '';
      document.getElementById('editPty').checked = false;
      document.getElementById('editPortMode').value = 'none';
      document.getElementById('editPortArg').value = '';
      document.getElementById('editPortEnv').value = '';
      document.getElementById('editEnv').value = '';
      document.getElementById('editAutoAnswers').value = '';
      document.getElementById('addPort').value = '8080';
      renderGroupDatalist();
      document.getElementById('addModal').style.display = 'flex';
    }
    async function editProject(id) {
      try {
        const res = await fetch(`/api/projects/${id}`);
        const data = await res.json();
        if (!res.ok || !data.ok) throw new Error(data.error || '读取失败');
        openAddModal();
        editingProject = data.project;
        document.getElementById('editOptions').hidden = false;
        for (const [key, field] of Object.entries({Health: 'health_url', LogDir: 'log_dir', Log: 'log_file', Timeout: 'start_timeout_secs', Grace: 'stop_grace_secs', Reason: 'reason'})) document.getElementById('edit' + key).value = editingProject[field] ?? '';
        document.getElementById('editEnabled').checked = editingProject.enabled;
        document.getElementById('editAutoInstall').checked = editingProject.auto_install;
        document.getElementById('editPty').checked = Boolean(editingProject.pty);
        document.getElementById('editPortMode').value = editingProject.port_mode || 'none';
        document.getElementById('editPortArg').value = editingProject.port_arg || '';
        document.getElementById('editPortEnv').value = editingProject.port_env || '';
        document.getElementById('editEnv').value = formatEnvLines(editingProject.env);
        document.getElementById('editAutoAnswers').value = formatAutoAnswersLines(editingProject.auto_answers);
        document.getElementById('editLinks').value = formatLinksLines(editingProject.links);
        document.getElementById('editDefaultProfile').value = editingProject.default_profile || '';
        document.getElementById('editProfiles').value = formatProfilesLines(editingProject.profiles);
        document.getElementById('editDependsOn').value = (editingProject.depends_on || []).join(', ');
        const fields = {Id: 'id', Name: 'name', Group: 'group', Root: 'root', Start: 'start', Stop: 'stop', Port: 'port', Url: 'login_url', Node: 'node', Java: 'java'};
        for (const [key, field] of Object.entries(fields)) document.getElementById('add' + key).value = editingProject[field] ?? '';
        if (editingProject.group) {
          document.getElementById('addGroup').value = getInitialGroupLabel(editingProject.group);
        }
        document.getElementById('addId').disabled = true;
        document.getElementById('addStart').disabled = !editingProject.start_editable;
        document.getElementById('addStop').disabled = !editingProject.stop_editable;
        document.getElementById('projectFormTitle').textContent = '项目设置';
      } catch (e) { alert('读取设置失败：' + e.message); }
    }
    function closeAddModal() {
      document.getElementById('addModal').style.display = 'none';
    }

    async function submitAddProject() {
      const id = document.getElementById('addId').value.trim();
      const name = document.getElementById('addName').value.trim();
      const rawGroup = document.getElementById('addGroup').value.trim();
      const root = document.getElementById('addRoot').value.trim();
      const start = document.getElementById('addStart').value.trim();
      const stop = document.getElementById('addStop').value.trim();
      const port = Number(document.getElementById('addPort').value);
      const url = document.getElementById('addUrl').value.trim();
      const node = document.getElementById('addNode').value.trim();
      const java = document.getElementById('addJava').value.trim();

      if (!id || !root || (!editingProject && !start)) {
        alert('请补全 ID、路径和启动命令！');
        return;
      }

      const minimumPort = editingProject && !document.getElementById('editEnabled').checked ? 0 : 1;
      if (!Number.isInteger(port) || port < minimumPort || port > 65535) { alert('启用项目的端口须为 1–65535 的整数'); return; }

      let links = [];
      let profiles = {};
      let env = {};
      let autoAnswers = [];
      let dependsOn = [];
      const linksText = document.getElementById('editLinks').value;
      const defaultProfile = document.getElementById('editDefaultProfile').value.trim();
      const profilesText = document.getElementById('editProfiles').value;
      const rawDeps = document.getElementById('editDependsOn').value.trim();
      if (rawDeps) {
        dependsOn = rawDeps.split(/[\n,]+/).map(s => s.trim()).filter(Boolean);
      }

      try {
        links = parseLinksLines(linksText);
        profiles = parseProfilesLines(profilesText);
        env = parseEnvLines(document.getElementById('editEnv').value);
        autoAnswers = parseAutoAnswersLines(document.getElementById('editAutoAnswers').value);
      } catch (err) {
        alert(err.message);
        return;
      }

      const pty = document.getElementById('editPty').checked;
      const portMode = document.getElementById('editPortMode').value;
      const portArg = document.getElementById('editPortArg').value.trim();
      const portEnv = document.getElementById('editPortEnv').value.trim();

      if (defaultProfile && Object.keys(profiles).length > 0 && !profiles[defaultProfile]) {
        alert(`默认环境 "${defaultProfile}" 不在配置的环境列表 profiles 中`);
        return;
      }

      const save = document.getElementById('projectSave');
      if (save.disabled) return;
      save.disabled = true;
      try {
        const resolved = resolveFormGroup(rawGroup, lastGroups || []);
        if (resolved.newGroup) {
          try {
            await postJson('/api/groups', {
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ key: resolved.newGroup.key, label: resolved.newGroup.label })
            });
            if (!lastGroups.some(g => g.key === resolved.newGroup.key)) {
              lastGroups.push({ key: resolved.newGroup.key, label: resolved.newGroup.label });
              renderGroupDatalist();
            }
          } catch (ge) {
            console.warn('创建分组失败或已存在：', ge);
          }
        }
        const group = resolved.key || '';

        const payload = {
          id, name, group: group || null, root, start,
          stop: stop || null,
          port,
          login_url: url || null,
          node: node || null,
          java: java || null
        };

        if (editingProject) {
          delete payload.id;
          payload.group = group;
          payload.enabled = document.getElementById('editEnabled').checked;
          payload.auto_install = document.getElementById('editAutoInstall').checked;
          payload.pty = pty;
          payload.port_mode = portMode;
          payload.port_arg = portMode === 'arg' ? (portArg || '--port') : null;
          payload.port_env = portMode === 'env' ? (portEnv || 'PORT') : null;
          payload.env = env;
          payload.auto_answers = autoAnswers;
          payload.reason = document.getElementById('editReason').value.trim();
          payload.health_url = document.getElementById('editHealth').value.trim();
          payload.log_dir = document.getElementById('editLogDir').value.trim();
          payload.log_file = document.getElementById('editLog').value.trim();
          for (const [key, field] of Object.entries({Timeout: 'start_timeout_secs', Grace: 'stop_grace_secs'})) {
            const value = document.getElementById('edit' + key).value.trim();
            if (value) {
              const n = Number(value);
              if (!Number.isSafeInteger(n) || n < 1) throw new Error('超时和宽限须为正整数');
              payload[field] = n;
            }
          }
          payload.stop = stop;
          payload.login_url = url;
          payload.node = node;
          payload.java = java;
          payload.links = links;
          payload.default_profile = defaultProfile || null;
          payload.profiles = profiles;
          payload.depends_on = dependsOn;
          if (!editingProject.start_editable) delete payload.start;
          if (!editingProject.stop_editable) delete payload.stop;
        } else {
          if (pty) payload.pty = true;
          if (portMode && portMode !== 'none') payload.port_mode = portMode;
          if (portArg) payload.port_arg = portArg;
          if (portEnv) payload.port_env = portEnv;
          if (Object.keys(env).length) payload.env = env;
          if (autoAnswers.length) payload.auto_answers = autoAnswers;
          if (links.length) payload.links = links;
          if (defaultProfile) payload.default_profile = defaultProfile;
          if (Object.keys(profiles).length) payload.profiles = profiles;
          if (dependsOn.length) payload.depends_on = dependsOn;
        }
        await postJson(editingProject ? `/api/projects/${editingProject.id}` : '/api/projects', {
          method: editingProject ? 'PATCH' : 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        });
        closeAddModal();
        fetchProjects();
      } catch (e) {
        alert('保存失败：' + e.message);
      } finally { save.disabled = false; }
    }

    async function openLogModal(id, name) {
      currentLogId = id;
      document.getElementById('logTitle').innerText = `${name} (${id}) · 实时日志`;
      document.getElementById('logModal').style.display = 'flex';
      refreshCurrentLogs();
    }

    function closeLogModal() {
      document.getElementById('logModal').style.display = 'none';
      currentLogId = null;
    }

    async function refreshCurrentLogs() {
      if (!currentLogId) return;
      const box = document.getElementById('logContent');
      try {
        const res = await fetch(`/api/projects/${currentLogId}/logs`);
        const data = await res.json();
        if (data.ok && data.missing) {
          box.innerText = '日志文件还没生成：' + data.path;
        } else if (data.ok && data.lines) {
          box.innerHTML = data.lines.length
            ? data.lines.map(l => `<div class="ll ${/(ERROR|Exception|Caused by|FATAL)/.test(l) ? 'err' : /WARN/.test(l) ? 'warn' : ''}">${escapeHtml(l) || ' '}</div>`).join('')
            : '（日志文件暂无新内容）';
          box.scrollTop = box.scrollHeight;
        } else {
          box.innerText = '日志暂不可读: ' + (data.error || '未知错误');
        }
      } catch (e) {
        box.innerText = '加载日志失败: ' + e;
      }
    }

    function escapeHtml(str) {
      if (!str) return '';
      return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
    }

    // 搜索框：/ 或 Ctrl/⌘+K 聚焦，Esc 清空
    (function initSearch() {
      const input = document.getElementById('searchInput');
      input.addEventListener('input', () => { query = input.value; syncSearchUi(); renderProjects(lastProjects); });
      input.addEventListener('keydown', e => {
        if (e.key === 'Escape') {
          closeAllSplitMenus();
          e.preventDefault();
          if (query) { input.value = ''; query = ''; syncSearchUi(); renderProjects(lastProjects); } else { input.blur(); }
        }
      });
      document.getElementById('searchClear').addEventListener('click', e => { e.preventDefault(); input.value = ''; query = ''; syncSearchUi(); renderProjects(lastProjects); input.focus(); });
      document.addEventListener('click', closeAllSplitMenus);
      document.addEventListener('keydown', e => {
        if (e.key === 'Escape') { closeAllSplitMenus(); }
        const t = e.target;
        const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
        const modalOpen = [...document.querySelectorAll('.modal-backdrop')].some(m => m.style.display === 'flex');
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); input.focus(); input.select(); }
        else if (e.key === '/' && !typing && !modalOpen) { e.preventDefault(); input.focus(); }
      });
    })();

    // 初始加载与轮询刷新
    fetchProjects();
    setInterval(fetchProjects, 1500);
  </script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_html_contains_starting_and_port_open_branches() {
        assert!(UI_HTML.contains("starting"));
        assert!(UI_HTML.contains("port_open"));
        assert!(UI_HTML.contains("端口已通（等登录页）"));
        assert!(UI_HTML.contains("启动中"));
        assert!(UI_HTML.contains("就绪"));
        assert!(UI_HTML.contains("已停止"));
    }

    #[test]
    fn home_html_contains_profiles_links_and_terminal_controls() {
        assert!(UI_HTML.contains("tag-profile"));
        assert!(UI_HTML.contains("profile-select-bar"));
        assert!(UI_HTML.contains("split-menu"));
        assert!(UI_HTML.contains("openTerminal"));
        assert!(UI_HTML.contains("editLinks"));
        assert!(UI_HTML.contains("editProfiles"));
        assert!(UI_HTML.contains("editPty"));
        assert!(UI_HTML.contains("editPortMode"));
        assert!(UI_HTML.contains("editEnv"));
        assert!(UI_HTML.contains("editAutoAnswers"));
        assert!(UI_HTML.contains("default_profile"));
        assert!(UI_HTML.contains("Profile 候选"));
    }

    #[test]
    fn home_html_has_search_filters_and_empty_states() {
        assert!(UI_HTML.contains("id=\"searchInput\""));
        assert!(UI_HTML.contains("未配置"));
        assert!(UI_HTML.contains("没有匹配的项目"));
        assert!(UI_HTML.contains("--ty-matcha"));
        assert!(!UI_HTML.contains("#0b0f19"));
        assert!(UI_HTML.contains("startGroup"));
        assert!(UI_HTML.contains("stopGroup"));
        assert!(UI_HTML.contains("stopAllProjects"));
        assert!(UI_HTML.contains("stopAllBtn"));
        assert!(UI_HTML.contains("group-actions"));
        assert!(UI_HTML.contains("group-card"));
        assert!(UI_HTML.contains("全部启动"));
        assert!(UI_HTML.contains("openInCursor"));
        assert!(UI_HTML.contains("openLogFile"));
        assert!(UI_HTML.contains("copyStartCommand"));
    }

    #[test]
    fn tests_group_route_and_url_decode() {
        assert_eq!(group_route("/api/groups/demo/start", "start"), Some("demo"));
        assert_eq!(group_route("/api/groups/demo/stop", "stop"), Some("demo"));
        assert_eq!(
            group_route("/api/groups/demo/restart", "restart"),
            Some("demo")
        );
        assert_eq!(group_route("/api/groups/demo/restart", "start"), None);
        assert_eq!(group_route("/api/groups/demo/start", "stop"), None);
        assert_eq!(group_route("/api/groups/demo", ""), Some("demo"));
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("%E5%90%8E%E7%AB%AF"), "后端");
    }
}

#[cfg(test)]
mod tests_pure_helpers {
    use super::*;

    #[test]
    fn test_find_bytes() {
        assert_eq!(find(b"hello world", b"world"), Some(6));
        assert_eq!(find(b"abc", b"def"), None);
        assert_eq!(find(b"abc", b"abcd"), None);
        assert_eq!(find(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
    }

    #[test]
    fn test_trusted_logic() {
        let req_get = Request {
            method: "GET".to_string(),
            path: "/api/projects".to_string(),
            host: "127.0.0.1:9099".to_string(),
            origin: None,
            body: Vec::new(),
        };
        assert!(trusted(&req_get, 9099));

        let req_get_localhost = Request {
            method: "GET".to_string(),
            path: "/api/projects".to_string(),
            host: "localhost:9099".to_string(),
            origin: None,
            body: Vec::new(),
        };
        assert!(trusted(&req_get_localhost, 9099));

        let req_get_bad_host = Request {
            method: "GET".to_string(),
            path: "/api/projects".to_string(),
            host: "example.com:9099".to_string(),
            origin: None,
            body: Vec::new(),
        };
        assert!(!trusted(&req_get_bad_host, 9099));

        let req_post_no_origin = Request {
            method: "POST".to_string(),
            path: "/api/projects/demo/start".to_string(),
            host: "127.0.0.1:9099".to_string(),
            origin: None,
            body: Vec::new(),
        };
        assert!(trusted(&req_post_no_origin, 9099));

        let req_post_good_origin = Request {
            method: "POST".to_string(),
            path: "/api/projects/demo/start".to_string(),
            host: "127.0.0.1:9099".to_string(),
            origin: Some("http://127.0.0.1:9099".to_string()),
            body: Vec::new(),
        };
        assert!(trusted(&req_post_good_origin, 9099));

        let req_post_bad_origin = Request {
            method: "POST".to_string(),
            path: "/api/projects/demo/start".to_string(),
            host: "127.0.0.1:9099".to_string(),
            origin: Some("http://attacker.example.com".to_string()),
            body: Vec::new(),
        };
        assert!(!trusted(&req_post_bad_origin, 9099));
    }

    #[test]
    fn test_project_route_matching() {
        assert_eq!(
            project_route("/api/projects/demo_web", ""),
            Some("demo_web")
        );
        assert_eq!(
            project_route("/api/projects/demo_web/start", "start"),
            Some("demo_web")
        );
        assert_eq!(
            project_route("/api/projects/demo_web/stop", "stop"),
            Some("demo_web")
        );
        assert_eq!(
            project_route("/api/projects/demo_web/logs", "logs"),
            Some("demo_web")
        );
        assert_eq!(
            project_route("/api/projects/demo_web/input", "input"),
            Some("demo_web")
        );

        assert_eq!(project_route("/api/projects/demo_web/start", "stop"), None);
        assert_eq!(project_route("/api/projects//start", "start"), None);
        assert_eq!(
            project_route("/api/projects/nested/sub/start", "start"),
            None
        );
        assert_eq!(project_route("/api/projects/", ""), None);
        assert_eq!(project_route("/api/other/demo_web", ""), None);
    }

    #[test]
    fn test_url_decode_edge_cases() {
        assert_eq!(url_decode("a+b%20c"), "a b c");
        assert_eq!(url_decode("no_escapes"), "no_escapes");
        assert_eq!(url_decode("invalid%zz"), "invalid%zz");
        assert_eq!(url_decode("tail%"), "tail%");
        assert_eq!(url_decode("tail%a"), "tail%a");
    }

    #[test]
    fn test_json_shaping_helpers() {
        let (code, ctype, body) = json_body(200, json!({"test_key": "test_val"}));
        assert_eq!(code, 200);
        assert_eq!(ctype, "application/json; charset=utf-8");
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["test_key"], "test_val");

        let (err_code, err_ctype, err_body) = err_json(404, "resource not found");
        assert_eq!(err_code, 404);
        assert_eq!(err_ctype, "application/json; charset=utf-8");
        let err_parsed: Value = serde_json::from_slice(&err_body).unwrap();
        assert_eq!(err_parsed["ok"], false);
        assert_eq!(err_parsed["error"], "resource not found");
    }

    #[test]
    fn test_reason_codes() {
        assert_eq!(reason(200), "OK");
        assert_eq!(reason(400), "Bad Request");
        assert_eq!(reason(403), "Forbidden");
        assert_eq!(reason(404), "Not Found");
        assert_eq!(reason(405), "Method Not Allowed");
        assert_eq!(reason(500), "Internal Server Error");
        assert_eq!(reason(502), "Internal Server Error");
    }
}
