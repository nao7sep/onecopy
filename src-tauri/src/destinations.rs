//! The destination tree: immediate subdirectories of one directory (the tree
//! expands lazily; files are never listed — it is a destination panel, not a
//! file manager), creating a subfolder, and removing an empty one.

use std::path::Path;


use crate::{paths, storage, trash, visibility};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    name: String,
    path: String,
    has_children: bool,
    is_empty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_error: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    entries: Vec<DirEntry>,
    is_empty: bool,
}

pub fn list_subdirs(data_root: &Path, path: &str) -> Result<DirListing, String> {
    let policy = visibility::Policy::from_config(&*storage::config(data_root)?)?;
    list_subdirs_at(
        crate::native_path::from_text(path).as_ref(), &policy, data_root,
    )
}

fn list_subdirs_at(
    path: &Path,
    policy: &visibility::Policy,
    data_root: &Path,
) -> Result<DirListing, String> {
    if trash::is_trash_path(path) || paths::is_within_data_root(path, data_root) {
        return Ok(DirListing {
            entries: Vec::new(),
            is_empty: false,
        });
    }
    let mut entries: Vec<DirEntry> = Vec::new();
    // Every listing is one bounded call; a drive that does not answer fails
    // the request instead of leaving the tree waiting.
    let raw_entries = crate::volume_io::read_dir(path, true).map_err(|e| e.to_string())?;
    // Files and hidden entries still make this folder unsafe to delete.
    let is_empty = raw_entries.is_empty();
    for entry in raw_entries {
        if !is_browsable_destination_child(&entry, policy, data_root)? {
            continue;
        }
        let name = crate::native_path::encode(&entry.file_name).to_string();
        // Child inspection is local to this row. A failed probe cannot erase
        // accessible siblings or authorize deletion from unknown emptiness.
        let (has_children, is_empty, read_error) = match child_directory_facts(&entry.path, policy, data_root) {
            Ok((has_children, is_empty)) => (has_children, is_empty, None),
            Err(error) => (true, false, Some(error)),
        };
        entries.push(DirEntry {
            name,
            path: crate::native_path::encode(&entry.path).to_string(),
            has_children,
            is_empty,
            read_error,
        });
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(DirListing { entries, is_empty })
}

fn is_browsable_destination_child(
    entry: &crate::volume_io::DirEntryInfo,
    policy: &visibility::Policy,
    data_root: &Path,
) -> Result<bool, String> {
    let file_type = entry
        .file_type
        .ok_or_else(|| format!("could not read {}", crate::native_path::label(&entry.path)))?;
    if trash::is_trash_path(&entry.path)
        || paths::is_within_data_root(&entry.path, data_root)
        || !file_type.is_dir()
    {
        return Ok(false);
    }
    let metadata = match &entry.metadata {
        Some(Ok(metadata)) => metadata,
        Some(Err(error)) => return Err(error.to_string()),
        None => return Err(format!("could not read {}", crate::native_path::label(&entry.path))),
    };
    Ok(policy.visible(
        &crate::native_path::encode(&entry.file_name),
        true,
        visibility::entry_flags(&entry.path, metadata),
    ))
}

/// Whether a child folder has browsable children, and whether it is empty. A
/// failed probe propagates to the row owner, which keeps the child expandable
/// and nonempty with a visible failure. Expanding it asks again.
fn child_directory_facts(
    path: &Path,
    policy: &visibility::Policy,
    data_root: &Path,
) -> Result<(bool, bool), String> {
    let policy = policy.clone();
    let data_root = data_root.to_path_buf();
    crate::volume_io::probe_dir(path, move |child| {
        is_browsable_destination_child(child, &policy, &data_root).map_err(std::io::Error::other)
    }).map_err(|error| error.to_string())
}

/// The folder name a user typed, trimmed; refused when empty, when it holds
/// a path separator, or when it holds a control character (a pasted newline
/// is the real case), since such a name cannot be typed or read sanely.
fn folder_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains(['/', '\\']) || trimmed.chars().any(char::is_control)
    {
        return Err("folder names must be non-empty, slash-free, and single-line".to_string());
    }
    Ok(trimmed)
}

/// Creates a subfolder under a tree node. The name must be
/// case-insensitively unique within its directory (storage-path conventions'
/// hard invariant).
pub fn create_subdir(parent: &Path, name: &str) -> Result<String, String> {
    let name = folder_name(name)?;
    let lower = name.to_lowercase();
    for entry in crate::volume_io::read_dir(parent, false).map_err(|error| error.to_string())? {
        if crate::native_path::encode(&entry.file_name).to_lowercase() == lower {
            return Err(format!(
                "\"{name}\" already exists here (names are case-insensitively unique)"
            ));
        }
    }
    let target = parent.join(name);
    crate::volume_io::create_dir(&target).map_err(|e| e.to_string())?;
    Ok(crate::native_path::encode(&target).to_string())
}

/// Deletes a tree folder ONLY when empty — `remove_dir` refuses otherwise,
/// which is the entire safety model (empty folders render distinctly in the
/// tree).
pub fn delete_empty_dir(path: &Path) -> Result<(), String> {
    crate::volume_io::remove_dir(path).map_err(|e| e.to_string())
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private listing and
// name validation behind the destination commands; promoting them would
// widen the crate's API only for these tests.
#[path = "../tests/unit/destinations.rs"]
mod tests;
