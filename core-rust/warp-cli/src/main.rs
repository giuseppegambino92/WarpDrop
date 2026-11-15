//! `warp` — headless WarpDrop node.
//!
//! ```text
//! warp receive [--dir DIR] [--relay URL]          show a code and accept incoming transfers
//! warp send <CODE|QR-URI|DEVICE-ID> <PATH>... [--relay URL]
//! warp clip <CODE|DEVICE-ID> <TEXT> [--relay URL]
//! warp peers                                      list devices on the LAN
//! warp ifaces                                     show local links and their tier
//! warp hash <FILE>                                BLAKE3 (same as b3sum)
//!
//! Env: WARP_STATE (identity/trust dir), WARP_RELAY (relay URL),
//!      WARP_NAME (device name), WARP_KIND (mac|iphone|ipad|android|windows|linux).
//! ```

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use warp::*;

struct Listener(mpsc::Sender<WarpEvent>);
impl WarpEventListener for Listener {
    fn on_event(&self, e: WarpEvent) {
        let _ = self.0.send(e);
    }
}

fn human(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

fn badge(p: Option<PathKind>) -> &'static str {
    match p {
        Some(PathKind::UsbCable | PathKind::AdbUsb) => "⚡ USB cable",
        Some(PathKind::ThunderboltBridge) => "⚡ Thunderbolt",
        Some(PathKind::WifiDirect) => "Wi-Fi Direct",
        Some(PathKind::Ethernet) => "Ethernet",
        Some(PathKind::Wifi) => "Wi-Fi",
        Some(PathKind::Lan) => "LAN",
        Some(PathKind::Loopback) => "this machine",
        Some(PathKind::InternetP2p) => "☁️ P2P",
        Some(PathKind::Relay) => "☁️ Relay",
        None => "…",
    }
}

fn progress(t: &TransferSnapshot) {
    let pct = (t.done_bytes * 100).checked_div(t.total_bytes).unwrap_or(0);
    eprint!(
        "\r{:>3}%  {} / {}  {}/s  {}  [{}]          ",
        pct,
        human(t.done_bytes),
        human(t.total_bytes),
        human(t.speed_bps),
        t.eta_secs.map(|s| format!("ETA {s}s")).unwrap_or_default(),
        badge(t.path)
    );
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let take_flag = |args: &mut Vec<String>, name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.remove(i);
        (i < args.len()).then(|| args.remove(i))
    };
    let relay = take_flag(&mut args, "--relay").or_else(|| std::env::var("WARP_RELAY").ok());
    let dir = take_flag(&mut args, "--dir");
    let cmd = args.first().cloned().unwrap_or_default();

    if cmd == "hash" {
        let f = args.get(1).ok_or_else(|| anyhow::anyhow!("usage: warp hash FILE"))?;
        println!("{}  {f}", hex(&hashing::hash_file(std::path::Path::new(f))?));
        return Ok(());
    }

    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
    let state = std::env::var("WARP_STATE").unwrap_or_else(|_| format!("{home}/.warpdrop-cli"));
    let downloads = dir.unwrap_or_else(|| format!("{home}/Downloads/WarpDrop"));
    let host = std::env::var("WARP_NAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "warp-cli".into());
    let mut cfg = default_node_config(host, state, downloads);
    if let Ok(k) = std::env::var("WARP_KIND") {
        cfg.device_kind = DeviceKind::from_tag(&k);
    }
    cfg.relay_url = relay;
    // USB to Android via adb (Tier 0) whenever we look for or address devices.
    cfg.enable_adb = matches!(cmd.as_str(), "peers" | "send" | "clip");
    let (tx, rx) = mpsc::channel();
    let node = WarpNode::new(cfg, Arc::new(Listener(tx)))?;

    match cmd.as_str() {
        "receive" => {
            let c = node.current_code();
            println!("Your code: {}   ({})", c.display, node.identity().fingerprint);
            println!("QR: {}", c.qr_payload);
            println!("Saving to {}", node.download_dir());
            for e in rx {
                match e {
                    WarpEvent::CodeChanged { code } => println!("\nYour code: {}", code.display),
                    WarpEvent::OfferReceived { offer } => {
                        println!("\n{} wants to send {} ({}) — SAS {}", offer.peer_name, offer.title, human(offer.total_bytes), offer.sas_emoji.join(" "));
                        node.respond_to_offer(offer.offer_id, true);
                    }
                    WarpEvent::TransferUpdated { transfer: t } if t.direction == TransferDirection::Incoming => match t.state {
                        TransferState::Completed => println!("\n✓ saved {}", t.saved_paths.join(", ")),
                        TransferState::Failed => println!("\n✗ {}", t.error.unwrap_or_default()),
                        _ => progress(&t),
                    },
                    WarpEvent::ClipboardReceived { from_name, content, .. } => println!("\n📋 from {from_name}: {content:?}"),
                    _ => {}
                }
            }
        }
        "send" | "clip" => {
            let target = args.get(1).ok_or_else(|| anyhow::anyhow!("missing target"))?.clone();
            let is_code = normalize_code(target.clone()).is_some();
            if !is_code {
                // Addressed by device id: wait for discovery (mDNS, beacon, adb).
                for _ in 0..80 {
                    if node.peers().iter().any(|p| p.device_id == target) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let id = if cmd == "clip" {
                let text = args[2..].join(" ");
                let content = ClipboardContent::Text { text };
                if is_code { node.send_clipboard_to_code(target, content)? } else { node.send_clipboard(target, content)? }
            } else {
                let items: Vec<SendItem> = args[2..].iter().map(|p| SendItem { path: p.clone(), name: None }).collect();
                if is_code { node.send_files_to_code(target, items)? } else { node.send_files(target, items)? }
            };
            for e in rx {
                if let WarpEvent::TransferUpdated { transfer: t } = e {
                    if t.id != id {
                        continue;
                    }
                    match t.state {
                        TransferState::Completed => {
                            println!("\n✓ sent {} to {} via {}", human(t.total_bytes), t.peer_name, badge(t.path));
                            break;
                        }
                        TransferState::Failed | TransferState::Cancelled => {
                            println!("\n✗ {}", t.error.unwrap_or_else(|| "cancelled".into()));
                            std::process::exit(1);
                        }
                        TransferState::WaitingForAccept => eprint!("\rwaiting for the other device to accept…"),
                        TransferState::Reconnecting => eprint!("\rreconnecting…                         "),
                        _ => progress(&t),
                    }
                }
            }
        }
        "peers" => {
            std::thread::sleep(Duration::from_secs(4));
            for p in node.peers() {
                println!("{:<18} {:<28} {:?}  {}  {}", p.device_id, p.name, p.kind, badge(p.best_path), if p.trusted { "✓ trusted" } else { "" });
            }
        }
        "ifaces" => {
            for i in node.interfaces() {
                println!("{:<12} {:<40} {}", i.name, i.address, badge(Some(i.kind)));
            }
        }
        _ => {
            eprintln!("usage: warp receive | send <code|device> <paths…> | clip <code|device> <text> | peers | ifaces | hash <file>");
            std::process::exit(2);
        }
    }
    node.shutdown();
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
