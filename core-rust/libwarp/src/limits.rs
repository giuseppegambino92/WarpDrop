//! Per-transfer memory budget on the receiving side, sized to the device.
//!
//! An incoming transfer costs memory per chunk (its verified hash) and per
//! entry (manifest entry, local path, lookup tables). The hard caps in
//! `manifest` bound a single transfer on any device; this budget scales the
//! limits down on phones and small machines: by default 1/32 of physical
//! RAM, between 32 MiB and 1 GiB, or exactly `NodeConfig::transfer_memory_mb`
//! when set. A quarter goes to per-chunk state, the rest to entries.
//!
//! Free disk space is checked separately and is usually the tighter limit
//! (a 256 GB phone cannot accept more than ~60 000 chunks anyway).

use crate::hashing::CHUNK_SIZE;
use crate::manifest::{Manifest, MAX_ENTRIES, MAX_TOTAL_CHUNKS};

const MIB: u64 = 1024 * 1024;
/// Peak per chunk while a transfer is set up: the verified-hash table
/// (`Option<[u8; 32]>`, 33 B), the already-held list sent back to the sender
/// (40 B) and the log-dedup bitmap (1 B), rounded up.
const BYTES_PER_CHUNK: u64 = 80;
/// Conservative estimate of the peak per entry while a transfer is set up
/// (decoded manifest, its copy for staging, local path, collision tables).
const BYTES_PER_ENTRY: u64 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    pub max_chunks: u64,
}

impl Limits {
    pub fn for_budget(bytes: u64) -> Limits {
        Limits {
            max_chunks: (bytes / 4 / BYTES_PER_CHUNK).min(MAX_TOTAL_CHUNKS),
            max_entries: ((bytes - bytes / 4) / BYTES_PER_ENTRY).min(MAX_ENTRIES as u64) as usize,
        }
    }

    /// `configured_mb = 0`: automatic, from physical RAM.
    pub fn for_device(configured_mb: u32) -> Limits {
        let budget = if configured_mb > 0 {
            configured_mb as u64 * MIB
        } else {
            physical_memory().map(|ram| ram / 32).unwrap_or(128 * MIB).clamp(32 * MIB, 1024 * MIB)
        };
        Limits::for_budget(budget)
    }

    /// Largest transfer (bytes) these limits accept.
    pub fn max_bytes(&self) -> u64 {
        self.max_chunks * CHUNK_SIZE
    }

    pub fn allows(&self, m: &Manifest) -> bool {
        m.entries.len() <= self.max_entries && m.entries.iter().map(|e| e.chunks()).sum::<u64>() <= self.max_chunks
    }
}

/// Installed RAM in bytes.
pub fn physical_memory() -> Option<u64> {
    sys::physical_memory().filter(|b| *b > 0)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod sys {
    pub fn physical_memory() -> Option<u64> {
        let mut v: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        // SAFETY: "hw.memsize" is a u64; `v`/`len` describe a valid buffer.
        let r = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut v as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
        (r == 0).then_some(v)
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod sys {
    pub fn physical_memory() -> Option<u64> {
        // SAFETY: plain sysconf queries.
        let (pages, size) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
        (pages > 0 && size > 0).then(|| pages as u64 * size as u64)
    }
}

#[cfg(windows)]
mod sys {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub fn physical_memory() -> Option<u64> {
        // SAFETY: MEMORYSTATUSEX is plain data; dwLength must be set first.
        let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (unsafe { GlobalMemoryStatusEx(&mut st) } != 0).then_some(st.ullTotalPhys)
    }
}

#[cfg(not(any(unix, windows)))]
mod sys {
    pub fn physical_memory() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Entry, EntryKind};

    #[test]
    fn budget_scales_with_the_device() {
        // A 3 GB phone: 96 MiB → ~315 k chunks (≈ 1.2 TiB) and 73 k entries.
        let phone = Limits::for_budget(3 * 1024 * MIB / 32);
        assert_eq!(phone.max_entries, 73_728);
        assert_eq!(phone.max_chunks, 314_572);
        // Big machines hit the hard caps.
        let big = Limits::for_budget(1024 * MIB);
        assert_eq!((big.max_entries, big.max_chunks), (786_432, MAX_TOTAL_CHUNKS));
        // Explicit setting wins.
        assert_eq!(Limits::for_device(64), Limits::for_budget(64 * MIB));
        // Automatic stays within 32 MiB..1 GiB.
        let (auto, lo, hi) = (Limits::for_device(0), Limits::for_budget(32 * MIB), Limits::for_budget(1024 * MIB));
        assert!((lo.max_entries..=hi.max_entries).contains(&auto.max_entries), "{auto:?}");
        assert!((lo.max_chunks..=hi.max_chunks).contains(&auto.max_chunks), "{auto:?}");
        assert!(physical_memory().unwrap() >= 1024 * MIB);
    }

    #[test]
    fn allows_checks_entries_and_chunks() {
        let l = Limits { max_entries: 2, max_chunks: 3 };
        let f = |p: &str, size| Entry { path: p.into(), kind: EntryKind::File { size, mtime_ms: 0, executable: false } };
        assert!(l.allows(&Manifest { entries: vec![f("a", 3 * CHUNK_SIZE)] }));
        assert!(!l.allows(&Manifest { entries: vec![f("a", 3 * CHUNK_SIZE + 1)] }), "4 chunks");
        assert!(!l.allows(&Manifest { entries: vec![f("a", 1), f("b", 1), f("c", 1)] }), "3 entries");
    }
}
