//! QUIC endpoints and multi-path connection establishment.

pub mod adb;
pub mod rendezvous;
pub mod sockets;

use crate::discovery::PeerAddr;
use crate::netif::InterfaceTable;
use crate::tls::SERVER_NAME;
use crate::types::PathKind;
use anyhow::{anyhow, Result};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// Dual-stack UDP socket tuned for bulk transfer.
pub fn bind_udp(port: u16) -> std::io::Result<std::net::UdpSocket> {
    let try_bind = |port: u16| -> std::io::Result<Socket> {
        match Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)) {
            Ok(s) => {
                s.set_only_v6(false)?;
                s.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
                Ok(s)
            }
            Err(_) => {
                let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
                s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
                Ok(s)
            }
        }
    };
    let s = try_bind(port).or_else(|_| try_bind(0))?;
    tune(&s);
    s.set_nonblocking(true)?;
    Ok(s.into())
}

pub fn bind_udp_v4() -> std::io::Result<std::net::UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())?;
    tune(&s);
    s.set_nonblocking(true)?;
    Ok(s.into())
}

fn tune(s: &Socket) {
    let _ = s.set_recv_buffer_size(8 * 1024 * 1024);
    let _ = s.set_send_buffer_size(8 * 1024 * 1024);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        // SO_RECV_ANYIF: also receive on restricted interfaces such as AWDL
        // (awdl0) and the USB-NCM link to an iPhone, so peers found through
        // Bonjour peer-to-peer browsing are reachable from this socket.
        use std::os::fd::AsRawFd;
        const SO_RECV_ANYIF: libc::c_int = 0x1104;
        let one: libc::c_int = 1;
        unsafe {
            libc::setsockopt(
                s.as_raw_fd(),
                libc::SOL_SOCKET,
                SO_RECV_ANYIF,
                &one as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
    }
}

/// Dual-stack TCP listener for the stream carriage, preferably on the same
/// port number as the UDP endpoint.
pub fn bind_tcp(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    let try_bind = |port: u16| -> std::io::Result<Socket> {
        let s = match Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP)) {
            Ok(s) => {
                s.set_only_v6(false)?;
                s.set_reuse_address(true)?;
                s.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
                s
            }
            Err(_) => {
                let s = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
                s.set_reuse_address(true)?;
                s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
                s
            }
        };
        s.listen(128)?;
        Ok(s)
    };
    let s = try_bind(port).or_else(|_| try_bind(0))?;
    s.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(s.into())
}

pub fn runtime() -> Arc<dyn quinn::Runtime> {
    Arc::new(quinn::TokioRuntime)
}

pub struct Candidate {
    pub addr: PeerAddr,
    pub kind: PathKind,
}

pub fn rank(addrs: impl IntoIterator<Item = PeerAddr>, ifs: &InterfaceTable) -> Vec<Candidate> {
    let mut v: Vec<Candidate> = addrs
        .into_iter()
        .map(|addr| Candidate {
            kind: match addr {
                PeerAddr::Udp(sa) => ifs.path_to(&sa),
                PeerAddr::Tcp(sa) => ifs.path_to(&sa),
                PeerAddr::Tunnel(_) => PathKind::AdbUsb,
            },
            addr,
        })
        .collect();
    // Within a link kind: stream carriage first (kernel TCP offloads, jumbo
    // QUIC packets), then native UDP; IPv4 before IPv6.
    v.sort_by_key(|c| {
        let (udp, v6) = match c.addr {
            PeerAddr::Udp(a) => (true, a.is_ipv6()),
            PeerAddr::Tcp(a) => (false, a.is_ipv6()),
            PeerAddr::Tunnel(_) => (false, false),
        };
        (c.kind.rank(), udp, v6)
    });
    v
}

/// Happy-eyeballs across paths: start with the best-ranked candidate and
/// launch the next one every 250 ms until one completes the handshake.
pub async fn race_connect(
    endpoint: &quinn::Endpoint,
    client: quinn::ClientConfig,
    candidates: Vec<Candidate>,
) -> Result<(quinn::Connection, PathKind)> {
    if candidates.is_empty() {
        return Err(crate::WarpError::Unreachable { msg: "no known address for this device".into() }.into());
    }
    let mut set = tokio::task::JoinSet::new();
    for (i, c) in candidates.into_iter().enumerate() {
        let (endpoint, client) = (endpoint.clone(), client.clone());
        set.spawn(async move {
            tokio::time::sleep(Duration::from_millis(250 * i as u64)).await;
            let conn = match c.addr {
                PeerAddr::Udp(sa) => {
                    let connecting = endpoint.connect_with(client, sa, SERVER_NAME)?;
                    tokio::time::timeout(Duration::from_secs(6), connecting).await??
                }
                PeerAddr::Tcp(sa) => sockets::connect_tunnel(sa, client).await?,
                PeerAddr::Tunnel(port) => sockets::connect_tunnel(SocketAddr::from(([127, 0, 0, 1], port)), client).await?,
            };
            anyhow::Ok((conn, c.kind))
        });
    }
    let mut last_err = None;
    while let Some(r) = set.join_next().await {
        match r {
            Ok(Ok(ok)) => {
                set.abort_all();
                return Ok(ok);
            }
            Ok(Err(e)) => last_err = Some(e),
            Err(e) => last_err = Some(anyhow!("connect task failed: {e}")),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no path to device")))
}
