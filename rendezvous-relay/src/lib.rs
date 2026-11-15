//! WarpDrop rendezvous server.
//!
//! *Zero knowledge*: the server sees Argon2id channel ids (never codes),
//! public UDP addresses, and QUIC packets whose TLS 1.3 keys it never holds.
//! *Stateless*: everything lives in memory with short TTLs; restart = clean.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const MAGIC: &[u8; 4] = b"WRP1";
const PROBE: u8 = 1;
const PROBE_ACK: u8 = 2;
const DATA: u8 = 3;
/// `header | keyed_hash(bind_key, BIND_CONTEXT | session | side | observed addr)`:
/// moves a side of the session to the sender's address. Only the client that
/// received `bind_key` over the (TLS) WebSocket can produce it, and the MAC
/// covers the source address, so a sniffed BIND cannot be replayed from
/// elsewhere.
const BIND: u8 = 4;
/// Sent back for DATA from an address that is not bound (e.g. after a NAT
/// rebinding): `header | observed addr`. The client answers with a BIND.
const NEED_BIND: u8 = 5;
const BIND_ACK: u8 = 6;
const HDR: usize = 22;
const MAC_LEN: usize = 32;
pub const BIND_CONTEXT: &[u8] = b"warpdrop relay bind v1";
/// Outgoing WebSocket messages queued per client before new ones are dropped.
const WS_QUEUE: usize = 64;

#[derive(Clone, Debug)]
pub struct Config {
    /// Per-session relay byte budget.
    pub max_session_bytes: u64,
    /// Sessions idle longer than this are dropped.
    pub session_idle: Duration,
    /// Sessions that never saw a UDP binding are dropped after this.
    pub pending_session_ttl: Duration,
    /// Maximum persistent (resume-ticket) channels per WebSocket, in
    /// addition to its one rotating code channel.
    pub max_channels_per_conn: usize,
    /// Global caps, so a flood degrades to "busy" instead of exhausting memory.
    pub max_sessions: usize,
    pub max_listeners: usize,
    /// Concurrent WebSockets per client IP.
    pub max_conns_per_ip: usize,
    /// `Connect` requests per WebSocket per minute (burst of 10).
    pub connects_per_minute: u32,
    /// WebSockets silent for longer than this (no message, no pong) are closed.
    pub ws_idle: Duration,
    /// Take the client IP from `X-Forwarded-For` (only behind a trusted proxy).
    pub trust_proxy: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_session_bytes: 64 * 1024 * 1024 * 1024,
            session_idle: Duration::from_secs(600),
            pending_session_ttl: Duration::from_secs(30),
            max_channels_per_conn: 64,
            max_sessions: 20_000,
            max_listeners: 100_000,
            max_conns_per_ip: 32,
            connects_per_minute: 30,
            ws_idle: Duration::from_secs(90),
            trust_proxy: false,
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let mut c = Config::default();
        let num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
        if let Some(gb) = num("WARP_SIGNAL_MAX_SESSION_GB") {
            c.max_session_bytes = gb * 1024 * 1024 * 1024;
        }
        if let Some(n) = num("WARP_SIGNAL_MAX_SESSIONS") {
            c.max_sessions = n as usize;
        }
        if let Some(n) = num("WARP_SIGNAL_MAX_CONNS_PER_IP") {
            c.max_conns_per_ip = n as usize;
        }
        c.trust_proxy = matches!(std::env::var("WARP_SIGNAL_TRUST_PROXY").as_deref(), Ok("1" | "true"));
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
    Paired { session: String, side: u8, udp_port: u16, bind_key: String },
    Signal { session: String, data: Candidates },
    Error {
        code: String,
        /// For `channel_busy`: which channel.
        #[serde(skip_serializing_if = "Option::is_none")]
        channel: Option<String>,
    },
}

type Tx = mpsc::Sender<ServerMsg>;

/// Queues a message for a client; drops it if that client is not keeping up.
fn push(tx: &Tx, m: ServerMsg) {
    let _ = tx.try_send(m);
}

struct Session {
    ws: [Option<(u64, Tx)>; 2],
    relay: [Option<SocketAddr>; 2],
    /// Per-side secret for authenticated (re)binding.
    bind_key: [[u8; 32]; 2],
    /// Once a side has bound with a MAC, only MAC-authenticated BINDs can move it.
    authed: [bool; 2],
    bytes: u64,
    created: Instant,
    last: Instant,
}

impl Session {
    fn bound(&self) -> bool {
        self.relay.iter().any(Option::is_some)
    }
}

pub fn bind_mac(key: &[u8; 32], session: &[u8; 16], side: u8, observed: &SocketAddr) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(BIND_CONTEXT);
    h.update(session);
    h.update(&[side]);
    h.update(observed.to_string().as_bytes());
    *h.finalize().as_bytes()
}

struct Shared {
    cfg: Config,
    udp_port: u16,
    listeners: Mutex<HashMap<String, (u64, Tx)>>,
    sessions: Mutex<HashMap<[u8; 16], Session>>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    next_conn: AtomicU64,
}

pub async fn serve(tcp: tokio::net::TcpListener, udp: tokio::net::UdpSocket, cfg: Config) -> anyhow::Result<()> {
    let shared = Arc::new(Shared {
        cfg,
        udp_port: udp.local_addr()?.port(),
        listeners: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        per_ip: Mutex::new(HashMap::new()),
        next_conn: AtomicU64::new(1),
    });
    let udp = Arc::new(udp);
    tokio::spawn(udp_loop(udp, shared.clone()));
    {
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut t = tokio::time::interval(Duration::from_secs(10));
            loop {
                t.tick().await;
                let (idle, pending) = (shared.cfg.session_idle, shared.cfg.pending_session_ttl);
                shared
                    .sessions
                    .lock()
                    .retain(|_, s| s.last.elapsed() < idle && (s.bound() || s.created.elapsed() < pending));
            }
        });
    }
    let app = Router::new()
        .route("/v1/ws", get(ws_handler))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(shared);
    axum::serve(tcp, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
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

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(s): State<Arc<Shared>>,
) -> axum::response::Response {
    let ip = client_ip(&s.cfg, peer, &headers);
    {
        let mut per_ip = s.per_ip.lock();
        let n = per_ip.entry(ip).or_insert(0);
        if *n >= s.cfg.max_conns_per_ip {
            return (StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response();
        }
        *n += 1;
    }
    let guard = IpSlot { shared: s.clone(), ip };
    ws.max_message_size(16 * 1024).on_upgrade(move |sock| async move {
        handle_ws(sock, s).await;
        drop(guard);
    })
}

fn client_ip(cfg: &Config, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if cfg.trust_proxy {
        let forwarded = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next_back())
            .and_then(|v| v.trim().parse().ok());
        if let Some(ip) = forwarded {
            return ip;
        }
    }
    peer.ip()
}

/// Releases a per-IP connection slot when the WebSocket ends (or never starts).
struct IpSlot {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut per_ip = self.shared.per_ip.lock();
        if let Some(n) = per_ip.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                per_ip.remove(&self.ip);
            }
        }
    }
}

/// Token bucket for `Connect` requests.
struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    const BURST: f64 = 10.0;
    fn take(&mut self, per_minute: u32) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * per_minute as f64 / 60.0).min(Self::BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

async fn handle_ws(sock: WebSocket, s: Arc<Shared>) {
    let id = s.next_conn.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = sock.split();
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(WS_QUEUE);
    let writer = tokio::spawn(async move {
        // Pings let both ends notice dead connections (and free their channels).
        let mut ping = tokio::time::interval(Duration::from_secs(30));
        ping.tick().await;
        loop {
            let msg = tokio::select! {
                m = rx.recv() => match m {
                    Some(m) => Message::Text(serde_json::to_string(&m).unwrap_or_default().into()),
                    None => break,
                },
                _ = ping.tick() => Message::Ping(Vec::new().into()),
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });
    let mut code_channel: Option<String> = None;
    let mut my_channels: Vec<String> = vec![];
    let mut my_sessions: Vec<[u8; 16]> = vec![];
    let mut bucket = Bucket { tokens: Bucket::BURST, at: Instant::now() };
    let err = |code: &str| push(&tx, ServerMsg::Error { code: code.into(), channel: None });
    loop {
        let msg = match tokio::time::timeout(s.cfg.ws_idle, stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            _ => break, // closed, failed, or silent for too long
        };
        let Message::Text(text) = msg else { continue };
        let Ok(m) = serde_json::from_str::<ClientMsg>(text.as_str()) else {
            err("bad_message");
            continue;
        };
        match m {
            ClientMsg::Listen { channel, persistent } => {
                if channel.len() != 64 || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
                    err("bad_channel");
                    continue;
                }
                let mut l = s.listeners.lock();
                // First come, first served: a live listener is never replaced
                // by another connection (its channel frees when it leaves).
                match l.get(&channel) {
                    Some((owner, _)) if *owner != id => {
                        drop(l);
                        push(&tx, ServerMsg::Error { code: "channel_busy".into(), channel: Some(channel) });
                        continue;
                    }
                    Some(_) => {}
                    None if l.len() >= s.cfg.max_listeners => {
                        drop(l);
                        err("busy");
                        continue;
                    }
                    None => {}
                }
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
                        if old != channel {
                            remove_if_mine(&mut l, &old, id);
                        }
                    }
                }
                l.insert(channel, (id, tx.clone()));
            }
            ClientMsg::Unlisten { channel } => {
                my_channels.retain(|c| c != &channel);
                if code_channel.as_ref() == Some(&channel) {
                    code_channel = None;
                }
                remove_if_mine(&mut s.listeners.lock(), &channel, id);
            }
            ClientMsg::Connect { channel } => {
                if !bucket.take(s.cfg.connects_per_minute) {
                    err("rate_limited");
                    continue;
                }
                let listener = s.listeners.lock().get(&channel).cloned();
                let Some((lid, ltx)) = listener else {
                    err("no_listener");
                    continue;
                };
                let sid: [u8; 16] = rand::random();
                let keys: [[u8; 32]; 2] = [rand::random(), rand::random()];
                {
                    let mut sessions = s.sessions.lock();
                    if sessions.len() >= s.cfg.max_sessions {
                        drop(sessions);
                        err("busy");
                        continue;
                    }
                    let now = Instant::now();
                    sessions.insert(
                        sid,
                        Session {
                            ws: [Some((lid, ltx.clone())), Some((id, tx.clone()))],
                            relay: [None, None],
                            bind_key: keys,
                            authed: [false, false],
                            bytes: 0,
                            created: now,
                            last: now,
                        },
                    );
                }
                my_sessions.retain(|sid| s.sessions.lock().contains_key(sid));
                my_sessions.push(sid);
                let hex_id = hex::encode(sid);
                push(&ltx, ServerMsg::Paired { session: hex_id.clone(), side: 0, udp_port: s.udp_port, bind_key: hex::encode(keys[0]) });
                push(&tx, ServerMsg::Paired { session: hex_id, side: 1, udp_port: s.udp_port, bind_key: hex::encode(keys[1]) });
            }
            ClientMsg::Signal { session, data } => {
                let Some(sid) = hex::decode(&session).ok().and_then(|v| <[u8; 16]>::try_from(v).ok()) else { continue };
                let sessions = s.sessions.lock();
                let Some(sess) = sessions.get(&sid) else { continue };
                let Some(side) = sess.ws.iter().position(|w| matches!(w, Some((cid, _)) if *cid == id)) else { continue };
                if let Some((_, other)) = &sess.ws[1 - side] {
                    push(other, ServerMsg::Signal { session, data });
                }
            }
        }
    }
    {
        let mut l = s.listeners.lock();
        for c in my_channels.iter().chain(code_channel.iter()) {
            remove_if_mine(&mut l, c, id);
        }
    }
    // Forget this client in every session it took part in. Sessions that never
    // carried relay traffic go now; bound ones keep relaying until idle (the
    // dialing side closes its WebSocket as soon as QUIC is up).
    s.sessions.lock().retain(|_, sess| {
        for slot in sess.ws.iter_mut() {
            if matches!(slot, Some((cid, _)) if *cid == id) {
                *slot = None;
            }
        }
        sess.bound() || sess.ws.iter().any(Option::is_some)
    });
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
        let side = side as usize;
        let mut header = [0u8; HDR];
        header.copy_from_slice(&buf[..HDR]);
        match kind {
            PROBE => {
                if !s.sessions.lock().contains_key(&sid) {
                    continue;
                }
                let mut reply = header.to_vec();
                reply[4] = PROBE_ACK;
                reply.extend_from_slice(from.to_string().as_bytes());
                let _ = sock.send_to(&reply, from).await;
            }
            BIND => {
                if n != HDR + MAC_LEN {
                    continue;
                }
                let ok = {
                    let mut sessions = s.sessions.lock();
                    let Some(sess) = sessions.get_mut(&sid) else { continue };
                    let expect = blake3::Hash::from(bind_mac(&sess.bind_key[side], &sid, side as u8, &from));
                    let mac = <[u8; MAC_LEN]>::try_from(&buf[HDR..n]).expect("length checked");
                    // blake3::Hash equality is constant-time.
                    if expect == blake3::Hash::from(mac) {
                        sess.relay[side] = Some(from);
                        sess.authed[side] = true;
                        sess.last = Instant::now();
                        true
                    } else {
                        false
                    }
                };
                if ok {
                    header[4] = BIND_ACK;
                    let _ = sock.send_to(&header, from).await;
                }
            }
            DATA => {
                enum Route {
                    Forward(SocketAddr),
                    NeedBind,
                    Drop,
                }
                let route = {
                    let mut sessions = s.sessions.lock();
                    let Some(sess) = sessions.get_mut(&sid) else { continue };
                    let known = match sess.relay[side] {
                        Some(a) => a == from,
                        // Clients without bind keys (v1.0.0) bind with their
                        // first packet; nothing can move an address after that
                        // except an authenticated BIND.
                        None if !sess.authed[side] => {
                            sess.relay[side] = Some(from);
                            true
                        }
                        None => false,
                    };
                    if !known {
                        Route::NeedBind
                    } else {
                        sess.last = Instant::now();
                        if n == HDR {
                            Route::Drop // bind only
                        } else {
                            sess.bytes += n as u64;
                            match sess.relay[1 - side] {
                                Some(to) if sess.bytes <= s.cfg.max_session_bytes => Route::Forward(to),
                                _ => Route::Drop,
                            }
                        }
                    }
                };
                match route {
                    Route::Forward(to) => {
                        let _ = sock.send_to(&buf[..n], to).await;
                    }
                    Route::NeedBind => {
                        // Never larger than the packet that triggered it.
                        let observed = from.to_string();
                        if HDR + observed.len() <= n {
                            header[4] = NEED_BIND;
                            let mut pkt = header.to_vec();
                            pkt.extend_from_slice(observed.as_bytes());
                            let _ = sock.send_to(&pkt, from).await;
                        }
                    }
                    Route::Drop => {}
                }
            }
            _ => {}
        }
    }
}
