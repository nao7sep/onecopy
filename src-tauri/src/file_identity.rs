//! Physical file identity for app-created private files, published outputs,
//! and filesystem alias checks.
//!
//! A pathname is only a lookup. Once this operation creates or opens a file,
//! private cleanup and verified publication bind to the filesystem identity
//! returned by that handle so an unrelated replacement is never treated as an
//! app-created temporary output.

#[cfg(not(windows))]
use std::fs::Metadata;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u32,
    #[cfg(windows)]
    index: u64,
}

impl FileIdentity {
    pub fn from_file(file: &File) -> io::Result<Self> {
        #[cfg(windows)]
        {
            return from_windows_file(file);
        }
        #[cfg(not(windows))]
        {
            Self::from_metadata(&file.metadata()?)
        }
    }

    pub fn from_path(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            let fs_path = crate::winpath::for_fs(path);
            let mut options = OpenOptions::new();
            options.read(true).custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
                    | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS,
            );
            return Self::from_file(&options.open(fs_path.as_ref())?);
        }
        #[cfg(not(windows))]
        {
            // Do not follow a replacement symlink: the directory entry itself is
            // not the regular file this operation created.
            Self::from_metadata(&std::fs::symlink_metadata(path)?)
        }
    }

    #[cfg(not(windows))]
    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = metadata;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "physical file identity is unsupported on this platform",
            ))
        }
    }
}

/// The physical volume holding `path`, following a final symlink. Two paths
/// are on the same filesystem exactly when these agree, however each is
/// spelled (a mapped or `subst` drive, a symlinked root, a verbatim path).
pub fn volume_of(path: &Path) -> io::Result<u64> {
    let fs_path = crate::winpath::for_fs(path);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .access_mode(0)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS)
            .open(fs_path.as_ref())?;
        return Ok(u64::from(from_windows_file(&file)?.volume));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(std::fs::metadata(fs_path.as_ref())?.dev())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fs_path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "volume identity is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn from_windows_file(file: &File) -> io::Result<FileIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut information = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: `file` owns a valid handle for this call, and `information` points
    // to writable storage for exactly the structure Windows initializes.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a nonzero result guarantees the structure was initialized.
    let information = unsafe { information.assume_init() };
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(FileIdentity {
        volume: information.dwVolumeSerialNumber,
        index,
    })
}

/// Opens one existing regular file without following a final symlink/reparse
/// point, then captures the physical identity of that exact descriptor. The
/// open never waits on a special file: a FIFO put at the path is refused
/// rather than blocking the operation that asked.
pub fn open_regular_nofollow(path: &Path) -> io::Result<(File, FileIdentity)> {
    let fs_path = crate::winpath::for_fs(path);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NONBLOCK only changes opening a FIFO or device; reads of the
        // regular file this function accepts are unaffected by it.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(fs_path.as_ref())?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a no-follow regular file: {}", path.display()),
        ));
    }
    let identity = FileIdentity::from_file(&file)?;
    if !path_names(path, identity) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("file was replaced while opening: {}", path.display()),
        ));
    }
    Ok((file, identity))
}

pub fn path_names(path: &Path, expected: FileIdentity) -> bool {
    FileIdentity::from_path(path).is_ok_and(|actual| actual == expected)
}

/// Whether `path` currently names the file behind `file`. The identity is read
/// from the open descriptor at the moment of the check, not remembered: FAT
/// and exFAT derive an empty file's number from its directory entry, so a
/// rename changes it, while the open descriptor follows the file.
pub fn path_names_file(path: &Path, file: &File) -> bool {
    FileIdentity::from_file(file).is_ok_and(|expected| path_names(path, expected))
}

/// Moves a private staging pathname into a fresh private hold and verifies the
/// physical file that actually moved against this operation's open
/// descriptor. This is the operation-owned claim used by both publication and
/// cleanup. A replacement is restored (or retained in the hold if its old name
/// was occupied again), never treated as ours.
pub fn claim_private(path: &Path, file: &File) -> io::Result<std::path::PathBuf> {
    let Some(parent) = path.parent() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private staging path has no parent",
        ));
    };
    for _ in 0..4 {
        let hold = parent.join(format!(
            ".onecopy-claim-{}.tmp",
            crate::nanoid::generate().map_err(io::Error::other)?
        ));
        match crate::fs_publish::rename_no_replace(path, &hold) {
            Ok(()) => {
                if path_names_file(&hold, file) {
                    return Ok(hold);
                } else {
                    // The pathname was replaced before our claim. Put that
                    // file back when possible; otherwise leave it recoverable
                    // under the private hold rather than deleting a winner.
                    return match crate::fs_publish::rename_no_replace(&hold, path) {
                        Ok(()) => Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "private staging pathname was replaced",
                        )),
                        Err(restore_error) => Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            format!(
                                "private staging pathname was replaced; the replacement remains at {} because restoring it failed: {restore_error}",
                                hold.display()
                            ),
                        )),
                    };
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a private physical-claim pathname",
    ))
}

/// Best-effort cleanup of a private staging pathname. Callers never use this
/// for a public committed target; public targets are never unlinked as rollback.
pub fn remove_private_if_owned(path: &Path, file: &File) {
    match claim_private(path, file) {
        Ok(hold) => crate::fs_recovery::remove_file(&hold, "private staging cleanup"),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::AlreadyExists
            ) => {}
        Err(error) => crate::logging::warn(
            "private staging claim failed during cleanup",
            serde_json::json!({
                "path": path,
                "error": { "message": error.to_string() },
            }),
        ),
    }
}

/// One file this operation created under a private destination name, bound
/// to the descriptor that wrote it. Until it is published, dropping it removes
/// the private file, but only while that name still holds this file, so every
/// early return, failure, and cancellation abandons its private output.
#[derive(Debug)]
pub struct PrivateFile {
    path: std::path::PathBuf,
    file: File,
    published: bool,
}

impl PrivateFile {
    pub fn new(path: std::path::PathBuf, file: File) -> Self {
        Self {
            path,
            file,
            published: false,
        }
    }

    /// The private name, or the final name once published.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn file_mut(&mut self) -> &mut File {
        &mut self.file
    }

    /// Whether `path` currently names this file.
    pub fn is_named_by(&self, path: &Path) -> bool {
        path_names_file(path, &self.file)
    }

    /// Moves the file to a fresh private hold so no other writer can have
    /// replaced the name between this check and publication.
    pub fn claim(&mut self) -> io::Result<()> {
        self.path = claim_private(&self.path, &self.file)?;
        Ok(())
    }

    /// Publishes the file at `target` without replacing another entry. An
    /// occupied target answers `AlreadyExists` and the file stays private.
    pub fn publish(&mut self, target: &Path) -> io::Result<()> {
        crate::fs_publish::rename_no_replace(&self.path, target)?;
        self.path = target.to_path_buf();
        self.published = true;
        if !path_names_file(target, &self.file) {
            // Not `AlreadyExists`: the name was ours and is now someone
            // else's, which is a failure, not an occupied target to review.
            return Err(io::Error::other(
                format!(
                    "published output was replaced before completion: {}",
                    target.display()
                )));
        }
        Ok(())
    }
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        if !self.published {
            remove_private_if_owned(&self.path, &self.file);
        }
    }
}
