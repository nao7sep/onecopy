//! Atomic publication for completed same-volume files.
//!
//! macOS and Windows are OneCopy's shipped platforms. macOS exposes
//! `renamex_np(RENAME_EXCL)`; Windows exposes `MoveFileExW` without
//! `MOVEFILE_REPLACE_EXISTING`. Both move the exact staged file into the final
//! directory entry in one atomic commit, so a crash cannot expose partial bytes
//! and an existing public-destination winner is untouched. Private rebuildable
//! cache entries also use an explicit atomic replacement path.

use std::io;
use std::path::Path;

#[cfg(unix)]
pub fn sync_directory(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
pub fn sync_directory(_path: &Path) -> io::Result<()> {
    // The completed file was flushed before rename. Windows' no-replace move
    // is journaled by the filesystem; Rust has no portable directory handle.
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path contains NUL"))?;
    // SAFETY: both arguments are owned NUL-terminated path buffers for the
    // duration of the call; RENAME_EXCL requests an ordinary same-volume move
    // that fails rather than replacing an occupied target.
    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Atomically publishes a private same-volume staging file over an existing
/// rebuildable cache artifact. Public user destinations never use this.
#[cfg(not(windows))]
pub fn replace_existing(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(source, target)
}

#[cfg(windows)]
pub fn replace_existing(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = crate::winpath::for_fs(source)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = crate::winpath::for_fs(target)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both path buffers are NUL-terminated and remain alive for the
    // call. The flags request a write-through replacement of this private,
    // rebuildable cache entry.
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let source: Vec<u16> = crate::winpath::for_fs(source)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = crate::winpath::for_fs(target)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: both path buffers are NUL-terminated and remain alive for the
    // call. Zero flags deliberately omit MOVEFILE_REPLACE_EXISTING, so an
    // exact-boundary winner is preserved instead of overwritten.
    let succeeded = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    // Non-shipping test/development platforms: hard-link publication has the
    // same atomic no-clobber property. The source remains recovery authority
    // if removing the staging name fails.
    std::fs::hard_link(source, target)?;
    std::fs::remove_file(source)
}

#[cfg(test)]
mod tests {
    // EXCEPTION to tests-folder conventions: this verifies a private platform
    // syscall wrapper and would otherwise require making it broader than needed.
    use super::*;

    #[test]
    fn publication_is_atomic_no_clobber_and_moves_the_exact_file() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("output-random.tmp");
        let target = dir.path().join("output.jpg");
        std::fs::write(&staged, b"ours").unwrap();
        let expected = crate::file_identity::FileIdentity::from_path(&staged).unwrap();

        rename_no_replace(&staged, &target).unwrap();
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"ours");
        assert!(crate::file_identity::path_names(&target, expected));
    }

    #[test]
    fn occupied_target_survives_and_completed_stage_remains_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("output-random.tmp");
        let target = dir.path().join("output.jpg");
        std::fs::write(&staged, b"ours").unwrap();
        std::fs::write(&target, b"winner").unwrap();

        assert!(rename_no_replace(&staged, &target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"winner");
        assert_eq!(std::fs::read(&staged).unwrap(), b"ours");
    }

    #[test]
    fn private_cache_replacement_keeps_one_complete_version() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("transcript-random.tmp");
        let target = dir.path().join("transcript.txt");
        std::fs::write(&staged, b"replacement").unwrap();
        std::fs::write(&target, b"previous").unwrap();

        replace_existing(&staged, &target).unwrap();

        assert!(!staged.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
    }
}
