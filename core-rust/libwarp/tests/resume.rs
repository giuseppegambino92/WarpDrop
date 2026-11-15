//! (c) Resume: the connection dies mid-transfer ("cable pulled"); the engine
//! reconnects on its own and only the chunks the receiver does not already
//! hold — verified by BLAKE3 content hash — cross the wire again.
//!
//! Lives in its own test binary because it uses a process-global debug hook.

mod common;

use common::*;
use std::sync::atomic::Ordering;
use warp::*;

#[test]
fn interrupted_transfer_resumes_without_resending_verified_chunks() {
    let a = TestNode::new("Sender", |c| c.parallel_streams = 2);
    let b = TestNode::new("Receiver", |_| {});
    a.see(&b);

    let src = tempfile::tempdir().unwrap();
    let dir = src.path().join("dataset");
    std::fs::create_dir_all(&dir).unwrap();
    random_file(&dir.join("part-1.bin"), 24 * 1024 * 1024 + 99); // 7 chunks
    random_file(&dir.join("part-2.bin"), 16 * 1024 * 1024); // 4 chunks
    let total: u64 = 40 * 1024 * 1024 + 99;
    let before = tree_digest(&dir);

    // Pull the cable once the receiver has 6 of 11 chunks safely on disk.
    warp::receiver::DEBUG_DROP_AFTER_CHUNKS.store(6, Ordering::SeqCst);
    let id = a.node.send_files_to_code(b.node.current_code().code, vec![item(&dir)]).unwrap();
    let t = a.wait_transfer(&id, std::time::Duration::from_secs(120));
    assert_eq!(t.state, TransferState::Completed, "{:?}", t.error);
    assert!(
        a.events.events.lock().iter().any(|e| matches!(e, WarpEvent::TransferUpdated { transfer } if transfer.id == id && transfer.state == TransferState::Reconnecting)),
        "transfer never went through Reconnecting"
    );

    // The 6 verified chunks survived the drop and were skipped on resume…
    assert!(t.resumed_bytes >= 6 * 4 * 1024 * 1024 - 4 * 1024 * 1024, "resumed only {} bytes", t.resumed_bytes);

    let inc = b.wait_incoming_done(T);
    assert_eq!(inc.state, TransferState::Completed, "{:?}", inc.error);
    // Across both connections the receiver got each chunk ~once: no full
    // re-send (allow one in-flight duplicate chunk + frame headers).
    let slack = 4 * 1024 * 1024 + 4096;
    assert!(inc.wire_bytes <= total + slack, "receiver got {} bytes for a {total}-byte transfer", inc.wire_bytes);
    assert_eq!(tree_digest(std::path::Path::new(&inc.saved_paths[0])), before);
    // One logical transfer on the receiver, not two.
    assert_eq!(b.node.transfers().len(), 1);
}
