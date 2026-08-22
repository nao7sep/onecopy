//! Physical file identity used at destructive cleanup and publication edges.
//!
//! A pathname is only a lookup. Once this operation creates or opens a file,
//! cleanup and commit verification bind to the filesystem identity returned by
//! that handle so an external replacement is never mistaken for ours.

use std::fs::{File, Metadata};
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
        Self::from_metadata(&file.metadata()?)
    }

    pub fn from_path(path: &Path) -> io::Result<Self> {
        // Do not follow a replacement symlink: the directory entry itself is
        // not the regular file this operation created.
        Self::from_metadata(&std::fs::symlink_metadata(path)?)
    }

    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let volume = metadata.volume_serial_number().ok_or_else(|| {
                io::Error::other("filesystem did not report a volume serial number")
            })?;
            let index = metadata
                .file_index()
                .ok_or_else(|| io::Error::other("filesystem did not report a file index"))?;
            Ok(Self { volume, index })
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

pub fn path_names(path: &Path, expected: FileIdentity) -> bool {
    FileIdentity::from_path(path).is_ok_and(|actual| actual == expected)
}

/// Best-effort cleanup of a private staging pathname. Cleanup first moves the
/// name to another private no-clobber hold, then inspects what actually moved.
/// A replacement is restored (or retained in the hold if its old name was
/// occupied again), never unlinked. Callers never use this for a public target.
pub fn remove_private_if_owned(path: &Path, expected: FileIdentity) {
    let Some(parent) = path.parent() else {
        return;
    };
    for _ in 0..4 {
        let hold = parent.join(format!(".onecopy-cleanup-{}.tmp", crate::nanoid::generate()));
        match crate::fs_publish::rename_no_replace(path, &hold) {
            Ok(()) => {
                if path_names(&hold, expected) {
                    let _ = std::fs::remove_file(&hold);
                } else {
                    // The pathname was replaced before our claim. Put that
                    // file back when possible; otherwise leave it recoverable
                    // under the private hold rather than deleting a winner.
                    let _ = crate::fs_publish::rename_no_replace(&hold, path);
                }
                return;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return,
        }
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
}
