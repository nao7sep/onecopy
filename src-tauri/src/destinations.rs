//! The destination tree: immediate subdirectories of one directory (the tree
//! expands lazily; files are never listed — it is a destination panel, not a
//! file manager), creating a subfolder, and removing an empty one.

use std::path::Path;

use serde_json::json;

use crate::{paths, storage, trash, visibility};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    name: String,
    path: String,
    has_children: bool,
    is_empty: bool,
}

pub fn list_subdirs(data_root: &Path, path: &str) -> Result<Vec<DirEntry>, String> {
    let config = storage::read_config_for_setup(data_root)?;
    let policy = visibility::Policy::from_config(config.as_ref().unwrap_or(&json!({})))?;
    list_subdirs_at(Path::new(path), &policy, data_root)
}

fn list_subdirs_at(
    path: &Path,
    policy: &visibility::Policy,
    data_root: &Path,
) -> Result<Vec<DirEntry>, String> {
    if trash::is_trash_path(path) || paths::is_within_data_root(path, data_root) {
        return Ok(Vec::new());
    }
    let mut entries: Vec<DirEntry> = Vec::new();
    let read = std::fs::read_dir(path).map_err(|e| e.to_string())?;
    for entry in read {
        let entry = entry.map_err(|error| error.to_string())?;
        if !is_browsable_destination_child(&entry, policy, data_root)? {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let child_path = entry.path();
        let (has_children, is_empty) = child_directory_facts(&child_path, policy, data_root)?;
        entries.push(DirEntry {
            name,
            path: child_path.to_string_lossy().to_string(),
            has_children,
            is_empty,
        });
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

fn is_browsable_destination_child(
    entry: &std::fs::DirEntry,
    policy: &visibility::Policy,
    data_root: &Path,
) -> Result<bool, String> {
    if trash::is_trash_path(&entry.path())
        || paths::is_within_data_root(&entry.path(), data_root)
        || !entry.file_type().map_err(|error| error.to_string())?.is_dir()
    {
        return Ok(false);
    }
    let metadata = entry.metadata().map_err(|error| error.to_string())?;
    Ok(policy.visible(
        &entry.file_name().to_string_lossy(),
        true,
        visibility::entry_flags(&entry.path(), &metadata),
    ))
}

fn child_directory_facts(
    path: &Path,
    policy: &visibility::Policy,
    data_root: &Path,
) -> Result<(bool, bool), String> {
    let children = std::fs::read_dir(path).map_err(|error| error.to_string())?;
    let mut is_empty = true;
    for child in children {
        let child = child.map_err(|error| error.to_string())?;
        is_empty = false;
        if is_browsable_destination_child(&child, policy, data_root)? {
            return Ok((true, false));
        }
    }
    Ok((false, is_empty))
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
    for entry in std::fs::read_dir(parent).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.file_name().to_string_lossy().to_lowercase() == lower {
            return Err(format!(
                "\"{name}\" already exists here (names are case-insensitively unique)"
            ));
        }
    }
    let target = parent.join(name);
    std::fs::create_dir(&target).map_err(|e| e.to_string())?;
    Ok(target.to_string_lossy().to_string())
}

/// Deletes a tree folder ONLY when empty — `remove_dir` refuses otherwise,
/// which is the entire safety model (empty folders render distinctly in the
/// tree).
pub fn delete_empty_dir(path: &Path) -> Result<(), String> {
    std::fs::remove_dir(path).map_err(|e| e.to_string())
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private listing and
// name validation behind the destination commands; promoting them would
// widen the crate's API only for these tests.
#[path = "../tests/unit/destinations.rs"]
mod tests;
