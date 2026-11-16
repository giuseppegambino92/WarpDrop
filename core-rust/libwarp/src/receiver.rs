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
//! items are renamed into the download directory (same filesystem → atomic),
//! never over an existing name, and all or nothing.

use crate::compress::{decompress, ENC_RAW, ENC_ZSTD};
use crate::hashing::{chunk_count, chunk_hash, chunk_len, file_root, CHUNK_SIZE};
use crate::fsx;
use crate::manifest::{local_paths, validate, EntryKind, Manifest};
use crate::protocol::{close, read_msg, write_msg, FrameHeader, HaveChunk, Msg, FRAME_HEADER_LEN};
use crate::sender::read_exact_at;
use crate::transfer::Transfer;
use crate::types::TransferState;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;

pub const PARTIAL_DIR: &str = ".warpdrop-partial";

/// The verified data could not be moved into the download folder. Final for
/// this attempt (both sides report it; nothing to reconnect for), but the
/// data stays staged, so sending the same items again completes it.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SaveError(pub String);

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
/// Test hook: the next hand-off fails to move its second top-level item and
/// also fails to roll back the first (a disk error at the worst moment).
pub static DEBUG_PLACE_FAULT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const LOG_RECORD: usize = 4 + 4 + 32;

/// Write-ahead journal of the hand-off (`{"<top>": "<destination>"}`), kept in
/// the staging area until the transfer completes.
///
/// Before each rename attempt the exact destination is written and synced to
/// disk; a failure there aborts the hand-off before anything moves. So for
/// every item that ever left staging the journal already names where it went.
/// On the next attempt an entry counts only if its item is no longer in
/// staging (otherwise the rename never happened, e.g. a crash right after the
/// journal write) and is still intact at the destination; such items are
/// neither re-sent nor saved a second time.
const PLACED_LOG: &str = ".placed";

/// Written (durably) once every item is saved. The staging area then holds
/// only the journal and the chunk log, and is kept for `COMPLETE_TTL` so that
/// a retry of the very same transfer (e.g. our "done" never reached the
/// sender) is answered from it instead of saving everything again. A crash
/// while cleaning up therefore cannot cause duplicates either.
const COMPLETE_MARK: &str = ".complete";
const COMPLETE_TTL: Duration = Duration::from_secs(30 * 60);

fn completed_long_ago(stage: &Path) -> bool {
    std::fs::metadata(stage.join(COMPLETE_MARK))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > COMPLETE_TTL)
}

/// Top-level item (relative path, one component) → where it was saved.
type Placed = HashMap<PathBuf, PathBuf>;

/// The journal as read back: entries that pass `read_placed`'s checks, and
/// whether anything was dropped (so it is rewritten).
struct Journal {
    placed: Placed,
    dropped: bool,
}

/// On-disk form of the journal. Strict: unknown fields, another version or
/// an item named twice make the whole file corrupt.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalFile {
    version: u32,
    placed: Vec<JournalEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalEntry {
    item: String,
    to: String,
}

const JOURNAL_VERSION: u32 = 1;

/// The largest journal this transfer can produce: every top-level item
/// saved under the longest name `move_unique` picks, with worst-case JSON
/// escaping (6 bytes per byte). Anything larger is corrupt and is never
/// read into memory.
fn journal_limit(download_dir: &Path, tops: &HashSet<&Path>) -> u64 {
    let dl = download_dir.as_os_str().len() as u64;
    let per_item = |top: &Path| {
        let n = top.as_os_str().len() as u64;
        6 * (n + dl + 1 + n + " (9999)".len() as u64) + 32
    };
    tops.iter().map(|t| per_item(t)).sum::<u64>() + 64
}

/// Reads the journal, trusting nothing in it: it is read only up to
/// `journal_limit`, parsed strictly (`JournalFile`), and an entry is kept
/// only if it names a top-level item of this manifest and a destination that
/// is a direct child of `download_dir` (the only places `place` ever
/// writes). A missing journal is the normal "nothing saved yet". An
/// unreadable, oversized or malformed one is quarantined as
/// `.placed.corrupt` (renamed, never deleted) and reported; the outcome is
/// safe: an item the journal no longer vouches for is received again and
/// saved under a free name, never written into whatever is at the old one.
/// If it cannot even be renamed aside, the staging area is not usable and
/// the transfer fails up front, before any data is exchanged, with an error
/// naming it.
fn read_placed(stage: &Path, download_dir: &Path, tops: &HashSet<&Path>) -> std::result::Result<Journal, SaveError> {
    use std::io::Read;
    let path = stage.join(PLACED_LOG);
    let limit = journal_limit(download_dir, tops);
    let parsed = match std::fs::File::open(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Journal { placed: Placed::new(), dropped: false }),
        Err(e) => Err(e.to_string()),
        Ok(f) => {
            let mut bytes = vec![];
            match f.take(limit + 1).read_to_end(&mut bytes) {
                Err(e) => Err(e.to_string()),
                Ok(_) if bytes.len() as u64 > limit => Err(format!("larger than {limit} bytes, the most this transfer can produce")),
                Ok(_) => serde_json::from_slice::<JournalFile>(&bytes).map_err(|e| e.to_string()).and_then(|j| {
                    if j.version != JOURNAL_VERSION {
                        return Err(format!("unknown version {}", j.version));
                    }
                    let mut seen = HashSet::new();
                    match j.placed.iter().find(|e| !seen.insert(e.item.as_str())) {
                        Some(dup) => Err(format!("{} is listed twice", dup.item)),
                        None => Ok(j.placed),
                    }
                }),
            }
        }
    };
    let raw = match parsed {
        Ok(raw) => raw,
        Err(e) => {
            let aside = stage.join(".placed.corrupt");
            return match std::fs::rename(&path, &aside) {
                Ok(()) => {
                    tracing::warn!(
                        "hand-off journal {} is corrupt ({e}); quarantined as {}; its items will be received again",
                        path.display(),
                        aside.display()
                    );
                    Ok(Journal { placed: Placed::new(), dropped: true })
                }
                Err(e2) => Err(SaveError(format!(
                    "the hand-off journal {} is corrupt ({e}) and cannot be quarantined ({e2}); check the folder {} and its disk",
                    path.display(),
                    stage.display()
                ))),
            };
        }
    };
    let n = raw.len();
    let placed: Placed = raw
        .into_iter()
        .map(|e| (PathBuf::from(e.item), PathBuf::from(e.to)))
        .filter(|(top, dst)| {
            let ok = tops.contains(top.as_path())
                && dst.parent() == Some(download_dir)
                && dst.file_name().is_some_and(|n| Path::new(n).components().eq([std::path::Component::Normal(n)]));
            if !ok {
                tracing::warn!("ignoring hand-off journal entry {} → {}", top.display(), dst.display());
            }
            ok
        })
        .collect();
    Ok(Journal { dropped: placed.len() != n, placed })
}

fn write_placed(stage: &Path, placed: &Placed) -> std::io::Result<()> {
    let mut entries: Vec<JournalEntry> = placed
        .iter()
        .map(|(k, v)| JournalEntry { item: k.to_string_lossy().into_owned(), to: v.to_string_lossy().into_owned() })
        .collect();
    entries.sort_by(|a, b| a.item.cmp(&b.item));
    let file = JournalFile { version: JOURNAL_VERSION, placed: entries };
    fsx::write_durable(&stage.join(PLACED_LOG), &serde_json::to_vec(&file).map_err(std::io::Error::other)?)
}

fn log_record(entry: u32, chunk: u32, hash: &[u8; 32]) -> [u8; LOG_RECORD] {
    let mut rec = [0u8; LOG_RECORD];
    rec[0..4].copy_from_slice(&entry.to_be_bytes());
    rec[4..8].copy_from_slice(&chunk.to_be_bytes());
    rec[8..].copy_from_slice(hash);
    rec
}

/// Rewrites the chunk log with only the records that verified. Best effort:
/// the log is a hint, re-checked against the disk on every resume.
fn compact_log(log: &Path, have: &[HaveChunk]) {
    let mut bytes = Vec::with_capacity(have.len() * LOG_RECORD);
    for h in have {
        bytes.extend_from_slice(&log_record(h.entry, h.chunk, &h.hash));
    }
    if let Err(e) = fsx::write_durable(log, &bytes) {
        tracing::warn!("could not compact {}: {e}", log.display());
    }
}

/// Splits a local relative path into its top-level item and the rest.
fn split_top(rel: &Path) -> (PathBuf, PathBuf) {
    let mut c = rel.components();
    let top = c.next().map(|t| PathBuf::from(t.as_os_str())).unwrap_or_default();
    (top, c.as_path().to_path_buf())
}

/// Where the data of an entry with local path `rel` currently is: in staging,
/// or inside an item that was already saved.
fn data_path(stage: &Path, placed: &Placed, rel: &Path) -> PathBuf {
    let (top, rest) = split_top(rel);
    match placed.get(&top) {
        Some(dst) if rest.as_os_str().is_empty() => dst.clone(),
        Some(dst) => dst.join(rest),
        None => stage.join(rel),
    }
}

pub fn staging_key(peer_key: &[u8; 32], m: &Manifest) -> String {
    let mut h = blake3::Hasher::new();
    h.update(peer_key);
    h.update(&m.fingerprint());
    hex::encode(&h.finalize().as_bytes()[..16])
}

struct State {
    manifest: Manifest,
    /// Where each manifest entry lives, relative to `stage` (and later to the
    /// download directory): sanitized and collision-free.
    paths: Vec<PathBuf>,
    stage: PathBuf,
    /// Items saved by an earlier, interrupted hand-off of this transfer.
    placed: Placed,
    hashes: Mutex<Vec<Vec<Option<[u8; 32]>>>>,
    remaining: AtomicU64,
    notify: Notify,
    failure: Mutex<Option<String>>,
}

impl State {
    fn data_path(&self, entry: usize) -> PathBuf {
        data_path(&self.stage, &self.placed, &self.paths[entry])
    }
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

/// Prepares the staging area and figures out which chunks are already present
/// (in staging, or in items an interrupted hand-off already saved).
fn prepare(stage: &Path, download_dir: &Path, m: &Manifest, paths: &[PathBuf]) -> Result<(Vec<HaveChunk>, Placed)> {
    if completed_long_ago(stage) {
        // Same items sent again later: a new, independent copy.
        std::fs::remove_dir_all(stage)?;
    }
    std::fs::create_dir_all(stage)?;
    let _ = std::fs::remove_file(stage.join(COMPLETE_MARK));
    let tops: HashSet<&Path> = m.entries.iter().zip(paths).filter(|(e, _)| !e.path.contains('/')).map(|(_, p)| p.as_path()).collect();
    let Journal { mut placed, dropped } = read_placed(stage, download_dir, &tops)?;
    let journaled = if dropped { usize::MAX } else { placed.len() };
    // Still in staging → never moved; gone from the destination → nothing to count.
    let exists = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    placed.retain(|top, dst| !exists(&stage.join(top)) && exists(dst));

    // The chunk log is streamed, never loaded whole: memory stays one bit per
    // chunk plus the result. Every chunk is logged once (rarely twice, when a
    // duplicate races), so anything past twice the chunk count is junk and is
    // not read. The log is then rewritten with just the verified records.
    let mut have = vec![];
    let log = stage.join(".chunks");
    let mut first_chunk = Vec::with_capacity(m.entries.len());
    let mut total_chunks: u64 = 0;
    for e in &m.entries {
        first_chunk.push(total_chunks);
        total_chunks += e.chunks();
    }
    let mut seen = vec![false; total_chunks as usize];
    let mut records_read: u64 = 0;
    if let Ok(f) = std::fs::File::open(&log) {
        use std::io::Read;
        let mut r = std::io::BufReader::with_capacity(256 * 1024, f);
        let mut rec = [0u8; LOG_RECORD];
        let limit = total_chunks.saturating_mul(2) + 64;
        while records_read < limit && r.read_exact(&mut rec).is_ok() {
            records_read += 1;
            let entry = u32::from_be_bytes(rec[0..4].try_into()?);
            let chunk = u32::from_be_bytes(rec[4..8].try_into()?);
            let Some(e) = m.entries.get(entry as usize) else { continue };
            let EntryKind::File { size, .. } = e.kind else { continue };
            let n = chunk_count(size);
            if chunk as u64 >= n {
                continue;
            }
            let idx = (first_chunk[entry as usize] + chunk as u64) as usize;
            if std::mem::replace(&mut seen[idx], true) {
                continue;
            }
            // Trust the disk, not the log.
            let path = data_path(stage, &placed, &paths[entry as usize]);
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
    }
    drop(seen);
    if records_read > have.len() as u64 {
        compact_log(&log, &have);
    }

    // An already saved item only counts if all of it is still intact where it
    // was saved: every entry still there, of the same kind and size, with
    // every chunk verified. Otherwise (edited, grown, partly deleted) it is
    // received again into staging and saved next to it, never written into.
    if !placed.is_empty() {
        let same_shape = |e: &crate::manifest::Entry, p: &Path| match (std::fs::symlink_metadata(p), &e.kind) {
            (Ok(md), EntryKind::File { size, .. }) => md.is_file() && md.len() == *size,
            (Ok(md), EntryKind::Dir) => md.is_dir(),
            (Err(_), _) => false,
        };
        let mut verified = vec![false; total_chunks as usize];
        for h in &have {
            verified[(first_chunk[h.entry as usize] + h.chunk as u64) as usize] = true;
        }
        let mut broken = HashSet::new();
        for (i, (e, rel)) in m.entries.iter().zip(paths).enumerate() {
            let (top, _) = split_top(rel);
            let range = first_chunk[i] as usize..(first_chunk[i] + e.chunks()) as usize;
            if placed.contains_key(&top) && !(verified[range].iter().all(|v| *v) && same_shape(e, &data_path(stage, &placed, rel))) {
                broken.insert(top);
            }
        }
        if !broken.is_empty() {
            placed.retain(|top, _| !broken.contains(top));
            have.retain(|h| !broken.contains(&split_top(&paths[h.entry as usize]).0));
        }
    }
    if placed.len() != journaled {
        // Best effort: a stale entry is harmless (see the rule above).
        if let Err(e) = write_placed(stage, &placed) {
            tracing::warn!("could not prune the hand-off journal: {e}");
        }
    }

    for (e, rel) in m.entries.iter().zip(paths) {
        if placed.contains_key(&split_top(rel).0) {
            continue;
        }
        let p = stage.join(rel);
        match e.kind {
            EntryKind::Dir => std::fs::create_dir_all(&p)?,
            EntryKind::File { .. } => {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
            }
        }
    }
    Ok((have, placed))
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
    let paths = local_paths(&manifest)?;
    let (have, placed) = {
        let (stage, dl, m, paths) = (stage.clone(), download_dir.clone(), manifest.clone(), paths.clone());
        tokio::task::spawn_blocking(move || prepare(&stage, &dl, &m, &paths)).await??
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
        paths,
        placed,
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
        if let Err(e) = tokio::task::spawn_blocking(move || verify(&st, &roots)).await? {
            let _ = write_msg(ctrl_send, &Msg::Result { ok: false, error: Some(format!("{e:#}")) }).await;
            // Content mismatch: never resume from this staging area.
            let _ = std::fs::remove_dir_all(&state.stage);
            return Err(e);
        }
        let st = state.clone();
        match tokio::task::spawn_blocking(move || place(&st, &download_dir)).await? {
            Ok(paths) => {
                write_msg(ctrl_send, &Msg::Result { ok: true, error: None }).await?;
                Ok(Received { saved_paths: paths })
            }
            Err(e) => {
                // Verified data stays staged (nothing was left half-moved),
                // so sending the same items again completes instantly.
                let _ = write_msg(ctrl_send, &Msg::Result { ok: false, error: Some(format!("{e:#}")) }).await;
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
        let path = state.data_path(h.entry as usize);
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
            // Duplicate (e.g. sent twice across a reconnect). Two copies of the
            // same chunk may race past this check; that is harmless: both
            // write identical, already verified bytes to the same offset, the
            // log tolerates repeats, and `remaining` drops once (below).
            if st.hashes.lock()[h.entry as usize][h.chunk as usize].is_some() {
                return Ok(());
            }
            let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&path)?;
            write_at(&f, &data, h.chunk as u64 * CHUNK_SIZE)?;
            let rec = log_record(h.entry, h.chunk, &hash);
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

/// Checks every file against the sender's BLAKE3 roots and applies size,
/// mtime and permissions. Nothing is moved yet.
fn verify(state: &State, roots: &[[u8; 32]]) -> Result<()> {
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
        let p = state.data_path(i);
        let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&p)?;
        f.set_len(size)?;
        if mtime_ms > 0 {
            let _ = f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(mtime_ms));
        }
        // The data must be on disk before the file is renamed into place:
        // otherwise a power cut could leave a saved name with lost content.
        fsx::sync_data(&f).with_context(|| format!("syncing {}", e.path))?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(not(unix))]
        let _ = executable;
    }
    // And the chunk log: after a power cut it is what lets the next attempt
    // count items already saved instead of saving them a second time.
    if let Ok(log) = std::fs::OpenOptions::new().append(true).open(state.stage.join(".chunks")) {
        fsx::sync_data(&log).context("syncing the chunk log")?;
    }
    Ok(())
}

/// Moves the top-level items into the download directory, each under a name
/// that is free at that instant (atomic no-replace rename, never
/// overwriting).
///
/// Each item's move is atomic; the set is all or nothing on a best-effort
/// basis: if one move fails, the items already moved are renamed back into
/// staging. Should that rollback itself fail (the download folder vanished,
/// a disk error), those items stay saved, the error names them, and the
/// `.placed` journal makes the next attempt of this transfer count them as
/// done instead of saving them again. The rest stays staged.
fn place(state: &State, download_dir: &Path) -> Result<Vec<String>> {
    if DEBUG_PLACE_FAULT.swap(false, Ordering::AcqRel) {
        let first = Mutex::new(None::<PathBuf>);
        let rename = |from: &Path, to: &Path| {
            let mut first = first.lock();
            if first.get_or_insert_with(|| from.to_path_buf()) != from {
                return Err(std::io::Error::other("injected fault"));
            }
            fsx::rename_noreplace(from, to)
        };
        return place_with(state, download_dir, rename, |_, _| Err(std::io::Error::other("injected fault")), &|| {});
    }
    place_with(state, download_dir, fsx::rename_noreplace, fsx::rename_noreplace, &|| {})
}

/// `step` is called between every two durable effects (crash-injection tests
/// abort the process there).
fn place_with(
    state: &State,
    download_dir: &Path,
    rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
    move_back: impl Fn(&Path, &Path) -> std::io::Result<()>,
    step: &dyn Fn(),
) -> Result<Vec<String>> {
    std::fs::create_dir_all(download_dir).context("creating download directory")?;
    // Staged names and (on Apple, where `sync_data` stops at the drive cache)
    // the files' data reach stable storage before any rename.
    fsx::sync_dir(&state.stage).context("syncing the staging area")?;
    let tops = state.manifest.entries.iter().zip(&state.paths).filter(|(e, _)| !e.path.contains('/')).map(|(_, rel)| rel);
    let mut journal = state.placed.clone();
    let mut saved = vec![];
    let mut moved: Vec<(&PathBuf, PathBuf, PathBuf)> = vec![];
    let fail = |name: &str, e: &dyn std::fmt::Display, journal: &mut Placed, moved: &[(&PathBuf, PathBuf, PathBuf)]| -> anyhow::Error {
        let mut stuck = vec![];
        for (rel, from, to) in moved.iter().rev() {
            match move_back(to, from) {
                Ok(()) => {
                    journal.remove(*rel);
                }
                Err(e) => {
                    tracing::warn!("could not roll back {}: {e}", to.display());
                    stuck.push(to.display().to_string());
                }
            }
        }
        let mut msg = format!("cannot save {name} in {}: {e}", download_dir.display());
        if !stuck.is_empty() {
            msg += &format!(" (already saved and left in place: {})", stuck.join(", "));
        }
        // Items moved back are in staging again, which by itself voids their
        // journal entries; updating the journal is tidiness, and a failure is
        // reported rather than hidden.
        let synced = fsx::sync_dir(&state.stage).and_then(|_| write_placed(&state.stage, journal));
        if let Err(e) = synced {
            msg += &format!(" (journal not updated: {e})");
        }
        SaveError(msg).into()
    };
    for rel in tops {
        if let Some(dst) = state.placed.get(rel) {
            saved.push(dst.clone()); // saved by an earlier attempt, verified intact
            continue;
        }
        let from = state.stage.join(rel);
        let name = rel.to_string_lossy();
        // Write-ahead: the journal names each candidate before the rename.
        let intend = |to: &Path| {
            step();
            journal.insert(rel.clone(), to.to_path_buf());
            let r = write_placed(&state.stage, &journal).map_err(|e| std::io::Error::other(format!("journal: {e}")));
            step();
            r
        };
        match fsx::move_unique_with(&from, download_dir, &name, intend, &rename) {
            Ok(to) => {
                step();
                saved.push(to.clone());
                moved.push((rel, from, to));
            }
            Err(e) => {
                journal.remove(rel);
                return Err(fail(&name, &e, &mut journal, &moved));
            }
        }
    }
    // The renames must be on disk before the sender is told "done".
    if let Err(e) = fsx::sync_dir(download_dir) {
        return Err(fail("the received items", &format!("sync: {e}"), &mut journal, &moved));
    }
    step();
    if let Err(e) = fsx::write_durable(&state.stage.join(COMPLETE_MARK), b"") {
        // Harmless: without the mark the staging area is simply kept longer
        // (up to a week) and still answers retries of this transfer.
        tracing::warn!("could not mark {} complete: {e}", state.stage.display());
    }
    step();
    Ok(saved.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
}

/// Removes staging areas untouched for a week, and completed ones past
/// `COMPLETE_TTL`.
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
        if old || completed_long_ago(&e.path()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(stage: PathBuf, entries: Vec<crate::manifest::Entry>) -> State {
        let manifest = Manifest { entries };
        let paths = local_paths(&manifest).unwrap();
        State {
            hashes: Mutex::new(vec![vec![]; manifest.entries.len()]),
            manifest,
            paths,
            stage,
            placed: Placed::new(),
            remaining: AtomicU64::new(0),
            notify: Notify::new(),
            failure: Mutex::new(None),
        }
    }

    fn file(path: &str) -> crate::manifest::Entry {
        crate::manifest::Entry { path: path.into(), kind: EntryKind::File { size: 1, mtime_ms: 0, executable: false } }
    }

    #[test]
    fn place_never_overwrites_and_renames_collisions() {
        let d = tempfile::tempdir().unwrap();
        let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::create_dir_all(&dl).unwrap();
        std::fs::write(dl.join("report_2026.txt"), b"mine").unwrap();
        let st = state(stage.clone(), vec![file("report:2026.txt"), file("report?2026.txt")]);
        std::fs::write(stage.join(&st.paths[0]), b"A").unwrap();
        std::fs::write(stage.join(&st.paths[1]), b"B").unwrap();

        let saved = place(&st, &dl).unwrap();
        assert_eq!(std::fs::read(dl.join("report_2026.txt")).unwrap(), b"mine", "existing file untouched");
        let got: Vec<Vec<u8>> = saved.iter().map(|p| std::fs::read(p).unwrap()).collect();
        assert_eq!(got, [b"A".to_vec(), b"B".to_vec()], "both arrive, neither overwritten");
        assert_eq!(saved.iter().collect::<HashSet<_>>().len(), 2);
        assert!(only_metadata(&stage), "no item data left in staging");
    }

    #[test]
    fn place_is_all_or_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
        std::fs::create_dir_all(&stage).unwrap();
        let st = state(stage.clone(), vec![file("first.txt"), file("second.txt")]);
        std::fs::write(stage.join("first.txt"), b"1").unwrap();
        // "second.txt" is missing from staging, so its move fails.
        let err = place(&st, &dl).unwrap_err();
        assert!(err.downcast_ref::<SaveError>().is_some(), "{err:#}");
        assert!(!dl.join("first.txt").exists(), "first item rolled back");
        assert_eq!(std::fs::read(stage.join("first.txt")).unwrap(), b"1", "and kept in staging");
    }

    /// Writes `data` as a staged-and-logged single-chunk file of entry `i`.
    fn log_chunk(stage: &Path, i: u32, data: &[u8]) {
        let rec = log_record(i, 0, &chunk_hash(data, 0, 1));
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(stage.join(".chunks")).unwrap();
        f.write_all(&rec).unwrap();
    }

    fn sized(path: &str, size: u64) -> crate::manifest::Entry {
        crate::manifest::Entry { path: path.into(), kind: EntryKind::File { size, mtime_ms: 0, executable: false } }
    }

    /// Staging holds nothing but its bookkeeping files (journal, log, mark).
    fn only_metadata(stage: &Path) -> bool {
        walkdir::WalkDir::new(stage).min_depth(1).into_iter().flatten().all(|e| e.file_name().to_string_lossy().starts_with('.'))
    }

    // ---- crash injection: the hand-off aborted at every step ----

    fn crash_manifest() -> Manifest {
        Manifest {
            entries: vec![
                sized("a.txt", 3),
                sized("b.txt", 4),
                crate::manifest::Entry { path: "emptydir".into(), kind: EntryKind::Dir },
                crate::manifest::Entry { path: "folder".into(), kind: EntryKind::Dir },
                sized("folder/inner.txt", 5),
            ],
        }
    }
    const CRASH_DATA: [(usize, &[u8]); 3] = [(0, b"AAA"), (1, b"BBBB"), (4, b"inner")];

    /// The receiving side as a fresh attempt sees it: `prepare`, then `State`.
    fn recover(stage: &Path) -> (State, usize) {
        let m = crash_manifest();
        let paths = local_paths(&m).unwrap();
        let (have, placed) = prepare(stage, &stage.parent().unwrap().join("dl"), &m, &paths).unwrap();
        let mut st = state(stage.to_path_buf(), m.entries);
        st.placed = placed;
        let mut hashes: Vec<Vec<Option<[u8; 32]>>> = st.manifest.entries.iter().map(|e| vec![None; e.chunks() as usize]).collect();
        for h in &have {
            hashes[h.entry as usize][h.chunk as usize] = Some(h.hash);
        }
        *st.hashes.lock() = hashes;
        (st, have.len())
    }

    /// The end of a transfer: `verify` (which makes the received data
    /// durable), then the hand-off.
    fn finish(st: &State, dl: &Path, step: &dyn Fn()) -> Result<Vec<String>> {
        let roots: Vec<[u8; 32]> = st.hashes.lock().iter().map(|hs| file_root(&hs.iter().flatten().copied().collect::<Vec<_>>())).collect();
        verify(st, &roots)?;
        place_with(st, dl, fsx::rename_noreplace, fsx::rename_noreplace, step)
    }

    /// Child process of the test below; does nothing unless told to.
    #[test]
    fn crash_child() {
        let (Ok(dir), Ok(k)) = (std::env::var("WARP_CRASH_DIR"), std::env::var("WARP_CRASH_AT")) else { return };
        let k: usize = k.parse().unwrap();
        let dir = PathBuf::from(dir);
        let (st, _) = recover(&dir.join("stage"));
        let n = std::sync::atomic::AtomicUsize::new(0);
        let step = || {
            if n.fetch_add(1, Ordering::SeqCst) == k {
                std::process::abort(); // like a crash: no unwinding, no cleanup
            }
        };
        finish(&st, &dir.join("dl"), &step).unwrap();
        std::process::exit(42); // finished before step k
    }

    /// With `WARP_POWER_CUT` set to a shell command that discards all data
    /// not yet synced (CI runs this on LazyFS), every crash is also a power
    /// cut.
    #[cfg(unix)]
    #[test]
    fn a_crash_at_any_step_of_the_hand_off_loses_and_duplicates_nothing() {
        use std::os::unix::process::ExitStatusExt;
        let power_cut = std::env::var("WARP_POWER_CUT").ok().filter(|c| !c.is_empty());
        let exe = std::env::current_exe().unwrap();
        let mut finished = false;
        let mut k = 0;
        while !finished {
            assert!(k < 200, "hand-off never finished");
            let d = tempfile::tempdir().unwrap();
            let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
            std::fs::create_dir_all(stage.join("folder")).unwrap();
            std::fs::create_dir_all(stage.join("emptydir")).unwrap();
            std::fs::create_dir_all(&dl).unwrap();
            let paths = local_paths(&crash_manifest()).unwrap();
            for (i, data) in CRASH_DATA {
                std::fs::write(stage.join(&paths[i]), data).unwrap();
                log_chunk(&stage, i as u32, data);
            }
            // A file saved long before (so on disk); forces a rename collision.
            std::fs::write(dl.join("a.txt"), b"theirs").unwrap();
            std::fs::File::open(dl.join("a.txt")).unwrap().sync_all().unwrap();

            let status = std::process::Command::new(&exe)
                .args(["receiver::tests::crash_child", "--exact", "--test-threads=1"])
                .env("WARP_CRASH_DIR", d.path())
                .env("WARP_CRASH_AT", k.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            finished = status.code() == Some(42);
            assert!(finished || status.signal() == Some(libc::SIGABRT), "k={k}: child failed: {status:?}");
            if let Some(cut) = &power_cut {
                // Everything not yet on stable storage is lost.
                let ok = std::process::Command::new("sh").args(["-c", cut]).status().unwrap();
                assert!(ok.success(), "power cut command failed");
            }

            // The next attempt of the same transfer.
            let (st, have) = recover(&stage);
            assert_eq!(have, CRASH_DATA.len(), "k={k}: every chunk is still accounted for, nothing to re-send");
            let saved = finish(&st, &dl, &|| {}).unwrap_or_else(|e| panic!("k={k}: {e:#}"));
            let mut names: Vec<String> = std::fs::read_dir(&dl).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            assert_eq!(names, ["a (2).txt", "a.txt", "b.txt", "emptydir", "folder"], "k={k}: each item exactly once");
            assert_eq!(std::fs::read(dl.join("a.txt")).unwrap(), b"theirs", "k={k}: existing file untouched");
            assert_eq!(std::fs::read(dl.join("a (2).txt")).unwrap(), b"AAA");
            assert_eq!(std::fs::read(dl.join("b.txt")).unwrap(), b"BBBB");
            assert_eq!(std::fs::read(dl.join("folder/inner.txt")).unwrap(), b"inner");
            assert_eq!(saved.len(), 4, "k={k}");
            assert!(only_metadata(&stage), "k={k}");
            k += 1;
        }
        assert!(k > 8, "only {k} steps exercised");
    }

    #[test]
    fn a_journal_failure_stops_before_anything_moves() {
        let d = tempfile::tempdir().unwrap();
        let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
        std::fs::create_dir_all(&stage).unwrap();
        let st = state(stage.clone(), vec![file("a.txt")]);
        std::fs::write(stage.join("a.txt"), b"a").unwrap();
        // The journal cannot be written (its temp path is taken by a folder).
        std::fs::create_dir(stage.join(".placed.tmp")).unwrap();
        let err = format!("{:#}", place(&st, &dl).unwrap_err());
        assert!(err.contains("journal"), "{err}");
        assert!(!dl.join("a.txt").exists(), "nothing moved without a journal entry");
        assert!(stage.join("a.txt").exists());
    }

    #[test]
    fn journal_entries_are_reconciled_on_restart() {
        let d = tempfile::tempdir().unwrap();
        let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::create_dir_all(&dl).unwrap();
        let m = Manifest { entries: vec![sized("moved.txt", 5), sized("intent.txt", 6), sized("edited.txt", 6)] };
        let paths = local_paths(&m).unwrap();
        // "moved": crashed after its rename → only in Downloads, intact.
        std::fs::write(dl.join("moved.txt"), b"moved").unwrap();
        log_chunk(&stage, 0, b"moved");
        // "intent": crashed after the journal write, before the rename → still
        // staged; the destination name is someone else's file.
        std::fs::write(stage.join("intent.txt"), b"intent").unwrap();
        std::fs::write(dl.join("intent.txt"), b"theirs").unwrap();
        log_chunk(&stage, 1, b"intent");
        // "edited": moved, then changed by the user.
        std::fs::write(dl.join("edited.txt"), b"EDITED").unwrap();
        log_chunk(&stage, 2, b"edited");
        let journal: Placed = ["moved.txt", "intent.txt", "edited.txt"].iter().map(|n| (PathBuf::from(n), dl.join(n))).collect();
        write_placed(&stage, &journal).unwrap();

        let (have, placed) = prepare(&stage, &dl, &m, &paths).unwrap();
        assert_eq!(placed.keys().collect::<Vec<_>>(), [&PathBuf::from("moved.txt")]);
        let mut got: Vec<u32> = have.iter().map(|h| h.entry).collect();
        got.sort();
        assert_eq!(got, [0, 1], "moved verified in place, intent in staging, edited must be re-sent");
        let tops = paths.iter().map(PathBuf::as_path).collect();
        assert_eq!(read_placed(&stage, &dl, &tops).unwrap().placed, placed, "journal pruned");
    }

    #[test]
    fn the_journal_is_validated_and_a_corrupt_one_set_aside() {
        let d = tempfile::tempdir().unwrap();
        let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::create_dir_all(&dl).unwrap();
        let elsewhere = d.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let m = Manifest { entries: vec![sized("a.txt", 1), sized("b.txt", 1), sized("c.txt", 1)] };
        let paths = local_paths(&m).unwrap();
        // Every item left staging and has identical content at the journal's
        // destination, but only "a" is somewhere `place` could have put it.
        for (n, dst) in [("a.txt", dl.join("a.txt")), ("b.txt", elsewhere.join("b.txt")), ("c.txt", dl.join("sub/../c.txt"))] {
            std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
            std::fs::write(&dst, b"x").unwrap();
            log_chunk(&stage, paths.iter().position(|p| p == Path::new(n)).unwrap() as u32, b"x");
        }
        std::fs::write(dl.join("not-ours.txt"), b"x").unwrap();
        let entry = |item: &str, to: PathBuf| JournalEntry { item: item.into(), to: to.to_string_lossy().into_owned() };
        let journal = |placed: Vec<JournalEntry>| serde_json::to_vec(&JournalFile { version: JOURNAL_VERSION, placed }).unwrap();
        let entries = || {
            vec![
                entry("a.txt", dl.join("a.txt")),
                entry("b.txt", elsewhere.join("b.txt")), // outside the download folder
                entry("c.txt", dl.join("sub/../c.txt")), // not a direct child
                entry("../x", dl.join("not-ours.txt")),  // not an item of this transfer
                entry("/etc", dl.join("not-ours.txt")),
            ]
        };
        std::fs::write(stage.join(PLACED_LOG), journal(entries())).unwrap();
        let (have, placed) = prepare(&stage, &dl, &m, &paths).unwrap();
        assert_eq!(placed, Placed::from([(PathBuf::from("a.txt"), dl.join("a.txt"))]));
        assert_eq!(have.iter().map(|h| h.entry).collect::<Vec<_>>(), [0], "b and c are received again");
        let tops = paths.iter().map(PathBuf::as_path).collect();
        assert_eq!(read_placed(&stage, &dl, &tops).unwrap().placed, placed, "rewritten without the bad entries");

        // Missing: nothing saved yet, nothing reported.
        std::fs::remove_file(stage.join(PLACED_LOG)).unwrap();
        let j = read_placed(&stage, &dl, &tops).unwrap();
        assert!(j.placed.is_empty() && !j.dropped);

        // Corrupt in any way: quarantined whole (renamed, never deleted), vouches for nothing.
        let mut dup = entries();
        dup.push(entry("a.txt", dl.join("a (2).txt")));
        let huge = [b"{\"version\":1,\"placed\":[".as_slice(), &vec![b' '; (journal_limit(&dl, &tops) + 1) as usize], b"]}"].concat();
        let bad: [(&str, Vec<u8>); 6] = [
            ("truncated", b"{\"version\":1,\"placed\":[".to_vec()),
            ("an item twice", journal(dup)),
            ("unknown field", br#"{"version":1,"placed":[],"extra":1}"#.to_vec()),
            ("unknown version", br#"{"version":2,"placed":[]}"#.to_vec()),
            ("the old map format", br#"{"a.txt":"/x"}"#.to_vec()),
            ("larger than this transfer can produce", huge),
        ];
        for (what, bytes) in bad {
            let _ = std::fs::remove_file(stage.join(".placed.corrupt"));
            std::fs::write(stage.join(PLACED_LOG), &bytes).unwrap();
            let j = read_placed(&stage, &dl, &tops).unwrap();
            assert!(j.placed.is_empty() && j.dropped, "{what}");
            assert!(!stage.join(PLACED_LOG).exists(), "{what}");
            assert_eq!(std::fs::read(stage.join(".placed.corrupt")).unwrap(), bytes, "{what}: kept intact for inspection");
        }
        let (_, placed) = prepare(&stage, &dl, &m, &paths).unwrap();
        assert!(placed.is_empty(), "a corrupt journal vouches for nothing");

        // A folder where the journal should be: moved aside whole, its
        // contents untouched (nothing is ever deleted recursively).
        std::fs::remove_file(stage.join(".placed.corrupt")).unwrap();
        std::fs::create_dir_all(stage.join(PLACED_LOG).join("inner")).unwrap();
        std::fs::write(stage.join(PLACED_LOG).join("inner/keep"), b"keep").unwrap();
        let j = read_placed(&stage, &dl, &tops).unwrap();
        assert!(j.placed.is_empty() && j.dropped);
        assert_eq!(std::fs::read(stage.join(".placed.corrupt/inner/keep")).unwrap(), b"keep");
        std::fs::remove_dir_all(stage.join(".placed.corrupt")).unwrap();

        // A corrupt journal that cannot be quarantined: the transfer stops up
        // front with an error that says so, and the file is left as it is.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::write(stage.join(PLACED_LOG), b"garbage").unwrap();
            std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o555)).unwrap();
            let enforced = std::fs::write(stage.join("probe"), b"").is_err(); // not on FAT/exFAT/NTFS mounts, or as root
            if enforced {
                let err = prepare(&stage, &dl, &m, &paths).unwrap_err();
                std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o755)).unwrap();
                assert!(err.downcast_ref::<SaveError>().is_some(), "not retried: {err:#}");
                assert!(format!("{err:#}").contains("cannot be quarantined"), "{err:#}");
                assert_eq!(std::fs::read(stage.join(PLACED_LOG)).unwrap(), b"garbage", "left for inspection");
            } else {
                std::fs::set_permissions(&stage, std::fs::Permissions::from_mode(0o755)).unwrap();
                eprintln!("permissions not enforced here; skipped the read-only case");
            }
        }
    }

    #[test]
    fn the_chunk_log_is_streamed_bounded_and_compacted() {
        let d = tempfile::tempdir().unwrap();
        let stage = d.path().join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        let m = Manifest { entries: vec![sized("a.bin", 3)] };
        let paths = local_paths(&m).unwrap();
        std::fs::write(stage.join("a.bin"), b"abc").unwrap();
        // One good record, then a flood of junk (wrong hashes, bad indices).
        log_chunk(&stage, 0, b"abc");
        let mut f = std::fs::OpenOptions::new().append(true).open(stage.join(".chunks")).unwrap();
        for i in 0..10_000u32 {
            f.write_all(&log_record(i % 3, i, &[7; 32])).unwrap();
        }
        drop(f);
        let (have, _) = prepare(&stage, &d.path().join("dl"), &m, &paths).unwrap();
        assert_eq!(have.len(), 1);
        assert_eq!(std::fs::metadata(stage.join(".chunks")).unwrap().len(), LOG_RECORD as u64, "compacted to the verified record");
        // Junk before the good record beyond the read limit is not even read.
        let junk: Vec<u8> = (0..100u32).flat_map(|i| log_record(5, i, &[0; 32])).collect();
        std::fs::write(stage.join(".chunks"), junk).unwrap();
        log_chunk(&stage, 0, b"abc");
        let (have, _) = prepare(&stage, &d.path().join("dl"), &m, &paths).unwrap();
        assert!(have.is_empty(), "limit is 2 × chunks + 64 records");
    }

    #[test]
    fn a_failed_rollback_is_reported_and_loses_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (stage, dl) = (d.path().join("stage"), d.path().join("dl"));
        std::fs::create_dir_all(&stage).unwrap();
        let st = state(stage.clone(), vec![file("a.txt"), file("b.txt"), file("c.txt")]);
        for n in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(stage.join(n), n).unwrap();
        }
        // "c.txt" fails to move in; rolling back "b.txt" fails too.
        let move_in = |from: &Path, to: &Path| {
            if from.ends_with("c.txt") {
                return Err(std::io::Error::other("disk error"));
            }
            fsx::rename_noreplace(from, to)
        };
        let move_back = |to: &Path, from: &Path| {
            if to.ends_with("b.txt") {
                return Err(std::io::Error::other("disk error"));
            }
            fsx::rename_noreplace(to, from)
        };
        let err = format!("{:#}", place_with(&st, &dl, move_in, move_back, &|| {}).unwrap_err());
        assert!(err.contains("cannot save c.txt") && err.contains("left in place") && err.contains("b.txt"), "{err}");
        // Every file is in exactly one place.
        assert_eq!(std::fs::read(stage.join("a.txt")).unwrap(), b"a.txt", "rolled back");
        assert!(!dl.join("a.txt").exists());
        assert_eq!(std::fs::read(dl.join("b.txt")).unwrap(), b"b.txt", "reported as saved");
        assert!(!stage.join("b.txt").exists());
        assert_eq!(std::fs::read(stage.join("c.txt")).unwrap(), b"c.txt", "still staged");
    }
}
