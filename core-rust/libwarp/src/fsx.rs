//! Small filesystem primitives the receiver needs and `std` doesn't offer:
//! a rename that never replaces an existing name, and free-space queries.

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
/// and FUSE filesystems). Files: a hard link fails atomically if `to` exists.
/// Folders: best effort, check then rename.
fn fallback(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_file() {
        match std::fs::hard_link(from, to) {
            Ok(()) => return std::fs::remove_file(from),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {}
        }
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}

/// Moves `from` into `dir` as `name`, or `name (2)`, `name (3)`, … if taken.
/// Returns the final path.
pub fn move_unique(from: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1..10_000u32 {
        let to = if n == 1 { dir.join(name) } else { dir.join(format!("{stem} ({n}){ext}")) };
        match rename_noreplace(from, &to) {
            Ok(()) => return Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!("no free name for {name}")))
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
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, MoveFileExW};

    fn w(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
        let (f, t) = (w(from), w(to));
        // Without MOVEFILE_REPLACE_EXISTING the call fails if `to` exists.
        // SAFETY: both arguments are valid NUL-terminated wide strings.
        if unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
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
