//! Wire protocol.
//!
//! Every connection opens one bidirectional *control* stream carrying
//! length-prefixed postcard messages. File data travels on N unidirectional
//! *data* streams, each a sequence of self-describing chunk frames, so chunks
//! can arrive in any order over any stream (and, after a reconnect, over any
//! path).

use crate::manifest::Manifest;
use crate::types::{ClipboardContent, DeviceKind};
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Bump on every wire-format change. v2: resumption tickets (`Auth::Resume`,
/// `Accept.ticket`).
pub const PROTOCOL_VERSION: u16 = 2;
const MAX_MSG: usize = 64 * 1024 * 1024;

/// QUIC application close codes.
pub mod close {
    pub const DONE: u32 = 0;
    pub const CANCELLED: u32 = 1;
    pub const REJECTED: u32 = 2;
    pub const PROTOCOL: u32 = 3;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub kind: DeviceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    /// Both keys are in each other's trust ring (mTLS with pinned keys).
    Trusted,
    /// SPAKE2 over the receiver's Wormhole code follows.
    Code,
    /// Session token issued by the receiver after an earlier successful auth.
    Token([u8; 32]),
    /// Unknown nearby device; the receiver's user is asked (with SAS emoji).
    Ask,
    /// Resumption ticket for one specific transfer (see `resume.rs`).
    /// Variants are encoded by position: only ever append, so `Hello` stays
    /// decodable across versions and the version check can reject cleanly.
    Resume([u8; 32]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Purpose {
    Transfer,
    Clipboard,
    /// Join each other's Personal Trust Ring.
    Link,
    /// Exchange trust-ring membership (trusted peers only).
    RingSync,
    /// Learn the peer's identity (used for ADB tunnels).
    Identify,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingEntry {
    pub public_key: [u8; 32],
    pub name: String,
    pub kind: DeviceKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaveChunk {
    pub entry: u32,
    pub chunk: u32,
    pub hash: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Msg {
    Hello { version: u16, device: DeviceInfo, auth: Auth, purpose: Purpose },
    Pake(Vec<u8>),
    Confirm([u8; 32]),
    Welcome { device: DeviceInfo, trusted: bool, resume_token: [u8; 32] },
    Reject { reason: String },

    Offer { manifest: Manifest },
    /// `ticket` lets the sender resume this transfer without a new code.
    Accept { have: Vec<HaveChunk>, ticket: [u8; 32] },
    /// All chunks were written; per-entry BLAKE3 roots (files only, zero for dirs).
    Done { roots: Vec<[u8; 32]> },
    Result { ok: bool, error: Option<String> },

    Clipboard(ClipboardContent),
    Ring(Vec<RingEntry>),
    Ack,
}

pub async fn write_msg<W: AsyncWrite + Unpin>(w: &mut W, m: &Msg) -> Result<()> {
    let bytes = postcard::to_stdvec(m)?;
    w.write_all(&(bytes.len() as u32).to_be_bytes()).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<Msg> {
    read_as(r).await
}

async fn read_as<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.context("control stream closed")?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_MSG {
        bail!("control message too large ({len} bytes)");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(postcard::from_bytes(&buf)?)
}

/// Chunk frame header on data streams (big endian):
/// `entry u32 | chunk u32 | raw_len u32 | enc u8 | payload_len u32 | hash [32]`
pub const FRAME_HEADER_LEN: usize = 4 + 4 + 4 + 1 + 4 + 32;

#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub entry: u32,
    pub chunk: u32,
    pub raw_len: u32,
    pub enc: u8,
    pub payload_len: u32,
    pub hash: [u8; 32],
}

impl FrameHeader {
    pub fn encode(&self) -> [u8; FRAME_HEADER_LEN] {
        let mut b = [0u8; FRAME_HEADER_LEN];
        b[0..4].copy_from_slice(&self.entry.to_be_bytes());
        b[4..8].copy_from_slice(&self.chunk.to_be_bytes());
        b[8..12].copy_from_slice(&self.raw_len.to_be_bytes());
        b[12] = self.enc;
        b[13..17].copy_from_slice(&self.payload_len.to_be_bytes());
        b[17..49].copy_from_slice(&self.hash);
        b
    }
    pub fn decode(b: &[u8; FRAME_HEADER_LEN]) -> Self {
        let u = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&b[17..49]);
        FrameHeader { entry: u(0), chunk: u(4), raw_len: u(8), enc: b[12], payload_len: u(13), hash }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Hello` must stay decodable by every version so mismatches are
    /// rejected with `protocol_version` instead of a decode error.
    #[test]
    fn hello_layout_is_stable() {
        let h = Msg::Hello {
            version: 1,
            device: DeviceInfo { name: "x".into(), kind: DeviceKind::Mac },
            auth: Auth::Ask,
            purpose: Purpose::Transfer,
        };
        // Msg::Hello = variant 0; version 1; name "x"; kind Mac = 0; Ask = 3; Transfer = 0.
        assert_eq!(postcard::to_stdvec(&h).unwrap(), vec![0, 1, 1, b'x', 0, 3, 0]);
    }

    #[tokio::test]
    async fn msg_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_msg(&mut a, &Msg::Pake(vec![1, 2, 3])).await.unwrap();
        match read_msg(&mut b).await.unwrap() {
            Msg::Pake(v) => assert_eq!(v, vec![1, 2, 3]),
            m => panic!("unexpected {m:?}"),
        }
        let h = FrameHeader { entry: 3, chunk: 9, raw_len: 77, enc: 1, payload_len: 50, hash: [5; 32] };
        let d = FrameHeader::decode(&h.encode());
        assert_eq!((d.entry, d.chunk, d.raw_len, d.enc, d.payload_len, d.hash), (3, 9, 77, 1, 50, [5; 32]));
    }
}
