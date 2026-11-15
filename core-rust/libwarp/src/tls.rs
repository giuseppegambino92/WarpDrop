//! rustls / QUIC configuration.
//!
//! * TLS 1.3 only, Ed25519 certificates only.
//! * Cipher preference: AES-256-GCM (hardware AES on ARMv8 / AES-NI), then
//!   ChaCha20-Poly1305. AES-128-GCM stays in the list solely because RFC 9001
//!   mandates it for QUIC *Initial* packet protection.
//! * Both sides always present certificates (mutual TLS). The client pins the
//!   server key when it knows it (trust ring / QR ticket / mDNS TXT); the
//!   server authorises the client's key at the application layer, because a
//!   first contact over a Wormhole code is authenticated by the PAKE that runs
//!   *inside* the TLS channel and is bound to its exporter secret.

use crate::identity::{ed25519_key_from_cert, Identity};
use anyhow::{anyhow, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls13_signature, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use std::sync::Arc;
use std::time::Duration;

pub const ALPN: &[u8] = b"warpdrop/1";
pub const SERVER_NAME: &str = "warpdrop.local";
pub const EXPORTER_LABEL: &[u8] = b"EXPORTER-warpdrop-channel-binding";

pub fn provider() -> Arc<CryptoProvider> {
    use rustls::crypto::ring::{cipher_suite, default_provider};
    let mut p = default_provider();
    p.cipher_suites = vec![
        cipher_suite::TLS13_AES_256_GCM_SHA384,
        cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
        cipher_suite::TLS13_AES_128_GCM_SHA256,
    ];
    Arc::new(p)
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    provider().signature_verification_algorithms
}

#[derive(Debug)]
struct Ed25519ServerVerifier {
    pinned: Option<[u8; 32]>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for Ed25519ServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let key = ed25519_key_from_cert(end_entity.as_ref())
            .map_err(|e| rustls::Error::General(e.to_string()))?;
        if let Some(pin) = self.pinned {
            if !crate::pake::ct_eq(&pin, &key) {
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ));
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

#[derive(Debug)]
struct Ed25519ClientVerifier {
    algs: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for Ed25519ClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        ed25519_key_from_cert(end_entity.as_ref()).map_err(|e| rustls::Error::General(e.to_string()))?;
        Ok(ClientCertVerified::assertion())
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

pub fn transport_config() -> quinn::TransportConfig {
    use quinn::VarInt;
    let mut t = quinn::TransportConfig::default();
    // Generous windows so a single connection can fill multi-gigabit links
    // with a few milliseconds of RTT (BDP of 10 Gbit/s × 5 ms ≈ 6 MB).
    t.stream_receive_window(VarInt::from_u32(24 * 1024 * 1024));
    t.receive_window(VarInt::from_u32(96 * 1024 * 1024));
    t.send_window(96 * 1024 * 1024);
    t.max_concurrent_uni_streams(VarInt::from_u32(64));
    t.max_concurrent_bidi_streams(VarInt::from_u32(16));
    t.keep_alive_interval(Some(Duration::from_secs(4)));
    t.max_idle_timeout(Some(Duration::from_secs(20).try_into().expect("valid idle timeout")));
    t
}

pub fn server_config(id: &Identity) -> Result<quinn::ServerConfig> {
    let verifier = Arc::new(Ed25519ClientVerifier { algs: algorithms() });
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(id.cert_chain(), id.private_key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.max_early_data_size = 0;
    let quic = QuicServerConfig::try_from(tls).map_err(|e| anyhow!("QUIC server config: {e}"))?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(quic));
    cfg.transport_config(Arc::new(transport_config()));
    Ok(cfg)
}

/// `pinned = Some(key)` refuses any server that does not hold that key.
pub fn client_config(id: &Identity, pinned: Option<[u8; 32]>) -> Result<quinn::ClientConfig> {
    let verifier = Arc::new(Ed25519ServerVerifier { pinned, algs: algorithms() });
    let mut tls = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(id.cert_chain(), id.private_key())?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let quic = QuicClientConfig::try_from(tls).map_err(|e| anyhow!("QUIC client config: {e}"))?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(quic));
    cfg.transport_config(Arc::new(transport_config()));
    Ok(cfg)
}

/// The peer's Ed25519 key, as proven by its TLS CertificateVerify signature.
pub fn peer_key(conn: &quinn::Connection) -> Result<[u8; 32]> {
    let any = conn.peer_identity().ok_or_else(|| anyhow!("peer presented no certificate"))?;
    let certs = any
        .downcast::<Vec<CertificateDer<'static>>>()
        .map_err(|_| anyhow!("unexpected peer identity type"))?;
    let first = certs.first().ok_or_else(|| anyhow!("empty certificate chain"))?;
    ed25519_key_from_cert(first.as_ref())
}

/// 32 bytes unique to this TLS session (RFC 5705 / RFC 8446 §7.5). A MITM
/// that terminates TLS on both sides ends up with two different values.
pub fn channel_binding(conn: &quinn::Connection) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    conn.export_keying_material(&mut out, EXPORTER_LABEL, b"")
        .map_err(|_| anyhow!("TLS exporter unavailable"))?;
    Ok(out)
}
