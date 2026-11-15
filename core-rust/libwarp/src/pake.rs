//! Wormhole codes, the PAKE that authenticates them, and SAS emoji.
//!
//! ## Protocol
//! The 6-digit code is fed to SPAKE2 (Ed25519 group). SPAKE2 is the
//! *symmetric/balanced* member of the SPAKE family; SPAKE2+ is the augmented
//! variant whose only extra property — a server storing a verifier instead of
//! the password — does not apply to a short-lived code both screens know.
//!
//! The SPAKE2 identity string contains the TLS exporter of the QUIC session,
//! so the derived key is bound to *this* TLS 1.3 transcript (and therefore to
//! both certificates). A MITM terminating TLS twice sees two different
//! exporters, the keys diverge, and explicit key confirmation fails: an active
//! attacker gets exactly one online guess per attempt (1 in 10⁶), and the
//! receiver rotates its code after three failures.

use anyhow::{anyhow, Result};
use rand::Rng;
use spake2::{Ed25519Group, Identity as SpakeIdentity, Password, Spake2};

pub const CODE_LEN: usize = 6;

pub fn generate_code() -> String {
    let n: u32 = rand::rngs::OsRng.gen_range(0..1_000_000);
    format!("{n:06}")
}

/// Accepts `482910`, `482-910`, `482 910`.
pub fn normalize_code(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(|c| !matches!(c, '-' | ' ' | '\u{00a0}')).collect();
    if digits.len() == CODE_LEN && digits.chars().all(|c| c.is_ascii_digit()) {
        Some(digits)
    } else {
        None
    }
}

pub fn display_code(code: &str) -> String {
    if code.len() == CODE_LEN {
        format!("{}-{}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

pub struct PakeState {
    inner: Spake2<Ed25519Group>,
    binding: [u8; 32],
}

/// Starts the exchange; returns the state and the message to send.
pub fn start(code: &str, binding: &[u8; 32]) -> (PakeState, Vec<u8>) {
    let mut pw = b"warpdrop-code-v1:".to_vec();
    pw.extend_from_slice(code.as_bytes());
    let mut ident = b"warpdrop/1 tls-exporter:".to_vec();
    ident.extend_from_slice(binding);
    let (inner, msg) = Spake2::<Ed25519Group>::start_symmetric(&Password::new(&pw), &SpakeIdentity::new(&ident));
    (PakeState { inner, binding: *binding }, msg)
}

pub struct PakeKeys {
    key: [u8; 32],
}

impl PakeState {
    pub fn finish(self, peer_msg: &[u8]) -> Result<PakeKeys> {
        let raw = self.inner.finish(peer_msg).map_err(|e| anyhow!("PAKE failed: {e:?}"))?;
        let mut h = blake3::Hasher::new_derive_key("warpdrop 2025 pake session key");
        h.update(&raw);
        h.update(&self.binding);
        Ok(PakeKeys { key: *h.finalize().as_bytes() })
    }
}

impl PakeKeys {
    fn tag(&self, label: &str) -> [u8; 32] {
        *blake3::keyed_hash(&self.key, label.as_bytes()).as_bytes()
    }
    pub fn client_confirm(&self) -> [u8; 32] {
        self.tag("confirm:client")
    }
    pub fn server_confirm(&self) -> [u8; 32] {
        self.tag("confirm:server")
    }
    pub fn sas(&self) -> Vec<String> {
        sas_emoji(&self.tag("sas"))
    }
}

/// Constant-time equality for tags and keys.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// 64 visually distinct, easily named emoji → 6 bits each, 24 bits for four.
const EMOJI: [&str; 64] = [
    "🐶", "🐱", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🐦", "🦆",
    "🦉", "🐴", "🦄", "🐝", "🦋", "🐌", "🐞", "🐢", "🐍", "🐙", "🦀", "🐠", "🐬", "🐳", "🦈", "🐘",
    "🌵", "🌲", "🌻", "🍄", "🌙", "⭐", "🔥", "🌈", "❄️", "☂️", "🍎", "🍋", "🍌", "🍇", "🍓", "🍒",
    "🥑", "🌽", "🍕", "🍩", "🎂", "☕", "⚽", "🏀", "🎸", "🎲", "🚀", "🚲", "⛵", "🔑", "💡", "🎁",
];

pub fn sas_emoji(bytes: &[u8]) -> Vec<String> {
    let v = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
    (0..4).map(|i| EMOJI[((v >> (18 - 6 * i)) & 0x3f) as usize].to_string()).collect()
}

/// SAS derived only from the TLS session (no PAKE): used for first contact
/// with an unknown nearby device, where the humans compare four emoji.
pub fn sas_from_binding(binding: &[u8; 32]) -> Vec<String> {
    sas_emoji(blake3::keyed_hash(binding, b"warpdrop sas").as_bytes())
}

/// Rendezvous channel id for a code. Memory-hard so that the relay operator
/// cannot trivially invert observed channel ids into codes within a code's
/// lifetime. (Even with the code, a *passive* relay learns nothing: SPAKE2 is
/// secure against eavesdroppers that know the password, and all payload is
/// end-to-end TLS 1.3. An *active* relay is limited to one online guess.)
pub fn rendezvous_channel(code: &str) -> Result<String> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(8 * 1024, 2, 1, Some(32)).map_err(|e| anyhow!("{e}"))?;
    let a = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    a.hash_password_into(code.as_bytes(), b"warpdrop-rendezvous-v1", &mut out)
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(hex::encode(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_formats() {
        let c = generate_code();
        assert_eq!(c.len(), 6);
        assert_eq!(normalize_code("482-910").as_deref(), Some("482910"));
        assert_eq!(normalize_code("482 910").as_deref(), Some("482910"));
        assert_eq!(normalize_code("48291"), None);
        assert_eq!(normalize_code("48291a"), None);
        assert_eq!(display_code("482910"), "482-910");
    }

    #[test]
    fn pake_agrees_with_same_code_and_binding() {
        let b = [7u8; 32];
        let (a, ma) = start("123456", &b);
        let (c, mc) = start("123456", &b);
        let ka = a.finish(&mc).unwrap();
        let kc = c.finish(&ma).unwrap();
        assert_eq!(ka.client_confirm(), kc.client_confirm());
        assert_eq!(ka.sas(), kc.sas());
    }

    #[test]
    fn pake_diverges_on_wrong_code_or_mitm() {
        let b = [7u8; 32];
        let (a, ma) = start("123456", &b);
        let (c, mc) = start("123457", &b);
        assert_ne!(a.finish(&mc).unwrap().client_confirm(), c.finish(&ma).unwrap().client_confirm());

        // Same code, different TLS sessions (a MITM terminating TLS).
        let (a, ma) = start("123456", &[1u8; 32]);
        let (c, mc) = start("123456", &[2u8; 32]);
        assert_ne!(a.finish(&mc).unwrap().client_confirm(), c.finish(&ma).unwrap().client_confirm());
    }

    #[test]
    fn rendezvous_channel_is_deterministic() {
        assert_eq!(rendezvous_channel("000001").unwrap(), rendezvous_channel("000001").unwrap());
        assert_ne!(rendezvous_channel("000001").unwrap(), rendezvous_channel("000002").unwrap());
    }
}
