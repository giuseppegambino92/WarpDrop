//! Content-addressed chunk hashing that composes into the *standard* BLAKE3
//! hash of the whole file.
//!
//! Files are split into 4 MiB chunks. 4 MiB is a power-of-two multiple of the
//! BLAKE3 1 KiB chunk size, so each of our chunks is a complete BLAKE3 subtree
//! and its chaining value (CV) can be computed independently — in parallel, on
//! either side, in any order — and later merged into the root. The root is
//! bit-for-bit what `b3sum` prints for the file.
//!
//! A file with a single chunk has no subtree merge: its "chunk hash" is simply
//! the root hash.

use blake3::hazmat::{merge_subtrees_non_root, merge_subtrees_root, ChainingValue, HasherExt, Mode};

pub const CHUNK_SIZE: u64 = 4 * 1024 * 1024;

pub fn chunk_count(size: u64) -> u64 {
    size.div_ceil(CHUNK_SIZE)
}

pub fn chunk_len(size: u64, idx: u64) -> u64 {
    let start = idx * CHUNK_SIZE;
    (size - start).min(CHUNK_SIZE)
}

/// Hash for chunk `idx` of a file with `n_chunks` chunks.
pub fn chunk_hash(data: &[u8], idx: u64, n_chunks: u64) -> [u8; 32] {
    if n_chunks <= 1 {
        return *blake3::hash(data).as_bytes();
    }
    let mut h = blake3::Hasher::new();
    h.set_input_offset(idx * CHUNK_SIZE);
    h.update(data);
    h.finalize_non_root()
}

/// Root hash from the ordered chunk hashes produced by [`chunk_hash`].
pub fn file_root(chunks: &[[u8; 32]]) -> [u8; 32] {
    match chunks.len() {
        0 => *blake3::hash(b"").as_bytes(),
        1 => chunks[0],
        _ => *merge_root(chunks).as_bytes(),
    }
}

/// BLAKE3's tree is left-complete: the left subtree holds the largest power of
/// two number of chunks that is strictly less than the total.
fn split(n: usize) -> usize {
    debug_assert!(n > 1);
    1usize << (usize::BITS - 1 - (n - 1).leading_zeros())
}

fn merge_non_root(cvs: &[ChainingValue]) -> ChainingValue {
    if cvs.len() == 1 {
        return cvs[0];
    }
    let l = split(cvs.len());
    merge_subtrees_non_root(&merge_non_root(&cvs[..l]), &merge_non_root(&cvs[l..]), Mode::Hash)
}

fn merge_root(cvs: &[ChainingValue]) -> blake3::Hash {
    let l = split(cvs.len());
    merge_subtrees_root(&merge_non_root(&cvs[..l]), &merge_non_root(&cvs[l..]), Mode::Hash)
}

/// Streams a whole file and returns its BLAKE3 root (used by tests and the CLI).
pub fn hash_file(path: &std::path::Path) -> std::io::Result<[u8; 32]> {
    let mut h = blake3::Hasher::new();
    h.update_reader(std::fs::File::open(path)?)?;
    Ok(*h.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_hash(data: &[u8]) -> [u8; 32] {
        let n = chunk_count(data.len() as u64);
        let hashes: Vec<[u8; 32]> = (0..n)
            .map(|i| {
                let s = (i * CHUNK_SIZE) as usize;
                let e = (s + CHUNK_SIZE as usize).min(data.len());
                chunk_hash(&data[s..e], i, n)
            })
            .collect();
        file_root(&hashes)
    }

    #[test]
    fn matches_reference_blake3_for_many_sizes() {
        let c = CHUNK_SIZE as usize;
        let big: Vec<u8> = (0..(c * 7 + 12345)).map(|i| (i * 31 % 251) as u8).collect();
        for size in [0, 1, 1023, 1024, c - 1, c, c + 1, 2 * c, 2 * c + 7, 3 * c, 4 * c + 1, 5 * c, 7 * c + 12345] {
            let d = &big[..size];
            assert_eq!(tree_hash(d), *blake3::hash(d).as_bytes(), "size {size}");
        }
    }
}
