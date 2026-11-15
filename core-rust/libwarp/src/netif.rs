//! Local interface scanner and link classifier.
//!
//! Maps every local interface to a [`PathKind`] so the connector can rank a
//! peer's addresses (cable → LAN/Wi-Fi → Internet) and the UI can show the
//! "⚡ Direct Cable" badge. Classification combines:
//! * well-known point-to-point subnets: iPhone USB tethering `172.20.10.0/28`,
//!   Android USB tethering `192.168.42.0/24`, IPv4 link-local `169.254/16`;
//! * interface names (`bridge0` Thunderbolt, `awdl0`/`llw0` AWDL, `usb0`,
//!   `rndis0`, `enx…`);
//! * the kernel driver on Linux (`rndis_host`, `cdc_ncm`, `cdc_ether`, `ipheth`);
//! * the hardware-port table from `networksetup` on macOS ("iPhone USB",
//!   "Thunderbolt Bridge", "Wi-Fi", "USB 10/100/1000 LAN"…).

use crate::types::{NetInterface, PathKind};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct LocalIf {
    pub name: String,
    pub index: Option<u32>,
    pub ip: IpAddr,
    pub prefix: u8,
    pub kind: PathKind,
}

fn v4_in(ip: Ipv4Addr, net: [u8; 4], prefix: u8) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    (u32::from(ip) & mask) == (u32::from(Ipv4Addr::from(net)) & mask)
}

#[cfg(target_os = "macos")]
fn mac_hardware_ports() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(out) = std::process::Command::new("/usr/sbin/networksetup").arg("-listallhardwareports").output() else {
        return map;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut port = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("Hardware Port: ") {
            port = Some(p.trim().to_string());
        } else if let Some(d) = line.strip_prefix("Device: ") {
            if let Some(p) = port.take() {
                map.insert(d.trim().to_string(), p);
            }
        }
    }
    map
}

#[cfg(not(target_os = "macos"))]
fn mac_hardware_ports() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(target_os = "linux")]
fn linux_driver(name: &str) -> Option<String> {
    std::fs::read_link(format!("/sys/class/net/{name}/device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

#[cfg(target_os = "linux")]
fn linux_is_wireless(name: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{name}/wireless")).exists()
}

pub fn classify(name: &str, ip: IpAddr, hw_ports: &HashMap<String, String>) -> PathKind {
    let lname = name.to_ascii_lowercase();
    if ip.is_loopback() || lname.starts_with("lo") && !lname.starts_with("low") {
        return PathKind::Loopback;
    }
    if lname.starts_with("awdl") || lname.starts_with("llw") || lname.starts_with("p2p") {
        return PathKind::WifiDirect;
    }
    if let Some(port) = hw_ports.get(name) {
        let p = port.to_ascii_lowercase();
        if p.contains("thunderbolt bridge") {
            return PathKind::ThunderboltBridge;
        }
        if p.contains("iphone") || p.contains("ipad") || p.contains("rndis") || p.contains("android") || p.contains("ncm") {
            return PathKind::UsbCable;
        }
        if p.contains("wi-fi") || p.contains("airport") {
            return PathKind::Wifi;
        }
        if p.contains("ethernet") || p.contains("lan") || p.contains("thunderbolt") {
            // A USB/Thunderbolt Ethernet adapter on a link-local subnet is a
            // direct cable between two machines.
            if let IpAddr::V4(v4) = ip {
                if v4.is_link_local() {
                    return PathKind::UsbCable;
                }
            }
            return PathKind::Ethernet;
        }
    }
    if lname.starts_with("bridge") && !lname.starts_with("bridge10") {
        return PathKind::ThunderboltBridge;
    }
    if let IpAddr::V4(v4) = ip {
        if v4_in(v4, [172, 20, 10, 0], 28) || v4_in(v4, [192, 168, 42, 0], 24) || v4.is_link_local() {
            return PathKind::UsbCable;
        }
    }
    if lname.starts_with("usb") || lname.starts_with("rndis") || lname.starts_with("ncm") || lname.starts_with("enx") {
        return PathKind::UsbCable;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(d) = linux_driver(name) {
            if ["rndis_host", "cdc_ncm", "cdc_ether", "ipheth", "cdc_eem", "r8152"].iter().any(|x| d.contains(x)) {
                return PathKind::UsbCable;
            }
        }
        if linux_is_wireless(name) {
            return PathKind::Wifi;
        }
    }
    if lname.starts_with("wl") || lname.starts_with("wifi") || lname.starts_with("wi-fi") {
        return PathKind::Wifi;
    }
    if lname.starts_with("eth") || lname.starts_with("eno") || lname.starts_with("enp") || lname.starts_with("ethernet") {
        return PathKind::Ethernet;
    }
    if cfg!(any(target_os = "ios", target_os = "android")) && lname == "en0" || lname.starts_with("wlan") {
        return PathKind::Wifi;
    }
    PathKind::Lan
}

pub fn scan() -> Vec<LocalIf> {
    let ports = mac_hardware_ports();
    let Ok(ifs) = if_addrs::get_if_addrs() else { return vec![] };
    ifs.into_iter()
        .map(|i| {
            let (ip, prefix) = match &i.addr {
                if_addrs::IfAddr::V4(a) => (IpAddr::V4(a.ip), u32::from(a.netmask).count_ones() as u8),
                if_addrs::IfAddr::V6(a) => (IpAddr::V6(a.ip), u128::from(a.netmask).count_ones() as u8),
            };
            LocalIf { kind: classify(&i.name, ip, &ports), name: i.name, index: i.index, ip, prefix }
        })
        .collect()
}

/// Cached view of the local interfaces (rescanned at most every 3 s).
pub struct InterfaceTable {
    cache: Mutex<(Option<Instant>, Vec<LocalIf>)>,
}

impl Default for InterfaceTable {
    fn default() -> Self {
        InterfaceTable { cache: Mutex::new((None, vec![])) }
    }
}

impl InterfaceTable {
    pub fn get(&self) -> Vec<LocalIf> {
        let mut c = self.cache.lock();
        if c.0.map(|t| t.elapsed() > Duration::from_secs(3)).unwrap_or(true) {
            c.1 = scan();
            c.0 = Some(Instant::now());
        }
        c.1.clone()
    }

    /// Which kind of link would carry traffic to `remote`.
    pub fn path_to(&self, remote: &SocketAddr) -> PathKind {
        let ifs = self.get();
        let ip = match remote.ip() {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
            v4 => v4,
        };
        if ip.is_loopback() || ifs.iter().any(|i| i.ip == ip) {
            return PathKind::Loopback;
        }
        match ip {
            IpAddr::V4(v4) => {
                let best = ifs
                    .iter()
                    .filter(|i| match i.ip {
                        IpAddr::V4(l) => i.prefix > 0 && v4_in(v4, l.octets(), i.prefix),
                        _ => false,
                    })
                    .max_by_key(|i| i.prefix);
                match best {
                    Some(i) => i.kind,
                    None if is_private_v4(v4) => PathKind::Lan,
                    None => PathKind::InternetP2p,
                }
            }
            IpAddr::V6(v6) => {
                let link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
                if link_local {
                    if let SocketAddr::V6(s6) = remote {
                        if let Some(i) = ifs.iter().find(|i| i.index == Some(s6.scope_id()) && s6.scope_id() != 0) {
                            return i.kind;
                        }
                    }
                    return PathKind::Lan;
                }
                let best = ifs
                    .iter()
                    .filter(|i| match i.ip {
                        IpAddr::V6(l) => {
                            let p = i.prefix.min(128) as u32;
                            let mask = if p == 0 { 0 } else { u128::MAX << (128 - p) };
                            p > 0 && (u128::from(l) & mask) == (u128::from(v6) & mask)
                        }
                        _ => false,
                    })
                    .max_by_key(|i| i.prefix);
                best.map(|i| i.kind).unwrap_or(PathKind::InternetP2p)
            }
        }
    }

    /// Addresses worth advertising (no loopback unless nothing else exists).
    pub fn advertised_ips(&self) -> Vec<IpAddr> {
        let ifs = self.get();
        let mut v: Vec<&LocalIf> = ifs
            .iter()
            .filter(|i| i.kind != PathKind::Loopback && !is_virtual_tunnel(&i.name))
            .collect();
        v.sort_by_key(|i| i.kind.rank());
        let mut ips: Vec<IpAddr> = v.into_iter().map(|i| i.ip).collect();
        ips.dedup();
        if ips.is_empty() {
            ips.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
        }
        ips
    }

    pub fn public_list(&self) -> Vec<NetInterface> {
        self.get()
            .into_iter()
            .map(|i| NetInterface { name: i.name, address: i.ip.to_string(), kind: i.kind })
            .collect()
    }
}

/// VPN / tunnel interfaces whose addresses are pointless to advertise.
fn is_virtual_tunnel(name: &str) -> bool {
    ["utun", "ipsec", "gif", "stf", "tun", "tap", "wg", "nan", "docker", "veth", "vmnet", "vboxnet"]
        .iter()
        .any(|p| name.starts_with(p))
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local() || v4_in(ip, [100, 64, 0, 0], 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_tethering_and_bridges() {
        let none = HashMap::new();
        assert_eq!(classify("en7", "172.20.10.2".parse().unwrap(), &none), PathKind::UsbCable);
        assert_eq!(classify("usb0", "10.1.1.2".parse().unwrap(), &none), PathKind::UsbCable);
        assert_eq!(classify("rndis0", "192.168.42.129".parse().unwrap(), &none), PathKind::UsbCable);
        assert_eq!(classify("bridge0", "169.254.3.3".parse().unwrap(), &none), PathKind::ThunderboltBridge);
        assert_eq!(classify("awdl0", "fe80::1".parse().unwrap(), &none), PathKind::WifiDirect);
        assert_eq!(classify("wlan0", "192.168.1.5".parse().unwrap(), &none), PathKind::Wifi);
        let mut ports = HashMap::new();
        ports.insert("en0".to_string(), "Wi-Fi".to_string());
        ports.insert("en8".to_string(), "iPhone USB".to_string());
        assert_eq!(classify("en0", "192.168.1.5".parse().unwrap(), &ports), PathKind::Wifi);
        assert_eq!(classify("en8", "fe80::2".parse().unwrap(), &ports), PathKind::UsbCable);
    }
}
