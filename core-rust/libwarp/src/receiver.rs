//! Receiving side: staging, verification, resume, and atomic hand-off.
//!
//! Data lands in `<download_dir>/.warpdrop-partial/<key>/…` where `key` is
//! derived from the sender's identity and the manifest. Every chunk is
//! decompressed, hashed and compared with the sender's declared hash before it
//! is written. A small append-only log records verified chunks; on resume the
//! log is only a hint — each listed chunk is re-hashed from disk before it is
//! offered back to the sender as "already have".
//!
//! When every file's merged BLAKE3 root matches the sender's, the top-level
//! items are renamed into the download directory (same filesystem → atomic).

use crate::compress::{decompress, ENC_RAW, ENC_ZSTD};
use crate::hashing::{chunk_count, chunk_hash, chunk_len, file_root, CHUNK_SIZE};
use crate::manifest::{safe_join, validate, EntryKind, Manifest};
use crate::protocol::{close, read_msg, write_msg, FrameHeader, HaveChunk, Msg, FRAME_HEADER_LEN};
use crate::sender::read_exact_at;
use crate::transfer::Transfer;
use crate::types::TransferState;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

pub const PARTIAL_DIR: &str = ".warpdrop-partial";

/// Test hook ("pull the cable"): the receiver drops the connection once this
/// many more chunks have been verified and written. 0 = disabled. Fires once
/// per arming.
pub static DEBUG_DROP_AFTER_CHUNKS: AtomicU64 = AtomicU64::new(0);

/// Decrements the hook counter (never below 0); true when it just hit 0.
fn debug_drop_tick() -> bool {
    let mut cur = DEBUG_DROP_AFTER_CHUNKS.load(Ordering::Acquire);
    while cur > 0 {
        match DEBUG_DROP_AFTER_CHUNKS.compare_exchange_weak(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return cur == 1,
            Err(now) => cur = now,
        }
    }
    false
}
const LOG_RECORD: usize = 4 + 4 + 32;

pub fn staging_key(peer_key: &[u8; 32], m: &Manifest) -> String {
    let mut h = blake3::Hasher::new();
    h.update(peer_key);
    h.update(&m.fingerprint());
    hex::encode(&h.finalize().as_bytes()[..16])
}

struct State {
    manifest: Manifest,
    stage: PathBuf,
    hashes: Mutex<Vec<Vec<Option<[u8; 32]>>>>,
    remaining: AtomicU64,
    notify: Notify,
    failure: Mutex<Option<String>>,
}

fn write_at(f: &std::fs::File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.write_all_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            done += f.seek_write(&buf[done..], offset + done as u64)?;
        }
        Ok(())
    }
}

/// Prepares the staging area and figures out which chunks are already present.
fn prepare(stage: &Path, m: &Manifest) -> Result<Vec<HaveChunk>> {
    std::fs::create_dir_all(stage)?;
    for e in &m.entries {
        let p = safe_join(stage, &e.path)?;
        match e.kind {
            EntryKind::Dir => std::fs::create_dir_all(&p)?,
            EntryKind::File { .. } => {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }
    }
    let mut have = vec![];
    let log = stage.join(".chunks");
    let Ok(bytes) = std::fs::read(&log) else { return Ok(have) };
    let mut seen = HashSet::new();
    for rec in bytes.as_chunks::<LOG_RECORD>().0 {
        let entry = u32::from_be_bytes(rec[0..4].try_into()?);
        let chunk = u32::from_be_bytes(rec[4..8].try_into()?);
        let Some(e) = m.entries.get(entry as usize) else { continue };
        let EntryKind::File { size, .. } = e.kind else { continue };
        let n = chunk_count(size);
        if chunk as u64 >= n || !seen.insert((entry, chunk)) {
            continue;
        }
        // Trust the disk, not the log.
        let path = safe_join(stage, &e.path)?;
        let Ok(f) = std::fs::File::open(&path) else { continue };
        let len = chunk_len(size, chunk as u64) as usize;
        let mut buf = vec![0u8; len];
        if read_exact_at(&f, &mut buf, chunk as u64 * CHUNK_SIZE).is_err() {
            continue;
        }
        let hash = chunk_hash(&buf, chunk as u64, n);
        if rec[8..40] == hash {
            have.push(HaveChunk { entry, chunk, hash });
        }
    }
    Ok(have)
}

/// What the receiver needs to know about one accepted transfer.
pub struct Job {
    pub manifest: Manifest,
    /// Staging directory (`<download_dir>/.warpdrop-partial/<key>`).
    pub stage: PathBuf,
    pub download_dir: PathBuf,
    /// Resumption ticket handed to the sender with `Accept`.
    pub ticket: [u8; 32],
}

pub struct Received {
    pub saved_paths: Vec<String>,
}

/// Runs after the offer has been accepted. `t` may be a transfer object that
/// survived an earlier, interrupted connection.
pub async fn receive(
    conn: &quinn::Connection,
    ctrl_send: &mut quinn::SendStream,
    ctrl_recv: &mut quinn::RecvStream,
    job: Job,
    t: Arc<Transfer>,
) -> Result<Received> {
    let Job { manifest, stage, download_dir, ticket } = job;
    validate(&manifest)?;
    let have = {
        let (stage, m) = (stage.clone(), manifest.clone());
        tokio::task::spawn_blocking(move || prepare(&stage, &m)).await??
    };

    let mut hashes: Vec<Vec<Option<[u8; 32]>>> = manifest.entries.iter().map(|e| vec![None; e.chunks() as usize]).collect();
    let total_chunks: u64 = manifest.entries.iter().map(|e| e.chunks()).sum();
    t.reset_progress();
    for h in &have {
        hashes[h.entry as usize][h.chunk as usize] = Some(h.hash);
        let e = &manifest.entries[h.entry as usize];
        t.add_resumed(chunk_len(e.size(), h.chunk as u64));
    }
    let state = Arc::new(State {
        stage: stage.clone(),
        remaining: AtomicU64::new(total_chunks - have.len() as u64),
        hashes: Mutex::new(hashes),
        manifest,
        notify: Notify::new(),
        failure: Mutex::new(None),
    });

    write_msg(ctrl_send, &Msg::Accept { have, ticket }).await?;
    t.set_state(TransferState::Transferring);

    let acceptor = {
        let (conn, state, t) = (conn.clone(), state.clone(), t.clone());
        tokio::spawn(async move {
            let mut handlers = tokio::task::JoinSet::new();
            while let Ok(stream) = conn.accept_uni().await {
                let (state, t, conn) = (state.clone(), t.clone(), conn.clone());
                handlers.spawn(async move {
                    if let Err(e) = handle_stream(stream, &state, &t, &conn).await {
                        if e.downcast_ref::<quinn::ReadExactError>().is_some() || e.downcast_ref::<quinn::ReadError>().is_some() {
                            // Connection lost: the control loop notices and
                            // the sender reconnects and resumes.
                            return;
                        }
                        tracing::warn!("data stream failed: {e:#}");
                        *state.failure.lock() = Some(format!("{e:#}"));
                        state.notify.notify_waiters();
                        conn.close(quinn::VarInt::from_u32(close::PROTOCOL), b"bad data");
                    }
                });
            }
            while handlers.join_next().await.is_some() {}
        })
    };

    let result = async {
        let roots = match read_msg(ctrl_recv).await? {
            Msg::Done { roots } => roots,
            other => bail!("unexpected message: {other:?}"),
        };
        // Data streams may still be draining.
        loop {
            let notified = state.notify.notified();
            if let Some(f) = state.failure.lock().clone() {
                bail!(f);
            }
            if state.remaining.load(Ordering::Acquire) == 0 {
                break;
            }
            tokio::select! {
                _ = notified => {}
                e = conn.closed() => bail!("connection lost: {e}"),
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
        tracing::debug!("all chunks stored, finalizing");
        t.set_state(TransferState::Verifying);
        let st = state.clone();
        let saved = tokio::task::spawn_blocking(move || finalize(&st, &roots, &download_dir)).await?;
        match saved {
            Ok(paths) => {
                write_msg(ctrl_send, &Msg::Result { ok: true, error: None }).await?;
                Ok(Received { saved_paths: paths })
            }
            Err(e) => {
                let _ = write_msg(ctrl_send, &Msg::Result { ok: false, error: Some(format!("{e:#}")) }).await;
                // Content mismatch: never resume from this staging area.
                let _ = std::fs::remove_dir_all(&state.stage);
                Err(e)
            }
        }
    }
    .await;
    acceptor.abort();
    result
}

async fn handle_stream(mut s: quinn::RecvStream, state: &Arc<State>, t: &Arc<Transfer>, conn: &quinn::Connection) -> Result<()> {
    let mut hb = [0u8; FRAME_HEADER_LEN];
    loop {
        match s.read_exact(&mut hb).await {
            Ok(()) => {}
            Err(quinn::ReadExactError::FinishedEarly(0)) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        let h = FrameHeader::decode(&hb);
        let (entry, size) = {
            let e = state
                .manifest
                .entries
                .get(h.entry as usize)
                .ok_or_else(|| anyhow!("frame for unknown entry {}", h.entry))?;
            (e.clone(), e.size())
        };
        let n = chunk_count(size);
        if h.chunk as u64 >= n {
            bail!("chunk index out of range");
        }
        let expect_len = chunk_len(size, h.chunk as u64);
        if h.raw_len as u64 != expect_len || h.payload_len as u64 > CHUNK_SIZE + 1024 * 64 {
            bail!("bad chunk length");
        }
        if h.enc == ENC_RAW && h.payload_len != h.raw_len {
            bail!("raw chunk length mismatch");
        }
        let mut payload = vec![0u8; h.payload_len as usize];
        s.read_exact(&mut payload).await?;
        let wire = (FRAME_HEADER_LEN + payload.len()) as u64;

        let st = state.clone();
        let path = safe_join(&state.stage, &entry.path)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let data = match h.enc {
                ENC_RAW => payload,
                ENC_ZSTD => decompress(&payload, h.raw_len as usize)?,
                x => bail!("unknown encoding {x}"),
            };
            let hash = chunk_hash(&data, h.chunk as u64, n);
            if hash != h.hash {
                bail!("chunk {}/{} of {} failed verification", h.chunk, n, entry.path);
            }
            if st.hashes.lock()[h.entry as usize][h.chunk as usize].is_some() {
                return Ok(()); // duplicate (e.g. sent twice across a reconnect)
            }
            let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&path)?;
            write_at(&f, &data, h.chunk as u64 * CHUNK_SIZE)?;
            let mut rec = Vec::with_capacity(LOG_RECORD);
            rec.extend_from_slice(&h.entry.to_be_bytes());
            rec.extend_from_slice(&h.chunk.to_be_bytes());
            rec.extend_from_slice(&hash);
            std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(st.stage.join(".chunks"))?
                .write_all(&rec)?;
            let mut hs = st.hashes.lock();
            if hs[h.entry as usize][h.chunk as usize].is_none() {
                hs[h.entry as usize][h.chunk as usize] = Some(hash);
                drop(hs);
                st.remaining.fetch_sub(1, Ordering::AcqRel);
            }
            Ok(())
        })
        .await??;
        t.add_progress(expect_len, wire);
        state.notify.notify_waiters();
        if debug_drop_tick() {
            conn.close(quinn::VarInt::from_u32(99), b"simulated cable pull");
        }
    }
}

fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded search")
}

fn finalize(state: &State, roots: &[[u8; 32]], download_dir: &Path) -> Result<Vec<String>> {
    let m = &state.manifest;
    if roots.len() != m.entries.len() {
        bail!("root count mismatch");
    }
    let hashes = state.hashes.lock().clone();
    for (i, e) in m.entries.iter().enumerate() {
        let EntryKind::File { size, mtime_ms, executable } = e.kind else { continue };
        let hs: Vec<[u8; 32]> = hashes[i]
            .iter()
            .map(|h| h.ok_or_else(|| anyhow!("missing chunk in {}", e.path)))
            .collect::<Result<_>>()?;
        let root = file_root(&hs);
        if root != roots[i] {
            bail!("BLAKE3 mismatch for {}", e.path);
        }
        let p = safe_join(&state.stage, &e.path)?;
        let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&p)?;
        f.set_len(size)?;
        if mtime_ms > 0 {
            let _ = f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(mtime_ms));
        }
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(not(unix))]
        let _ = executable;
    }
    std::fs::create_dir_all(download_dir).context("creating download directory")?;
    let mut saved = vec![];
    for top in m.top_level() {
        let from = safe_join(&state.stage, top)?;
        let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let to = unique_dest(download_dir, &name);
        std::fs::rename(&from, &to).with_context(|| format!("moving {} into place", to.display()))?;
        saved.push(to.to_string_lossy().into_owned());
    }
    let _ = std::fs::remove_dir_all(&state.stage);
    Ok(saved)
}

/// Removes staging areas untouched for a week.
pub fn cleanup_stale(download_dir: &Path) {
    let root = download_dir.join(PARTIAL_DIR);
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| d > Duration::from_secs(7 * 24 * 3600))
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}
