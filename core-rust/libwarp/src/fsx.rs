//! Small filesystem primitives the receiver needs and `std` doesn't offer:
//! a rename that never replaces an existing name, and free-space queries.
//!
//! Guarantees, and where they stop:
//! * On APFS/HFS+, ext4/btrfs/xfs/f2fs (Android), NTFS and ReFS the
//!   "is the name free?" check and the rename are one atomic system call.
//! * Elsewhere `fallback` is used. Files: a hard link, which cannot replace
//!   anything. Where there are no hard links (FAT, exFAT) and for folders the
//!   name is first reserved atomically (`O_EXCL` create / `mkdir`) and the
//!   item renamed over its own empty placeholder, so nothing anyone else
//!   created is ever replaced. A crash in that instant leaves the empty
//!   placeholder behind (the item stays staged and is saved under the next
//!   free name).
//!
//! CI runs the receiving side on ext4, XFS, Btrfs, FAT32, exFAT, NTFS
//! (ntfs-3g, FUSE), NFS and tmpfs (`scripts/test-filesystems.sh`).

use std::io;
use std::path::{Path, PathBuf};

/// Renames `from` to `to`, failing with `ErrorKind::AlreadyExists` instead of
/// replacing anything already at `to`. The check and the rename are one
/// atomic operation where the OS supports it (macOS/iOS `renamex_np`,
/// Linux/Android `renameat2`, Windows `MoveFileExW`).
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    match sys::rename_noreplace(from, to) {
        Err(e) if sys::unsupported(&e) => fallback(from, to),
        r => r,
    }
}

/// For filesystems without an atomic no-replace rename (FAT, some network
/// and FUSE filesystems).
fn fallback(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_file() {
        match std::fs::hard_link(from, to) {
            Ok(()) => return std::fs::remove_file(from),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {} // no hard links here
        }
    }
    reserve_then_rename(from, to)
}

/// Claims `to` atomically with an empty placeholder (fails if the name is
/// taken), then renames `from` over that placeholder, which is the only thing
/// it can replace.
fn reserve_then_rename(from: &Path, to: &Path) -> io::Result<()> {
    let dir = from.is_dir();
    if dir {
        std::fs::create_dir(to)?;
    } else {
        std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    }
    std::fs::rename(from, to).inspect_err(|_| {
        let _ = if dir { std::fs::remove_dir(to) } else { std::fs::remove_file(to) };
    })
}

/// Moves `from` into `dir` as `name`, or `name (2)`, `name (3)`, … if taken.
/// Returns the final path.
pub fn move_unique(from: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    move_unique_with(from, dir, name, |_| Ok(()), rename_noreplace)
}

/// `move_unique`, calling `before(candidate)` ahead of every rename attempt
/// (a failure there aborts before anything moves) and moving with `rename`.
pub fn move_unique_with(
    from: &Path,
    dir: &Path,
    name: &str,
    mut before: impl FnMut(&Path) -> io::Result<()>,
    rename: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..10_000u32 {
        let to = if n == 1 { dir.join(name) } else { dir.join(format!("{stem} ({n}){ext}")) };
        before(&to)?;
        match rename(from, &to) {
            Ok(()) => return Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!("no free name for {name}")))
}

/// Makes renames into or out of `dir` durable (Unix: fsync of the directory;
/// Windows: renames here are already write-through).
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    sys::sync_dir(dir)
}

/// Pushes a written file's data to the storage device. Cheap per file on
/// Apple platforms (plain `fsync`; the drive cache is flushed once, by the
/// `sync_dir` that precedes the renames); `fdatasync`/`FlushFileBuffers`
/// elsewhere. Filesystems that cannot sync a file are skipped.
pub fn sync_data(f: &std::fs::File) -> io::Result<()> {
    sys::sync_data(f)
}

/// Writes `bytes` to `path` so that after a crash `path` holds either the old
/// or the new content, completely: temporary file, fsync, rename, and fsync of
/// the directory (write-through rename on Windows).
pub fn write_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    sys::replace_durable(&tmp, path)?;
    match path.parent() {
        Some(dir) => sync_dir(dir),
        None => Ok(()),
    }
}

/// Bytes available to this user on the filesystem holding `path` (or its
/// nearest existing ancestor). `None` if it cannot be determined.
pub fn available_space(path: &Path) -> Option<u64> {
    let mut p = path;
    while !p.exists() {
        p = p.parent()?;
    }
    sys::available_space(p)
}

#[cfg(unix)]
mod sys {
    use std::ffi::CString;
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c(p: &Path) -> io::Result<CString> {
        CString::new(p.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput.into())
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        let (f, t) = (c(from)?, c(to)?);
        // SAFETY: both arguments are valid NUL-terminated paths.
        if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        const RENAME_NOREPLACE: libc::c_uint = 1;
        let (f, t) = (c(from)?, c(to)?);
        // SAFETY: raw renameat2 (glibc and bionic wrappers are not always
        // available); both paths are valid NUL-terminated strings.
        let r = unsafe { libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), RENAME_NOREPLACE) };
        if r == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android")))]
    pub fn rename_noreplace(_from: &Path, _to: &Path) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }

    pub fn unsupported(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(code) if code == libc::EINVAL || code == libc::ENOSYS || code == libc::ENOTSUP || code == libc::EOPNOTSUPP)
    }

    pub fn replace_durable(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    pub fn sync_data(f: &std::fs::File) -> io::Result<()> {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        let r = {
            use std::os::fd::AsRawFd;
            // SAFETY: a valid open descriptor.
            if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        };
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        let r = f.sync_data();
        match r {
            Err(e) if unsupported(&e) => Ok(()),
            r => r,
        }
    }

    /// Makes renames into/out of `dir` durable. (`sync_all` is F_FULLFSYNC on
    /// Apple platforms.) Filesystems that cannot sync a directory are skipped.
    pub fn sync_dir(dir: &Path) -> io::Result<()> {
        match std::fs::File::open(dir).and_then(|d| d.sync_all()) {
            Err(e) if unsupported(&e) || e.raw_os_error() == Some(libc::EBADF) => Ok(()),
            r => r,
        }
    }

    pub fn available_space(p: &Path) -> Option<u64> {
        let cp = c(p).ok()?;
        // SAFETY: statvfs fills the zeroed struct; the path is NUL-terminated.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(cp.as_ptr(), &mut st) } != 0 {
            return None;
        }
        #[allow(clippy::unnecessary_cast)]
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(windows)]
mod sys {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};

    fn w(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn move_file(from: &Path, to: &Path, flags: u32) -> io::Result<()> {
        let (f, t) = (w(from), w(to));
        // SAFETY: both arguments are valid NUL-terminated wide strings.
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), flags) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Without MOVEFILE_REPLACE_EXISTING the call fails if `to` exists;
    /// WRITE_THROUGH returns only once the move is on disk.
    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        move_file(from, to, MOVEFILE_WRITE_THROUGH)
    }

    pub fn replace_durable(from: &Path, to: &Path) -> io::Result<()> {
        move_file(from, to, MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)
    }

    /// Renames above are write-through; NTFS has no directory fsync.
    pub fn sync_data(f: &std::fs::File) -> io::Result<()> {
        f.sync_data()
    }

    pub fn sync_dir(_dir: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn unsupported(_e: &io::Error) -> bool {
        false
    }

    pub fn available_space(p: &Path) -> Option<u64> {
        let mut avail: u64 = 0;
        // SAFETY: the path is NUL-terminated; unused out-pointers may be null.
        let ok = unsafe { GetDiskFreeSpaceExW(w(p).as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(avail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_replaces_existing_names() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        let e = rename_noreplace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        assert!(a.exists());

        // Folders, too.
        let (x, y) = (d.path().join("x"), d.path().join("y"));
        std::fs::create_dir(&x).unwrap();
        std::fs::create_dir(&y).unwrap();
        assert_eq!(rename_noreplace(&x, &y).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn fallback_never_replaces_either() {
        let d = tempfile::tempdir().unwrap();
        let p = |n: &str| d.path().join(n);
        std::fs::write(p("f1"), b"1").unwrap();
        std::fs::write(p("f2"), b"2").unwrap();
        assert_eq!(fallback(&p("f1"), &p("f2")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(p("f2")).unwrap(), b"2");
        fallback(&p("f1"), &p("f3")).unwrap();
        assert!(!p("f1").exists() && std::fs::read(p("f3")).unwrap() == b"1");

        std::fs::create_dir(p("d1")).unwrap();
        std::fs::write(p("d1/x"), b"x").unwrap();
        std::fs::create_dir(p("d2")).unwrap();
        assert_eq!(fallback(&p("d1"), &p("d2")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        fallback(&p("d1"), &p("d3")).unwrap();
        assert!(p("d3/x").exists() && !p("d1").exists());

        // Without hard links (FAT, exFAT): reserve, then rename over the placeholder.
        std::fs::write(p("f4"), b"4").unwrap();
        assert_eq!(reserve_then_rename(&p("f4"), &p("f2")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(p("f2")).unwrap(), b"2");
        reserve_then_rename(&p("f4"), &p("f5")).unwrap();
        assert!(!p("f4").exists() && std::fs::read(p("f5")).unwrap() == b"4");
        std::fs::create_dir(p("d4")).unwrap();
        std::fs::write(p("d4/y"), b"y").unwrap();
        assert_eq!(reserve_then_rename(&p("d4"), &p("d2")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(p("d2").read_dir().unwrap().next().is_none(), "the existing empty folder is untouched");
        reserve_then_rename(&p("d4"), &p("d5")).unwrap();
        assert!(p("d5/y").exists() && !p("d4").exists());
    }

    #[test]
    fn durable_writes_replace_whole_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("j");
        write_durable(&p, b"one").unwrap();
        write_durable(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert!(!d.path().join("j.tmp").exists());
        // A failure is reported, and the old content survives.
        std::fs::create_dir(d.path().join("j.tmp")).unwrap();
        assert!(write_durable(&p, b"three").is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
    }

    #[test]
    fn move_unique_picks_the_next_free_name() {
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("r.txt"), b"1").unwrap();
        std::fs::write(out.join("r (2).txt"), b"2").unwrap();
        let src = d.path().join("src");
        std::fs::write(&src, b"3").unwrap();
        let to = move_unique(&src, &out, "r.txt").unwrap();
        assert_eq!(to, out.join("r (3).txt"));
        assert_eq!(std::fs::read(out.join("r.txt")).unwrap(), b"1");
        assert!(available_space(&out.join("missing/child")).unwrap() > 0);
    }
}
