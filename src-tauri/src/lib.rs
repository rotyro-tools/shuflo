mod backup;
mod commands;
mod settings;
mod shuffle;
mod spotify;
mod state;

use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, WindowEvent};

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .setup(|app| {
            // Unlike typox, Shuflo is a regular app: Dock/taskbar icon, window shown at launch.
            // ── Tray menu ─────────────────────────────────────────────────────
            let settings_item = MenuItemBuilder::with_id("settings", "Settings").build(app)?;
            let sep = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quit Shuflo").build(app)?;

            let menu = MenuBuilder::new(app)
                .items(&[&settings_item, &sep, &quit_item])
                .build()?;

            // macOS draws a black-on-transparent template that adapts to the menu bar;
            // other platforms use the full-colour app icon.
            #[cfg(target_os = "macos")]
            let tray_icon =
                tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
            #[cfg(not(target_os = "macos"))]
            let tray_icon = app
                .default_window_icon()
                .ok_or("no app icon configured")?
                .clone();

            TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(true)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    // Settings is a view inside the main window.
                    "settings" => {
                        commands::show_window(app, "main");
                        let _ = app.emit_to("main", "show-settings", ());
                    }
                    // Through app.exit, so a running write can hold it back (see below).
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            commands::show_window(app.handle(), "main");

            // ── Non-blocking startup update check ─────────────────────────
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                commands::run_update_check(handle).await;
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            if window.label() != "main" {
                return;
            }
            if window.state::<AppState>().is_running() {
                // Quitting mid-write would leave the playlist half rewritten.
                api.prevent_close();
                let _ = window.emit_to("main", "close-blocked", ());
            } else if cfg!(target_os = "macos") {
                // macOS keeps running in the Dock (clicking the icon reopens the window);
                // on Windows and Linux closing the last window quits, as apps do there.
                // cfg! (not #[cfg]) keeps this code compiled and linted on every platform.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::login,
            commands::list_playlists,
            commands::set_playlist,
            commands::shuffle,
            commands::open_playlist,
            commands::open_spotify_dashboard,
            commands::set_client_id,
            commands::get_settings,
            commands::set_launch_at_login,
            commands::logout,
            commands::list_backups,
            commands::restore_backup,
            commands::open_backups_folder,
            commands::get_app_version,
            commands::check_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // Quitting mid-write would leave the playlist half rewritten.
            tauri::RunEvent::ExitRequested { api, .. } if app.state::<AppState>().is_running() => {
                api.prevent_exit();
                commands::show_window(app, "main");
                let _ = app.emit_to("main", "close-blocked", ());
            }
            // macOS: clicking the Dock icon brings back the hidden main window.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => commands::show_window(app, "main"),
            _ => {}
        });
}
