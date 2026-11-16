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
use std::collections::{BTreeMap, HashMap, HashSet};
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

/// Journal of top-level items already moved into the download directory
/// (`{"<top>": "<final path>"}`), kept in the staging area until the transfer
/// completes. If a hand-off stops halfway (a move and its rollback failed, or
/// the app died), the next attempt for the same transfer re-verifies those
/// items where they now are and does not save them a second time.
const PLACED_LOG: &str = ".placed";

/// Top-level item (relative path, one component) → where it was saved.
type Placed = HashMap<PathBuf, PathBuf>;

fn read_placed(stage: &Path) -> Placed {
    std::fs::read(stage.join(PLACED_LOG))
        .ok()
        .and_then(|b| serde_json::from_slice::<BTreeMap<String, String>>(&b).ok())
        .map(|m| m.into_iter().map(|(k, v)| (PathBuf::from(k), PathBuf::from(v))).collect())
        .unwrap_or_default()
}

fn write_placed(stage: &Path, placed: &Placed) {
    let m: BTreeMap<String, String> =
        placed.iter().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
    let (tmp, dst) = (stage.join(".placed.tmp"), stage.join(PLACED_LOG));
    let r = std::fs::write(&tmp, serde_json::to_vec(&m).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &dst));
    if let Err(e) = r {
        tracing::warn!("could not update {}: {e}", dst.display());
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
fn prepare(stage: &Path, m: &Manifest, paths: &[PathBuf]) -> Result<(Vec<HaveChunk>, Placed)> {
    std::fs::create_dir_all(stage)?;
    let mut placed = read_placed(stage);
    let journaled = placed.len();
    placed.retain(|_, dst| std::fs::symlink_metadata(dst).is_ok());

    let mut have = vec![];
    let log = stage.join(".chunks");
    if let Ok(bytes) = std::fs::read(&log) {
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

    // An already saved item only counts if all of it is still intact where it
    // was saved; otherwise (edited, partly deleted) it is received again into
    // staging and saved next to it, never written into.
    if !placed.is_empty() {
        let verified: HashSet<(u32, u32)> = have.iter().map(|h| (h.entry, h.chunk)).collect();
        let mut broken = HashSet::new();
        for (i, (e, rel)) in m.entries.iter().zip(paths).enumerate() {
            let (top, _) = split_top(rel);
            if placed.contains_key(&top) && (0..e.chunks() as u32).any(|c| !verified.contains(&(i as u32, c))) {
                broken.insert(top);
            }
        }
        if !broken.is_empty() {
            placed.retain(|top, _| !broken.contains(top));
            have.retain(|h| !broken.contains(&split_top(&paths[h.entry as usize]).0));
        }
    }
    if placed.len() != journaled {
        write_placed(stage, &placed);
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
        let (stage, m, paths) = (stage.clone(), manifest.clone(), paths.clone());
        tokio::task::spawn_blocking(move || prepare(&stage, &m, &paths)).await??
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
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(not(unix))]
        let _ = executable;
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
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let move_in = |from: &Path, dir: &Path, name: &str| {
            if calls.fetch_add(1, Ordering::AcqRel) == 1 {
                return Err(std::io::Error::other("injected fault"));
            }
            fsx::move_unique(from, dir, name)
        };
        return place_with(state, download_dir, move_in, |_, _| Err(std::io::Error::other("injected fault")));
    }
    place_with(state, download_dir, fsx::move_unique, fsx::rename_noreplace)
}

fn place_with(
    state: &State,
    download_dir: &Path,
    move_in: impl Fn(&Path, &Path, &str) -> std::io::Result<PathBuf>,
    move_back: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(download_dir).context("creating download directory")?;
    let tops = state.manifest.entries.iter().zip(&state.paths).filter(|(e, _)| !e.path.contains('/')).map(|(_, rel)| rel);
    let mut journal = state.placed.clone();
    let mut saved = vec![];
    let mut moved: Vec<(&PathBuf, PathBuf, PathBuf)> = vec![];
    for rel in tops {
        if let Some(dst) = state.placed.get(rel) {
            saved.push(dst.clone()); // saved by an earlier attempt, verified intact
            continue;
        }
        let from = state.stage.join(rel);
        let name = rel.to_string_lossy();
        match move_in(&from, download_dir, &name) {
            Ok(to) => {
                journal.insert(rel.clone(), to.clone());
                write_placed(&state.stage, &journal);
                saved.push(to.clone());
                moved.push((rel, from, to));
            }
            Err(e) => {
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
                write_placed(&state.stage, &journal);
                let mut msg = format!("cannot save {name} in {}: {e}", download_dir.display());
                if !stuck.is_empty() {
                    msg += &format!(" (already saved and left in place: {})", stuck.join(", "));
                }
                return Err(SaveError(msg).into());
            }
        }
    }
    let _ = std::fs::remove_dir_all(&state.stage);
    Ok(saved.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
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
        assert!(!stage.exists());
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
        let move_in = |from: &Path, dir: &Path, name: &str| {
            if name == "c.txt" {
                return Err(std::io::Error::other("disk error"));
            }
            fsx::move_unique(from, dir, name)
        };
        let move_back = |to: &Path, from: &Path| {
            if to.ends_with("b.txt") {
                return Err(std::io::Error::other("disk error"));
            }
            fsx::rename_noreplace(to, from)
        };
        let err = format!("{:#}", place_with(&st, &dl, move_in, move_back).unwrap_err());
        assert!(err.contains("cannot save c.txt") && err.contains("left in place") && err.contains("b.txt"), "{err}");
        // Every file is in exactly one place.
        assert_eq!(std::fs::read(stage.join("a.txt")).unwrap(), b"a.txt", "rolled back");
        assert!(!dl.join("a.txt").exists());
        assert_eq!(std::fs::read(dl.join("b.txt")).unwrap(), b"b.txt", "reported as saved");
        assert!(!stage.join("b.txt").exists());
        assert_eq!(std::fs::read(stage.join("c.txt")).unwrap(), b"c.txt", "still staged");
    }
}
