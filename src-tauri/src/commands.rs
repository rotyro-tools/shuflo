use std::path::PathBuf;
use std::sync::atomic::Ordering;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::backup::{self, BackupSummary};
use crate::shuffle::fast::{self, Outcome, Progress};
use crate::spotify::api::{check_id, ApiError, PlaylistInfo};
use crate::spotify::auth;
use crate::state::AppState;

/// Rejects the call if it did not originate from one of the expected windows.
fn require_window(window: &tauri::WebviewWindow, labels: &[&str]) -> Result<(), String> {
    if !labels.contains(&window.label()) {
        return Err("command not available from this window".to_string());
    }
    Ok(())
}

fn is_not_found_os_error(message: &str) -> bool {
    message.to_ascii_lowercase().contains("os error 2")
}

fn backups_root(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("backups"))
        .map_err(|e| e.to_string())
}

pub fn show_window(app: &AppHandle, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Tells the window to reload what it shows (login, playlist or settings changed).
fn emit_status_changed(app: &AppHandle) {
    let _ = app.emit("status-changed", ());
}

// ── Main window ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    client_id_configured: bool,
    client_id: Option<String>,
    logged_in: bool,
    user_name: Option<String>,
    playlist_id: Option<String>,
    playlist: Option<PlaylistInfo>,
    playlist_error: Option<String>,
    last_shuffled_at: Option<u64>,
    running: bool,
}

#[tauri::command]
pub async fn get_status(window: tauri::WebviewWindow, app: AppHandle) -> Result<Status, String> {
    require_window(&window, &["main"])?;
    let settings = crate::settings::load(&app);
    let mut status = Status {
        client_id_configured: crate::settings::client_id(&app).is_ok(),
        client_id: settings.client_id.clone(),
        logged_in: false,
        user_name: None,
        playlist_id: settings.playlist_id.clone(),
        playlist: None,
        playlist_error: None,
        last_shuffled_at: None,
        running: app.state::<AppState>().is_running(),
    };
    if !auth::has_session(&app) {
        return Ok(status);
    }

    match auth::current_user(&app).await {
        Ok(user) => {
            status.logged_in = true;
            status.user_name = Some(user.display_name.unwrap_or(user.id));
        }
        Err(ApiError::LoggedOut) => return Ok(status),
        // Offline or Spotify trouble: keep the session and show the error on the playlist card.
        Err(e) => {
            status.logged_in = true;
            status.playlist_error = Some(e.to_string());
            return Ok(status);
        }
    }

    if let Some(id) = settings.playlist_id {
        status.last_shuffled_at = crate::settings::last_shuffled(&app, &id);
        match auth::client(&app).playlist(&id).await {
            Ok(info) => status.playlist = Some(info),
            Err(ApiError::LoggedOut) => status.logged_in = false,
            Err(e) => status.playlist_error = Some(e.to_string()),
        }
    }
    Ok(status)
}

#[tauri::command]
pub async fn login(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    auth::login(&app).await?;
    emit_status_changed(&app);
    // Bring Shuflo back in front of the browser.
    show_window(&app, window.label());
    Ok(())
}

#[tauri::command]
pub async fn list_playlists(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Vec<PlaylistInfo>, String> {
    require_window(&window, &["main"])?;
    let user = auth::current_user(&app).await?;
    Ok(auth::client(&app).my_playlists(&user.id).await?)
}

#[tauri::command]
pub async fn set_playlist(
    window: tauri::WebviewWindow,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    require_window(&window, &["main"])?;
    check_id(&id)?;
    let user = auth::current_user(&app).await?;
    let info = auth::client(&app).playlist(&id).await?;
    if !info.editable_by(&user.id) {
        return Err(format!(
            "\"{}\" belongs to another account that hasn't added you as a collaborator. \
             Ask its owner to invite you, or pick another playlist.",
            info.name
        ));
    }
    let mut settings = crate::settings::load(&app);
    settings.playlist_id = Some(info.id);
    crate::settings::save(&app, &settings)?;
    emit_status_changed(&app);
    Ok(())
}

/// Backs up and shuffles the chosen playlist.
#[tauri::command]
pub async fn shuffle(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    let id = crate::settings::load(&app)
        .playlist_id
        .ok_or("Choose a playlist first.")?;
    let client = auth::client(&app);
    let progress = emit_progress(&app);
    run_locked(&app, RunKind::Shuffle, |root, user_id| async move {
        fast::shuffle(&client, &id, &user_id, &root, &progress).await
    })
    .await
    .map(|_| ())
}

/// Opens the playlist in the Spotify desktop app, or in the web player when the app
/// isn't installed. The web player can show a cached order until it is reloaded.
#[tauri::command]
pub fn open_playlist(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    let id = crate::settings::load(&app)
        .playlist_id
        .ok_or("No playlist chosen yet.")?;
    check_id(&id)?;
    if app
        .opener()
        .open_url(format!("spotify:playlist:{id}"), None::<&str>)
        .is_ok()
    {
        return Ok(());
    }
    app.opener()
        .open_url(
            format!("https://open.spotify.com/playlist/{id}"),
            None::<&str>,
        )
        .map_err(|e| e.to_string())
}

/// Opens the page where the Spotify app (and its Client ID) is created.
#[tauri::command]
pub fn open_spotify_dashboard(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    app.opener()
        .open_url(SPOTIFY_DASHBOARD_URL, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Saves the Client ID entered during setup or in Settings. A login belongs to the
/// Spotify app it was made with, so changing the Client ID logs out.
#[tauri::command]
pub async fn set_client_id(
    window: tauri::WebviewWindow,
    app: AppHandle,
    client_id: String,
) -> Result<(), String> {
    require_window(&window, &["main"])?;
    let client_id = client_id.trim().to_string();
    if !crate::settings::is_valid_client_id(&client_id) {
        return Err(
            "That doesn't look like a Client ID. It's 32 letters and numbers, \
             shown in your Spotify app's settings."
                .to_string(),
        );
    }
    let mut settings = crate::settings::load(&app);
    let changed = settings.client_id.as_deref() != Some(client_id.as_str());
    settings.client_id = Some(client_id);
    crate::settings::save(&app, &settings)?;
    if changed {
        auth::logout(&app).await;
    }
    emit_status_changed(&app);
    Ok(())
}

const SPOTIFY_DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";

// ── Shuffle and restore runs ─────────────────────────────────────────────────

fn notify(app: &AppHandle, title: &str, body: &str) {
    // The main window already shows the result when it has focus.
    let main_focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if main_focused {
        return;
    }
    let _ = app.notification().builder().title(title).body(body).show();
}

#[derive(Clone, Copy, PartialEq)]
enum RunKind {
    Shuffle,
    Restore,
}

/// Runs one write to Spotify under the run lock, emitting `shuffle-state` and
/// `shuffle-progress` events and a notification at the end.
async fn run_locked<F, Fut>(app: &AppHandle, kind: RunKind, work: F) -> Result<Outcome, String>
where
    F: FnOnce(PathBuf, String) -> Fut,
    Fut: std::future::Future<Output = Result<Outcome, String>>,
{
    let state = app.state::<AppState>();
    let guard = state
        .try_start_run()
        .ok_or("Shuflo is already updating the playlist.")?;
    let kind_name = if kind == RunKind::Shuffle {
        "shuffle"
    } else {
        "restore"
    };
    let _ = app.emit(
        "shuffle-state",
        json!({ "state": "running", "kind": kind_name }),
    );

    let result = async {
        let user = auth::current_user(app).await?;
        work(backups_root(app)?, user.id).await
    }
    .await;

    if let (RunKind::Shuffle, Ok(outcome)) = (kind, &result) {
        let _ = crate::settings::set_last_shuffled(app, &outcome.info.id, backup::now_ms());
    }
    // Release the lock before announcing the result: the window refreshes on that
    // event and must not see the run as still in progress.
    drop(guard);
    match &result {
        Ok(outcome) => {
            let _ = app.emit(
                "shuffle-state",
                json!({ "state": "done", "kind": kind_name }),
            );
            let (title, body) = match kind {
                RunKind::Shuffle => (
                    "Playlist shuffled",
                    format!(
                        "{} is ready. Play it with Spotify's shuffle turned off.",
                        outcome.info.name
                    ),
                ),
                RunKind::Restore => (
                    "Previous order restored",
                    format!("{} is back in its saved order.", outcome.info.name),
                ),
            };
            notify(app, title, &body);
        }
        Err(message) => {
            let _ = app.emit(
                "shuffle-state",
                json!({ "state": "error", "kind": kind_name, "message": message }),
            );
            let title = match kind {
                RunKind::Shuffle => "Shuffle failed",
                RunKind::Restore => "Restore failed",
            };
            notify(app, title, message);
        }
    }
    result
}

fn emit_progress(app: &AppHandle) -> impl Fn(Progress) + Send + Sync {
    let app = app.clone();
    move |p: Progress| {
        let _ = app.emit("shuffle-progress", p);
    }
}

// ── Settings view ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    launch_at_login: bool,
    client_id: Option<String>,
    user_name: Option<String>,
    keychain_ok: bool,
}

#[tauri::command]
pub async fn get_settings(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<SettingsView, String> {
    require_window(&window, &["main"])?;
    let settings = crate::settings::load(&app);
    let user = if auth::has_session(&app) {
        auth::current_user(&app).await.ok()
    } else {
        None
    };
    Ok(SettingsView {
        // The OS login item is the source of truth; it can be removed outside Shuflo.
        launch_at_login: app
            .autolaunch()
            .is_enabled()
            .unwrap_or(settings.launch_at_login),
        client_id: settings.client_id,
        user_name: user.map(|u| u.display_name.unwrap_or(u.id)),
        keychain_ok: app.state::<AppState>().keychain_ok.load(Ordering::SeqCst),
    })
}

#[tauri::command]
pub fn set_launch_at_login(
    window: tauri::WebviewWindow,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    require_window(&window, &["main"])?;
    let mut settings = crate::settings::load(&app);
    settings.launch_at_login = enabled;
    crate::settings::save(&app, &settings)?;

    if enabled {
        app.autolaunch().enable().map_err(|e| e.to_string())?;
    } else if let Err(e) = app.autolaunch().disable() {
        let msg = e.to_string();
        if !is_not_found_os_error(&msg) {
            return Err(msg);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn logout(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    auth::logout(&app).await;
    emit_status_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn list_backups(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> Result<Vec<BackupSummary>, String> {
    require_window(&window, &["main"])?;
    match crate::settings::load(&app).playlist_id {
        Some(id) => backup::list(&backups_root(&app)?, &id),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub async fn restore_backup(
    window: tauri::WebviewWindow,
    app: AppHandle,
    backup_id: String,
) -> Result<(), String> {
    require_window(&window, &["main"])?;
    let id = crate::settings::load(&app)
        .playlist_id
        .ok_or("Choose a playlist first.")?;
    let saved = backup::load(&backups_root(&app)?, &id, &backup_id)?;
    let client = auth::client(&app);
    let progress = emit_progress(&app);
    run_locked(&app, RunKind::Restore, |root, user_id| async move {
        fast::restore(&client, &id, &user_id, &root, &saved, &progress).await
    })
    .await
    .map(|_| ())
}

#[tauri::command]
pub fn open_backups_folder(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    let dir = backups_root(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Returns the app version from Cargo metadata.
#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

// ── Updates ──────────────────────────────────────────────────────────────────

/// Spawns a full update flow (check → download → install → restart) with live
/// status events emitted to the main window for UI feedback.
#[tauri::command]
pub async fn check_update(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_window(&window, &["main"])?;
    tauri::async_runtime::spawn(async move {
        run_update_install(app).await;
    });
    Ok(())
}

/// Full update flow triggered from the Settings view.
/// Emits "update-status" events to the main window at each stage.
async fn run_update_install(app: AppHandle) {
    use tauri_plugin_updater::UpdaterExt;

    macro_rules! emit {
        ($payload:expr) => {
            let _ = app.emit_to("main", "update-status", $payload);
        };
    }

    emit!(json!({ "state": "checking" }));

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            emit!(json!({ "state": "error", "message": e.to_string() }));
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            emit!(json!({ "state": "up-to-date" }));
            return;
        }
        Err(e) => {
            emit!(json!({ "state": "error", "message": e.to_string() }));
            return;
        }
    };

    let version = update.version.clone();
    emit!(json!({ "state": "downloading", "version": version }));
    let _ = app
        .notification()
        .builder()
        .title("Shuflo update available")
        .body(format!("Downloading version {version}…"))
        .show();

    if let Err(e) = update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
    {
        emit!(json!({ "state": "error", "message": e.to_string() }));
        return;
    }

    emit!(json!({ "state": "installing" }));
    app.restart();
}

/// Silent startup update check — shows a system notification if an update is available.
pub async fn run_update_check(app: AppHandle) {
    use tauri_plugin_updater::UpdaterExt;

    let updater = match app.updater() {
        Ok(u) => u,
        Err(_) => return,
    };

    if let Ok(Some(update)) = updater.check().await {
        let _ = app
            .notification()
            .builder()
            .title("Update available")
            .body(format!("v{} is available.", update.version))
            .show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_os_error_detection_handles_windows_style_messages() {
        assert!(is_not_found_os_error(
            "The system cannot find the file specified. (os error 2)"
        ));
        assert!(!is_not_found_os_error("permission denied (os error 5)"));
    }
}
