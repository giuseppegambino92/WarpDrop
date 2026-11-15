//! Abuse cases for `warp-signal`: channel squatting, UDP session hijacking,
//! request floods. Raw WebSocket/UDP clients against a local relay.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio_tungstenite::tungstenite::Message;
use warp::transport::sockets::{relay_bind_mac, relay_header, RELAY_BIND, RELAY_BIND_ACK, RELAY_DATA, RELAY_HDR, RELAY_NEED_BIND, RELAY_PROBE, RELAY_PROBE_ACK};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws(relay: SocketAddr) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{relay}/v1/ws")).await.unwrap().0
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::text(v.to_string())).await.unwrap();
}

/// Next JSON message, or `None` within `ms`.
async fn recv(ws: &mut Ws, ms: u64) -> Option<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let m = tokio::time::timeout_at(deadline, ws.next()).await.ok()??.ok()?;
        if let Message::Text(t) = m {
            return serde_json::from_str(t.as_str()).ok();
        }
    }
}

fn channel(n: u8) -> String {
    hex::encode([n; 32])
}

struct Paired {
    session: [u8; 16],
    side: u8,
    key: [u8; 32],
}

fn paired(v: &Value) -> Paired {
    assert_eq!(v["t"], "paired", "{v}");
    Paired {
        session: hex::decode(v["session"].as_str().unwrap()).unwrap().try_into().unwrap(),
        side: v["side"].as_u64().unwrap() as u8,
        key: hex::decode(v["bind_key"].as_str().unwrap()).unwrap().try_into().unwrap(),
    }
}

async fn pair(relay: SocketAddr, ch: u8) -> (Ws, Paired, Ws, Paired) {
    let mut listener = ws(relay).await;
    send(&mut listener, json!({"t": "listen", "channel": channel(ch)})).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut dialer = ws(relay).await;
    send(&mut dialer, json!({"t": "connect", "channel": channel(ch)})).await;
    let a = paired(&recv(&mut listener, 2000).await.unwrap());
    let b = paired(&recv(&mut dialer, 2000).await.unwrap());
    assert_eq!(a.session, b.session);
    assert_ne!(a.key, b.key);
    (listener, a, dialer, b)
}

async fn udp() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").await.unwrap()
}

/// A side binds the way current clients do: probe, then MAC over the observed address.
async fn bind(sock: &UdpSocket, relay: SocketAddr, p: &Paired) {
    sock.send_to(&relay_header(RELAY_PROBE, &p.session, p.side), relay).await.unwrap();
    let mut buf = [0u8; 256];
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await.unwrap().unwrap();
    assert_eq!(buf[4], RELAY_PROBE_ACK);
    let observed: SocketAddr = std::str::from_utf8(&buf[RELAY_HDR..n]).unwrap().parse().unwrap();
    assert_eq!(observed, sock.local_addr().unwrap());
    let mut pkt = relay_header(RELAY_BIND, &p.session, p.side).to_vec();
    pkt.extend_from_slice(&relay_bind_mac(&p.key, &p.session, p.side, &observed));
    sock.send_to(&pkt, relay).await.unwrap();
    let (n, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf)).await.unwrap().unwrap();
    assert_eq!((n, buf[4]), (RELAY_HDR, RELAY_BIND_ACK));
}

fn data(p: &Paired, payload: &[u8]) -> Vec<u8> {
    let mut pkt = relay_header(RELAY_DATA, &p.session, p.side).to_vec();
    pkt.extend_from_slice(payload);
    pkt
}

async fn recv_udp(sock: &UdpSocket, ms: u64) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 2048];
    let (n, _) = tokio::time::timeout(Duration::from_millis(ms), sock.recv_from(&mut buf)).await.ok()?.ok()?;
    buf.truncate(n);
    Some(buf)
}

#[test]
fn bind_mac_matches_the_relay() {
    let (k, s, a): ([u8; 32], [u8; 16], SocketAddr) = ([7; 32], [9; 16], "203.0.113.5:4242".parse().unwrap());
    assert_eq!(relay_bind_mac(&k, &s, 1, &a), warp_signal::bind_mac(&k, &s, 1, &a));
}

#[tokio::test]
async fn a_live_channel_cannot_be_taken_over() {
    let relay = warp_signal::spawn_local().await.unwrap();
    let mut owner = ws(relay).await;
    send(&mut owner, json!({"t": "listen", "channel": channel(1)})).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut squatter = ws(relay).await;
    send(&mut squatter, json!({"t": "listen", "channel": channel(1), "persistent": true})).await;
    let e = recv(&mut squatter, 2000).await.unwrap();
    assert_eq!((e["code"].as_str(), e["channel"].as_str()), (Some("channel_busy"), Some(channel(1).as_str())));

    // Dialers still reach the owner.
    let mut dialer = ws(relay).await;
    send(&mut dialer, json!({"t": "connect", "channel": channel(1)})).await;
    assert_eq!(recv(&mut owner, 2000).await.unwrap()["t"], "paired");
    assert!(recv(&mut squatter, 300).await.is_none());

    // Once the owner leaves, the channel is free again.
    drop(owner);
    tokio::time::sleep(Duration::from_millis(200)).await;
    send(&mut squatter, json!({"t": "listen", "channel": channel(1)})).await;
    assert!(recv(&mut squatter, 300).await.is_none(), "no error this time");
}

#[tokio::test]
async fn udp_sides_cannot_be_hijacked() {
    let relay = warp_signal::spawn_local().await.unwrap();
    let (_l, a, _d, b) = pair(relay, 2).await;
    let (sa, sb, attacker) = (udp().await, udp().await, udp().await);
    bind(&sa, relay, &a).await;
    bind(&sb, relay, &b).await;

    sb.send_to(&data(&b, b"hello"), relay).await.unwrap();
    assert!(recv_udp(&sa, 1000).await.unwrap().ends_with(b"hello"));

    // Someone who learned the session id sends DATA as side A: not forwarded,
    // and A's binding stays. The attacker only gets a NEED_BIND (no bigger
    // than what it sent).
    let pkt = data(&a, &[0u8; 64]);
    attacker.send_to(&pkt, relay).await.unwrap();
    let nb = recv_udp(&attacker, 1000).await.unwrap();
    assert_eq!(nb[4], RELAY_NEED_BIND);
    assert!(nb.len() <= pkt.len());
    assert!(recv_udp(&sb, 300).await.is_none(), "attacker packet must not be relayed");
    sb.send_to(&data(&b, b"still A"), relay).await.unwrap();
    assert!(recv_udp(&sa, 1000).await.unwrap().ends_with(b"still A"));

    // A BIND with a MAC for someone else's address (sniffed and replayed), or
    // with a guessed key, is ignored.
    let mut replay = relay_header(RELAY_BIND, &a.session, a.side).to_vec();
    replay.extend_from_slice(&relay_bind_mac(&a.key, &a.session, a.side, &sa.local_addr().unwrap()));
    attacker.send_to(&replay, relay).await.unwrap();
    let mut forged = relay_header(RELAY_BIND, &a.session, a.side).to_vec();
    forged.extend_from_slice(&relay_bind_mac(&[0; 32], &a.session, a.side, &attacker.local_addr().unwrap()));
    attacker.send_to(&forged, relay).await.unwrap();
    assert!(recv_udp(&attacker, 300).await.is_none(), "no BIND_ACK");
    sb.send_to(&data(&b, b"again A"), relay).await.unwrap();
    assert!(recv_udp(&sa, 1000).await.unwrap().ends_with(b"again A"));

    // The real side A moving to a new address (NAT rebinding) works.
    let sa2 = udp().await;
    bind(&sa2, relay, &a).await;
    sb.send_to(&data(&b, b"moved"), relay).await.unwrap();
    assert!(recv_udp(&sa2, 1000).await.unwrap().ends_with(b"moved"));
}

#[tokio::test]
async fn connect_floods_are_rate_limited() {
    let relay = warp_signal::spawn_local().await.unwrap();
    let mut listener = ws(relay).await;
    send(&mut listener, json!({"t": "listen", "channel": channel(3)})).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut flood = ws(relay).await;
    for _ in 0..40 {
        send(&mut flood, json!({"t": "connect", "channel": channel(3)})).await;
    }
    let mut paired = 0;
    let mut limited = 0;
    while let Some(m) = recv(&mut flood, 500).await {
        match (m["t"].as_str(), m["code"].as_str()) {
            (Some("paired"), _) => paired += 1,
            (_, Some("rate_limited")) => limited += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(paired, 10, "burst");
    assert_eq!(limited, 30);
}

#[tokio::test]
async fn connections_per_ip_are_capped() {
    let relay = warp_signal::spawn_local().await.unwrap();
    let mut open = vec![];
    for _ in 0..32 {
        open.push(ws(relay).await);
    }
    assert!(tokio_tungstenite::connect_async(format!("ws://{relay}/v1/ws")).await.is_err(), "33rd is refused");
    open.pop();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(tokio_tungstenite::connect_async(format!("ws://{relay}/v1/ws")).await.is_ok(), "slot freed");
}

async fn relay_with(cfg: warp_signal::Config) -> SocketAddr {
    loop {
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        if let Ok(udp) = UdpSocket::bind(addr).await {
            tokio::spawn(warp_signal::serve(tcp, udp, cfg));
            return addr;
        }
    }
}

#[tokio::test]
async fn unauthenticated_first_binding_is_refused_by_default() {
    let relay = warp_signal::spawn_local().await.unwrap();
    let (_l, a, _d, b) = pair(relay, 4).await;
    let (sa, sb, attacker) = (udp().await, udp().await, udp().await);
    bind(&sb, relay, &b).await;

    // Before A binds, someone with the session id tries to claim side A.
    let pkt = data(&a, &[0u8; 64]);
    attacker.send_to(&pkt, relay).await.unwrap();
    assert_eq!(recv_udp(&attacker, 1000).await.unwrap()[4], RELAY_NEED_BIND);
    sb.send_to(&data(&b, b"for A"), relay).await.unwrap();
    assert!(recv_udp(&attacker, 300).await.is_none(), "attacker never receives A's traffic");

    // The real A, still unbound, gets NEED_BIND for its first QUIC packet,
    // binds with its key, and from then on receives.
    sa.send_to(&data(&a, &[0u8; 64]), relay).await.unwrap();
    assert_eq!(recv_udp(&sa, 1000).await.unwrap()[4], RELAY_NEED_BIND);
    bind(&sa, relay, &a).await;
    sb.send_to(&data(&b, b"hi A"), relay).await.unwrap();
    assert!(recv_udp(&sa, 1000).await.unwrap().ends_with(b"hi A"));
}

#[tokio::test]
async fn legacy_binding_is_an_explicit_opt_in() {
    let relay = relay_with(warp_signal::Config { legacy_bind: true, ..Default::default() }).await;
    let (_l, a, _d, b) = pair(relay, 5).await;
    let (old_a, sb, attacker) = (udp().await, udp().await, udp().await);
    bind(&sb, relay, &b).await;
    // A v1.0.0 client binds with an empty DATA packet…
    old_a.send_to(&relay_header(RELAY_DATA, &a.session, a.side), relay).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    sb.send_to(&data(&b, b"old client"), relay).await.unwrap();
    assert!(recv_udp(&old_a, 1000).await.unwrap().ends_with(b"old client"));
    // …and still nobody can move it without the key.
    attacker.send_to(&data(&a, &[0u8; 64]), relay).await.unwrap();
    sb.send_to(&data(&b, b"still old"), relay).await.unwrap();
    assert!(recv_udp(&old_a, 1000).await.unwrap().ends_with(b"still old"));
}
