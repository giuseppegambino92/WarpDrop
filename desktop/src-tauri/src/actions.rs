//! Actions shared by the tray menu and the webview commands.

use crate::bridge::notify;
use crate::clipboard::ClipError;
use crate::state::AppState;
use crate::tray;
use tauri::{AppHandle, Manager};

/// Display name of the last device we sent something to, if still known.
pub fn last_peer_name(app: &AppHandle) -> Option<String> {
    let state = app.state::<AppState>();
    let id = state.settings.lock().last_peer_id.clone()?;
    peer_name(&state, &id)
}

pub fn peer_name(state: &AppState, id: &str) -> Option<String> {
    let node = state.node()?;
    node.peers()
        .into_iter()
        .find(|p| p.device_id == id)
        .map(|p| p.name)
        .or_else(|| node.trusted_devices().into_iter().find(|d| d.device_id == id).map(|d| d.name))
}

pub fn remember_last_peer(app: &AppHandle, peer_id: &str) {
    let state = app.state::<AppState>();
    let changed = {
        let mut s = state.settings.lock();
        let changed = s.last_peer_id.as_deref() != Some(peer_id);
        s.last_peer_id = Some(peer_id.to_string());
        changed
    };
    if changed {
        state.save_settings();
        tray::schedule_refresh(app);
    }
}

pub fn copy_current_code(app: &AppHandle) {
    let state = app.state::<AppState>();
    let code = state.code.lock().as_ref().map(|c| c.display.clone());
    if let Some(code) = code {
        if state.clipboard.write_text(code).is_ok() {
            notify(app, &state.tr("code_copied", &[]), "");
        }
    }
}

/// Tray "Send Clipboard to <last device>". Runs off the main thread because
/// reading the clipboard waits on the clipboard thread.
pub fn send_clipboard_to_last(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let (Some(node), Some(peer)) = (state.node(), state.settings.lock().last_peer_id.clone()) else { return };
        match state.clipboard.read() {
            Ok(content) => {
                if let Err(e) = node.send_clipboard(peer, content) {
                    notify(&app, &state.tr("state_failed", &[]), &e.to_string());
                }
            }
            Err(ClipError::Empty) => notify(&app, &state.tr("clipboard_empty", &[]), ""),
            Err(ClipError::Unavailable) => notify(&app, &state.tr("error_clipboard_read", &[]), ""),
        }
    });
}

pub fn quit(app: &AppHandle) {
    if let Some(node) = app.state::<AppState>().node() {
        node.shutdown();
    }
    app.exit(0);
}
