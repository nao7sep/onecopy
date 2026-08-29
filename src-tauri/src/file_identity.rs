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
/// point, then captures the physical identity of that exact descriptor.
pub fn open_regular_nofollow(path: &Path) -> io::Result<(File, FileIdentity)> {
    let fs_path = crate::winpath::for_fs(path);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
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

/// Moves a private staging pathname into a fresh private hold and verifies the
/// physical file that actually moved. This is the operation-owned claim used
/// by both publication and cleanup. A replacement is restored (or retained in
/// the hold if its old name was occupied again), never treated as ours.
pub fn claim_private(path: &Path, expected: FileIdentity) -> io::Result<std::path::PathBuf> {
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
                if path_names(&hold, expected) {
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
pub fn remove_private_if_owned(path: &Path, expected: FileIdentity) {
    if let Ok(hold) = claim_private(path, expected) {
        crate::fs_recovery::remove_file(&hold, "private staging cleanup");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_cleanup_preserves_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stage.tmp");
        let held = dir.path().join("held.tmp");
        std::fs::write(&path, b"ours").unwrap();
        let identity = FileIdentity::from_path(&path).unwrap();
        std::fs::rename(&path, &held).unwrap();
        std::fs::write(&path, b"winner").unwrap();

        remove_private_if_owned(&path, identity);

        assert_eq!(std::fs::read(&path).unwrap(), b"winner");
        assert_eq!(std::fs::read(&held).unwrap(), b"ours");
    }

    #[test]
    fn physical_claim_rejects_and_restores_a_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stage.tmp");
        let ours = dir.path().join("ours.tmp");
        std::fs::write(&path, b"ours").unwrap();
        let identity = FileIdentity::from_path(&path).unwrap();
        std::fs::rename(&path, &ours).unwrap();
        std::fs::write(&path, b"winner").unwrap();

        assert!(claim_private(&path, identity).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"winner");
        assert_eq!(std::fs::read(&ours).unwrap(), b"ours");
    }

    #[cfg(unix)]
    #[test]
    fn nofollow_regular_open_rejects_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.bin");
        let link = dir.path().join("link.bin");
        std::fs::write(&real, b"bytes").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(open_regular_nofollow(&link).is_err());
        assert_eq!(std::fs::read(&real).unwrap(), b"bytes");
    }
}
