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
            let action = ["start", "stop", "open", "open-dir"]
                .into_iter()
                .find_map(|a| project_route(path, a).map(|id| (a, id)));
            let Some((action, id)) = action else {
                return err_json(404, "not found");
            };
            let res: Result<Value> = match action {
                "start" => loaded.start(id).map(|st| json!({"status": st})),
                "stop" => loaded.stop(id).map(|st| json!({"status": st})),
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

const UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>tongyun-dev · 本地项目控制台</title>
  <style>
    :root {
      --bg: #0b0f19;
      --card-bg: #141c2e;
      --card-border: #23314d;
      --text-main: #f1f5f9;
      --text-muted: #94a3b8;
      --primary: #38bdf8;
      --primary-hover: #0ea5e9;
      --success: #10b981;
      --success-bg: rgba(16, 185, 129, 0.15);
      --danger: #ef4444;
      --danger-bg: rgba(239, 68, 68, 0.15);
      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      background: var(--bg);
      color: var(--text-main);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      padding: 24px;
    }
    .container { max-width: 1080px; margin: 0 auto; }
    header {
      display: flex;
      justify-content: space-between;
      align-items: center;
      padding-bottom: 24px;
      border-bottom: 1px solid var(--card-border);
      margin-bottom: 28px;
    }
    .brand { display: flex; align-items: center; gap: 12px; }
    .brand h1 { font-size: 22px; font-weight: 700; color: #fff; }
    .brand span { font-size: 13px; color: var(--text-muted); background: #1e293b; padding: 3px 8px; border-radius: 6px; }
    .header-actions { display: flex; gap: 10px; }
    button {
      cursor: pointer;
      border: none;
      border-radius: 6px;
      font-size: 13px;
      font-weight: 500;
      padding: 8px 14px;
      transition: all 0.2s;
    }
    .btn-primary { background: var(--primary); color: #0b0f19; font-weight: 600; }
    .btn-primary:hover { background: var(--primary-hover); }
    .btn-secondary { background: #23314d; color: #cbd5e1; }
    .btn-secondary:hover { background: #33446b; }
    .btn-start { background: #059669; color: #fff; }
    .btn-start:hover { background: #10b981; }
    .btn-stop { background: #dc2626; color: #fff; }
    .btn-stop:hover { background: #ef4444; }
    .btn-outline { background: transparent; border: 1px solid var(--card-border); color: #cbd5e1; }
    .btn-outline:hover { background: #1e293b; }
    .btn-danger-text { background: transparent; color: #f87171; }
    .btn-danger-text:hover { color: #ef4444; }

    .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(330px, 1fr)); gap: 18px; }
    .card {
      background: var(--card-bg);
      border: 1px solid var(--card-border);
      border-radius: 10px;
      padding: 20px;
      display: flex;
      flex-direction: column;
      gap: 16px;
      box-shadow: 0 4px 12px rgba(0,0,0,0.25);
    }
    .card-head { display: flex; justify-content: space-between; align-items: flex-start; }
    .card-title h3 { font-size: 17px; font-weight: 600; margin-bottom: 4px; }
    .card-title code { font-size: 12px; color: var(--text-muted); font-family: var(--font-mono); }
    .badge {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      font-size: 12px;
      font-weight: 500;
      padding: 4px 9px;
      border-radius: 12px;
    }
    .badge-running { background: var(--success-bg); color: #34d399; }
    .badge-stopped { background: rgba(148, 163, 184, 0.1); color: #94a3b8; }
    .dot { width: 7px; height: 7px; border-radius: 50%; }
    .badge-running .dot { background: #10b981; box-shadow: 0 0 8px #10b981; }
    .badge-stopped .dot { background: #64748b; }

    .meta-list { display: flex; flex-direction: column; gap: 8px; font-size: 13px; color: #cbd5e1; }
    .meta-item { display: flex; justify-content: space-between; }
    .meta-label { color: var(--text-muted); }
    .meta-val { font-family: var(--font-mono); }
    .val-ok { color: #34d399; }
    .val-down { color: #94a3b8; }

    .card.disabled { opacity: 0.5; }
    .card.disabled .card-actions button.btn-start { cursor: not-allowed; }
    .reason { font-size: 12px; color: #fbbf24; }
    .warn { font-size: 12px; color: #fbbf24; background: rgba(251,191,36,0.12); padding: 6px 8px; border-radius: 6px; }
    .chips { display: flex; gap: 6px; flex-wrap: wrap; }
    .chip { font-size: 11px; font-family: var(--font-mono); background: #1e293b; color: #cbd5e1; padding: 2px 7px; border-radius: 10px; }
    button:disabled { opacity: 0.6; cursor: progress; }
    .card-actions {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      padding-top: 14px;
      border-top: 1px solid var(--card-border);
    }

    /* Modal */
    .modal-backdrop {
      display: none;
      position: fixed;
      inset: 0;
      background: rgba(0,0,0,0.7);
      backdrop-filter: blur(3px);
      align-items: center;
      justify-content: center;
      z-index: 100;
    }
    .modal {
      background: var(--card-bg);
      border: 1px solid var(--card-border);
      border-radius: 10px;
      width: 90%;
      max-width: 580px;
      padding: 24px;
    }
    .modal-head { display: flex; justify-content: space-between; margin-bottom: 18px; }
    .form-group { margin-bottom: 14px; }
    .form-group label { display: block; font-size: 12px; color: var(--text-muted); margin-bottom: 6px; }
    .form-group input {
      width: 100%;
      background: #0b0f19;
      border: 1px solid var(--card-border);
      color: #fff;
      padding: 8px 12px;
      border-radius: 6px;
      font-size: 13px;
    }
    .modal-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 20px; }
    .log-box {
      background: #090d16;
      border: 1px solid #1a2438;
      border-radius: 6px;
      padding: 12px;
      height: 380px;
      overflow-y: auto;
      font-family: var(--font-mono);
      font-size: 12px;
      line-height: 1.5;
      color: #94a3b8;
      white-space: pre-wrap;
    }
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="brand">
        <h1>⚡ tongyun-dev</h1>
        <span>本机项目控制台</span>
      </div>
      <div class="header-actions">
        <button class="btn-secondary" onclick="fetchProjects()">🔄 刷新</button>
        <button class="btn-primary" onclick="openAddModal()">＋ 注册项目</button>
      </div>
    </header>

    <div class="grid" id="projectGrid">
      <div style="color: #64748b; font-size: 14px;">正在扫描本地项目...</div>
    </div>
  </div>

  <!-- Add Project Modal -->
  <div class="modal-backdrop" id="addModal">
    <div class="modal">
      <div class="modal-head">
        <h3>注册新项目</h3>
        <button class="btn-outline" onclick="closeAddModal()">✕</button>
      </div>
      <div class="form-group">
        <label>项目 ID（英文，如 student_web）</label>
        <input id="addId" placeholder="例如：acme_fe" required />
      </div>
      <div class="form-group">
        <label>显示名称（可选，默认同 ID）</label>
        <input id="addName" placeholder="例如：前端开发服务" />
      </div>
      <div class="form-group">
        <label>项目根路径</label>
        <input id="addRoot" placeholder="例如：~/code/acme/student-web" required />
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
        <label>Node 版本（可选，走 fnm，如 22）</label>
        <input id="addNode" placeholder="例如：22" />
      </div>
      <div class="form-group">
        <label>Java 版本（可选，走 sdkman，如 17.0.10-tem）</label>
        <input id="addJava" placeholder="例如：17.0.10-tem" />
      </div>
      <div class="modal-actions">
        <button class="btn-secondary" onclick="closeAddModal()">取消</button>
        <button class="btn-primary" onclick="submitAddProject()">保存注册</button>
      </div>
    </div>
  </div>

  <!-- Log Modal -->
  <div class="modal-backdrop" id="logModal">
    <div class="modal" style="max-width: 800px;">
      <div class="modal-head">
        <h3 id="logTitle">项目日志</h3>
        <button class="btn-outline" onclick="closeLogModal()">✕</button>
      </div>
      <div class="log-box" id="logContent">加载中...</div>
      <div class="modal-actions">
        <button class="btn-secondary" onclick="refreshCurrentLogs()">刷新日志</button>
        <button class="btn-primary" onclick="closeLogModal()">关闭</button>
      </div>
    </div>
  </div>

  <script>
    let currentLogId = null;
    let lastProjects = [];
    const busyIds = {};

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

    function renderProjects(projects) {
      const grid = document.getElementById('projectGrid');
      if (!projects.length) {
        grid.innerHTML = '<div style="color:#64748b;grid-column:1/-1;">暂未注册任何项目，点击上方“＋ 注册项目”开始。</div>';
        return;
      }
      grid.innerHTML = projects.map(p => {
        const isRunning = p.running;
        const enabled = p.enabled !== false;
        const portOpen = p.port_open;
        const probeOk = p.login_probe.ok;
        const code = p.login_probe.http_code;
        const busy = busyIds[p.id];
        const chips = [p.node ? `node ${escapeHtml(p.node)}` : '', p.java ? `java ${escapeHtml(p.java)}` : '']
          .filter(Boolean).map(c => `<span class="chip">${c}</span>`).join('');
        const conflict = p.port_conflict
          ? `<div class="warn">⚠ 端口 ${p.port} 被占用${p.port_holder ? '：' + escapeHtml(p.port_holder) : '（非本项目进程）'}</div>`
          : '';
        let primary;
        if (!enabled) {
          primary = `<button class="btn-start" disabled title="${escapeHtml(p.reason)}">▶ 启动</button>`;
        } else if (busy) {
          primary = `<button class="btn-secondary" disabled>${busy}</button>`;
        } else if (isRunning) {
          primary = `<button class="btn-stop" onclick="stopProject('${p.id}')">⏹ 停止</button>`;
        } else {
          primary = `<button class="btn-start" onclick="startProject('${p.id}')">▶ 启动</button>`;
        }

        return `
          <div class="card ${enabled ? '' : 'disabled'}">
            <div class="card-head">
              <div class="card-title">
                <h3>${escapeHtml(p.name)}</h3>
                <code>${escapeHtml(p.id)}</code>
              </div>
              <div class="badge ${isRunning ? 'badge-running' : 'badge-stopped'}">
                <span class="dot"></span>
                <span>${!enabled ? '已停用' : (isRunning ? '运行中' : '未启动')}</span>
              </div>
            </div>
            ${!enabled ? `<div class="reason">停用原因：${escapeHtml(p.reason)}</div>` : ''}
            ${chips ? `<div class="chips">${chips}</div>` : ''}
            ${conflict}

            <div class="meta-list">
              <div class="meta-item">
                <span class="meta-label">端口</span>
                <span class="meta-val ${portOpen ? 'val-ok' : 'val-down'}">${p.port} (${portOpen ? '占用' : '空闲'})</span>
              </div>
              <div class="meta-item">
                <span class="meta-label">PID 进程</span>
                <span class="meta-val">${p.pid || '-'}</span>
              </div>
              <div class="meta-item">
                <span class="meta-label">探针响应</span>
                <span class="meta-val ${probeOk ? 'val-ok' : 'val-down'}">${code ? code + (probeOk ? ' OK' : '') : (isRunning ? '未就绪' : '-')}</span>
              </div>
              <div class="meta-item" style="font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeHtml(p.root)}">
                <span class="meta-label">根路径</span>
                <span class="meta-val">${escapeHtml(p.root)}</span>
              </div>
            </div>

            <div class="card-actions">
              ${primary}
              <button class="btn-outline" onclick="openProjectInBrowser('${p.id}')" title="在系统浏览器打开登录/访问页面">🌐 页面</button>
              <button class="btn-outline" onclick="openProjectDir('${p.id}')" title="在系统文件管理器中打开源码目录">📁 目录</button>
              <button class="btn-outline" onclick="openLogModal('${p.id}', '${escapeHtml(p.name)}')">📜 日志</button>
              <button class="btn-danger-text" onclick="deleteProject('${p.id}')" title="删除项目注册">🗑</button>
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
      if (!confirm(`确定要移除项目 ${id} 的注册吗？（不会删除本地源码）`)) return;
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
          box.innerText = data.lines.join('\n') || '（日志文件暂无新内容）';
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

    // 初始加载与轮询刷新
    fetchProjects();
    setInterval(fetchProjects, 3000);
  </script>
</body>
</html>
"#;
