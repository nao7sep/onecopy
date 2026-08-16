//! Config/state persistence under the storage root, plus the atomic-write choke
//! point every managed-text save funnels through.
//!
//! The data directory's managed files, each named in exactly one place (pinned
//! by the storage_file_names integration test):
//!
//! - `config.json`       — durable user settings.               RECORDED (managed text)
//! - `state.json`        — volatile UI/session state.           RECORDED (managed text)
//! - `index.sqlite3`     — the scan index (facts/cache).        not recorded (binary, reconstructible)
//! - `backups.sqlite3`   — the write-through backup store.      not recorded (the store itself)
//! - `logs/`             — per-session logs.                    not recorded (append-mode, by construction)
//! - `cache/`            — derived thumbnails/previews/strips.  not recorded (binary, reconstructible)
//! - `dependencies.json` — managed-binaries facts.              recorded via write_atomic (self-healing text; harmless in the store)
//! - `bin/`, `temp/`     — managed binaries + download staging. not recorded (binary; staging is wiped at launch)
//! - `trash/`            — the home-volume trash tree.          not recorded (the user's own moved files, never app text)
//!
//! Corrupt-config policy (storage-path conventions): a present-but-unreadable
//! JSON store is quarantined aside to `<stem>-<yyyymmdd-hhmmss-fff-utc>.invalid`
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
/// loaded config; each feature validates what it consumes (config-seeding
/// conventions).
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
    /// Embedding (cross-device) pairing: the one toggle and its cosine
    /// threshold as a percent (90 = 0.90).
    pub similarity_embedding_enabled: bool,
    pub similarity_embedding_threshold_percent: u32,
    /// Long edge of the screen-fit preview cache entries.
    pub preview_long_edge_px: u32,
    /// Edge of the grid thumbnail cache entries.
    pub thumbnail_edge_px: u32,
    pub video_strip_seconds_per_frame: u32,
    pub video_strip_min_frames: u32,
    pub video_strip_max_frames: u32,
    /// Scenes modal grid (columns × rows of strip frames).
    pub scenes_grid_columns: u32,
    pub scenes_grid_rows: u32,
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
    /// Cache directory override; null means `<root>/cache`.
    pub cache_dir: Option<String>,
    pub keep_awake_during_indexing: bool,
    /// Read-back verification of every copy/move-out against the indexed hash.
    pub verify_after_copy: bool,
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
            similarity_phash_max_distance: 4,
            similarity_embedding_enabled: true,
            similarity_embedding_threshold_percent: 90,
            preview_long_edge_px: 1600,
            thumbnail_edge_px: 320,
            video_strip_seconds_per_frame: 20,
            video_strip_min_frames: 5,
            // The ceiling must cover the scenes grid (default 6×4 = 24).
            video_strip_max_frames: 40,
            scenes_grid_columns: 6,
            scenes_grid_rows: 4,
            pairing_enabled: true,
            theme: "system".to_string(),
            ui_font_family:
                "system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, sans-serif"
                    .to_string(),
            check_updates_at_launch: false,
            cache_dir: None,
            keep_awake_during_indexing: true,
            verify_after_copy: true,
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
}

pub fn load_app_data(app: &AppHandle) -> Result<LoadedAppData, String> {
    let root = paths::data_root(app)?;
    let config = read_json_optional(&root.join(CONFIG_FILE_NAME))?;
    let state = read_json_optional(&root.join(STATE_FILE_NAME))?;
    Ok(LoadedAppData {
        config,
        state,
        data_root: root.to_string_lossy().into_owned(),
        debug_enabled: false,
    })
}

/// The configured source roots, read straight from `config.json` under a data
/// root. Used by the startup resume, which decides before any AppHandle-bound
/// load and needs only this one key.
pub fn load_config_source_dirs(data_root: &Path) -> Result<Vec<String>, String> {
    let config = read_json_optional(&data_root.join(CONFIG_FILE_NAME))?;
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
pub fn patch_config(app: &AppHandle, patch: &JsonValue) -> Result<JsonValue, String> {
    // records: config.json is durable user settings — managed text, recorded on
    // every save (data-backup conventions).
    let root = paths::data_root(app)?;
    patch_json_store(&root.join(CONFIG_FILE_NAME), patch)
}

/// Patch-merges into `state.json` (same one-owner contract as `patch_config`).
pub fn patch_state(app: &AppHandle, patch: &JsonValue) -> Result<JsonValue, String> {
    // records: state.json is volatile UI state, still managed text — recorded on
    // every save; the store's per-path content dedup absorbs the churn.
    let root = paths::data_root(app)?;
    patch_json_store(&root.join(STATE_FILE_NAME), patch)
}

/// Shallow merge: each top-level key in `patch` replaces the stored value
/// wholesale (arrays and objects included — `sourceDirs` is a list you set,
/// not a list you splice). Keys are never deleted; `null` stores as null,
/// which is a meaningful value here (`cacheDir: null` = the default).
pub fn patch_json_store(target: &Path, patch: &JsonValue) -> Result<JsonValue, String> {
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

    let mut current = read_json_optional(target)?.unwrap_or_else(|| serde_json::json!({}));
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
    Ok(current)
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

/// Reads an optional JSON store: missing → None; parseable → Some; corrupt →
/// quarantine aside (rename to `<stem>-<utc>.invalid`, preserving the original
/// bytes) then None so the caller proceeds with defaults. The quarantine rename
/// is OUTSIDE the parse-failure swallow: its own failure propagates.
fn read_json_optional(path: &Path) -> Result<Option<JsonValue>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    match serde_json::from_slice::<JsonValue>(&bytes) {
        Ok(value) => Ok(Some(value)),
        Err(parse_err) => {
            let quarantined = quarantine_name(path);
            std::fs::rename(path, &quarantined).map_err(|rename_err| {
                format!(
                    "could not quarantine corrupt {}: {rename_err} (parse error: {parse_err})",
                    path.display()
                )
            })?;
            logging::warn(
                "corrupt JSON store quarantined; recreating defaults",
                serde_json::json!({
                    "file": path.to_string_lossy(),
                    "quarantinedTo": quarantined.to_string_lossy(),
                    "error": { "message": parse_err.to_string() },
                }),
            );
            Ok(None)
        }
    }
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
    use std::io::Write;
    let parent = target
        .parent()
        .ok_or_else(|| "path has no parent directory".to_string())?;
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "path has no file name".to_string())?;
    let tmp = parent.join(atomic_temp_name(file_name));

    let write_tmp = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_tmp {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }

    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }

    // Best-effort: persist the rename itself by fsyncing the directory.
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }

    backup_store::record(target, bytes);

    Ok(())
}

/// The staging temp-file name an atomic write renames into place:
/// `<stem>-<nanoid>.tmp` (one final extension; the target's extension is
/// dropped, never dot-appended after).
fn atomic_temp_name(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name);
    format!("{}-{}.tmp", stem, nanoid::generate())
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
        assert!(read_json_optional(&path).unwrap().is_none());

        // Valid → Some.
        write_atomic(&path, b"{\"a\": 1}").unwrap();
        assert_eq!(
            read_json_optional(&path).unwrap(),
            Some(serde_json::json!({"a": 1}))
        );

        // Corrupt → quarantined aside (original bytes preserved) and None.
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(read_json_optional(&path).unwrap().is_none());
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
        let name = atomic_temp_name("config.json");
        assert!(name.starts_with("config-"), "{name:?}");
        assert!(name.ends_with(".tmp"), "{name:?}");
        let discriminator = &name["config-".len()..name.len() - ".tmp".len()];
        assert_eq!(discriminator.len(), 21);
        assert_ne!(atomic_temp_name("config.json"), atomic_temp_name("config.json"));
    }

}
