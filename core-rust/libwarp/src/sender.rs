//! Sending side of a file/folder transfer.
//!
//! N workers pull chunks from a shared queue (in manifest order, so the first
//! files finish first), read them with positional I/O, hash them into the
//! BLAKE3 tree, compress when worthwhile, and write frames to their own QUIC
//! stream. Chunks the receiver already holds — verified by content hash — are
//! hashed locally but never sent.

use crate::compress::{self, Compressor, ENC_RAW, ENC_ZSTD};
use crate::hashing::{chunk_hash, chunk_len, file_root, CHUNK_SIZE};
use crate::manifest::{EntryKind, SourceSet};
use crate::protocol::{read_msg, write_msg, FrameHeader, Msg};
use crate::transfer::Transfer;
use crate::types::TransferState;
use crate::WarpError;
use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

pub fn read_exact_at(f: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.read_exact_at(buf, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = f.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
}

struct Job {
    entry: u32,
    chunk: u32,
}

struct Shared {
    src: Arc<SourceSet>,
    jobs: Vec<Job>,
    next: AtomicUsize,
    hashes: Mutex<Vec<Vec<[u8; 32]>>>,
    incompressible: Vec<AtomicBool>,
    have: HashMap<(u32, u32), [u8; 32]>,
}

pub async fn send(
    conn: &quinn::Connection,
    ctrl_send: &mut quinn::SendStream,
    ctrl_recv: &mut quinn::RecvStream,
    src: Arc<SourceSet>,
    t: Arc<Transfer>,
    streams: usize,
) -> Result<()> {
    let m = &src.manifest;
    write_msg(ctrl_send, &Msg::Offer { manifest: m.clone() }).await?;
    t.set_state(TransferState::WaitingForAccept);

    let have = match read_msg(ctrl_recv).await? {
        Msg::Accept { have, ticket } => {
            *t.resume_ticket.lock() = Some(ticket);
            have
        }
        Msg::Reject { reason } => return Err(WarpError::Rejected { reason }.into()),
        other => bail!("unexpected reply to offer: {other:?}"),
    };
    t.reset_progress();
    // A pause requested while connecting must survive the accept.
    t.set_state(if t.is_paused() { TransferState::Paused } else { TransferState::Transferring });

    let mut jobs = vec![];
    let mut hashes = vec![];
    let mut incompressible = vec![];
    for (i, e) in m.entries.iter().enumerate() {
        let n = e.chunks();
        hashes.push(vec![[0u8; 32]; n as usize]);
        let skip = src.sources[i].as_deref().map(compress::ext_incompressible).unwrap_or(true);
        incompressible.push(AtomicBool::new(skip));
        for c in 0..n {
            jobs.push(Job { entry: i as u32, chunk: c as u32 });
        }
    }
    let shared = Arc::new(Shared {
        src: src.clone(),
        jobs,
        next: AtomicUsize::new(0),
        hashes: Mutex::new(hashes),
        incompressible,
        have: have.into_iter().map(|h| ((h.entry, h.chunk), h.hash)).collect(),
    });

    let n_workers = streams.clamp(1, 16).min(shared.jobs.len().max(1));
    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..n_workers {
        let (conn, shared, t) = (conn.clone(), shared.clone(), t.clone());
        workers.spawn(async move { worker(conn, shared, t).await });
    }
    while let Some(r) = workers.join_next().await {
        if let Err(e) = r.map_err(|e| anyhow!("worker panicked: {e}")).and_then(|r| r) {
            workers.abort_all();
            return Err(e);
        }
    }
    if t.is_cancelled() {
        return Err(WarpError::Cancelled.into());
    }

    let roots: Vec<[u8; 32]> = {
        let hashes = shared.hashes.lock();
        m.entries
            .iter()
            .zip(hashes.iter())
            .map(|(e, h)| match e.kind {
                EntryKind::File { .. } => file_root(h),
                EntryKind::Dir => [0u8; 32],
            })
            .collect()
    };
    tracing::debug!("all chunks sent, sending Done");
    write_msg(ctrl_send, &Msg::Done { roots }).await?;
    t.set_state(TransferState::Verifying);
    match read_msg(ctrl_recv).await? {
        Msg::Result { ok: true, .. } => Ok(()),
        Msg::Result { ok: false, error } => bail!("receiver reported: {}", error.unwrap_or_default()),
        other => bail!("unexpected message: {other:?}"),
    }
}

async fn worker(conn: quinn::Connection, s: Arc<Shared>, t: Arc<Transfer>) -> Result<()> {
    let mut stream: Option<quinn::SendStream> = None;
    let mut compressor = Some(Compressor::new());
    loop {
        if !t.wait_unpaused().await {
            return Err(WarpError::Cancelled.into());
        }
        let idx = s.next.fetch_add(1, Ordering::Relaxed);
        let Some(job) = s.jobs.get(idx) else { break };
        let (entry, chunk) = (job.entry, job.chunk);
        let e = &s.src.manifest.entries[entry as usize];
        let size = e.size();
        let n_chunks = e.chunks();
        let path = s.src.sources[entry as usize].clone().ok_or_else(|| anyhow!("missing source"))?;
        let len = chunk_len(size, chunk as u64) as usize;
        let have = s.have.get(&(entry, chunk)).copied();
        let try_compress = !s.incompressible[entry as usize].load(Ordering::Relaxed);

        let mut comp = compressor.take().expect("compressor");
        let (comp, data, hash, payload) = tokio::task::spawn_blocking(move || -> Result<_> {
            let f = std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
            let mut buf = vec![0u8; len];
            read_exact_at(&f, &mut buf, chunk as u64 * CHUNK_SIZE)
                .with_context(|| format!("{} changed while sending", path.display()))?;
            let hash = chunk_hash(&buf, chunk as u64, n_chunks);
            if have == Some(hash) {
                return Ok((comp, buf, hash, None));
            }
            let first = chunk == 0 && compress::magic_incompressible(&buf[..buf.len().min(64)]);
            let payload = if try_compress && !first { Some(comp.compress(&buf)) } else { Some(None) };
            Ok((comp, buf, hash, payload))
        })
        .await??;
        compressor = Some(comp);
        s.hashes.lock()[entry as usize][chunk as usize] = hash;

        let Some(compressed) = payload else {
            // Receiver already holds this exact content.
            t.add_resumed(len as u64);
            continue;
        };
        if compressed.is_none() && try_compress && size > CHUNK_SIZE {
            // zstd did not help on this chunk: stop trying for this file.
            s.incompressible[entry as usize].store(true, Ordering::Relaxed);
        }
        let (enc, body): (u8, &[u8]) = match &compressed {
            Some(z) => (ENC_ZSTD, z),
            None => (ENC_RAW, &data),
        };
        let header = FrameHeader {
            entry,
            chunk,
            raw_len: len as u32,
            enc,
            payload_len: body.len() as u32,
            hash,
        };
        if stream.is_none() {
            stream = Some(conn.open_uni().await?);
        }
        let st = stream.as_mut().expect("stream");
        st.write_all(&header.encode()).await?;
        st.write_all(body).await?;
        t.add_progress(len as u64, (body.len() + header.encode().len()) as u64);
    }
    if let Some(mut st) = stream {
        st.finish()?;
        // Wait until the peer has received everything on this stream.
        let _ = st.stopped().await;
    }
    Ok(())
}
