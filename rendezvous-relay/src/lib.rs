//! WarpDrop rendezvous server.
//!
//! *Zero knowledge*: the server sees Argon2id channel ids (never codes),
//! public UDP addresses, and QUIC packets whose TLS 1.3 keys it never holds.
//! *Stateless*: everything lives in memory with short TTLs; restart = clean.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const MAGIC: &[u8; 4] = b"WRP1";
const PROBE: u8 = 1;
const PROBE_ACK: u8 = 2;
const DATA: u8 = 3;
const HDR: usize = 22;

#[derive(Clone, Debug)]
pub struct Config {
    /// Per-session relay byte budget.
    pub max_session_bytes: u64,
    /// Sessions idle longer than this are dropped.
    pub session_idle: Duration,
    /// Maximum persistent (resume-ticket) channels per WebSocket, in
    /// addition to its one rotating code channel.
    pub max_channels_per_conn: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_session_bytes: 64 * 1024 * 1024 * 1024,
            session_idle: Duration::from_secs(600),
            max_channels_per_conn: 64,
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let mut c = Config::default();
        if let Some(gb) = std::env::var("WARP_SIGNAL_MAX_SESSION_GB").ok().and_then(|v| v.parse::<u64>().ok()) {
            c.max_session_bytes = gb * 1024 * 1024 * 1024;
        }
        c
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidates {
    pub public: Option<SocketAddr>,
    pub local: Vec<SocketAddr>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ClientMsg {
    /// `persistent = false`: the client's rotating code channel (replaces the
    /// previous one). `persistent = true`: an extra resume-ticket channel.
    Listen {
        channel: String,
        #[serde(default)]
        persistent: bool,
    },
    Unlisten { channel: String },
    Connect { channel: String },
    Signal { session: String, data: Candidates },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ServerMsg {
    Paired { session: String, side: u8, udp_port: u16 },
    Signal { session: String, data: Candidates },
    Error { code: String },
}

type Tx = mpsc::UnboundedSender<ServerMsg>;

struct Session {
    ws: [Option<(u64, Tx)>; 2],
    relay: [Option<SocketAddr>; 2],
    bytes: u64,
    last: Instant,
}

struct Shared {
    cfg: Config,
    udp_port: u16,
    listeners: Mutex<HashMap<String, (u64, Tx)>>,
    sessions: Mutex<HashMap<[u8; 16], Session>>,
    next_conn: AtomicU64,
}

pub async fn serve(tcp: tokio::net::TcpListener, udp: tokio::net::UdpSocket, cfg: Config) -> anyhow::Result<()> {
    let shared = Arc::new(Shared {
        cfg,
        udp_port: udp.local_addr()?.port(),
        listeners: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        next_conn: AtomicU64::new(1),
    });
    let udp = Arc::new(udp);
    tokio::spawn(udp_loop(udp, shared.clone()));
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs(30));
            loop {
                t.tick().await;
                let idle = shared.cfg.session_idle;
                shared.sessions.lock().retain(|_, s| s.last.elapsed() < idle);
            }
        });
    }
    let app = Router::new()
        .route("/v1/ws", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(shared);
    axum::serve(tcp, app).await?;
    Ok(())
}

/// Binds TCP and UDP on the same free loopback port; for tests and local dev.
pub async fn spawn_local() -> anyhow::Result<SocketAddr> {
    for _ in 0..20 {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = tcp.local_addr()?;
        if let Ok(udp) = tokio::net::UdpSocket::bind(addr).await {
            tokio::spawn(async move {
                if let Err(e) = serve(tcp, udp, Config::default()).await {
                    tracing::error!("warp-signal stopped: {e:#}");
                }
            });
            return Ok(addr);
        }
    }
    anyhow::bail!("could not bind a matching TCP/UDP port")
}

async fn ws_handler(ws: WebSocketUpgrade, State(s): State<Arc<Shared>>) -> impl IntoResponse {
    ws.max_message_size(16 * 1024).on_upgrade(move |sock| handle_ws(sock, s))
}

async fn handle_ws(sock: WebSocket, s: Arc<Shared>) {
    let id = s.next_conn.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = sock.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let text = serde_json::to_string(&m).unwrap_or_default();
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });
    let mut code_channel: Option<String> = None;
    let mut my_channels: Vec<String> = vec![];
    while let Some(Ok(msg)) = stream.next().await {
        let Message::Text(text) = msg else { continue };
        let Ok(m) = serde_json::from_str::<ClientMsg>(text.as_str()) else {
            let _ = tx.send(ServerMsg::Error { code: "bad_message".into() });
            continue;
        };
        match m {
            ClientMsg::Listen { channel, persistent } => {
                if channel.len() != 64 || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
                    let _ = tx.send(ServerMsg::Error { code: "bad_channel".into() });
                    continue;
                }
                let mut l = s.listeners.lock();
                if persistent {
                    if my_channels.contains(&channel) {
                        continue;
                    }
                    while my_channels.len() >= s.cfg.max_channels_per_conn {
                        let old = my_channels.remove(0);
                        remove_if_mine(&mut l, &old, id);
                    }
                    my_channels.push(channel.clone());
                } else {
                    // A rotated code replaces this client's previous code.
                    if let Some(old) = code_channel.replace(channel.clone()) {
                        remove_if_mine(&mut l, &old, id);
                    }
                }
                l.insert(channel, (id, tx.clone()));
            }
            ClientMsg::Unlisten { channel } => {
                my_channels.retain(|c| c != &channel);
                remove_if_mine(&mut s.listeners.lock(), &channel, id);
            }
            ClientMsg::Connect { channel } => {
                let listener = s.listeners.lock().get(&channel).cloned();
                let Some((lid, ltx)) = listener else {
                    let _ = tx.send(ServerMsg::Error { code: "no_listener".into() });
                    continue;
                };
                let sid: [u8; 16] = rand::random();
                let hex_id = hex::encode(sid);
                s.sessions.lock().insert(
                    sid,
                    Session { ws: [Some((lid, ltx.clone())), Some((id, tx.clone()))], relay: [None, None], bytes: 0, last: Instant::now() },
                );
                let _ = ltx.send(ServerMsg::Paired { session: hex_id.clone(), side: 0, udp_port: s.udp_port });
                let _ = tx.send(ServerMsg::Paired { session: hex_id, side: 1, udp_port: s.udp_port });
            }
            ClientMsg::Signal { session, data } => {
                let Some(sid) = hex::decode(&session).ok().and_then(|v| <[u8; 16]>::try_from(v).ok()) else { continue };
                let sessions = s.sessions.lock();
                let Some(sess) = sessions.get(&sid) else { continue };
                let Some(side) = sess.ws.iter().position(|w| matches!(w, Some((cid, _)) if *cid == id)) else { continue };
                if let Some((_, other)) = &sess.ws[1 - side] {
                    let _ = other.send(ServerMsg::Signal { session, data });
                }
            }
        }
    }
    let mut l = s.listeners.lock();
    for c in my_channels.iter().chain(code_channel.iter()) {
        remove_if_mine(&mut l, c, id);
    }
    drop(l);
    writer.abort();
}

fn remove_if_mine(l: &mut HashMap<String, (u64, Tx)>, channel: &str, conn: u64) {
    if matches!(l.get(channel), Some((cid, _)) if *cid == conn) {
        l.remove(channel);
    }
}

async fn udp_loop(sock: Arc<tokio::net::UdpSocket>, s: Arc<Shared>) {
    let mut buf = vec![0u8; 65536];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
        if n < HDR || &buf[..4] != MAGIC {
            continue;
        }
        let kind = buf[4];
        let Ok(sid) = <[u8; 16]>::try_from(&buf[5..21]) else { continue };
        let side = buf[21];
        if side > 1 {
            continue;
        }
        match kind {
            PROBE => {
                if !s.sessions.lock().contains_key(&sid) {
                    continue;
                }
                let mut reply = buf[..HDR].to_vec();
                reply[4] = PROBE_ACK;
                reply.extend_from_slice(from.to_string().as_bytes());
                let _ = sock.send_to(&reply, from).await;
            }
            DATA => {
                let target = {
                    let mut sessions = s.sessions.lock();
                    let Some(sess) = sessions.get_mut(&sid) else { continue };
                    sess.relay[side as usize] = Some(from);
                    sess.last = Instant::now();
                    if n == HDR {
                        continue; // bind only
                    }
                    sess.bytes += n as u64;
                    if sess.bytes > s.cfg.max_session_bytes {
                        continue;
                    }
                    sess.relay[1 - side as usize]
                };
                if let Some(to) = target {
                    let _ = sock.send_to(&buf[..n], to).await;
                }
            }
            _ => {}
        }
    }
}
