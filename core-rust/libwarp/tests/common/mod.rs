#![allow(dead_code)]

use parking_lot::Mutex;
use rand::RngCore;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use warp::*;

#[derive(Default)]
pub struct Collector {
    pub events: Mutex<Vec<WarpEvent>>,
}

impl WarpEventListener for Collector {
    fn on_event(&self, event: WarpEvent) {
        self.events.lock().push(event);
    }
}

pub struct TestNode {
    pub node: Arc<WarpNode>,
    pub events: Arc<Collector>,
    pub dir: tempfile::TempDir,
}

impl TestNode {
    pub fn new(name: &str, tweak: impl FnOnce(&mut NodeConfig)) -> Self {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
            .with_test_writer()
            .try_init();
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = NodeConfig::with_dirs(
            name,
            dir.path().join("state").to_str().unwrap(),
            dir.path().join("Downloads").to_str().unwrap(),
        );
        cfg.enable_mdns = false;
        cfg.enable_beacon = false;
        tweak(&mut cfg);
        let events = Arc::new(Collector::default());
        let node = WarpNode::new(cfg, events.clone()).unwrap();
        TestNode { node, events, dir }
    }

    pub fn downloads(&self) -> PathBuf {
        PathBuf::from(self.node.download_dir())
    }

    /// Simulates platform discovery (Bonjour / NsdManager) of `other` on loopback.
    pub fn see(&self, other: &TestNode) {
        let ad = other.node.advertisement();
        self.node.inject_peer(PeerAdvert {
            txt: ad.txt,
            addresses: vec![format!("127.0.0.1:{}", ad.port)],
        });
    }

    pub fn id(&self) -> String {
        self.node.identity().device_id
    }

    pub fn wait_transfer(&self, id: &str, timeout: Duration) -> TransferSnapshot {
        let start = Instant::now();
        loop {
            if let Some(t) = self.node.transfers().into_iter().find(|t| t.id == id) {
                if t.state.is_terminal() {
                    return t;
                }
            }
            assert!(start.elapsed() < timeout, "transfer {id} did not finish in {timeout:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn wait_incoming_done(&self, timeout: Duration) -> TransferSnapshot {
        let start = Instant::now();
        loop {
            if let Some(t) = self
                .node
                .transfers()
                .into_iter()
                .find(|t| t.direction == TransferDirection::Incoming && t.state.is_terminal())
            {
                return t;
            }
            assert!(start.elapsed() < timeout, "no incoming transfer finished in {timeout:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn find_event<T>(&self, timeout: Duration, f: impl Fn(&WarpEvent) -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            if let Some(v) = self.events.events.lock().iter().find_map(&f) {
                return v;
            }
            assert!(start.elapsed() < timeout, "expected event did not arrive in {timeout:?}");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

pub fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < timeout, "condition not met within {timeout:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn random_file(path: &Path, size: usize) {
    let mut v = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut v);
    std::fs::write(path, v).unwrap();
}

/// A nested tree exercising every path in the engine: multi-chunk random
/// data, exact chunk boundaries, compressible text, already-compressed
/// "media", empty files, empty directories and non-ASCII names.
pub fn make_tree(root: &Path) -> PathBuf {
    let base = root.join("Progetto Vacanze ☀️");
    std::fs::create_dir_all(base.join("src/deep/er/est")).unwrap();
    std::fs::create_dir_all(base.join("empty dir")).unwrap();
    std::fs::create_dir_all(base.join("foto")).unwrap();
    random_file(&base.join("foto/video.mov"), 9 * 1024 * 1024 + 12345);
    random_file(&base.join("foto/exact-4MiB.bin"), 4 * 1024 * 1024);
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0];
    jpeg.extend((0..300_000).map(|i| (i % 7) as u8));
    std::fs::write(base.join("foto/IMG_0001.jpg"), jpeg).unwrap();
    let text: String = (0..60_000).map(|i| format!("line {i}: the quick brown fox jumps over the lazy dog\n")).collect();
    std::fs::write(base.join("src/main.rs"), &text).unwrap();
    std::fs::write(base.join("src/deep/er/est/notes è à.txt"), "ciao mondo\n".repeat(1000)).unwrap();
    std::fs::write(base.join("src/empty.txt"), b"").unwrap();
    std::fs::write(base.join("README.md"), "# WarpDrop test\n").unwrap();
    base
}

/// Relative path → BLAKE3 hex (files) or "dir".
pub fn tree_digest(root: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in walkdir::WalkDir::new(root).min_depth(1) {
        let e = e.unwrap();
        let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
        if e.file_type().is_dir() {
            m.insert(rel, "dir".to_string());
        } else {
            let h = warp::hashing::hash_file(e.path()).unwrap();
            m.insert(rel, hex::encode(h));
        }
    }
    m
}

pub fn item(p: &Path) -> SendItem {
    SendItem { path: p.to_string_lossy().into_owned(), name: None }
}

pub const T: Duration = Duration::from_secs(60);
