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
use tongyun_dev_core::{Loaded, NewProject};

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
        let mut v = serde_json::to_value(loaded.status(&row.id)?)?;
        if let Value::Object(m) = &mut v {
            m.insert("enabled".into(), json!(row.enabled));
            m.insert("configured".into(), json!(row.configured));
            m.insert("reason".into(), json!(row.reason));
            m.insert("node".into(), json!(row.node));
            m.insert("java".into(), json!(row.java));
            m.insert("java_home".into(), json!(row.java_home));
            m.insert("node_home".into(), json!(row.node_home));
            m.insert("shell".into(), json!(row.shell));
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
            err_json(404, "not found")
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
            let new: NewProject = match serde_json::from_slice(&req.body) {
                Ok(n) => n,
                Err(e) => return err_json(400, format!("bad payload: {e}")),
            };
            let _guard = state.write_lock.lock().unwrap_or_else(|p| p.into_inner());
            // Reload under the lock so concurrent adds see each other.
            let mut loaded = match load(state) {
                Ok(l) => l,
                Err(e) => return err_json(500, format!("{e:#}")),
            };
            match loaded.add_project(new) {
                Ok(row) => json_body(200, json!({"ok": true, "project": row})),
                Err(e) => err_json(400, format!("{e:#}")),
            }
        }
        "POST" => {
            let action = ["start", "stop", "install", "open", "open-dir"]
                .into_iter()
                .find_map(|a| project_route(path, a).map(|id| (a, id)));
            let Some((action, id)) = action else {
                return err_json(404, "not found");
            };
            let res: Result<Value> = match action {
                "start" => loaded.start_detached(id).map(|st| json!({"status": st})),
                "stop" => loaded.stop(id).map(|st| json!({"status": st})),
                "install" => loaded.install_deps(id).map(|st| json!({"status": st})),
                "open" => loaded
                    .page_url(id)
                    .and_then(|u| tongyun_dev_core::open_url(&u).map(|_| json!({"url": u}))),
                _ => loaded
                    .root_dir(id)
                    .and_then(|p| tongyun_dev_core::open_path(&p).map(|_| json!({"path": p}))),
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
    .summary { margin-left: auto; color: var(--ty-text-muted); font-size: var(--ty-font-size-sm); }

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
    .scan-fields input { width: 100%; box-sizing: border-box; }
    .scan-why { margin-top: 6px; font-size: 12px; color: var(--ty-text-muted); }
    .scan-why ul { margin: 4px 0 0 18px; padding: 0; }
    .chip.conf-high { background: var(--ty-matcha-tint); color: var(--ty-matcha); }
    .chip.conf-medium { background: var(--ty-sky-tint); color: var(--ty-sky); }
    .chip.conf-low { background: var(--ty-honey-tint); color: var(--ty-honey); }
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
      <span class="summary" id="summary"></span>
    </div>

    <div class="grid" id="projectGrid">
      <div class="empty">正在扫描本地项目…</div>
    </div>
  </div>

  <!-- Add Project Modal -->
  <div class="modal-backdrop" id="addModal">
    <div class="modal">
      <div class="modal-head">
        <h3>新增项目</h3>
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
      <div class="modal-actions">
        <button class="btn-secondary" onclick="closeAddModal()">取消</button>
        <button class="btn-primary" onclick="submitAddProject()">保存</button>
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
        return `<div class="scan-row ${done ? 'done' : ''}">
          <label class="scan-pick"><input type="checkbox" ${c.picked && !done ? 'checked' : ''} ${done ? 'disabled' : ''} onchange="scanField(${i}, 'picked', this.checked)" /></label>
          <div class="scan-main">
            <div class="scan-title"><strong>${escapeHtml(c.label)}</strong><span class="chip conf-${c.confidence}">${conf[c.confidence] || c.confidence}</span>${tools}${done ? `<span class="chip">已注册为 ${escapeHtml(c.existing_id)}</span>` : ''}${c.needs_tomcat ? '<span class="chip">需要 Tomcat</span>' : ''}</div>
            <div class="scan-root">${escapeHtml(c.root)}</div>
            <div class="scan-fields">
              <label>ID<input value="${escapeHtml(c.id)}" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'id', this.value)" /></label>
              <label>名称<input value="${escapeHtml(c.name)}" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'name', this.value)" /></label>
              <label class="wide">启动命令<input value="${escapeHtml(c.start)}" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'start', this.value)" /></label>
              <label class="narrow">端口<input type="number" value="${c.port || ''}" placeholder="?" ${done ? 'disabled' : ''} oninput="scanField(${i}, 'port', this.value)" /></label>
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
        alert('添加失败：' + e.message);
      }
    }
    let currentLogId = null;
    let lastProjects = [];
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

    async function runAction(id, label, path, failText) {
      busyIds[id] = label;
      renderProjects(lastProjects);
      try {
        await postJson(`/api/projects/${id}/${path}`);
      } catch (e) {
        alert(failText + '：' + e.message);
      } finally {
        delete busyIds[id];
        fetchProjects();
      }
    }

    async function fetchProjects() {
      try {
        const res = await fetch('/api/projects');
        const data = await res.json();
        if (!Array.isArray(data)) throw new Error(data.error || '加载失败');
        lastProjects = data;
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
      document.getElementById('summary').textContent = `${projects.length} 个项目 · ${running} 个运行中`;
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
      grid.innerHTML = visible.map(p => {
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

        return `
          <div class="card ${enabled ? '' : 'disabled'}">
            <div class="card-head">
              <div class="card-title">
                <h3>${highlight(p.name)}</h3>
                <code>${highlight(p.id)}</code>
              </div>
              <div class="badge ${badgeClass}" title="${stateText}">
                <span class="dot"></span>
                <span>${stateText}</span>
              </div>
            </div>
            ${!enabled ? `<div class="reason">停用原因：${escapeHtml(p.reason)}</div>` : ''}
            ${!configured ? `<div class="note">还没配置好：缺端口或启动命令。在桌面 App 里打开这个项目的「项目设置」填好就能启动。</div>` : ''}
            ${conflict}
            ${configured && enabled && state === 'failed' && p.failure ? `<div class="fail">${escapeHtml(p.failure)}</div>` : ''}
            <div class="meta-list">
              <div class="meta-item"><span class="meta-label">端口</span><span class="meta-val ${portCls}">${portText}</span></div>
              <div class="meta-item"><span class="meta-label">进程</span><span class="meta-val ${p.pid ? '' : 'val-down'}">${p.pid ? 'PID ' + p.pid : '未运行'}</span></div>
              <div class="meta-item"><span class="meta-label">登录页</span><span class="meta-val ${probeOk ? 'val-ok' : 'val-down'}">${probeText}</span></div>
            </div>
            <div class="root" title="${escapeHtml(p.root)}">${ICON_FOLDER}<span>${highlight(p.root)}</span></div>
            ${chips ? `<div class="chips">${chips}</div>` : ''}
            <div class="card-actions">
              ${actionBtns}
              <button class="btn-outline" ${canOpenPage ? `onclick="openProjectInBrowser('${p.id}')"` : 'disabled'} title="在系统浏览器打开登录/访问页面">网页</button>
              <button class="btn-outline" onclick="openProjectDir('${p.id}')" title="在系统文件管理器中打开源码目录">目录</button>
              <button class="btn-outline" onclick="openLogModal('${p.id}', '${escapeHtml(p.name)}')">日志</button>
              <button class="btn-danger-text" onclick="deleteProject('${p.id}')" title="从配置里删除这个项目" aria-label="删除"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg></button>
            </div>
          </div>
        `;
      }).join('');
    }

    async function openProjectDir(id) {
      try {
        await postJson(`/api/projects/${id}/open-dir`);
      } catch (e) {
        alert('打开目录失败：' + e.message);
      }
    }

    function startProject(id) {
      return runAction(id, '启动中…', 'start', '启动失败');
    }

    function installDeps(id) {
      return runAction(id, '安装依赖中…', 'install', '安装依赖失败');
    }

    function stopProject(id) {
      return runAction(id, '停止中…', 'stop', '停止失败');
    }

    async function openProjectInBrowser(id) {
      try {
        await postJson(`/api/projects/${id}/open`);
      } catch (e) {
        alert('打开页面失败：' + e.message);
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

    function openAddModal() {
      document.getElementById('addModal').style.display = 'flex';
    }
    function closeAddModal() {
      document.getElementById('addModal').style.display = 'none';
    }

    async function submitAddProject() {
      const id = document.getElementById('addId').value.trim();
      const name = document.getElementById('addName').value.trim();
      const root = document.getElementById('addRoot').value.trim();
      const start = document.getElementById('addStart').value.trim();
      const stop = document.getElementById('addStop').value.trim();
      const port = parseInt(document.getElementById('addPort').value) || 8080;
      const url = document.getElementById('addUrl').value.trim();
      const node = document.getElementById('addNode').value.trim();
      const java = document.getElementById('addJava').value.trim();

      if (!id || !root || !start) {
        alert('请补全 ID、路径和启动命令！');
        return;
      }

      const payload = {
        id, name, root, start,
        stop: stop || null,
        port,
        login_url: url || null,
        node: node || null,
        java: java || null
      };

      try {
        await postJson('/api/projects', {
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        });
        closeAddModal();
        fetchProjects();
      } catch (e) {
        alert('添加失败：' + e.message);
      }
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
        if (e.key === 'Escape') { e.preventDefault(); if (query) { input.value = ''; query = ''; syncSearchUi(); renderProjects(lastProjects); } else { input.blur(); } }
      });
      document.getElementById('searchClear').addEventListener('click', e => { e.preventDefault(); input.value = ''; query = ''; syncSearchUi(); renderProjects(lastProjects); input.focus(); });
      document.addEventListener('keydown', e => {
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
    fn home_html_has_search_filters_and_empty_states() {
        assert!(UI_HTML.contains("id=\"searchInput\""));
        assert!(UI_HTML.contains("未配置"));
        assert!(UI_HTML.contains("没有匹配的项目"));
        assert!(UI_HTML.contains("--ty-matcha"));
        assert!(!UI_HTML.contains("#0b0f19"));
    }
}
