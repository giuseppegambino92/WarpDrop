//! Long-term Ed25519 device identity and the self-signed X.509 certificate
//! presented in every TLS 1.3 handshake.
//!
//! The certificate is only a container for the public key: peers never check
//! its issuer or validity period. Authentication is "the TLS CertificateVerify
//! was signed by *this* Ed25519 key", and the key is then either pinned (trust
//! ring), authenticated by the PAKE (Wormhole code), or confirmed by a human
//! (SAS emoji).

use anyhow::{anyhow, Context, Result};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::path::Path;

/// PKCS#8 v1 header for an Ed25519 private key (RFC 8410).
const PKCS8_ED25519_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

pub struct Identity {
    seed: [u8; 32],
    public_key: [u8; 32],
    cert_der: CertificateDer<'static>,
}

impl Identity {
    pub fn generate() -> Result<Self> {
        let mut seed = [0u8; 32];
        SystemRandom::new()
            .fill(&mut seed)
            .map_err(|_| anyhow!("system RNG unavailable"))?;
        Self::from_seed(&seed)
    }

    pub fn from_seed(seed: &[u8]) -> Result<Self> {
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| anyhow!("identity seed must be 32 bytes"))?;
        let kp = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|_| anyhow!("invalid Ed25519 seed"))?;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(kp.public_key().as_ref());

        let pkcs8 = pkcs8_from_seed(&seed);
        let key_pair = rcgen::KeyPair::from_pkcs8_der_and_sign_algo(
            &PrivatePkcs8KeyDer::from(pkcs8.clone()),
            &rcgen::PKCS_ED25519,
        )
        .context("loading identity into rcgen")?;
        let mut params = rcgen::CertificateParams::new(vec!["warpdrop.local".to_string()])?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, device_id_from_key(&public_key));
        // Wide validity: identity is the key, not the certificate lifetime.
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2124, 1, 1);
        let cert = params.self_signed(&key_pair).context("self-signing identity certificate")?;

        Ok(Identity {
            seed,
            public_key,
            cert_der: cert.der().clone(),
        })
    }

    /// Loads `identity.key` from `dir`, creating it on first launch.
    pub fn load_or_create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("identity.key");
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.len() == 32 {
                return Self::from_seed(&bytes);
            }
        }
        let id = Self::generate()?;
        write_private(&path, &id.seed)?;
        Ok(id)
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn device_id(&self) -> String {
        device_id_from_key(&self.public_key)
    }

    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }

    pub fn cert_chain(&self) -> Vec<CertificateDer<'static>> {
        vec![self.cert_der.clone()]
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_from_seed(&self.seed)))
    }
}

fn pkcs8_from_seed(seed: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(48);
    v.extend_from_slice(&PKCS8_ED25519_PREFIX);
    v.extend_from_slice(seed);
    v
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, data)?;
    Ok(())
}

/// Device id = first 8 bytes of BLAKE3(public key), hex. Stable, short, and
/// collision-resistant enough for display/indexing (the full key is always
/// what gets pinned).
pub fn device_id_from_key(pk: &[u8; 32]) -> String {
    let h = blake3::hash(pk);
    hex::encode(&h.as_bytes()[..8])
}

pub fn fingerprint(pk: &[u8; 32]) -> String {
    let id = device_id_from_key(pk).to_uppercase();
    id.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_public_key_hex(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim()).context("public key is not hex")?;
    v.as_slice()
        .try_into()
        .map_err(|_| anyhow!("public key must be 32 bytes"))
}

/// Extracts the Ed25519 public key from a DER certificate using a real X.509
/// parser (never a byte search: an attacker could embed someone else's key
/// bytes elsewhere in a certificate).
pub fn ed25519_key_from_cert(der: &[u8]) -> Result<[u8; 32]> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).map_err(|e| anyhow!("bad certificate: {e}"))?;
    let spki = cert.public_key();
    let oid = spki.algorithm.algorithm.to_id_string();
    if oid != "1.3.101.112" {
        return Err(anyhow!("certificate key is not Ed25519 ({oid})"));
    }
    spki.subject_public_key
        .data
        .as_ref()
        .try_into()
        .map_err(|_| anyhow!("malformed Ed25519 public key"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_roundtrip() {
        let a = Identity::generate().unwrap();
        let b = Identity::from_seed(a.seed()).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        let k = ed25519_key_from_cert(a.cert_chain()[0].as_ref()).unwrap();
        assert_eq!(&k, a.public_key());
        assert_eq!(a.device_id().len(), 16);
        assert_eq!(fingerprint(a.public_key()).len(), 19);
    }
}
