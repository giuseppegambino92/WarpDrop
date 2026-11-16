//! Hand-off into the download folder failing halfway, then the same items
//! being sent again: nothing is lost, nothing is saved twice, and what is
//! already safely saved is not sent again.
//!
//! Own test binary: uses a process-global debug hook. Each scenario takes
//! the lock so they never overlap.

mod common;

use common::*;
use parking_lot::Mutex;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use warp::*;

static HOOK: Mutex<()> = Mutex::new(());

const MIB: usize = 1024 * 1024;

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

fn digest(p: &Path) -> String {
    hex::encode(warp::hashing::hash_file(p).unwrap())
}

/// Sends `items` from `a` to `b` by code and waits for both sides.
fn send(a: &TestNode, b: &TestNode, items: &[PathBuf]) -> (TransferSnapshot, TransferSnapshot) {
    let before: Vec<String> = b.node.transfers().into_iter().map(|t| t.id).collect();
    let id = a.node.send_files_to_code(b.node.current_code().code, items.iter().map(PathBuf::as_path).map(item).collect()).unwrap();
    let out = a.wait_transfer(&id, T);
    let mut inc = None;
    wait_until(T, || {
        inc = b
            .node
            .transfers()
            .into_iter()
            .find(|t| t.direction == TransferDirection::Incoming && t.state.is_terminal() && !before.contains(&t.id));
        inc.is_some()
    });
    (out, inc.unwrap())
}

struct Setup {
    a: TestNode,
    b: TestNode,
    _src: tempfile::TempDir,
    items: Vec<PathBuf>,
}

fn setup() -> Setup {
    let a = TestNode::new("Sender", |_| {});
    let b = TestNode::new("Receiver", |_| {});
    a.see(&b);
    let src = tempfile::tempdir().unwrap();
    let mut items = vec![];
    for (name, size) in [("alpha.bin", 6 * MIB + 1), ("beta.bin", 5 * MIB), ("gamma.bin", 1234)] {
        let p = src.path().join(name);
        random_file(&p, size);
        items.push(p);
    }
    Setup { a, b, _src: src, items }
}

/// First attempt: "beta" cannot be moved in and rolling "alpha" back fails,
/// so alpha is left in Downloads.
fn failed_first_attempt(s: &Setup) {
    warp::receiver::DEBUG_PLACE_FAULT.store(true, Ordering::SeqCst);
    let (out, inc) = send(&s.a, &s.b, &s.items);
    assert_eq!(out.state, TransferState::Failed);
    assert_eq!(inc.state, TransferState::Failed);
    let err = inc.error.unwrap_or_default();
    assert!(err.contains("cannot save beta.bin") && err.contains("alpha.bin"), "{err}");
    assert_eq!(names(&s.b.downloads()), ["alpha.bin"]);
}

#[test]
fn retry_after_a_failed_rollback_does_not_duplicate_or_resend() {
    let _g = HOOK.lock();
    let s = setup();
    failed_first_attempt(&s);

    let (out, inc) = send(&s.a, &s.b, &s.items);
    assert_eq!(out.state, TransferState::Completed, "{:?}", out.error);
    assert_eq!(inc.state, TransferState::Completed, "{:?}", inc.error);
    // Exactly one copy of each item, and alpha is reported where it already was.
    assert_eq!(names(&s.b.downloads()), ["alpha.bin", "beta.bin", "gamma.bin"]);
    let dl = s.b.downloads();
    assert_eq!(inc.saved_paths, ["alpha.bin", "beta.bin", "gamma.bin"].map(|n| dl.join(n).to_string_lossy().into_owned()));
    for p in &s.items {
        assert_eq!(digest(&dl.join(p.file_name().unwrap())), digest(p));
    }
    // Everything was already verified on disk: no file data crossed the wire again.
    assert!(inc.wire_bytes < 64 * 1024, "re-sent {} bytes", inc.wire_bytes);
    // A retry of the same, finished transfer (e.g. our "done" was lost) is
    // answered from the completed staging area: no third copy.
    let (out, inc) = send(&s.a, &s.b, &s.items);
    assert_eq!((out.state, inc.state), (TransferState::Completed, TransferState::Completed));
    assert_eq!(names(&s.b.downloads()), ["alpha.bin", "beta.bin", "gamma.bin"]);
    assert!(inc.wire_bytes < 64 * 1024);
    let leftover: Vec<_> = walkdir::WalkDir::new(dl.join(".warpdrop-partial"))
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file() && !e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftover.is_empty(), "no item data left in staging: {leftover:?}");
}

#[test]
fn an_item_edited_after_a_failed_rollback_is_kept_and_received_again() {
    let _g = HOOK.lock();
    let s = setup();
    failed_first_attempt(&s);
    // The user edits the left-over copy before the retry.
    let leftover = s.b.downloads().join("alpha.bin");
    std::fs::write(&leftover, b"my edits").unwrap();

    let (out, inc) = send(&s.a, &s.b, &s.items);
    assert_eq!(out.state, TransferState::Completed, "{:?}", out.error);
    assert_eq!(inc.state, TransferState::Completed, "{:?}", inc.error);
    // The edited file is never written into; a fresh copy lands next to it.
    assert_eq!(std::fs::read(&leftover).unwrap(), b"my edits");
    assert_eq!(names(&s.b.downloads()), ["alpha (2).bin", "alpha.bin", "beta.bin", "gamma.bin"]);
    assert_eq!(digest(&s.b.downloads().join("alpha (2).bin")), digest(&s.items[0]));
    // Only alpha travels again (beta and gamma stayed staged).
    assert!(inc.wire_bytes < 6 * MIB as u64 + 256 * 1024, "re-sent {} bytes", inc.wire_bytes);
}

#[test]
fn transfers_over_the_memory_budget_are_declined_up_front() {
    let a = TestNode::new("Sender", |_| {});
    // 1 MiB budget: 7 943 chunks of state, 768 entries.
    let b = TestNode::new("Small phone", |c| c.transfer_memory_mb = 1);
    a.see(&b);
    let src = tempfile::tempdir().unwrap();
    let dir = src.path().join("many");
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..800 {
        std::fs::write(dir.join(format!("{i}.txt")), b"x").unwrap();
    }
    let id = a.node.send_files_to_code(b.node.current_code().code, vec![item(&dir)]).unwrap();
    let out = a.wait_transfer(&id, T);
    assert_eq!(out.state, TransferState::Failed);
    assert!(out.error.unwrap_or_default().contains("too_large"));
    assert!(names(&b.downloads()).is_empty());
}

#[test]
fn items_changed_after_a_completed_transfer_are_received_again_next_to_them() {
    let _g = HOOK.lock();
    let mut s = setup();
    let album = s._src.path().join("album");
    std::fs::create_dir_all(album.join("sub")).unwrap();
    std::fs::write(album.join("a.txt"), b"photo").unwrap();
    std::fs::write(album.join("empty.txt"), b"").unwrap();
    s.items.push(album);
    let (out, inc) = send(&s.a, &s.b, &s.items);
    assert_eq!((out.state, inc.state), (TransferState::Completed, TransferState::Completed), "{:?}", inc.error);
    let dl = s.b.downloads();

    // Within minutes, the user changes what was received, then the sender
    // sends the same items again.
    let edited = dl.join("alpha.bin");
    let mut bytes = std::fs::read(&edited).unwrap();
    bytes[0] ^= 1; // same size, different content
    std::fs::write(&edited, &bytes).unwrap();
    let grown = dl.join("beta.bin");
    std::fs::OpenOptions::new().append(true).open(&grown).unwrap().write_all(b"notes").unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::rename(dl.join("gamma.bin"), elsewhere.path().join("gamma.bin")).unwrap();
    std::fs::remove_file(dl.join("album/empty.txt")).unwrap();
    let mine = |p: &Path| (std::fs::metadata(p).unwrap().len(), digest(p));
    let before = [mine(&edited), mine(&grown), mine(&elsewhere.path().join("gamma.bin")), mine(&dl.join("album/a.txt"))];

    let (out, inc) = send(&s.a, &s.b, &s.items);
    assert_eq!((out.state, inc.state), (TransferState::Completed, TransferState::Completed), "{:?}", inc.error);
    // Nothing the user changed is written into; each changed item arrives as
    // a fresh, complete copy (at its old name if that is free again).
    assert_eq!(names(&dl), ["album", "album (2)", "alpha (2).bin", "alpha.bin", "beta (2).bin", "beta.bin", "gamma.bin"]);
    let after = [mine(&edited), mine(&grown), mine(&elsewhere.path().join("gamma.bin")), mine(&dl.join("album/a.txt"))];
    assert_eq!(before, after, "the user's files are untouched");
    assert!(!dl.join("album/empty.txt").exists());
    for (copy, orig) in [("alpha (2).bin", &s.items[0]), ("beta (2).bin", &s.items[1]), ("gamma.bin", &s.items[2])] {
        assert_eq!(digest(&dl.join(copy)), digest(orig), "{copy}");
    }
    for f in ["a.txt", "empty.txt"] {
        assert_eq!(std::fs::read(dl.join("album (2)").join(f)).unwrap(), std::fs::read(s.items[3].join(f)).unwrap());
    }
    assert!(dl.join("album (2)/sub").is_dir());
    assert_eq!(
        inc.saved_paths,
        ["alpha (2).bin", "beta (2).bin", "gamma.bin", "album (2)"].map(|n| dl.join(n).to_string_lossy().into_owned())
    );
}
