//! Volume identity — an identifier stronger than "the folder exists". The
//! developer's backup drives share identical directory structures, so
//! directory presence proves nothing about WHICH drive is mounted; the
//! session gate must catch a substituted volume, not just an absent one.
//!
//! macOS: the volume UUID via `diskutil info -plist` on the mount point.
//! Windows: the volume serial via a direct `GetVolumeInformationW` call (a
//! ~20-line FFI declaration beats a whole windows-crate dependency).
//! A volume whose identity cannot be read yields None — nothing is recorded
//! and presence remains the only verification (honest degradation for
//! filesystems without a stable identity, e.g. some network mounts).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::{logging, paths, storage};

/// The identity of the volume containing `path`, when the platform can say.
pub fn volume_identity(path: &Path) -> Option<String> {
    let root = crate::trash::volume_root_of(path).ok()?;
    platform_identity(&root)
}

/// `diskutil info` normally answers in milliseconds; it talks to
/// diskarbitrationd and stats the mount, so a failing external drive or a stale
/// network mount can block it indefinitely — which is this app's expected input,
/// not an exotic one. Bounded like the `-version` probe in `binaries_manager`,
/// and for the same reason: a wedged probe must not hold a check open forever.
#[cfg(target_os = "macos")]
const DISKUTIL_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(target_os = "macos")]
fn platform_identity(root: &Path) -> Option<String> {
    let mut command = std::process::Command::new("diskutil");
    command.args(["info", "-plist"]).arg(root);
    // No cancel flag reaches here — every caller is a synchronous gate, so the
    // idle bound is the whole protection. A killed or failed probe is simply an
    // unreadable identity, which this module already degrades to honestly.
    let run =
        crate::subprocess::run_bounded_idle(command, &|| false, DISKUTIL_IDLE_TIMEOUT).ok()?;
    if !run.status_ok {
        return None;
    }
    extract_plist_string(&String::from_utf8_lossy(&run.stdout), "VolumeUUID")
}

/// Pulls `<key>NAME</key><string>VALUE</string>` out of plist XML by string
/// search — the one value we need does not justify a plist crate.
#[cfg(target_os = "macos")]
fn extract_plist_string(plist: &str, key: &str) -> Option<String> {
    let key_tag = format!("<key>{key}</key>");
    let after = &plist[plist.find(&key_tag)? + key_tag.len()..];
    let start = after.find("<string>")? + "<string>".len();
    let end = after.find("</string>")?;
    if start >= end {
        return None;
    }
    let value = after[start..end].trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(windows)]
fn platform_identity(root: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetVolumeInformationW(
            root_path_name: *const u16,
            volume_name_buffer: *mut u16,
            volume_name_size: u32,
            volume_serial_number: *mut u32,
            maximum_component_length: *mut u32,
            file_system_flags: *mut u32,
            file_system_name_buffer: *mut u16,
            file_system_name_size: u32,
        ) -> i32;
    }
    let mut wide: Vec<u16> = root.as_os_str().encode_wide().collect();
    if wide.last() != Some(&u16::from(b'\\')) {
        wide.push(u16::from(b'\\'));
    }
    wide.push(0);
    let mut serial: u32 = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            wide.as_ptr(),
            std::ptr::null_mut(),
            0,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    };
    (ok != 0 && serial != 0).then(|| format!("{serial:08X}"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_identity(_root: &Path) -> Option<String> {
    None
}

/// What comparing a directory's CURRENT volume identity against the recorded
/// one means. Kept apart from the command shell because it is the gate that
/// catches a different drive mounted at a configured path — the case
/// `volume_identity` exists for, since backup drives share directory
/// structures — and it runs before every destructive operation.
#[derive(Debug, PartialEq, Eq)]
pub enum IdentityCheck {
    /// Nothing was recorded for this directory; the identity is now stored.
    FirstSight,
    Unchanged,
    /// A DIFFERENT volume is mounted here. The record is deliberately left
    /// alone: overwriting it would launder the substitution into the new
    /// normal, and the developer must resolve it.
    Substituted {
        recorded: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceVolume {
    dir: String,
    identity: String,
    recorded_at_utc: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceVolumeStore {
    sources: Vec<SourceVolume>,
}

fn store_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn load_unlocked(root: &Path) -> Result<BTreeMap<String, SourceVolume>, String> {
    let file = root.join(paths::SOURCE_VOLUMES_FILE_NAME);
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("could not read {}: {error}", file.display())),
    };
    let store: SourceVolumeStore = serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not read {}: {error}", file.display()))?;
    Ok(store
        .sources
        .into_iter()
        .map(|source| (source.dir.clone(), source))
        .collect())
}

fn save_unlocked(root: &Path, sources: BTreeMap<String, SourceVolume>) -> Result<(), String> {
    let store = SourceVolumeStore {
        sources: sources.into_values().collect(),
    };
    let mut text = serde_json::to_string_pretty(&store).map_err(|error| error.to_string())?;
    text.push('\n');
    storage::write_atomic(&root.join(paths::SOURCE_VOLUMES_FILE_NAME), text.as_bytes())
}

pub fn check_identity(root: &Path, dir: &str, current: &str) -> Result<IdentityCheck, String> {
    let _guard = store_lock();
    let mut sources = load_unlocked(root)?;
    match sources.get(dir).map(|source| source.identity.clone()) {
        None => {
            sources.insert(
                dir.to_string(),
                SourceVolume {
                    dir: dir.to_string(),
                    identity: current.to_string(),
                    recorded_at_utc: logging::now_iso_millis(),
                },
            );
            save_unlocked(root, sources)?;
            Ok(IdentityCheck::FirstSight)
        }
        Some(recorded) if recorded != current => Ok(IdentityCheck::Substituted { recorded }),
        Some(_) => Ok(IdentityCheck::Unchanged),
    }
}

/// Drops recorded identities for directories that are no longer configured,
/// so removing a source root does not leave a record that would later flag a
/// re-added path as substituted.
pub fn prune_identities(root: &Path, configured: &[String]) -> Result<u64, String> {
    let _guard = store_lock();
    let mut sources = load_unlocked(root)?;
    let before = sources.len();
    sources.retain(|dir, _| configured.contains(dir));
    let pruned = before - sources.len();
    if pruned > 0 {
        save_unlocked(root, sources)?;
    }
    Ok(pruned as u64)
}

#[cfg(test)]
mod tests {
    // EXCEPTION to the tests-live-in-tests/ rule (tests-folder conventions,
    // Rust form): the plist string extractor is a private parsing detail —
    // promoting it would widen the surface just to test through it. The
    // public volume_identity is exercised from tests/volume_tests.rs.
    #[cfg(target_os = "macos")]
    #[test]
    fn plist_extraction_finds_the_keyed_string() {
        let plist = r#"<dict>
            <key>VolumeName</key><string>Macintosh HD</string>
            <key>VolumeUUID</key>
            <string>  AAAA-BBBB  </string>
        </dict>"#;
        assert_eq!(
            super::extract_plist_string(plist, "VolumeUUID").as_deref(),
            Some("AAAA-BBBB")
        );
        assert_eq!(super::extract_plist_string(plist, "Missing"), None);
    }
}
