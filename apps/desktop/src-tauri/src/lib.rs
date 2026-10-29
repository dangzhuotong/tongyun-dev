use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Mutex;
use tongyun_dev_cli::server::{ServerHandle, DEFAULT_UI_PORT};
use tongyun_dev_core::{
    BatchOutput, ImportReport, LogTail, NewProject, ProjectDetail, ProjectEdit, ProjectList,
    ProjectRow, ScanReport, StatusJson,
};

static SERVER_HANDLE: Mutex<Option<ServerHandle>> = Mutex::new(None);
static AGENT_PORT: AtomicU16 = AtomicU16::new(0);
static AGENT_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AgentHttpInfo {
    pub running: bool,
    pub port: Option<u16>,
    pub url: Option<String>,
}

#[tauri::command]
fn agent_http_info() -> AgentHttpInfo {
    if AGENT_RUNNING.load(Ordering::Relaxed) {
        let port = AGENT_PORT.load(Ordering::Relaxed);
        AgentHttpInfo {
            running: true,
            port: Some(port),
            url: Some(format!("http://127.0.0.1:{port}")),
        }
    } else {
        AgentHttpInfo {
            running: false,
            port: None,
            url: None,
        }
    }
}

fn report(err: impl std::fmt::Display) -> String {
    format!("{err:#}")
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(err) => Err(err.to_string()),
    }
}

#[tauri::command]
async fn list_projects() -> Result<ProjectList, String> {
    blocking(|| {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.list())
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn project_status(id: String) -> Result<StatusJson, String> {
    blocking(move || {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.status(&id))
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn start_project(id: String, profile: Option<String>) -> Result<StatusJson, String> {
    blocking(move || {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.start_detached_with_profile(&id, profile.as_deref()))
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn stop_project(id: String) -> Result<StatusJson, String> {
    blocking(move || {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.stop(&id))
            .map_err(report)
    })
    .await
}

/// 后台装前端依赖（pnpm / yarn / npm 按锁文件选），立刻返回 installing 状态。
#[tauri::command]
async fn install_deps(id: String) -> Result<StatusJson, String> {
    blocking(move || {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.install_deps(&id))
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn restart_project(id: String, profile: Option<String>) -> Result<StatusJson, String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded
            .restart_detached_with_profile(&id, profile.as_deref())
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn start_batch(
    group: Option<String>,
    ids: Option<Vec<String>>,
) -> Result<BatchOutput, String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        if group.is_some() {
            loaded
                .start_batch(group.as_deref(), ids.as_deref(), None)
                .map_err(report)
        } else {
            loaded
                .start_batch_detached(group.as_deref(), ids.as_deref())
                .map_err(report)
        }
    })
    .await
}

#[tauri::command]
async fn stop_batch(
    group: Option<String>,
    ids: Option<Vec<String>>,
) -> Result<BatchOutput, String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded
            .stop_batch(group.as_deref(), ids.as_deref())
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn restart_batch(
    group: Option<String>,
    ids: Option<Vec<String>>,
) -> Result<BatchOutput, String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        if group.is_some() {
            loaded
                .restart_batch(group.as_deref(), ids.as_deref(), None)
                .map_err(report)
        } else {
            loaded
                .restart_batch_detached(group.as_deref(), ids.as_deref())
                .map_err(report)
        }
    })
    .await
}

#[tauri::command]
async fn open_terminal(id: String, profile: Option<String>) -> Result<(), String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded
            .open_in_terminal(&id, profile.as_deref())
            .map_err(report)
    })
    .await
}

#[tauri::command]
fn open_pages(urls: Vec<String>) -> Result<(), String> {
    for url in urls {
        tongyun_dev_core::open_url(&url).map_err(report)?;
    }
    Ok(())
}

/// Open the log file with the system default program (opener shared with the CLI / web console).
#[tauri::command]
fn open_log(path: String) -> Result<(), String> {
    let path = path.trim();
    if !Path::new(path).is_file() {
        return Err(format!("log file missing: {path}"));
    }
    tongyun_dev_core::open_path(Path::new(path)).map_err(report)
}

#[tauri::command]
fn open_in_cursor(path: String) -> Result<(), String> {
    let path = path.trim();
    if !Path::new(path).is_dir() {
        return Err(format!("folder missing: {path}"));
    }
    tongyun_dev_core::open_in_cursor(Path::new(path)).map_err(report)
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let path = path.trim();
    if !Path::new(path).is_dir() {
        return Err(format!("folder missing: {path}"));
    }
    tongyun_dev_core::open_path(Path::new(path)).map_err(report)
}

#[tauri::command]
fn open_page(url: String) -> Result<(), String> {
    tongyun_dev_core::open_url(&url).map_err(report)
}

#[tauri::command]
async fn log_tail(id: String, lines: Option<usize>) -> Result<LogTail, String> {
    blocking(move || {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.log_tail(&id, lines.unwrap_or(200)))
            .map_err(report)
    })
    .await
}

#[tauri::command]
async fn send_input(
    id: String,
    text: Option<String>,
    enter: Option<bool>,
    key: Option<String>,
) -> Result<(), String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        let data = tongyun_dev_core::resolve_input_data(
            text.as_deref(),
            enter.unwrap_or(true),
            key.as_deref(),
        )
        .map_err(report)?;
        loaded.send_input(&id, &data).map_err(report)
    })
    .await
}

#[tauri::command]
fn config_info() -> tongyun_dev_core::ConfigInfo {
    tongyun_dev_core::config_info()
}

#[tauri::command]
async fn pick_config_file() -> Result<Option<tongyun_dev_core::ConfigInfo>, String> {
    let picked = rfd::AsyncFileDialog::new()
        .add_filter("TOML config", &["toml"])
        .pick_file()
        .await;

    let Some(file_handle) = picked else {
        return Ok(None);
    };

    let path = file_handle.path();
    tongyun_dev_core::remember_config_path(path).map_err(report)?;
    Ok(Some(tongyun_dev_core::config_info()))
}

#[tauri::command]
async fn create_default_config() -> Result<tongyun_dev_core::ConfigInfo, String> {
    blocking(|| {
        tongyun_dev_core::create_default_config().map_err(report)?;
        Ok(tongyun_dev_core::config_info())
    })
    .await
}

#[tauri::command]
async fn all_status() -> Result<Vec<StatusJson>, String> {
    blocking(|| {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        let list = loaded.list().map_err(report)?;
        let mut results = Vec::new();
        for p in list.projects {
            if p.enabled && p.configured {
                if let Ok(st) = loaded.status(&p.id) {
                    results.push(st);
                }
            }
        }
        Ok(results)
    })
    .await
}

#[tauri::command]
async fn open_config_file() -> Result<(), String> {
    blocking(|| {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        tongyun_dev_core::open_path(loaded.path()).map_err(report)
    })
    .await
}

#[tauri::command]
async fn project_detail(id: String) -> Result<ProjectDetail, String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded.project_detail(&id).map_err(report)
    })
    .await
}

#[tauri::command]
async fn add_project(req: NewProject) -> Result<ProjectRow, String> {
    blocking(move || {
        let mut loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded.add_project(req).map_err(report)
    })
    .await
}

#[tauri::command]
async fn update_project(id: String, edit: ProjectEdit) -> Result<ProjectRow, String> {
    blocking(move || {
        let mut loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded.update_project(&id, edit).map_err(report)
    })
    .await
}

#[tauri::command]
async fn remove_project(id: String) -> Result<(), String> {
    blocking(move || {
        let mut loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded.remove_project(&id).map_err(report)
    })
    .await
}

#[tauri::command]
async fn scan_dir(dir: String, depth: Option<usize>) -> Result<ScanReport, String> {
    blocking(move || {
        let depth = depth.unwrap_or(tongyun_dev_core::DEFAULT_SCAN_DEPTH);
        match tongyun_dev_core::load(None) {
            Ok(loaded) => loaded.scan(&dir, depth).map_err(report),
            // No config yet: scan anyway, nothing can clash.
            Err(_) => tongyun_dev_core::scan_without_config(&dir, depth).map_err(report),
        }
    })
    .await
}

#[tauri::command]
async fn import_scanned(items: Vec<NewProject>) -> Result<ImportReport, String> {
    blocking(move || {
        let mut loaded = match tongyun_dev_core::load(None) {
            Ok(l) => l,
            Err(_) => {
                let path = tongyun_dev_core::create_default_config().map_err(report)?;
                tongyun_dev_core::load(Some(&path)).map_err(report)?
            }
        };
        loaded.import_scanned(items).map_err(report)
    })
    .await
}

#[tauri::command]
async fn pick_folder() -> Result<Option<String>, String> {
    let picked = rfd::AsyncFileDialog::new().pick_folder().await;
    Ok(picked.map(|h| h.path().to_string_lossy().to_string()))
}

#[tauri::command]
async fn create_group(
    key: String,
    label: Option<String>,
) -> Result<tongyun_dev_core::GroupInfo, String> {
    blocking(move || {
        let mut loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded.create_group(&key, label.as_deref()).map_err(report)
    })
    .await
}

#[tauri::command]
async fn set_project_group(id: String, group: Option<String>) -> Result<ProjectRow, String> {
    blocking(move || {
        let mut loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded
            .set_project_group(&id, group.as_deref())
            .map_err(report)
    })
    .await
}

/// Set when the user asks the window to close (close button / Cmd-Q path that
/// delivers `CloseRequested`). A launch-time window drop does not set this.
static USER_CLOSED_MAIN: AtomicBool = AtomicBool::new(false);

/// Bring `main` forward on the next event-loop turn.
///
/// On macOS, `setup` runs inside `applicationDidFinishLaunching`, after tao has
/// already called `activateIgnoringOtherApps` and before that callback returns.
/// The config window does not exist yet at that earlier call, and AppKit ignores
/// `makeKeyAndOrderFront` issued before `didFinishLaunching` returns, so the
/// window can show without the process becoming frontmost. There is no
/// single-instance plugin and no tray. Tauri then exits when the last window is
/// destroyed. Queue the show/activate until the run loop is actually running.
fn schedule_front(app: &tauri::AppHandle) {
    let app = app.clone();
    let _ = app
        .clone()
        .run_on_main_thread(move || bring_main_window_front(&app));
}

fn bring_main_window_front(app: &tauri::AppHandle) {
    use tauri::Manager;
    if app.get_webview_window("main").is_none() {
        let config = app
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "main")
            .cloned();
        if let Some(config) = config {
            if let Ok(builder) = tauri::WebviewWindowBuilder::from_config(app, &config) {
                let _ = builder.build();
            }
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    #[cfg(target_os = "macos")]
    {
        // `show` / `set_focus` are themselves queued. Activate only after them.
        let _ = app.run_on_main_thread(move || activate_macos_app());
    }
}

#[cfg(target_os = "macos")]
fn activate_macos_app() {
    let Some(mtm) = objc2::MainThreadMarker::new() else {
        return;
    };
    let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    ns_app.activateIgnoringOtherApps(true);
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // First launch after an update runs migration. This app has no
            // registered legacy keychain accounts, so the call does not open
            // the OS keychain. See `secret_store` module docs for the trigger.
            let _ = tongyun_dev_core::secret_store::migrate_app_secrets(
                &tongyun_dev_core::config_dir(None),
            );
            schedule_front(app.handle());

            // Start Agent HTTP server on 127.0.0.1:9099 (or next free port up to +20).
            match tongyun_dev_cli::server::start_background_server(None, DEFAULT_UI_PORT) {
                Ok(srv) => {
                    AGENT_PORT.store(srv.port(), Ordering::Relaxed);
                    AGENT_RUNNING.store(true, Ordering::Relaxed);
                    if let Ok(mut lock) = SERVER_HANDLE.lock() {
                        *lock = Some(srv);
                    }
                }
                Err(err) => {
                    AGENT_RUNNING.store(false, Ordering::Relaxed);
                    AGENT_PORT.store(0, Ordering::Relaxed);
                    eprintln!("Failed to start Agent HTTP server: {err:#}");
                }
            }

            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                USER_CLOSED_MAIN.store(true, Ordering::Relaxed);
            }
        })
        .invoke_handler(tauri::generate_handler![
            agent_http_info,
            list_projects,
            project_status,
            all_status,
            start_project,
            stop_project,
            install_deps,
            restart_project,
            log_tail,
            open_log,
            open_in_cursor,
            open_folder,
            open_page,
            config_info,
            pick_config_file,
            create_default_config,
            open_config_file,
            project_detail,
            add_project,
            update_project,
            remove_project,
            pick_folder,
            scan_dir,
            import_scanned,
            create_group,
            set_project_group,
            send_input,
            open_terminal,
            open_pages,
            start_batch,
            stop_batch,
            restart_batch
        ])
        .build(tauri::generate_context!())
        .expect("error while building tongyun-dev")
        .run(|app, event| match event {
            // Dock click on an already-running app. Show the same window again.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => schedule_front(app),
            // Last window destroyed with no tray. A user close is allowed to
            // exit. A launch that drops the window without CloseRequested is not.
            tauri::RunEvent::ExitRequested { api, code, .. } if code.is_none() => {
                if !USER_CLOSED_MAIN.load(Ordering::Relaxed) {
                    api.prevent_exit();
                    schedule_front(app);
                }
            }
            tauri::RunEvent::Exit => {
                AGENT_RUNNING.store(false, Ordering::Relaxed);
                if let Ok(mut lock) = SERVER_HANDLE.lock() {
                    if let Some(mut srv) = lock.take() {
                        srv.stop();
                    }
                }
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_http_info() {
        AGENT_RUNNING.store(false, Ordering::Relaxed);
        let stopped = agent_http_info();
        assert!(!stopped.running);
        assert_eq!(stopped.port, None);
        assert_eq!(stopped.url, None);

        AGENT_RUNNING.store(true, Ordering::Relaxed);
        AGENT_PORT.store(9099, Ordering::Relaxed);
        let running = agent_http_info();
        assert!(running.running);
        assert_eq!(running.port, Some(9099));
        assert_eq!(running.url, Some("http://127.0.0.1:9099".to_string()));

        // Restore stopped state
        AGENT_RUNNING.store(false, Ordering::Relaxed);
    }

    #[test]
    fn test_report_formatting() {
        assert_eq!(report("simple error message"), "simple error message");
        assert_eq!(report(12345), "12345");
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        assert_eq!(report(io_err), "file not found");
    }
}
