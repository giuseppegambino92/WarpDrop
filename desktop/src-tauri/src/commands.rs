//! `invoke` commands for the webview. Every command is async so it runs on
//! Tauri's worker pool, never on the UI thread.

use crate::actions::remember_last_peer;
use crate::clipboard::ClipError;
use crate::i18n::{self, Lang};
use crate::settings::{normalize_relay, LanguagePref, Settings};
use crate::state::AppState;
use crate::tray;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt as _;
use warp::{
    CodeInfo, DeviceIdentity, PeerInfo, QrMatrix, SendItem, TransferSnapshot, TrustedDevice, WarpError, WarpNode,
};

/// Error shape the UI localizes: `code` selects the message, `message` is the
/// engine's (English) detail for the generic case.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CmdError {
    pub code: &'static str,
    pub message: String,
}

impl CmdError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        CmdError { code, message: message.into() }
    }
}

impl From<WarpError> for CmdError {
    fn from(e: WarpError) -> Self {
        let code = match &e {
            WarpError::InvalidCode => "invalidCode",
            WarpError::WrongCode => "wrongCode",
            WarpError::CodeNotFound => "codeNotFound",
            WarpError::PeerNotFound => "peerNotFound",
            WarpError::Unreachable { .. } => "unreachable",
            WarpError::Rejected { .. } => "rejected",
            WarpError::Timeout => "timeout",
            WarpError::Cancelled => "cancelled",
            WarpError::TransferNotFound => "transferNotFound",
            WarpError::Io { .. } => "io",
            WarpError::Config { .. } | WarpError::Crypto { .. } | WarpError::Protocol { .. } => "generic",
        };
        CmdError::new(code, e.to_string())
    }
}

impl From<ClipError> for CmdError {
    fn from(e: ClipError) -> Self {
        match e {
            ClipError::Empty => CmdError::new("clipboardEmpty", "clipboard is empty"),
            ClipError::Unavailable => CmdError::new("clipboardRead", "clipboard unavailable"),
        }
    }
}

type CmdResult<T> = Result<T, CmdError>;

fn node(state: &AppState) -> CmdResult<Arc<WarpNode>> {
    state.node().ok_or_else(|| {
        CmdError::new("notReady", state.node_error.lock().clone().unwrap_or_else(|| "engine not started".into()))
    })
}

fn items(paths: Vec<String>) -> Vec<SendItem> {
    paths.into_iter().map(|path| SendItem { path, name: None }).collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    identity: DeviceIdentity,
    code: CodeInfo,
    peers: Vec<PeerInfo>,
    trusted: Vec<TrustedDevice>,
    transfers: Vec<TransferSnapshot>,
    settings: Settings,
    language: Lang,
    platform: &'static str,
    material: &'static str,
    version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootInfo {
    /// `None` when the engine is running.
    error: Option<String>,
    language: Lang,
    platform: &'static str,
    material: &'static str,
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Cheap first call: language/platform/material for the first paint, plus
/// the startup error if the engine failed to start.
#[tauri::command]
pub async fn boot_info(state: State<'_, AppState>) -> CmdResult<BootInfo> {
    Ok(BootInfo {
        error: if state.node().is_some() { None } else { Some(state.node_error.lock().clone().unwrap_or_default()) },
        language: *state.lang.read(),
        platform: platform(),
        material: *state.material.lock(),
    })
}

#[tauri::command]
pub async fn get_snapshot(state: State<'_, AppState>) -> CmdResult<Snapshot> {
    let n = node(&state)?;
    let mut settings = state.settings.lock().clone();
    settings.download_dir = n.download_dir();
    Ok(Snapshot {
        identity: n.identity(),
        code: n.current_code(),
        peers: n.peers(),
        trusted: n.trusted_devices(),
        transfers: n.transfers(),
        settings,
        language: *state.lang.read(),
        platform: platform(),
        material: *state.material.lock(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
pub async fn rotate_code(state: State<'_, AppState>) -> CmdResult<()> {
    node(&state)?.rotate_code();
    Ok(())
}

#[tauri::command]
pub async fn qr_matrix(text: String) -> CmdResult<QrMatrix> {
    Ok(warp::qr_matrix(text)?)
}

/// Six digits for a code / `warpdrop:` link, `None` when it isn't one.
#[tauri::command]
pub async fn normalize_code(input: String) -> CmdResult<Option<String>> {
    Ok(warp::normalize_code(input))
}

#[tauri::command]
pub async fn send_files(app: AppHandle, state: State<'_, AppState>, peer_id: String, paths: Vec<String>) -> CmdResult<String> {
    let id = node(&state)?.send_files(peer_id.clone(), items(paths))?;
    remember_last_peer(&app, &peer_id);
    Ok(id)
}

#[tauri::command]
pub async fn send_files_to_code(state: State<'_, AppState>, code: String, paths: Vec<String>) -> CmdResult<String> {
    Ok(node(&state)?.send_files_to_code(code, items(paths))?)
}

/// Reads the system clipboard and sends it to a device or a code.
#[tauri::command]
pub async fn send_clipboard(
    app: AppHandle,
    state: State<'_, AppState>,
    peer_id: Option<String>,
    code: Option<String>,
) -> CmdResult<String> {
    let n = node(&state)?;
    let content = state.clipboard.read()?;
    match (peer_id, code) {
        (Some(peer), _) => {
            let id = n.send_clipboard(peer.clone(), content)?;
            remember_last_peer(&app, &peer);
            Ok(id)
        }
        (None, Some(code)) => Ok(n.send_clipboard_to_code(code, content)?),
        (None, None) => Err(CmdError::new("pickDevice", "no target")),
    }
}

#[tauri::command]
pub async fn respond_to_offer(state: State<'_, AppState>, offer_id: String, accept: bool) -> CmdResult<()> {
    node(&state)?.respond_to_offer(offer_id, accept);
    Ok(())
}

#[tauri::command]
pub async fn pause_transfer(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(node(&state)?.pause_transfer(id)?)
}

#[tauri::command]
pub async fn resume_transfer(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(node(&state)?.resume_transfer(id)?)
}

#[tauri::command]
pub async fn cancel_transfer(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    Ok(node(&state)?.cancel_transfer(id)?)
}

/// Drops finished rows and returns what is left.
#[tauri::command]
pub async fn clear_finished_transfers(state: State<'_, AppState>) -> CmdResult<Vec<TransferSnapshot>> {
    let n = node(&state)?;
    n.clear_finished_transfers();
    let left = n.transfers();
    state.transfer_states.lock().retain(|id, _| left.iter().any(|t| &t.id == id));
    Ok(left)
}

#[tauri::command]
pub async fn open_link_window(state: State<'_, AppState>, seconds: u32) -> CmdResult<()> {
    node(&state)?.open_link_window(seconds);
    Ok(())
}

#[tauri::command]
pub async fn close_link_window(state: State<'_, AppState>) -> CmdResult<()> {
    node(&state)?.close_link_window();
    Ok(())
}

#[tauri::command]
pub async fn link_with_code(state: State<'_, AppState>, code: String) -> CmdResult<()> {
    Ok(node(&state)?.link_with_code(code)?)
}

#[tauri::command]
pub async fn untrust_device(app: AppHandle, state: State<'_, AppState>, device_id: String) -> CmdResult<()> {
    node(&state)?.untrust_device(device_id)?;
    tray::schedule_refresh(&app);
    Ok(())
}

#[tauri::command]
pub async fn copy_text(state: State<'_, AppState>, text: String) -> CmdResult<()> {
    Ok(state.clipboard.write_text(text)?)
}

#[tauri::command]
pub async fn reveal_path(path: String) -> CmdResult<()> {
    tauri_plugin_opener::reveal_item_in_dir(&path).map_err(|e| CmdError::new("io", e.to_string()))
}

fn updated_settings(state: &AppState) -> Settings {
    let mut s = state.settings.lock().clone();
    if let Some(n) = state.node() {
        s.download_dir = n.download_dir();
    }
    s
}

#[tauri::command]
pub async fn set_device_name(state: State<'_, AppState>, name: String) -> CmdResult<Settings> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(CmdError::new("invalidName", "name must be 1–64 characters"));
    }
    node(&state)?.set_device_name(name.clone());
    state.settings.lock().device_name = name;
    state.save_settings();
    Ok(updated_settings(&state))
}

#[tauri::command]
pub async fn set_download_dir(state: State<'_, AppState>, dir: String) -> CmdResult<Settings> {
    node(&state)?.set_download_dir(dir.clone())?;
    state.settings.lock().download_dir = dir;
    state.save_settings();
    Ok(updated_settings(&state))
}

#[tauri::command]
pub async fn set_relay_url(state: State<'_, AppState>, url: Option<String>) -> CmdResult<Settings> {
    let url = normalize_relay(url).map_err(|_| CmdError::new("invalidRelay", "relay must be ws:// or wss://"))?;
    node(&state)?.set_relay_url(url.clone());
    state.settings.lock().relay_url = url;
    state.save_settings();
    Ok(updated_settings(&state))
}

/// Switches the UI + tray language immediately. Returns the resolved language.
#[tauri::command]
pub async fn set_language(app: AppHandle, state: State<'_, AppState>, language: LanguagePref) -> CmdResult<Lang> {
    let lang = i18n::resolve(&language);
    state.settings.lock().language = language;
    *state.lang.write() = lang;
    state.save_settings();
    tray::schedule_refresh(&app);
    Ok(lang)
}

#[tauri::command]
pub async fn set_launch_at_login(app: AppHandle, state: State<'_, AppState>, enabled: bool) -> CmdResult<Settings> {
    let al = app.autolaunch();
    let r = if enabled { al.enable() } else { al.disable() };
    r.map_err(|e| CmdError::new("io", e.to_string()))?;
    state.settings.lock().launch_at_login = al.is_enabled().unwrap_or(enabled);
    state.save_settings();
    Ok(updated_settings(&state))
}
