//! Review eligibility is independent of inventory and duplicate membership.
//! Native facts are collected by discovery; policy changes use indexed facts.

use serde_json::Value;
use std::collections::HashSet;
use std::fs::Metadata;
use std::path::Path;

pub const DOT: i64 = 1;
pub const HIDDEN: i64 = 2;
pub const SYSTEM: i64 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub hidden_flags: i64,
    pub ignored_file_names: HashSet<String>,
}

impl Policy {
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let defaults = crate::storage::DefaultConfig::default();
        let boolean = |key: &str, default: bool| match config.get(key) {
            None => Ok(default),
            Some(value) => value
                .as_bool()
                .ok_or_else(|| format!("{key} must be a boolean")),
        };
        let names = match config.get("ignoredFileNames") {
            None => defaults.ignored_file_names,
            Some(value) => clean_ignored_file_names(value)?,
        };
        Ok(Self {
            hidden_flags: if boolean("hideDotNames", defaults.hide_dot_names)? {
                DOT
            } else {
                0
            } | if boolean("hideHiddenAttributes", defaults.hide_hidden_attributes)? {
                HIDDEN
            } else {
                0
            } | if boolean("hideSystemAttributes", defaults.hide_system_attributes)? {
                SYSTEM
            } else {
                0
            },
            ignored_file_names: names.into_iter().map(|name| name.to_lowercase()).collect(),
        })
    }

    pub fn visible(&self, name: &str, is_directory: bool, flags: i64) -> bool {
        flags & self.hidden_flags == 0
            && (is_directory || !self.ignored_file_names.contains(&name.to_lowercase()))
    }
}

/// Whether a trimmed, non-blank item names a file: the whole name a folder
/// listing gives a file, which is what [`Policy::visible`] compares. A path,
/// `.` or `..`, or a name holding a NUL or a line break would never match.
/// Settings marks such a row by the same rule (`src/models/ignoredFileNames.ts`);
/// both sides read the cases in `tests/fixtures/ignored-file-names.json`.
pub fn is_file_name(name: &str) -> bool {
    !matches!(name, "" | "." | "..") && !name.contains(['/', '\\', '\0', '\n', '\r'])
}

/// The ignored file names as the app holds them: each item trimmed, blank
/// items dropped, and an item repeated ignoring case, as matching does, kept
/// once where it first appears. The error names the first item that is not a
/// file name.
pub fn clean_ignored_file_names(value: &Value) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for item in value.as_array().ok_or("Ignored file names must be a list")? {
        let name = item.as_str().ok_or("Ignored file names must contain text")?.trim();
        if name.is_empty() {
            continue;
        }
        if !is_file_name(name) {
            return Err(format!("{name} is not a file name"));
        }
        if seen.insert(name.to_lowercase()) {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Whether two lists ignore the same files. Order carries no meaning and
/// matching ignores case, so the list is compared as a set.
pub fn same_ignored_file_names(left: &Value, right: &Value) -> bool {
    let set = |value: &Value| {
        value.as_array().map(|names| {
            names.iter().filter_map(Value::as_str).map(str::to_lowercase).collect::<HashSet<_>>()
        })
    };
    set(left) == set(right)
}

pub fn windows_flags(attributes: u32) -> i64 {
    (if attributes & 2 != 0 { HIDDEN } else { 0 }) | (if attributes & 4 != 0 { SYSTEM } else { 0 })
}

pub fn macos_flags(flags: u32) -> i64 {
    // Darwin UF_HIDDEN. Other UF_/SF_ flags do not mean a Windows system file.
    if flags & 0x0000_8000 != 0 {
        HIDDEN
    } else {
        0
    }
}

pub fn native_flags(metadata: &Metadata) -> i64 {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        windows_flags(metadata.file_attributes())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        macos_flags(metadata.st_flags())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = metadata;
        0
    }
}

pub fn entry_flags(path: &Path, metadata: &Metadata) -> i64 {
    native_flags(metadata)
        | if path
            .file_name()
            .is_some_and(|name| crate::native_path::encode(&name).starts_with('.'))
        {
            DOT
        } else {
            0
        }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub hidden_attributes: bool,
    pub system_attributes: bool,
}

pub fn capabilities() -> Capabilities {
    Capabilities {
        hidden_attributes: cfg!(any(target_os = "macos", target_os = "windows")),
        system_attributes: cfg!(target_os = "windows"),
    }
}
