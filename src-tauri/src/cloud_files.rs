//! Files kept by a sync service (iCloud Drive, OneDrive, Dropbox, Google Drive
//! and others). An online-only file is a placeholder whose contents live with
//! the service: reading it downloads it, so OneCopy neither reads nor indexes
//! one, and says how many each source folder holds. A deletion inside a
//! synced folder is a deletion the service carries to the cloud and the
//! user's other devices, so permanent deletion and Empty say so first.

use std::fs::Metadata;
use std::path::{Path, PathBuf};

/// macOS `SF_DATALESS`: the file's contents are not on this computer.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

/// Windows cloud-file and offline attributes: the contents are fetched when
/// the file is opened or read.
#[cfg(windows)]
const ONLINE_ONLY_ATTRIBUTES: u32 = 0x0040_0000 // FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    | 0x0004_0000 // FILE_ATTRIBUTE_RECALL_ON_OPEN
    | 0x0000_1000; // FILE_ATTRIBUTE_OFFLINE

/// Whether `metadata` describes an online-only placeholder.
#[allow(unused_variables)]
pub fn is_online_only(metadata: &Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        metadata.st_flags() & SF_DATALESS != 0
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & ONLINE_ONLY_ATTRIBUTES != 0
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

/// The folders a sync service keeps on this computer, in their usual
/// places: on macOS iCloud Drive (`~/Library/Mobile Documents`) and the
/// File Provider folders other services use (`~/Library/CloudStorage`); on
/// Windows the OneDrive folders the system names in the environment; and
/// a Dropbox folder in the home folder on either.
pub fn synced_folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        {
            folders.push(home.join("Library").join("Mobile Documents"));
            folders.push(home.join("Library").join("CloudStorage"));
        }
        folders.push(home.join("Dropbox"));
    }
    #[cfg(windows)]
    for name in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Some(folder) = std::env::var_os(name) {
            folders.push(PathBuf::from(folder));
        }
    }
    folders
}

/// Whether `path` is inside one of `synced` (case-insensitively on Windows
/// and macOS, whose default volumes ignore case).
pub fn is_synced(path: &Path, synced: &[PathBuf]) -> bool {
    let path = lowercase(path);
    synced.iter().any(|folder| path.starts_with(lowercase(folder)))
}

/// Whether any live copy of `items` sits in a synced folder.
pub fn items_in_synced_folders(
    conn: &rusqlite::Connection,
    items: &[crate::operations::ItemIdentity],
    synced: &[PathBuf],
) -> Result<bool, String> {
    if synced.is_empty() {
        return Ok(false);
    }
    let mut by_hash = conn
        .prepare_cached("SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0")
        .map_err(|error| error.to_string())?;
    let mut by_id = conn
        .prepare_cached("SELECT abs_path FROM paths WHERE id = ?1 AND missing = 0")
        .map_err(|error| error.to_string())?;
    for item in items {
        let paths: Vec<String> = match (&item.hash, item.path_id) {
            (Some(hash), _) => by_hash
                .query_map([hash], |row| row.get(0))
                .and_then(|rows| rows.collect()),
            (None, Some(id)) => by_id
                .query_map([id], |row| row.get(0))
                .and_then(|rows| rows.collect()),
            (None, None) => continue,
        }
        .map_err(|error| error.to_string())?;
        if paths.iter().any(|path| is_synced(Path::new(path), synced)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn lowercase(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[cfg(test)]
#[path = "../tests/unit/cloud_files.rs"]
mod tests;
