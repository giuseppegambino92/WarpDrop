//! Describes what is being sent: a flat list of files and directories with
//! relative `/`-separated paths. Folders are walked lazily into the manifest
//! and streamed chunk by chunk — nothing is ever archived to disk.

use crate::hashing::chunk_count;
use crate::types::SendItem;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
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

pub fn validate(m: &Manifest) -> Result<()> {
    if m.entries.is_empty() {
        bail!("empty manifest");
    }
    let tmp = Path::new("/");
    let mut seen = HashSet::new();
    for e in &m.entries {
        safe_join(tmp, &e.path)?;
        if !seen.insert(e.path.as_str()) {
            bail!("duplicate path in manifest: {}", e.path);
        }
    }
    Ok(())
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

    #[test]
    fn unique_top_level_names() {
        let mut t = HashSet::new();
        assert_eq!(unique_name("a.txt", &mut t), "a.txt");
        assert_eq!(unique_name("a.txt", &mut t), "a (2).txt");
        assert_eq!(unique_name("a.txt", &mut t), "a (3).txt");
    }
}
