//! Atomic publication for completed same-volume files.
//!
//! macOS and Windows are OneCopy's shipped platforms. macOS exposes
//! `renamex_np(RENAME_EXCL)`; Windows exposes `MoveFileExW` without
//! `MOVEFILE_REPLACE_EXISTING`. Both move the exact staged file into the final
//! directory entry in one atomic commit, so a crash cannot expose partial bytes
//! and an existing public-destination winner is untouched. macOS volumes that
//! refuse `RENAME_EXCL` (exFAT) reserve the final name with an exclusive empty
//! placeholder and replace only that placeholder. Private rebuildable
//! cache entries also use an explicit atomic replacement path.
//!
//! Publication and directory flushes on user volumes are bounded calls
//! (`volume_io`); the `*_raw` functions run on its worker.

use std::io;
use std::path::Path;

use crate::volume_io::{self, Op};

/// Flushes a directory's entries to the device, bounded. Windows' no-replace
/// move is journaled by the filesystem and Rust has no portable directory
/// handle, so there it does nothing.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    volume_io::sync_dir(path)
}

/// Moves `source` to `target` in one atomic step that never replaces an
/// existing `target`, as one bounded call. A call given up on has an unknown
/// outcome (`volume_io::outcome_unknown`).
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    #[cfg(all(test, target_os = "macos"))]
    let exclusive_unsupported = seam::EXCLUSIVE_RENAME_UNSUPPORTED.with(std::cell::Cell::get);
    #[cfg(not(all(test, target_os = "macos")))]
    let exclusive_unsupported = false;
    let (from, to) = (source.to_path_buf(), target.to_path_buf());
    volume_io::call(target, Op::Rename, None, move || {
        rename_no_replace_raw(&from, &to, exclusive_unsupported)
    })
}

#[cfg(target_os = "macos")]
fn rename_no_replace_raw(
    source: &Path,
    target: &Path,
    exclusive_unsupported: bool,
) -> io::Result<()> {
    if exclusive_unsupported {
        return publish_without_exclusive_rename_raw(source, target);
    }
    match rename_exclusive(source, target) {
        // exFAT (and other volumes macOS mounts without RENAME_EXCL) refuse
        // the exclusive rename outright rather than failing on an occupied
        // target, so they publish through an exclusive placeholder instead.
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => {
            publish_without_exclusive_rename_raw(source, target)
        }
        result => result,
    }
}

#[cfg(target_os = "macos")]
fn rename_exclusive(source: &Path, target: &Path) -> io::Result<()> {
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

/// No-clobber publication for a volume without an exclusive rename. The final
/// name is reserved by creating it exclusively, so an occupied target answers
/// `AlreadyExists` exactly as the exclusive rename would, and the ordinary
/// rename then replaces only that app-created empty placeholder. The rename
/// itself is still atomic: the name holds either the empty placeholder or the
/// complete file, never partial bytes. A placeholder that is no longer ours at
/// the moment of replacement is left alone.
#[cfg(target_os = "macos")]
fn publish_without_exclusive_rename_raw(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::symlink_metadata(source)?;
    let placeholder = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    if !crate::file_identity::path_names_file(target, &placeholder) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("the reserved name was replaced: {}", target.display()),
        ));
    }
    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) => {
            if crate::file_identity::path_names_file(target, &placeholder) {
                crate::fs_recovery::remove_file(target, "publication placeholder cleanup");
            }
            Err(error)
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod seam {
    thread_local! {
        /// Makes publications this thread starts behave as on a volume that
        /// refuses `RENAME_EXCL` (exFAT).
        pub static EXCLUSIVE_RENAME_UNSUPPORTED: std::cell::Cell<bool> =
            const { std::cell::Cell::new(false) };
    }

    pub fn without_exclusive_rename<T>(run: impl FnOnce() -> T) -> T {
        EXCLUSIVE_RENAME_UNSUPPORTED.with(|flag| flag.set(true));
        let result = run();
        EXCLUSIVE_RENAME_UNSUPPORTED.with(|flag| flag.set(false));
        result
    }
}

#[cfg(all(test, target_os = "macos"))]
// EXCEPTION to tests-folder conventions: the seam that stands in for a volume
// without RENAME_EXCL is private and must not widen the shipped API.
#[path = "../tests/unit/fs_publish.rs"]
mod unsupported_exclusive_rename_tests;

/// Atomically publishes a private same-volume staging file over an existing
/// rebuildable cache artifact. Public user destinations never use this.
#[cfg(not(windows))]
pub fn replace_existing(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(source, target) // data root
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
fn rename_no_replace_raw(source: &Path, target: &Path, _exclusive_unsupported: bool) -> io::Result<()> {
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
fn rename_no_replace_raw(source: &Path, target: &Path, _exclusive_unsupported: bool) -> io::Result<()> {
    // Non-shipping test/development platforms: hard-link publication has the
    // same atomic no-clobber property. The source remains recovery authority
    // if removing the staging name fails.
    std::fs::hard_link(source, target)?;
    std::fs::remove_file(source)
}
