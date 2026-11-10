//! The Tauri shell: commands call the core queue engine, and engine events go to the window
//! as `engine` events. All the work lives in `upscale-core`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use upscale_core::disc::{Disc, Selection};
use upscale_core::library::{NameRequest, ShowLayout, TitleName};
use upscale_core::process::SystemRunner;
use upscale_core::queue::{AppSettings, Engine, QueueSnapshot};
use upscale_core::tmdb;
use upscale_core::tools::set::ToolStatus;

// Commands that aren't `async fn` are marked `#[tauri::command(async)]`: Tauri would otherwise
// run them on the main thread, where reading a folder on a slow network share, saving the
// queue or reading a log freezes the window.

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

#[tauri::command(async)]
fn app_info(engine: State<Engine>) -> AppInfo {
    AppInfo {
        core_version: upscale_core::VERSION,
        gpu: engine.gpu().name.clone(),
    }
}

#[tauri::command(async)]
fn queue_snapshot(engine: State<Engine>) -> QueueSnapshot {
    engine.snapshot()
}

#[tauri::command(async)]
fn queue_add(
    engine: State<Engine>,
    paths: Vec<PathBuf>,
    options: Map<String, Value>,
) -> CmdResult<usize> {
    engine.add(&paths, options).map_err(err)
}

#[tauri::command(async)]
fn queue_remove(engine: State<Engine>, id: String) -> CmdResult<()> {
    engine.remove(&id).map_err(err)
}

#[tauri::command(async)]
fn queue_move(engine: State<Engine>, id: String, delta: i64) {
    engine.move_job(&id, delta);
}

#[tauri::command(async)]
fn queue_move_to(engine: State<Engine>, id: String, index: usize) {
    engine.move_job_to(&id, index);
}

#[tauri::command(async)]
fn queue_retry(engine: State<Engine>, id: String) {
    engine.retry(&id);
}

#[tauri::command(async)]
fn queue_clear_finished(engine: State<Engine>) {
    engine.clear_finished();
}

#[tauri::command(async)]
fn queue_retry_failed(engine: State<Engine>) {
    engine.retry_failed();
}

#[tauri::command(async)]
fn queue_clear_failed(engine: State<Engine>) {
    engine.clear_failed();
}

#[tauri::command(async)]
fn queue_set_options(
    engine: State<Engine>,
    ids: Vec<String>,
    options: Map<String, Value>,
) -> usize {
    engine.set_options(&ids, options)
}

#[tauri::command(async)]
fn queue_start(engine: State<Engine>) {
    engine.start();
}

#[tauri::command(async)]
fn queue_pause(engine: State<Engine>) {
    engine.pause();
}

#[tauri::command(async)]
fn queue_resume(engine: State<Engine>) {
    engine.resume();
}

#[tauri::command(async)]
fn queue_stop_after(engine: State<Engine>, on: bool) {
    engine.stop_after_current(on);
}

#[tauri::command(async)]
fn queue_cancel(engine: State<Engine>) {
    engine.cancel();
}

#[tauri::command(async)]
fn job_log(engine: State<Engine>, id: String) -> String {
    engine.job_log(&id, 400)
}

#[tauri::command(async)]
fn settings_get(engine: State<Engine>) -> AppSettings {
    engine.settings()
}

#[tauri::command(async)]
fn settings_set(
    app: tauri::AppHandle,
    engine: State<Engine>,
    settings: AppSettings,
) -> CmdResult<()> {
    allow_output(&app, &settings);
    engine.set_settings(settings).map_err(err)
}

/// The webview may load comparison images from the output folder, and nothing else.
fn allow_output(app: &tauri::AppHandle, settings: &AppSettings) {
    let _ = app
        .asset_protocol_scope()
        .allow_directory(&settings.output_dir, true);
}

#[tauri::command(async)]
fn job_compare_images(engine: State<Engine>, id: String) -> Vec<PathBuf> {
    engine.compare_images(&id)
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

/// The titles Upscale would rip from `disc`, and the languages on them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscChoice {
    selection: Selection,
    languages: Vec<Language>,
}

#[derive(Serialize)]
struct Language {
    code: String,
    name: String,
}

#[tauri::command(async)]
fn rip_start(engine: State<Engine>, request: upscale_core::rip::RipRequest) -> CmdResult<()> {
    engine.rip(request).map_err(err)
}

#[tauri::command(async)]
fn rip_cancel(engine: State<Engine>) {
    engine.cancel_rip();
}

#[tauri::command(async)]
fn disc_current(engine: State<Engine>) -> Option<Disc> {
    engine.current_disc()
}

#[tauri::command(async)]
fn disc_choose(disc: Disc) -> DiscChoice {
    let selection = upscale_core::disc::choose_titles(&disc);
    let languages = upscale_core::disc::languages(&disc, &selection)
        .into_iter()
        .map(|(code, name)| Language { code, name })
        .collect();
    DiscChoice {
        selection,
        languages,
    }
}

/// A TMDB GET with the token from Settings, off the UI thread.
async fn tmdb_get(engine: &Engine, url: String) -> CmdResult<Value> {
    let token = engine.settings().tmdb_token;
    tauri::async_runtime::spawn_blocking(move || {
        let curl = upscale_core::tools::find_exe("curl", &[]).ok_or("curl not found")?;
        tmdb::get(&SystemRunner, &curl, &url, &token).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn tmdb_search(engine: State<'_, Engine>, query: String) -> CmdResult<Vec<tmdb::Match>> {
    let v = tmdb_get(&engine, tmdb::search_url(&query)).await?;
    Ok(tmdb::parse_search(&v))
}

#[tauri::command]
async fn tmdb_show(engine: State<'_, Engine>, id: u64) -> CmdResult<Vec<tmdb::Season>> {
    let v = tmdb_get(&engine, tmdb::show_url(id)).await?;
    Ok(tmdb::parse_show(&v))
}

#[tauri::command]
async fn tmdb_season(
    engine: State<'_, Engine>,
    id: u64,
    season: u32,
) -> CmdResult<Vec<tmdb::Episode>> {
    let v = tmdb_get(&engine, tmdb::season_url(id, season)).await?;
    Ok(tmdb::parse_season(&v))
}

/// A show's folder and style in a library, and the first episode to suggest.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShowInfo {
    layout: ShowLayout,
    season: u32,
    episode: u32,
}

#[tauri::command(async)]
fn library_show(
    root: PathBuf,
    name: String,
    year: Option<u32>,
    seasons: Vec<tmdb::Season>,
) -> ShowInfo {
    let layout = upscale_core::library::show_layout(&root, &name, year);
    let sizes: Vec<(u32, u32)> = seasons.iter().map(|s| (s.number, s.episodes)).collect();
    let (season, episode) = upscale_core::library::suggest_first(&layout, &sizes);
    ShowInfo {
        layout,
        season,
        episode,
    }
}

#[tauri::command(async)]
fn library_names(request: NameRequest) -> Vec<TitleName> {
    upscale_core::library::name_titles(&request)
}

/// Settings → Ripping → Install MakeMKV, after the user confirmed in the app.
#[tauri::command]
async fn makemkv_install(engine: State<'_, Engine>) -> CmdResult<Vec<String>> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.install_makemkv())
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command(async)]
fn makemkv_status(engine: State<Engine>) -> upscale_core::tools::makemkv::Registration {
    engine.makemkv_status()
}

#[tauri::command]
async fn makemkv_check(
    engine: State<'_, Engine>,
) -> CmdResult<upscale_core::tools::makemkv::Registration> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.check_makemkv())
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command]
async fn makemkv_register(engine: State<'_, Engine>, key: String) -> CmdResult<Vec<String>> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.register_makemkv(&key))
        .await
        .map_err(err)?
        .map_err(err)
}

#[tauri::command(async)]
fn show_in_folder(path: PathBuf) -> CmdResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(err)
}

#[tauri::command(async)]
fn open_path(path: PathBuf) -> CmdResult<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Opened from Finder, the app wouldn't see Homebrew or the VapourSynth venv
    upscale_core::tools::add_mac_tool_dirs();
    let builder = tauri::Builder::default();
    // One Upscale at a time: a second copy would run the same queue.json and encode the same
    // files. Starting it again brings the window that's already open to the front instead.
    // (Registered first, as the plugin asks.)
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // Updates come from the latest GitHub release (`plugins.updater` in tauri.conf.json);
            // process restarts the app after installing one
            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
                app.handle().plugin(tauri_plugin_process::init())?;
            }
            let data_dir = app.path().app_data_dir()?;
            // A crash leaves its reason in app.log (release builds abort on a panic)
            let _ = std::fs::create_dir_all(&data_dir);
            upscale_core::log::log_panics(data_dir.join("app.log"));
            let handle = app.handle().clone();
            let engine = Engine::new(
                data_dir,
                Arc::new(move |event| {
                    let _ = handle.emit("engine", &event);
                }),
            )?;
            allow_output(app.handle(), &engine.settings());
            // Discs open the Rip sheet
            engine.start_disc_watch();
            app.manage(engine);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window ends the app and whatever it's doing: ask first
            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            let Some(engine) = window.try_state::<Engine>() else {
                return;
            };
            let (encoding, ripping) = engine.busy();
            if !encoding && !ripping {
                return;
            }
            api.prevent_close();
            let what = match (encoding, ripping) {
                (true, true) => "A file is being encoded and a disc is being ripped.",
                (false, true) => "A disc is being ripped.",
                _ => "A file is being encoded.",
            };
            let window = window.clone();
            window
                .dialog()
                .message(format!(
                    "{what} Closing Upscale stops it. A resumable encode continues from its last finished chunk next time; anything else starts over, and a title that was ripping has to be ripped again."
                ))
                .title("Close Upscale?")
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "Close".into(),
                    "Keep running".into(),
                ))
                .show({
                    let window = window.clone();
                    move |close| {
                        if close {
                            let _ = window.destroy();
                        }
                    }
                });
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            queue_snapshot,
            queue_add,
            queue_remove,
            queue_move,
            queue_move_to,
            queue_retry,
            queue_clear_finished,
            queue_retry_failed,
            queue_clear_failed,
            queue_set_options,
            queue_start,
            queue_pause,
            queue_resume,
            queue_stop_after,
            queue_cancel,
            job_log,
            job_compare_images,
            settings_get,
            settings_set,
            tools_status,
            tools_update,
            show_in_folder,
            open_path,
            disc_choose,
            disc_current,
            rip_start,
            rip_cancel,
            tmdb_search,
            tmdb_show,
            tmdb_season,
            library_show,
            library_names,
            makemkv_install,
            makemkv_status,
            makemkv_check,
            makemkv_register,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
