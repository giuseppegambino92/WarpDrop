//! Loopback throughput benchmark: `cargo run --release -p libwarp --example bench -- 1024 [streams]`
use std::sync::Arc;
use std::time::Instant;
use warp::*;

struct Nop;
impl WarpEventListener for Nop {
    fn on_event(&self, _e: WarpEvent) {}
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into())).init();
    let mb: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(512);
    let streams: u8 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(6);
    let dir = std::env::temp_dir().join(format!("warp-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("payload.bin");
    let mut data = vec![0u8; mb * 1024 * 1024];
    let mut x: u64 = 0x9E3779B97F4A7C15;
    for b in data.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    std::fs::write(&file, &data).unwrap();
    drop(data);
    let mk = |n: &str| {
        let mut c = default_node_config(n.into(), dir.join(n).join("s").to_string_lossy().into(), dir.join(n).join("dl").to_string_lossy().into());
        c.enable_mdns = false;
        c.enable_beacon = false;
        c.parallel_streams = streams;
        WarpNode::new(c, Arc::new(Nop)).unwrap()
    };
    let a = mk("a");
    let b = mk("b");
    let ad = b.advertisement();
    a.inject_peer(PeerAdvert { txt: ad.txt, addresses: vec![format!("127.0.0.1:{}", ad.port)] });
    let t0 = Instant::now();
    let id = a
        .send_files_to_code(b.current_code().code, vec![SendItem { path: file.to_string_lossy().into(), name: None }])
        .unwrap();
    loop {
        let t = a.transfers().into_iter().find(|t| t.id == id).unwrap();
        if t.state == TransferState::Completed {
            break;
        }
        assert!(!t.state.is_terminal(), "{:?}", t.error);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let s = t0.elapsed().as_secs_f64();
    println!("{mb} MiB, {streams} streams: {:.2}s = {:.0} MB/s", s, (mb * 1024 * 1024) as f64 / s / 1e6);
    let _ = std::fs::remove_dir_all(&dir);
}
