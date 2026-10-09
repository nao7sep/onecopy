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

use crate::{formats, logging, paths, storage};

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

/// The serial is read in one bounded call: the query touches the volume, and
/// an unanswered one is an unreadable identity.
#[cfg(windows)]
fn platform_identity(root: &Path) -> Option<String> {
    let owned = root.to_path_buf();
    crate::volume_io::call(root, crate::volume_io::Op::Stat, None, move || {
        Ok(windows_serial(&owned))
    })
    .ok()
    .flatten()
}

#[cfg(windows)]
fn windows_serial(root: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        // volume_io worker: a declaration; the one call runs in `platform_identity`'s bounded call
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
        // volume_io worker
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
    let document = match storage::read_json_file(&file, formats::SOURCE_VOLUMES) // data root
        .map_err(|error| format!("could not read {}: {error}", file.display()))?
    {
        storage::JsonFile::Absent => return Ok(BTreeMap::new()),
        storage::JsonFile::Document(document) => document,
        storage::JsonFile::Unreadable(reason) => {
            return Err(format!("could not read {}: {reason}", file.display()))
        }
        storage::JsonFile::Newer(newer) => return Err(newer.to_string()),
    };
    let store: SourceVolumeStore = serde_json::from_value(document)
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
    let document = serde_json::to_value(&store).map_err(|error| error.to_string())?;
    storage::write_json_file(
        &root.join(paths::SOURCE_VOLUMES_FILE_NAME),
        &document,
        formats::SOURCE_VOLUMES,
        true,
    )
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
/// left to a frontend check the caller might skip (R1-14): `Err` naming every
/// directory in `dirs` whose current volume identity differs from the one
/// recorded for it, or whose recorded identity cannot even be read. A read
/// failure keeps that directory's gate CLOSED rather than reading as "nothing
/// recorded" (R3-07) — the opposite of `verify_source_dirs`'s own frontend
/// status, which must still answer something for every directory even when
/// this fails. A directory that is absent, or that has no recorded identity
/// because its filesystem had none to read, is not this gate's concern and is
/// skipped. Callers scope `dirs` to the configured roots the work at hand
/// actually touches, so a source that failed verification blocks only work
/// on that source; every other configured root is unaffected (R3-07, R1-14).
/// A failure reading the whole record is the one case that closes the gate
/// for every directory passed in, because then nothing about any of them can
/// be verified.
pub fn enforce_no_substitution(data_root: &Path, dirs: &[String]) -> Result<(), String> {
    enforce_no_substitution_with(data_root, dirs, &platform_identity)
}

/// [`enforce_no_substitution`] with the identity probe passed in, so the
/// gate's scoping and verdicts are exercised without the platform's probe.
pub fn enforce_no_substitution_with(
    data_root: &Path,
    dirs: &[String],
    identity_of_volume_root: &dyn Fn(&Path) -> Option<String>,
) -> Result<(), String> {
    // The store lock covers only reading the record: the probes below touch
    // volumes that can stall, and no lock is held across them.
    let recorded = {
        let _guard = store_lock();
        load_unlocked(data_root)?
    };
    let mut affected = Vec::new();
    for dir in dirs {
        let path = Path::new(dir);
        let Some(known) = recorded.get(dir) else {
            continue;
        };
        // A check that could not answer is never verified-safe.
        match crate::volume_io::is_dir(path) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                affected.push(format!("{dir} (could not verify the volume: {error})"));
                continue;
            }
        }
        let root = match crate::trash::volume_root_of(path) {
            Ok(root) => root,
            Err(error) => {
                affected.push(format!("{dir} (could not verify the volume: {error})"));
                continue;
            }
        };
        // A filesystem without a stable identity is never recorded, so a
        // recorded directory whose identity cannot be read now is either a
        // different volume without one or a check that failed: neither is
        // verified-safe.
        let Some(current) = identity_of_volume_root(&root) else {
            affected.push(format!("{dir} (could not verify the volume)"));
            continue;
        };
        if current != known.identity {
            affected.push(format!("{dir} (a different volume now answers there)"));
        }
    }
    if affected.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "recheck source folders before continuing; affected: {}",
            affected.join(", ")
        ))
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
    /// Present folders whose drive has no identity OneCopy can read (some
    /// network shares): a different drive mounted there would go unnoticed.
    pub unidentified: Vec<String>,
}

/// A missing pathname alone cannot distinguish a deleted folder from an
/// absent drive. Compare the containing volume with the recorded identity;
/// every unanswered probe keeps the deliberately general wording.
pub fn unavailable_message_with(
    data_root: &Path, dir: &str, identity_of_path: &dyn Fn(&Path) -> Option<String>,
) -> &'static str {
    let path = Path::new(dir);
    if !matches!(crate::volume_io::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
        return "source.unavailable";
    }
    let recorded = {
        let _guard = store_lock();
        match load_unlocked(data_root) {
            Ok(records) => records.get(dir).cloned(),
            Err(_) => None,
        }
    };
    match (recorded, identity_of_path(path)) {
        (Some(recorded), Some(current)) if recorded.identity == current => "source.folderMissing",
        (Some(_), Some(_)) => "source.driveUnavailable",
        _ => "source.unavailable",
    }
}

// Presence AND identity verification over the configured source dirs: a dir
// that is not there is missing; a dir whose volume identity differs from the
// recorded one is substituted (the developer's backup drives share identical
// trees, so presence alone proves nothing). First sight records the identity
// — the "when the directory was added" moment as the core observes it. Rows
// for since-removed dirs are pruned; a volume without a readable identity
// degrades to presence-only, logged at debug.
pub fn verify_source_dirs(data_root: &Path) -> Result<SourceDirsStatus, String> {
    verify_source_dirs_with(data_root, &volume_identity)
}

/// Presence verification with a replaceable platform probe. A recorded identity
/// must remain verifiable before a returning source can leave the safety gate.
pub fn verify_source_dirs_with(
    data_root: &Path,
    identity_of_path: &dyn Fn(&Path) -> Option<String>,
) -> Result<SourceDirsStatus, String> {
    let config = storage::config(data_root)?;
    let settings = crate::scanner::settings_from_config(Some(&config), data_root, 0);
    let recorded = {
        let _guard = store_lock();
        load_unlocked(data_root)?
    };
    let mut status = SourceDirsStatus::default();
    for dir in &settings.source_dirs {
        let path = std::path::Path::new(dir);
        // A folder whose drive does not answer is as unavailable as a
        // missing one.
        if !crate::volume_io::is_dir(path).unwrap_or(false) {
            status.missing.push(dir.clone());
            continue;
        }
        let Some(current) = identity_of_path(path) else {
            if recorded.contains_key(dir) {
                return Err(format!("could not verify the recorded volume for {dir}"));
            }
            logging::debug(
                "no volume identity readable; presence-only verification",
                serde_json::json!({ "dir": dir }),
            );
            status.unidentified.push(dir.clone());
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
