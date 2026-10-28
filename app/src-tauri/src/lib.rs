//! The Tauri shell: commands call the core queue engine, and engine events go to the window
//! as `engine` events. All the work lives in `upscale-core`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{Emitter, Manager, State};
use upscale_core::queue::{AppSettings, Engine, QueueSnapshot};
use upscale_core::tools::set::ToolStatus;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    core_version: &'static str,
    gpu: Option<String>,
}

#[tauri::command]
fn app_info(engine: State<Engine>) -> AppInfo {
    AppInfo {
        core_version: upscale_core::VERSION,
        gpu: engine.gpu().name.clone(),
    }
}

#[tauri::command]
fn queue_snapshot(engine: State<Engine>) -> QueueSnapshot {
    engine.snapshot()
}

#[tauri::command]
fn queue_add(
    engine: State<Engine>,
    paths: Vec<PathBuf>,
    options: Map<String, Value>,
) -> CmdResult<usize> {
    engine.add(&paths, options).map_err(err)
}

#[tauri::command]
fn queue_remove(engine: State<Engine>, id: String) -> CmdResult<()> {
    engine.remove(&id).map_err(err)
}

#[tauri::command]
fn queue_move(engine: State<Engine>, id: String, delta: i64) {
    engine.move_job(&id, delta);
}

#[tauri::command]
fn queue_retry(engine: State<Engine>, id: String) {
    engine.retry(&id);
}

#[tauri::command]
fn queue_clear_finished(engine: State<Engine>) {
    engine.clear_finished();
}

#[tauri::command]
fn queue_start(engine: State<Engine>) {
    engine.start();
}

#[tauri::command]
fn queue_pause(engine: State<Engine>) {
    engine.pause();
}

#[tauri::command]
fn queue_resume(engine: State<Engine>) {
    engine.resume();
}

#[tauri::command]
fn queue_stop_after(engine: State<Engine>, on: bool) {
    engine.stop_after_current(on);
}

#[tauri::command]
fn queue_cancel(engine: State<Engine>) {
    engine.cancel();
}

#[tauri::command]
fn job_log(engine: State<Engine>, id: String) -> String {
    engine.job_log(&id, 400)
}

#[tauri::command]
fn settings_get(engine: State<Engine>) -> AppSettings {
    engine.settings()
}

#[tauri::command]
fn settings_set(engine: State<Engine>, settings: AppSettings) -> CmdResult<()> {
    engine.set_settings(settings).map_err(err)
}

#[tauri::command]
async fn tools_status(engine: State<'_, Engine>) -> CmdResult<Vec<ToolStatus>> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.tools_status())
        .await
        .map_err(err)
}

#[tauri::command]
async fn tools_update(engine: State<'_, Engine>) -> CmdResult<Vec<String>> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.update_tools())
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
fn show_in_folder(path: PathBuf) -> CmdResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(err)
}

#[tauri::command]
fn open_path(path: PathBuf) -> CmdResult<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let handle = app.handle().clone();
            let engine = Engine::new(
                data_dir,
                Arc::new(move |event| {
                    let _ = handle.emit("engine", &event);
                }),
            )?;
            app.manage(engine);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            queue_snapshot,
            queue_add,
            queue_remove,
            queue_move,
            queue_retry,
            queue_clear_finished,
            queue_start,
            queue_pause,
            queue_resume,
            queue_stop_after,
            queue_cancel,
            job_log,
            settings_get,
            settings_set,
            tools_status,
            tools_update,
            show_in_folder,
            open_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
