//! Custom datagram sockets that let the *same* QUIC + TLS 1.3 stack run over
//! paths that are not plain UDP:
//!
//! * [`RelaySocket`] — QUIC packets wrapped in a 22-byte header and forwarded
//!   by `warp-signal`'s UDP relay. The relay only ever sees QUIC ciphertext.
//! * [`TunnelSocket`] — QUIC packets length-prefixed over a TCP byte stream,
//!   which is what `adb forward` can carry over a USB cable.

use anyhow::Result;
use parking_lot::Mutex;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll, Waker};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadBuf};
use tokio::sync::mpsc;

// ---------------------------------------------------------------- relay ----

pub const RELAY_MAGIC: &[u8; 4] = b"WRP1";
pub const RELAY_PROBE: u8 = 1;
pub const RELAY_PROBE_ACK: u8 = 2;
pub const RELAY_DATA: u8 = 3;
/// magic(4) | type(1) | session(16) | side(1)
pub const RELAY_HDR: usize = 22;

pub fn relay_header(kind: u8, session: &[u8; 16], side: u8) -> [u8; RELAY_HDR] {
    let mut h = [0u8; RELAY_HDR];
    h[..4].copy_from_slice(RELAY_MAGIC);
    h[4] = kind;
    h[5..21].copy_from_slice(session);
    h[21] = side;
    h
}

pub struct RelaySocket {
    io: tokio::net::UdpSocket,
    relay: SocketAddr,
    session: [u8; 16],
    side: u8,
    buf: Mutex<Vec<u8>>,
}

impl fmt::Debug for RelaySocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelaySocket").field("relay", &self.relay).finish()
    }
}

impl RelaySocket {
    pub fn new(io: tokio::net::UdpSocket, relay: SocketAddr, session: [u8; 16], side: u8) -> Arc<Self> {
        Arc::new(RelaySocket { io, relay, session, side, buf: Mutex::new(vec![0u8; 65536]) })
    }

    /// Registers this socket's address with the relay (empty DATA packet).
    pub async fn bind_on_relay(&self) -> io::Result<()> {
        let h = relay_header(RELAY_DATA, &self.session, self.side);
        self.io.send_to(&h, self.relay).await?;
        Ok(())
    }

    pub fn relay_addr(&self) -> SocketAddr {
        self.relay
    }
}

#[derive(Debug)]
struct RelayPoller(Arc<RelaySocket>);

impl UdpPoller for RelayPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.io.poll_send_ready(cx)
    }
}

impl AsyncUdpSocket for RelaySocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(RelayPoller(self))
    }

    fn try_send(&self, t: &Transmit<'_>) -> io::Result<()> {
        let mut pkt = Vec::with_capacity(RELAY_HDR + t.contents.len());
        pkt.extend_from_slice(&relay_header(RELAY_DATA, &self.session, self.side));
        pkt.extend_from_slice(t.contents);
        self.io.try_send_to(&pkt, self.relay).map(|_| ())
    }

    fn poll_recv(&self, cx: &mut Context<'_>, bufs: &mut [IoSliceMut<'_>], meta: &mut [RecvMeta]) -> Poll<io::Result<usize>> {
        let mut tmp = self.buf.lock();
        loop {
            let mut rb = ReadBuf::new(&mut tmp[..]);
            let from = ready!(self.io.poll_recv_from(cx, &mut rb))?;
            let pkt = rb.filled();
            if from != self.relay
                || pkt.len() <= RELAY_HDR
                || &pkt[..4] != RELAY_MAGIC
                || pkt[4] != RELAY_DATA
                || pkt[5..21] != self.session
            {
                continue;
            }
            let payload = &pkt[RELAY_HDR..];
            let n = payload.len().min(bufs[0].len());
            bufs[0][..n].copy_from_slice(&payload[..n]);
            meta[0] = RecvMeta { addr: self.relay, len: n, stride: n, ..Default::default() };
            return Poll::Ready(Ok(1));
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

// --------------------------------------------------------------- tunnel ----

const TUNNEL_QUEUE_LIMIT: usize = 4 * 1024 * 1024;

pub struct TunnelSocket {
    rx: Mutex<mpsc::Receiver<Vec<u8>>>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    waker: Arc<Mutex<Option<Waker>>>,
    closed: Arc<AtomicBool>,
    local: SocketAddr,
    peer: SocketAddr,
}

impl fmt::Debug for TunnelSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelSocket").field("peer", &self.peer).finish()
    }
}

impl TunnelSocket {
    pub fn new(stream: tokio::net::TcpStream) -> io::Result<Arc<Self>> {
        stream.set_nodelay(true)?;
        let local = stream.local_addr()?;
        let peer = stream.peer_addr()?;
        let (rd, mut wr) = stream.into_split();
        let mut rd = tokio::io::BufReader::with_capacity(512 * 1024, rd);
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(1024);
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let queued = Arc::new(AtomicUsize::new(0));
        let waker: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
        let closed = Arc::new(AtomicBool::new(false));

        let c = closed.clone();
        tokio::spawn(async move {
            let mut len = [0u8; 2];
            loop {
                if rd.read_exact(&mut len).await.is_err() {
                    break;
                }
                let n = u16::from_be_bytes(len) as usize;
                let mut pkt = vec![0u8; n];
                if rd.read_exact(&mut pkt).await.is_err() || in_tx.send(pkt).await.is_err() {
                    break;
                }
            }
            c.store(true, Ordering::Release);
        });
        let (q, w, c) = (queued.clone(), waker.clone(), closed.clone());
        tokio::spawn(async move {
            let mut frame = Vec::with_capacity(512 * 1024);
            while let Some(first) = out_rx.recv().await {
                // Coalesce everything already queued into one write.
                frame.clear();
                let mut bytes = 0;
                let mut next = Some(first);
                while let Some(pkt) = next {
                    bytes += pkt.len();
                    frame.extend_from_slice(&(pkt.len() as u16).to_be_bytes());
                    frame.extend_from_slice(&pkt);
                    next = if frame.len() < 256 * 1024 { out_rx.try_recv().ok() } else { None };
                }
                if wr.write_all(&frame).await.is_err() {
                    break;
                }
                q.fetch_sub(bytes, Ordering::AcqRel);
                if let Some(wk) = w.lock().take() {
                    wk.wake();
                }
            }
            c.store(true, Ordering::Release);
        });
        Ok(Arc::new(TunnelSocket {
            rx: Mutex::new(in_rx),
            tx: out_tx,
            queued,
            waker,
            closed,
            local,
            peer,
        }))
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }
}

#[derive(Debug)]
struct TunnelPoller(Arc<TunnelSocket>);

impl UdpPoller for TunnelPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let s = &self.0;
        if s.queued.load(Ordering::Acquire) < TUNNEL_QUEUE_LIMIT {
            return Poll::Ready(Ok(()));
        }
        *s.waker.lock() = Some(cx.waker().clone());
        if s.queued.load(Ordering::Acquire) < TUNNEL_QUEUE_LIMIT {
            return Poll::Ready(Ok(()));
        }
        Poll::Pending
    }
}

impl AsyncUdpSocket for TunnelSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(TunnelPoller(self))
    }

    fn try_send(&self, t: &Transmit<'_>) -> io::Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "tunnel closed"));
        }
        if self.queued.load(Ordering::Acquire) >= TUNNEL_QUEUE_LIMIT {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        // GSO is disabled (max_transmit_segments = 1) so this is one datagram.
        self.queued.fetch_add(t.contents.len(), Ordering::AcqRel);
        self.tx
            .send(t.contents.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "tunnel closed"))
    }

    fn poll_recv(&self, cx: &mut Context<'_>, bufs: &mut [IoSliceMut<'_>], meta: &mut [RecvMeta]) -> Poll<io::Result<usize>> {
        let mut rx = self.rx.lock();
        match ready!(rx.poll_recv(cx)) {
            Some(pkt) => {
                let n = pkt.len().min(bufs[0].len());
                bufs[0][..n].copy_from_slice(&pkt[..n]);
                meta[0] = RecvMeta { addr: self.peer, len: n, stride: n, ..Default::default() };
                Poll::Ready(Ok(1))
            }
            None => Poll::Pending, // tunnel closed: QUIC idle timeout ends the connection
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

/// Datagram size used inside stream tunnels. The byte stream never drops or
/// fragments, so QUIC can use jumbo "packets": ~11× fewer per-packet crypto
/// and bookkeeping operations than 1452-byte UDP datagrams.
pub const TUNNEL_MTU: u16 = 16 * 1024;

fn tunnel_transport() -> Arc<quinn::TransportConfig> {
    let mut t = crate::tls::transport_config();
    t.initial_mtu(TUNNEL_MTU);
    t.min_mtu(TUNNEL_MTU);
    t.mtu_discovery_config(None);
    // The default initial window (14 720 B) is smaller than one jumbo packet.
    let mut cc = quinn::congestion::CubicConfig::default();
    cc.initial_window(32 * TUNNEL_MTU as u64);
    t.congestion_controller_factory(Arc::new(cc));
    Arc::new(t)
}

pub fn tunnel_endpoint_config() -> quinn::EndpointConfig {
    let mut c = quinn::EndpointConfig::default();
    c.max_udp_payload_size(TUNNEL_MTU).expect("valid payload size");
    c
}

pub fn relay_transport() -> Arc<quinn::TransportConfig> {
    let mut t = crate::tls::transport_config();
    t.mtu_discovery_config(None);
    Arc::new(t)
}

/// Client side: QUIC through a TCP stream to `addr` (a peer's stream port, or
/// `127.0.0.1:<adb forward port>`).
pub async fn connect_tunnel(addr: SocketAddr, mut client: quinn::ClientConfig) -> Result<quinn::Connection> {
    let tcp = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(addr)).await??;
    let _ = socket2::SockRef::from(&tcp).set_send_buffer_size(4 * 1024 * 1024);
    let sock = TunnelSocket::new(tcp)?;
    let peer = sock.peer();
    let ep = quinn::Endpoint::new_with_abstract_socket(tunnel_endpoint_config(), None, sock, super::runtime())?;
    client.transport_config(tunnel_transport());
    let conn = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        ep.connect_with(client, peer, crate::tls::SERVER_NAME)?,
    )
    .await??;
    Ok(conn)
}

/// Server side: wraps one accepted TCP tunnel in its own QUIC endpoint.
pub fn tunnel_server_endpoint(tcp: tokio::net::TcpStream, mut server: quinn::ServerConfig) -> Result<quinn::Endpoint> {
    let _ = socket2::SockRef::from(&tcp).set_recv_buffer_size(4 * 1024 * 1024);
    let sock = TunnelSocket::new(tcp)?;
    server.transport_config(tunnel_transport());
    Ok(quinn::Endpoint::new_with_abstract_socket(
        tunnel_endpoint_config(),
        Some(server),
        sock,
        super::runtime(),
    )?)
}
