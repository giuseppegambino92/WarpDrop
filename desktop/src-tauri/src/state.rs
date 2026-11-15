//! Shared application state held by Tauri.

use crate::clipboard::ClipboardService;
use crate::i18n::{self, Lang, Strings};
use crate::settings::{Paths, Settings};
use crate::tray::TrayHandles;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use warp::{CodeInfo, TransferState, WarpNode};

pub struct AppState {
    pub node: OnceLock<Arc<WarpNode>>,
    /// Why the engine could not start (shown instead of the UI).
    pub node_error: Mutex<Option<String>>,
    pub settings: Mutex<Settings>,
    pub paths: Paths,
    pub lang: RwLock<Lang>,
    pub strings: Strings,
    pub tray: Mutex<Option<TrayHandles>>,
    pub clipboard: ClipboardService,
    /// "vibrancy" | "mica" | "acrylic" | "none"
    pub material: Mutex<&'static str>,
    pub code: Mutex<Option<CodeInfo>>,
    /// Last state seen per transfer, to fire notifications on transitions only.
    pub transfer_states: Mutex<HashMap<String, TransferState>>,
    pub debug_print: bool,
}

impl AppState {
    pub fn new(settings: Settings, paths: Paths, debug_print: bool) -> AppState {
        let lang = i18n::resolve(&settings.language);
        AppState {
            node: OnceLock::new(),
            node_error: Mutex::new(None),
            settings: Mutex::new(settings),
            paths,
            lang: RwLock::new(lang),
            strings: Strings::load(),
            tray: Mutex::new(None),
            clipboard: ClipboardService::start(),
            material: Mutex::new("none"),
            code: Mutex::new(None),
            transfer_states: Mutex::new(HashMap::new()),
            debug_print,
        }
    }

    pub fn tr(&self, key: &str, args: &[&str]) -> String {
        self.strings.tr(*self.lang.read(), key, args)
    }

    pub fn node(&self) -> Option<Arc<WarpNode>> {
        self.node.get().cloned()
    }

    pub fn save_settings(&self) {
        let s = self.settings.lock().clone();
        if let Err(e) = s.save(&self.paths.settings_file) {
            eprintln!("warpdrop: could not save settings: {e}");
        }
    }
}
