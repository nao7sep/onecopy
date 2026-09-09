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
            Some(value) => value
                .as_array()
                .ok_or("Ignored file names must be a list")?
                .iter()
                .map(|value| {
                    let name = value
                        .as_str()
                        .ok_or("Ignored file names must contain text")?;
                    if name.is_empty() || name.contains(['/', '\\', '\0', '\n', '\r']) {
                        return Err("Ignored file names must be complete basenames, not paths");
                    }
                    Ok(name.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?,
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
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
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
