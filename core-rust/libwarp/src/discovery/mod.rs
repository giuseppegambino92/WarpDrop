//! Peer table fed by several discovery sources: the built-in mDNS responder,
//! the UDP broadcast beacon, platform-native discovery (Bonjour/AWDL on Apple,
//! NsdManager on Android) via `inject_peer`, ADB USB probing, and QR tickets.

pub mod beacon;
pub mod mdns;

use crate::types::{DeviceKind, PeerAdvert, ServiceAdvert};
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_warpdrop._udp.local.";
pub const DEFAULT_BEACON_PORT: u16 = 47822;
pub const DEFAULT_TUNNEL_PORT: u16 = 47821;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PeerAddr {
    /// Native QUIC over UDP.
    Udp(SocketAddr),
    /// The same QUIC session carried over a TCP stream (LAN fast path).
    Tcp(SocketAddr),
    /// QUIC-over-TCP tunnel on a loopback port (`adb forward`).
    Tunnel(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Mdns,
    Beacon,
    Native,
    Adb,
    Hint,
}

impl Source {
    fn ttl(&self) -> Option<Duration> {
        match self {
            Source::Beacon => Some(Duration::from_secs(12)),
            Source::Hint => Some(Duration::from_secs(600)),
            Source::Adb => Some(Duration::from_secs(15)),
            Source::Mdns | Source::Native => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub device_id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub public_key: Option<[u8; 32]>,
    pub code_open: bool,
    pub addrs: BTreeMap<PeerAddr, (Source, Instant)>,
    pub last_seen: Instant,
}

/// The fields a device advertises (mDNS TXT / beacon / native).
#[derive(Debug, Clone)]
pub struct Announce {
    pub device_id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub public_key: Option<[u8; 32]>,
    pub code_open: bool,
    /// TCP port of the stream carriage, if the device offers one.
    pub stream_port: Option<u16>,
}

/// Adds a stream-carriage address for every UDP address of a device.
pub fn with_stream_addrs(addrs: Vec<PeerAddr>, stream_port: Option<u16>) -> Vec<PeerAddr> {
    let Some(tp) = stream_port.filter(|p| *p != 0) else { return addrs };
    let mut out = addrs.clone();
    for a in addrs {
        if let PeerAddr::Udp(sa) = a {
            let mut t = sa;
            t.set_port(tp);
            out.push(PeerAddr::Tcp(t));
        }
    }
    out
}

impl Announce {
    pub fn to_txt(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("v".into(), "1".into());
        m.insert("id".into(), self.device_id.clone());
        m.insert("n".into(), self.name.chars().take(60).collect());
        m.insert("k".into(), self.kind.as_tag().into());
        if let Some(pk) = self.public_key {
            m.insert("pk".into(), hex::encode(pk));
        }
        m.insert("c".into(), if self.code_open { "1" } else { "0" }.into());
        if let Some(t) = self.stream_port {
            m.insert("t".into(), t.to_string());
        }
        m
    }

    pub fn from_txt(txt: &HashMap<String, String>) -> Option<Self> {
        let pk = txt.get("pk").and_then(|s| crate::identity::parse_public_key_hex(s).ok());
        let device_id = match (txt.get("id"), pk) {
            (_, Some(pk)) => crate::identity::device_id_from_key(&pk),
            (Some(id), None) => id.clone(),
            _ => return None,
        };
        Some(Announce {
            device_id,
            name: txt.get("n").cloned().unwrap_or_else(|| "WarpDrop".into()),
            kind: DeviceKind::from_tag(txt.get("k").map(String::as_str).unwrap_or("")),
            public_key: pk,
            code_open: txt.get("c").map(|c| c == "1").unwrap_or(false),
            stream_port: txt.get("t").and_then(|t| t.parse().ok()),
        })
    }
}

pub fn service_advert(a: &Announce, port: u16) -> ServiceAdvert {
    ServiceAdvert {
        service_type: SERVICE_TYPE.trim_end_matches(".local.").to_string(),
        instance_name: a.device_id.clone(),
        port,
        txt: a.to_txt(),
    }
}

#[derive(Default)]
pub struct PeerTable {
    peers: RwLock<HashMap<String, Peer>>,
    self_id: RwLock<String>,
}

impl PeerTable {
    pub fn set_self(&self, id: &str) {
        *self.self_id.write() = id.to_string();
    }

    /// Returns true when something user-visible changed.
    pub fn upsert(&self, a: &Announce, addrs: &[PeerAddr], src: Source) -> bool {
        if *self.self_id.read() == a.device_id {
            return false;
        }
        let now = Instant::now();
        let mut peers = self.peers.write();
        let p = peers.entry(a.device_id.clone()).or_insert_with(|| Peer {
            device_id: a.device_id.clone(),
            name: a.name.clone(),
            kind: a.kind,
            public_key: a.public_key,
            code_open: a.code_open,
            addrs: BTreeMap::new(),
            last_seen: now,
        });
        let mut changed = p.name != a.name || p.kind != a.kind || p.code_open != a.code_open;
        // Never let an unauthenticated source *replace* a known key.
        if p.public_key.is_none() && a.public_key.is_some() {
            p.public_key = a.public_key;
            changed = true;
        }
        p.name = a.name.clone();
        p.kind = a.kind;
        p.code_open = a.code_open;
        p.last_seen = now;
        for addr in addrs {
            if p.addrs.insert(*addr, (src, now)).is_none() {
                changed = true;
            }
        }
        changed
    }

    pub fn remove_source(&self, device_id: &str, src: Source) -> bool {
        let mut peers = self.peers.write();
        let Some(p) = peers.get_mut(device_id) else { return false };
        let before = p.addrs.len();
        p.addrs.retain(|_, (s, _)| *s != src);
        let changed = before != p.addrs.len();
        if p.addrs.is_empty() {
            peers.remove(device_id);
            return true;
        }
        changed
    }

    pub fn remove_addr(&self, addr: &PeerAddr) -> bool {
        let mut changed = false;
        let mut peers = self.peers.write();
        for p in peers.values_mut() {
            changed |= p.addrs.remove(addr).is_some();
        }
        peers.retain(|_, p| !p.addrs.is_empty());
        changed
    }

    /// Drops expired addresses; returns true if anything disappeared.
    pub fn sweep(&self) -> bool {
        let now = Instant::now();
        let mut peers = self.peers.write();
        let mut changed = false;
        for p in peers.values_mut() {
            let before = p.addrs.len();
            p.addrs.retain(|_, (s, t)| s.ttl().map(|ttl| now.duration_since(*t) < ttl).unwrap_or(true));
            changed |= before != p.addrs.len();
        }
        let before = peers.len();
        peers.retain(|_, p| !p.addrs.is_empty());
        changed || before != peers.len()
    }

    pub fn get(&self, id: &str) -> Option<Peer> {
        self.peers.read().get(id).cloned()
    }

    pub fn all(&self) -> Vec<Peer> {
        let mut v: Vec<Peer> = self.peers.read().values().cloned().collect();
        v.sort_by_key(|p| p.name.to_lowercase());
        v
    }

    pub fn find_by_key(&self, key: &[u8; 32]) -> Option<Peer> {
        self.peers.read().values().find(|p| p.public_key.as_ref() == Some(key)).cloned()
    }
}

pub fn parse_native(advert: &PeerAdvert) -> Option<(Announce, Vec<PeerAddr>)> {
    let a = Announce::from_txt(&advert.txt)?;
    let addrs = advert
        .addresses
        .iter()
        .filter_map(|s| parse_socket_addr(s))
        .map(PeerAddr::Udp)
        .collect();
    let addrs = with_stream_addrs(addrs, a.stream_port);
    Some((a, addrs))
}

/// Parses `1.2.3.4:5`, `[fe80::1%en0]:5`, `[fe80::1%4]:5`.
pub fn parse_socket_addr(s: &str) -> Option<SocketAddr> {
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Some(a);
    }
    // Named scope (`%en0`) → numeric index.
    let (host, port) = s.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let (ip, scope) = host.split_once('%')?;
    let ip: std::net::Ipv6Addr = ip.parse().ok()?;
    let port: u16 = port.parse().ok()?;
    let scope_id = scope.parse::<u32>().ok().or_else(|| if_index(scope))?;
    Some(SocketAddr::V6(std::net::SocketAddrV6::new(ip, port, 0, scope_id)))
}

#[cfg(unix)]
fn if_index(name: &str) -> Option<u32> {
    let c = std::ffi::CString::new(name).ok()?;
    let i = unsafe { libc::if_nametoindex(c.as_ptr()) };
    (i != 0).then_some(i)
}
#[cfg(not(unix))]
fn if_index(_name: &str) -> Option<u32> {
    None
}
