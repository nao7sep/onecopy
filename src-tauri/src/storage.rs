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
//! - `records-window.json` — volatile Records placement state.  not recorded (volatile state; unrecorded atomic path)
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
//!
//! Every JSON store carries its format version (`formats`) as a top-level
//! `formatVersion`, stamped on write and removed on read; a store without it
//! is unreadable and set aside like any other. A store a newer OneCopy wrote is never quarantined or written to: the settings stop the
//! launch by name, and volatile state reads as absent and is not saved over.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::{backup_store, formats, logging, nanoid, paths};

pub const CONFIG_FILE_NAME: &str = "config.json";
pub const STATE_FILE_NAME: &str = "state.json";
pub const WINDOW_FILE_NAME: &str = "window.json";
pub const PREVIEW_WINDOW_FILE_NAME: &str = "preview-window.json";
pub const RECORDS_WINDOW_FILE_NAME: &str = "records-window.json";
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
    pub video_snapshots_enabled: bool,
    pub similar_photo_analysis_enabled: bool,
    pub video_transcription_enabled: bool,
    pub audio_transcription_enabled: bool,
    /// Runtime-selected backend per AI engine. The acceleration owner
    /// validates known keys and platform availability before publication.
    pub ai_acceleration: JsonValue,
    pub autoplay: bool,
    pub similar_photo_grouping: String,
    /// One choice for the preview and the fullscreen view.
    pub enlarge_small_images: bool,
    pub text_fallback_encoding: String,
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
}

impl Default for DefaultConfig {
    fn default() -> Self {
        DefaultConfig {
            ignored_file_names: vec![".DS_Store".into(), "Thumbs.db".into(), "desktop.ini".into()],
            hide_dot_names: true,
            hide_hidden_attributes: true,
            hide_system_attributes: true,
            default_timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
            video_snapshots_enabled: true,
            similar_photo_analysis_enabled: true,
            video_transcription_enabled: true,
            audio_transcription_enabled: true,
            ai_acceleration: crate::ai_acceleration::default_config(),
            autoplay: true,
            similar_photo_grouping: "normal".to_string(),
            enlarge_small_images: true,
            text_fallback_encoding: crate::text_preview::DEFAULT_FALLBACK_ENCODING.to_string(),
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
            maximum_images_in_comparison: 16,
            notification_display_seconds: 6,
            confirm_trash_delete: true,
            screen_priority: Vec::new(),
            source_dirs: Vec::new(),
            destination_roots: Vec::new(),
        }
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
        "similarPhotoGrouping" => matches!(value.as_str(), Some("stricter" | "normal" | "looser")),
        "playbackVolume" => value.as_f64().is_some_and(|value| value.is_finite() && (0.01..=1.0).contains(&value)),
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
/// exception needing a buffer: the startup load of the settings, and the
/// pre-window state reads, can set a store aside before any webview exists to
/// report to, so they park the record here and the frontend's
/// `load_from_root` picks it up. Nothing else feeds or drains this.
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

/// Preferences needed by auxiliary windows: a read-only projection of the
/// settings in memory. It never repairs a store or drains the quarantine
/// notices owned by Main.
pub fn appearance_preferences(root: &Path) -> Result<JsonValue, String> {
    let config = config(root)?;
    Ok(serde_json::json!({
        "uiFontFamily": config.get("uiFontFamily"),
        "enlargeSmallImages": config.get("enlargeSmallImages"),
        "videoTranscriptionEnabled": config.get("videoTranscriptionEnabled"),
        "audioTranscriptionEnabled": config.get("audioTranscriptionEnabled"),
    }))
}

/// Projects `LanguageState`'s CURRENT values into an appearance-preferences
/// document, the way the `appearance_preferences` command does after calling
/// `appearance_preferences` above. Kept apart from that command's
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

/// Reads volatile state for a pre-frontend runtime decision while preserving
/// the ordinary load path's duty to report any quarantine to Main.
pub fn read_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(STATE_FILE_NAME), formats::STATE)?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn read_window_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(WINDOW_FILE_NAME), formats::WINDOW_PLACEMENT)?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn save_window_state(root: &Path, state: &JsonValue) -> Result<(), String> {
    write_json_file(&root.join(WINDOW_FILE_NAME), state, formats::WINDOW_PLACEMENT, false)
}

pub fn read_preview_window_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(PREVIEW_WINDOW_FILE_NAME), formats::WINDOW_PLACEMENT)?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn save_preview_window_state(root: &Path, state: &JsonValue) -> Result<(), String> {
    write_json_file(&root.join(PREVIEW_WINDOW_FILE_NAME), state, formats::WINDOW_PLACEMENT, false)
}

pub fn read_records_window_state_for_setup(root: &Path) -> Result<Option<JsonValue>, String> {
    let read = read_json_optional(&root.join(RECORDS_WINDOW_FILE_NAME), formats::WINDOW_PLACEMENT)?;
    if let Some(record) = read.quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(read.value)
}

pub fn save_records_window_state(root: &Path, state: &JsonValue) -> Result<(), String> {
    write_json_file(&root.join(RECORDS_WINDOW_FILE_NAME), state, formats::WINDOW_PLACEMENT, false)
}

/// Reads `state.json` for a surface other than Main's startup load. A store
/// this read sets aside is returned beside the value for its caller to report.
pub fn read_state(root: &Path) -> Result<(Option<JsonValue>, Option<QuarantineRecord>), String> {
    let read = read_json_optional(&root.join(STATE_FILE_NAME), formats::STATE)?;
    Ok((read.value, read.quarantined))
}

pub fn load_from_root(root: &Path) -> Result<LoadedAppData, String> {
    let config = (*config(root)?).clone();
    let mut quarantines = take_pending_quarantines();
    let state_read = read_json_optional(&root.join(STATE_FILE_NAME), formats::STATE)?;
    if let Some(record) = state_read.quarantined {
        quarantines.push(record);
    }
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

/// The configured source roots, as the settings in memory list them.
pub fn configured_source_dirs(data_root: &Path) -> Result<Vec<String>, String> {
    Ok(config(data_root)?
        .get("sourceDirs")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(JsonValue::as_str)
        .map(str::to_string)
        .collect())
}

/// All roots whose own permission boundary must contain recoverable deleted
/// files. The returned set is an operation-planning snapshot.
pub fn configured_file_roots(data_root: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(configured_roots(data_root)?.all())
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

pub fn configured_roots(data_root: &Path) -> Result<ConfiguredRoots, String> {
    Ok(configured_roots_in(Some(&*config(data_root)?)))
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
/// conventions). Keyed by root so a different root is read afresh. Held as a
/// shared snapshot: a reader clones the `Arc` and works without the lock, and
/// a save installs a new snapshot whole, so every read that starts after a
/// save sees it and work already running keeps the settings it started with.
type HeldConfig = Option<(PathBuf, Arc<JsonValue>)>;
static HELD_CONFIG: std::sync::Mutex<HeldConfig> = std::sync::Mutex::new(None);

/// The held settings for `root`, reading and holding them first when another
/// root or nothing is held, with the quarantine that read performed, if any.
fn held_or_read(held: &mut HeldConfig, root: &Path) -> Result<(Arc<JsonValue>, Option<QuarantineRecord>), String> {
    if let Some((held_root, effective)) = held.as_ref() {
        if held_root == root {
            return Ok((Arc::clone(effective), None));
        }
    }
    let read = read_config_optional(&root.join(CONFIG_FILE_NAME))?;
    let effective = Arc::new(read.value.unwrap_or_else(|| effective_config(None)));
    *held = Some((root.to_path_buf(), Arc::clone(&effective)));
    Ok((effective, read.quarantined))
}

/// The settings in memory, which every read of a set goes through. The first
/// call for a root is the one load: the lock owner's startup load
/// (`startup::prepare_data`), unless [`held_config`] already held a readable
/// file before the lock. That load sets an unreadable file aside and parks
/// the record for Main's `load_from_root` to report. Every later call returns
/// the held snapshot; the file is not read again.
pub fn config(root: &Path) -> Result<Arc<JsonValue>, String> {
    let (effective, quarantined) = {
        let mut held = HELD_CONFIG.lock().unwrap_or_else(|p| p.into_inner());
        held_or_read(&mut held, root)?
    };
    if let Some(record) = quarantined {
        PENDING_QUARANTINES
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(record);
    }
    Ok(effective)
}

/// The held settings for the reads that come before the window shows: the
/// interface language, read before this process owns the instance lock, and
/// the theme. The first call reads `config.json` and holds it. A file that
/// cannot be read, is not a settings object, or was written by a newer
/// OneCopy is neither held nor touched, because setting a store aside and
/// stopping the launch belong to the lock owner's load; the built-ins answer
/// until that load holds what it recovers.
pub fn held_config(root: &Path) -> JsonValue {
    let mut held = HELD_CONFIG.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((held_root, effective)) = held.as_ref() {
        if held_root == root {
            return JsonValue::clone(effective);
        }
    }
    let stored = match read_json_file(&root.join(CONFIG_FILE_NAME), formats::CONFIG) {
        Ok(JsonFile::Document(document)) if document.is_object() => Some(document),
        Ok(JsonFile::Absent) => None,
        _ => return effective_config(None),
    };
    let effective = effective_config(stored.as_ref());
    register_volume_roots(Some(&effective));
    *held = Some((root.to_path_buf(), Arc::new(effective.clone())));
    effective
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
    let (current, quarantined) = held_or_read(&mut held, root)?;
    let mut effective = JsonValue::clone(&current);
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
    write_json_file(&root.join(CONFIG_FILE_NAME), &JsonValue::Object(stored), formats::CONFIG, true)?;
    *held = Some((root.to_path_buf(), Arc::new(effective.clone())));
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
    patch_json_store(&root.join(STATE_FILE_NAME), formats::STATE, patch)
}

/// A patch's merged document plus the quarantine this read-modify-write
/// performed, if any.
pub struct PatchOutcome {
    pub merged: JsonValue,
    pub quarantined: Option<QuarantineRecord>,
}

/// Merges top-level keys into a JSON store of format `version`; null is a
/// stored value. A store a newer OneCopy wrote is not patched.
pub fn patch_json_store(target: &Path, version: i64, patch: &JsonValue) -> Result<PatchOutcome, String> {
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

    let read = read_json_optional(target, version)?;
    if let Some(newer) = read.newer {
        return Err(newer.to_string());
    }
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
    write_json_file(target, &current, version, record)?;
    Ok(PatchOutcome {
        merged: current,
        quarantined,
    })
}

/// One versioned JSON store file as read: its document with the format
/// marker removed, or why it cannot be used.
pub enum JsonFile {
    Absent,
    Document(JsonValue),
    /// Not JSON, or a marker that is not a version.
    Unreadable(String),
    /// Written by a newer OneCopy: left exactly in place.
    Newer(formats::NewerStore),
}

/// Reads a JSON store of format `supported`. Only a failure to read the
/// bytes is an error; what the bytes mean is the caller's branch.
pub fn read_json_file(path: &Path, supported: i64) -> Result<JsonFile, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(JsonFile::Absent),
        Err(err) => return Err(err.to_string()),
    };
    let mut document = match serde_json::from_slice::<JsonValue>(&bytes) {
        Ok(document) => document,
        Err(parse_error) => return Ok(JsonFile::Unreadable(format!("parse error: {parse_error}"))),
    };
    Ok(match formats::take_json_version(&mut document) {
        Err(reason) => JsonFile::Unreadable(reason),
        Ok(version) => match formats::NewerStore::check(path, version, supported) {
            Some(newer) => JsonFile::Newer(newer),
            None => JsonFile::Document(document),
        },
    })
}

/// Writes a JSON store of format `version` atomically, the marker stamped on
/// the way out, recording the bytes in the backup store when `record` is
/// set. A file a newer OneCopy wrote is never written over.
pub fn write_json_file(target: &Path, document: &JsonValue, version: i64, record: bool) -> Result<(), String> {
    refuse_newer(target, version)?;
    let mut document = document.clone();
    formats::stamp_json(&mut document, version)?;
    let mut text = serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?;
    text.push('\n');
    write_atomic_inner(target, text.as_bytes(), record, || refuse_newer(target, version))
}

/// Refuses to replace or remove a JSON store a newer OneCopy wrote.
pub fn refuse_newer(target: &Path, version: i64) -> Result<(), String> {
    match read_json_file(target, version)? {
        JsonFile::Newer(newer) => Err(newer.to_string()),
        _ => Ok(()),
    }
}

/// Logs a store a newer OneCopy wrote, which is read as absent and left in
/// place.
pub fn warn_newer(newer: &formats::NewerStore) {
    logging::warn(
        "store written by a newer OneCopy; left as it is",
        serde_json::json!({
            "file": newer.path,
            "formatVersion": newer.version,
            "supported": newer.supported,
        }),
    );
}

struct JsonRead {
    value: Option<JsonValue>,
    /// Set when this read set the store aside; the caller owns getting the
    /// record to a reporting surface.
    quarantined: Option<QuarantineRecord>,
    /// Set when a newer OneCopy wrote the store; `value` is then absent.
    newer: Option<formats::NewerStore>,
}

/// Reads an optional JSON store; invalid content is quarantined aside and the
/// record returned. Config additionally requires an object root, which is an
/// envelope invariant rather than feature-level value validation. The rename
/// failure propagates before any caller can write defaults.
fn read_json_optional(path: &Path, version: i64) -> Result<JsonRead, String> {
    read_json_optional_with_envelope(path, version, false)
}

fn read_config_optional(path: &Path) -> Result<JsonRead, String> {
    let mut read = read_json_optional_with_envelope(path, formats::CONFIG, true)?;
    if let Some(newer) = &read.newer {
        return Err(newer.to_string());
    }
    read.value = read.value.as_ref().map(|value| effective_config(Some(value)));
    register_volume_roots(read.value.as_ref());
    Ok(read)
}

/// Whether `config.json` was written by a newer OneCopy, read without
/// touching it, for the launch to stop on by name.
pub fn config_newer(root: &Path) -> Result<Option<formats::NewerStore>, String> {
    Ok(match read_json_file(&root.join(CONFIG_FILE_NAME), formats::CONFIG)? {
        JsonFile::Newer(newer) => Some(newer),
        _ => None,
    })
}

fn read_json_optional_with_envelope(
    path: &Path,
    version: i64,
    require_object_root: bool,
) -> Result<JsonRead, String> {
    let read = |value| JsonRead { value, quarantined: None, newer: None };
    match read_json_file(path, version)? {
        JsonFile::Absent => Ok(read(None)),
        JsonFile::Document(value) if !require_object_root || value.is_object() => Ok(read(Some(value))),
        JsonFile::Document(_) => quarantine_invalid_store(path, "config root must be a JSON object"),
        JsonFile::Unreadable(reason) => quarantine_invalid_store(path, &reason),
        JsonFile::Newer(newer) => {
            warn_newer(&newer);
            Ok(JsonRead { value: None, quarantined: None, newer: Some(newer) })
        }
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
        newer: None,
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

/// Atomic write: write to a `<stem>-<nanoid>.tmp` sibling, fsync it, rename over
/// the target, fsync the directory — a crash can never leave a half-written
/// store. Strictly AFTER the rename lands, the exact bytes are recorded into the
/// write-through backup store (the one managed-text choke point).
pub fn write_atomic(target: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_inner(target, bytes, true, || Ok(()))
}

/// The same atomic write, WITHOUT the backup record. For text that is excluded
/// from the history by a design-time, per-write-site decision (see the table
/// above); the JSON stores choose through `write_json_file`.
pub fn write_atomic_unrecorded(target: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_inner(target, bytes, false, || Ok(()))
}

fn write_atomic_inner(
    target: &Path,
    bytes: &[u8],
    record: bool,
    admit: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    use std::io::{Read, Write};
    let existing = match std::fs::File::open(target) {
        Ok(mut file) => {
            let mut current = Vec::new();
            file.read_to_end(&mut current).map_err(|error| error.to_string())?;
            if current == bytes {
                return Ok(());
            }
            Some(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let parent = target
        .parent()
        .ok_or_else(|| "path has no parent directory".to_string())?;
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "path has no file name".to_string())?;
    let tmp = parent.join(atomic_temp_name(file_name)?);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|error| error.to_string())?;
    if let Err(error) = crate::copy_metadata::make_private(&file) {
        crate::file_identity::remove_private_if_owned(&tmp, &file);
        return Err(error.to_string());
    }
    let publish = (|| -> Result<(), String> {
        file.write_all(bytes).map_err(|error| error.to_string())?;
        if let Some(source) = &existing {
            crate::copy_metadata::apply_replacement(source, &file).map_err(|error| error.to_string())?;
        }
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        drop(existing);
        admit()?;
        std::fs::rename(&tmp, target).map_err(|error| error.to_string())
    })();
    if let Err(error) = publish {
        #[cfg(windows)] {
            let writable = (|| -> std::io::Result<()> {
                let mut permissions = std::fs::metadata(&tmp)?.permissions();
                if permissions.readonly() {
                    permissions.set_readonly(false);
                    std::fs::set_permissions(&tmp, permissions)?;
                }
                Ok(())
            })();
            if let Err(cleanup_error) = writable {
                crate::logging::warn(
                    "atomic store staging permissions cleanup failed",
                    serde_json::json!({ "path": tmp, "error": { "message": cleanup_error.to_string() } }),
                );
            }
        }
        crate::fs_recovery::remove_file(&tmp, "atomic store publication cleanup");
        return Err(error);
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
