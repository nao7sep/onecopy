//! Config/state persistence under the storage root, plus the atomic-write choke
//! point every managed-text save funnels through.
//!
//! The data directory's managed files, each named in exactly one place (pinned
//! by the storage_file_names integration test):
//!
//! - `config.json`       — durable user settings.               RECORDED (managed text)
//! - `state.json`        — volatile UI/session state.           RECORDED (managed text)
//! - `index.sqlite3`     — the scan index (facts/cache).        not recorded (binary, reconstructible)
//! - `source-volumes.json` — destructive-operation trust baselines. RECORDED (managed safety text)
//! - `backups.sqlite3`   — the write-through backup store.      not recorded (the store itself)
//! - `logs/`             — per-session logs.                    not recorded (append-mode, by construction)
//! - `cache/`            — derived thumbnails/previews/strips.  not recorded (binary, reconstructible)
//! - `dependencies.json` — managed-binaries facts.              not recorded (re-derivable dependency/update facts)
//! - `similar-exclusions.json` — durable user verdicts.         RECORDED (managed authored text)
//! - `bin/`, `temp/`     — managed binaries + download staging. not recorded (binary; staging is wiped at launch; the version sidecar in `bin/` rides along, written via write_atomic_unrecorded)
//! - `trash/`            — the home-volume trash tree.          not recorded (the user's own moved files, never app text)
//!
//! Invalid-config policy (storage-path conventions): malformed JSON or a
//! non-object config envelope is quarantined aside to
//! `<stem>-<yyyymmdd-hhmmss-fff-utc>.invalid`
//! and defaults are recreated — never silently overwritten. The quarantine
//! rename runs OUTSIDE the parse-failure handling: a failed rename propagates as
//! an error instead of falling through to a default-reset that would clobber the
//! very bytes quarantine exists to preserve.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value as JsonValue;
use tauri::AppHandle;

use crate::{backup_store, logging, nanoid, paths};

pub const CONFIG_FILE_NAME: &str = "config.json";
pub const STATE_FILE_NAME: &str = "state.json";
pub const INDEX_DB_FILE_NAME: &str = "index.sqlite3";
pub const CACHE_DIR_NAME: &str = "cache";

/// Durable user settings — the single canonical defaults definition. Serialized
/// through the same save path the app uses (never a hand-written JSON literal)
/// to materialize `config.json` on first run. The store never validates a
/// loaded feature values; each feature validates what it consumes
/// (config-seeding conventions). The storage boundary validates only that the
/// config document itself has the required object envelope.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultConfig {
    /// IANA name applied when interpreting naive local timestamps (EXIF without
    /// an offset). Seeded from the system timezone; the wizard owns it after.
    pub default_timezone: String,
    /// Timestamps resolving before this year are rejected as implausible.
    pub good_range_start_year: i32,
    /// Burst-split gap (seconds) inside a visual cluster with camera data.
    pub similarity_max_gap_seconds: u32,
    /// Max perceptual-hash Hamming distance for two photos to cluster.
    pub similarity_phash_max_distance: u32,
    /// The relaxed distance for pairs within the burst gap (Phase 33): real
    /// bursts spread wider than the strict line tolerates, and capture time
    /// is the cheap, strong signal that they belong together. 10 is a
    /// REASONED default — it awaits tuning on the developer's real family
    /// index (the strict default stays icon-tuned at 3).
    pub similarity_phash_max_distance_burst: u32,
    /// How much WIDER than one pairing step a single family may spread, as a
    /// multiple of the pairing thresholds. 1 = every member must resemble the
    /// family's leader directly; 2 (the default) allows a burst whose ends
    /// meet only through their shared middle. Raising it invites chaining —
    /// the setting exists so a corpus that needs looser families can have
    /// them deliberately, never by accident.
    pub similarity_diameter_multiplier: u32,
    /// Long edge of the screen-fit preview cache entries.
    pub preview_long_edge_px: u32,
    /// Edge of the grid thumbnail cache entries.
    pub thumbnail_edge_px: u32,
    pub video_strip_seconds_per_frame: u32,
    pub video_strip_min_frames: u32,
    pub video_strip_max_frames: u32,
    /// Selection-follow playback and scene-click playback are separate user
    /// intents; both default on, but neither should force the other on.
    pub video_autoplay_on_show: bool,
    pub video_autoplay_after_snapshot: bool,
    /// The one global companion-pairing toggle (all kinds together).
    pub pairing_enabled: bool,
    /// UI theme: "system" (follow the OS), "light", or "dark".
    pub theme: String,
    /// UI font family: a free-text CSS family string, stored verbatim (the
    /// app-chrome conventions' family-only rule — CSS resolves the stack, and
    /// there is deliberately no size knob; zoom is the size remedy).
    pub ui_font_family: String,
    /// The managed-runtime-dependencies conventions' one update switch:
    /// check installed tools for updates at launch (throttled to ~daily).
    pub check_updates_at_launch: bool,
    /// Run the stat-only configured-source reconciliation after launch.
    pub check_source_folders_at_launch: bool,
    pub keep_awake_during_indexing: bool,
    /// Face scoring for comparison-group ordering: OPT-IN (Phase 33). Off
    /// means the models are not even downloaded by any automatic path, and
    /// the coordinator never runs the pass; ordering falls back to sharpness.
    pub score_faces: bool,
    /// Show an existing face score as a subtle thumbnail/comparison hint.
    /// This is presentation-only and never causes scoring or model downloads.
    pub show_face_stars: bool,
    /// Confirm ordinary Delete/Backspace trash-deletes in the grid. OFF by
    /// default (developer, 2026-08-17): the trash is the net, and a dialog on
    /// every Delete would break the keystroke-paced cull — but a deliberate
    /// user can opt into the extra stop. Permanent deletion always confirms
    /// regardless; that rule is not configurable.
    pub confirm_trash_delete: bool,
    /// Source directories to scan (wizard-configured; absolute paths).
    pub source_dirs: Vec<String>,
    /// Destination roots for the move/copy-out tree (absolute paths).
    pub destination_roots: Vec<String>,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        DefaultConfig {
            default_timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
            good_range_start_year: 1995,
            similarity_max_gap_seconds: 90,
            // Deliberately tight: on a measured 548-image corpus, 12 collapsed
            // everything into one 484-member hairball; 2-4 recovered families.
            similarity_phash_max_distance: 3,
            similarity_phash_max_distance_burst: 10,
            similarity_diameter_multiplier: 2,
            preview_long_edge_px: 1600,
            thumbnail_edge_px: 320,
            video_strip_seconds_per_frame: 20,
            video_strip_min_frames: 5,
            video_strip_max_frames: 40,
            video_autoplay_on_show: true,
            video_autoplay_after_snapshot: true,
            pairing_enabled: true,
            theme: "system".to_string(),
            // Blank means the stylesheet's explicit system stack. Persist
            // only a real user override here, never CSS implementation detail.
            ui_font_family: String::new(),
            check_updates_at_launch: false,
            check_source_folders_at_launch: true,
            keep_awake_during_indexing: true,
            score_faces: false,
            show_face_stars: true,
            confirm_trash_delete: false,
            source_dirs: Vec::new(),
            destination_roots: Vec::new(),
        }
    }
}

/// Everything the frontend needs at startup, in one command round-trip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedAppData {
    pub config: Option<JsonValue>,
    pub state: Option<JsonValue>,
    pub data_root: String,
    /// Set by the command layer from logging::debug_enabled(); storage leaves it false.
    pub debug_enabled: bool,
    /// Stores quarantined during this launch, for the frontend to REPORT. An
    /// unreported quarantine is a silent reset with extra steps
    /// (storage-path-conventions), so a log line alone is not enough.
    pub quarantines: Vec<QuarantineRecord>,
}

/// One quarantined store: which file, and where its original bytes now live.
/// What the app started with instead is phrased at the reporting edge, which
/// is the layer that knows how to say it.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineRecord {
    /// The store's file name — `config.json`, `state.json`.
    pub file: String,
    /// The `.invalid` path holding the original bytes, verbatim.
    pub quarantined_to: String,
}

/// Every read reports its own quarantine outcome in its result. The one
/// exception needing a buffer: a pre-window setup read can set a store aside
/// before any webview exists to report to, so `read_config_for_setup` parks
/// the record here and the frontend's `load_from_root` picks it up. Nothing
/// else feeds or drains this.
static PENDING_QUARANTINES: std::sync::Mutex<Vec<QuarantineRecord>> =
    std::sync::Mutex::new(Vec::new());

fn take_pending_quarantines() -> Vec<QuarantineRecord> {
    let mut pending = PENDING_QUARANTINES.lock().unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut *pending)
}

pub fn load_app_data(app: &AppHandle) -> Result<LoadedAppData, String> {
    load_from_root(&paths::data_root(app)?)
}

/// Reads config for the pre-window setup paths. A quarantine here happens
/// before any reporting surface exists, so its record is parked for the
/// frontend's `load_from_root` to publish.
pub fn read_config_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_config_optional(&root.join(CONFIG_FILE_NAME))?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
        materialize_config_if_missing(root)?;
        return Ok(read_config_optional(&root.join(CONFIG_FILE_NAME))?.value);
    }
    Ok(read.value)
}

pub fn load_from_root(root: &Path) -> Result<LoadedAppData, String> {
    let mut quarantines = take_pending_quarantines();
    let config_read = read_config_optional(&root.join(CONFIG_FILE_NAME))?;
    let state_read = read_json_optional(&root.join(STATE_FILE_NAME))?;
    let mut config = config_read.value;
    if let Some(record) = config_read.quarantined {
        quarantines.push(record);
        materialize_config_if_missing(root)?;
        config = read_config_optional(&root.join(CONFIG_FILE_NAME))?.value;
    }
    if let Some(record) = state_read.quarantined {
        quarantines.push(record);
    }
    Ok(LoadedAppData {
        config,
        state: state_read.value,
        data_root: root.to_string_lossy().into_owned(),
        debug_enabled: false,
        quarantines,
    })
}

/// The configured source roots, read straight from `config.json` under a data
/// root. Used by the startup resume, which decides before any AppHandle-bound
/// load and needs only this one key.
pub fn load_config_source_dirs(data_root: &Path) -> Result<Vec<String>, String> {
    let config = read_config_for_setup(data_root)?;
    Ok(config
        .as_ref()
        .and_then(|c| c.get("sourceDirs"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| e.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default())
}

/// Patch-merges into `config.json` and returns the merged document. The core
/// holds the file, so it is the one owner of the read-modify-write — the
/// frontend sends only the keys it changes, and a stale cached copy in one
/// store can never blind-overwrite another store's save (the lost-update the
/// persisted-store-separation conventions' one-owner rule exists to prevent).
pub fn patch_config(app: &AppHandle, patch: &JsonValue) -> Result<PatchOutcome, String> {
    // records: config.json is durable user settings — managed text, recorded on
    // every save (data-backup conventions).
    let root = paths::data_root(app)?;
    patch_json_store(&root.join(CONFIG_FILE_NAME), patch)
}

/// Patch-merges into `state.json` (same one-owner contract as `patch_config`).
pub fn patch_state(app: &AppHandle, patch: &JsonValue) -> Result<PatchOutcome, String> {
    // records: state.json is volatile UI state, still managed text — recorded on
    // every save; the store's per-path content dedup absorbs the churn.
    let root = paths::data_root(app)?;
    patch_json_store(&root.join(STATE_FILE_NAME), patch)
}

/// A patch's merged document plus the quarantine this read-modify-write
/// performed, if any — a mid-session quarantine has no load result to ride
/// home on, so the command layer publishes it from here.
pub struct PatchOutcome {
    pub merged: JsonValue,
    pub quarantined: Option<QuarantineRecord>,
}

/// Shallow merge: each top-level key in `patch` replaces the stored value
/// wholesale (arrays and objects included — `sourceDirs` is a list you set,
/// not a list you splice). Keys are never deleted; `null` stores as null.
pub fn patch_json_store(target: &Path, patch: &JsonValue) -> Result<PatchOutcome, String> {
    // Serialized: this is a read-modify-write, and both `patch_config` and
    // `patch_state` are Tauri commands dispatched on a thread pool, so two
    // surfaces saving at once could otherwise interleave their reads and the
    // second write would drop the first's keys — the exact lost update the
    // one-owner rule exists to prevent. One global lock is enough: patches are
    // small and rare, and holding it across the atomic write is what makes the
    // whole read-merge-write atomic with respect to other patchers.
    static PATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = PATCH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let read = if target.file_name().is_some_and(|name| name == CONFIG_FILE_NAME) {
        read_config_optional(target)?
    } else {
        read_json_optional(target)?
    };
    let quarantined = read.quarantined;
    let mut current = read.value;
    if quarantined.is_some() && target.file_name().is_some_and(|name| name == CONFIG_FILE_NAME) {
        if let Some(root) = target.parent() {
            materialize_config_if_missing(root)?;
        }
        current = read_config_optional(target)?.value;
    }
    let mut current = current.unwrap_or_else(|| serde_json::json!({}));
    if !current.is_object() {
        current = serde_json::json!({});
    }
    let (Some(doc), Some(fields)) = (current.as_object_mut(), patch.as_object()) else {
        return Err("patch must be a JSON object".to_string());
    };
    for (key, value) in fields {
        doc.insert(key.clone(), value.clone());
    }
    atomic_write_json(target, &current)?;
    Ok(PatchOutcome {
        merged: current,
        quarantined,
    })
}

/// First-run materialization (storage-path conventions): write `config.json`
/// from the canonical defaults, through the app's own save path, ONLY when the
/// file is absent. An existing file — even a corrupt one — is never touched
/// here; the load path owns the quarantine decision. `state.json` is deliberately
/// not materialized (volatile, written only when there is state to record).
pub fn materialize_config_if_missing(root: &Path) -> Result<(), String> {
    let target = root.join(CONFIG_FILE_NAME);
    if target.exists() {
        return Ok(());
    }
    let defaults =
        serde_json::to_value(DefaultConfig::default()).map_err(|e| e.to_string())?;
    atomic_write_json(&target, &defaults)
}

struct JsonRead {
    value: Option<JsonValue>,
    /// Set when this read set the store aside; the caller owns getting the
    /// record to a reporting surface.
    quarantined: Option<QuarantineRecord>,
}

/// Reads an optional JSON store; invalid content is quarantined aside and the
/// record returned. Config additionally requires an object root, which is an
/// envelope invariant rather than feature-level value validation. The rename
/// failure propagates before any caller can write defaults.
fn read_json_optional(path: &Path) -> Result<JsonRead, String> {
    read_json_optional_with_envelope(path, false)
}

fn read_config_optional(path: &Path) -> Result<JsonRead, String> {
    let mut read = read_json_optional_with_envelope(path, true)?;
    if let Some(value) = read.value.as_mut() {
        let removed = value
            .as_object_mut()
            .is_some_and(|fields| fields.remove("verifyAfterCopy").is_some());
        if removed {
            atomic_write_json(path, value)?;
        }
    }
    Ok(read)
}

fn read_json_optional_with_envelope(
    path: &Path,
    require_object_root: bool,
) -> Result<JsonRead, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JsonRead {
                value: None,
                quarantined: None,
            });
        }
        Err(err) => return Err(err.to_string()),
    };
    match serde_json::from_slice::<JsonValue>(&bytes) {
        Ok(value) if !require_object_root || value.is_object() => Ok(JsonRead {
            value: Some(value),
            quarantined: None,
        }),
        Ok(_) => quarantine_invalid_store(path, "config root must be a JSON object"),
        Err(parse_error) => quarantine_invalid_store(path, &format!("parse error: {parse_error}")),
    }
}

fn quarantine_invalid_store(path: &Path, reason: &str) -> Result<JsonRead, String> {
    let quarantined = quarantine_name(path);
    // not recorded: an invalid quarantine preserves the original raw bytes
    // rather than creating new managed user text.
    std::fs::rename(path, &quarantined).map_err(|rename_error| {
        format!(
            "could not quarantine invalid {}: {rename_error} ({reason})",
            path.display()
        )
    })?;
    logging::warn(
        "invalid JSON store quarantined; recreating defaults",
        serde_json::json!({
            "file": path.to_string_lossy(),
            "quarantinedTo": quarantined.to_string_lossy(),
            "error": { "message": reason },
        }),
    );
    Ok(JsonRead {
        value: None,
        quarantined: Some(QuarantineRecord {
            file: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            quarantined_to: quarantined.to_string_lossy().into_owned(),
        }),
    })
}

/// `<stem>-<yyyymmdd-hhmmss-fff-utc>.invalid`, sibling to the target — the
/// derived-filename grammar with a moment discriminator.
fn quarantine_name(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("store");
    path.with_file_name(format!("{stem}-{}.invalid", logging::filename_stamp_now()))
}

/// Serializes through serde (never a hand-written literal) and writes atomically.
fn atomic_write_json(target: &Path, value: &JsonValue) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    write_atomic(target, text.as_bytes())
}

/// Atomic write: write to a `<stem>-<nanoid>.tmp` sibling, fsync it, rename over
/// the target, fsync the directory — a crash can never leave a half-written
/// store. Strictly AFTER the rename lands, the exact bytes are recorded into the
/// write-through backup store (the one managed-text choke point).
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_inner(target, bytes, true)
}

/// The same atomic write, WITHOUT the backup record. For text that is excluded
/// from the history by a design-time, per-write-site decision: today the version
/// sidecar in the binary-bearing `bin/`, which describes the re-fetchable binary
/// it sits beside and is rewritten by the next install (see the table above).
pub fn write_atomic_unrecorded(target: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_inner(target, bytes, false)
}

fn write_atomic_inner(target: &Path, bytes: &[u8], record: bool) -> Result<(), String> {
    use std::io::Write;
    let parent = target
        .parent()
        .ok_or_else(|| "path has no parent directory".to_string())?;
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "path has no file name".to_string())?;
    let tmp = parent.join(atomic_temp_name(file_name)?);

    let write_tmp = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_tmp {
        crate::fs_recovery::remove_file(&tmp, "atomic store write cleanup");
        return Err(e.to_string());
    }

    if let Err(e) = std::fs::rename(&tmp, target) {
        crate::fs_recovery::remove_file(&tmp, "atomic store publication cleanup");
        return Err(e.to_string());
    }

    // Best-effort: persist the rename itself by fsyncing the directory.
    if let Ok(dir) = std::fs::File::open(parent) {
        crate::fs_recovery::sync_all(&dir, parent, "atomic store directory sync");
    }

    if record {
        backup_store::record(target, bytes);
    }

    Ok(())
}

/// The staging temp-file name an atomic write renames into place:
/// `<stem>-<nanoid>.tmp` (one final extension; the target's extension is
/// dropped, never dot-appended after).
fn atomic_temp_name(file_name: &str) -> Result<String, String> {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    Ok(format!("{}-{}.tmp", stem, nanoid::generate()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // EXCEPTION to the tests-live-in-tests/ rule (tests-folder
    // conventions, Rust form): the quarantine/temp-name grammar and the
    // optional-read policy are private internals of this store —
    // promoting them would widen the surface just to test through it.

    use serial_test::serial;

    fn temp_dir(label: &str) -> PathBuf {
        tempfile::Builder::new()
            .prefix(&format!("onecopy-storage-{label}-"))
            .tempdir()
            .unwrap()
            .keep()
    }

    #[test]
    fn quarantine_name_follows_the_derived_grammar() {
        let q = quarantine_name(Path::new("/data/config.json"));
        let name = q.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("config-"), "{name}");
        assert!(name.ends_with("-utc.invalid"), "{name}");
        assert!(!name.contains(".json"), "role extension replaces the original: {name}");
    }

    #[test]
    #[serial(backup_store)]
    fn read_json_optional_missing_valid_and_corrupt() {
        let dir = temp_dir("read-optional");
        let path = dir.join("config.json");

        // Missing → None.
        let missing = read_json_optional(&path).unwrap();
        assert!(missing.value.is_none());
        assert!(missing.quarantined.is_none());

        // Valid → Some.
        write_atomic(&path, b"{\"a\": 1}").unwrap();
        assert_eq!(
            read_json_optional(&path).unwrap().value,
            Some(serde_json::json!({"a": 1}))
        );

        // Corrupt → quarantined aside (original bytes preserved) and None.
        std::fs::write(&path, b"{ not json").unwrap();
        let corrupt = read_json_optional(&path).unwrap();
        assert!(corrupt.value.is_none());
        assert!(corrupt.quarantined.is_some());
        assert!(!path.exists(), "corrupt file must be renamed aside, not left in place");
        let quarantined: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".invalid"))
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(
            std::fs::read(quarantined[0].path()).unwrap(),
            b"{ not json",
            "quarantine preserves the original bytes"
        );
    }

    #[test]
    fn atomic_temp_name_is_stem_plus_nanoid_dot_tmp() {
        let name = atomic_temp_name("config.json").unwrap();
        assert!(name.starts_with("config-"), "{name:?}");
        assert!(name.ends_with(".tmp"), "{name:?}");
        let discriminator = &name["config-".len()..name.len() - ".tmp".len()];
        assert_eq!(discriminator.len(), 21);
        assert_ne!(
            atomic_temp_name("config.json").unwrap(),
            atomic_temp_name("config.json").unwrap()
        );
    }

}
