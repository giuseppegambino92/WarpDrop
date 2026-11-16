//! End-to-end tests: two (or three) real nodes on this machine talking QUIC +
//! TLS 1.3 over loopback, with real files on disk.

mod common;

use common::*;
use std::time::Duration;
use warp::*;

/// (a) Discovery: nodes find each other over mDNS / UDP broadcast and the
/// advertised identity is the TLS identity.
#[test]
fn nodes_discover_each_other_on_the_lan() {
    let a = TestNode::new("Alpha Mac", |c| {
        c.enable_mdns = true;
        c.enable_beacon = true;
        c.beacon_port = 47951;
    });
    let b = TestNode::new("Beta PC", |c| {
        c.enable_mdns = true;
        c.enable_beacon = true;
        c.beacon_port = 47951;
    });
    let bid = b.id();
    wait_until(Duration::from_secs(20), || a.node.peers().iter().any(|p| p.device_id == bid));
    let p = a.node.peers().into_iter().find(|p| p.device_id == bid).unwrap();
    assert_eq!(p.name, "Beta PC");
    assert!(p.code_open);
    assert!(p.best_path.is_some());
    // Discovery alone is enough to send (prompted, since they are strangers).
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("hello.txt");
    std::fs::write(&f, "ciao").unwrap();
    let id = a.node.send_files(bid, vec![item(&f)]).unwrap();
    let offer = b.find_event(T, |e| match e {
        WarpEvent::OfferReceived { offer } => Some(offer.clone()),
        _ => None,
    });
    b.node.respond_to_offer(offer.offer_id, true);
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Completed);
    assert_eq!(std::fs::read_to_string(b.downloads().join("hello.txt")).unwrap(), "ciao");
}

/// (a) + (b) Pair with a 6-digit code (SPAKE2 bound to TLS) and move a nested
/// directory, verifying every byte with BLAKE3.
#[test]
fn code_pairing_transfers_nested_directory_bit_for_bit() {
    let a = TestNode::new("Sender", |_| {});
    let b = TestNode::new("Receiver", |_| {});
    a.see(&b); // b is "nearby" and shows a code
    let src = tempfile::tempdir().unwrap();
    let tree = make_tree(src.path());
    let before = tree_digest(&tree);

    let code = b.node.current_code();
    assert_eq!(code.display.len(), 7);
    let id = a.node.send_files_to_code(code.display.clone(), vec![item(&tree)]).unwrap();
    let out = a.wait_transfer(&id, T);
    assert_eq!(out.state, TransferState::Completed, "{:?}", out.error);
    assert_eq!(out.path, Some(PathKind::Loopback));
    assert_eq!(out.resumed_bytes, 0);
    // Compressible text must have been compressed, media must not.
    assert!(out.wire_bytes < out.total_bytes, "adaptive compression saved nothing");

    let inc = b.wait_incoming_done(T);
    assert_eq!(inc.state, TransferState::Completed, "{:?}", inc.error);
    let saved = std::path::PathBuf::from(&inc.saved_paths[0]);
    assert_eq!(saved.file_name().unwrap(), "Progetto Vacanze ☀️");
    assert_eq!(tree_digest(&saved), before, "received tree differs from source");
    // Only bookkeeping (kept 30 min for idempotent retries), no item data.
    let leftover: Vec<_> = walkdir::WalkDir::new(b.downloads().join(".warpdrop-partial"))
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftover.is_empty(), "staging not cleaned: {leftover:?}");

    // The code is single-use.
    assert_ne!(b.node.current_code().code, code.code);
}

#[test]
fn qr_ticket_works_without_discovery() {
    let a = TestNode::new("Phone", |_| {});
    let b = TestNode::new("Laptop", |_| {});
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("doc.pdf");
    random_file(&f, 123_456);
    let mut ticket = b.node.current_code().qr_payload;
    // Loopback-only test environment: point the ticket at 127.0.0.1.
    let port = b.node.port();
    let base = ticket.split("&a=").next().unwrap().to_string();
    ticket = format!("{base}&a=127.0.0.1:{port}");
    let id = a.node.send_files_to_code(ticket, vec![item(&f)]).unwrap();
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Completed);
    let got = b.downloads().join("doc.pdf");
    assert_eq!(warp::hashing::hash_file(&got).unwrap(), warp::hashing::hash_file(&f).unwrap());
    let m = qr_matrix(b.node.current_code().qr_payload).unwrap();
    assert!(m.size >= 21 && m.modules.len() == (m.size * m.size) as usize);
}

#[test]
fn wrong_code_is_rejected_and_code_rotates_after_three_failures() {
    let a = TestNode::new("Attacker", |_| {});
    let b = TestNode::new("Victim", |_| {});
    a.see(&b);
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("x.txt");
    std::fs::write(&f, "x").unwrap();
    let real = b.node.current_code().code;
    let wrong = if real == "000000" { "000001" } else { "000000" };
    for i in 0..3 {
        let id = a.node.send_files_to_code(wrong.into(), vec![item(&f)]).unwrap();
        let t = a.wait_transfer(&id, T);
        assert_eq!(t.state, TransferState::Failed);
        assert!(t.error.unwrap().contains("wrong code"), "attempt {i}");
    }
    wait_until(Duration::from_secs(5), || b.node.current_code().code != real);
    assert!(b.node.transfers().is_empty(), "nothing may reach the victim");
}

/// Trust ring: link once with a code, then send with zero prompts; ring
/// membership propagates to the user's other devices.
#[test]
fn link_my_device_then_zero_prompt_send_and_ring_gossip() {
    let mac = TestNode::new("MacBook", |_| {});
    let android = TestNode::new("Pixel", |c| c.device_kind = DeviceKind::Android);
    let linux = TestNode::new("ThinkPad", |c| c.device_kind = DeviceKind::Linux);

    // Linking needs the receiver to open its link window explicitly.
    android.see(&mac);
    android.node.link_with_code(mac.node.current_code().code).unwrap();
    let err = android.find_event(T, |e| match e {
        WarpEvent::Error { message } => Some(message.clone()),
        _ => None,
    });
    assert!(err.contains("link_not_open"), "{err}");

    mac.node.open_link_window(60);
    android.node.link_with_code(mac.node.current_code().code).unwrap();
    let linked = android.find_event(T, |e| match e {
        WarpEvent::DeviceLinked { device } => Some(device.clone()),
        _ => None,
    });
    assert_eq!(linked.name, "MacBook");
    assert_eq!(linked.origin, TrustOrigin::Linked);
    wait_until(T, || mac.node.trusted_devices().iter().any(|d| d.name == "Pixel"));

    // Zero-prompt send between linked devices.
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("song.flac");
    random_file(&f, 5_000_000);
    let id = android.node.send_files(mac.id(), vec![item(&f)]).unwrap();
    assert_eq!(android.wait_transfer(&id, T).state, TransferState::Completed);
    assert!(mac.events.events.lock().iter().all(|e| !matches!(e, WarpEvent::OfferReceived { .. })));

    // Linux links with the Mac; the Pixel learns about it through ring sync.
    linux.see(&mac);
    mac.node.open_link_window(60);
    linux.node.link_with_code(mac.node.current_code().code).unwrap();
    wait_until(T, || mac.node.trusted_devices().iter().any(|d| d.name == "ThinkPad"));
    mac.see(&android);
    android.see(&mac);
    wait_until(T, || {
        android
            .node
            .trusted_devices()
            .iter()
            .any(|d| d.name == "ThinkPad" && d.origin == TrustOrigin::Ring)
    });
}

#[test]
fn strangers_must_be_accepted_and_see_matching_sas() {
    let a = TestNode::new("Guest", |_| {});
    let b = TestNode::new("Host", |_| {});
    a.see(&b);
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("photo.heic");
    random_file(&f, 50_000);

    // Declined.
    let id = a.node.send_files(b.id(), vec![item(&f)]).unwrap();
    let offer = b.find_event(T, |e| match e {
        WarpEvent::OfferReceived { offer } => Some(offer.clone()),
        _ => None,
    });
    assert_eq!(offer.sas_emoji.len(), 4);
    assert_eq!(offer.title, "photo.heic");
    b.node.respond_to_offer(offer.offer_id.clone(), false);
    let t = a.wait_transfer(&id, T);
    assert_eq!(t.state, TransferState::Failed);
    assert!(t.error.unwrap().contains("declined"));

    // Accepted.
    b.events.events.lock().clear();
    let id = a.node.send_files(b.id(), vec![item(&f)]).unwrap();
    let offer = b.find_event(T, |e| match e {
        WarpEvent::OfferReceived { offer } => Some(offer.clone()),
        _ => None,
    });
    b.node.respond_to_offer(offer.offer_id, true);
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Completed);
    assert!(b.downloads().join("photo.heic").exists());
}

#[test]
fn clipboard_push_text_and_image() {
    let a = TestNode::new("Desk", |_| {});
    let b = TestNode::new("Phone", |_| {});
    a.see(&b);
    let id = a
        .node
        .send_clipboard_to_code(b.node.current_code().code, ClipboardContent::Text { text: "https://warpdrop.app ✨".into() })
        .unwrap();
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Completed);
    let got = b.find_event(T, |e| match e {
        WarpEvent::ClipboardReceived { content, from_name, .. } => Some((content.clone(), from_name.clone())),
        _ => None,
    });
    assert_eq!(got.0, ClipboardContent::Text { text: "https://warpdrop.app ✨".into() });
    assert_eq!(got.1, "Desk");

    // Strangers cannot push to the clipboard without a code or trust.
    let id = a
        .node
        .send_clipboard(b.id(), ClipboardContent::Image { mime: "image/png".into(), data: vec![1, 2, 3] })
        .unwrap();
    // `a` holds a resume token from the code session, so this is allowed…
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Completed);
    // …but a fresh stranger is refused.
    let c = TestNode::new("Stranger", |_| {});
    c.see(&b);
    let id = c.node.send_clipboard(b.id(), ClipboardContent::Text { text: "spam".into() }).unwrap();
    let t = c.wait_transfer(&id, T);
    assert_eq!(t.state, TransferState::Failed);
    assert!(t.error.unwrap().contains("requires_code"));
}

/// Internet path through the rendezvous server: hole punching first,
/// relay when forced. Same QUIC/TLS/PAKE stack either way.
#[test]
fn internet_rendezvous_direct_and_relayed() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let relay = rt.block_on(warp_signal::spawn_local()).unwrap();
    let url = format!("ws://{relay}");

    for force_relay in [false, true] {
        let u = url.clone();
        let a = TestNode::new("Roma", |c| {
            c.relay_url = Some(u.clone());
            c.force_relay = force_relay;
        });
        let b = TestNode::new("Milano", |c| {
            c.relay_url = Some(u.clone());
            c.force_relay = force_relay;
        });
        std::thread::sleep(Duration::from_millis(800)); // b registers its channel
        let src = tempfile::tempdir().unwrap();
        let tree = make_tree(src.path());
        let before = tree_digest(&tree);
        // No discovery at all: only the code.
        let id = a.node.send_files_to_code(b.node.current_code().code, vec![item(&tree)]).unwrap();
        let t = a.wait_transfer(&id, Duration::from_secs(90));
        assert_eq!(t.state, TransferState::Completed, "force_relay={force_relay}: {:?}", t.error);
        if force_relay {
            assert_eq!(t.path, Some(PathKind::Relay));
        } else {
            assert_ne!(t.path, Some(PathKind::Relay));
        }
        let inc = b.wait_incoming_done(T);
        assert_eq!(tree_digest(std::path::Path::new(&inc.saved_paths[0])), before);
    }

    // Unknown code → CodeNotFound.
    let a = TestNode::new("Solo", |c| c.relay_url = Some(url.clone()));
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("a.txt");
    std::fs::write(&f, "a").unwrap();
    let id = a.node.send_files_to_code("999-999".into(), vec![item(&f)]).unwrap();
    let t = a.wait_transfer(&id, T);
    assert_eq!(t.state, TransferState::Failed);
    drop(rt);
}

/// Tier 0 cable path for Android: QUIC carried over a TCP tunnel, exactly as
/// `adb forward` provides it.
#[test]
fn usb_adb_style_tunnel_transfer() {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let android = TestNode::new("Galaxy", |c| {
        c.device_kind = DeviceKind::Android;
        c.tunnel_listen_port = Some(port);
    });
    let pc = TestNode::new("Windows PC", |c| c.device_kind = DeviceKind::Windows);
    std::thread::sleep(Duration::from_millis(300));
    pc.node.add_tunnel(port);
    let aid = android.id();
    wait_until(T, || pc.node.peers().iter().any(|p| p.device_id == aid));
    let p = pc.node.peers().into_iter().find(|p| p.device_id == aid).unwrap();
    assert_eq!(p.best_path, Some(PathKind::AdbUsb));
    assert_eq!(p.kind, DeviceKind::Android);

    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("build.apk");
    random_file(&f, 6 * 1024 * 1024 + 5);
    android.node.open_link_window(30);
    let id = pc.node.send_files(aid, vec![item(&f)]).unwrap();
    let offer = android.find_event(T, |e| match e {
        WarpEvent::OfferReceived { offer } => Some(offer.clone()),
        _ => None,
    });
    android.node.respond_to_offer(offer.offer_id, true);
    let t = pc.wait_transfer(&id, T);
    assert_eq!(t.state, TransferState::Completed, "{:?}", t.error);
    assert_eq!(t.path, Some(PathKind::AdbUsb));
    let got = android.downloads().join("build.apk");
    assert_eq!(warp::hashing::hash_file(&got).unwrap(), warp::hashing::hash_file(&f).unwrap());
}

#[test]
fn pause_resume_and_cancel() {
    let a = TestNode::new("A", |_| {});
    let b = TestNode::new("B", |_| {});
    a.see(&b);
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("big.bin");
    random_file(&f, 24 * 1024 * 1024);

    // Paused before any byte moves: nothing is sent until resumed.
    let id = a.node.send_files_to_code(b.node.current_code().code, vec![item(&f)]).unwrap();
    a.node.pause_transfer(id.clone()).unwrap();
    wait_until(T, || a.node.transfers().iter().any(|t| t.id == id && t.state == TransferState::Paused && t.total_bytes > 0));
    std::thread::sleep(Duration::from_millis(700));
    let p = a.node.transfers().into_iter().find(|t| t.id == id).unwrap();
    assert_eq!(p.state, TransferState::Paused);
    assert_eq!(p.done_bytes, 0, "data moved while paused");
    a.node.resume_transfer(id.clone()).unwrap();
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Completed);

    // Cancel while paused.
    let id = a.node.send_files_to_code(b.node.current_code().code, vec![item(&f)]).unwrap();
    a.node.pause_transfer(id.clone()).unwrap();
    wait_until(T, || a.node.transfers().iter().any(|t| t.id == id && t.state == TransferState::Paused && t.total_bytes > 0));
    a.node.cancel_transfer(id.clone()).unwrap();
    assert_eq!(a.wait_transfer(&id, T).state, TransferState::Cancelled);
}

/// Several WarpDrop devices nearby, the intended one discovered late: the
/// "wrong code" answers from the others must not end the search early.
#[test]
fn code_finds_late_discovered_device_among_decoys() {
    let a = TestNode::new("Sender", |_| {});
    let d1 = TestNode::new("Decoy 1", |_| {});
    let d2 = TestNode::new("Decoy 2", |_| {});
    let b = TestNode::new("Target", |_| {});
    a.see(&d1);
    a.see(&d2);
    let src = tempfile::tempdir().unwrap();
    let f = src.path().join("late.txt");
    std::fs::write(&f, "found you").unwrap();
    let code = b.node.current_code().code;
    let id = a.node.send_files_to_code(code, vec![item(&f)]).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    a.see(&b); // discovered only now
    let t = a.wait_transfer(&id, T);
    assert_eq!(t.state, TransferState::Completed, "{:?}", t.error);
    assert_eq!(std::fs::read_to_string(b.downloads().join("late.txt")).unwrap(), "found you");
    // Each decoy was asked exactly once, so their codes did not rotate.
    let (c1, c2) = (d1.node.current_code().code, d2.node.current_code().code);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(d1.node.current_code().code, c1);
    assert_eq!(d2.node.current_code().code, c2);
}
