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

/// The volume-substitution gate itself, enforced in the backend rather than
/// left to a frontend check the caller might skip (R1-14): `Err` when a
/// configured directory's current volume identity differs from the one
/// recorded for it, or when the recorded identities cannot even be read. A
/// read failure keeps the gate CLOSED rather than reading as "nothing
/// recorded" (R3-07) — the opposite of `verify_source_dirs`'s own frontend
/// status, which must still answer something for every directory even when
/// this fails. A directory that is absent, or that has no recorded identity
/// because its filesystem had none to read, is not this gate's concern and is
/// skipped.
pub fn enforce_no_substitution(data_root: &Path, dirs: &[String]) -> Result<(), String> {
    let _guard = store_lock();
    let recorded = load_unlocked(data_root)?;
    for dir in dirs {
        let path = Path::new(dir);
        if !path.is_dir() {
            continue;
        }
        let Some(known) = recorded.get(dir) else {
            continue;
        };
        let root = crate::trash::volume_root_of(path)
            .map_err(|error| format!("could not verify the volume for {dir}: {error}"))?;
        // A filesystem without a stable identity is never recorded, so a
        // recorded directory whose identity cannot be read now is either a
        // different volume without one or a check that failed: neither is
        // verified-safe.
        let Some(current) = platform_identity(&root) else {
            return Err(format!(
                "could not verify the volume for {dir}; recheck source folders before continuing"
            ));
        };
        if current != known.identity {
            return Err(format!(
                "{dir} is a different volume than the one OneCopy recorded there; recheck source folders before continuing"
            ));
        }
    }
    Ok(())
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
// EXCEPTION to the tests-live-in-tests/ rule (tests-folder conventions,
// Rust form): the plist string extractor is a private parsing detail —
// promoting it would widen the surface just to test through it. The
// public volume_identity is exercised from tests/volume_tests.rs.
#[path = "../tests/unit/volume.rs"]
mod tests;

#[derive(serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SourceDirsStatus {
    pub missing: Vec<String>,
    pub substituted: Vec<String>,
}

// Presence AND identity verification over the configured source dirs: a dir
// that is not there is missing; a dir whose volume identity differs from the
// recorded one is substituted (the developer's backup drives share identical
// trees, so presence alone proves nothing). First sight records the identity
// — the "when the directory was added" moment as the core observes it. Rows
// for since-removed dirs are pruned; a volume without a readable identity
// degrades to presence-only, logged at debug.
pub fn verify_source_dirs(data_root: &Path) -> Result<SourceDirsStatus, String> {
    let config = storage::read_config_for_setup(data_root)?;
    let settings = crate::scanner::settings_from_config(config.as_ref(), data_root, 0);
    let mut status = SourceDirsStatus::default();
    for dir in &settings.source_dirs {
        let path = std::path::Path::new(dir);
        if !path.is_dir() {
            status.missing.push(dir.clone());
            continue;
        }
        let Some(current) = volume_identity(path) else {
            logging::debug(
                "no volume identity readable; presence-only verification",
                serde_json::json!({ "dir": dir }),
            );
            continue;
        };
        match check_identity(data_root, dir, &current)? {
            IdentityCheck::FirstSight => logging::info(
                "source volume identity recorded",
                serde_json::json!({ "dir": dir, "identity": current }),
            ),
            IdentityCheck::Substituted { recorded } => {
                logging::warn(
                    "source volume SUBSTITUTED",
                    serde_json::json!({ "dir": dir, "recorded": recorded, "current": current }),
                );
                status.substituted.push(dir.clone());
            }
            IdentityCheck::Unchanged => {}
        }
    }

    // Identities for directories no longer configured are stale — prune.
    prune_identities(data_root, &settings.source_dirs)?;

    Ok(status)
}
