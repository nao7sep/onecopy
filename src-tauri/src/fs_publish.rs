//! Atomic publication for completed same-volume files.
//!
//! macOS and Windows are OneCopy's shipped platforms. macOS exposes
//! `renamex_np(RENAME_EXCL)`; Windows exposes `MoveFileExW` without
//! `MOVEFILE_REPLACE_EXISTING`. Both move the exact staged file into the final
//! directory entry in one atomic commit, so a crash cannot expose partial bytes
//! and an existing public-destination winner is untouched. macOS volumes that
//! refuse `RENAME_EXCL` reject publication without changing either path.
//! Private rebuildable cache entries also use an explicit atomic replacement
//! path.
//!
//! Publication and directory flushes on user volumes are bounded calls
//! (`volume_io`); the `*_raw` functions run on its worker.

use std::io;
use std::path::Path;

use crate::volume_io::{self, Op};

/// Flushes a directory's entries on Unix, bounded. Windows has no directory
/// barrier here: its publication requests write-through and file callers flush
/// their actual descriptors. A successful Windows no-op is not independent
/// evidence of rename durability on a particular filesystem.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    volume_io::sync_dir(path)
}

/// Moves `source` to `target` in one atomic step that never replaces an
/// existing `target`, as one bounded call. A call given up on has an unknown
/// outcome (`volume_io::outcome_unknown`). Volumes without an atomic
/// no-replace rename return `Unsupported` without publishing the file.
pub fn rename_no_replace(source: &Path, target: &Path) -> io::Result<()> {
    rename_no_replace_checked(source, target, None)
}

fn rename_no_replace_checked(
    source: &Path,
    target: &Path,
    source_version: Option<crate::file_identity::FileVersion>,
) -> io::Result<()> {
    #[cfg(all(test, target_os = "macos"))]
    let exclusive_unsupported = seam::EXCLUSIVE_RENAME_UNSUPPORTED.with(std::cell::Cell::get);
    #[cfg(not(all(test, target_os = "macos")))]
    let exclusive_unsupported = false;
    let (from, to) = (source.to_path_buf(), target.to_path_buf());
    volume_io::call(target, Op::Rename, None, move || {
        rename_no_replace_raw(&from, &to, exclusive_unsupported, source_version)
    })
}

/// A private directory is held by descriptor so replacement of its public
/// pathname cannot redirect newly deleted bytes into a shared directory.
#[cfg(target_os = "macos")]
pub(crate) fn rename_into_directory(
    source: &Path,
    target: &Path,
    directory: std::sync::Arc<std::fs::File>,
    mut snapshot: crate::file_identity::SourceSnapshot,
    destination: Option<crate::file_identity::VerifiedDestination>,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    #[cfg(test)]
    let unsupported = seam::EXCLUSIVE_RENAME_UNSUPPORTED.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let unsupported = false;
    let source = source.to_path_buf();
    let parent = target.parent().ok_or_else(|| io::Error::other("missing stored parent"))?.to_path_buf();
    let name = target.file_name().ok_or_else(|| io::Error::other("missing stored name"))?.to_os_string();
    volume_io::call(target, Op::Rename, None, move || {
        if unsupported { return Err(exclusive_rename_unsupported()); }
        if !crate::file_identity::path_names_file(&parent, &directory) {
            return Err(io::Error::other("private deleted-file directory was replaced"));
        }
        snapshot.require_current(&source)?;
        if let Some(destination) = &destination { destination.require_current()?; }
        rename_exclusive_at(&source, Path::new(&name), directory.as_raw_fd())
    })
}

#[cfg(target_os = "macos")]
fn rename_no_replace_raw(
    source: &Path,
    target: &Path,
    exclusive_unsupported: bool,
    source_version: Option<crate::file_identity::FileVersion>,
) -> io::Result<()> {
    if exclusive_unsupported {
        return Err(exclusive_rename_unsupported());
    }
    require_source_version(source, source_version)?;
    match rename_exclusive(source, target) {
        // A replacing rename cannot safely emulate this operation: reserving
        // the name first would expose a placeholder and race another writer.
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => {
            Err(exclusive_rename_unsupported())
        }
        result => result,
    }
}

#[cfg(target_os = "macos")]
fn rename_exclusive(source: &Path, target: &Path) -> io::Result<()> {
    rename_exclusive_at(source, target, libc::AT_FDCWD)
}

#[cfg(target_os = "macos")]
fn rename_exclusive_at(source: &Path, target: &Path, directory: libc::c_int) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path contains NUL"))?;
    // SAFETY: both arguments are owned NUL-terminated path buffers for the
    // duration of the call; RENAME_EXCL requests an ordinary same-volume move
    // that fails rather than replacing an occupied target.
    let result = unsafe { libc::renameatx_np(libc::AT_FDCWD, source.as_ptr(), directory, target.as_ptr(), libc::RENAME_EXCL) }; // volume_io worker
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn exclusive_rename_unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "the filesystem does not support atomic no-replace publication",
    )
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
        // data root
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
fn rename_no_replace_raw(
    source: &Path,
    target: &Path,
    _exclusive_unsupported: bool,
    source_version: Option<crate::file_identity::FileVersion>,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

    require_source_version(source, source_version)?;
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
    // call. Write-through requests disk completion without permitting either
    // replacement or a cross-volume copy/delete fallback. Its exact power-loss
    // contract still depends on the destination filesystem; it is not a
    // substitute for flushing the actual file before source cleanup.
    let succeeded = unsafe {
        MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) // volume_io worker
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn rename_no_replace_raw(
    source: &Path,
    target: &Path,
    _exclusive_unsupported: bool,
    source_version: Option<crate::file_identity::FileVersion>,
) -> io::Result<()> {
    // Non-shipping test/development platforms: hard-link publication has the
    // same atomic no-clobber property. The source remains recovery authority
    // if removing the staging name fails.
    require_source_version(source, source_version)?;
    std::fs::hard_link(source, target)?; // volume_io worker
    require_source_version(source, source_version)?;
    std::fs::remove_file(source) // volume_io worker
}

fn require_source_version(
    source: &Path,
    version: Option<crate::file_identity::FileVersion>,
) -> io::Result<()> {
    if let Some(version) = version {
        version.require_path(source)?;
    }
    Ok(())
}
