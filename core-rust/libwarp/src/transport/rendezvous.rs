//! Internet path: Wormhole-code rendezvous, UDP hole punching, relay fallback.
//!
//! 1. Receiver keeps a WebSocket to `warp-signal` and *listens* on
//!    `channel = Argon2id(code)`. The sender *connects* to the same channel.
//! 2. The server pairs them into a session and each side learns its public
//!    UDP mapping by probing the server's UDP port (STUN-like).
//! 3. Candidates (public + LAN addresses) are exchanged over the WebSocket.
//! 4. Both sides fire punch packets at each other; the sender then attempts a
//!    direct QUIC handshake. If NAT types prevent it (symmetric NAT), QUIC
//!    runs through the UDP relay instead — the relay forwards opaque QUIC
//!    packets and never holds a key.
//!
//! The QUIC session then performs the normal SPAKE2-over-TLS-exporter
//! authentication, so neither path trusts the server.

use super::sockets::{relay_header, RelaySocket, RELAY_HDR, RELAY_MAGIC, RELAY_PROBE, RELAY_PROBE_ACK};
use super::{bind_udp_v4, race_connect, Candidate};
use crate::discovery::PeerAddr;
use crate::netif::InterfaceTable;
use crate::types::PathKind;
use crate::WarpError;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// `persistent = false`: the rotating code channel (replaces the previous
    /// one). `persistent = true`: an additional resume-ticket channel.
    Listen {
        channel: String,
        #[serde(default)]
        persistent: bool,
    },
    Unlisten { channel: String },
    Connect { channel: String },
    Signal { session: String, data: Candidates },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Paired {
        session: String,
        side: u8,
        udp_port: u16,
        /// Secret for authenticated UDP binding (relays since 1.0.1).
        #[serde(default)]
        bind_key: Option<String>,
    },
    Signal { session: String, data: Candidates },
    Error {
        code: String,
        #[serde(default)]
        channel: Option<String>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Candidates {
    pub public: Option<SocketAddr>,
    pub local: Vec<SocketAddr>,
}

pub type ConnHandler = Arc<dyn Fn(quinn::Connection, PathKind) + Send + Sync>;

/// What the receiver wants to be reachable under on the rendezvous server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListenSet {
    /// Current Wormhole code (hashed with Argon2id before it leaves the device).
    pub code: String,
    /// Resume-ticket channels (already hashed, see `resume::resume_channel`).
    pub resume: Vec<String>,
}

struct Session {
    id: [u8; 16],
    side: u8,
    relay_udp: SocketAddr,
    bind_key: Option<[u8; 32]>,
}

fn parse_bind_key(k: Option<&str>) -> Option<[u8; 32]> {
    hex::decode(k?).ok()?.try_into().ok()
}

/// Concurrent rendezvous sessions a listener serves; extra pairings are ignored.
const MAX_LISTENER_SESSIONS: usize = 16;

fn ws_url(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/v1/ws") {
        b.to_string()
    } else {
        format!("{b}/v1/ws")
    }
}

async fn relay_host_addr(base: &str, udp_port: u16) -> Result<SocketAddr> {
    let url = url::Url::parse(base).context("invalid relay URL")?;
    let host = url.host_str().ok_or_else(|| anyhow!("relay URL has no host"))?.to_string();
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), udp_port)).await?.collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| anyhow!("cannot resolve relay host"))
}

/// Learns our public UDP mapping as seen by the relay.
async fn probe(sock: &tokio::net::UdpSocket, s: &Session) -> Option<SocketAddr> {
    let pkt = relay_header(RELAY_PROBE, &s.id, s.side);
    let mut buf = [0u8; 256];
    for _ in 0..4 {
        let _ = sock.send_to(&pkt, s.relay_udp).await;
        if let Ok(Ok((n, _))) = tokio::time::timeout(Duration::from_millis(400), sock.recv_from(&mut buf)).await {
            if n > RELAY_HDR && &buf[..4] == RELAY_MAGIC && buf[4] == RELAY_PROBE_ACK {
                if let Ok(a) = std::str::from_utf8(&buf[RELAY_HDR..n]).unwrap_or("").parse() {
                    return Some(a);
                }
            }
        }
    }
    None
}

fn local_candidates(port: u16, ifs: &InterfaceTable) -> Vec<SocketAddr> {
    ifs.advertised_ips()
        .into_iter()
        .filter(|ip| matches!(ip, IpAddr::V4(v4) if !v4.is_loopback()))
        .map(|ip| SocketAddr::new(ip, port))
        .collect()
}

fn punch(sock: &std::net::UdpSocket, peer: &Candidates) {
    let Ok(s) = sock.try_clone() else { return };
    let mut targets: Vec<SocketAddr> = peer.local.clone();
    targets.extend(peer.public);
    std::thread::spawn(move || {
        for _ in 0..12 {
            for t in &targets {
                let _ = s.send_to(b"WDPUNCH", t);
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    });
}

struct Prepared {
    direct: std::net::UdpSocket,
    relay: Arc<RelaySocket>,
    mine: Candidates,
}

async fn prepare(s: &Session, ifs: &InterfaceTable) -> Result<Prepared> {
    let direct = tokio::net::UdpSocket::from_std(bind_udp_v4()?)?;
    let public = probe(&direct, s).await;
    let port = direct.local_addr()?.port();
    let relay_io = tokio::net::UdpSocket::from_std(bind_udp_v4()?)?;
    let relay = RelaySocket::new(relay_io, s.relay_udp, s.id, s.side, s.bind_key);
    relay.bind_on_relay().await?;
    Ok(Prepared {
        direct: direct.into_std()?,
        relay,
        mine: Candidates { public, local: local_candidates(port, ifs) },
    })
}

fn parse_session(hex_id: &str) -> Result<[u8; 16]> {
    hex::decode(hex_id)?.try_into().map_err(|_| anyhow!("bad session id"))
}

// ------------------------------------------------------------ receiver ----

/// Keeps the receiver registered on the relay under its current code and all
/// live resume-ticket channels.
pub async fn run_listener(
    relay_url: String,
    mut code_rx: watch::Receiver<ListenSet>,
    server: quinn::ServerConfig,
    ifs: Arc<InterfaceTable>,
    force_relay: bool,
    on_conn: ConnHandler,
) {
    let mut backoff = 1u64;
    loop {
        match listener_once(&relay_url, &mut code_rx, &server, &ifs, force_relay, &on_conn).await {
            Ok(()) => return,
            Err(e) => tracing::debug!("relay listener: {e:#}"),
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

async fn listener_once(
    relay_url: &str,
    code_rx: &mut watch::Receiver<ListenSet>,
    server: &quinn::ServerConfig,
    ifs: &Arc<InterfaceTable>,
    force_relay: bool,
    on_conn: &ConnHandler,
) -> Result<()> {
    let (ws, _) = tokio_tungstenite::connect_async(ws_url(relay_url)).await?;
    let (mut sink, mut stream) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let text = serde_json::to_string(&m).unwrap_or_default();
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
    });
    let mut sessions: std::collections::HashMap<String, mpsc::UnboundedSender<Candidates>> = Default::default();
    // What the server currently has for this connection.
    let mut sent = ListenSet::default();
    // Argon2id hash of `sent.code`, kept to re-send it without recomputing.
    let mut code_channel = String::new();
    // Channels another connection still holds ("channel_busy"), retried below.
    let mut busy: Vec<(String, bool)> = vec![];
    let mut retry = tokio::time::interval(Duration::from_secs(10));
    let sync = |want: &ListenSet, sent: &mut ListenSet, code_channel: &mut String| -> Result<()> {
        let gone = || anyhow!("relay writer gone");
        if want.code != sent.code {
            let channel = crate::pake::rendezvous_channel(&want.code)?;
            *code_channel = channel.clone();
            out_tx.send(ClientMsg::Listen { channel, persistent: false }).map_err(|_| gone())?;
        }
        for ch in want.resume.iter().filter(|c| !sent.resume.contains(c)) {
            out_tx.send(ClientMsg::Listen { channel: ch.clone(), persistent: true }).map_err(|_| gone())?;
        }
        for ch in sent.resume.iter().filter(|c| !want.resume.contains(c)) {
            out_tx.send(ClientMsg::Unlisten { channel: ch.clone() }).map_err(|_| gone())?;
        }
        *sent = want.clone();
        Ok(())
    };
    sync(&code_rx.borrow_and_update().clone(), &mut sent, &mut code_channel)?;
    let result = loop {
        tokio::select! {
            changed = code_rx.changed() => {
                if changed.is_err() { break Ok(()); }
                let want = code_rx.borrow_and_update().clone();
                sync(&want, &mut sent, &mut code_channel)?;
            }
            _ = retry.tick(), if !busy.is_empty() => {
                // E.g. our own previous connection that the server has not
                // noticed is dead yet. Only channels we still want.
                for (channel, persistent) in busy.drain(..) {
                    let wanted = if persistent { sent.resume.contains(&channel) } else { channel == code_channel };
                    if wanted {
                        out_tx.send(ClientMsg::Listen { channel, persistent }).map_err(|_| anyhow!("relay writer gone"))?;
                    }
                }
            }
            msg = stream.next() => {
                let Some(msg) = msg else { break Err(anyhow!("relay closed the connection")) };
                let Message::Text(t) = msg? else { continue };
                let Ok(m) = serde_json::from_str::<ServerMsg>(t.as_str()) else { continue };
                match m {
                    ServerMsg::Paired { session, side, udp_port, bind_key } => {
                        sessions.retain(|_, tx| !tx.is_closed());
                        if sessions.len() >= MAX_LISTENER_SESSIONS {
                            tracing::debug!("relay: too many concurrent sessions, ignoring pairing");
                            continue;
                        }
                        let (tx, rx) = mpsc::unbounded_channel();
                        sessions.insert(session.clone(), tx);
                        let relay_udp = relay_host_addr(relay_url, udp_port).await?;
                        let s = Session { id: parse_session(&session)?, side, relay_udp, bind_key: parse_bind_key(bind_key.as_deref()) };
                        let (server, ifs, on_conn, out) = (server.clone(), ifs.clone(), on_conn.clone(), out_tx.clone());
                        tokio::spawn(async move {
                            if let Err(e) = serve_session(s, session, rx, out, server, &ifs, force_relay, on_conn).await {
                                tracing::debug!("relay session failed: {e:#}");
                            }
                        });
                        // The code is single-use once someone reached us.
                    }
                    ServerMsg::Signal { session, data } => {
                        if let Some(tx) = sessions.get(&session) { let _ = tx.send(data); }
                    }
                    ServerMsg::Error { code, channel: Some(channel) } if code == "channel_busy" => {
                        let persistent = channel != code_channel;
                        busy.push((channel, persistent));
                    }
                    ServerMsg::Error { code, .. } => tracing::debug!("relay error: {code}"),
                }
            }
        }
    };
    writer.abort();
    result
}

#[allow(clippy::too_many_arguments)]
async fn serve_session(
    s: Session,
    session_hex: String,
    mut sig: mpsc::UnboundedReceiver<Candidates>,
    out: mpsc::UnboundedSender<ClientMsg>,
    server: quinn::ServerConfig,
    ifs: &InterfaceTable,
    force_relay: bool,
    on_conn: ConnHandler,
) -> Result<()> {
    let p = prepare(&s, ifs).await?;
    out.send(ClientMsg::Signal { session: session_hex, data: p.mine.clone() })?;
    let peer = tokio::time::timeout(Duration::from_secs(10), sig.recv())
        .await?
        .ok_or_else(|| anyhow!("peer vanished"))?;

    let mut relay_server = server.clone();
    relay_server.transport_config(super::sockets::relay_transport());
    let relay_ep = quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        Some(relay_server),
        p.relay.clone(),
        super::runtime(),
    )?;
    let direct_ep = if force_relay {
        None
    } else {
        punch(&p.direct, &peer);
        Some(quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(server), p.direct, super::runtime())?)
    };
    let accept_direct = async {
        match &direct_ep {
            Some(ep) => ep.accept().await,
            None => std::future::pending().await,
        }
    };
    let (incoming, relayed) = tokio::time::timeout(Duration::from_secs(40), async {
        tokio::select! {
            i = accept_direct => (i, false),
            i = relay_ep.accept() => (i, true),
        }
    })
    .await?;
    let incoming = incoming.ok_or_else(|| anyhow!("endpoint closed"))?;
    let remote = incoming.remote_address();
    let conn = incoming.await?;
    let kind = if relayed { PathKind::Relay } else { classify_direct(ifs, &remote) };
    on_conn(conn.clone(), kind);
    // Keep both endpoints alive for the lifetime of the connection.
    conn.closed().await;
    drop(direct_ep);
    drop(relay_ep);
    Ok(())
}

fn classify_direct(ifs: &InterfaceTable, remote: &SocketAddr) -> PathKind {
    ifs.path_to(remote)
}

// -------------------------------------------------------------- sender ----

pub struct Dialed {
    pub conn: quinn::Connection,
    pub kind: PathKind,
    pub endpoint: quinn::Endpoint,
}

async fn next_msg<S>(stream: &mut S) -> Result<ServerMsg>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let fut = async {
        loop {
            let Some(msg) = stream.next().await else { bail!("relay closed the connection") };
            if let Message::Text(t) = msg? {
                if let Ok(m) = serde_json::from_str::<ServerMsg>(t.as_str()) {
                    return Ok(m);
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), fut).await?
}

/// Reaches whoever listens on `channel` (a code channel from
/// `pake::rendezvous_channel`, or a resume channel).
pub async fn dial(
    relay_url: &str,
    channel: String,
    client: quinn::ClientConfig,
    ifs: &InterfaceTable,
    force_relay: bool,
) -> Result<Dialed> {
    let (ws, _) = tokio::time::timeout(Duration::from_secs(8), tokio_tungstenite::connect_async(ws_url(relay_url)))
        .await
        .map_err(|_| WarpError::Unreachable { msg: "relay server timed out".into() })??;
    let (mut sink, mut stream) = ws.split();
    let send = |m: ClientMsg| Message::text(serde_json::to_string(&m).unwrap_or_default());
    sink.send(send(ClientMsg::Connect { channel })).await?;

    let (session_hex, side, udp_port, bind_key) = match next_msg(&mut stream).await? {
        ServerMsg::Paired { session, side, udp_port, bind_key } => (session, side, udp_port, bind_key),
        ServerMsg::Error { code, .. } if code == "no_listener" => return Err(WarpError::CodeNotFound.into()),
        other => bail!("unexpected relay message: {other:?}"),
    };
    let s = Session {
        id: parse_session(&session_hex)?,
        side,
        relay_udp: relay_host_addr(relay_url, udp_port).await?,
        bind_key: parse_bind_key(bind_key.as_deref()),
    };
    let p = prepare(&s, ifs).await?;
    sink.send(send(ClientMsg::Signal { session: session_hex, data: p.mine.clone() })).await?;
    let peer = loop {
        if let ServerMsg::Signal { data, .. } = next_msg(&mut stream).await? {
            break data;
        }
    };

    if !force_relay {
        punch(&p.direct, &peer);
        let ep = quinn::Endpoint::new(quinn::EndpointConfig::default(), None, p.direct, super::runtime())?;
        let mut cands: Vec<Candidate> = peer
            .local
            .iter()
            .map(|a| Candidate { addr: PeerAddr::Udp(*a), kind: ifs.path_to(a) })
            .collect();
        if let Some(public) = peer.public {
            cands.push(Candidate { addr: PeerAddr::Udp(public), kind: PathKind::InternetP2p });
        }
        cands.sort_by_key(|c| c.kind.rank());
        // Give the receiver's punch packets a moment to open its NAT.
        tokio::time::sleep(Duration::from_millis(150)).await;
        match tokio::time::timeout(Duration::from_secs(4), race_connect(&ep, client.clone(), cands)).await {
            Ok(Ok((conn, kind))) => {
                let kind = if kind == PathKind::Loopback && !conn.remote_address().ip().is_loopback() {
                    PathKind::Lan
                } else {
                    kind
                };
                return Ok(Dialed { conn, kind, endpoint: ep });
            }
            Ok(Err(e)) => tracing::debug!("direct path failed, using relay: {e:#}"),
            Err(_) => tracing::debug!("hole punching timed out, using relay"),
        }
    }

    let ep = quinn::Endpoint::new_with_abstract_socket(quinn::EndpointConfig::default(), None, p.relay.clone(), super::runtime())?;
    let mut client = client;
    client.transport_config(super::sockets::relay_transport());
    let conn = tokio::time::timeout(
        Duration::from_secs(12),
        ep.connect_with(client, p.relay.relay_addr(), crate::tls::SERVER_NAME)?,
    )
    .await
    .map_err(|_| WarpError::Unreachable { msg: "relay handshake timed out".into() })??;
    let _ = sink.close().await;
    Ok(Dialed { conn, kind: PathKind::Relay, endpoint: ep })
}
