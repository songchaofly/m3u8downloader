#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use m3u8_core::{
    engine::{Diagnostics, PreviewInfo},
    AppError, Engine, Playlist, Result, Settings, TaskInput, TaskView,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{Emitter, Manager};

struct AppState {
    engine: Arc<Engine>,
    exiting: AtomicBool,
}
#[tauri::command]
fn list_tasks(state: tauri::State<AppState>) -> Vec<TaskView> {
    state.engine.list()
}
#[tauri::command]
fn get_settings(state: tauri::State<AppState>) -> Settings {
    state.engine.settings()
}
#[tauri::command]
fn save_settings(state: tauri::State<AppState>, settings: Settings) -> Result<()> {
    state.engine.save_settings(settings)
}
#[tauri::command]
async fn inspect_source(state: tauri::State<'_, AppState>, input: TaskInput) -> Result<Playlist> {
    state.engine.inspect(&input).await
}
#[tauri::command]
async fn add_task(
    state: tauri::State<'_, AppState>,
    input: TaskInput,
    start: bool,
) -> Result<TaskView> {
    let t = state.engine.add(input)?;
    if start {
        state.engine.start(&t.id)?;
    }
    Ok(t)
}
#[tauri::command]
fn task_input(state: tauri::State<AppState>, id: String) -> Result<Option<TaskInput>> {
    state.engine.input(&id)
}
#[tauri::command]
fn update_task(state: tauri::State<AppState>, id: String, input: TaskInput) -> Result<()> {
    let supplied = state.engine.input(&id)?.is_none_or(|old| {
        old.source != input.source
            || old.network.headers != input.network.headers
            || old.network.origin_headers != input.network.origin_headers
            || old.network.proxy != input.network.proxy
            || old.manual_keys != input.manual_keys
            || old.manual_ivs != input.manual_ivs
            || old.force_key != input.force_key
    });
    state.engine.update_options(&id, input, supplied)
}
#[tauri::command]
async fn task_action(state: tauri::State<'_, AppState>, id: String, action: String) -> Result<()> {
    match action.as_str() {
        "confirm_gaps" => {
            let mut input = state
                .engine
                .input(&id)?
                .ok_or_else(|| AppError::new("INPUT_INVALID", "任务配置不存在"))?;
            input.confirm_gaps = true;
            state.engine.update_options(&id, input, false)?;
            state.engine.start(&id)
        }
        "start" => state.engine.start(&id),
        "pause" => state.engine.stop(&id, false).await,
        "cancel" => state.engine.stop(&id, true).await,
        "delete" => state.engine.delete(&id, false).await,
        "delete_cache" => state.engine.delete(&id, true).await,
        "clear_cache" => state.engine.clear_cache(&id),
        _ => Err(AppError::new("INPUT_INVALID", "未知操作")),
    }
}
#[tauri::command]
fn duplicate_sources(state: tauri::State<AppState>, sources: Vec<String>) -> Vec<String> {
    state.engine.duplicate_sources(&sources)
}
#[tauri::command]
async fn test_connection(
    state: tauri::State<'_, AppState>,
    input: TaskInput,
) -> Result<serde_json::Value> {
    state.engine.test_connection(&input).await
}
#[tauri::command]
fn task_details(
    state: tauri::State<AppState>,
    id: String,
    offset: usize,
    limit: usize,
) -> Result<serde_json::Value> {
    state.engine.task_details(&id, offset, limit)
}
#[tauri::command]
fn prune_history(state: tauri::State<AppState>, days: u64) -> Result<usize> {
    state.engine.prune_history(days)
}
#[tauri::command]
async fn diagnostics(state: tauri::State<'_, AppState>) -> Result<Diagnostics> {
    Ok(state.engine.diagnostics().await)
}
#[tauri::command]
fn read_logs(state: tauri::State<AppState>) -> Result<String> {
    state.engine.logs()
}
#[tauri::command]
fn export_logs(state: tauri::State<AppState>, path: String) -> Result<()> {
    let path = std::path::PathBuf::from(path);
    if path.exists() {
        return Err(AppError::new(
            "OUTPUT_CONFLICT",
            "导出目标已存在，请选择新文件名",
        ));
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?
        .write_all(state.engine.logs()?.as_bytes())?;
    Ok(())
}
#[tauri::command]
async fn open_preview(state: tauri::State<'_, AppState>, input: TaskInput) -> Result<PreviewInfo> {
    state.engine.preview(input).await
}
#[tauri::command]
async fn preview_resource(
    state: tauri::State<'_, AppState>,
    id: String,
    token: String,
) -> Result<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(
        state.engine.preview_resource(&id, &token).await?,
    ))
}
#[tauri::command]
fn close_preview(state: tauri::State<AppState>, id: String) {
    state.engine.close_preview(&id)
}
#[tauri::command]
async fn prepare_local_media(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    path: String,
) -> Result<String> {
    let path = state
        .engine
        .local_preview(&id, std::path::Path::new(&path))
        .await?;
    allow_media(app, path)
}
#[tauri::command]
fn allow_media(app: tauri::AppHandle, path: String) -> Result<String> {
    let p = std::fs::canonicalize(path)?;
    let ext = p
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["mp4", "ts", "mkv", "vtt"].contains(&ext.as_str()) {
        return Err(AppError::new("HLS_UNSUPPORTED", "请选择媒体或 WebVTT 文件"));
    }
    app.asset_protocol_scope()
        .allow_file(&p)
        .map_err(|_| AppError::new("PERMISSION_DENIED", "无法授权本地媒体"))?;
    Ok(p.to_string_lossy().into_owned())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let root = std::env::var_os("M3U8_STUDIO_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_local_data_dir()?);
            let engine = Engine::open(&root)?;
            let mut events = engine.subscribe();
            let handle = app.handle().clone();
            app.manage(AppState {
                engine: engine.clone(),
                exiting: AtomicBool::new(false),
            });
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(view) => {
                            let _ = handle.emit("task-updated", &view);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_tasks,
            get_settings,
            save_settings,
            inspect_source,
            add_task,
            task_input,
            update_task,
            task_action,
            duplicate_sources,
            test_connection,
            task_details,
            prune_history,
            diagnostics,
            read_logs,
            export_logs,
            open_preview,
            preview_resource,
            close_preview,
            prepare_local_media,
            allow_media
        ])
        .build(tauri::generate_context!())
        .expect("M3U8 Studio failed to initialize");
    app.run(|handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let state = handle.state::<AppState>();
            if state.exiting.load(Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            state.exiting.store(true, Ordering::SeqCst);
            let engine = state.engine.clone();
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move {
                engine.shutdown().await;
                handle.exit(0);
            });
        }
    });
}
