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

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_projects,
            project_status,
            start_project,
            stop_project,
            log_tail,
            open_log,
            open_page,
            config_info,
            pick_config_file,
            create_default_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tongyun-dev");
}
