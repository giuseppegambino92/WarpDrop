//! User preferences persisted as JSON in the app config directory.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LanguagePref {
    System,
    En,
    It,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub device_name: String,
    pub download_dir: String,
    pub language: LanguagePref,
    pub relay_url: Option<String>,
    pub launch_at_login: bool,
    /// Device that received the last file/clipboard send (tray "Send Clipboard").
    pub last_peer_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            device_name: default_device_name(),
            download_dir: String::new(),
            language: LanguagePref::System,
            relay_url: None,
            launch_at_login: false,
            last_peer_id: None,
        }
    }
}

pub fn default_device_name() -> String {
    let name = whoami::devicename();
    let name = name.trim();
    if name.is_empty() {
        "WarpDrop".to_string()
    } else {
        name.to_string()
    }
}

/// Where the app keeps its files. `WARPDROP_PROFILE_DIR` relocates everything
/// (handy for running two instances side by side during development).
#[derive(Debug, Clone)]
pub struct Paths {
    pub settings_file: PathBuf,
    pub state_dir: PathBuf,
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("warpdrop: ignoring unreadable settings {}: {e}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }
}

/// Accepts an empty string (no relay) or a `ws://` / `wss://` URL.
pub fn normalize_relay(input: Option<String>) -> Result<Option<String>, ()> {
    let Some(raw) = input else { return Ok(None) };
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let lower = s.to_ascii_lowercase();
    let rest = lower.strip_prefix("wss://").or_else(|| lower.strip_prefix("ws://")).ok_or(())?;
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return Err(());
    }
    Ok(Some(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_validation() {
        assert_eq!(normalize_relay(None), Ok(None));
        assert_eq!(normalize_relay(Some("  ".into())), Ok(None));
        assert_eq!(normalize_relay(Some("wss://relay.example.com".into())), Ok(Some("wss://relay.example.com".into())));
        assert_eq!(normalize_relay(Some(" ws://10.0.0.2:47900 ".into())), Ok(Some("ws://10.0.0.2:47900".into())));
        assert!(normalize_relay(Some("https://x".into())).is_err());
        assert!(normalize_relay(Some("wss://".into())).is_err());
    }

    #[test]
    fn settings_roundtrip_and_defaults() {
        let s: Settings = serde_json::from_str(r#"{"language":"it","relayUrl":"wss://r"}"#).unwrap();
        assert_eq!(s.language, LanguagePref::It);
        assert_eq!(s.relay_url.as_deref(), Some("wss://r"));
        assert!(!s.device_name.is_empty());
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.language, LanguagePref::It);
    }
}
