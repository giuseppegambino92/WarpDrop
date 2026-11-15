//! System tray "droplet": code display, quick clipboard send, open, quit.

use crate::state::AppState;
use crate::{actions, show_main_window, toggle_main_window};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

pub struct TrayHandles {
    pub _icon: TrayIcon<Wry>,
    code: MenuItem<Wry>,
    send_clipboard: MenuItem<Wry>,
    open: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

const ID_CODE: &str = "tray-code";
const ID_CLIP: &str = "tray-send-clipboard";
const ID_OPEN: &str = "tray-open";
const ID_QUIT: &str = "tray-quit";

pub fn create(app: &AppHandle) -> tauri::Result<TrayHandles> {
    let code = MenuItem::with_id(app, ID_CODE, "…", true, None::<&str>)?;
    let send_clipboard = MenuItem::with_id(app, ID_CLIP, "…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, ID_OPEN, "…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "…", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&code, &PredefinedMenuItem::separator(app)?, &send_clipboard, &open, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    let (size, rgba, template) = tray_glyph();
    let icon = TrayIconBuilder::with_id("warpdrop")
        .icon(Image::new_owned(rgba, size, size))
        .icon_as_template(template)
        .tooltip("WarpDrop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_CODE => actions::copy_current_code(app),
            ID_CLIP => actions::send_clipboard_to_last(app),
            ID_OPEN => show_main_window(app),
            ID_QUIT => actions::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(TrayHandles { _icon: icon, code, send_clipboard, open, quit })
}

/// Queues a [`refresh`] on the main thread. Menu updates hop to the main
/// thread anyway; doing the whole refresh there keeps the tray lock from ever
/// being held while waiting on it.
pub fn schedule_refresh(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || refresh(&handle));
}

/// Re-applies localized labels, the current code and the last-used device.
pub fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let guard = state.tray.lock();
    let Some(t) = guard.as_ref() else { return };
    let code = state.code.lock().as_ref().map(|c| c.display.clone()).unwrap_or_else(|| "–".into());
    let _ = t.code.set_text(state.tr("tray_your_code", &[&code]));
    let last = actions::last_peer_name(app);
    match last {
        Some(name) => {
            let _ = t.send_clipboard.set_text(state.tr("tray_send_clipboard_to", &[&name]));
            let _ = t.send_clipboard.set_enabled(true);
        }
        None => {
            let _ = t.send_clipboard.set_text(state.tr("send_clipboard", &[]));
            let _ = t.send_clipboard.set_enabled(false);
        }
    }
    let _ = t.open.set_text(state.tr("menu_open", &[]));
    let _ = t.quit.set_text(state.tr("menu_quit", &[]));
}

/// The droplet as RGBA. macOS gets a black template image (the system tints
/// it); Windows and Linux get a monochrome glyph that contrasts with the
/// taskbar / panel.
fn tray_glyph() -> (u32, Vec<u8>, bool) {
    #[cfg(target_os = "macos")]
    {
        (36, droplet_rgba(36, [0, 0, 0]), true)
    }
    #[cfg(target_os = "windows")]
    {
        let color = if windows_taskbar_is_light() { [28, 28, 30] } else { [255, 255, 255] };
        (32, droplet_rgba(32, color), false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // GNOME's top bar and most panels are dark.
        (32, droplet_rgba(32, [255, 255, 255]), false)
    }
}

#[cfg(target_os = "windows")]
fn windows_taskbar_is_light() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "SystemUsesLightTheme",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("0x1"))
        .unwrap_or(false)
}

/// Anti-aliased droplet: a circle plus the cone tangent to it from the tip.
pub fn droplet_rgba(size: u32, rgb: [u8; 3]) -> Vec<u8> {
    const SS: u32 = 4; // supersampling per axis
    let (cx, cy, r) = (0.5_f64, 0.615_f64, 0.285_f64);
    let tip_y = 0.075_f64;
    let d = cy - tip_y;
    let half_angle = (r / d).asin();
    let tan_a = half_angle.tan();
    // Y of the tangent points: beyond them the circle alone defines the shape.
    let tangent_y = tip_y + (d * d - r * r).sqrt() * half_angle.cos();
    let inside = |x: f64, y: f64| {
        let (dx, dy) = (x - cx, y - cy);
        if dx * dx + dy * dy <= r * r {
            return true;
        }
        y >= tip_y && y <= tangent_y && dx.abs() <= (y - tip_y) * tan_a
    };
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    let n = size as f64;
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f64 + (sx as f64 + 0.5) / SS as f64) / n;
                    let y = (py as f64 + (sy as f64 + 0.5) / SS as f64) / n;
                    if inside(x, y) {
                        hits += 1;
                    }
                }
            }
            let a = (hits * 255 / (SS * SS)) as u8;
            out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::droplet_rgba;

    #[test]
    fn droplet_shape() {
        let n = 32usize;
        let px = droplet_rgba(n as u32, [0, 0, 0]);
        assert_eq!(px.len(), n * n * 4);
        let alpha = |x: usize, y: usize| px[(y * n + x) * 4 + 3];
        assert_eq!(alpha(16, 20), 255, "body is solid");
        assert_eq!(alpha(0, 0), 0, "corner is clear");
        assert_eq!(alpha(4, 4), 0, "shoulder is clear");
        assert!(alpha(16, 4) > 0, "tip is drawn");
    }
}
