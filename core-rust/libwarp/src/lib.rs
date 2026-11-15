//! # libwarp — the WarpDrop engine
//!
//! * Discovery: mDNS/DNS-SD, UDP broadcast beacon, platform injection
//!   (Bonjour/AWDL, NsdManager), ADB USB probing, QR tickets.
//! * Transport: QUIC (quinn) over UDP, over a UDP relay, or over a TCP tunnel,
//!   with automatic path ranking (cable → LAN → Internet P2P → relay).
//! * Security: TLS 1.3 with Ed25519 identities; pinned keys for the trust
//!   ring; SPAKE2 bound to the TLS exporter for Wormhole codes; SAS emoji.
//! * Transfer: streaming multi-file/folder protocol, adaptive zstd, BLAKE3
//!   tree-verified 4 MiB chunks, parallel streams, pause, resume after drop.

pub mod compress;
pub mod discovery;
pub mod error;
pub mod hashing;
pub mod identity;
pub mod manifest;
pub mod netif;
pub mod node;
pub mod pake;
pub mod protocol;
pub mod receiver;
pub mod resume;
pub mod sender;
pub mod tls;
pub mod transfer;
pub mod transport;
pub mod trust;
pub mod types;

pub use error::{WarpError, WarpResult};
pub use node::{core_version, format_code, normalize_code, qr_matrix, WarpNode};
pub use types::*;

uniffi::setup_scaffolding!("warp");
