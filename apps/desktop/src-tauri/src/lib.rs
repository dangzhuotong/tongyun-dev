use std::path::Path;
use tongyun_dev_core::{
    ImportReport, LogTail, NewProject, ProjectDetail, ProjectEdit, ProjectList, ProjectRow,
    ScanReport, StatusJson,
};

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
            .and_then(|loaded| loaded.start_detached(&id))
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
async fn restart_project(id: String) -> Result<StatusJson, String> {
    blocking(move || {
        let loaded = tongyun_dev_core::load(None).map_err(report)?;
        loaded.restart_detached(&id).map_err(report)
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
        loaded.set_project_group(&id, group.as_deref()).map_err(report)
    })
    .await
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_projects,
            project_status,
            all_status,
            start_project,
            stop_project,
            install_deps,
            restart_project,
            log_tail,
            open_log,
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
            set_project_group
        ])
        .run(tauri::generate_context!())
        .expect("error while running tongyun-dev");
}
