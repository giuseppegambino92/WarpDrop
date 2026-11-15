//! Personal Trust Ring: pinned Ed25519 public keys of the user's own devices.

use crate::identity::device_id_from_key;
use crate::types::{DeviceKind, TrustOrigin};
use anyhow::Result;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEntry {
    pub public_key: [u8; 32],
    pub name: String,
    pub kind: DeviceKind,
    pub origin: TrustOrigin,
    pub added_at_ms: u64,
}

impl TrustEntry {
    pub fn device_id(&self) -> String {
        device_id_from_key(&self.public_key)
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub struct TrustStore {
    path: PathBuf,
    entries: RwLock<Vec<TrustEntry>>,
}

impl TrustStore {
    pub fn open(path: PathBuf) -> Self {
        let entries = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<TrustEntry>>(&b).ok())
            .unwrap_or_default();
        TrustStore { path, entries: RwLock::new(entries) }
    }

    fn save(&self, entries: &[TrustEntry]) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)?;
        std::fs::rename(tmp, &self.path)?;
        Ok(())
    }

    pub fn contains(&self, key: &[u8; 32]) -> bool {
        self.entries.read().iter().any(|e| &e.public_key == key)
    }

    pub fn get(&self, key: &[u8; 32]) -> Option<TrustEntry> {
        self.entries.read().iter().find(|e| &e.public_key == key).cloned()
    }

    pub fn by_device_id(&self, id: &str) -> Option<TrustEntry> {
        self.entries.read().iter().find(|e| e.device_id() == id).cloned()
    }

    pub fn list(&self) -> Vec<TrustEntry> {
        self.entries.read().clone()
    }

    /// Adds or refreshes an entry. Returns true when the ring changed.
    /// A stronger origin (iCloud/Linked) is never downgraded to Ring.
    pub fn upsert(&self, key: [u8; 32], name: &str, kind: DeviceKind, origin: TrustOrigin) -> Result<bool> {
        let mut entries = self.entries.write();
        if let Some(e) = entries.iter_mut().find(|e| e.public_key == key) {
            let mut changed = false;
            if !name.is_empty() && e.name != name {
                e.name = name.to_string();
                changed = true;
            }
            if kind != DeviceKind::Unknown && e.kind != kind {
                e.kind = kind;
                changed = true;
            }
            if e.origin == TrustOrigin::Ring && origin != TrustOrigin::Ring {
                e.origin = origin;
                changed = true;
            }
            if changed {
                self.save(&entries)?;
            }
            return Ok(changed);
        }
        entries.push(TrustEntry { public_key: key, name: name.to_string(), kind, origin, added_at_ms: now_ms() });
        self.save(&entries)?;
        Ok(true)
    }

    pub fn remove(&self, device_id: &str) -> Result<bool> {
        let mut entries = self.entries.write();
        let before = entries.len();
        entries.retain(|e| e.device_id() != device_id);
        let changed = before != entries.len();
        if changed {
            self.save(&entries)?;
        }
        Ok(changed)
    }
}
