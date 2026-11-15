//! Resumption tickets: zero-prompt resume of an interrupted transfer, on any
//! path — including across the Internet, where the original Wormhole code has
//! already been consumed.
//!
//! When the receiver accepts an offer it issues a 256-bit random ticket bound
//! to (a) the sender's Ed25519 key *as proven in the TLS handshake* and (b) the
//! manifest fingerprint, valid for [`TICKET_TTL`] (refreshed on each use).
//!
//! * A ticket alone is useless: the reconnecting peer must also hold the
//!   private key it is bound to, and offer exactly the same manifest.
//! * The sender pins the receiver's key when it reconnects.
//! * The receiver also listens on the rendezvous server under
//!   `resume_channel(ticket)` — a keyed BLAKE3 hash of the ticket — so the
//!   sender can find it again over the Internet. The relay learns nothing it
//!   could use: the hash is not invertible and the ticket is never sent to it.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const TICKET_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_TICKETS: usize = 32;

/// Rendezvous channel for a ticket (64 hex chars, like code channels).
pub fn resume_channel(ticket: &[u8; 32]) -> String {
    hex::encode(blake3::keyed_hash(ticket, b"warpdrop resume rendezvous v1").as_bytes())
}

#[derive(Debug, Clone)]
struct Entry {
    peer_key: [u8; 32],
    manifest: [u8; 32],
    expires: Instant,
    /// Listen for this ticket on the rendezvous server.
    via_rendezvous: bool,
}

#[derive(Default)]
pub struct TicketStore {
    entries: Mutex<HashMap<[u8; 32], Entry>>,
}

impl TicketStore {
    /// Issues (or, for an existing ticket of the same binding, refreshes) a ticket.
    pub fn issue(&self, peer_key: [u8; 32], manifest: [u8; 32], via_rendezvous: bool) -> [u8; 32] {
        let mut m = self.entries.lock();
        let now = Instant::now();
        m.retain(|_, e| e.expires > now);
        if let Some((t, e)) = m.iter_mut().find(|(_, e)| e.peer_key == peer_key && e.manifest == manifest) {
            e.expires = now + TICKET_TTL;
            e.via_rendezvous |= via_rendezvous;
            return *t;
        }
        while m.len() >= MAX_TICKETS {
            let oldest = *m.iter().min_by_key(|(_, e)| e.expires).map(|(t, _)| t).expect("non-empty");
            m.remove(&oldest);
        }
        let t: [u8; 32] = rand::random();
        m.insert(t, Entry { peer_key, manifest, expires: now + TICKET_TTL, via_rendezvous });
        t
    }

    /// Ticket presented in the handshake: valid and bound to this TLS key?
    /// Returns the manifest fingerprint the offer must match.
    pub fn check_peer(&self, ticket: &[u8; 32], peer_key: &[u8; 32]) -> Option<[u8; 32]> {
        let m = self.entries.lock();
        m.get(ticket)
            .filter(|e| crate::pake::ct_eq(&e.peer_key, peer_key) && e.expires > Instant::now())
            .map(|e| e.manifest)
    }

    pub fn remove_for(&self, peer_key: &[u8; 32], manifest: &[u8; 32]) {
        self.entries.lock().retain(|_, e| !(e.peer_key == *peer_key && e.manifest == *manifest));
    }

    /// Rendezvous channels that should currently be listened on.
    pub fn channels(&self) -> Vec<String> {
        let now = Instant::now();
        let mut v: Vec<String> = self
            .entries
            .lock()
            .iter()
            .filter(|(_, e)| e.via_rendezvous && e.expires > now)
            .map(|(t, _)| resume_channel(t))
            .collect();
        v.sort();
        v
    }

    pub fn expire(&self) -> bool {
        let mut m = self.entries.lock();
        let before = m.len();
        let now = Instant::now();
        m.retain(|_, e| e.expires > now);
        before != m.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_is_bound_to_key_and_manifest() {
        let s = TicketStore::default();
        let (alice, mallory) = ([1u8; 32], [2u8; 32]);
        let (m1, m2) = ([7u8; 32], [8u8; 32]);
        let t = s.issue(alice, m1, true);
        assert_eq!(s.check_peer(&t, &alice), Some(m1));
        assert_eq!(s.check_peer(&t, &mallory), None, "stolen ticket must not work with another key");
        assert_eq!(s.check_peer(&[0u8; 32], &alice), None);
        // Same binding → same ticket (refresh), different manifest → new ticket.
        assert_eq!(s.issue(alice, m1, false), t);
        assert_ne!(s.issue(alice, m2, false), t);
        assert_eq!(s.channels(), vec![resume_channel(&t)]);
        s.remove_for(&alice, &m1);
        assert_eq!(s.check_peer(&t, &alice), None);
        assert_eq!(resume_channel(&t).len(), 64);
    }

    #[test]
    fn store_is_bounded() {
        let s = TicketStore::default();
        for i in 0..100u8 {
            s.issue([i; 32], [i; 32], false);
        }
        assert!(s.entries.lock().len() <= MAX_TICKETS);
    }
}
