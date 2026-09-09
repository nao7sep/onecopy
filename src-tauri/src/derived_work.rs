//! The single owner of reconstructible media work. Indexing records file and
//! time facts, then wakes this coordinator; previews, video posters, scene
//! strips, transcripts, face scores, and similarity are never scan phases.
//!
//! Preview/poster work runs in bounded fair turns. Independent native image
//! conversions may share a turn under live CPU and memory budgets; database
//! publication, ffmpeg routes, and every other heavy class remain serialized.
//! All runnable backlogs progress continuously; attention orders bounded turns.
//! Activity adjusts conversion capacity, never eligibility. All classes re-read settings for each
//! pass, so installing a tool or changing a feature takes effect on the next
//! wake without restarting either indexing or the app.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};
use std::thread::JoinHandle;

use rusqlite::Connection;
use serde_json::json;
use tauri::AppHandle;

use crate::derived_runtime::{
    cancelled, emit_state_changed, is_paused as class_paused, progress as record_progress,
    with_active,
};
use crate::derived_state::WorkClass;
use crate::logging;
use crate::preview::CachePaths;

pub const WORKER_FAILED: &str = "derived-worker-failed";

static LAST_ACTIVITY_MS: AtomicI64 = AtomicI64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static AUTOMATIC_ADMITTED: AtomicBool = AtomicBool::new(false);
static DEBT_REVISION: AtomicU64 = AtomicU64::new(0);
static ATTENTION_GENERATION: AtomicU64 = AtomicU64::new(0);
static WORKERS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());
static WAKE: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
static PRIORITY: LazyLock<Mutex<PriorityHints>> =
    LazyLock::new(|| Mutex::new(PriorityHints::default()));
static REQUESTED_PREVIEWS: LazyLock<Mutex<HashMap<String, Arc<RequestedPreviewFlight>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct RequestedPreviewFlight {
    result: Mutex<Option<Result<String, String>>>,
    ready: Condvar,
}

const IDLE_AFTER_MS: i64 = 60_000;
const POLL_SECONDS: u64 = 15;
const VISIBLE_PREVIEW_TURN: usize = 8;
const SECTION_PREVIEW_TURN: usize = 1;
const SECTION_HINT_LIMIT: usize = 256;
const OPTIONAL_CLASSES: [WorkClass; 5] = [
    WorkClass::Similarity,
    WorkClass::Snapshots,
    WorkClass::VideoTranscripts,
    WorkClass::AudioTranscripts,
    WorkClass::Faces,
];

#[derive(Clone, Default)]
struct PriorityHints {
    generation: u64,
    selected: Option<String>,
    visible: Vec<String>,
    nearby: Vec<String>,
    section: Option<SectionPriority>,
    traversal: Option<SectionTraversal>,
}

#[derive(Clone)]
pub struct SectionTraversal {
    pub sort: crate::queries::SectionSort,
    pub anchor: u64,
    pub total: u64,
}

#[derive(Default)]
struct CandidateCursor {
    after_hash: Option<String>,
    exhausted: bool,
}

#[derive(Default)]
struct CandidateCursors {
    snapshots: CandidateCursor,
    faces: CandidateCursor,
    video_transcripts: CandidateCursor,
    audio_transcripts: CandidateCursor,
    next_optional: usize,
    turns: crate::work_priority::Turns,
    attention_generation: u64,
    section: SectionCursor,
    library_optional: bool,
}

#[derive(Default)]
struct SectionCursor {
    initialized: bool,
    left: Option<crate::queries::SectionWorkPosition>,
    right: Option<crate::queries::SectionWorkPosition>,
    before: bool,
    current: Option<Vec<String>>,
}

impl SectionCursor {
    fn pending(&self) -> bool {
        !self.initialized || self.current.is_some() || self.left.is_some() || self.right.is_some()
    }
}

impl CandidateCursors {
    fn invalidate(&mut self) {
        self.snapshots = CandidateCursor::default();
        self.faces = CandidateCursor::default();
        self.video_transcripts = CandidateCursor::default();
        self.audio_transcripts = CandidateCursor::default();
        self.attention_generation = u64::MAX;
    }
}

#[derive(Clone)]
pub struct SectionPriority {
    pub kind: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

pub struct Settings {
    pub data_root: PathBuf,
    pub cache_root: PathBuf,
    pub similarity: crate::similarity::SimilarityConfig,
    pub strip: crate::video::StripConfig,
    pub thumb_edge: u32,
    pub preview_long_edge: u32,
    pub ffmpeg: Option<PathBuf>,
    pub video_snapshots_enabled: bool,
    pub similarity_enabled: bool,
    pub face_enabled: bool,
    pub face_models: Option<FaceAssets>,
    pub transcription_model: Option<PathBuf>,
    pub transcription_acceleration: crate::ai_acceleration::Mode,
    pub video_transcription_enabled: bool,
    pub audio_transcription_enabled: bool,
    pub temp_dir: PathBuf,
}

pub type FaceAssets = crate::ai_dependencies::FaceScoringDependencies;

impl Settings {
    fn capabilities(&self) -> crate::derived_state::WorkCapabilities {
        crate::derived_state::WorkCapabilities {
            ffmpeg: self.ffmpeg.is_some(),
            video_snapshots_enabled: self.video_snapshots_enabled,
            similarity_enabled: self.similarity_enabled,
            face_enabled: self.face_enabled,
            face_models: self.face_models.is_some(),
            transcription_model: self.transcription_model.is_some(),
            video_transcription_enabled: self.video_transcription_enabled,
            audio_transcription_enabled: self.audio_transcription_enabled,
        }
    }
}

pub fn settings_from_config(
    config: Option<&serde_json::Value>,
    data_root: &Path,
) -> Result<Settings, String> {
    let defaults = crate::storage::DefaultConfig::default();
    let acceleration = crate::ai_acceleration::selection_from_config(config)?;
    let get = |key: &str| config.and_then(|c| c.get(key));
    let u32_of = |key: &str, fallback: u32| -> u32 {
        get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(fallback)
    };
    let transcription_dependencies = crate::ai_dependencies::production_transcription(data_root);
    let score_faces = get("scoreFaces")
        .and_then(|v| v.as_bool())
        .unwrap_or(defaults.score_faces);
    let bool_of = |key: &str, fallback: bool| {
        get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(fallback)
    };

    Ok(Settings {
        data_root: data_root.to_path_buf(),
        cache_root: data_root.join(crate::storage::CACHE_DIR_NAME),
        similarity: crate::similarity::SimilarityConfig {
            max_gap_seconds: u32_of(
                "similarityMaxGapSeconds",
                defaults.similarity_max_gap_seconds,
            ),
            phash_max_distance: u32_of(
                "similarityPhashMaxDistance",
                defaults.similarity_phash_max_distance,
            ),
            phash_max_distance_burst: u32_of(
                "similarityPhashMaxDistanceBurst",
                defaults.similarity_phash_max_distance_burst,
            ),
            diameter_multiplier: u32_of(
                "similarityDiameterMultiplier",
                defaults.similarity_diameter_multiplier,
            ),
        },
        strip: crate::video::StripConfig {
            seconds_per_frame: u32_of(
                "videoStripSecondsPerFrame",
                defaults.video_strip_seconds_per_frame,
            ),
            min_frames: u32_of("videoStripMinFrames", defaults.video_strip_min_frames),
            max_frames: u32_of("videoStripMaxFrames", defaults.video_strip_max_frames),
        },
        thumb_edge: u32_of("thumbnailEdgePx", defaults.thumbnail_edge_px),
        preview_long_edge: u32_of("previewLongEdgePx", defaults.preview_long_edge_px),
        ffmpeg: transcription_dependencies.ffmpeg,
        video_snapshots_enabled: bool_of("videoSnapshotsEnabled", defaults.video_snapshots_enabled),
        similarity_enabled: bool_of(
            "similarPhotoAnalysisEnabled",
            defaults.similar_photo_analysis_enabled,
        ),
        face_enabled: score_faces,
        face_models: score_faces
            .then(|| crate::ai_dependencies::production_face_scoring(data_root))
            .flatten(),
        transcription_model: transcription_dependencies.model,
        transcription_acceleration: acceleration.transcription,
        video_transcription_enabled: bool_of(
            "videoTranscriptionEnabled",
            defaults.video_transcription_enabled,
        ),
        audio_transcription_enabled: bool_of(
            "audioTranscriptionEnabled",
            defaults.audio_transcription_enabled,
        ),
        temp_dir: data_root.join(crate::binaries_manager::TEMP_DIR_NAME),
    })
}

pub fn work_capabilities(
    data_root: &Path,
) -> Result<crate::derived_state::WorkCapabilities, String> {
    let config = crate::storage::read_config_for_setup(data_root)?;
    let settings = settings_from_config(config.as_ref(), data_root)?;
    Ok(settings.capabilities())
}

pub fn note_activity() {
    LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn is_idle() -> bool {
    now_ms() - LAST_ACTIVITY_MS.load(Ordering::SeqCst) >= IDLE_AFTER_MS
        && !crate::derived_runtime::exclusive()
        && !crate::scan_runtime::running()
}

pub(crate) fn available() -> bool {
    !crate::derived_runtime::shutting_down()
        && AUTOMATIC_ADMITTED.load(Ordering::SeqCst)
        && !crate::derived_runtime::exclusive()
        && !crate::scan_runtime::running()
}

/// Opens the automatic media queue only after the launch source decision has
/// reached its terminal boundary. Starting the worker before this point keeps
/// startup cheap, but it must not enrich stale rows before source
/// reconciliation has had the first chance to retire them.
pub(crate) fn admit_automatic() {
    if crate::derived_runtime::shutting_down() {
        return;
    }
    AUTOMATIC_ADMITTED.store(true, Ordering::SeqCst);
    if crate::derived_runtime::shutting_down() {
        AUTOMATIC_ADMITTED.store(false, Ordering::SeqCst);
        return;
    }
    wake();
}

/// Wake after index, settings, tool, priority, or lifecycle changes. Durable
/// source triggers own derived invalidation; this signal only schedules work.
pub fn wake() {
    DEBT_REVISION.fetch_add(1, Ordering::SeqCst);
    wake_priority();
}

fn wake_priority() {
    let (generation, ready) = WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()));
    match generation.lock() {
        Ok(mut value) => {
            *value = value.wrapping_add(1);
            ready.notify_one();
        }
        Err(_) => logging::error("derived-work wake state is unavailable", json!({})),
    }
}

/// Replaces the current UI priority hints. They are deliberately ephemeral:
/// output absence remains the queue and a restart needs no job recovery.
pub fn set_priority(
    selected: Option<String>,
    visible: Vec<String>,
    nearby: Vec<String>,
    section: Option<SectionPriority>,
    traversal: Option<SectionTraversal>,
    generation: u64,
) {
    match PRIORITY.lock() {
        Ok(mut hints) => {
            if generation <= hints.generation {
                return;
            }
            hints.generation = generation;
            hints.selected = selected;
            hints.visible = visible.into_iter().take(SECTION_HINT_LIMIT).collect();
            hints.nearby = nearby.into_iter().take(SECTION_HINT_LIMIT).collect();
            hints.section = section;
            hints.traversal = traversal;
            ATTENTION_GENERATION.store(generation, Ordering::SeqCst);
            note_activity();
        }
        Err(_) => logging::error("derived-work priority state is unavailable", json!({})),
    }
    if crate::derived_runtime::automatic_optional_active() {
        let hints = PRIORITY.lock().map(|hints| hints.clone());
        if let Ok(hints) = hints {
            let required = hints
                .visible
                .iter()
                .chain(&hints.nearby)
                .cloned()
                .collect::<Vec<_>>();
            if required_priority_pending(hints.selected.as_deref(), &required) {
                crate::derived_runtime::preempt_automatic_optional_for_required();
            }
        }
    }
    wake_priority();
}

fn required_priority_pending(selected: Option<&str>, visible: &[String]) -> bool {
    if class_paused(WorkClass::Previews) {
        return false;
    }
    let Some(data_root) = crate::DATA_ROOT.get() else {
        return false;
    };
    let pending = (|| -> Result<bool, String> {
        let config = crate::storage::read_config_for_setup(data_root)?;
        let settings = settings_from_config(config.as_ref(), data_root)?;
        let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
        priority_candidates_for_class(
            &conn,
            &settings,
            WorkClass::Previews.id(),
            selected,
            visible,
            None,
        )
        .map(|candidates| !candidates.is_empty())
    })();
    pending.unwrap_or_else(|error| {
        logging::error(
            "urgent preview priority could not be evaluated",
            json!({ "error": { "message": error } }),
        );
        // Yield optional work; the coordinator owns the terminal failure if
        // its next ordinary settings/database read also fails.
        true
    })
}

/// Recheck only on a new attention generation, including the race between a
/// hint arriving and an optional executor acquiring its runtime claim.
fn optional_stop() -> impl Fn() -> bool + Send + 'static {
    let observed = std::cell::Cell::new(u64::MAX);
    move || {
        if cancelled() {
            return true;
        }
        let generation = ATTENTION_GENERATION.load(Ordering::SeqCst);
        if generation == observed.get() {
            return false;
        }
        observed.set(generation);
        let hints = PRIORITY.lock().map(|hints| hints.clone());
        if let Ok(hints) = hints {
            let hashes = hints
                .visible
                .iter()
                .chain(&hints.nearby)
                .cloned()
                .collect::<Vec<_>>();
            if required_priority_pending(hints.selected.as_deref(), &hashes) {
                crate::derived_runtime::preempt_automatic_optional_for_required();
                return true;
            }
        }
        false
    }
}

pub fn start(app: AppHandle) -> Result<bool, String> {
    let mut workers = WORKERS
        .lock()
        .map_err(|_| "previews-and-analysis worker state is unavailable".to_string())?;
    if crate::derived_runtime::shutting_down() {
        return Err("previews and analysis are shutting down".to_string());
    }
    join_finished(&mut workers);
    if STARTED.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    let handle = app.clone();
    let worker = std::thread::Builder::new()
        .name("onecopy-derived-work".to_string())
        .spawn(move || derived_worker(handle))
        .map_err(|error| {
            STARTED.store(false, Ordering::SeqCst);
            format!("could not start previews-and-analysis worker: {error}")
        })?;
    workers.push(worker);
    drop(workers);
    crate::derived_runtime::emit_state_changed(&app);
    wake();
    Ok(true)
}

/// Owns each requested transcription thread beside the automatic coordinator
/// so final shutdown closes their shared admission and joins both populations
/// through one derived-media lifecycle.
pub fn spawn_manual_transcription(work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let mut workers = WORKERS
        .lock()
        .map_err(|_| "derived-media worker state is unavailable".to_string())?;
    if crate::derived_runtime::shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    join_finished(&mut workers);
    let worker = std::thread::Builder::new()
        .name("onecopy-manual-transcription".to_string())
        .spawn(work)
        .map_err(|error| error.to_string())?;
    workers.push(worker);
    Ok(())
}

pub fn started() -> bool {
    STARTED.load(Ordering::SeqCst)
}

fn derived_worker(app: AppHandle) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_worker_loop(&app)));
    let failure = match outcome {
        Ok(Ok(())) => {
            STARTED.store(false, Ordering::SeqCst);
            crate::derived_runtime::emit_state_changed(&app);
            return;
        }
        Ok(Err(error)) => error,
        Err(payload) => crate::failure_runtime::panic_message(payload),
    };
    if crate::app_lifecycle::shutting_down() {
        STARTED.store(false, Ordering::SeqCst);
        logging::error(
            "derived-media worker failed during shutdown",
            json!({ "error": { "message": failure } }),
        );
        return;
    }
    let _ = crate::failure_runtime::report(
        &app,
        WORKER_FAILED,
        None,
        &failure,
    );
    // Finish the old worker's failure record before admitting its replacement;
    // otherwise the replacement could resolve a condition not yet recorded.
    STARTED.store(false, Ordering::SeqCst);
    crate::derived_runtime::emit_state_changed(&app);
    crate::failure_runtime::emit_or_record(
        &app,
        "derived://worker-failed",
        json!({ "message": failure }),
    );
}

fn run_worker_loop(app: &AppHandle) -> Result<(), String> {
    let (generation, ready) = WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()));
    let mut observed = 0u64;
    let mut run_again = true;
    let mut cursors = CandidateCursors::default();
    let mut debt_revision = DEBT_REVISION.load(Ordering::SeqCst);
    let mut cleared_previous_failure = false;
    loop {
        if crate::derived_runtime::shutting_down() {
            return Ok(());
        }
        if !run_again {
            let value = generation
                .lock()
                .map_err(|_| "derived-work wake state is unavailable".to_string())?;
            let (current, _) = ready
                .wait_timeout_while(
                    value,
                    std::time::Duration::from_secs(POLL_SECONDS),
                    |current| *current == observed && !crate::derived_runtime::shutting_down(),
                )
                .map_err(|_| "derived-work wake state is unavailable".to_string())?;
            observed = *current;
        } else {
            let current = *generation
                .lock()
                .map_err(|_| "derived-work wake state is unavailable".to_string())?;
            if current != observed {
                observed = current;
            }
        }
        if crate::derived_runtime::shutting_down() {
            return Ok(());
        }
        run_again = false;
        let revision = DEBT_REVISION.load(Ordering::SeqCst);
        if revision != debt_revision {
            cursors.invalidate();
            debt_revision = revision;
        }
        if !available() {
            continue;
        }
        let Some(pass) =
            crate::scan_runtime::try_with_derived_claim(|| run_one_pass(app, &mut cursors))
        else {
            continue;
        };
        match pass {
            Ok(did_work) => {
                if !cleared_previous_failure {
                    crate::failure_runtime::clear(
                        app,
                        WORKER_FAILED,
                        None,
                    )?;
                    cleared_previous_failure = true;
                }
                run_again = did_work;
            }
            Err(error) if error.starts_with(crate::scanner::CANCELLED) => {
                logging::debug("derived work stopped", json!({ "reason": "cancelled" }));
                run_again = true;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Stops new automatic turns, cancels active/queued derived-media work, and
/// wakes the coordinator so the app's shutdown owner can join it promptly.
pub fn shutdown(app: &AppHandle) {
    let _workers = match WORKERS.lock() {
        Ok(workers) => workers,
        Err(_) => {
            logging::error("derived-media worker state is unavailable", json!({}));
            AUTOMATIC_ADMITTED.store(false, Ordering::SeqCst);
            crate::derived_runtime::begin_shutdown(app);
            wake();
            return;
        }
    };
    AUTOMATIC_ADMITTED.store(false, Ordering::SeqCst);
    crate::derived_runtime::begin_shutdown(app);
    wake();
}

pub fn join() {
    let workers = match WORKERS.lock() {
        Ok(mut workers) => workers.drain(..).collect::<Vec<_>>(),
        Err(_) => {
            logging::error("derived-media worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in workers {
        if worker.join().is_err() {
            logging::error("derived-media worker join failed", json!({}));
        }
    }
}

fn join_finished(workers: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            if worker.join().is_err() {
                logging::error("derived-media worker join failed", json!({}));
            }
        } else {
            index += 1;
        }
    }
}

/// Synchronously produces the selected preview through the same ownership
/// boundary as background work. The command can await this result for an
/// immediate preview without racing the coordinator on the same cache entry.
pub fn ensure_preview(
    app: &AppHandle,
    data_root: &Path,
    config: Option<&serde_json::Value>,
    hash: &str,
) -> Result<EnsurePreviewResult, String> {
    let (canonical_hash, coalesced) =
        coalesce_requested_preview(hash, || ensure_preview_once(app, data_root, config, hash))?;
    Ok(EnsurePreviewResult {
        canonical_hash,
        coalesced,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsurePreviewResult {
    pub canonical_hash: String,
    pub coalesced: bool,
}

fn ensure_preview_once(
    app: &AppHandle,
    data_root: &Path,
    config: Option<&serde_json::Value>,
    hash: &str,
) -> Result<String, String> {
    let _active = crate::derived_runtime::begin_manual(app, WorkClass::Previews.id())?;
    crate::derived_runtime::active_item(app, WorkClass::Previews, hash);
    let settings = settings_from_config(config, data_root)?;
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let cache = CachePaths::new(settings.cache_root.clone());
    let result = crate::preview::derive_one(
        &conn,
        &cache,
        settings.thumb_edge,
        settings.preview_long_edge,
        settings.ffmpeg.as_deref(),
        hash,
    );
    if result.is_ok() {
        wake();
    }
    let projection = crate::queries::ItemProjectionContext {
        capabilities: settings.capabilities(),
    };
    notify_item_update(
        app,
        &conn,
        projection,
        "previews",
        hash,
        result.as_deref().unwrap_or(hash),
    );
    notify_issues(app);
    result
}

fn coalesce_requested_preview(
    hash: &str,
    work: impl FnOnce() -> Result<String, String>,
) -> Result<(String, bool), String> {
    let (flight, leader) = {
        let mut active = REQUESTED_PREVIEWS
            .lock()
            .map_err(|_| "requested preview state is unavailable".to_string())?;
        match active.get(hash) {
            Some(flight) => (flight.clone(), false),
            None => {
                let flight = Arc::new(RequestedPreviewFlight {
                    result: Mutex::new(None),
                    ready: Condvar::new(),
                });
                active.insert(hash.to_string(), flight.clone());
                (flight, true)
            }
        }
    };

    if !leader {
        let result = flight
            .ready
            .wait_while(
                flight
                    .result
                    .lock()
                    .map_err(|_| "requested preview state is unavailable".to_string())?,
                |result| result.is_none(),
            )
            .map_err(|_| "requested preview state is unavailable".to_string())?;
        let canonical_hash = result
            .clone()
            .ok_or_else(|| "requested preview ended without a result".to_string())?;
        return canonical_hash.map(|hash| (hash, true));
    }

    let result = work();
    {
        let mut published = flight
            .result
            .lock()
            .map_err(|_| "requested preview state is unavailable".to_string())?;
        *published = Some(result.clone());
        flight.ready.notify_all();
    }
    let mut active = REQUESTED_PREVIEWS
        .lock()
        .map_err(|_| "requested preview state is unavailable".to_string())?;
    if active
        .get(hash)
        .is_some_and(|current| Arc::ptr_eq(current, &flight))
    {
        active.remove(hash);
    }
    result.map(|hash| (hash, false))
}

#[cfg(test)]
mod requested_preview_tests {
    // Private single-flight bookkeeping is tested here because promoting it
    // would expose a concurrency primitive that no production caller needs.
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn identical_requested_previews_share_one_result() {
        let key = format!("test-preview-{}", crate::nanoid::generate().unwrap());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_key = key.clone();
        let leader = std::thread::spawn(move || {
            coalesce_requested_preview(&first_key, || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok("canonical".to_string())
            })
        });
        started_rx.recv().unwrap();

        let observed = REQUESTED_PREVIEWS
            .lock()
            .unwrap()
            .get(&key)
            .unwrap()
            .clone();
        let executions = Arc::new(AtomicUsize::new(0));
        let follower_executions = executions.clone();
        let follower_key = key.clone();
        let follower = std::thread::spawn(move || {
            coalesce_requested_preview(&follower_key, || {
                follower_executions.fetch_add(1, Ordering::SeqCst);
                Ok("wrong".to_string())
            })
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while Arc::strong_count(&observed) < 4 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let joined = Arc::strong_count(&observed) >= 4;
        release_tx.send(()).unwrap();
        assert!(joined, "follower did not join the active request");

        assert_eq!(leader.join().unwrap().unwrap(), ("canonical".to_string(), false));
        assert_eq!(follower.join().unwrap().unwrap(), ("canonical".to_string(), true));
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }
}

/// One bounded pass. Settings and SQLite are opened once per batch, while
/// every media item is still independently claimed and checkpointed. This
/// avoids reopening both millions of times without holding stale settings
/// indefinitely.
fn run_one_pass(app: &AppHandle, cursors: &mut CandidateCursors) -> Result<bool, String> {
    let data_root = crate::DATA_ROOT.get().ok_or("data root unset")?.clone();
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = settings_from_config(config.as_ref(), &data_root)?;
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    crate::similarity::ensure_config_current(&conn, &settings.similarity)?;
    let cache = CachePaths::new(settings.cache_root.clone());
    let projection = crate::queries::ItemProjectionContext {
        capabilities: settings.capabilities(),
    };

    let hints = PRIORITY
        .lock()
        .map_err(|_| "derived-work priority state is unavailable".to_string())?
        .clone();
    if WorkClass::ALL.into_iter().all(class_paused) {
        crate::failure_runtime::emit_or_record(app, "derived://quiet", json!({}));
        return Ok(false);
    }
    if cursors.attention_generation != hints.generation {
        cursors.section = SectionCursor::default();
        cursors.attention_generation = hints.generation;
    }
    let mut section_hashes = None;
    for tier in cursors.turns.order() {
        use crate::work_priority::Tier;
        if ATTENTION_GENERATION.load(Ordering::SeqCst) != hints.generation {
            return Ok(true);
        }
        if matches!(tier, Tier::SectionRequired | Tier::SectionOptional) && section_hashes.is_none()
        {
            section_hashes = Some(section_window_hashes(
                &conn,
                &settings,
                &hints,
                &mut cursors.section,
            )?);
        }
        let did_work = match tier {
            Tier::VisibleRequired | Tier::NearbyRequired | Tier::SectionRequired => {
                let (selected, hashes, limit) = match tier {
                    Tier::VisibleRequired => (
                        hints.selected.as_deref(),
                        &hints.visible,
                        VISIBLE_PREVIEW_TURN,
                    ),
                    Tier::NearbyRequired => (None, &hints.nearby, VISIBLE_PREVIEW_TURN),
                    _ => (None, section_hashes.as_ref().unwrap(), SECTION_PREVIEW_TURN),
                };
                let candidates = priority_candidates_for_class(
                    &conn,
                    &settings,
                    WorkClass::Previews.id(),
                    selected,
                    hashes,
                    None,
                )?;
                derive_priority_previews(
                    app,
                    &conn,
                    &cache,
                    &settings,
                    projection,
                    &candidates,
                    limit,
                )?
            }
            Tier::VisibleOptional | Tier::SectionOptional => {
                let (selected, hashes) = if tier == Tier::VisibleOptional {
                    (hints.selected.as_deref(), &hints.visible)
                } else {
                    (None, section_hashes.as_ref().unwrap())
                };
                run_priority_optional_turn(
                    app, &conn, &cache, &settings, projection, cursors, selected, hashes, None,
                )?
            }
            Tier::Library => {
                let optional_first = cursors.library_optional;
                let mut worked = false;
                for optional in [optional_first, !optional_first] {
                    worked = if optional {
                        run_global_optional_turn(
                            app, &conn, &cache, &settings, projection, cursors,
                        )?
                    } else {
                        derive_global_required(app, &conn, &cache, &settings, projection)?
                    };
                    if worked {
                        cursors.library_optional = !optional;
                        break;
                    }
                }
                worked
            }
        };
        if did_work {
            cursors.turns.completed(tier);
            return Ok(true);
        }
        if tier == Tier::SectionOptional {
            cursors.section.current = None;
        }
    }
    // Empty prepared windows still advance; there is no wait for input or idle.
    if cursors.section.pending() {
        return Ok(true);
    }
    crate::failure_runtime::emit_or_record(app, "derived://quiet", json!({}));
    emit_state_changed(app);
    Ok(false)
}

fn section_window_hashes(
    conn: &Connection,
    settings: &Settings,
    hints: &PriorityHints,
    sweep: &mut SectionCursor,
) -> Result<Vec<String>, String> {
    if let Some(hashes) = &sweep.current {
        return Ok(hashes.clone());
    }
    let (Some(section), Some(view)) = (hints.section.as_ref(), hints.traversal.as_ref()) else {
        sweep.initialized = true;
        return Ok(Vec::new());
    };
    let bounds = section.start_ms.zip(section.end_ms);
    if !sweep.initialized {
        sweep.initialized = true;
        if view.total == 0 {
            return Ok(Vec::new());
        }
        let position = crate::queries::section_work_anchor(
            conn,
            &section.kind,
            bounds,
            view.sort,
            view.anchor.min(view.total - 1),
        )?;
        let hashes = position
            .as_ref()
            .and_then(|position| position.hash.clone())
            .into_iter()
            .collect::<Vec<_>>();
        sweep.left = position.clone();
        sweep.right = position;
        sweep.current = Some(hashes.clone());
        return Ok(hashes);
    }
    let before = sweep.left.is_some() && (sweep.before || sweep.right.is_none());
    let edge = if before {
        &mut sweep.left
    } else {
        &mut sweep.right
    };
    let Some(position) = edge.as_ref() else {
        return Ok(Vec::new());
    };
    let rows = section_pending_candidates(conn, settings, section, view.sort, position, before)?;
    let hashes = rows
        .iter()
        .filter_map(|row| row.hash.clone())
        .collect::<Vec<_>>();
    *edge = rows.into_iter().last();
    sweep.before = !before;
    sweep.current = Some(hashes.clone());
    Ok(hashes)
}

/// Seek only runnable output debt; completed, disabled, blocked, and paused
/// classes must not force a walk through a prepared section on every scroll.
pub fn section_pending_candidates(
    conn: &Connection,
    settings: &Settings,
    section: &SectionPriority,
    sort: crate::queries::SectionSort,
    position: &crate::queries::SectionWorkPosition,
    before: bool,
) -> Result<Vec<crate::queries::SectionWorkPosition>, String> {
    let pending = crate::derived_state::pending_work_predicate(
        settings.capabilities(), WorkClass::ALL.into_iter().filter(|class| !class_paused(*class)),
    );
    crate::queries::section_pending_work_page(
        conn, &section.kind, section.start_ms.zip(section.end_ms), sort, position, before, &pending,
    )
}

fn derive_priority_previews(
    app: &AppHandle,
    conn: &Connection,
    cache: &CachePaths,
    settings: &Settings,
    projection: crate::queries::ItemProjectionContext,
    hashes: &[String],
    limit: usize,
) -> Result<bool, String> {
    let turn = hashes.iter().take(limit).cloned().collect::<Vec<_>>();
    if turn.is_empty() {
        return Ok(false);
    }
    let mut did_work = false;
    let generation = ATTENTION_GENERATION.load(Ordering::SeqCst);
    let image = with_active(app, WorkClass::Previews, || {
        crate::derived_runtime::active_item(app, WorkClass::Previews, &turn[0]);
        crate::preview::derive_image_hashes(
            conn,
            cache,
            settings.thumb_edge,
            settings.preview_long_edge,
            settings.ffmpeg.as_deref(),
            &turn,
            is_idle(),
            &|| ATTENTION_GENERATION.load(Ordering::SeqCst) != generation,
        )
    })?
    .unwrap_or_default();
    if image.derived + image.failed + image.blocked_no_ffmpeg > 0 {
        emit_progress(app, WorkClass::Previews, None);
        notify_image_changes(app, conn, projection, &image.changes);
        notify_issues(app);
        did_work = true;
    }

    for hash in &turn {
        if !available() || ATTENTION_GENERATION.load(Ordering::SeqCst) != generation {
            break;
        }
        let video = with_active(app, WorkClass::Previews, || {
            crate::derived_runtime::active_item(app, WorkClass::Previews, hash);
            crate::video::derive_video_hash(
                conn,
                cache,
                settings.ffmpeg.as_deref(),
                &settings.temp_dir,
                settings.thumb_edge,
                settings.preview_long_edge,
                hash,
            )
        })?
        .unwrap_or_default();
        if video.derived + video.failed > 0 {
            emit_progress(app, WorkClass::Previews, None);
            notify_video_changes(app, conn, projection, &video.changed_hashes);
            notify_issues(app);
            did_work = true;
        }
    }
    Ok(did_work)
}

fn derive_global_required(
    app: &AppHandle,
    conn: &Connection,
    cache: &CachePaths,
    settings: &Settings,
    projection: crate::queries::ItemProjectionContext,
) -> Result<bool, String> {
    if !available() {
        return Ok(false);
    }
    let generation = ATTENTION_GENERATION.load(Ordering::SeqCst);
    let image = with_active(app, WorkClass::Previews, || {
        crate::preview::derive_next_images(
            conn,
            cache,
            settings.thumb_edge,
            settings.preview_long_edge,
            settings.ffmpeg.as_deref(),
            is_idle(),
            &|hash| crate::derived_runtime::active_item(app, WorkClass::Previews, hash),
        )
    })?
    .unwrap_or_default();
    let mut did_work = image.derived + image.failed + image.blocked_no_ffmpeg > 0;
    if did_work {
        emit_progress(app, WorkClass::Previews, None);
        notify_image_changes(app, conn, projection, &image.changes);
        notify_issues(app);
    }
    if !available() || ATTENTION_GENERATION.load(Ordering::SeqCst) != generation {
        return Ok(did_work);
    }
    let video = with_active(app, WorkClass::Previews, || {
        crate::video::derive_next_video(
            conn,
            cache,
            settings.ffmpeg.as_deref(),
            &settings.temp_dir,
            settings.thumb_edge,
            settings.preview_long_edge,
            &|hash| crate::derived_runtime::active_item(app, WorkClass::Previews, hash),
        )
    })?
    .unwrap_or_default();
    if video.derived + video.failed > 0 {
        emit_progress(app, WorkClass::Previews, None);
        notify_video_changes(app, conn, projection, &video.changed_hashes);
        notify_issues(app);
        did_work = true;
    }
    Ok(did_work)
}

#[allow(clippy::too_many_arguments)]
fn run_priority_optional_turn(
    app: &AppHandle,
    conn: &Connection,
    cache: &CachePaths,
    settings: &Settings,
    projection: crate::queries::ItemProjectionContext,
    cursors: &mut CandidateCursors,
    selected: Option<&str>,
    visible: &[String],
    section: Option<&SectionPriority>,
) -> Result<bool, String> {
    for class in OPTIONAL_CLASSES {
        let mut candidates =
            priority_candidates_for_class(conn, settings, class.id(), selected, visible, section)?;
        candidates.truncate(1);
        if !candidates.is_empty()
            && run_optional_class(
                app,
                conn,
                cache,
                settings,
                projection,
                cursors,
                class,
                &candidates,
                true,
            )?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn run_global_optional_turn(
    app: &AppHandle,
    conn: &Connection,
    cache: &CachePaths,
    settings: &Settings,
    projection: crate::queries::ItemProjectionContext,
    cursors: &mut CandidateCursors,
) -> Result<bool, String> {
    for offset in 0..OPTIONAL_CLASSES.len() {
        let index = (cursors.next_optional + offset) % OPTIONAL_CLASSES.len();
        let class = OPTIONAL_CLASSES[index];
        if run_optional_class(
            app,
            conn,
            cache,
            settings,
            projection,
            cursors,
            class,
            &[],
            false,
        )? {
            cursors.next_optional = (index + 1) % OPTIONAL_CLASSES.len();
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
fn run_optional_class(
    app: &AppHandle,
    conn: &Connection,
    cache: &CachePaths,
    settings: &Settings,
    projection: crate::queries::ItemProjectionContext,
    cursors: &mut CandidateCursors,
    class: WorkClass,
    priority: &[String],
    foreground: bool,
) -> Result<bool, String> {
    if class_paused(class) || !optional_enabled(settings, class) {
        return Ok(false);
    }
    let stop = optional_stop();
    match class {
        WorkClass::Similarity => {
            let result = with_active(app, class, || {
                if foreground {
                    crate::similarity::rebuild_priority_bucket_cancellable(
                        conn,
                        &settings.similarity,
                        priority,
                        &stop,
                    )
                } else {
                    crate::similarity::rebuild_next_dirty_bucket_cancellable(
                        conn,
                        &settings.similarity,
                        &stop,
                    )
                }
            });
            match result {
                Ok(Some(Some(stats))) => {
                    emit_progress(app, class, None);
                    crate::failure_runtime::emit_or_record(
                        app,
                        "derived://similarity-updated",
                        json!({}),
                    );
                    logging::info(
                        "similarity rebuilt",
                        json!({
                            "bucket": stats.last_bucket,
                            "groups": stats.groups,
                            "items": stats.grouped_items,
                        }),
                    );
                    Ok(true)
                }
                Ok(Some(None)) | Ok(None) => Ok(false),
                Err(error) => {
                    if crate::resource_limits::is_safety_error(&error) {
                        pause_for_resource_safety(app, conn, class, &error)?;
                        Ok(false)
                    } else {
                        Err(error)
                    }
                }
            }
        }
        WorkClass::Snapshots => {
            let cursor = &mut cursors.snapshots;
            if !foreground && cursor.exhausted {
                return Ok(false);
            }
            let Some(ffmpeg) = settings.ffmpeg.as_deref() else {
                return Ok(false);
            };
            let stats = with_active(app, class, || {
                crate::video::derive_strips_pending(
                    conn,
                    cache,
                    ffmpeg,
                    &settings.temp_dir,
                    &settings.strip,
                    priority,
                    &|hash| crate::derived_runtime::active_item(app, class, hash),
                    &|hash| notify_item_update(app, conn, projection, "snapshots", hash, hash),
                    if foreground {
                        None
                    } else {
                        cursor.after_hash.as_deref()
                    },
                    &stop,
                    &progress(app, class),
                )
            })?
            .unwrap_or_default();
            if stats.attempted > 0 {
                notify_issues(app);
                if !foreground {
                    cursor.after_hash = stats.last_attempted_hash;
                }
                return Ok(true);
            }
            if !foreground && !stats.candidates_found {
                cursor.exhausted = true;
            }
            Ok(false)
        }
        WorkClass::Faces => {
            let cursor = &mut cursors.faces;
            if !foreground && cursor.exhausted {
                return Ok(false);
            }
            let Some(assets) = settings.face_models.as_ref() else {
                return Ok(false);
            };
            let result = with_active(app, class, || {
                crate::face::face_scores_pending(
                    conn,
                    cache,
                    Some((
                        assets.runtime.as_deref(),
                        assets.detector.as_path(),
                        assets.emotion.as_path(),
                    )),
                    priority,
                    |hash| crate::derived_runtime::active_item(app, class, hash),
                    |hash| notify_item_update(app, conn, projection, "faces", hash, hash),
                    |done, total| emit_progress(app, class, Some((done, total))),
                    if foreground {
                        None
                    } else {
                        cursor.after_hash.as_deref()
                    },
                    &stop,
                )
            });
            let stats = match result {
                Ok(value) => value.unwrap_or_default(),
                Err(error) if crate::resource_limits::is_safety_error(&error) => {
                    pause_for_resource_safety(app, conn, class, &error)?;
                    return Ok(false);
                }
                Err(error) => return Err(error),
            };
            if stats.attempted > 0 {
                notify_issues(app);
                if !foreground {
                    cursor.after_hash = stats.last_attempted_hash;
                }
                return Ok(true);
            }
            if !foreground && !stats.candidates_found {
                cursor.exhausted = true;
            }
            Ok(false)
        }
        WorkClass::VideoTranscripts | WorkClass::AudioTranscripts => {
            let cursor = match class {
                WorkClass::VideoTranscripts => &mut cursors.video_transcripts,
                WorkClass::AudioTranscripts => &mut cursors.audio_transcripts,
                _ => unreachable!(),
            };
            if !foreground && cursor.exhausted {
                return Ok(false);
            }
            if settings.transcription_model.is_none() || settings.ffmpeg.is_none() {
                return Ok(false);
            }
            let context = TranscriptContext {
                conn,
                cache,
                data_root: &settings.data_root,
                transcription_acceleration: settings.transcription_acceleration,
                app,
                projection,
            };
            let step = with_active(app, class, || {
                transcribe_next(
                    class,
                    &context,
                    priority,
                    if foreground {
                        None
                    } else {
                        cursor.after_hash.as_deref()
                    },
                    foreground,
                )
            })?
            .unwrap_or_default();
            if step.attempted_hash.is_some() {
                notify_issues(app);
                if !foreground {
                    cursor.after_hash = step.attempted_hash;
                }
                return Ok(true);
            }
            if !foreground {
                cursor.exhausted = step.exhausted;
            }
            Ok(false)
        }
        WorkClass::Previews => Ok(false),
    }
}

fn optional_enabled(settings: &Settings, class: WorkClass) -> bool {
    match class {
        WorkClass::Snapshots => settings.video_snapshots_enabled,
        WorkClass::Similarity => settings.similarity_enabled,
        WorkClass::Faces => settings.face_enabled,
        WorkClass::VideoTranscripts => settings.video_transcription_enabled,
        WorkClass::AudioTranscripts => settings.audio_transcription_enabled,
        WorkClass::Previews => true,
    }
}

pub fn priority_candidates(
    conn: &Connection,
    settings: &Settings,
    selected: Option<&str>,
    visible: &[String],
    section: Option<&SectionPriority>,
) -> Result<Vec<String>, String> {
    priority_candidates_for_class(
        conn,
        settings,
        WorkClass::Previews.id(),
        selected,
        visible,
        section,
    )
}

pub fn priority_candidates_for_class(
    conn: &Connection,
    settings: &Settings,
    class: &str,
    selected: Option<&str>,
    visible: &[String],
    section: Option<&SectionPriority>,
) -> Result<Vec<String>, String> {
    let class = WorkClass::parse(class)
        .ok_or_else(|| format!("unknown background-work class: {class}"))?;
    crate::derived_state::priority_candidates(
        conn,
        class,
        settings.capabilities(),
        selected,
        visible,
        section.map(|section| (section.kind.as_str(), section.start_ms, section.end_ms)),
        SECTION_HINT_LIMIT,
    )
}

fn emit_progress(app: &AppHandle, class: WorkClass, counts: Option<(u64, u64)>) {
    record_progress(app, class, counts);
}

/// Derived failures are current state in SQLite. One invalidation per
/// attempted batch keeps every frontend surface on that authority without a
/// polling loop or class-specific issue store.
fn notify_issues(app: &AppHandle) {
    crate::failure_runtime::emit_or_record(app, "derived://issues", json!({}));
}

pub(crate) fn pause_for_resource_safety(
    app: &AppHandle,
    conn: &Connection,
    class: WorkClass,
    error: &str,
) -> Result<(), String> {
    crate::derived_runtime::pause_for_safety(app, class)?;
    crate::index_store::upsert_issue(
        conn,
        None,
        &format!("resource-limit-{}", class.id()),
        crate::resource_limits::safety_message(error),
    )?;
    notify_issues(app);
    Ok(())
}

pub fn notify_item_update(
    app: &AppHandle,
    conn: &Connection,
    projection: crate::queries::ItemProjectionContext,
    class: &str,
    previous_hash: &str,
    hash: &str,
) {
    if crate::app_lifecycle::shutting_down() {
        return;
    }
    match crate::queries::item_by_hash(conn, hash, projection) {
        Ok(Some(item)) => {
            crate::failure_runtime::emit_or_record(
                app,
                "derived://item",
                json!({ "class": class, "previousHash": previous_hash, "item": item }),
            );
        }
        Ok(None) => {}
        Err(error) => logging::warn(
            "derived item notification failed",
            json!({ "hash": hash, "error": { "message": error } }),
        ),
    }
}

fn notify_image_changes(
    app: &AppHandle,
    conn: &Connection,
    projection: crate::queries::ItemProjectionContext,
    changes: &[(String, String)],
) {
    for (previous, current) in changes {
        notify_item_update(app, conn, projection, "previews", previous, current);
    }
}

fn notify_video_changes(
    app: &AppHandle,
    conn: &Connection,
    projection: crate::queries::ItemProjectionContext,
    hashes: &[String],
) {
    for hash in hashes {
        notify_item_update(app, conn, projection, "video-posters", hash, hash);
    }
}

fn progress(app: &AppHandle, class: WorkClass) -> impl Fn(u64, u64) + '_ {
    move |done, total| emit_progress(app, class, Some((done, total)))
}

#[derive(Default)]
struct TranscriptStep {
    attempted_hash: Option<String>,
    exhausted: bool,
}

struct TranscriptContext<'a> {
    conn: &'a Connection,
    cache: &'a CachePaths,
    data_root: &'a Path,
    transcription_acceleration: crate::ai_acceleration::Mode,
    app: &'a AppHandle,
    projection: crate::queries::ItemProjectionContext,
}

struct FinishSignal(std::sync::Arc<AtomicBool>);

impl Drop for FinishSignal {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub fn ensure_exact_identity(
    conn: &Connection,
    cache: &CachePaths,
    hash: &str,
    path: &Path,
) -> Result<String, String> {
    if !crate::scanner::is_provisional(hash) {
        return Ok(hash.to_string());
    }
    let _awake = crate::sleep_prevention::begin_work();
    let real = crate::hashing::full_hash_with_cancel(path, &cancelled).map_err(|error| {
        if error.kind() == std::io::ErrorKind::Interrupted {
            crate::scanner::CANCELLED.to_string()
        } else {
            format!("could not identify the file before transcription: {error}")
        }
    })?;
    crate::scanner::promote_identity(conn, cache, hash, &real)?;
    Ok(real)
}

pub struct TranscriptionAttempt<'a> {
    pub conn: &'a Connection,
    pub cache: &'a CachePaths,
    pub data_root: &'a Path,
    pub temp_dir: PathBuf,
    pub source_hash: &'a str,
    pub source_path: &'a str,
    pub replace_existing: bool,
    pub acceleration: crate::ai_acceleration::Mode,
    pub cancel_when: Option<Box<dyn Fn() -> bool + Send + 'static>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TranscriptionAttemptOutcome {
    Completed { hash: String, text: String },
    Cancelled { hash: String },
    Unavailable { hash: String, message: String },
    ResourceSafety { hash: String, message: String },
    Failed { hash: String, message: String },
}

enum TranscriptionPreparation {
    Ready(String),
    Terminal(TranscriptionAttemptOutcome),
}

fn prepare_transcription_attempt(
    attempt: &TranscriptionAttempt<'_>,
    on_identity: &mut impl FnMut(&str),
) -> Result<TranscriptionPreparation, String> {
    let hash = match ensure_exact_identity(
        attempt.conn,
        attempt.cache,
        attempt.source_hash,
        Path::new(attempt.source_path),
    ) {
        Ok(hash) => hash,
        Err(error) if error == crate::scanner::CANCELLED => {
            return Ok(TranscriptionPreparation::Terminal(
                TranscriptionAttemptOutcome::Cancelled {
                    hash: attempt.source_hash.to_string(),
                },
            ))
        }
        Err(error) => return Err(error),
    };
    on_identity(&hash);
    if attempt.cancel_when.as_ref().is_some_and(|stop| stop()) {
        return Ok(TranscriptionPreparation::Terminal(
            TranscriptionAttemptOutcome::Cancelled { hash },
        ));
    }

    if !attempt.replace_existing {
        let existing = crate::derived_state::transcript_result(attempt.conn, attempt.cache, &hash)?;
        if existing.status == crate::derived_state::READY {
            return Ok(TranscriptionPreparation::Terminal(
                TranscriptionAttemptOutcome::Completed {
                    hash,
                    text: existing.text.unwrap_or_default(),
                },
            ));
        }
    }
    Ok(TranscriptionPreparation::Ready(hash))
}

fn finish_transcription_attempt(
    attempt: &TranscriptionAttempt<'_>,
    hash: String,
    result: Result<String, String>,
) -> Result<TranscriptionAttemptOutcome, String> {
    let result = result.and_then(|text| {
        crate::transcription::publish_transcript(&attempt.cache.transcript(&hash), &text)?;
        Ok(text)
    });
    match result {
        Ok(text) => {
            crate::derived_state::record_transcript_success(
                attempt.conn,
                &hash,
                attempt.source_path,
                !text.trim().is_empty(),
            )?;
            Ok(TranscriptionAttemptOutcome::Completed { hash, text })
        }
        Err(error) if error == crate::scanner::CANCELLED => {
            Ok(TranscriptionAttemptOutcome::Cancelled { hash })
        }
        Err(error) if crate::resource_limits::is_safety_error(&error) => {
            Ok(TranscriptionAttemptOutcome::ResourceSafety {
                hash,
                message: error,
            })
        }
        Err(error) => {
            if attempt.replace_existing {
                crate::derived_state::record_transcript_replacement_failure(
                    attempt.conn,
                    attempt.source_path,
                    &error,
                )?;
            } else {
                crate::derived_state::record_transcript_failure(
                    attempt.conn,
                    &hash,
                    attempt.source_path,
                    &error,
                )?;
            }
            Ok(TranscriptionAttemptOutcome::Failed {
                hash,
                message: error,
            })
        }
    }
}

/// Complete one production transcription attempt. Manual and automatic callers
/// retain their different admission, priority, preemption, and UI-event
/// responsibilities; this operation owns the common identity, cache reuse,
/// dependency, engine claim, generation, durable-result, and terminal-
/// classification boundary.
pub fn complete_transcription_attempt(
    mut attempt: TranscriptionAttempt<'_>,
    mut on_identity: impl FnMut(&str),
    on_started: impl FnOnce(&str),
    mut on_progress: impl FnMut(&str, i32) + 'static,
) -> Result<TranscriptionAttemptOutcome, String> {
    let hash = match prepare_transcription_attempt(&attempt, &mut on_identity)? {
        TranscriptionPreparation::Ready(hash) => hash,
        TranscriptionPreparation::Terminal(outcome) => return Ok(outcome),
    };

    let dependencies = crate::ai_dependencies::production_transcription(attempt.data_root);
    let Some(model) = dependencies.model else {
        return Ok(TranscriptionAttemptOutcome::Unavailable {
            hash,
            message: "the transcription model is not installed — install it from Managed tools"
                .to_string(),
        });
    };
    let Some(ffmpeg) = dependencies.ffmpeg else {
        return Ok(TranscriptionAttemptOutcome::Unavailable {
            hash,
            message: "ffmpeg is not installed — install it from Managed tools".to_string(),
        });
    };
    if attempt.cancel_when.as_ref().is_some_and(|stop| stop()) {
        return Ok(TranscriptionAttemptOutcome::Cancelled { hash });
    }
    let claim = crate::transcription::claim()?;
    let _awake = crate::sleep_prevention::begin_work();
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let memory_pressure = Arc::new(AtomicBool::new(false));
    let pressure_signal = memory_pressure.clone();
    let finish_signal = FinishSignal(std::sync::Arc::clone(&finished));
    let watch = attempt
        .cancel_when
        .take()
        .map(|cancel_when| {
            std::thread::Builder::new()
                .name("onecopy-transcription-cancel-watch".to_string())
                .spawn(move || loop {
                    if finished.load(Ordering::SeqCst) {
                        return;
                    }
                    if cancel_when() {
                        crate::transcription::request_cancel();
                        return;
                    }
                    if let Err(error) = crate::resource_limits::require_available(
                        crate::resource_limits::MODEL_RUNNING_HEADROOM,
                        "Transcription",
                    ) {
                        logging::warn(
                            "transcription yielded memory headroom",
                            json!({ "error": { "message": error } }),
                        );
                        pressure_signal.store(true, Ordering::SeqCst);
                        crate::transcription::request_cancel();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                })
                .map_err(|error| format!("could not start transcription cancel watcher: {error}"))
        })
        .transpose()?;
    on_started(&hash);
    let progress_hash = hash.clone();

    let result = crate::transcription::generate_transcript_claimed(
        &claim,
        &attempt.temp_dir,
        &model,
        &ffmpeg,
        Path::new(attempt.source_path),
        attempt.acceleration,
        move |percent| on_progress(&progress_hash, percent),
    );
    drop(finish_signal);
    if let Some(watch) = watch {
        watch
            .join()
            .map_err(crate::failure_runtime::panic_message)?;
    }
    if memory_pressure.load(Ordering::SeqCst) {
        return Ok(TranscriptionAttemptOutcome::ResourceSafety {
            hash,
            message: "Transcription paused to leave memory available for other work. Resume it from Background Work when memory is available.".to_string(),
        });
    }
    let cancelled_hash = hash.clone();
    let outcome = crate::transcription::publish_if_active(&claim, || {
        finish_transcription_attempt(&attempt, hash, result)
    })?;
    Ok(outcome.unwrap_or(TranscriptionAttemptOutcome::Cancelled {
        hash: cancelled_hash,
    }))
}

/// Runs the same identity, cache, publication, receipt, replacement, and
/// terminal-classification operation with a caller-supplied inference step.
/// Production callers use [`complete_transcription_attempt`]; this narrow seam
/// lets integration tests replace native model execution without adding a
/// second persistence workflow or a runtime-selectable test provider.
pub fn complete_transcription_attempt_with_inference(
    attempt: TranscriptionAttempt<'_>,
    mut on_identity: impl FnMut(&str),
    on_started: impl FnOnce(&str),
    mut on_progress: impl FnMut(&str, i32),
    inference: impl FnOnce(&mut dyn FnMut(i32)) -> Result<String, String>,
) -> Result<TranscriptionAttemptOutcome, String> {
    let hash = match prepare_transcription_attempt(&attempt, &mut on_identity)? {
        TranscriptionPreparation::Ready(hash) => hash,
        TranscriptionPreparation::Terminal(outcome) => return Ok(outcome),
    };
    on_started(&hash);
    let progress_hash = hash.clone();
    let mut progress = |percent| on_progress(&progress_hash, percent);
    let result = inference(&mut progress);
    if attempt.cancel_when.as_ref().is_some_and(|stop| stop()) {
        return Ok(TranscriptionAttemptOutcome::Cancelled { hash });
    }
    finish_transcription_attempt(&attempt, hash, result)
}

fn transcribe_next(
    class: WorkClass,
    context: &TranscriptContext<'_>,
    priority_hashes: &[String],
    after_hash: Option<&str>,
    _foreground: bool,
) -> Result<TranscriptStep, String> {
    let kind = class
        .content_kind()
        .ok_or_else(|| "transcription work has no media kind".to_string())?;
    let rows = if priority_hashes.is_empty() {
        crate::derived_state::transcript_candidates(
            context.conn,
            kind,
            after_hash,
            crate::derived_state::TRANSCRIPT_CANDIDATE_PAGE_SIZE,
        )?
    } else {
        crate::derived_state::prioritized_transcript_candidates(
            context.conn,
            kind,
            priority_hashes,
            crate::derived_state::TRANSCRIPT_CANDIDATE_PAGE_SIZE,
        )?
    };
    let Some((candidate_hash, path)) = rows.into_iter().next() else {
        return Ok(TranscriptStep {
            exhausted: true,
            ..TranscriptStep::default()
        });
    };

    let result = complete_transcription_attempt(
        TranscriptionAttempt {
            conn: context.conn,
            cache: context.cache,
            data_root: context.data_root,
            temp_dir: context
                .data_root
                .join(crate::binaries_manager::TEMP_DIR_NAME),
            source_hash: &candidate_hash,
            source_path: &path,
            replace_existing: false,
            acceleration: context.transcription_acceleration,
            cancel_when: Some(Box::new(optional_stop())),
        },
        |hash| {
            if candidate_hash != hash {
                notify_item_update(
                    context.app,
                    context.conn,
                    context.projection,
                    class.id(),
                    &candidate_hash,
                    hash,
                );
            }
        },
        |hash| {
            crate::derived_runtime::active_item(context.app, class, hash);
            emit_progress(context.app, class, None);
            // Extraction and model loading precede Whisper's first percentage.
            crate::failure_runtime::emit_or_record(
                context.app,
                "transcribe://progress",
                json!({ "hash": hash, "percent": 0 }),
            );
        },
        {
            let progress_handle = context.app.clone();
            move |progress_hash, percent| {
                let percent = percent.clamp(0, 100);
                record_progress(&progress_handle, class, Some((percent as u64, 100)));
                crate::failure_runtime::emit_or_record(
                    &progress_handle,
                    "transcribe://progress",
                    json!({ "hash": progress_hash, "percent": percent }),
                );
            }
        },
    );
    let result = match result {
        Err(error) if error == crate::transcription::TRANSCRIPTION_BUSY => {
            return Ok(TranscriptStep::default())
        }
        other => other?,
    };
    match result {
        TranscriptionAttemptOutcome::Completed { hash, text } => {
            notify_item_update(
                context.app,
                context.conn,
                context.projection,
                "transcripts",
                &hash,
                &hash,
            );
            crate::failure_runtime::emit_or_record(
                context.app,
                "transcribe://done",
                json!({ "hash": hash, "text": text }),
            );
            Ok(TranscriptStep {
                attempted_hash: Some(hash),
                exhausted: false,
            })
        }
        TranscriptionAttemptOutcome::Cancelled { hash } => {
            logging::debug(
                "derived transcription stopped",
                json!({ "hash": hash, "reason": "cancelled" }),
            );
            crate::failure_runtime::emit_or_record(
                context.app,
                "transcribe://cancelled",
                json!({ "hash": hash }),
            );
            Ok(TranscriptStep::default())
        }
        TranscriptionAttemptOutcome::Unavailable { message, .. } => {
            logging::debug(
                "derived transcription unavailable",
                json!({ "message": message }),
            );
            Ok(TranscriptStep::default())
        }
        TranscriptionAttemptOutcome::ResourceSafety { message, .. } => {
            pause_for_resource_safety(context.app, context.conn, class, &message)?;
            Ok(TranscriptStep::default())
        }
        TranscriptionAttemptOutcome::Failed { hash, message } => {
            notify_item_update(
                context.app,
                context.conn,
                context.projection,
                "transcripts",
                &hash,
                &hash,
            );
            logging::debug(
                "derived transcription failed",
                json!({ "hash": hash, "error": { "message": message } }),
            );
            crate::failure_runtime::emit_or_record(
                context.app,
                "transcribe://error",
                json!({ "hash": hash, "message": message }),
            );
            Ok(TranscriptStep {
                attempted_hash: Some(hash),
                exhausted: false,
            })
        }
    }
}
