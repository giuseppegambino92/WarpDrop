//! Adaptive per-chunk compression: zstd level 1 for compressible data, raw
//! pass-through for already-compressed media and archives.
//!
//! Decision order: extension deny-list → magic-byte sniff of the first chunk →
//! empirical ratio (if zstd saves < 5% on a chunk, the rest of the file is
//! sent raw).

use std::path::Path;

pub const ENC_RAW: u8 = 0;
pub const ENC_ZSTD: u8 = 1;
const ZSTD_LEVEL: i32 = 1;
const MIN_COMPRESS_LEN: usize = 512;

const INCOMPRESSIBLE_EXT: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "hevc", "mp3", "m4a", "aac", "ogg", "opus", "flac", "jpg",
    "jpeg", "heic", "heif", "avif", "png", "gif", "webp", "jxl", "zip", "gz", "tgz", "bz2", "xz", "zst", "7z",
    "rar", "dmg", "ipa", "apk", "aab", "xapk", "jar", "pkg", "msi", "cab", "docx", "xlsx", "pptx", "pages",
    "numbers", "key", "epub", "woff", "woff2", "br", "lz4", "lzma", "sit", "iso",
];

pub fn ext_incompressible(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INCOMPRESSIBLE_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Magic numbers of formats that are already entropy coded.
pub fn magic_incompressible(head: &[u8]) -> bool {
    const SIGS: &[&[u8]] = &[
        b"\xFF\xD8\xFF",          // JPEG
        b"\x89PNG\r\n\x1a\n",     // PNG
        b"GIF8",                  // GIF
        b"PK\x03\x04",            // ZIP / JAR / APK / IPA / OOXML
        b"\x1f\x8b",              // gzip
        b"\x28\xb5\x2f\xfd",      // zstd
        b"\xfd7zXZ\x00",          // xz
        b"7z\xbc\xaf\x27\x1c",    // 7z
        b"Rar!\x1a\x07",          // RAR
        b"BZh",                   // bzip2
        b"\x1a\x45\xdf\xa3",      // Matroska / WebM
        b"ID3",                   // MP3
        b"fLaC",                  // FLAC
        b"OggS",                  // Ogg
        b"\x04\x22\x4d\x18",      // LZ4
        b"wOFF",                  // WOFF
        b"wOF2",                  // WOFF2
    ];
    if SIGS.iter().any(|s| head.starts_with(s)) {
        return true;
    }
    // ISO-BMFF (mp4/mov/heic/avif/3gp): "....ftyp"
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return true;
    }
    // RIFF....WEBP
    if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        return true;
    }
    false
}

pub struct Compressor {
    inner: zstd::bulk::Compressor<'static>,
}

impl Compressor {
    pub fn new() -> Self {
        Compressor {
            inner: zstd::bulk::Compressor::new(ZSTD_LEVEL).expect("zstd compressor"),
        }
    }

    /// Returns `(encoding, payload)`. `None` payload means "send the input as is".
    pub fn compress(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < MIN_COMPRESS_LEN {
            return None;
        }
        let out = self.inner.compress(data).ok()?;
        // Require ≥ 5% savings to be worth the receiver's CPU.
        if out.len() * 100 <= data.len() * 95 {
            Some(out)
        } else {
            None
        }
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

pub fn decompress(payload: &[u8], raw_len: usize) -> std::io::Result<Vec<u8>> {
    let out = zstd::bulk::decompress(payload, raw_len)?;
    if out.len() != raw_len {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "decompressed length mismatch"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_media() {
        assert!(ext_incompressible(Path::new("a/IMG_0001.HEIC")));
        assert!(ext_incompressible(Path::new("movie.mp4")));
        assert!(!ext_incompressible(Path::new("main.rs")));
        assert!(magic_incompressible(b"\xFF\xD8\xFF\xE0rest"));
        assert!(magic_incompressible(b"\x00\x00\x00\x18ftypheic...."));
        assert!(!magic_incompressible(b"fn main() {}"));
    }

    #[test]
    fn compresses_text_but_not_noise() {
        let mut c = Compressor::new();
        let text = "hello warpdrop ".repeat(1000);
        let z = c.compress(text.as_bytes()).expect("text compresses");
        assert_eq!(decompress(&z, text.len()).unwrap(), text.as_bytes());
        let noise: Vec<u8> = (0..100_000).map(|_| rand::random::<u8>()).collect();
        assert!(c.compress(&noise).is_none());
    }
}
