use std::path::Path;
use std::process::{Command, Stdio};
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

fn spawn_open(program: &str, arg: &str) -> Result<(), String> {
    Command::new(program)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("cannot open with {program}: {err}"))
}

fn open_target(target: &str) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        spawn_open("open", target)
    } else if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", "", target])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|err| format!("cannot open: {err}"))
    } else {
        spawn_open("xdg-open", target)
    }
}

#[tauri::command]
fn open_log(path: String) -> Result<(), String> {
    let path = path.trim();
    let file = Path::new(path);
    if !file.is_absolute() {
        return Err("log path must be absolute".into());
    }
    if !file.is_file() {
        return Err(format!("log file missing: {path}"));
    }
    open_target(path)
}

#[tauri::command]
fn open_page(url: String) -> Result<(), String> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("only http(s) URLs can be opened".into());
    }
    if url
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'')
    {
        return Err("url is not usable".into());
    }
    open_target(url)
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
