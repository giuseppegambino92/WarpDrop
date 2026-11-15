//! UDP broadcast beacon: a fallback for routers/APs that drop multicast
//! (mDNS) between clients. Every 3 s each node sends a tiny datagram to the
//! directed broadcast address of each IPv4 interface.

use super::{Announce, PeerAddr, PeerTable, Source};
use crate::types::DeviceKind;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

const MAGIC: &[u8; 4] = b"WDB1";

#[derive(Serialize, Deserialize)]
struct Beacon {
    pk: [u8; 32],
    name: String,
    kind: DeviceKind,
    port: u16,
    code_open: bool,
    stream_port: u16,
}

fn bind(port: u16) -> std::io::Result<tokio::net::UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris")))]
    s.set_reuse_port(true)?;
    s.set_broadcast(true)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    tokio::net::UdpSocket::from_std(s.into())
}

fn broadcast_targets() -> Vec<Ipv4Addr> {
    let mut v = vec![Ipv4Addr::BROADCAST];
    if let Ok(ifs) = if_addrs::get_if_addrs() {
        for i in ifs {
            if let if_addrs::IfAddr::V4(a) = i.addr {
                if let Some(b) = a.broadcast {
                    if !a.ip.is_loopback() && !v.contains(&b) {
                        v.push(b);
                    }
                }
            }
        }
    }
    v
}

pub async fn run(
    port: u16,
    announce: Arc<dyn Fn() -> (Announce, u16) + Send + Sync>,
    table: Arc<PeerTable>,
    on_change: Arc<dyn Fn() + Send + Sync>,
) {
    let sock = match bind(port) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::warn!("beacon disabled: cannot bind UDP {port}: {e}");
            return;
        }
    };
    let tx = sock.clone();
    let sender = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        loop {
            tick.tick().await;
            let (a, quic_port) = announce();
            let Some(pk) = a.public_key else { continue };
            let b = Beacon { pk, name: a.name, kind: a.kind, port: quic_port, code_open: a.code_open, stream_port: a.stream_port.unwrap_or(0) };
            let mut pkt = MAGIC.to_vec();
            pkt.extend(postcard::to_stdvec(&b).unwrap_or_default());
            for target in broadcast_targets() {
                let _ = tx.send_to(&pkt, SocketAddr::from((target, port))).await;
            }
        }
    });
    let mut buf = vec![0u8; 2048];
    loop {
        let Ok((n, from)) = sock.recv_from(&mut buf).await else { break };
        if n < 4 || &buf[..4] != MAGIC {
            continue;
        }
        let Ok(b) = postcard::from_bytes::<Beacon>(&buf[4..n]) else { continue };
        let a = Announce {
            device_id: crate::identity::device_id_from_key(&b.pk),
            name: b.name,
            kind: b.kind,
            public_key: Some(b.pk),
            code_open: b.code_open,
            stream_port: (b.stream_port != 0).then_some(b.stream_port),
        };
        let ip = match from.ip() {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            v4 => v4,
        };
        let addrs = super::with_stream_addrs(vec![PeerAddr::Udp(SocketAddr::new(ip, b.port))], a.stream_port);
        if table.upsert(&a, &addrs, Source::Beacon) {
            on_change();
        }
    }
    sender.abort();
}
