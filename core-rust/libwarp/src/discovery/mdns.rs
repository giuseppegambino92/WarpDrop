//! Built-in mDNS / DNS-SD (`_warpdrop._udp.local.`) for desktop platforms.

use super::{Announce, PeerAddr, PeerTable, Source, SERVICE_TYPE};
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

pub struct Mdns {
    daemon: ServiceDaemon,
    fullname: parking_lot::Mutex<Option<String>>,
}

impl Mdns {
    pub fn start(table: Arc<PeerTable>, on_change: Arc<dyn Fn() + Send + Sync>) -> Result<Arc<Self>> {
        let daemon = ServiceDaemon::new()?;
        let rx = daemon.browse(SERVICE_TYPE)?;
        let me = Arc::new(Mdns { daemon, fullname: parking_lot::Mutex::new(None) });
        std::thread::Builder::new().name("warp-mdns".into()).spawn(move || {
            // fullname → device id, to handle removals.
            let mut names: HashMap<String, String> = HashMap::new();
            while let Ok(ev) = rx.recv() {
                match ev {
                    ServiceEvent::ServiceResolved(info) => {
                        let txt: HashMap<String, String> = info
                            .get_properties()
                            .iter()
                            .map(|p| (p.key().to_string(), p.val_str().to_string()))
                            .collect();
                        let Some(a) = Announce::from_txt(&txt) else { continue };
                        let port = info.get_port();
                        let addrs: Vec<PeerAddr> = info
                            .get_addresses()
                            .iter()
                            .filter(|ip| match ip {
                                // Link-local v6 is useless without a scope id.
                                IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
                                IpAddr::V4(_) => true,
                            })
                            .map(|ip| PeerAddr::Udp(SocketAddr::new(*ip, port)))
                            .collect();
                        let addrs = super::with_stream_addrs(addrs, a.stream_port);
                        names.insert(info.get_fullname().to_string(), a.device_id.clone());
                        if table.upsert(&a, &addrs, Source::Mdns) {
                            on_change();
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        if let Some(id) = names.remove(&fullname) {
                            if table.remove_source(&id, Source::Mdns) {
                                on_change();
                            }
                        }
                    }
                    _ => {}
                }
            }
        })?;
        Ok(me)
    }

    pub fn advertise(&self, a: &Announce, port: u16) -> Result<()> {
        let host = format!("warp-{}.local.", a.device_id);
        let info = ServiceInfo::new(SERVICE_TYPE, &a.device_id, &host, "", port, a.to_txt())?.enable_addr_auto();
        *self.fullname.lock() = Some(info.get_fullname().to_string());
        self.daemon.register(info)?;
        Ok(())
    }

    pub fn shutdown(&self) {
        if let Some(n) = self.fullname.lock().take() {
            let _ = self.daemon.unregister(&n);
        }
        let _ = self.daemon.shutdown();
    }
}
