//! Live transfer bookkeeping: progress, speed/ETA, pause, cancel, and event
//! emission (throttled so UIs get smooth ~10 Hz updates, never a flood).

use crate::types::{PathKind, TransferDirection, TransferSnapshot, TransferState, WarpEvent, WarpEventListener};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Delivers events to the platform listener on a dedicated thread so a slow
/// UI callback can never stall the network engine.
pub struct EventBus {
    tx: std::sync::mpsc::Sender<WarpEvent>,
}

impl EventBus {
    pub fn new(listener: Arc<dyn WarpEventListener>) -> Arc<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<WarpEvent>();
        std::thread::Builder::new()
            .name("warp-events".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    listener.on_event(ev);
                }
            })
            .expect("spawn event thread");
        Arc::new(EventBus { tx })
    }
    pub fn emit(&self, ev: WarpEvent) {
        let _ = self.tx.send(ev);
    }
}

struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
    smoothed: f64,
}

impl SpeedMeter {
    fn new() -> Self {
        SpeedMeter { samples: VecDeque::new(), smoothed: 0.0 }
    }
    fn sample(&mut self, wire_progress: u64) -> f64 {
        let now = Instant::now();
        self.samples.push_back((now, wire_progress));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > Duration::from_secs(3) {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        let dt = now.duration_since(t0).as_secs_f64();
        if dt > 0.05 {
            let inst = (wire_progress.saturating_sub(b0)) as f64 / dt;
            self.smoothed = if self.smoothed == 0.0 { inst } else { self.smoothed * 0.6 + inst * 0.4 };
        }
        self.smoothed
    }
}

pub struct Transfer {
    pub id: String,
    pub direction: TransferDirection,
    pub peer_id: String,
    pub peer_name: Mutex<String>,
    pub title: Mutex<String>,
    pub file_count: AtomicU64,
    pub total_bytes: AtomicU64,
    /// Raw (uncompressed) bytes accounted for, including resumed bytes.
    pub done_bytes: AtomicU64,
    pub resumed_bytes: AtomicU64,
    pub wire_bytes: AtomicU64,
    state: Mutex<TransferState>,
    path: Mutex<Option<PathKind>>,
    error: Mutex<Option<String>>,
    error_code: Mutex<Option<String>>,
    started_at_ms: u64,
    saved_paths: Mutex<Vec<String>>,
    speed: Mutex<SpeedMeter>,
    last_emit: Mutex<Option<Instant>>,
    pause_tx: watch::Sender<bool>,
    cancel_tx: watch::Sender<bool>,
    pub conn: Mutex<Option<quinn::Connection>>,
    /// Sender side: resumption ticket received with the receiver's Accept.
    pub resume_ticket: Mutex<Option<[u8; 32]>>,
    /// Bumped on every (re)connection so a stale handler cannot finish a
    /// transfer that has already resumed on a newer connection.
    pub generation: AtomicU64,
    bus: Arc<EventBus>,
}

impl Transfer {
    pub fn new(id: String, direction: TransferDirection, peer_id: String, peer_name: String, bus: Arc<EventBus>) -> Arc<Self> {
        Arc::new(Transfer {
            id,
            direction,
            peer_id,
            peer_name: Mutex::new(peer_name),
            title: Mutex::new(String::new()),
            file_count: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            done_bytes: AtomicU64::new(0),
            resumed_bytes: AtomicU64::new(0),
            wire_bytes: AtomicU64::new(0),
            state: Mutex::new(TransferState::Connecting),
            path: Mutex::new(None),
            error: Mutex::new(None),
            error_code: Mutex::new(None),
            started_at_ms: crate::trust::now_ms(),
            saved_paths: Mutex::new(vec![]),
            speed: Mutex::new(SpeedMeter::new()),
            last_emit: Mutex::new(None),
            pause_tx: watch::channel(false).0,
            cancel_tx: watch::channel(false).0,
            conn: Mutex::new(None),
            resume_ticket: Mutex::new(None),
            generation: AtomicU64::new(0),
            bus,
        })
    }

    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn is_generation(&self, g: u64) -> bool {
        self.generation.load(Ordering::Acquire) == g
    }

    pub fn set_meta(&self, title: String, files: u64, total: u64) {
        *self.title.lock() = title;
        self.file_count.store(files, Ordering::Relaxed);
        self.total_bytes.store(total, Ordering::Relaxed);
    }

    pub fn state(&self) -> TransferState {
        *self.state.lock()
    }

    pub fn set_state(&self, s: TransferState) {
        {
            let mut cur = self.state.lock();
            if cur.is_terminal() {
                return;
            }
            *cur = s;
        }
        self.emit(true);
    }

    pub fn set_path(&self, p: PathKind) {
        *self.path.lock() = Some(p);
        self.emit(true);
    }

    pub fn finish_ok(&self, saved: Vec<String>) {
        *self.saved_paths.lock() = saved;
        self.set_state(TransferState::Completed);
    }

    pub fn finish_err(&self, err: String, code: &str) {
        if self.is_cancelled() {
            self.set_state(TransferState::Cancelled);
        } else {
            *self.error.lock() = Some(err);
            *self.error_code.lock() = Some(code.to_string());
            self.set_state(TransferState::Failed);
        }
    }

    pub fn add_progress(&self, raw: u64, wire: u64) {
        self.done_bytes.fetch_add(raw, Ordering::Relaxed);
        self.wire_bytes.fetch_add(wire, Ordering::Relaxed);
        self.emit(false);
    }

    pub fn add_resumed(&self, raw: u64) {
        self.done_bytes.fetch_add(raw, Ordering::Relaxed);
        self.resumed_bytes.fetch_add(raw, Ordering::Relaxed);
        self.emit(false);
    }

    /// Discards progress counted on a connection that then failed, before a
    /// resume re-establishes the authoritative counts.
    pub fn reset_progress(&self) {
        self.done_bytes.store(0, Ordering::Relaxed);
        self.resumed_bytes.store(0, Ordering::Relaxed);
    }

    pub fn pause(&self, paused: bool) {
        self.pause_tx.send_replace(paused);
        if !self.state().is_terminal() {
            self.set_state(if paused { TransferState::Paused } else { TransferState::Transferring });
        }
    }

    pub fn is_paused(&self) -> bool {
        *self.pause_tx.borrow()
    }

    /// Waits while paused; returns false if cancelled.
    pub async fn wait_unpaused(&self) -> bool {
        let mut rx = self.pause_tx.subscribe();
        loop {
            if self.is_cancelled() {
                return false;
            }
            if !*rx.borrow_and_update() {
                return true;
            }
            let mut c = self.cancel_tx.subscribe();
            tokio::select! {
                r = rx.changed() => { if r.is_err() { return false; } }
                _ = c.changed() => {}
            }
        }
    }

    pub fn cancel(&self) {
        self.cancel_tx.send_replace(true);
        if let Some(c) = self.conn.lock().take() {
            c.close(quinn::VarInt::from_u32(crate::protocol::close::CANCELLED), b"cancelled");
        }
        self.set_state(TransferState::Cancelled);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancel_tx.borrow()
    }

    pub async fn cancelled(&self) {
        let mut rx = self.cancel_tx.subscribe();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    pub fn snapshot(&self) -> TransferSnapshot {
        let state = self.state();
        let total = self.total_bytes.load(Ordering::Relaxed);
        let done = self.done_bytes.load(Ordering::Relaxed).min(total);
        let wire = self.wire_bytes.load(Ordering::Relaxed);
        let speed = if state == TransferState::Transferring {
            // Speed of *useful* data: raw bytes per second excluding resumed bytes.
            let useful = done.saturating_sub(self.resumed_bytes.load(Ordering::Relaxed));
            self.speed.lock().sample(useful)
        } else {
            0.0
        };
        let eta = if speed > 1.0 && total > done { Some(((total - done) as f64 / speed).ceil() as u64) } else { None };
        TransferSnapshot {
            id: self.id.clone(),
            direction: self.direction,
            peer_id: self.peer_id.clone(),
            peer_name: self.peer_name.lock().clone(),
            title: self.title.lock().clone(),
            file_count: self.file_count.load(Ordering::Relaxed),
            total_bytes: total,
            done_bytes: done,
            resumed_bytes: self.resumed_bytes.load(Ordering::Relaxed),
            wire_bytes: wire,
            speed_bps: speed as u64,
            eta_secs: eta,
            path: *self.path.lock(),
            state,
            error: self.error.lock().clone(),
            error_code: self.error_code.lock().clone(),
            started_at_ms: self.started_at_ms,
            saved_paths: self.saved_paths.lock().clone(),
        }
    }

    pub fn emit(&self, force: bool) {
        {
            let mut last = self.last_emit.lock();
            let now = Instant::now();
            if !force {
                if let Some(t) = *last {
                    if now.duration_since(t) < Duration::from_millis(100) {
                        return;
                    }
                }
            }
            *last = Some(now);
        }
        self.bus.emit(WarpEvent::TransferUpdated { transfer: self.snapshot() });
    }
}
