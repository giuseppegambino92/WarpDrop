//! Zero-prompt resume across the Internet: the devices only ever meet through
//! the rendezvous server (no LAN discovery), the 6-digit code is consumed by
//! the first connection, the link drops mid-transfer, and the sender comes
//! back through the rendezvous server with its resumption ticket — no new
//! code, no prompt — skipping every chunk the receiver already verified.
//!
//! Own test binary: uses the process-global "cable pull" hook.

mod common;

use common::*;
use std::sync::atomic::Ordering;
use std::time::Duration;
use warp::*;

#[test]
fn internet_transfer_resumes_with_ticket_without_new_code() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let relay = rt.block_on(warp_signal::spawn_local()).unwrap();
    let url = format!("ws://{relay}");

    for force_relay in [true, false] {
        let u = url.clone();
        let a = TestNode::new("Roma", |c| {
            c.relay_url = Some(u.clone());
            c.force_relay = force_relay;
            c.parallel_streams = 2;
        });
        let b = TestNode::new("Milano", |c| {
            c.relay_url = Some(u.clone());
            c.force_relay = force_relay;
        });
        std::thread::sleep(Duration::from_millis(800)); // b registers its code channel

        let src = tempfile::tempdir().unwrap();
        let dir = src.path().join("dataset");
        std::fs::create_dir_all(&dir).unwrap();
        random_file(&dir.join("part-1.bin"), 24 * 1024 * 1024 + 99); // 7 chunks
        random_file(&dir.join("part-2.bin"), 16 * 1024 * 1024); // 4 chunks
        let total: u64 = 40 * 1024 * 1024 + 99;
        let before = tree_digest(&dir);

        let code = b.node.current_code().code;
        warp::receiver::DEBUG_DROP_AFTER_CHUNKS.store(6, Ordering::SeqCst);
        let id = a.node.send_files_to_code(code.clone(), vec![item(&dir)]).unwrap();
        let t = a.wait_transfer(&id, Duration::from_secs(120));
        let ctx = format!("force_relay={force_relay}");
        assert_eq!(t.state, TransferState::Completed, "{ctx}: {:?}", t.error);
        assert!(
            a.events.events.lock().iter().any(|e| matches!(e, WarpEvent::TransferUpdated { transfer } if transfer.id == id && transfer.state == TransferState::Reconnecting)),
            "{ctx}: never went through Reconnecting"
        );
        if force_relay {
            assert_eq!(t.path, Some(PathKind::Relay), "{ctx}");
        }
        // The first connection consumed the code; resuming did not need a new one.
        assert_ne!(b.node.current_code().code, code, "{ctx}");
        // Verified chunks were not re-sent.
        assert!(t.resumed_bytes >= 5 * 4 * 1024 * 1024, "{ctx}: resumed only {}", t.resumed_bytes);
        let inc = b.wait_incoming_done(T);
        assert_eq!(inc.state, TransferState::Completed, "{ctx}: {:?}", inc.error);
        assert!(inc.wire_bytes <= total + 4 * 1024 * 1024 + 4096, "{ctx}: receiver got {} bytes", inc.wire_bytes);
        assert_eq!(tree_digest(std::path::Path::new(&inc.saved_paths[0])), before, "{ctx}");
        assert_eq!(b.node.transfers().len(), 1, "{ctx}: one logical transfer on the receiver");
        // Nothing prompted the receiver.
        assert!(b.events.events.lock().iter().all(|e| !matches!(e, WarpEvent::OfferReceived { .. })), "{ctx}");
    }
    drop(rt);
}
