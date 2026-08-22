//! Atomic no-clobber publication for a completed same-volume file.
//!
//! macOS and Windows are OneCopy's shipped platforms. macOS exposes
//! `renamex_np(RENAME_EXCL)`; Windows rename is already no-replace. Both move
//! the exact staged file into the final directory entry in one atomic commit,
//! so a crash cannot expose partial bytes and an existing winner is untouched.

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

#[cfg(windows)]
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    // Unlike Unix rename(2), MoveFileEx without REPLACE_EXISTING fails when
    // the destination is occupied. std::fs::rename uses that no-clobber form.
    std::fs::rename(
        crate::winpath::for_fs(source).as_ref(),
        crate::winpath::for_fs(target).as_ref(),
    )
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
}
