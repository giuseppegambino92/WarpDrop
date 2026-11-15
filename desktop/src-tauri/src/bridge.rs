//! Engine → app bridge: native side effects (tray, clipboard, notifications)
//! and forwarding every event to the webview as `warp-event`.

use crate::i18n::capitalize;
use crate::state::AppState;
use crate::{show_main_window, tray};
use serde_json::{json, Value};
use std::io::Write as _;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use warp::{ClipboardContent, TransferDirection, TransferSnapshot, TransferState, WarpEvent, WarpEventListener};

pub const EVENT_NAME: &str = "warp-event";

pub struct Bridge {
    pub app: AppHandle,
}

impl WarpEventListener for Bridge {
    fn on_event(&self, event: WarpEvent) {
        handle_event(&self.app, event);
    }
}

fn handle_event(app: &AppHandle, event: WarpEvent) {
    let state = app.state::<AppState>();
    let payload = match &event {
        WarpEvent::CodeChanged { code } => {
            *state.code.lock() = Some(code.clone());
            if state.debug_print {
                debug_line(&format!("WARPDROP_CODE {}", code.code));
            }
            tray::schedule_refresh(app);
            to_value(&event)
        }
        WarpEvent::PeersChanged { .. } | WarpEvent::TrustRingChanged { .. } => {
            // Names of the last-used device may have become known/changed.
            tray::schedule_refresh(app);
            to_value(&event)
        }
        WarpEvent::TransferUpdated { transfer } => {
            on_transfer(app, &state, transfer);
            to_value(&event)
        }
        WarpEvent::OfferReceived { offer } => {
            let was_focused = main_window_focused(app);
            show_main_window(app);
            if !was_focused {
                let title = state.tr("offer_title", &[&offer.peer_name]);
                notify(app, &format!("{title} {}", offer.title), &state.tr("notif_offer_body", &[]));
            }
            to_value(&event)
        }
        WarpEvent::DeviceLinked { device } => {
            if !main_window_focused(app) {
                notify(app, &state.tr("link_success", &[&device.name]), "");
            }
            to_value(&event)
        }
        WarpEvent::ClipboardReceived { from_device_id, from_name, content } => {
            let ok = state.clipboard.write(content).is_ok();
            let title = state.tr("clipboard_received", &[from_name]);
            let body = if ok { state.tr("clipboard_received_body", &[]) } else { state.tr("error_clipboard_read", &[]) };
            notify(app, &title, &body);
            if state.debug_print {
                debug_line(&format!("WARPDROP_CLIPBOARD from={from_name}"));
            }
            // Never ship image bytes through the IPC as a number array.
            let (kind, preview) = match content {
                ClipboardContent::Text { text } => ("text", text.chars().take(200).collect::<String>()),
                ClipboardContent::Url { url } => ("url", url.clone()),
                ClipboardContent::Image { data, .. } => ("image", format!("{}", data.len())),
            };
            json!({
                "type": "clipboardReceived",
                "fromDeviceId": from_device_id,
                "fromName": from_name,
                "kind": kind,
                "preview": preview,
                "written": ok,
            })
        }
        WarpEvent::OfferWithdrawn { .. } | WarpEvent::Error { .. } => to_value(&event),
    };
    let _ = app.emit(EVENT_NAME, payload);
}

fn to_value(event: &WarpEvent) -> Value {
    serde_json::to_value(event).unwrap_or(Value::Null)
}

fn on_transfer(app: &AppHandle, state: &AppState, t: &TransferSnapshot) {
    let previous = state.transfer_states.lock().insert(t.id.clone(), t.state);
    if previous == Some(t.state) || !t.state.is_terminal() {
        return;
    }
    let peer = if t.peer_name.is_empty() { capitalize(&state.tr("other_device", &[])) } else { t.peer_name.clone() };
    let focused = main_window_focused(app);
    match (t.state, t.direction) {
        (TransferState::Completed, TransferDirection::Incoming) => {
            if state.debug_print {
                for p in &t.saved_paths {
                    debug_line(&format!("WARPDROP_SAVED {p}"));
                }
            }
            notify(app, &state.tr("notif_received_title", &[&peer]), &t.title);
        }
        (TransferState::Completed, TransferDirection::Outgoing) => {
            if !focused {
                notify(app, &state.tr("notif_sent_title", &[&peer]), &t.title);
            }
        }
        (TransferState::Failed, _) => {
            if state.debug_print {
                debug_line(&format!("WARPDROP_FAILED {}", t.error.clone().unwrap_or_default()));
            }
            if !focused {
                notify(app, &state.tr("notif_failed_title", &[&peer]), &failure_text(state, t, &peer));
            }
        }
        _ => {}
    }
}

/// Localized failure reason from the engine's stable `error_code`.
fn failure_text(state: &AppState, t: &TransferSnapshot, peer: &str) -> String {
    let detail = t.error.as_deref().unwrap_or("");
    match t.error_code.as_deref() {
        Some("wrong_code") => state.tr("error_wrong_code", &[]),
        Some("code_not_found") => state.tr("error_code_not_found", &[]),
        Some("declined") => capitalize(&state.tr("error_declined", &[peer])),
        Some("rejected") => capitalize(&state.tr("error_rejected", &[peer])),
        Some("unreachable") => state.tr("error_unreachable", &[peer]),
        Some("timeout") => capitalize(&state.tr("error_timeout", &[peer])),
        Some("cancelled") => state.tr("state_cancelled", &[]),
        Some("io") => state.tr("error_disk", &[detail]),
        Some("integrity") => state.tr("error_integrity", &[]),
        _ if detail.is_empty() => state.tr("state_failed", &[]),
        _ => state.tr("error_generic", &[detail]),
    }
}

fn main_window_focused(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .map(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false))
        .unwrap_or(false)
}

pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let mut b = app.notification().builder().title(title);
    if !body.is_empty() {
        b = b.body(body);
    }
    if let Err(e) = b.show() {
        eprintln!("warpdrop: notification failed: {e}");
    }
}

pub fn debug_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
