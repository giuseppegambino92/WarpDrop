//! Describes what is being sent: a flat list of files and directories with
//! relative `/`-separated paths. Folders are walked lazily into the manifest
//! and streamed chunk by chunk — nothing is ever archived to disk.

use crate::hashing::chunk_count;
use crate::types::SendItem;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    Dir,
    File { size: u64, mtime_ms: u64, executable: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
}

impl Entry {
    pub fn size(&self) -> u64 {
        match self.kind {
            EntryKind::File { size, .. } => size,
            EntryKind::Dir => 0,
        }
    }
    pub fn chunks(&self) -> u64 {
        chunk_count(self.size())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size()).sum()
    }
    pub fn file_count(&self) -> u64 {
        self.entries.iter().filter(|e| matches!(e.kind, EntryKind::File { .. })).count() as u64
    }
    pub fn top_level(&self) -> Vec<&str> {
        let mut seen = HashSet::new();
        let mut out = vec![];
        for e in &self.entries {
            let top = e.path.split('/').next().unwrap_or("");
            if seen.insert(top) {
                out.push(top);
            }
        }
        out
    }
    pub fn title(&self) -> String {
        let tops = self.top_level();
        match tops.len() {
            1 => tops[0].to_string(),
            n => format!("{n} items"),
        }
    }
    /// Stable identity of this transfer's content, used to find resume state.
    pub fn fingerprint(&self) -> [u8; 32] {
        let bytes = postcard::to_stdvec(self).unwrap_or_default();
        *blake3::hash(&bytes).as_bytes()
    }
}

/// Manifest plus the local source path of each entry (sender side only).
pub struct SourceSet {
    pub manifest: Manifest,
    pub sources: Vec<Option<PathBuf>>,
}

fn mtime_ms(md: &std::fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(unix)]
fn is_exec(md: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    md.permissions().mode() & 0o111 != 0
}
#[cfg(not(unix))]
fn is_exec(_md: &std::fs::Metadata) -> bool {
    false
}

fn unique_name(name: &str, taken: &mut HashSet<String>) -> String {
    if taken.insert(name.to_string()) {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 2.. {
        let cand = format!("{stem} ({n}){ext}");
        if taken.insert(cand.clone()) {
            return cand;
        }
    }
    unreachable!()
}

pub fn build(items: &[SendItem]) -> Result<SourceSet> {
    let mut entries = vec![];
    let mut sources = vec![];
    let mut taken = HashSet::new();
    // Top-level folders created implicitly by `name = "Folder/sub/file"` items,
    // mapped to their (possibly de-duplicated) final name, plus emitted dirs.
    let mut virtual_tops: std::collections::HashMap<String, String> = Default::default();
    let mut emitted_dirs: HashSet<String> = HashSet::new();
    for item in items {
        let path = PathBuf::from(&item.path);
        let md = std::fs::metadata(&path).with_context(|| format!("cannot read {}", item.path))?;
        if let Some(rel) = item.name.as_deref().filter(|n| n.contains('/')) {
            // A file placed at a relative path (lets platforms stream a picked
            // folder file-by-file, e.g. Android content URIs, without copying).
            if md.is_dir() {
                bail!("relative names are only supported for files: {rel}");
            }
            let parts: Vec<String> = rel.split('/').filter(|p| !p.is_empty()).map(sanitize_component).collect();
            if parts.len() < 2 {
                bail!("invalid relative name: {rel}");
            }
            let top = virtual_tops
                .entry(parts[0].clone())
                .or_insert_with(|| unique_name(&parts[0], &mut taken))
                .clone();
            let mut dir = top;
            if emitted_dirs.insert(dir.clone()) {
                entries.push(Entry { path: dir.clone(), kind: EntryKind::Dir });
                sources.push(None);
            }
            for p in &parts[1..parts.len() - 1] {
                dir = format!("{dir}/{p}");
                if emitted_dirs.insert(dir.clone()) {
                    entries.push(Entry { path: dir.clone(), kind: EntryKind::Dir });
                    sources.push(None);
                }
            }
            let file_path = format!("{dir}/{}", parts[parts.len() - 1]);
            if !emitted_dirs.insert(file_path.clone()) {
                bail!("duplicate path: {rel}");
            }
            entries.push(Entry {
                path: file_path,
                kind: EntryKind::File { size: md.len(), mtime_ms: mtime_ms(&md), executable: is_exec(&md) },
            });
            sources.push(Some(path));
            continue;
        }
        let base = item
            .name
            .clone()
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .ok_or_else(|| anyhow!("cannot determine a name for {}", item.path))?;
        let top = unique_name(&sanitize_component(&base), &mut taken);
        if md.is_dir() {
            entries.push(Entry { path: top.clone(), kind: EntryKind::Dir });
            sources.push(None);
            let mut walk: Vec<_> = walkdir::WalkDir::new(&path)
                .min_depth(1)
                .follow_links(false)
                .sort_by_file_name()
                .into_iter()
                .collect::<std::result::Result<_, _>>()
                .with_context(|| format!("walking {}", item.path))?;
            walk.retain(|e| !e.file_type().is_symlink());
            for e in walk {
                let rel = e.path().strip_prefix(&path)?;
                let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                let rel_path = format!("{top}/{}", rel.join("/"));
                let md = e.metadata()?;
                if md.is_dir() {
                    entries.push(Entry { path: rel_path, kind: EntryKind::Dir });
                    sources.push(None);
                } else if md.is_file() {
                    entries.push(Entry {
                        path: rel_path,
                        kind: EntryKind::File { size: md.len(), mtime_ms: mtime_ms(&md), executable: is_exec(&md) },
                    });
                    sources.push(Some(e.path().to_path_buf()));
                }
            }
        } else {
            entries.push(Entry {
                path: top,
                kind: EntryKind::File { size: md.len(), mtime_ms: mtime_ms(&md), executable: is_exec(&md) },
            });
            sources.push(Some(path));
        }
    }
    if entries.is_empty() {
        bail!("nothing to send");
    }
    Ok(SourceSet { manifest: Manifest { entries }, sources })
}

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
    "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes a single path component safe on every platform we write to.
pub fn sanitize_component(c: &str) -> String {
    let mut s: String = c
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let upper = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&upper.as_str()) {
        s.insert(0, '_');
    }
    if s.is_empty() || s == "." || s == ".." {
        s = "_".into();
    }
    if s.len() > 255 {
        let mut cut = 255;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// Validates an untrusted manifest path and maps it under `root`. Rejects
/// traversal outright rather than "fixing" it.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') || rel.contains('\0') {
        bail!("unsafe path in manifest: {rel:?}");
    }
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() > 128 {
        bail!("path too deep: {rel:?}");
    }
    let mut out = root.to_path_buf();
    for p in parts {
        if p.is_empty() || p == "." || p == ".." {
            bail!("unsafe path in manifest: {rel:?}");
        }
        out.push(sanitize_component(p));
    }
    Ok(out)
}

/// Upper bounds for an incoming manifest, checked before anything is allocated
/// per entry or per chunk. Together with the 64 MiB cap on control messages
/// they are the receiver's memory budget for one transfer:
/// * per-chunk state is 33 bytes (`Option<[u8; 32]>`), so `MAX_TOTAL_CHUNKS`
///   keeps it under 70 MiB;
/// * per-entry state (manifest, local path, lookup tables) stays in the low
///   hundreds of MiB even for `MAX_ENTRIES` entries with long names.
///
/// The receiver also refuses, before accepting, anything that does not fit in
/// its free disk space.
pub const MAX_ENTRIES: usize = 1_000_000;
const MAX_PATH_BYTES: usize = 4096;
/// 2²¹ chunks of 4 MiB = 8 TiB per transfer.
pub const MAX_TOTAL_CHUNKS: u64 = 1 << 21;
pub const MAX_TOTAL_BYTES: u64 = MAX_TOTAL_CHUNKS * crate::hashing::CHUNK_SIZE;

pub fn validate(m: &Manifest) -> Result<()> {
    if m.entries.is_empty() {
        bail!("empty manifest");
    }
    if m.entries.len() > MAX_ENTRIES {
        bail!("too many entries in manifest ({})", m.entries.len());
    }
    let (mut bytes, mut chunks): (u64, u64) = (0, 0);
    for e in &m.entries {
        if e.path.len() > MAX_PATH_BYTES {
            bail!("path too long in manifest");
        }
        bytes = bytes.checked_add(e.size()).filter(|b| *b <= MAX_TOTAL_BYTES).ok_or_else(|| anyhow!("transfer too large"))?;
        chunks = chunks.checked_add(e.chunks()).filter(|c| *c <= MAX_TOTAL_CHUNKS).ok_or_else(|| anyhow!("transfer too large"))?;
    }
    local_paths(m).map(|_| ())
}

/// Comparison key for a sanitized path: Unicode-normalized and case-folded,
/// because macOS (APFS) and Windows (NTFS) treat `é`/`é` and `A`/`a` as the
/// same name.
fn fold(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

/// `name` with a ` (n)` suffix before the extension, kept within 255 bytes.
fn numbered(name: &str, n: u32) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let suffix = format!(" ({n}){ext}");
    let mut cut = stem.len().min(255usize.saturating_sub(suffix.len()));
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{suffix}", &stem[..cut])
}

/// Maps every manifest entry to the relative path it is written to on this
/// device. Components are sanitized; names that become identical after
/// sanitizing, Unicode normalization or case folding get a ` (2)` suffix
/// instead of silently landing on the same file. A renamed folder carries its
/// children with it. Rejects traversal, duplicates and entries whose parent
/// folder is missing or is a file.
pub fn local_paths(m: &Manifest) -> Result<Vec<PathBuf>> {
    let root = Path::new("");
    // Original path → (mapped relative path, folded key, is_dir).
    let mut mapped: HashMap<&str, (String, String, bool)> = HashMap::with_capacity(m.entries.len());
    let mut taken: HashSet<String> = HashSet::with_capacity(m.entries.len());
    let mut out = Vec::with_capacity(m.entries.len());
    for e in &m.entries {
        safe_join(root, &e.path)?;
        if mapped.contains_key(e.path.as_str()) {
            bail!("duplicate path in manifest: {}", e.path);
        }
        let (parent, name) = match e.path.rsplit_once('/') {
            Some((parent, name)) => {
                let Some((dir, key, is_dir)) = mapped.get(parent) else {
                    bail!("missing parent folder in manifest: {}", e.path);
                };
                if !is_dir {
                    bail!("parent is a file in manifest: {}", e.path);
                }
                (Some((dir.clone(), key.clone())), name)
            }
            None => (None, e.path.as_str()),
        };
        let clean = sanitize_component(name);
        let join = |n: &str| match &parent {
            Some((dir, key)) => (format!("{dir}/{n}"), format!("{key}/{}", fold(n))),
            None => (n.to_string(), fold(n)),
        };
        let (mut path, mut key) = join(&clean);
        let mut n = 2;
        while !taken.insert(key.clone()) {
            (path, key) = join(&numbered(&clean, n));
            n += 1;
        }
        out.push(PathBuf::from(&path));
        mapped.insert(e.path.as_str(), (path, key, matches!(e.kind, EntryKind::Dir)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal() {
        let r = Path::new("/dl");
        assert!(safe_join(r, "../etc/passwd").is_err());
        assert!(safe_join(r, "a/../../b").is_err());
        assert!(safe_join(r, "/abs").is_err());
        assert!(safe_join(r, "a\\b").is_err());
        assert!(safe_join(r, "a//b").is_err());
        assert_eq!(safe_join(r, "a/b c/d.txt").unwrap(), Path::new("/dl/a/b c/d.txt"));
        assert_eq!(sanitize_component("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_component("what?.txt"), "what_.txt");
    }

    #[test]
    fn relative_names_build_a_folder() {
        let d = std::env::temp_dir().join(format!("warp-rel-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("a"), d.join("b"));
        std::fs::write(&a, b"1").unwrap();
        std::fs::write(&b, b"22").unwrap();
        let items = vec![
            SendItem { path: a.to_string_lossy().into(), name: Some("Photos/2025/a.jpg".into()) },
            SendItem { path: b.to_string_lossy().into(), name: Some("Photos/b.jpg".into()) },
        ];
        let m = build(&items).unwrap().manifest;
        let paths: Vec<&str> = m.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["Photos", "Photos/2025", "Photos/2025/a.jpg", "Photos/b.jpg"]);
        assert_eq!(m.title(), "Photos");
        validate(&m).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    fn file(path: &str) -> Entry {
        Entry { path: path.into(), kind: EntryKind::File { size: 1, mtime_ms: 0, executable: false } }
    }
    fn dir(path: &str) -> Entry {
        Entry { path: path.into(), kind: EntryKind::Dir }
    }
    fn mapped(entries: Vec<Entry>) -> Result<Vec<String>> {
        let m = Manifest { entries };
        validate(&m)?;
        Ok(local_paths(&m)?.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect())
    }

    #[test]
    fn names_that_sanitize_alike_do_not_collide() {
        let p = mapped(vec![file("report:2026.txt"), file("report?2026.txt"), file("report_2026.txt")]).unwrap();
        assert_eq!(p, ["report_2026.txt", "report_2026 (2).txt", "report_2026 (3).txt"]);
    }

    #[test]
    fn case_and_unicode_variants_do_not_collide() {
        let nfc = "caf\u{e9}.txt";
        let nfd = "cafe\u{301}.txt";
        let p = mapped(vec![file("Readme.md"), file("README.md"), file(nfc), file(nfd)]).unwrap();
        assert_eq!(p[0], "Readme.md");
        assert_eq!(p[1], "README (2).md");
        assert_eq!(p[2], nfc);
        assert_eq!(p[3], "cafe\u{301} (2).txt");
    }

    #[test]
    fn renamed_folder_carries_its_children() {
        let p = mapped(vec![dir("Photos"), file("Photos/a.jpg"), dir("photos"), file("photos/a.jpg")]).unwrap();
        assert_eq!(p, ["Photos", "Photos/a.jpg", "photos (2)", "photos (2)/a.jpg"]);
    }

    #[test]
    fn rejects_bad_structure() {
        assert!(mapped(vec![file("a"), file("a")]).is_err(), "exact duplicate");
        assert!(mapped(vec![file("a/b")]).is_err(), "missing parent");
        assert!(mapped(vec![file("a"), file("a/b")]).is_err(), "parent is a file");
        assert!(mapped(vec![dir("x"), file("x/../y")]).is_err(), "traversal");
    }

    #[test]
    fn rejects_absurd_sizes() {
        let huge = |size| Entry { path: "big".into(), kind: EntryKind::File { size, mtime_ms: 0, executable: false } };
        assert!(validate(&Manifest { entries: vec![huge(u64::MAX)] }).is_err());
        assert!(validate(&Manifest { entries: vec![huge(MAX_TOTAL_BYTES + 1)] }).is_err());
        let two = vec![huge(MAX_TOTAL_BYTES), Entry { path: "b".into(), kind: EntryKind::File { size: 1, mtime_ms: 0, executable: false } }];
        assert!(validate(&Manifest { entries: two }).is_err(), "sum overflow");
        assert!(validate(&Manifest { entries: vec![huge(10 << 30)] }).is_ok());
        assert!(validate(&Manifest { entries: vec![huge(MAX_TOTAL_BYTES)] }).is_ok(), "exactly at the budget");
    }

    #[test]
    fn numbered_names_stay_within_255_bytes() {
        let long = format!("{}.txt", "x".repeat(251));
        let n = numbered(&long, 12);
        assert!(n.len() <= 255 && n.ends_with(" (12).txt"), "{n}");
    }

    #[test]
    fn unique_top_level_names() {
        let mut t = HashSet::new();
        assert_eq!(unique_name("a.txt", &mut t), "a.txt");
        assert_eq!(unique_name("a.txt", &mut t), "a (2).txt");
        assert_eq!(unique_name("a.txt", &mut t), "a (3).txt");
    }
}
