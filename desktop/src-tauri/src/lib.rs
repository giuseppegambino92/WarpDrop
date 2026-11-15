//! WarpDrop desktop shell: hosts the `libwarp` engine in-process and exposes
//! it to the React UI, the system tray and native notifications.

mod actions;
mod bridge;
mod clipboard;
mod commands;
mod i18n;
mod settings;
mod state;
mod tray;

use settings::{Paths, Settings};
use state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};

/// Passed by the autostart entry so login launches stay in the tray.
const HIDDEN_ARG: &str = "--hidden";

pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn toggle_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
            let _ = w.hide();
        } else {
            show_main_window(app);
        }
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).map(|v| !v.is_empty() && v != "0").unwrap_or(false)
}

pub fn run() {
    let start_hidden = std::env::args().any(|a| a == HIDDEN_ARG);
    let profile_dir = std::env::var_os("WARPDROP_PROFILE_DIR").map(PathBuf::from);

    let mut builder = tauri::Builder::default();
    // A second launch focuses the running instance. Custom profiles opt out so
    // two instances can run side by side during development.
    if profile_dir.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main_window(app)));
    }
    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![HIDDEN_ARG])))
        .setup(move |app| {
            setup(app.handle(), start_hidden, profile_dir.clone())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps WarpDrop running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::boot_info,
            commands::get_snapshot,
            commands::rotate_code,
            commands::qr_matrix,
            commands::normalize_code,
            commands::send_files,
            commands::send_files_to_code,
            commands::send_clipboard,
            commands::respond_to_offer,
            commands::pause_transfer,
            commands::resume_transfer,
            commands::cancel_transfer,
            commands::clear_finished_transfers,
            commands::open_link_window,
            commands::close_link_window,
            commands::link_with_code,
            commands::untrust_device,
            commands::copy_text,
            commands::reveal_path,
            commands::set_device_name,
            commands::set_download_dir,
            commands::set_relay_url,
            commands::set_language,
            commands::set_launch_at_login,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build WarpDrop")
        .run(|app, event| match event {
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => show_main_window(app),
            RunEvent::Exit => {
                if let Some(node) = app.state::<AppState>().node() {
                    node.shutdown();
                }
            }
            _ => {}
        });
}

fn setup(app: &AppHandle, start_hidden: bool, profile_dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let (config_dir, data_dir) = match &profile_dir {
        Some(p) => (p.join("config"), p.join("data")),
        None => (app.path().app_config_dir()?, app.path().app_data_dir()?),
    };
    let paths = Paths { settings_file: config_dir.join("settings.json"), state_dir: data_dir.join("node") };

    let mut settings = Settings::load(&paths.settings_file);
    if let Ok(dir) = std::env::var("WARPDROP_DOWNLOAD_DIR") {
        settings.download_dir = dir;
    }
    if settings.download_dir.is_empty() {
        let downloads = app.path().download_dir().or_else(|_| app.path().home_dir().map(|h| h.join("Downloads")))?;
        settings.download_dir = downloads.to_string_lossy().into_owned();
    }
    settings.launch_at_login = app.autolaunch().is_enabled().unwrap_or(settings.launch_at_login);

    let debug_print = env_flag("WARPDROP_DEBUG_PRINT_CODE");
    app.manage(AppState::new(settings.clone(), paths.clone(), debug_print));
    let state = app.state::<AppState>();
    state.save_settings();

    if let Some(window) = app.get_webview_window("main") {
        *state.material.lock() = apply_material(&window);
    }

    match tray::create(app) {
        Ok(t) => *state.tray.lock() = Some(t),
        Err(e) => eprintln!("warpdrop: tray unavailable: {e}"),
    }

    let mut cfg = warp::default_node_config(
        settings.device_name.clone(),
        paths.state_dir.to_string_lossy().into_owned(),
        settings.download_dir.clone(),
    );
    cfg.device_kind = warp::DeviceKind::current();
    cfg.enable_mdns = true;
    cfg.enable_beacon = true;
    cfg.enable_adb = true;
    cfg.relay_url = settings.relay_url.clone();
    match warp::WarpNode::new(cfg, Arc::new(bridge::Bridge { app: app.clone() })) {
        Ok(node) => {
            // The code itself is printed by the bridge on the initial `codeChanged`.
            if debug_print {
                bridge::debug_line(&format!("WARPDROP_DOWNLOAD_DIR {}", node.download_dir()));
            }
            *state.code.lock() = Some(node.current_code());
            let _ = state.node.set(node);
        }
        Err(e) => {
            eprintln!("warpdrop: engine failed to start: {e}");
            *state.node_error.lock() = Some(e.to_string());
        }
    }
    tray::refresh(app);

    if !start_hidden {
        show_main_window(app);
    }
    Ok(())
}

/// Native window material behind the (transparent) webview.
fn apply_material(window: &WebviewWindow) -> &'static str {
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        if apply_vibrancy(window, NSVisualEffectMaterial::Sidebar, Some(NSVisualEffectState::FollowsWindowActiveState), None)
            .is_ok()
        {
            return "vibrancy";
        }
    }
    #[cfg(target_os = "windows")]
    {
        use window_vibrancy::{apply_acrylic, apply_mica};
        if apply_mica(window, None).is_ok() {
            return "mica";
        }
        if apply_acrylic(window, Some((0, 0, 0, 0))).is_ok() {
            return "acrylic";
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = window;
    "none"
}
