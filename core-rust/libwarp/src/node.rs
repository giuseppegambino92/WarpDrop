//! `WarpNode`: the engine every platform embeds. It owns the QUIC endpoint,
//! discovery, the Wormhole code, the trust ring and all transfers, and is
//! exported through UniFFI (Swift/Kotlin) and used directly from Rust (Tauri).

use crate::discovery::{self, beacon, mdns::Mdns, Announce, PeerAddr, PeerTable, Source};
use crate::identity::{device_id_from_key, fingerprint, parse_public_key_hex, Identity};
use crate::manifest::{self, Manifest};
use crate::netif::InterfaceTable;
use crate::pake;
use crate::protocol::{close, read_msg, write_msg, Auth, DeviceInfo, Msg, Purpose, RingEntry, PROTOCOL_VERSION};
use crate::receiver::{self, PARTIAL_DIR};
use crate::resume::{resume_channel, TicketStore, TICKET_TTL};
use crate::sender;
use crate::tls;
use crate::transfer::{EventBus, Transfer};
use crate::transport::{self, adb, rendezvous, sockets};
use crate::trust::{now_ms, TrustStore};
use crate::types::*;
use crate::{WarpError, WarpResult};
use anyhow::{anyhow, bail, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, watch, Notify};

const MAX_CODE_FAILURES: u32 = 3;
const TOKEN_TTL: Duration = Duration::from_secs(2 * 3600);
const OFFER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_CLIPBOARD: usize = 32 * 1024 * 1024;

/// Resume token → (peer key it was issued to, expiry).
type IssuedToken = ([u8; 32], Instant);

struct CodeState {
    code: String,
    expires: Instant,
    expires_ms: u64,
    failures: u32,
}

/// Where an outgoing session should go.
#[derive(Clone)]
enum Target {
    Peer(String),
    Code { code: String, ticket: Option<Ticket> },
    /// Reconnect to a key we already authenticated (resume an interrupted
    /// transfer): LAN addresses and/or the ticket's rendezvous channel.
    Resume { key: [u8; 32], addrs: Vec<PeerAddr>, ticket: Option<[u8; 32]>, via_rendezvous: bool },
}

#[derive(Clone, Debug)]
struct Ticket {
    code: String,
    key: Option<[u8; 32]>,
    addrs: Vec<SocketAddr>,
    stream_port: Option<u16>,
}

struct Session {
    conn: quinn::Connection,
    kind: PathKind,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    peer_key: [u8; 32],
    peer: DeviceInfo,
    sas: Option<Vec<String>>,
    /// Reached through the rendezvous server (hole punching or relay).
    via_rendezvous: bool,
}

pub(crate) struct Inner {
    cfg: RwLock<NodeConfig>,
    identity: Identity,
    trust: TrustStore,
    peers: Arc<PeerTable>,
    ifs: Arc<InterfaceTable>,
    bus: Arc<EventBus>,
    endpoint: quinn::Endpoint,
    /// TCP port of the stream carriage (0 = unavailable).
    stream_port: u16,
    stream_listener: Mutex<Option<tokio::net::TcpListener>>,
    server_cfg: quinn::ServerConfig,
    code: Mutex<CodeState>,
    listen_tx: watch::Sender<rendezvous::ListenSet>,
    tickets: TicketStore,
    link_window: Mutex<Option<Instant>>,
    transfers: Mutex<HashMap<String, Arc<Transfer>>>,
    incoming_by_stage: Mutex<HashMap<String, Arc<Transfer>>>,
    offers: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    issued_tokens: Mutex<HashMap<[u8; 32], IssuedToken>>,
    held_tokens: Mutex<HashMap<[u8; 32], [u8; 32]>>,
    ring_synced: Mutex<HashSet<String>>,
    mdns: Mutex<Option<Arc<Mdns>>>,
    relay_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    peers_dirty: Notify,
    shutting_down: AtomicBool,
}

#[derive(uniffi::Object)]
pub struct WarpNode {
    rt: Mutex<Option<tokio::runtime::Runtime>>,
    handle: tokio::runtime::Handle,
    inner: Arc<Inner>,
}

impl Drop for WarpNode {
    fn drop(&mut self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        if let Some(m) = self.inner.mdns.lock().take() {
            m.shutdown();
        }
        if let Some(rt) = self.rt.lock().take() {
            rt.shutdown_background();
        }
    }
}

fn new_id() -> String {
    hex::encode(rand::random::<[u8; 8]>())
}

fn retryable(e: &anyhow::Error) -> bool {
    if e.downcast_ref::<receiver::SaveError>().is_some() {
        return false;
    }
    if let Some(w) = e.downcast_ref::<WarpError>() {
        return matches!(w, WarpError::Timeout | WarpError::Unreachable { .. } | WarpError::Io { .. });
    }
    e.downcast_ref::<quinn::ConnectionError>().is_some()
        || e.downcast_ref::<quinn::WriteError>().is_some()
        || e.downcast_ref::<quinn::ReadError>().is_some()
        || e.downcast_ref::<quinn::ReadExactError>().is_some()
        || e.downcast_ref::<quinn::ConnectError>().is_some()
        || e.downcast_ref::<tokio::time::error::Elapsed>().is_some()
        || e.downcast_ref::<std::io::Error>().is_some()
        || format!("{e:#}").contains("control stream closed")
        || format!("{e:#}").contains("connection lost")
}

/// Stable error code for UIs (see `TransferSnapshot::error_code`).
fn error_code(e: &anyhow::Error) -> &'static str {
    match e.downcast_ref::<WarpError>() {
        Some(WarpError::WrongCode | WarpError::InvalidCode) => "wrong_code",
        Some(WarpError::CodeNotFound) => "code_not_found",
        Some(WarpError::Rejected { reason }) if reason == "declined" => "declined",
        Some(WarpError::Rejected { reason }) if reason.starts_with("protocol_version") => "incompatible",
        Some(WarpError::Rejected { .. }) => "rejected",
        Some(WarpError::Unreachable { .. } | WarpError::PeerNotFound) => "unreachable",
        Some(WarpError::Timeout) => "timeout",
        Some(WarpError::Cancelled) => "cancelled",
        Some(WarpError::Io { .. }) => "io",
        Some(_) => "protocol",
        None if e.downcast_ref::<receiver::SaveError>().is_some() => "io",
        None if format!("{e:#}").contains("BLAKE3 mismatch") || format!("{e:#}").contains("failed verification") => "integrity",
        None if retryable(e) => "unreachable",
        None => "protocol",
    }
}

fn fail(t: &Transfer, e: &anyhow::Error) {
    t.finish_err(user_message(e), error_code(e));
}

fn user_message(e: &anyhow::Error) -> String {
    match e.downcast_ref::<WarpError>() {
        Some(w) => w.to_string(),
        None => format!("{e:#}"),
    }
}

// ------------------------------------------------------------- tickets ----

fn ticket_uri(code: &str, key: &[u8; 32], stream_port: u16, addrs: &[SocketAddr]) -> String {
    let a: Vec<String> = addrs.iter().map(|a| a.to_string()).collect();
    format!("warpdrop://c/{code}?pk={}&t={stream_port}&a={}", hex::encode(key), a.join(","))
}

fn parse_target(input: &str) -> Result<Ticket> {
    let input = input.trim();
    if let Some(code) = pake::normalize_code(input) {
        return Ok(Ticket { code, key: None, addrs: vec![], stream_port: None });
    }
    let url = url::Url::parse(input).map_err(|_| WarpError::InvalidCode)?;
    if url.scheme() != "warpdrop" {
        return Err(WarpError::InvalidCode.into());
    }
    let code = url
        .path_segments()
        .and_then(|mut s| s.next_back().map(str::to_string))
        .and_then(|c| pake::normalize_code(&c))
        .ok_or(WarpError::InvalidCode)?;
    let mut t = Ticket { code, key: None, addrs: vec![], stream_port: None };
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "pk" => t.key = parse_public_key_hex(&v).ok(),
            "a" => t.addrs = v.split(',').filter_map(discovery::parse_socket_addr).collect(),
            "t" => t.stream_port = v.parse().ok().filter(|p| *p != 0),
            _ => {}
        }
    }
    Ok(t)
}

// ------------------------------------------------------------ the node ----

impl Inner {
    fn me(&self) -> DeviceInfo {
        let c = self.cfg.read();
        DeviceInfo { name: c.device_name.clone(), kind: c.device_kind }
    }

    fn announce(&self) -> Announce {
        let me = self.me();
        Announce {
            device_id: self.identity.device_id(),
            name: me.name,
            kind: me.kind,
            public_key: Some(*self.identity.public_key()),
            code_open: true,
            stream_port: (self.stream_port != 0).then_some(self.stream_port),
        }
    }

    fn port(&self) -> u16 {
        self.endpoint.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    fn download_dir(&self) -> PathBuf {
        PathBuf::from(&self.cfg.read().download_dir)
    }

    // ----- code -----

    fn code_info(&self) -> CodeInfo {
        let c = self.code.lock();
        let port = self.port();
        let addrs: Vec<SocketAddr> = self
            .ifs
            .advertised_ips()
            .into_iter()
            .filter(|ip| match ip {
                std::net::IpAddr::V4(_) => true,
                std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
            })
            .take(4)
            .map(|ip| SocketAddr::new(ip, port))
            .collect();
        CodeInfo {
            code: c.code.clone(),
            display: pake::display_code(&c.code),
            expires_at_ms: c.expires_ms,
            qr_payload: ticket_uri(&c.code, self.identity.public_key(), self.stream_port, &addrs),
        }
    }

    fn rotate_code(&self) {
        let secs = self.cfg.read().code_rotation_secs.max(30) as u64;
        let code = {
            let mut c = self.code.lock();
            let mut next = pake::generate_code();
            while next == c.code {
                next = pake::generate_code();
            }
            c.code = next.clone();
            c.expires = Instant::now() + Duration::from_secs(secs);
            c.expires_ms = now_ms() + secs * 1000;
            c.failures = 0;
            next
        };
        self.listen_tx.send_modify(|l| l.code = code);
        self.bus.emit(WarpEvent::CodeChanged { code: self.code_info() });
    }

    fn record_code_failure(&self) {
        let rotate = {
            let mut c = self.code.lock();
            c.failures += 1;
            c.failures >= MAX_CODE_FAILURES
        };
        if rotate {
            tracing::info!("too many wrong code attempts, rotating code");
            self.rotate_code();
        }
    }

    // ----- peers -----

    fn peer_infos(&self) -> Vec<PeerInfo> {
        self.peers
            .all()
            .into_iter()
            .map(|p| {
                let ranked = transport::rank(p.addrs.keys().copied(), &self.ifs);
                PeerInfo {
                    trusted: p.public_key.map(|k| self.trust.contains(&k)).unwrap_or(false),
                    device_id: p.device_id,
                    name: p.name,
                    kind: p.kind,
                    code_open: p.code_open,
                    best_path: ranked.first().map(|c| c.kind),
                    addresses: ranked
                        .iter()
                        .map(|c| match c.addr {
                            PeerAddr::Udp(a) => a.to_string(),
                            PeerAddr::Tcp(a) => format!("tcp:{a}"),
                            PeerAddr::Tunnel(p) => format!("adb:{p}"),
                        })
                        .collect(),
                    last_seen_ms: now_ms().saturating_sub(p.last_seen.elapsed().as_millis() as u64),
                }
            })
            .collect()
    }

    fn trusted_list(&self) -> Vec<TrustedDevice> {
        let online: HashSet<[u8; 32]> = self.peers.all().into_iter().filter_map(|p| p.public_key).collect();
        self.trust
            .list()
            .into_iter()
            .map(|e| TrustedDevice {
                device_id: e.device_id(),
                online: online.contains(&e.public_key),
                public_key_hex: hex::encode(e.public_key),
                name: e.name,
                kind: e.kind,
                origin: e.origin,
                added_at_ms: e.added_at_ms,
            })
            .collect()
    }

    fn mark_peers_dirty(&self) {
        self.peers_dirty.notify_one();
    }

    fn emit_trust(&self) {
        self.bus.emit(WarpEvent::TrustRingChanged { devices: self.trusted_list() });
    }

    // ----- tokens -----

    fn register_token(&self, token: [u8; 32], peer_key: [u8; 32]) {
        let mut m = self.issued_tokens.lock();
        m.retain(|_, (_, exp)| *exp > Instant::now());
        m.insert(token, (peer_key, Instant::now() + TOKEN_TTL));
    }

    fn check_token(&self, token: &[u8; 32], peer_key: &[u8; 32]) -> bool {
        let m = self.issued_tokens.lock();
        matches!(m.get(token), Some((k, exp)) if k == peer_key && *exp > Instant::now())
    }

    /// Publishes the live resume-ticket channels to the rendezvous listener.
    fn sync_ticket_channels(&self) {
        let channels = self.tickets.channels();
        self.listen_tx.send_if_modified(|l| {
            if l.resume == channels {
                return false;
            }
            l.resume = channels;
            true
        });
    }

    fn register_transfer(&self, t: &Arc<Transfer>) {
        self.transfers.lock().insert(t.id.clone(), t.clone());
    }
}

// ------------------------------------------------------ client sessions ----

async fn open_session(
    inner: &Arc<Inner>,
    conn: quinn::Connection,
    kind: PathKind,
    auth: Auth,
    code: Option<&str>,
    purpose: Purpose,
) -> Result<Session> {
    let peer_key = tls::peer_key(&conn)?;
    let binding = tls::channel_binding(&conn)?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(&mut send, &Msg::Hello { version: PROTOCOL_VERSION, device: inner.me(), auth, purpose }).await?;
    let mut sas = None;
    if auth == Auth::Code {
        let code = code.ok_or_else(|| anyhow!("missing code"))?;
        let (st, mine) = pake::start(code, &binding);
        write_msg(&mut send, &Msg::Pake(mine)).await?;
        let theirs = match read_msg(&mut recv).await? {
            Msg::Pake(m) => m,
            Msg::Reject { reason } => return Err(reject_error(reason)),
            m => bail!("unexpected {m:?}"),
        };
        let keys = st.finish(&theirs).map_err(|_| WarpError::WrongCode)?;
        write_msg(&mut send, &Msg::Confirm(keys.client_confirm())).await?;
        match read_msg(&mut recv).await {
            Ok(Msg::Confirm(tag)) if pake::ct_eq(&tag, &keys.server_confirm()) => {}
            Ok(Msg::Confirm(_)) | Ok(Msg::Reject { .. }) | Err(_) => return Err(WarpError::WrongCode.into()),
            Ok(m) => bail!("unexpected {m:?}"),
        }
        sas = Some(keys.sas());
    } else if auth == Auth::Ask {
        sas = Some(pake::sas_from_binding(&binding));
    }
    let (peer, resume_token) = match read_msg(&mut recv).await? {
        Msg::Welcome { device, resume_token, .. } => (device, resume_token),
        Msg::Reject { reason } => return Err(reject_error(reason)),
        m => bail!("unexpected {m:?}"),
    };
    inner.held_tokens.lock().insert(peer_key, resume_token);
    Ok(Session { conn, kind, send, recv, peer_key, peer, sas, via_rendezvous: false })
}

fn reject_error(reason: String) -> anyhow::Error {
    match reason.as_str() {
        "wrong_code" => WarpError::WrongCode.into(),
        "protocol_version" => WarpError::Rejected {
            reason: "protocol_version (the other device runs an incompatible WarpDrop version; update both)".into(),
        }
        .into(),
        _ => WarpError::Rejected { reason }.into(),
    }
}

async fn connect_addrs(
    inner: &Arc<Inner>,
    addrs: Vec<PeerAddr>,
    pin: Option<[u8; 32]>,
) -> Result<(quinn::Connection, PathKind)> {
    let client = tls::client_config(&inner.identity, pin)?;
    let cands = transport::rank(addrs, &inner.ifs);
    transport::race_connect(&inner.endpoint, client, cands).await
}

async fn connect_peer(inner: &Arc<Inner>, peer_id: &str, purpose: Purpose) -> Result<Session> {
    let peer = inner.peers.get(peer_id);
    let trusted = inner.trust.by_device_id(peer_id);
    let key = trusted.as_ref().map(|t| t.public_key).or(peer.as_ref().and_then(|p| p.public_key));
    let Some(peer) = peer else {
        return Err(if trusted.is_some() {
            WarpError::Unreachable { msg: "device is not nearby".into() }.into()
        } else {
            WarpError::PeerNotFound.into()
        });
    };
    let addrs: Vec<PeerAddr> = peer.addrs.keys().copied().collect();
    let (conn, kind) = connect_addrs(inner, addrs.clone(), key).await?;
    let token = key.and_then(|k| inner.held_tokens.lock().get(&k).copied());
    let auth = if trusted.is_some() {
        Auth::Trusted
    } else if let Some(t) = token {
        Auth::Token(t)
    } else {
        Auth::Ask
    };
    match open_session(inner, conn, kind, auth, None, purpose).await {
        Err(e) if auth != Auth::Ask && matches!(e.downcast_ref::<WarpError>(), Some(WarpError::Rejected { .. })) => {
            // They do not (or no longer) trust us / token expired: ask instead.
            let (conn, kind) = connect_addrs(inner, addrs, key).await?;
            open_session(inner, conn, kind, Auth::Ask, None, purpose).await
        }
        r => r,
    }
}

/// Finds whoever shows `ticket.code`. A receiver that was *just* opened may
/// not have been discovered yet, so new nearby devices are retried for a few
/// seconds before reporting "no device is showing this code".
async fn connect_code(inner: &Arc<Inner>, ticket: &Ticket, purpose: Purpose) -> Result<Session> {
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut tried = HashSet::new();
    let mut first = true;
    // Other nearby devices showing *their* code answer "wrong code"; that is
    // not final while the intended device may still be discovered.
    let mut wrong = false;
    loop {
        let r = connect_code_once(inner, ticket, purpose, &mut tried, first).await;
        let retry = match r.as_ref().err().and_then(|e| e.downcast_ref::<WarpError>()) {
            Some(WarpError::CodeNotFound) => true,
            Some(WarpError::WrongCode) => {
                wrong = true;
                true
            }
            _ => false,
        };
        if !retry || Instant::now() >= deadline {
            return match r {
                Err(e) if wrong && matches!(e.downcast_ref::<WarpError>(), Some(WarpError::CodeNotFound)) => {
                    Err(WarpError::WrongCode.into())
                }
                r => r,
            };
        }
        first = false;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

async fn connect_code_once(
    inner: &Arc<Inner>,
    ticket: &Ticket,
    purpose: Purpose,
    tried: &mut HashSet<String>,
    first: bool,
) -> Result<Session> {
    let mut set: tokio::task::JoinSet<Result<Session>> = tokio::task::JoinSet::new();
    // 1. Direct addresses from a scanned QR ticket.
    if first && !ticket.addrs.is_empty() {
        let (inner, t) = (inner.clone(), ticket.clone());
        set.spawn(async move {
            let addrs = discovery::with_stream_addrs(t.addrs.iter().map(|a| PeerAddr::Udp(*a)).collect(), t.stream_port);
            let (conn, kind) = connect_addrs(&inner, addrs, t.key).await?;
            open_session(&inner, conn, kind, Auth::Code, Some(&t.code), purpose).await
        });
    }
    // 2. Nearby devices currently showing a code.
    for p in inner.peers.all().into_iter().filter(|p| p.code_open) {
        if ticket.key.is_some() && p.public_key != ticket.key || !tried.insert(p.device_id.clone()) {
            continue;
        }
        let (inner, code) = (inner.clone(), ticket.code.clone());
        set.spawn(async move {
            let addrs = p.addrs.keys().copied().collect();
            let (conn, kind) = connect_addrs(&inner, addrs, p.public_key).await?;
            open_session(&inner, conn, kind, Auth::Code, Some(&code), purpose).await
        });
    }
    // 3. Internet rendezvous (started slightly later so LAN wins when possible).
    let relay = inner.cfg.read().relay_url.clone().filter(|_| first);
    if let Some(url) = relay {
        let (inner, t) = (inner.clone(), ticket.clone());
        let lan_attempts = !set.is_empty();
        set.spawn(async move {
            if lan_attempts {
                tokio::time::sleep(Duration::from_millis(1200)).await;
            }
            let force = inner.cfg.read().force_relay;
            let client = tls::client_config(&inner.identity, t.key)?;
            let channel = pake::rendezvous_channel(&t.code)?;
            let d = rendezvous::dial(&url, channel, client, &inner.ifs, force).await?;
            // Keep the per-session endpoint alive with the connection.
            let ep = d.endpoint.clone();
            let mut s = open_session(&inner, d.conn, d.kind, Auth::Code, Some(&t.code), purpose).await?;
            s.via_rendezvous = true;
            let c = s.conn.clone();
            tokio::spawn(async move {
                c.closed().await;
                drop(ep);
            });
            Ok(s)
        });
    }
    if set.is_empty() {
        return Err(WarpError::CodeNotFound.into());
    }
    let mut errors = vec![];
    while let Some(r) = set.join_next().await {
        match r {
            Ok(Ok(s)) => {
                set.abort_all();
                return Ok(s);
            }
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(anyhow!("{e}")),
        }
    }
    let is = |w: fn(&WarpError) -> bool| errors.iter().any(|e| e.downcast_ref::<WarpError>().map(w).unwrap_or(false));
    if is(|w| matches!(w, WarpError::WrongCode)) {
        return Err(WarpError::WrongCode.into());
    }
    if let Some(e) = errors.into_iter().find(|e| matches!(e.downcast_ref::<WarpError>(), Some(WarpError::Rejected { .. }))) {
        return Err(e);
    }
    Err(WarpError::CodeNotFound.into())
}

async fn connect_target(inner: &Arc<Inner>, target: &Target, purpose: Purpose) -> Result<Session> {
    match target {
        Target::Peer(id) => connect_peer(inner, id, purpose).await,
        Target::Code { code, ticket } => {
            let t = ticket.clone().unwrap_or(Ticket { code: code.clone(), key: None, addrs: vec![], stream_port: None });
            connect_code(inner, &t, purpose).await
        }
        Target::Resume { key, addrs, ticket, via_rendezvous } => connect_resume(inner, *key, addrs, *ticket, *via_rendezvous, purpose).await,
    }
}

/// Reconnects to an already-authenticated receiver without any user input:
/// over the LAN if it is reachable there, and/or through the rendezvous server
/// under the resumption ticket's channel (Internet sessions).
async fn connect_resume(
    inner: &Arc<Inner>,
    key: [u8; 32],
    addrs: &[PeerAddr],
    ticket: Option<[u8; 32]>,
    via_rendezvous: bool,
    purpose: Purpose,
) -> Result<Session> {
    let mut addrs = addrs.to_vec();
    if let Some(p) = inner.peers.find_by_key(&key) {
        addrs.extend(p.addrs.keys().copied());
    }
    addrs.sort();
    addrs.dedup();
    let auth = if inner.trust.contains(&key) {
        Auth::Trusted
    } else if let Some(t) = ticket {
        Auth::Resume(t)
    } else if let Some(t) = inner.held_tokens.lock().get(&key).copied() {
        Auth::Token(t)
    } else {
        return Err(WarpError::Unreachable { msg: "session expired".into() }.into());
    };
    let mut set: tokio::task::JoinSet<Result<Session>> = tokio::task::JoinSet::new();
    let lan = !addrs.is_empty();
    if lan {
        let inner = inner.clone();
        set.spawn(async move {
            let (conn, kind) = connect_addrs(&inner, addrs, Some(key)).await?;
            open_session(&inner, conn, kind, auth, None, purpose).await
        });
    }
    let relay = inner.cfg.read().relay_url.clone();
    if let (true, Some(url), Some(t)) = (via_rendezvous, relay, ticket) {
        let inner = inner.clone();
        set.spawn(async move {
            if lan {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            let force = inner.cfg.read().force_relay;
            // Pin the receiver's key: the relay cannot substitute anyone.
            let client = tls::client_config(&inner.identity, Some(key))?;
            let d = rendezvous::dial(&url, resume_channel(&t), client, &inner.ifs, force)
                .await
                .map_err(|e| match e.downcast_ref::<WarpError>() {
                    Some(WarpError::CodeNotFound) => WarpError::Unreachable { msg: "the other device is offline".into() }.into(),
                    _ => e,
                })?;
            let ep = d.endpoint.clone();
            let mut s = open_session(&inner, d.conn, d.kind, auth, None, purpose).await?;
            s.via_rendezvous = true;
            let c = s.conn.clone();
            tokio::spawn(async move {
                c.closed().await;
                drop(ep);
            });
            Ok(s)
        });
    }
    if set.is_empty() {
        return Err(WarpError::Unreachable { msg: "the other device is not reachable".into() }.into());
    }
    let mut last = None;
    while let Some(r) = set.join_next().await {
        match r {
            Ok(Ok(s)) => {
                set.abort_all();
                return Ok(s);
            }
            Ok(Err(e)) => last = Some(e),
            Err(e) => last = Some(anyhow!("{e}")),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("unreachable")))
}

// -------------------------------------------------------- outgoing work ----

async fn run_outgoing(inner: Arc<Inner>, t: Arc<Transfer>, mut target: Target, items: Vec<SendItem>) {
    let src = match tokio::task::spawn_blocking(move || manifest::build(&items)).await {
        Ok(Ok(s)) => Arc::new(s),
        Ok(Err(e)) => return fail(&t, &e),
        Err(e) => return t.finish_err(e.to_string(), "io"),
    };
    t.set_meta(src.manifest.title(), src.manifest.file_count(), src.manifest.total_bytes());
    let streams = inner.cfg.read().parallel_streams.max(1) as usize;
    let mut deadline = Instant::now() + Duration::from_secs(120);
    let mut attempt = 0u32;
    loop {
        if t.is_cancelled() {
            return;
        }
        let result: Result<()> = async {
            let mut s = connect_target(&inner, &target, Purpose::Transfer).await?;
            *t.peer_name.lock() = s.peer.name.clone();
            *t.conn.lock() = Some(s.conn.clone());
            t.set_path(s.kind);
            // From now on, reconnect straight to this authenticated key.
            let via_rendezvous = s.via_rendezvous || matches!(target, Target::Resume { via_rendezvous: true, .. });
            let addrs = if via_rendezvous { vec![] } else { vec![PeerAddr::Udp(s.conn.remote_address())] };
            let ticket = *t.resume_ticket.lock();
            target = Target::Resume { key: s.peer_key, addrs, ticket, via_rendezvous };
            let r = tokio::select! {
                r = sender::send(&s.conn, &mut s.send, &mut s.recv, src.clone(), t.clone(), streams) => r,
                _ = t.cancelled() => Err(WarpError::Cancelled.into()),
            };
            if r.is_ok() {
                s.conn.close(quinn::VarInt::from_u32(close::DONE), b"done");
            }
            r
        }
        .await;
        match result {
            Ok(()) => return t.finish_ok(vec![]),
            Err(e) if !t.is_cancelled() && retryable(&e) && Instant::now() < deadline => {
                attempt += 1;
                tracing::info!("transfer {} interrupted ({e:#}); reconnecting (attempt {attempt})", t.id);
                t.set_state(TransferState::Reconnecting);
                if let Target::Resume { ticket, key, addrs, via_rendezvous } = &mut target {
                    // The ticket arrives with the receiver's Accept.
                    if ticket.is_none() {
                        *ticket = *t.resume_ticket.lock();
                    }
                    if ticket.is_some() && attempt == 1 {
                        // A resumption ticket stays valid for 30 minutes.
                        deadline = Instant::now() + TICKET_TTL;
                    }
                    let reachable = !addrs.is_empty()
                        || inner.peers.find_by_key(key).is_some()
                        || (*via_rendezvous && ticket.is_some() && inner.cfg.read().relay_url.is_some());
                    if !reachable {
                        return fail(&t, &e);
                    }
                }
                let backoff = (500u64 << attempt.min(5)).min(10_000);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(backoff)) => {}
                    _ = t.cancelled() => return,
                }
            }
            Err(e) => return fail(&t, &e),
        }
    }
}

async fn run_clipboard(inner: Arc<Inner>, t: Arc<Transfer>, target: Target, content: ClipboardContent) {
    let r: Result<()> = async {
        let mut s = connect_target(&inner, &target, Purpose::Clipboard).await?;
        *t.peer_name.lock() = s.peer.name.clone();
        t.set_path(s.kind);
        t.set_state(TransferState::Transferring);
        write_msg(&mut s.send, &Msg::Clipboard(content)).await?;
        match read_msg(&mut s.recv).await? {
            Msg::Ack => {}
            Msg::Reject { reason } => return Err(WarpError::Rejected { reason }.into()),
            m => bail!("unexpected {m:?}"),
        }
        let total = t.total_bytes.load(Ordering::Relaxed);
        t.add_progress(total, total);
        s.conn.close(quinn::VarInt::from_u32(close::DONE), b"done");
        Ok(())
    }
    .await;
    match r {
        Ok(()) => t.finish_ok(vec![]),
        Err(e) => fail(&t, &e),
    }
}

async fn ring_sync(inner: Arc<Inner>, peer_id: String) -> Result<()> {
    let mut s = connect_peer(&inner, &peer_id, Purpose::RingSync).await?;
    write_msg(&mut s.send, &Msg::Ring(ring_entries(&inner))).await?;
    if let Msg::Ring(theirs) = read_msg(&mut s.recv).await? {
        merge_ring(&inner, theirs);
    }
    s.conn.close(quinn::VarInt::from_u32(close::DONE), b"done");
    Ok(())
}

fn ring_entries(inner: &Inner) -> Vec<RingEntry> {
    let me = inner.me();
    let mut v: Vec<RingEntry> = inner
        .trust
        .list()
        .into_iter()
        .map(|e| RingEntry { public_key: e.public_key, name: e.name, kind: e.kind })
        .collect();
    v.push(RingEntry { public_key: *inner.identity.public_key(), name: me.name, kind: me.kind });
    v
}

fn merge_ring(inner: &Inner, entries: Vec<RingEntry>) {
    let mut changed = false;
    for e in entries.into_iter().take(256) {
        if &e.public_key == inner.identity.public_key() {
            continue;
        }
        changed |= inner.trust.upsert(e.public_key, &e.name, e.kind, TrustOrigin::Ring).unwrap_or(false);
    }
    if changed {
        inner.emit_trust();
        inner.mark_peers_dirty();
    }
}

// -------------------------------------------------------- incoming work ----

fn handle_conn(inner: Arc<Inner>, conn: quinn::Connection, kind: PathKind, via_rendezvous: bool) {
    tokio::spawn(async move {
        if let Err(e) = serve(&inner, conn.clone(), kind, via_rendezvous).await {
            tracing::debug!("incoming session from {} ended: {e:#}", conn.remote_address());
        }
    });
}

/// Sends a rejection and keeps the connection open until the peer has read it
/// (dropping the last handle would close QUIC and discard queued data).
async fn reject(conn: &quinn::Connection, send: &mut quinn::SendStream, reason: &str) -> Result<()> {
    write_msg(send, &Msg::Reject { reason: reason.to_string() }).await?;
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
    Ok(())
}

async fn serve(inner: &Arc<Inner>, conn: quinn::Connection, kind: PathKind, via_rendezvous: bool) -> Result<()> {
    let peer_key = tls::peer_key(&conn)?;
    let binding = tls::channel_binding(&conn)?;
    let (mut send, mut recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi()).await??;
    let Msg::Hello { version, device, auth, purpose } = read_msg(&mut recv).await? else {
        bail!("expected hello");
    };
    if version != PROTOCOL_VERSION {
        return reject(&conn, &mut send, "protocol_version").await;
    }
    let trusted = inner.trust.contains(&peer_key);
    let mut sas = None;
    let mut code_auth = false;
    let mut resume_manifest = None;
    match auth {
        Auth::Trusted if trusted => {}
        Auth::Trusted => {
            return reject(&conn, &mut send, "not_trusted").await;
        }
        Auth::Token(tok) if inner.check_token(&tok, &peer_key) => {}
        Auth::Token(_) => {
            return reject(&conn, &mut send, "session_expired").await;
        }
        Auth::Resume(ticket) => match inner.tickets.check_peer(&ticket, &peer_key) {
            Some(m) if purpose == Purpose::Transfer => resume_manifest = Some(m),
            _ => return reject(&conn, &mut send, "ticket_expired").await,
        },
        Auth::Code => {
            let theirs = match read_msg(&mut recv).await? {
                Msg::Pake(m) => m,
                m => bail!("unexpected {m:?}"),
            };
            let code = inner.code.lock().code.clone();
            let (st, mine) = pake::start(&code, &binding);
            write_msg(&mut send, &Msg::Pake(mine)).await?;
            let keys = st.finish(&theirs);
            let tag = match read_msg(&mut recv).await? {
                Msg::Confirm(t) => t,
                m => bail!("unexpected {m:?}"),
            };
            match keys {
                Ok(k) if pake::ct_eq(&tag, &k.client_confirm()) => {
                    write_msg(&mut send, &Msg::Confirm(k.server_confirm())).await?;
                    sas = Some(k.sas());
                    code_auth = true;
                }
                _ => {
                    inner.record_code_failure();
                    return reject(&conn, &mut send, "wrong_code").await;
                }
            }
        }
        Auth::Ask => {
            if !matches!(purpose, Purpose::Transfer | Purpose::Identify) {
                return reject(&conn, &mut send, "requires_code").await;
            }
            sas = Some(pake::sas_from_binding(&binding));
        }
    }
    if purpose == Purpose::RingSync && !trusted {
        return reject(&conn, &mut send, "not_trusted").await;
    }
    if purpose == Purpose::Link {
        let open = inner.link_window.lock().map(|t| t > Instant::now()).unwrap_or(false);
        if !(code_auth && open) {
            return reject(&conn, &mut send, "link_not_open").await;
        }
    }
    if code_auth {
        // A Wormhole code is single use.
        inner.rotate_code();
    }
    // Strangers (Ask) only get a working resume token once the user accepts.
    let token: [u8; 32] = rand::random();
    if auth != Auth::Ask {
        inner.register_token(token, peer_key);
    }
    let me = inner.me();
    write_msg(&mut send, &Msg::Welcome { device: me, trusted, resume_token: token }).await?;

    match purpose {
        Purpose::Identify => Ok(()),
        Purpose::RingSync => {
            if let Msg::Ring(theirs) = read_msg(&mut recv).await? {
                merge_ring(inner, theirs);
            }
            write_msg(&mut send, &Msg::Ring(ring_entries(inner))).await?;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Purpose::Link => {
            inner.trust.upsert(peer_key, &device.name, device.kind, TrustOrigin::Linked)?;
            *inner.link_window.lock() = None;
            inner.ring_synced.lock().clear();
            write_msg(&mut send, &Msg::Ack).await?;
            if let Some(d) = inner.trusted_list().into_iter().find(|d| d.public_key_hex == hex::encode(peer_key)) {
                inner.bus.emit(WarpEvent::DeviceLinked { device: d });
            }
            inner.emit_trust();
            inner.mark_peers_dirty();
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Purpose::Clipboard => {
            let content = match read_msg(&mut recv).await? {
                Msg::Clipboard(c) => c,
                m => bail!("unexpected {m:?}"),
            };
            let size = match &content {
                ClipboardContent::Text { text } => text.len(),
                ClipboardContent::Url { url } => url.len(),
                ClipboardContent::Image { data, .. } => data.len(),
            };
            if size > MAX_CLIPBOARD {
                return reject(&conn, &mut send, "too_large").await;
            }
            inner.bus.emit(WarpEvent::ClipboardReceived {
                from_device_id: device_id_from_key(&peer_key),
                from_name: device.name.clone(),
                content,
            });
            write_msg(&mut send, &Msg::Ack).await?;
            let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
            Ok(())
        }
        Purpose::Transfer => {
            let ctx = OfferCtx { kind, peer_key, device, trusted, auth, sas, token, resume_manifest, via_rendezvous };
            serve_transfer(inner, conn, send, recv, ctx).await
        }
    }
}

/// Everything `serve` learned about the sender before the offer arrives.
struct OfferCtx {
    kind: PathKind,
    peer_key: [u8; 32],
    device: DeviceInfo,
    trusted: bool,
    auth: Auth,
    sas: Option<Vec<String>>,
    token: [u8; 32],
    /// Set when authenticated by a resumption ticket: the manifest it covers.
    resume_manifest: Option<[u8; 32]>,
    via_rendezvous: bool,
}

/// Whether what is still missing of this transfer fits in the free space of
/// the download volume. Unknown free space counts as "fits".
async fn fits_on_disk(download_dir: PathBuf, peer_key: &[u8; 32], manifest: &Manifest) -> bool {
    let stage = download_dir.join(PARTIAL_DIR).join(receiver::staging_key(peer_key, manifest));
    let total = manifest.total_bytes();
    tokio::task::spawn_blocking(move || {
        let staged: u64 = walkdir::WalkDir::new(&stage)
            .into_iter()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum();
        crate::fsx::available_space(&download_dir).is_none_or(|free| total.saturating_sub(staged) <= free)
    })
    .await
    .unwrap_or(true)
}

async fn serve_transfer(
    inner: &Arc<Inner>,
    conn: quinn::Connection,
    mut send: quinn::SendStream,
    mut recv: quinn::RecvStream,
    ctx: OfferCtx,
) -> Result<()> {
    let OfferCtx { kind, peer_key, device, trusted, auth, sas, token, resume_manifest, via_rendezvous } = ctx;
    let manifest: Manifest = match read_msg(&mut recv).await? {
        Msg::Offer { manifest } => manifest,
        m => bail!("unexpected {m:?}"),
    };
    if let Err(e) = manifest::validate(&manifest) {
        reject(&conn, &mut send, "invalid_manifest").await?;
        return Err(e);
    }
    let limits = crate::limits::Limits::for_device(inner.cfg.read().transfer_memory_mb);
    if !limits.allows(&manifest) {
        tracing::info!("declining a transfer over this device's budget ({limits:?})");
        return reject(&conn, &mut send, "too_large").await;
    }
    if !fits_on_disk(inner.download_dir(), &peer_key, &manifest).await {
        // Also bounds the receiver's per-chunk bookkeeping by real disk size.
        return reject(&conn, &mut send, "no_space").await;
    }
    let fingerprint = manifest.fingerprint();
    if resume_manifest.is_some_and(|m| m != fingerprint) {
        // The ticket covers exactly one transfer.
        return reject(&conn, &mut send, "ticket_mismatch").await;
    }
    let needs_prompt = match auth {
        Auth::Ask => true,
        Auth::Trusted => !inner.cfg.read().auto_accept_trusted,
        Auth::Code | Auth::Token(_) | Auth::Resume(_) => false,
    };
    if needs_prompt {
        let offer_id = new_id();
        let (tx, rx) = oneshot::channel();
        inner.offers.lock().insert(offer_id.clone(), tx);
        inner.bus.emit(WarpEvent::OfferReceived {
            offer: IncomingOffer {
                offer_id: offer_id.clone(),
                peer_id: device_id_from_key(&peer_key),
                peer_name: device.name.clone(),
                peer_kind: device.kind,
                trusted,
                item_count: manifest.top_level().len() as u64,
                total_bytes: manifest.total_bytes(),
                title: manifest.title(),
                sas_emoji: sas.clone().unwrap_or_default(),
            },
        });
        let accepted = tokio::select! {
            r = tokio::time::timeout(OFFER_TIMEOUT, rx) => matches!(r, Ok(Ok(true))),
            _ = conn.closed() => false,
        };
        inner.offers.lock().remove(&offer_id);
        if !accepted {
            inner.bus.emit(WarpEvent::OfferWithdrawn { offer_id });
            return reject(&conn, &mut send, "declined").await;
        }
        if auth == Auth::Ask {
            inner.register_token(token, peer_key);
        }
    }

    let download_dir = inner.download_dir();
    let stage_key = receiver::staging_key(&peer_key, &manifest);
    let stage = download_dir.join(PARTIAL_DIR).join(&stage_key);
    let t = {
        let mut by_stage = inner.incoming_by_stage.lock();
        match by_stage.get(&stage_key) {
            Some(t) if !t.state().is_terminal() => {
                if let Some(old) = t.conn.lock().take() {
                    old.close(quinn::VarInt::from_u32(close::CANCELLED), b"superseded");
                }
                t.clone()
            }
            _ => {
                let t = Transfer::new(new_id(), TransferDirection::Incoming, device_id_from_key(&peer_key), device.name.clone(), inner.bus.clone());
                by_stage.insert(stage_key.clone(), t.clone());
                t
            }
        }
    };
    inner.register_transfer(&t);
    let gen = t.next_generation();
    t.set_meta(manifest.title(), manifest.file_count(), manifest.total_bytes());
    *t.conn.lock() = Some(conn.clone());
    t.set_path(kind);

    let ticket = inner.tickets.issue(peer_key, fingerprint, via_rendezvous);
    inner.sync_ticket_channels();
    let r = tokio::select! {
        r = receiver::receive(&conn, &mut send, &mut recv, receiver::Job { manifest, stage, download_dir, ticket }, t.clone()) => r,
        _ = t.cancelled() => Err(WarpError::Cancelled.into()),
    };
    let drop_ticket = |inner: &Inner| {
        inner.tickets.remove_for(&peer_key, &fingerprint);
        inner.sync_ticket_channels();
    };
    if !t.is_generation(gen) {
        return Ok(()); // superseded by a newer connection
    }
    match r {
        Ok(rx) => {
            t.finish_ok(rx.saved_paths);
            inner.incoming_by_stage.lock().remove(&stage_key);
            // Completed staging areas are kept briefly for idempotent retries.
            let dl = inner.download_dir();
            tokio::task::spawn_blocking(move || receiver::cleanup_stale(&dl));
            drop_ticket(inner);
            let _ = tokio::time::timeout(Duration::from_secs(3), conn.closed()).await;
            Ok(())
        }
        Err(e) if !t.is_cancelled() && retryable(&e) => {
            // The sender will most likely reconnect and resume (its ticket
            // stays valid for 30 minutes, even after this UI state times out).
            t.set_state(TransferState::Reconnecting);
            let (inner, t) = (inner.clone(), t.clone());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(150)).await;
                if t.is_generation(gen) && t.state() == TransferState::Reconnecting {
                    t.finish_err("connection lost".into(), "unreachable");
                    inner.incoming_by_stage.lock().remove(&stage_key);
                }
            });
            Ok(())
        }
        Err(e) => {
            fail(&t, &e);
            inner.incoming_by_stage.lock().remove(&stage_key);
            drop_ticket(inner);
            Err(e)
        }
    }
}

// ------------------------------------------------------ background tasks ----

fn spawn_background(inner: &Arc<Inner>, rt: &tokio::runtime::Handle) {
    // QUIC accept loop.
    {
        let inner = inner.clone();
        rt.spawn(async move {
            while let Some(incoming) = inner.endpoint.accept().await {
                let inner = inner.clone();
                tokio::spawn(async move {
                    let remote = incoming.remote_address();
                    match incoming.await {
                        Ok(conn) => {
                            let kind = inner.ifs.path_to(&remote);
                            handle_conn(inner, conn, kind, false);
                        }
                        Err(e) => tracing::debug!("handshake from {remote} failed: {e}"),
                    }
                });
            }
        });
    }
    // Code rotation + peer table sweeping + ring sync triggering.
    {
        let inner = inner.clone();
        rt.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            let mut n = 0u64;
            loop {
                tick.tick().await;
                n += 1;
                let expired = inner.code.lock().expires <= Instant::now();
                if expired {
                    inner.rotate_code();
                }
                if n.is_multiple_of(30) && inner.tickets.expire() {
                    inner.sync_ticket_channels();
                }
                if n.is_multiple_of(3) && inner.peers.sweep() {
                    inner.mark_peers_dirty();
                }
            }
        });
    }
    // Debounced PeersChanged / TrustRingChanged emission + ring sync.
    {
        let inner = inner.clone();
        rt.spawn(async move {
            loop {
                inner.peers_dirty.notified().await;
                tokio::time::sleep(Duration::from_millis(150)).await;
                inner.bus.emit(WarpEvent::PeersChanged { peers: inner.peer_infos() });
                inner.emit_trust();
                for p in inner.peers.all() {
                    let Some(k) = p.public_key else { continue };
                    if !inner.trust.contains(&k) || !inner.ring_synced.lock().insert(p.device_id.clone()) {
                        continue;
                    }
                    let (inner, id) = (inner.clone(), p.device_id.clone());
                    tokio::spawn(async move {
                        if let Err(e) = ring_sync(inner.clone(), id.clone()).await {
                            tracing::debug!("ring sync with {id} failed: {e:#}");
                            inner.ring_synced.lock().remove(&id);
                        }
                    });
                }
            }
        });
    }
    let cfg = inner.cfg.read().clone();
    let on_change: Arc<dyn Fn() + Send + Sync> = {
        let inner = Arc::downgrade(inner);
        Arc::new(move || {
            if let Some(i) = inner.upgrade() {
                i.mark_peers_dirty();
            }
        })
    };
    if cfg.enable_mdns {
        let _guard = rt.enter();
        match Mdns::start(inner.peers.clone(), on_change.clone()) {
            Ok(m) => {
                if let Err(e) = m.advertise(&inner.announce(), inner.port()) {
                    tracing::warn!("mDNS advertise failed: {e:#}");
                }
                *inner.mdns.lock() = Some(m);
            }
            Err(e) => tracing::warn!("mDNS unavailable: {e:#}"),
        }
    }
    if cfg.enable_beacon {
        let weak = Arc::downgrade(inner);
        let announce: Arc<dyn Fn() -> (Announce, u16) + Send + Sync> = Arc::new(move || match weak.upgrade() {
            Some(i) => (i.announce(), i.port()),
            None => (Announce { device_id: String::new(), name: String::new(), kind: DeviceKind::Unknown, public_key: None, code_open: false, stream_port: None }, 0),
        });
        rt.spawn(beacon::run(cfg.beacon_port, announce, inner.peers.clone(), on_change.clone()));
    }
    restart_relay_listener(inner, rt);
    // QUIC over TCP (LAN fast path): one QUIC endpoint per accepted stream.
    if let Some(listener) = inner.stream_listener.lock().take() {
        let inner = inner.clone();
        rt.spawn(async move {
            while let Ok((tcp, remote)) = listener.accept().await {
                let inner = inner.clone();
                tokio::spawn(async move {
                    let Ok(ep) = sockets::tunnel_server_endpoint(tcp, inner.server_cfg.clone()) else { return };
                    let accepted = tokio::time::timeout(Duration::from_secs(10), ep.accept()).await;
                    if let Ok(Some(incoming)) = accepted {
                        if let Ok(conn) = incoming.await {
                            let kind = inner.ifs.path_to(&remote);
                            handle_conn(inner, conn.clone(), kind, false);
                            conn.closed().await;
                        }
                    }
                    drop(ep);
                });
            }
        });
    }
    if let Some(port) = cfg.tunnel_listen_port {
        let inner = inner.clone();
        rt.spawn(async move {
            let l = match tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
                Ok(l) => l,
                Err(e) => return tracing::warn!("tunnel listener disabled: {e}"),
            };
            while let Ok((tcp, _)) = l.accept().await {
                let inner = inner.clone();
                tokio::spawn(async move {
                    let Ok(ep) = sockets::tunnel_server_endpoint(tcp, inner.server_cfg.clone()) else { return };
                    if let Some(incoming) = ep.accept().await {
                        if let Ok(conn) = incoming.await {
                            handle_conn(inner, conn.clone(), PathKind::AdbUsb, false);
                            conn.closed().await;
                        }
                    }
                    drop(ep);
                });
            }
        });
    }
    if cfg.enable_adb {
        let inner = inner.clone();
        rt.spawn(async move {
            let mut known: HashMap<String, u16> = HashMap::new();
            loop {
                if let Some(adb_path) = adb::find_adb() {
                    let p = adb_path.clone();
                    let serials = tokio::task::spawn_blocking(move || adb::devices(&p)).await.unwrap_or_default();
                    known.retain(|s, port| {
                        let keep = serials.contains(s);
                        if !keep && inner.peers.remove_addr(&PeerAddr::Tunnel(*port)) {
                            inner.mark_peers_dirty();
                        }
                        keep
                    });
                    for serial in serials {
                        let p = adb_path.clone();
                        let s2 = serial.clone();
                        let Ok(Some(port)) = tokio::task::spawn_blocking(move || adb::forward(&p, &s2, discovery::DEFAULT_TUNNEL_PORT)).await else {
                            continue;
                        };
                        known.insert(serial, port);
                        if let Ok(s) = identify_tunnel(&inner, port).await {
                            let a = Announce {
                                device_id: device_id_from_key(&s.0),
                                name: s.1.name,
                                kind: s.1.kind,
                                public_key: Some(s.0),
                                code_open: true,
                                stream_port: None,
                            };
                            if inner.peers.upsert(&a, &[PeerAddr::Tunnel(port)], Source::Adb) {
                                inner.mark_peers_dirty();
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }
}

/// (Re)starts the rendezvous listener for the configured relay URL.
fn restart_relay_listener(inner: &Arc<Inner>, rt: &tokio::runtime::Handle) {
    if let Some(old) = inner.relay_task.lock().take() {
        old.abort();
    }
    let (url, force) = {
        let c = inner.cfg.read();
        (c.relay_url.clone(), c.force_relay)
    };
    let Some(url) = url.filter(|u| !u.trim().is_empty()) else { return };
    let weak = Arc::downgrade(inner);
    let on_conn: rendezvous::ConnHandler = Arc::new(move |conn, kind| {
        if let Some(i) = weak.upgrade() {
            handle_conn(i, conn, kind, true);
        }
    });
    let task = rt.spawn(rendezvous::run_listener(
        url,
        inner.listen_tx.subscribe(),
        inner.server_cfg.clone(),
        inner.ifs.clone(),
        force,
        on_conn,
    ));
    *inner.relay_task.lock() = Some(task);
}

async fn identify_tunnel(inner: &Arc<Inner>, port: u16) -> Result<([u8; 32], DeviceInfo)> {
    let client = tls::client_config(&inner.identity, None)?;
    let conn = sockets::connect_tunnel(SocketAddr::from(([127, 0, 0, 1], port)), client).await?;
    let s = open_session(inner, conn, PathKind::AdbUsb, Auth::Ask, None, Purpose::Identify).await?;
    s.conn.close(quinn::VarInt::from_u32(close::DONE), b"done");
    let _ = s.sas;
    Ok((s.peer_key, s.peer))
}

// ------------------------------------------------------------ public API ----

impl WarpNode {
    fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(&self, f: F) {
        self.handle.spawn(f);
    }

    fn transfer(&self, id: &str) -> WarpResult<Arc<Transfer>> {
        self.inner.transfers.lock().get(id).cloned().ok_or(WarpError::TransferNotFound)
    }

    fn new_outgoing(&self, peer_id: &str, peer_name: &str) -> Arc<Transfer> {
        let t = Transfer::new(new_id(), TransferDirection::Outgoing, peer_id.to_string(), peer_name.to_string(), self.inner.bus.clone());
        self.inner.register_transfer(&t);
        t.emit(true);
        t
    }

    fn peer_display_name(&self, peer_id: &str) -> String {
        self.inner
            .peers
            .get(peer_id)
            .map(|p| p.name)
            .or_else(|| self.inner.trust.by_device_id(peer_id).map(|e| e.name))
            .unwrap_or_default()
    }

    fn code_target(input: &str) -> WarpResult<Target> {
        let t = parse_target(input).map_err(|_| WarpError::InvalidCode)?;
        Ok(Target::Code { code: t.code.clone(), ticket: Some(t) })
    }

    /// Rust-side access for embedders (tests, Tauri) to the runtime handle.
    pub fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.handle.clone()
    }
}

#[uniffi::export]
impl WarpNode {
    /// Creates the node and starts networking and discovery immediately.
    #[uniffi::constructor]
    pub fn new(config: NodeConfig, listener: Arc<dyn WarpEventListener>) -> WarpResult<Arc<Self>> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let state_dir = PathBuf::from(&config.state_dir);
        std::fs::create_dir_all(&state_dir).map_err(WarpError::io)?;
        std::fs::create_dir_all(&config.download_dir).map_err(WarpError::io)?;
        let identity = match &config.identity_seed {
            Some(seed) => Identity::from_seed(seed),
            None => Identity::load_or_create(&state_dir),
        }
        .map_err(WarpError::crypto)?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("warp-core")
            .worker_threads(std::thread::available_parallelism().map(|n| n.get().clamp(2, 8)).unwrap_or(4))
            .build()
            .map_err(WarpError::io)?;
        let handle = rt.handle().clone();
        let server_cfg = tls::server_config(&identity).map_err(WarpError::crypto)?;
        let endpoint = {
            let _g = rt.enter();
            let sock = transport::bind_udp(config.port).map_err(WarpError::io)?;
            let mut ep = quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server_cfg.clone()), sock, transport::runtime())
                .map_err(WarpError::io)?;
            ep.set_default_client_config(tls::client_config(&identity, None).map_err(WarpError::crypto)?);
            ep
        };
        let stream_listener = {
            let _g = rt.enter();
            let udp_port = endpoint.local_addr().map(|a| a.port()).unwrap_or(0);
            match transport::bind_tcp(udp_port) {
                Ok(l) => Some(l),
                Err(e) => {
                    tracing::warn!("stream carriage disabled: {e}");
                    None
                }
            }
        };
        let stream_port = stream_listener.as_ref().and_then(|l| l.local_addr().ok()).map(|a| a.port()).unwrap_or(0);
        receiver::cleanup_stale(std::path::Path::new(&config.download_dir));
        let code = pake::generate_code();
        let secs = config.code_rotation_secs.max(30) as u64;
        let peers = Arc::new(PeerTable::default());
        peers.set_self(&identity.device_id());
        let inner = Arc::new(Inner {
            trust: TrustStore::open(state_dir.join("trust-ring.json")),
            cfg: RwLock::new(config),
            identity,
            peers,
            ifs: Arc::new(InterfaceTable::default()),
            bus: EventBus::new(listener),
            endpoint,
            stream_port,
            stream_listener: Mutex::new(stream_listener),
            server_cfg,
            code: Mutex::new(CodeState {
                code: code.clone(),
                expires: Instant::now() + Duration::from_secs(secs),
                expires_ms: now_ms() + secs * 1000,
                failures: 0,
            }),
            listen_tx: watch::channel(rendezvous::ListenSet { code, resume: vec![] }).0,
            tickets: TicketStore::default(),
            link_window: Mutex::new(None),
            transfers: Mutex::new(HashMap::new()),
            incoming_by_stage: Mutex::new(HashMap::new()),
            offers: Mutex::new(HashMap::new()),
            issued_tokens: Mutex::new(HashMap::new()),
            held_tokens: Mutex::new(HashMap::new()),
            ring_synced: Mutex::new(HashSet::new()),
            mdns: Mutex::new(None),
            relay_task: Mutex::new(None),
            peers_dirty: Notify::new(),
            shutting_down: AtomicBool::new(false),
        });
        spawn_background(&inner, &handle);
        inner.bus.emit(WarpEvent::CodeChanged { code: inner.code_info() });
        Ok(Arc::new(WarpNode { rt: Mutex::new(Some(rt)), handle, inner }))
    }

    pub fn identity(&self) -> DeviceIdentity {
        let me = self.inner.me();
        DeviceIdentity {
            device_id: self.inner.identity.device_id(),
            name: me.name,
            kind: me.kind,
            public_key_hex: hex::encode(self.inner.identity.public_key()),
            fingerprint: fingerprint(self.inner.identity.public_key()),
        }
    }

    /// The 32-byte identity seed, so the platform can move it into the
    /// Keychain / Keystore and pass it back via `NodeConfig.identity_seed`.
    pub fn export_identity_seed(&self) -> Vec<u8> {
        self.inner.identity.seed().to_vec()
    }

    pub fn port(&self) -> u16 {
        self.inner.port()
    }

    pub fn current_code(&self) -> CodeInfo {
        self.inner.code_info()
    }

    pub fn rotate_code(&self) {
        self.inner.rotate_code();
    }

    /// TXT record etc. for platform-native service registration.
    pub fn advertisement(&self) -> ServiceAdvert {
        discovery::service_advert(&self.inner.announce(), self.inner.port())
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        self.inner.peer_infos()
    }

    pub fn interfaces(&self) -> Vec<NetInterface> {
        self.inner.ifs.public_list()
    }

    /// Feed a peer found by Bonjour (NWBrowser, incl. AWDL) or NsdManager.
    pub fn inject_peer(&self, advert: PeerAdvert) {
        if let Some((a, addrs)) = discovery::parse_native(&advert) {
            if self.inner.peers.upsert(&a, &addrs, Source::Native) {
                self.inner.mark_peers_dirty();
            }
        }
    }

    pub fn remove_injected_peer(&self, device_id: String) {
        if self.inner.peers.remove_source(&device_id, Source::Native) {
            self.inner.mark_peers_dirty();
        }
    }

    /// Registers a QUIC-over-TCP tunnel on `127.0.0.1:port` (e.g. a manual
    /// `adb forward tcp:PORT tcp:47821`). The device appears in `peers()`.
    pub fn add_tunnel(&self, port: u16) {
        let inner = self.inner.clone();
        self.spawn(async move {
            match identify_tunnel(&inner, port).await {
                Ok((key, info)) => {
                    let a = Announce { device_id: device_id_from_key(&key), name: info.name, kind: info.kind, public_key: Some(key), code_open: true, stream_port: None };
                    if inner.peers.upsert(&a, &[PeerAddr::Tunnel(port)], Source::Hint) {
                        inner.mark_peers_dirty();
                    }
                }
                Err(e) => inner.bus.emit(WarpEvent::Error { message: format!("tunnel {port}: {}", user_message(&e)) }),
            }
        });
    }

    pub fn trusted_devices(&self) -> Vec<TrustedDevice> {
        self.inner.trusted_list()
    }

    /// Adds a key to the trust ring (e.g. synced through iCloud KVS).
    pub fn trust_device(&self, public_key_hex: String, name: String, kind: DeviceKind, origin: TrustOrigin) -> WarpResult<()> {
        let key = parse_public_key_hex(&public_key_hex).map_err(WarpError::crypto)?;
        if &key == self.inner.identity.public_key() {
            return Ok(());
        }
        if self.inner.trust.upsert(key, &name, kind, origin).map_err(WarpError::io)? {
            self.inner.emit_trust();
            self.inner.mark_peers_dirty();
        }
        Ok(())
    }

    pub fn untrust_device(&self, device_id: String) -> WarpResult<()> {
        if self.inner.trust.remove(&device_id).map_err(WarpError::io)? {
            self.inner.ring_synced.lock().remove(&device_id);
            self.inner.emit_trust();
            self.inner.mark_peers_dirty();
        }
        Ok(())
    }

    /// Allow "Link My Device" requests (authenticated by our code) for a while.
    pub fn open_link_window(&self, seconds: u32) {
        *self.inner.link_window.lock() = Some(Instant::now() + Duration::from_secs(seconds.clamp(10, 900) as u64));
    }

    pub fn close_link_window(&self) {
        *self.inner.link_window.lock() = None;
    }

    /// Joins the Personal Trust Ring of the device showing `code` (whose user
    /// has opened its link window). Result arrives as `DeviceLinked` / `Error`.
    pub fn link_with_code(&self, code: String) -> WarpResult<()> {
        let target = Self::code_target(&code)?;
        let inner = self.inner.clone();
        self.spawn(async move {
            let r: Result<()> = async {
                let mut s = connect_target(&inner, &target, Purpose::Link).await?;
                match read_msg(&mut s.recv).await? {
                    Msg::Ack => {}
                    Msg::Reject { reason } => return Err(WarpError::Rejected { reason }.into()),
                    m => bail!("unexpected {m:?}"),
                }
                inner.trust.upsert(s.peer_key, &s.peer.name, s.peer.kind, TrustOrigin::Linked)?;
                s.conn.close(quinn::VarInt::from_u32(close::DONE), b"done");
                inner.ring_synced.lock().clear();
                if let Some(d) = inner.trusted_list().into_iter().find(|d| d.public_key_hex == hex::encode(s.peer_key)) {
                    inner.bus.emit(WarpEvent::DeviceLinked { device: d });
                }
                inner.emit_trust();
                inner.mark_peers_dirty();
                Ok(())
            }
            .await;
            if let Err(e) = r {
                inner.bus.emit(WarpEvent::Error { message: user_message(&e) });
            }
        });
        Ok(())
    }

    /// Starts sending to a discovered or trusted device. Returns the transfer id.
    pub fn send_files(&self, peer_id: String, items: Vec<SendItem>) -> WarpResult<String> {
        if items.is_empty() {
            return Err(WarpError::Config { msg: "nothing to send".into() });
        }
        let t = self.new_outgoing(&peer_id, &self.peer_display_name(&peer_id));
        self.spawn(run_outgoing(self.inner.clone(), t.clone(), Target::Peer(peer_id), items));
        Ok(t.id.clone())
    }

    /// Sends to whoever shows this 6-digit code (or `warpdrop:` QR payload).
    pub fn send_files_to_code(&self, code: String, items: Vec<SendItem>) -> WarpResult<String> {
        if items.is_empty() {
            return Err(WarpError::Config { msg: "nothing to send".into() });
        }
        let target = Self::code_target(&code)?;
        let t = self.new_outgoing("", "");
        self.spawn(run_outgoing(self.inner.clone(), t.clone(), target, items));
        Ok(t.id.clone())
    }

    pub fn send_clipboard(&self, peer_id: String, content: ClipboardContent) -> WarpResult<String> {
        let t = self.new_outgoing(&peer_id, &self.peer_display_name(&peer_id));
        self.start_clipboard(t.clone(), Target::Peer(peer_id), content);
        Ok(t.id.clone())
    }

    pub fn send_clipboard_to_code(&self, code: String, content: ClipboardContent) -> WarpResult<String> {
        let target = Self::code_target(&code)?;
        let t = self.new_outgoing("", "");
        self.start_clipboard(t.clone(), target, content);
        Ok(t.id.clone())
    }

    pub fn respond_to_offer(&self, offer_id: String, accept: bool) {
        if let Some(tx) = self.inner.offers.lock().remove(&offer_id) {
            let _ = tx.send(accept);
        }
    }

    pub fn pause_transfer(&self, id: String) -> WarpResult<()> {
        let t = self.transfer(&id)?;
        if t.direction == TransferDirection::Outgoing && !t.state().is_terminal() {
            t.pause(true);
        }
        Ok(())
    }

    pub fn resume_transfer(&self, id: String) -> WarpResult<()> {
        let t = self.transfer(&id)?;
        if t.is_paused() {
            t.pause(false);
        }
        Ok(())
    }

    pub fn cancel_transfer(&self, id: String) -> WarpResult<()> {
        let t = self.transfer(&id)?;
        t.cancel();
        if t.direction == TransferDirection::Incoming {
            // Receiver cancelled on purpose: drop its partial data.
            let mut by_stage = self.inner.incoming_by_stage.lock();
            if let Some(k) = by_stage.iter().find(|(_, v)| v.id == id).map(|(k, _)| k.clone()) {
                by_stage.remove(&k);
                let _ = std::fs::remove_dir_all(self.inner.download_dir().join(PARTIAL_DIR).join(k));
            }
        }
        Ok(())
    }

    pub fn transfers(&self) -> Vec<TransferSnapshot> {
        self.inner.transfers.lock().values().map(|t| t.snapshot()).collect()
    }

    pub fn clear_finished_transfers(&self) {
        self.inner.transfers.lock().retain(|_, t| !t.state().is_terminal());
    }

    pub fn set_device_name(&self, name: String) {
        self.inner.cfg.write().device_name = name;
        if let Some(m) = self.inner.mdns.lock().as_ref() {
            let _ = m.advertise(&self.inner.announce(), self.inner.port());
        }
    }

    pub fn set_download_dir(&self, dir: String) -> WarpResult<()> {
        std::fs::create_dir_all(&dir).map_err(WarpError::io)?;
        self.inner.cfg.write().download_dir = dir;
        Ok(())
    }

    pub fn download_dir(&self) -> String {
        self.inner.cfg.read().download_dir.clone()
    }

    /// Takes effect immediately (the rendezvous listener restarts).
    pub fn set_relay_url(&self, url: Option<String>) {
        self.inner.cfg.write().relay_url = url;
        restart_relay_listener(&self.inner, &self.handle);
    }

    /// Stops discovery and closes all connections.
    pub fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        for t in self.inner.transfers.lock().values() {
            if !t.state().is_terminal() && t.direction == TransferDirection::Outgoing {
                t.cancel();
            }
        }
        if let Some(m) = self.inner.mdns.lock().take() {
            m.shutdown();
        }
        self.inner.endpoint.close(quinn::VarInt::from_u32(close::CANCELLED), b"shutdown");
    }
}

impl WarpNode {
    fn start_clipboard(&self, t: Arc<Transfer>, target: Target, content: ClipboardContent) {
        let (title, len) = match &content {
            ClipboardContent::Text { text } => ("Clipboard".to_string(), text.len()),
            ClipboardContent::Url { url } => (url.clone(), url.len()),
            ClipboardContent::Image { data, .. } => ("Image".to_string(), data.len()),
        };
        t.set_meta(title, 1, len as u64);
        self.spawn(run_clipboard(self.inner.clone(), t, target, content));
    }
}

/// Renders any string (normally `CodeInfo.qr_payload`) as a QR module matrix
/// so every UI can draw it natively and crisply.
#[uniffi::export]
pub fn qr_matrix(text: String) -> WarpResult<QrMatrix> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::M)
        .map_err(|e| WarpError::Config { msg: e.to_string() })?;
    let size = code.width() as u32;
    let modules = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    Ok(QrMatrix { size, modules })
}

#[uniffi::export]
pub fn normalize_code(input: String) -> Option<String> {
    parse_target(&input).ok().map(|t| t.code)
}

#[uniffi::export]
pub fn format_code(code: String) -> String {
    pake::display_code(&code)
}

#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
