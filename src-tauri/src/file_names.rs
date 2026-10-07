//! How a folder on a user volume compares file names, and the ordinary
//! Rename suffix. Shared by every operation that places a file under a name
//! it must not collide with: Copy and Move into a destination, and Restore
//! back into a configured root.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

use crate::volume_io;

/// The Rename suffix style: `name 2.ext` (macOS default) or `name (2).ext`
/// (Windows default), selected by the platform.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RenameStyle {
    SpaceNumber,
    ParenthesizedNumber,
}

impl RenameStyle {
    pub fn platform_default() -> Self {
        if cfg!(target_os = "windows") { Self::ParenthesizedNumber } else { Self::SpaceNumber }
    }
}

/// How a folder's filesystem compares names. Case-insensitive volumes (the
/// default on macOS and Windows) treat names differing only by case as one
/// entry, so planning, conflict review and renames must too. APFS and HFS+
/// (macOS) additionally normalize names on the way to disk, so a name that
/// differs only in Unicode normalization form (NFC vs. NFD) is also one entry
/// there, case sensitivity aside; NTFS, exFAT and FAT do not normalize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderNames {
    fold_case: bool,
    normalize_unicode: bool,
}

impl FolderNames {
    pub fn for_directory(directory: &Path) -> Self {
        Self {
            fold_case: !directory_is_case_sensitive(directory),
            normalize_unicode: cfg!(target_os = "macos"),
        }
    }

    /// Explicit rules, for planning without a filesystem.
    pub fn new(fold_case: bool, normalize_unicode: bool) -> Self {
        Self {
            fold_case,
            normalize_unicode,
        }
    }

    pub fn folds_case(self) -> bool {
        self.fold_case
    }

    /// The key under which the filesystem identifies `path`.
    pub fn key(self, path: &Path) -> std::ffi::OsString {
        if !self.fold_case && !self.normalize_unicode {
            return path.as_os_str().to_owned();
        }
        let mut text = path.to_string_lossy().into_owned();
        if self.normalize_unicode {
            text = text.nfc().collect();
        }
        if self.fold_case {
            text = text.to_lowercase();
        }
        text.into()
    }

    pub fn same(self, left: &Path, right: &Path) -> bool {
        self.key(left) == self.key(right)
    }
}

#[cfg(target_os = "macos")]
fn directory_is_case_sensitive(directory: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(directory.as_os_str().as_bytes()) else {
        return false;
    };
    // An unanswerable query (a volume not responding included) reads as
    // case-insensitive, which only ever presents more names as conflicts,
    // never fewer.
    volume_io::call(directory, volume_io::Op::Stat, None, move || {
        // SAFETY: `path` is an owned NUL-terminated buffer alive for the call.
        Ok(unsafe { libc::pathconf(path.as_ptr(), libc::_PC_CASE_SENSITIVE) == 1 }) // volume_io worker
    })
    .unwrap_or(false)
}

#[cfg(windows)]
fn directory_is_case_sensitive(_directory: &Path) -> bool {
    false
}

#[cfg(not(any(target_os = "macos", windows)))]
fn directory_is_case_sensitive(_directory: &Path) -> bool {
    true
}

/// Whether the filesystem refused a name itself rather than the storage.
pub fn is_name_error(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::InvalidFilename {
        return true;
    }
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::EILSEQ)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// `target` with the Rename suffix `number` in `style`; `None` for a name
/// that is not valid Unicode.
pub fn renamed(target: &Path, number: u32, style: RenameStyle) -> Option<PathBuf> {
    let stem = target.file_stem()?.to_str()?;
    let suffix = match style {
        RenameStyle::SpaceNumber => format!(" {number}"),
        RenameStyle::ParenthesizedNumber => format!(" ({number})"),
    };
    let mut name = format!("{stem}{suffix}");
    if let Some(extension) = target.extension().and_then(|value| value.to_str()) {
        name.push('.');
        name.push_str(extension);
    }
    Some(target.with_file_name(name))
}
