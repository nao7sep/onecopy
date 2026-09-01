//! The single owner of reconstructible media work. Indexing records file and
//! time facts, then wakes this coordinator; previews, video posters, scene
//! strips, transcripts, face scores, and similarity are never scan phases.
//!
//! Preview/poster work runs in bounded fair turns. Independent native image
//! conversions may share a turn under live CPU and memory budgets; database
//! publication, ffmpeg routes, and every other heavy class remain serialized.
//! Work near selected and visible items may run while the user is active;
//! global backlogs wait for idle time. All classes re-read settings for each
//! pass, so installing a tool or changing a feature takes effect on the next
//! wake without restarting either indexing or the app.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock};

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

static LAST_ACTIVITY_MS: AtomicI64 = AtomicI64::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static AUTOMATIC_ADMITTED: AtomicBool = AtomicBool::new(false);
static WAKE: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
static PRIORITY: LazyLock<Mutex<PriorityHints>> =
    LazyLock::new(|| Mutex::new(PriorityHints::default()));

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
    selected: Option<String>,
    visible: Vec<String>,
    section: Option<SectionPriority>,
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
    pub face_models: Option<(PathBuf, PathBuf)>,
    pub transcription_model: Option<PathBuf>,
    pub video_transcription_enabled: bool,
    pub audio_transcription_enabled: bool,
    pub temp_dir: PathBuf,
}

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

pub fn settings_from_config(config: Option<&serde_json::Value>, data_root: &Path) -> Settings {
    let defaults = crate::storage::DefaultConfig::default();
    let get = |key: &str| config.and_then(|c| c.get(key));
    let u32_of = |key: &str, fallback: u32| -> u32 {
        get(key)
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(fallback)
    };
    let installed = |id: &str| {
        crate::binaries_manager::spec_of(id).and_then(|spec| {
            let state = crate::binaries_manager::state_of(data_root, spec);
            (state.status != crate::binaries::BinaryStatus::NotInstalled)
                .then(|| crate::binaries_manager::installed_path(data_root, spec))
        })
    };
    let score_faces = get("scoreFaces")
        .and_then(|v| v.as_bool())
        .unwrap_or(defaults.score_faces);
    let bool_of = |key: &str, fallback: bool| {
        get(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(fallback)
    };

    Settings {
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
        ffmpeg: {
            let path = crate::binaries_manager::ffmpeg_path(data_root);
            path.is_file().then_some(path)
        },
        video_snapshots_enabled: bool_of("videoSnapshotsEnabled", defaults.video_snapshots_enabled),
        similarity_enabled: bool_of(
            "similarPhotoAnalysisEnabled",
            defaults.similar_photo_analysis_enabled,
        ),
        face_enabled: score_faces,
        face_models: score_faces
            .then(|| installed("ultraface-rfb640").zip(installed("hsemotion-enet-b2")))
            .flatten(),
        transcription_model: installed("whisper-large-v3-turbo"),
        video_transcription_enabled: bool_of(
            "videoTranscriptionEnabled",
            defaults.video_transcription_enabled,
        ),
        audio_transcription_enabled: bool_of(
            "audioTranscriptionEnabled",
            defaults.audio_transcription_enabled,
        ),
        temp_dir: data_root.join(crate::binaries_manager::TEMP_DIR_NAME),
    }
}

pub fn work_capabilities(
    data_root: &Path,
) -> Result<crate::derived_state::WorkCapabilities, String> {
    let config = crate::storage::read_config_for_setup(data_root)?;
    let settings = settings_from_config(config.as_ref(), data_root);
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
    AUTOMATIC_ADMITTED.load(Ordering::SeqCst)
        && !crate::derived_runtime::exclusive()
        && !crate::scan_runtime::running()
}

/// Opens the automatic media queue only after the launch source decision has
/// reached its terminal boundary. Starting the worker before this point keeps
/// startup cheap, but it must not enrich stale rows before source
/// reconciliation has had the first chance to retire them.
pub(crate) fn admit_automatic() {
    AUTOMATIC_ADMITTED.store(true, Ordering::SeqCst);
    wake();
}

/// Wake after index, settings, tool, priority, or lifecycle changes. Durable
/// source triggers own derived invalidation; this signal only schedules work.
pub fn wake() {
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
    section: Option<SectionPriority>,
) {
    let required_changed = crate::derived_runtime::automatic_optional_active()
        && required_priority_pending(selected.as_deref(), &visible);
    match PRIORITY.lock() {
        Ok(mut hints) => {
            hints.selected = selected;
            hints.visible = visible.into_iter().take(SECTION_HINT_LIMIT).collect();
            hints.section = section;
        }
        Err(_) => logging::error("derived-work priority state is unavailable", json!({})),
    }
    if required_changed {
        crate::derived_runtime::preempt_automatic_optional_for_required();
    }
    wake();
}

fn required_priority_pending(selected: Option<&str>, visible: &[String]) -> bool {
    let Some(data_root) = crate::DATA_ROOT.get() else {
        return false;
    };
    let Ok(config) = crate::storage::read_config_for_setup(data_root) else {
        return false;
    };
    let settings = settings_from_config(config.as_ref(), data_root);
    let Ok(conn) = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))
    else {
        return false;
    };
    priority_candidates_for_class(
        &conn,
        &settings,
        WorkClass::Previews.id(),
        selected,
        visible,
        None,
    )
    .is_ok_and(|candidates| !candidates.is_empty())
}

pub fn start(app: AppHandle) -> Result<bool, String> {
    if STARTED.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    let handle = app.clone();
    std::thread::Builder::new()
        .name("onecopy-derived-work".to_string())
        .spawn(move || derived_worker(handle))
        .map_err(|error| {
            STARTED.store(false, Ordering::SeqCst);
            format!("could not start previews-and-analysis worker: {error}")
        })?;
    wake();
    Ok(true)
}

pub fn started() -> bool {
    STARTED.load(Ordering::SeqCst)
}

fn derived_worker(app: AppHandle) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_worker_loop(&app)));
    STARTED.store(false, Ordering::SeqCst);
    let failure = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(error)) => error,
        Err(payload) => crate::failure_runtime::panic_message(payload),
    };
    let _ = crate::failure_runtime::report(
        &app,
        crate::issue_recovery::DERIVED_WORKER_FAILED,
        None,
        &failure,
    );
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
    let mut cleared_previous_failure = false;
    loop {
        if !run_again {
            let value = generation
                .lock()
                .map_err(|_| "derived-work wake state is unavailable".to_string())?;
            let (current, _) = ready
                .wait_timeout_while(
                    value,
                    std::time::Duration::from_secs(POLL_SECONDS),
                    |current| *current == observed,
                )
                .map_err(|_| "derived-work wake state is unavailable".to_string())?;
            if *current != observed {
                cursors = CandidateCursors::default();
            }
            observed = *current;
        } else {
            let current = *generation
                .lock()
                .map_err(|_| "derived-work wake state is unavailable".to_string())?;
            if current != observed {
                cursors = CandidateCursors::default();
                observed = current;
            }
        }
        run_again = false;
        if !available() {
            continue;
        }
        let Some(pass) = crate::scan_runtime::try_with_derived_claim(|| {
            run_one_pass(app, &mut cursors)
        }) else {
            continue;
        };
        match pass {
            Ok(did_work) => {
                if !cleared_previous_failure {
                    crate::failure_runtime::clear(
                        app,
                        crate::issue_recovery::DERIVED_WORKER_FAILED,
                        None,
                    )?;
                    cleared_previous_failure = true;
                }
                run_again = did_work;
            }
            Err(error) if error.starts_with(crate::scanner::CANCELLED) => {
                logging::debug("derived work stopped", json!({ "reason": "cancelled" }))
            }
            Err(error) => return Err(error),
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
) -> Result<String, String> {
    let _active = crate::derived_runtime::begin_manual(app, WorkClass::Previews.id())?;
    crate::derived_runtime::active_item(app, WorkClass::Previews, hash);
    let settings = settings_from_config(config, data_root);
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

/// One bounded pass. Settings and SQLite are opened once per batch, while
/// every media item is still independently claimed and checkpointed. This
/// avoids reopening both millions of times without holding stale settings
/// indefinitely.
fn run_one_pass(app: &AppHandle, cursors: &mut CandidateCursors) -> Result<bool, String> {
    let data_root = crate::DATA_ROOT.get().ok_or("data root unset")?.clone();
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = settings_from_config(config.as_ref(), &data_root);
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
    let visible_previews = priority_candidates_for_class(
        &conn,
        &settings,
        WorkClass::Previews.id(),
        hints.selected.as_deref(),
        &hints.visible,
        None,
    )?;
    if derive_priority_previews(
        app,
        &conn,
        &cache,
        &settings,
        projection,
        &visible_previews,
        VISIBLE_PREVIEW_TURN,
    )? {
        return Ok(true);
    }

    if run_priority_optional_turn(
        app,
        &conn,
        &cache,
        &settings,
        projection,
        cursors,
        hints.selected.as_deref(),
        &hints.visible,
        None,
    )? {
        return Ok(true);
    }

    let section_previews = priority_candidates_for_class(
        &conn,
        &settings,
        WorkClass::Previews.id(),
        None,
        &[],
        hints.section.as_ref(),
    )?;
    if derive_priority_previews(
        app,
        &conn,
        &cache,
        &settings,
        projection,
        &section_previews,
        SECTION_PREVIEW_TURN,
    )? {
        return Ok(true);
    }

    if run_priority_optional_turn(
        app,
        &conn,
        &cache,
        &settings,
        projection,
        cursors,
        None,
        &[],
        hints.section.as_ref(),
    )? {
        return Ok(true);
    }

    if !is_idle() {
        crate::failure_runtime::emit_or_record(app, "derived://quiet", json!({}));
        return Ok(false);
    }

    let required = derive_global_required(app, &conn, &cache, &settings, projection)?;
    let optional = run_global_optional_turn(app, &conn, &cache, &settings, projection, cursors)?;
    let did_work = required || optional;
    if !did_work {
        crate::failure_runtime::emit_or_record(app, "derived://quiet", json!({}));
        emit_state_changed(app);
    }
    Ok(did_work)
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
        if !available() {
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
    let image = with_active(app, WorkClass::Previews, || {
        crate::preview::derive_next_images(
            conn,
            cache,
            settings.thumb_edge,
            settings.preview_long_edge,
            settings.ffmpeg.as_deref(),
            true,
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
    if !available() {
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
    let stop = || cancelled() || (!foreground && !is_idle());
    match class {
        WorkClass::Similarity => {
            let result = with_active(app, class, || {
                crate::similarity::rebuild_next_dirty_bucket_for_root_cancellable(
                    conn,
                    &settings.similarity,
                    &settings.data_root,
                    &stop,
                )
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
            let Some((detector, emotion)) = settings.face_models.as_ref() else {
                return Ok(false);
            };
            let result = with_active(app, class, || {
                crate::face::face_scores_pending(
                    conn,
                    cache,
                    Some((detector.as_path(), emotion.as_path())),
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
            let (Some(model), Some(ffmpeg)) = (
                settings.transcription_model.as_deref(),
                settings.ffmpeg.as_deref(),
            ) else {
                return Ok(false);
            };
            let context = TranscriptContext {
                conn,
                cache,
                temp_dir: &settings.temp_dir,
                model,
                ffmpeg,
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
    temp_dir: &'a Path,
    model: &'a Path,
    ffmpeg: &'a Path,
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

fn transcribe_next(
    class: WorkClass,
    context: &TranscriptContext<'_>,
    priority_hashes: &[String],
    after_hash: Option<&str>,
    foreground: bool,
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

    let hash = ensure_exact_identity(
        context.conn,
        context.cache,
        &candidate_hash,
        Path::new(&path),
    )?;
    if crate::derived_state::transcript_result(context.conn, context.cache, &hash)?.status
        == crate::derived_state::READY
    {
        return Ok(TranscriptStep {
            attempted_hash: Some(hash),
            exhausted: false,
        });
    }
    if !foreground && !is_idle() {
        return Ok(TranscriptStep::default());
    }
    let claim = match crate::transcription::claim() {
        Ok(claim) => claim,
        Err(error) if error == crate::transcription::TRANSCRIPTION_BUSY => {
            return Ok(TranscriptStep::default())
        }
        Err(error) => return Err(error),
    };
    crate::derived_runtime::active_item(context.app, class, &hash);
    if candidate_hash != hash {
        notify_item_update(
            context.app,
            context.conn,
            context.projection,
            class.id(),
            &candidate_hash,
            &hash,
        );
    }
    emit_progress(context.app, class, None);
    // Audio extraction and model loading happen before Whisper's first
    // percentage callback; publish ownership now so an open video never
    // looks pending while its expensive work is already underway.
    crate::failure_runtime::emit_or_record(
        context.app,
        "transcribe://progress",
        json!({ "hash": hash, "percent": 0 }),
    );
    let finished = std::sync::Arc::new(AtomicBool::new(false));
    let finish_signal = FinishSignal(std::sync::Arc::clone(&finished));
    let watch = std::thread::Builder::new()
        .name("onecopy-transcription-cancel-watch".to_string())
        .spawn({
            let finished = std::sync::Arc::clone(&finished);
            move || loop {
                if finished.load(Ordering::SeqCst) {
                    return;
                }
                if (!foreground && !is_idle()) || cancelled() {
                    crate::transcription::request_cancel();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
        .map_err(|error| format!("could not start transcription cancel watcher: {error}"))?;
    let result = crate::transcription::transcribe_to_cache_claimed(
        &claim,
        context.cache,
        context.temp_dir,
        Some(context.model),
        Some(context.ffmpeg),
        Path::new(&path),
        &hash,
        false,
        {
            let progress_handle = context.app.clone();
            let progress_hash = hash.clone();
            move |percent| {
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
    drop(finish_signal);
    watch
        .join()
        .map_err(crate::failure_runtime::panic_message)?;
    // The claim resets cancellation when dropped, so classify this run while
    // it still owns the Whisper slot.
    let was_cancelled = crate::transcription::is_cancelled();
    drop(claim);
    match result {
        Ok(text) => {
            crate::derived_state::record_transcript_success(
                context.conn,
                &hash,
                &path,
                !text.trim().is_empty(),
            )?;
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
        Err(error) if error == crate::scanner::CANCELLED || was_cancelled => {
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
        Err(error) if crate::resource_limits::is_safety_error(&error) => {
            pause_for_resource_safety(context.app, context.conn, class, &error)?;
            Ok(TranscriptStep::default())
        }
        Err(error) => {
            crate::derived_state::record_transcript_failure(context.conn, &hash, &path, &error)?;
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
                json!({ "hash": hash, "error": { "message": error } }),
            );
            crate::failure_runtime::emit_or_record(
                context.app,
                "transcribe://error",
                json!({ "hash": hash, "message": error }),
            );
            Ok(TranscriptStep {
                attempted_hash: Some(hash),
                exhausted: false,
            })
        }
    }
}
