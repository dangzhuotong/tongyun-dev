use std::path::Path;
use tongyun_dev_core::{LogTail, ProjectList, StatusJson};

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
async fn start_project(id: String) -> Result<StatusJson, String> {
    blocking(move || {
        tongyun_dev_core::load(None)
            .and_then(|loaded| loaded.start(&id))
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

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_projects,
            project_status,
            start_project,
            stop_project,
            log_tail,
            open_log,
            open_page
        ])
        .run(tauri::generate_context!())
        .expect("error while running tongyun-dev");
}
