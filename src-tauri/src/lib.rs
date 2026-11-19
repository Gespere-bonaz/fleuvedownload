//! Couche Tauri : relie le moteur `fleuve-core` à l'interface React.

use fleuve_core::{server, AddRequest, DownloadView, Manager as Engine, QualityOption, RequestHeaders, Settings};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn list_downloads(engine: State<'_, Engine>) -> Vec<DownloadView> {
    engine.list()
}

#[tauri::command]
fn add_downloads(engine: State<'_, Engine>, requests: Vec<AddRequest>) -> CmdResult<Vec<String>> {
    requests.into_iter().map(|r| engine.add(r).map_err(err)).collect()
}

#[tauri::command]
fn pause_download(engine: State<'_, Engine>, id: String) -> CmdResult<()> {
    engine.pause(&id).map_err(err)
}

#[tauri::command]
fn clear_list(engine: State<'_, Engine>, all: bool) -> CmdResult<usize> {
    engine.clear(all).map_err(err)
}

#[tauri::command]
fn pause_all(engine: State<'_, Engine>) -> CmdResult<usize> {
    engine.pause_all().map_err(err)
}

#[tauri::command]
fn resume_download(engine: State<'_, Engine>, id: String) -> CmdResult<()> {
    engine.resume(&id).map_err(err)
}

#[tauri::command]
fn schedule_download(engine: State<'_, Engine>, id: String, at: i64) -> CmdResult<()> {
    engine.schedule(&id, at).map_err(err)
}

#[tauri::command]
fn remove_download(engine: State<'_, Engine>, id: String, delete_file: bool) -> CmdResult<()> {
    engine.remove(&id, delete_file).map_err(err)
}

#[tauri::command]
fn set_quality(engine: State<'_, Engine>, id: String, quality: String) -> CmdResult<()> {
    engine.set_quality(&id, &quality).map_err(err)
}

#[tauri::command]
fn is_video_url(url: String) -> bool {
    Engine::is_video_url(url.trim())
}

#[tauri::command]
async fn probe_qualities(engine: State<'_, Engine>, url: String, referer: Option<String>) -> CmdResult<Vec<QualityOption>> {
    let headers = RequestHeaders { referer, ..Default::default() };
    engine.probe_qualities(url.trim(), &headers).await.map_err(err)
}

#[tauri::command]
async fn probe_qualities_for(engine: State<'_, Engine>, id: String) -> CmdResult<Vec<QualityOption>> {
    engine.probe_qualities_for(&id).await.map_err(err)
}

#[tauri::command]
fn get_settings(engine: State<'_, Engine>) -> Settings {
    engine.settings()
}

#[tauri::command]
fn save_settings(engine: State<'_, Engine>, settings: Settings) -> CmdResult<Settings> {
    engine.set_settings(settings).map_err(err)
}

#[tauri::command]
fn open_download(app: AppHandle, engine: State<'_, Engine>, id: String) -> CmdResult<()> {
    let path = engine.file_path(&id).ok_or("Téléchargement introuvable")?;
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(err)
}

#[tauri::command]
fn reveal_download(app: AppHandle, engine: State<'_, Engine>, id: String) -> CmdResult<()> {
    let path = engine.file_path(&id).ok_or("Téléchargement introuvable")?;
    let target = if path.exists() { path } else { path.parent().map(|p| p.to_path_buf()).unwrap_or(path) };
    app.opener().reveal_item_in_dir(target).map_err(err)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let downloads = app
                .path()
                .download_dir()
                .unwrap_or_else(|_| data_dir.join("Téléchargements"));
            let engine = tauri::async_runtime::block_on(Engine::new(data_dir, downloads))?;
            app.manage(engine.clone());

            // Rafraîchit l'interface à chaque changement (au plus toutes les 500 ms
            // pendant un téléchargement).
            let handle = app.handle().clone();
            let watched = engine.clone();
            tauri::async_runtime::spawn(async move {
                let mut rx = watched.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = handle.emit("downloads", watched.list());
                        }
                        Err(_) => break,
                    }
                }
            });

            // Serveur local pour l'extension navigateur.
            let handle = app.handle().clone();
            let port = engine.settings().server_port;
            let on_added: server::OnAdded = Arc::new(move |id: String, focus: bool| {
                let _ = handle.emit("added-from-browser", id);
                if !focus {
                    return;
                }
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            });
            let srv_engine = engine.clone();
            let err_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::serve(srv_engine, port, on_added).await {
                    let _ = err_handle.emit(
                        "server-error",
                        format!("Le lien avec le navigateur n'a pas pu démarrer sur le port {port} : {e}"),
                    );
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_downloads,
            add_downloads,
            set_quality,
            is_video_url,
            probe_qualities,
            probe_qualities_for,
            pause_download,
            pause_all,
            clear_list,
            resume_download,
            schedule_download,
            remove_download,
            get_settings,
            save_settings,
            open_download,
            reveal_download
        ])
        .build(tauri::generate_context!())
        .expect("erreur au lancement de Fleuve")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                let engine = app.state::<Engine>().inner().clone();
                tauri::async_runtime::block_on(engine.shutdown());
            }
        });
}
