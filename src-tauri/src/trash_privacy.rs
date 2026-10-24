//! Account-private day directories. Existing shared days are never tightened.
//! Handles retain the permission boundary while a manifest or file is written.

#[cfg(any(windows, test))]
use crate::volume_io::VolumeFile;
use crate::volume_io::{self, Op};
use std::{fs::File, io, path::Path, sync::Arc};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
use crate::windows_account_privacy as windows;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

const PREFIX: &str = "private-";

pub(crate) fn account_prefix() -> io::Result<String> {
    Ok(format!("{PREFIX}{}--", platform::account_key()?))
}

/// Foreign private days are outside Browse and Empty, even for an elevated
/// process. Legacy names remain readable without changing their permissions.
pub(crate) fn visible_day(name: &std::ffi::OsStr, account: &str) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    !name.starts_with(PREFIX) || name.starts_with(account)
}

pub(crate) fn is_private(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(PREFIX))
}

pub(crate) struct Directory {
    handle: Arc<File>,
}

impl Directory {
    pub(crate) fn open(path: &Path, create: bool) -> io::Result<Self> {
        let owned = crate::winpath::for_fs(path).into_owned();
        let handle = volume_io::call(path, Op::Create, None, move || {
            platform::open_directory(&owned, create)
        })?;
        Ok(Self {
            handle: Arc::new(handle),
        })
    }

    pub(crate) fn append(&self, path: &Path, line: String) -> io::Result<()> {
        let handle = self.handle.clone();
        let owned = crate::winpath::for_fs(path).into_owned();
        volume_io::call(path, Op::Write, None, move || {
            #[cfg(unix)]
            let mut file = unix::open_manifest(&handle, &owned)?;
            #[cfg(windows)]
            let _guard = handle;
            #[cfg(windows)]
            let mut file = platform::open_manifest(&owned)?;
            volume_io::append_record_synced(&mut file, &line)
        })
    }

    pub(crate) fn sync(&self, path: &Path) -> io::Result<()> {
        let handle = self.handle.clone();
        volume_io::call(path, Op::Sync, None, move || {
            #[cfg(unix)]
            {
                handle.sync_all()
            }
            // Windows publication requests write-through; directory handles
            // do not add an independently qualified durability guarantee.
            #[cfg(windows)]
            {
                let _guard = handle;
                Ok(())
            }
        })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn move_source(
        &self,
        source: &Path,
        target: &Path,
        snapshot: crate::file_identity::SourceSnapshot,
        destination: Option<crate::file_identity::VerifiedDestination>,
    ) -> io::Result<()> {
        crate::fs_publish::rename_into_directory(source, target, self.handle.clone(), snapshot, destination)
    }

    #[cfg(any(windows, test))]
    pub(crate) fn create_file(&self, path: &Path) -> io::Result<crate::file_identity::PrivateFile> {
        let handle = self.handle.clone();
        let owned = crate::winpath::for_fs(path).into_owned();
        let private_path = path.to_path_buf();
        volume_io::call(path, Op::Create, None, move || {
            #[cfg(unix)]
            let file = unix::create_file(&handle, &owned)?;
            #[cfg(windows)]
            let file = {
                let _guard = handle;
                platform::create_file(&owned)?
            };
            let file = VolumeFile::new(file, private_path.clone());
            Ok(crate::file_identity::PrivateFile::new(private_path, file))
        })
    }
}

/// Called only by the final removal worker after recovery and source proof.
#[cfg(windows)]
pub(crate) fn remove_original(path: &Path, version: crate::file_identity::FileVersion) -> io::Result<()> {
    windows::remove_original(path, |file| version.require_file(file))
}

#[cfg(test)]
#[path = "../tests/unit/trash_privacy.rs"]
mod tests;
#[cfg(test)]
#[path = "../tests/unit/trash_privacy_windows.rs"]
mod windows_tests;
