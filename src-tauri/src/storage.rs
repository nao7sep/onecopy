//! Config/state persistence under the storage root, plus the atomic-write choke
//! point every managed-text save funnels through.
//!
//! The data directory's managed files, each named in exactly one place (pinned
//! by the storage_file_names integration test):
//!
//! - `config.json`       — durable user settings.               RECORDED (managed text)
//! - `state.json`        — volatile UI/session state.           not recorded (volatile state and nothing else; written via the unrecorded atomic path)
//! - `window.json`       — volatile Main placement state.       not recorded (volatile state; unrecorded atomic path)
//! - `preview-window.json` — volatile Preview placement state.  not recorded (volatile state; unrecorded atomic path)
//! - `index.sqlite3`     — scan facts, caches, diagnostics.    archived (binary store)
//! - `records.sqlite3`   — what happened, logs included.        not archived (records)
//! - `source-volumes.json` — destructive-operation trust baselines. RECORDED (managed safety text)
//! - `backups.sqlite3`   — the write-through backup store.      not recorded (the store itself)
//! - `logs/`             — log lines the records could not take. not recorded (append-mode, by construction)
//! - `cache/`            — derived thumbnails/previews/strips.  not recorded (binary, reconstructible)
//! - `dependencies.json` — managed-binaries facts, plus the two check-attempt timestamps. not recorded (re-derivable dependency/update facts)
//! - `bin/`, `temp/`     — managed binaries + download staging. not recorded (binary; staging is wiped at launch; the version sidecar in `bin/` rides along, written via write_atomic_unrecorded)
//! - `installation-id`   — this data root's random identity fact, used only to
//!                          fingerprint private staging/claim names so two
//!                          application homes never sweep each other's leftovers
//!                          (`file_identity.rs`). not recorded (re-derivable on
//!                          loss; regenerated if missing; written via
//!                          write_atomic_unrecorded)
//! Recoverable deleted files live below their configured roots, outside this
//! application-data directory.
//!
//! Invalid-config policy (storage-path conventions): malformed JSON or a
//! non-object config envelope is quarantined aside to
//! `<stem>-<yyyymmdd-hhmmss-fff-utc>.invalid`
//! and built-ins are read without creating a replacement. The quarantine
//! rename runs OUTSIDE the parse-failure handling: a failed rename propagates as
//! an error instead of falling through to a default-reset that would clobber the
//! very bytes quarantine exists to preserve.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::{backup_store, logging, nanoid, paths};

pub const CONFIG_FILE_NAME: &str = "config.json";
pub const STATE_FILE_NAME: &str = "state.json";
pub const WINDOW_FILE_NAME: &str = "window.json";
pub const PREVIEW_WINDOW_FILE_NAME: &str = "preview-window.json";
pub const INDEX_DB_FILE_NAME: &str = "index.sqlite3";
pub const CACHE_DIR_NAME: &str = "cache";

/// SQLite stores protected by whole-file archives; temp scratch is excluded.
pub const ARCHIVED_STORES: [(&str, &str); 1] = [(INDEX_DB_FILE_NAME, INDEX_DB_FILE_NAME)];

/// Canonical built-in config sets. Built-ins are read in memory, never seeded.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultConfig {
    pub ignored_file_names: Vec<String>,
    pub hide_dot_names: bool,
    pub hide_hidden_attributes: bool,
    pub hide_system_attributes: bool,
    /// IANA name applied when interpreting naive local timestamps (EXIF without
    /// an offset). The built-in follows the system timezone; the wizard saves the choice.
    pub default_timezone: String,
    /// Timestamps resolving before this year are rejected as implausible.
    pub good_range_start_year: i32,
    pub similarity: SimilaritySettings,
    /// Long edge of the screen-fit preview cache entries.
    pub preview_long_edge_px: u32,
    /// Edge of the grid thumbnail cache entries.
    pub thumbnail_edge_px: u32,
    pub video_strip_seconds_per_frame: u32,
    pub video_strip_min_frames: u32,
    pub video_strip_max_frames: u32,
    pub video_snapshots_enabled: bool,
    pub similar_photo_analysis_enabled: bool,
    pub video_transcription_enabled: bool,
    pub audio_transcription_enabled: bool,
    /// Runtime-selected backend per AI engine. The acceleration owner
    /// validates known keys and platform availability before publication.
    pub ai_acceleration: JsonValue,
    pub video_autoplay: bool,
    pub audio_autoplay: bool,
    pub enlarge_small_images_in_preview: bool,
    pub enlarge_small_images_in_quick_view: bool,
    pub text_preview_max_bytes: u64,
    pub text_fallback_encoding: String,
    /// The one global companion-pairing toggle (all kinds together).
    pub pairing_enabled: bool,
    /// UI theme: "system" (follow the OS), "light", or "dark".
    pub theme: String,
    /// Interface language: "system" (follow the computer at every launch) or a
    /// supported tag such as "ja" (see i18n::LANGUAGES). An unrecognized value
    /// means "system", the same rule the frontend applies.
    pub language: String,
    /// UI font family: a free-text CSS family string, stored verbatim (the
    /// app-chrome conventions' family-only rule — CSS resolves the stack, and
    /// there is deliberately no size knob; zoom is the size remedy).
    pub ui_font_family: String,
    /// The managed-runtime-dependencies conventions' one update switch:
    /// check installed tools for updates at launch (throttled to ~daily).
    pub check_updates_at_launch: bool,
    /// Check OneCopy's own public GitHub releases after launch (throttled by
    /// its distinct app-release attempt timestamp).
    pub check_github_releases_at_launch: bool,
    /// Run the stat-only configured-source reconciliation after launch.
    pub check_source_folders_at_launch: bool,
    pub keep_awake_during_indexing: bool,
    /// Whether playback is audible. Edited in Settings and from the status bar.
    pub sound_enabled: bool,
    /// Playback volume, 0.01 to 1.
    pub playback_volume: f64,
    /// Face scoring for comparison-group ordering. Off means the coordinator
    /// does not run the optional pass; ordering falls back to sharpness.
    pub score_faces: bool,
    /// Show an existing face score as a subtle thumbnail/comparison hint.
    /// This is presentation-only and never causes scoring or model downloads.
    pub show_face_stars: bool,
    /// Upper bound for one visible Comparison page. Connected displays and
    /// the current images' orientation may reduce the actual page size.
    pub maximum_images_in_comparison: u32,
    /// How long a safely missable notification remains visible. Persistent
    /// notifications ignore this setting.
    pub notification_display_seconds: u32,
    /// Confirm a direct single-item Delete/Backspace Trash command. New users
    /// start with this safeguard on and may opt out for keystroke-paced culling.
    /// Indirect, bulk, and permanent consequences always review regardless;
    /// those rules are not configurable.
    pub confirm_trash_delete: bool,
    /// Auxiliary display order as monitor keys, first preferred; an unlisted
    /// display follows in system order. Saved the moment it is reordered.
    pub screen_priority: Vec<String>,
    /// Source directories to scan (wizard-configured; absolute paths).
    pub source_dirs: Vec<String>,
    /// Destination roots for the move/copy-out tree (absolute paths).
    pub destination_roots: Vec<String>,
    /// Conflict renaming follows one familiar desktop style. The built-in follows the platform; users may choose the other.
    pub destination_conflict_rename_style: String,
}

impl Default for DefaultConfig {
    fn default() -> Self {
        DefaultConfig {
            ignored_file_names: vec![".DS_Store".into(), "Thumbs.db".into(), "desktop.ini".into()],
            hide_dot_names: true,
            hide_hidden_attributes: true,
            hide_system_attributes: true,
            default_timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
            good_range_start_year: 1995,
            similarity: SimilaritySettings::default(),
            preview_long_edge_px: 1600,
            thumbnail_edge_px: 320,
            video_strip_seconds_per_frame: 20,
            video_strip_min_frames: 5,
            video_strip_max_frames: 40,
            video_snapshots_enabled: true,
            similar_photo_analysis_enabled: true,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
            ai_acceleration: crate::ai_acceleration::default_config(),
            video_autoplay: true,
            audio_autoplay: true,
            enlarge_small_images_in_preview: true,
            enlarge_small_images_in_quick_view: true,
            text_preview_max_bytes: crate::text_preview::DEFAULT_MAX_BYTES,
            text_fallback_encoding: crate::text_preview::DEFAULT_FALLBACK_ENCODING.to_string(),
            pairing_enabled: true,
            theme: "system".to_string(),
            language: "system".to_string(),
            // Blank means the stylesheet's explicit system stack. Persist
            // only a real user override here, never CSS implementation detail.
            ui_font_family: String::new(),
            check_updates_at_launch: false,
            check_github_releases_at_launch: true,
            check_source_folders_at_launch: true,
            keep_awake_during_indexing: true,
            sound_enabled: true,
            playback_volume: 1.0,
            score_faces: true,
            show_face_stars: true,
            maximum_images_in_comparison: 16,
            notification_display_seconds: 6,
            confirm_trash_delete: true,
            screen_priority: Vec::new(),
            source_dirs: Vec::new(),
            destination_roots: Vec::new(),
            destination_conflict_rename_style: if cfg!(target_os = "windows") {
                "parenthesized-number".to_string()
            } else {
                "space-number".to_string()
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimilaritySettings {
    pub max_gap_seconds: u32,
    pub phash_max_distance: u32,
    pub phash_max_distance_burst: u32,
    pub diameter_multiplier: u32,
}

impl Default for SimilaritySettings {
    fn default() -> Self {
        Self { max_gap_seconds: 90, phash_max_distance: 3, phash_max_distance_burst: 10, diameter_multiplier: 2 }
    }
}

/// Known set keys and built-ins have one owner: `DefaultConfig`. The one
/// check on read (config-sets conventions' reading and healing).
pub fn effective_config(stored: Option<&JsonValue>) -> JsonValue {
    let mut effective = serde_json::to_value(DefaultConfig::default()).expect("the default config serializes");
    for (key, builtin) in effective.as_object_mut().expect("defaults are an object") {
        if let Some(value) = stored.and_then(|document| document.get(key)) {
            if valid_set(key, value, builtin) {
                *builtin = value.clone();
            } else {
                logging::warn(
                    "invalid config set; using built-in",
                    serde_json::json!({ "key": key, "value": value }),
                );
            }
        }
    }
    effective
}

fn valid_set(key: &str, value: &JsonValue, builtin: &JsonValue) -> bool {
    let shape = match builtin {
        JsonValue::Bool(_) => value.is_boolean(),
        JsonValue::Number(_) => value.is_number(),
        JsonValue::String(_) => value.is_string(),
        JsonValue::Array(_) => value.as_array().is_some_and(|items| items.iter().all(JsonValue::is_string)),
        JsonValue::Object(fields) => value.as_object().is_some_and(|copy| fields.iter().all(|(key, builtin)| copy.get(key).is_some_and(|value| valid_set(key, value, builtin)))),
        _ => false,
    };
    shape && match key {
        "theme" => matches!(value.as_str(), Some("system" | "light" | "dark")),
        "language" => value.as_str().is_some_and(|tag| tag == "system" || crate::i18n::LANGUAGES.contains(&tag)),
        "destinationConflictRenameStyle" => matches!(value.as_str(), Some("space-number" | "parenthesized-number")),
        "transcription" | "face-scoring" => matches!(value.as_str(), Some("none" | "metal")),
        _ => true,
    }
}

/// Everything the frontend needs at startup, in one command round-trip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedAppData {
    /// The effective configuration (see [`effective_config`]).
    pub config: JsonValue,
    /// The defaults a new installation starts with, for Settings' reset
    /// actions; the frontend keeps no default table of its own.
    pub config_defaults: JsonValue,
    pub state: Option<JsonValue>,
    /// The check-attempt timestamps kept in `dependencies.json`
    /// (see `binaries_manager::load_check_attempts`).
    pub check_attempts: JsonValue,
    pub data_root: String,
    /// Set by the command layer from logging::debug_enabled(); storage leaves it false.
    pub debug_enabled: bool,
    /// Platform and packaged-binary capability facts for Settings. These are
    /// independent of config validity so a bad saved choice remains repairable.
    pub ai_acceleration_capabilities: Vec<crate::ai_acceleration::Capability>,
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
    let mut pending = PENDING_QUARANTINES
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    std::mem::take(&mut *pending)
}

pub fn load_app_data() -> Result<LoadedAppData, String> {
    load_from_root(&paths::data_root()?)
}

/// Preferences needed by auxiliary windows are a read-only projection, not
/// another application bootstrap. It never materializes, repairs, or drains
/// quarantine notices owned by Main.
pub fn read_appearance_preferences(root: &Path) -> Result<JsonValue, String> {
    let config: JsonValue = match std::fs::read(root.join(CONFIG_FILE_NAME)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.to_string()),
    };
    if !config.is_object() {
        return Err("Appearance requires a configuration object".to_string());
    }
    let config = effective_config(Some(&config));
    Ok(serde_json::json!({
        "uiFontFamily": config.get("uiFontFamily"),
        "enlargeSmallImagesInPreview": config.get("enlargeSmallImagesInPreview"),
        "enlargeSmallImagesInQuickView": config.get("enlargeSmallImagesInQuickView"),
        "videoTranscriptionEnabled": config.get("videoTranscriptionEnabled"),
        "audioTranscriptionEnabled": config.get("audioTranscriptionEnabled"),
    }))
}

/// Projects `LanguageState`'s CURRENT values into an appearance-preferences
/// document, the way the `appearance_preferences` command does after calling
/// `read_appearance_preferences` above. Kept apart from that command's
/// `app.state::<i18n::LanguageState>()` lookup so the actual contract —
/// a language a settings save just set is what the very next read returns —
/// is directly testable against a `LanguageState` a test constructs itself,
/// with no `AppHandle` involved (R5.5 C3).
pub fn with_language_fields(mut preferences: JsonValue, state: &crate::i18n::LanguageState) -> JsonValue {
    if let Some(object) = preferences.as_object_mut() {
        object.insert("language".into(), serde_json::json!(state.current()));
        object.insert("systemLanguage".into(), serde_json::json!(state.system_language));
        object.insert("systemLocale".into(), serde_json::json!(state.system_locale));
    }
    preferences
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
    }
    Ok(read.value)
}

/// Reads volatile state for a pre-frontend runtime decision while preserving
/// the ordinary load path's duty to report any quarantine to Main.
pub fn read_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(STATE_FILE_NAME))?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn read_window_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(WINDOW_FILE_NAME))?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn save_window_state(root: &Path, state: &JsonValue) -> Result<(), String> {
    atomic_write_json(&root.join(WINDOW_FILE_NAME), state, false)
}

pub fn read_preview_window_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(PREVIEW_WINDOW_FILE_NAME))?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn save_preview_window_state(root: &Path, state: &JsonValue) -> Result<(), String> {
    atomic_write_json(&root.join(PREVIEW_WINDOW_FILE_NAME), state, false)
}

pub fn load_from_root(root: &Path) -> Result<LoadedAppData, String> {
    let mut quarantines = take_pending_quarantines();
    let config_read = read_config_optional(&root.join(CONFIG_FILE_NAME))?;
    let state_read = read_json_optional(&root.join(STATE_FILE_NAME))?;
    let config = config_read.value;
    if let Some(record) = config_read.quarantined {
        quarantines.push(record);
    }
    if let Some(record) = state_read.quarantined {
        quarantines.push(record);
    }
    let config = config.unwrap_or_else(|| effective_config(None));
    hold_config(root, &config);
    Ok(LoadedAppData {
        config,
        config_defaults: effective_config(None),
        state: state_read.value,
        check_attempts: crate::binaries_manager::load_check_attempts(root),
        data_root: root.to_string_lossy().into_owned(),
        debug_enabled: false,
        ai_acceleration_capabilities: crate::ai_acceleration::capabilities(None)
            .expect("built-in acceleration features are valid"),
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

/// All roots whose own permission boundary must contain recoverable deleted
/// files. The returned set is an operation-planning snapshot.
pub fn load_config_file_roots(data_root: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(load_configured_roots(data_root)?.all())
}

/// The configured source and destination roots, each in configured order.
#[derive(Clone, Debug, Default)]
pub struct ConfiguredRoots {
    pub sources: Vec<PathBuf>,
    pub destinations: Vec<PathBuf>,
}

impl ConfiguredRoots {
    /// Every distinct configured root, sources first.
    pub fn all(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for root in self.sources.iter().chain(&self.destinations) {
            if !roots.contains(root) {
                roots.push(root.clone());
            }
        }
        roots
    }
}

pub fn load_configured_roots(data_root: &Path) -> Result<ConfiguredRoots, String> {
    Ok(configured_roots_in(read_config_for_setup(data_root)?.as_ref()))
}

fn configured_roots_in(config: Option<&JsonValue>) -> ConfiguredRoots {
    let list = |key: &str| {
        config
            .and_then(|document| document.get(key))
            .and_then(JsonValue::as_array)
            .into_iter()
            .flatten()
            .filter_map(JsonValue::as_str)
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    };
    ConfiguredRoots {
        sources: list("sourceDirs"),
        destinations: list("destinationRoots"),
    }
}

/// Every read and write of `config.json` passes its roots to `volume_io`, so
/// each configured root fails fast as its own drive before any call on it.
fn register_volume_roots(config: Option<&JsonValue>) {
    crate::volume_io::register_roots(&configured_roots_in(config).all());
}

/// The settings this process holds for one storage root: every set's
/// effective value, read once and changed only by saves (config-sets
/// conventions). Keyed by root so a different root is read afresh.
static HELD_CONFIG: std::sync::Mutex<Option<(PathBuf, JsonValue)>> = std::sync::Mutex::new(None);

fn hold_config(root: &Path, effective: &JsonValue) {
    *HELD_CONFIG.lock().unwrap_or_else(|p| p.into_inner()) = Some((root.to_path_buf(), effective.clone()));
}

/// A save's resulting settings plus the quarantine its first read performed,
/// if any — a mid-session quarantine has no load result to ride home on, so
/// the command layer publishes it from here.
pub struct SaveOutcome {
    pub effective: JsonValue,
    pub quarantined: Option<QuarantineRecord>,
}

/// Saves settings: each changed set replaces the held set whole, then the
/// file is written from what the app holds, every set that differs from its
/// built-in and nothing else (config-sets conventions).
pub fn save_config(root: &Path, changes: &JsonValue) -> Result<SaveOutcome, String> {
    // records: config.json is durable user settings — managed text, recorded on
    // every save (data-backup conventions).
    let fields = changes
        .as_object()
        .ok_or_else(|| "settings changes must be a JSON object".to_string())?;
    let mut held = HELD_CONFIG.lock().unwrap_or_else(|p| p.into_inner());
    let mut quarantined = None;
    let mut effective = match held.as_ref() {
        Some((held_root, effective)) if held_root == root => effective.clone(),
        _ => {
            let read = read_config_optional(&root.join(CONFIG_FILE_NAME))?;
            quarantined = read.quarantined;
            read.value.unwrap_or_else(|| effective_config(None))
        }
    };
    let builtins = effective_config(None);
    for (key, value) in fields {
        let Some(builtin) = builtins.get(key) else { continue };
        if !valid_set(key, value, builtin) {
            return Err(format!("the {key} setting is not valid"));
        }
        effective[key] = value.clone();
    }
    let stored = builtins
        .as_object()
        .expect("defaults are an object")
        .iter()
        .filter(|(key, builtin)| !same_value(&effective[key.as_str()], builtin))
        .map(|(key, _)| (key.clone(), effective[key.as_str()].clone()))
        .collect::<serde_json::Map<_, _>>();
    atomic_write_json(&root.join(CONFIG_FILE_NAME), &JsonValue::Object(stored), true)?;
    *held = Some((root.to_path_buf(), effective.clone()));
    crate::sleep_prevention::configure(&effective);
    register_volume_roots(Some(&effective));
    Ok(SaveOutcome { effective, quarantined })
}

/// JSON equality with numbers compared by value, so `1` equals `1.0`.
fn same_value(left: &JsonValue, right: &JsonValue) -> bool {
    match (left, right) {
        (JsonValue::Number(left), JsonValue::Number(right)) => left.as_f64() == right.as_f64(),
        (JsonValue::Array(left), JsonValue::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(left, right)| same_value(left, right))
        }
        (JsonValue::Object(left), JsonValue::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, value)| right.get(key).is_some_and(|other| same_value(value, other)))
        }
        _ => left == right,
    }
}

/// Patch-merges into `state.json` and returns the merged document. The core
/// holds the file, so it is the one owner of the read-modify-write — the
/// frontend sends only the keys it changes, and a stale cached copy in one
/// store can never blind-overwrite another store's save.
pub fn patch_state(patch: &JsonValue) -> Result<PatchOutcome, String> {
    // not recorded: state.json is volatile UI state and nothing else; the
    // write goes through the unrecorded atomic path (see `patch_json_store`).
    let root = paths::data_root()?;
    patch_json_store(&root.join(STATE_FILE_NAME), patch)
}

/// A patch's merged document plus the quarantine this read-modify-write
/// performed, if any.
pub struct PatchOutcome {
    pub merged: JsonValue,
    pub quarantined: Option<QuarantineRecord>,
}

/// Merges top-level keys into a JSON store; null is a stored value.
pub fn patch_json_store(target: &Path, patch: &JsonValue) -> Result<PatchOutcome, String> {
    // Serialized: this is a read-modify-write dispatched on a thread pool, so
    // two surfaces saving at once could otherwise interleave their reads and
    // the second write would drop the first's keys. One global lock is
    // enough: patches are small and rare, and holding it across the atomic
    // write is what makes the whole read-merge-write atomic with respect to
    // other patchers.
    static PATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = PATCH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let read = read_json_optional(target)?;
    let quarantined = read.quarantined;
    let mut current = read.value.unwrap_or_else(|| serde_json::json!({}));
    if !current.is_object() {
        current = serde_json::json!({});
    }
    let (Some(doc), Some(fields)) = (current.as_object_mut(), patch.as_object()) else {
        return Err("patch must be a JSON object".to_string());
    };
    for (key, value) in fields {
        doc.insert(key.clone(), value.clone());
    }
    // state.json is volatile state and nothing else: written atomically but
    // not recorded in the backup history. Every other patched store records.
    let record = !target
        .file_name()
        .is_some_and(|name| name == STATE_FILE_NAME);
    atomic_write_json(target, &current, record)?;
    Ok(PatchOutcome {
        merged: current,
        quarantined,
    })
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
    read.value = read.value.as_ref().map(|value| effective_config(Some(value)));
    register_volume_roots(read.value.as_ref());
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
        "invalid JSON store quarantined; using built-ins",
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
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("store");
    path.with_file_name(format!("{stem}-{}.invalid", logging::filename_stamp_now()))
}

/// Serializes through serde (never a hand-written literal) and writes atomically,
/// recording the bytes in the backup store only when `record` is set.
fn atomic_write_json(target: &Path, value: &JsonValue, record: bool) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    text.push('\n');
    write_atomic_inner(target, text.as_bytes(), record)
}

/// Atomic write: write to a `<stem>-<nanoid>.tmp` sibling, fsync it, rename over
/// the target, fsync the directory — a crash can never leave a half-written
/// store. Strictly AFTER the rename lands, the exact bytes are recorded into the
/// write-through backup store (the one managed-text choke point).
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_inner(target, bytes, true)
}

/// The same atomic write, WITHOUT the backup record. For text that is excluded
/// from the history by a design-time, per-write-site decision: the version
/// sidecar in the binary-bearing `bin/`, the dependency facts, and the volatile
/// state stores (`state.json`, `window.json`, `preview-window.json`; see the
/// table above).
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

    // Best-effort: persist the rename itself by fsyncing the directory. This
    // owner already knows Windows has no portable directory handle to fsync
    // (a journaled no-replace move stands in for it there) — calling it
    // directly here, rather than opening the directory by hand, is what was
    // logging a spurious warning on every managed-text save on Windows (R1-13).
    if let Err(error) = crate::fs_publish::sync_directory(parent) {
        crate::logging::warn(
            "atomic store directory sync failed",
            serde_json::json!({
                "path": parent,
                "error": { "message": error.to_string() },
            }),
        );
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
// EXCEPTION to the tests-live-in-tests/ rule (tests-folder
// conventions, Rust form): the quarantine/temp-name grammar and the
// optional-read policy are private internals of this store —
// promoting them would widen the surface just to test through it.
#[path = "../tests/unit/storage.rs"]
mod tests;
