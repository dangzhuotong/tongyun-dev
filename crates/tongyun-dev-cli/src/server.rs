use anyhow::{Context, Result};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::config::{expand_user, render, save_config, FileConfig, Project};
use crate::runner::{cmd_start, cmd_stop, open_url, read_last_lines, status_of};

struct AppState {
    config_path: PathBuf,
    config: FileConfig,
}

pub fn start_ui_server(config_path: PathBuf, config: FileConfig, port: u16, auto_open: bool) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).with_context(|| format!("bind UI server to {addr}"))?;
    let url = format!("http://{addr}");

    println!();
    println!("┌────────────────────────────────────────────────────────┐");
    println!("│  🚀 tongyun-dev 可视化控制台已就绪!                     │");
    println!("│  访问地址: {url:<43} │", url = url);
    println!("│  支持在界面中一键启停、查看实时状态、看日志、添加项目   │");
    println!("│  按 Ctrl+C 可停止控制台服务                            │");
    println!("└────────────────────────────────────────────────────────┘");
    println!();

    if auto_open {
        let _ = open_url(&url);
    }

    let state = Arc::new(Mutex::new(AppState {
        config_path,
        config,
    }));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state_clone = Arc::clone(&state);
                thread::spawn(move || {
                    let _ = handle_connection(stream, state_clone);
                });
            }
            Err(e) => {
                eprintln!("UI server connection error: {e}");
            }
        }
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<AppState>>) -> Result<()> {
    let mut buffer = [0u8; 8192];
    let bytes_read = stream.read(&mut buffer)?;
    if bytes_read == 0 {
        return Ok(());
    }

    let req_str = String::from_utf8_lossy(&buffer[..bytes_read]);
    let mut lines = req_str.lines();
    let first_line = lines.next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let path = parts[1];

    // Find body if present
    let body = if let Some(idx) = req_str.find("\r\n\r\n") {
        &req_str[idx + 4..]
    } else {
        ""
    };

    match (method, path) {
        ("GET", "/") => {
            send_response(&mut stream, 200, "text/html; charset=utf-8", UI_HTML.as_bytes())?;
        }
        ("GET", "/api/projects") => {
            let projects_json = {
                let st = state.lock().unwrap();
                let mut list = Vec::new();
                for (id, p) in &st.config.projects {
                    let status = status_of(id, p);
                    list.push(status);
                }
                list.sort_by(|a, b| a.id.cmp(&b.id));
                serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
            };
            send_response(&mut stream, 200, "application/json", projects_json.as_bytes())?;
        }
        ("POST", p) if p.starts_with("/api/projects/") && p.ends_with("/start") => {
            let id = p
                .strip_prefix("/api/projects/")
                .and_then(|s| s.strip_suffix("/start"))
                .unwrap_or("");
            let proj_opt = {
                let st = state.lock().unwrap();
                st.config.projects.get(id).cloned()
            };
            match proj_opt {
                Some(proj) => {
                    let res = cmd_start(id, &proj);
                    let st = status_of(id, &proj);
                    let body = json!({
                        "ok": res.is_ok(),
                        "error": res.err().map(|e| e.to_string()),
                        "status": st
                    });
                    send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
                }
                None => {
                    send_response(&mut stream, 404, "application/json", b"{\"error\":\"not found\"}")?;
                }
            }
        }
        ("POST", p) if p.starts_with("/api/projects/") && p.ends_with("/stop") => {
            let id = p
                .strip_prefix("/api/projects/")
                .and_then(|s| s.strip_suffix("/stop"))
                .unwrap_or("");
            let proj_opt = {
                let st = state.lock().unwrap();
                st.config.projects.get(id).cloned()
            };
            match proj_opt {
                Some(proj) => {
                    let res = cmd_stop(id, &proj);
                    let st = status_of(id, &proj);
                    let body = json!({
                        "ok": res.is_ok(),
                        "error": res.err().map(|e| e.to_string()),
                        "status": st
                    });
                    send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
                }
                None => {
                    send_response(&mut stream, 404, "application/json", b"{\"error\":\"not found\"}")?;
                }
            }
        }
        ("POST", p) if p.starts_with("/api/projects/") && p.ends_with("/open") => {
            let id = p
                .strip_prefix("/api/projects/")
                .and_then(|s| s.strip_suffix("/open"))
                .unwrap_or("");
            let proj_opt = {
                let st = state.lock().unwrap();
                st.config.projects.get(id).cloned()
            };
            match proj_opt {
                Some(proj) => {
                    let target_url = if !proj.login_url.is_empty() {
                        &proj.login_url
                    } else {
                        &format!("http://127.0.0.1:{}", proj.port)
                    };
                    let res = open_url(target_url);
                    let body = json!({
                        "ok": res.is_ok(),
                        "url": target_url,
                        "error": res.err().map(|e| e.to_string())
                    });
                    send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
                }
                None => {
                    send_response(&mut stream, 404, "application/json", b"{\"error\":\"not found\"}")?;
                }
            }
        }
        ("GET", p) if p.starts_with("/api/projects/") && p.contains("/logs") => {
            let id = p
                .strip_prefix("/api/projects/")
                .and_then(|s| s.split('/').next())
                .unwrap_or("");
            let proj_opt = {
                let st = state.lock().unwrap();
                st.config.projects.get(id).cloned()
            };
            match proj_opt {
                Some(proj) => {
                    let log_path = expand_user(&render(&proj.log_file, &proj));
                    match read_last_lines(&log_path, 120) {
                        Ok(lines) => {
                            let body = json!({ "ok": true, "path": log_path.display().to_string(), "lines": lines });
                            send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
                        }
                        Err(e) => {
                            let body = json!({ "ok": false, "path": log_path.display().to_string(), "error": e.to_string() });
                            send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
                        }
                    }
                }
                None => {
                    send_response(&mut stream, 404, "application/json", b"{\"error\":\"not found\"}")?;
                }
            }
        }
        ("POST", "/api/projects") => {
            #[derive(serde::Deserialize)]
            struct AddReq {
                id: String,
                name: String,
                root: String,
                start: String,
                stop: Option<String>,
                port: Option<u16>,
                login_url: Option<String>,
                log_file: Option<String>,
            }
            if let Ok(req) = serde_json::from_str::<AddReq>(body) {
                let mut st = state.lock().unwrap();
                let port = req.port.unwrap_or(8080);
                let login_url = req.login_url.unwrap_or_else(|| format!("http://127.0.0.1:{port}"));
                let log_file = req.log_file.unwrap_or_default();
                let start_cmd: Vec<String> = if cfg!(windows) {
                    vec!["cmd".into(), "/c".into(), req.start]
                } else {
                    vec!["bash".into(), "-lc".into(), req.start]
                };
                let stop_cmd: Vec<String> = req.stop.map(|s| {
                    if cfg!(windows) {
                        vec!["cmd".into(), "/c".into(), s]
                    } else {
                        vec!["bash".into(), "-lc".into(), s]
                    }
                }).unwrap_or_default();

                let project = Project {
                    name: req.name,
                    root: req.root,
                    start: start_cmd,
                    stop: stop_cmd,
                    tomcat: String::new(),
                    log_dir: String::new(),
                    log_file,
                    pid_file: String::new(),
                    port,
                    login_url,
                };
                st.config.projects.insert(req.id, project);
                let _ = save_config(&st.config_path, &st.config);
                let body = json!({ "ok": true });
                send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
            } else {
                send_response(&mut stream, 400, "application/json", b"{\"error\":\"bad payload\"}")?;
            }
        }
        ("DELETE", p) if p.starts_with("/api/projects/") => {
            let id = p.strip_prefix("/api/projects/").unwrap_or("");
            let mut st = state.lock().unwrap();
            if st.config.projects.remove(id).is_some() {
                let _ = save_config(&st.config_path, &st.config);
                let body = json!({ "ok": true });
                send_response(&mut stream, 200, "application/json", body.to_string().as_bytes())?;
            } else {
                send_response(&mut stream, 404, "application/json", b"{\"error\":\"not found\"}")?;
            }
        }
        _ => {
            send_response(&mut stream, 404, "text/plain", b"Not Found")?;
        }
    }

    Ok(())
}

fn send_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
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
        <label>显示名称</label>
        <input id="addName" placeholder="例如：前端开发服务" required />
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
        <input id="addStop" placeholder="例如：kill -9 {{pid}}" />
      </div>
      <div class="form-group">
        <label>端口号</label>
        <input id="addPort" type="number" value="3000" />
      </div>
      <div class="form-group">
        <label>访问/登录页面 URL</label>
        <input id="addUrl" placeholder="例如：http://127.0.0.1:3000" />
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

    async function fetchProjects() {
      try {
        const res = await fetch('/api/projects');
        const data = await res.json();
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
        const portOpen = p.port_open;
        const probeOk = p.login_probe.ok;
        const code = p.login_probe.http_code;

        return `
          <div class="card">
            <div class="card-head">
              <div class="card-title">
                <h3>${escapeHtml(p.name)}</h3>
                <code>${escapeHtml(p.id)}</code>
              </div>
              <div class="badge ${isRunning ? 'badge-running' : 'badge-stopped'}">
                <span class="dot"></span>
                <span>${isRunning ? '运行中' : '未启动'}</span>
              </div>
            </div>

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
                <span class="meta-val ${probeOk ? 'val-ok' : 'val-down'}">${code ? code + ' OK' : (isRunning ? '未就绪' : '-')}</span>
              </div>
              <div class="meta-item" style="font-size:11px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;" title="${escapeHtml(p.root)}">
                <span class="meta-label">根路径</span>
                <span class="meta-val">${escapeHtml(p.root)}</span>
              </div>
            </div>

            <div class="card-actions">
              ${isRunning 
                ? `<button class="btn-stop" onclick="stopProject('${p.id}')">⏹ 停止</button>`
                : `<button class="btn-start" onclick="startProject('${p.id}')">▶ 启动</button>`
              }
              <button class="btn-outline" onclick="openProjectInBrowser('${p.id}')" title="在系统浏览器打开登录/访问页面">🌐 打开页面</button>
              <button class="btn-outline" onclick="openLogModal('${p.id}', '${escapeHtml(p.name)}')">📜 日志</button>
              <button class="btn-danger-text" onclick="deleteProject('${p.id}')" title="删除项目注册">🗑</button>
            </div>
          </div>
        `;
      }).join('');
    }

    async function startProject(id) {
      try {
        const res = await fetch(`/api/projects/${id}/start`, { method: 'POST' });
        fetchProjects();
      } catch (e) {
        alert('启动失败: ' + e);
      }
    }

    async function stopProject(id) {
      try {
        const res = await fetch(`/api/projects/${id}/stop`, { method: 'POST' });
        fetchProjects();
      } catch (e) {
        alert('停止失败: ' + e);
      }
    }

    async function openProjectInBrowser(id) {
      try {
        await fetch(`/api/projects/${id}/open`, { method: 'POST' });
      } catch (e) {
        alert('打开浏览器失败: ' + e);
      }
    }

    async function deleteProject(id) {
      if (!confirm(`确定要移除项目 ${id} 的注册吗？（不会删除本地源码）`)) return;
      try {
        await fetch(`/api/projects/${id}`, { method: 'DELETE' });
        fetchProjects();
      } catch (e) {
        alert('删除失败: ' + e);
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

      if (!id || !name || !root || !start) {
        alert('请补全 ID、名称、路径和启动命令！');
        return;
      }

      const payload = {
        id, name, root, start,
        stop: stop || null,
        port,
        login_url: url || null
      };

      try {
        const res = await fetch('/api/projects', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        });
        if (res.ok) {
          closeAddModal();
          fetchProjects();
        } else {
          alert('添加失败');
        }
      } catch (e) {
        alert('网络错误: ' + e);
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
        if (data.ok && data.lines) {
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
      return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
    }

    // 初始加载与轮询刷新
    fetchProjects();
    setInterval(fetchProjects, 3000);
  </script>
</body>
</html>
"#;
