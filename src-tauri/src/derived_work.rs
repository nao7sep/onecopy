//! The single owner of reconstructible media work. Indexing records file and
//! time facts, then wakes this coordinator; previews, video posters, scene
//! strips, transcripts, face scores, and similarity are never scan phases.
//!
//! Preview/poster work runs one item at a time in bounded fair batches and may run while the
//! user is active so newly indexed media becomes visible. Expensive optional
//! work runs only after a minute without input. All classes re-read settings
//! for each pass, so installing a tool or changing a feature takes effect on
//! the next wake without restarting either indexing or the app.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock};

use rusqlite::{params_from_iter, Connection};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::derived_runtime::{
    cancelled, emit_state_changed, is_paused as class_paused, progress as record_progress,
    with_active,
};
use crate::derived_state::WorkClass;
use crate::logging;
use crate::preview::CachePaths;

static LAST_ACTIVITY_MS: AtomicI64 = AtomicI64::new(0);
static HEAVY_OPS: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static SIMILARITY_DIRTY: AtomicBool = AtomicBool::new(true);
static WAKE: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
static MEDIA_WORK: Mutex<()> = Mutex::new(());
static PRIORITY: LazyLock<Mutex<PriorityHints>> =
    LazyLock::new(|| Mutex::new(PriorityHints::default()));

const IDLE_AFTER_MS: i64 = 60_000;
const POLL_SECONDS: u64 = 15;
const IMAGE_BATCH: usize = 64;
const VIDEO_BATCH: usize = 8;
const PRIORITY_BATCH: usize = 64;
const SECTION_HINT_LIMIT: usize = 256;

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
    transcripts: CandidateCursor,
    faces: CandidateCursor,
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
    pub face_enabled: bool,
    pub face_models: Option<(PathBuf, PathBuf)>,
    pub temp_dir: PathBuf,
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
    let score_faces = get("scoreFaces").and_then(|v| v.as_bool()).unwrap_or(false);

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
        face_enabled: score_faces,
        face_models: score_faces
            .then(|| installed("ultraface-rfb640").zip(installed("hsemotion-enet-b2")))
            .flatten(),
        temp_dir: data_root.join(crate::binaries_manager::TEMP_DIR_NAME),
    }
}

pub fn work_capabilities(data_root: &Path) -> Result<crate::derived_state::WorkCapabilities, String> {
    let config = crate::storage::read_config_for_setup(data_root)?;
    let settings = settings_from_config(config.as_ref(), data_root);
    Ok(crate::derived_state::WorkCapabilities {
        ffmpeg: settings.ffmpeg.is_some(),
        face_enabled: settings.face_enabled,
        face_models: settings.face_models.is_some(),
        transcripts: settings.ffmpeg.is_some() && whisper_model(data_root).is_some(),
    })
}

pub fn note_activity() {
    LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
}

pub struct HeavyOp;

pub fn heavy_op() -> HeavyOp {
    HEAVY_OPS.fetch_add(1, Ordering::SeqCst);
    HeavyOp
}

impl Drop for HeavyOp {
    fn drop(&mut self) {
        HEAVY_OPS.fetch_sub(1, Ordering::SeqCst);
        wake(false);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn is_idle() -> bool {
    now_ms() - LAST_ACTIVITY_MS.load(Ordering::SeqCst) >= IDLE_AFTER_MS
        && HEAVY_OPS.load(Ordering::SeqCst) == 0
        && !crate::scan_runtime::running()
}

pub(crate) fn available() -> bool {
    HEAVY_OPS.load(Ordering::SeqCst) == 0 && !crate::scan_runtime::running()
}

pub(crate) fn similarity_dirty() -> bool {
    SIMILARITY_DIRTY.load(Ordering::SeqCst)
}

/// Wake after index, settings, tool, priority, or lifecycle changes. Index
/// changes also invalidate the process-local similarity cohort.
pub fn wake(index_changed: bool) {
    if index_changed {
        SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
    }
    let (generation, ready) = WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()));
    if let Ok(mut value) = generation.lock() {
        *value = value.wrapping_add(1);
        ready.notify_one();
    }
}

/// Replaces the current UI priority hints. They are deliberately ephemeral:
/// output absence remains the queue and a restart needs no job recovery.
pub fn set_priority(
    selected: Option<String>,
    visible: Vec<String>,
    section: Option<SectionPriority>,
) {
    if let Ok(mut hints) = PRIORITY.lock() {
        hints.selected = selected;
        hints.visible = visible.into_iter().take(SECTION_HINT_LIMIT).collect();
        hints.section = section;
    }
    wake(false);
}

pub fn start(app: AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let (generation, ready) = WAKE.get_or_init(|| (Mutex::new(0), Condvar::new()));
        let mut observed = 0u64;
        let mut run_again = true;
        let mut cursors = CandidateCursors::default();
        loop {
            if !run_again {
                if let Ok(value) = generation.lock() {
                    let waited = ready
                        .wait_timeout_while(
                            value,
                            std::time::Duration::from_secs(POLL_SECONDS),
                            |current| *current == observed,
                        )
                        .map(|(current, _)| *current);
                    if let Ok(current) = waited {
                        if current != observed {
                            cursors = CandidateCursors::default();
                        }
                        observed = current;
                    }
                }
            } else if let Ok(current) = generation.lock().map(|current| *current) {
                if current != observed {
                    observed = current;
                    cursors = CandidateCursors::default();
                }
            }
            run_again = false;
            if !available() {
                continue;
            }
            match run_one_pass(&app, &mut cursors) {
                Ok(did_work) => run_again = did_work,
                Err(error) if error.starts_with(crate::scanner::CANCELLED) => logging::debug(
                    "derived work stopped",
                    json!({ "reason": "cancelled" }),
                ),
                Err(error) => logging::warn(
                    "derived work pass failed",
                    json!({ "error": { "message": error } }),
                ),
            }
        }
    });
    wake(true);
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
    let _claim = MEDIA_WORK
        .lock()
        .map_err(|_| "derived media owner is unavailable".to_string())?;
    let settings = settings_from_config(config, data_root);
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let cache = CachePaths::new(settings.cache_root.clone());
    let canonical = crate::preview::derive_one(
        &conn,
        &cache,
        settings.thumb_edge,
        settings.preview_long_edge,
        settings.ffmpeg.as_deref(),
        hash,
    )?;
    SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
    wake(false);
    Ok(canonical)
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
    let cache = CachePaths::new(settings.cache_root.clone());

    let hints = PRIORITY
        .lock()
        .map(|hints| hints.clone())
        .unwrap_or_default();
    let priority = priority_candidates(
        &conn,
        &settings,
        hints.selected.as_deref(),
        &hints.visible,
        hints.section.as_ref(),
    )?;
    let mut priority_done = 0usize;
    for hash in priority {
        if !available() {
            return Ok(false);
        }
        let image = with_active(app, WorkClass::Previews, || {
            let _claim = MEDIA_WORK
                .lock()
                .map_err(|_| "derived media owner is unavailable".to_string())?;
            crate::preview::derive_image_hash(
                &conn,
                &cache,
                settings.thumb_edge,
                settings.preview_long_edge,
                settings.ffmpeg.as_deref(),
                &hash,
            )
        })?
        .unwrap_or_default();
        if image.derived + image.failed + image.blocked_no_ffmpeg > 0 {
            if image.derived > 0 {
                SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
            }
            emit_progress(app, WorkClass::Previews, None);
            notify_image_changes(app, &conn, &image.changes);
            let _ = app.emit("derived://issues", json!({}));
            priority_done += 1;
        } else {
            let video = with_active(app, WorkClass::Previews, || {
                let _claim = MEDIA_WORK
                    .lock()
                    .map_err(|_| "derived media owner is unavailable".to_string())?;
                crate::video::derive_video_hash(
                    &conn,
                    &cache,
                    settings.ffmpeg.as_deref(),
                    &settings.temp_dir,
                    settings.thumb_edge,
                    settings.preview_long_edge,
                    &hash,
                )
            })?
            .unwrap_or_default();
            if video.derived + video.failed > 0 {
                emit_progress(app, WorkClass::Previews, None);
                notify_video_changes(app, &conn, &video.changed_hashes);
                let _ = app.emit("derived://issues", json!({}));
                priority_done += 1;
            }
        }
        if priority_done == PRIORITY_BATCH {
            return Ok(true);
        }
    }

    let mut image_budget_full = false;
    for index in 0..IMAGE_BATCH {
        if !available() {
            return Ok(false);
        }
        let image = with_active(app, WorkClass::Previews, || {
            let _claim = MEDIA_WORK
                .lock()
                .map_err(|_| "derived media owner is unavailable".to_string())?;
            crate::preview::derive_next_image(
                &conn,
                &cache,
                settings.thumb_edge,
                settings.preview_long_edge,
                settings.ffmpeg.as_deref(),
            )
        })?
        .unwrap_or_default();
        if image.derived + image.failed + image.blocked_no_ffmpeg == 0 {
            break;
        }
        if image.derived > 0 {
            SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
        }
        emit_progress(app, WorkClass::Previews, None);
        notify_image_changes(app, &conn, &image.changes);
        let _ = app.emit("derived://issues", json!({}));
        if index + 1 == IMAGE_BATCH {
            image_budget_full = true;
        }
    }

    let mut video_budget_full = false;
    for index in 0..VIDEO_BATCH {
        if !available() {
            return Ok(false);
        }
        let video = with_active(app, WorkClass::Previews, || {
            let _claim = MEDIA_WORK
                .lock()
                .map_err(|_| "derived media owner is unavailable".to_string())?;
            crate::video::derive_next_video(
                &conn,
                &cache,
                settings.ffmpeg.as_deref(),
                &settings.temp_dir,
                settings.thumb_edge,
                settings.preview_long_edge,
            )
        })?
        .unwrap_or_default();
        if video.derived + video.failed == 0 {
            break;
        }
        emit_progress(app, WorkClass::Previews, None);
        notify_video_changes(app, &conn, &video.changed_hashes);
        let _ = app.emit("derived://issues", json!({}));
        if index + 1 == VIDEO_BATCH {
            video_budget_full = true;
        }
    }

    if image_budget_full || video_budget_full {
        return Ok(true);
    }

    if !class_paused(WorkClass::Similarity) && SIMILARITY_DIRTY.load(Ordering::SeqCst) {
        let result = with_active(app, WorkClass::Similarity, || {
            // Clear only after claiming the owner. Preview work that completes
            // during this rebuild sets the bit again and therefore cannot be
            // lost behind this cohort's result.
            SIMILARITY_DIRTY.store(false, Ordering::SeqCst);
            crate::similarity::rebuild_groups_for_root_cancellable(
                &conn,
                &settings.similarity,
                &settings.data_root,
                &cancelled,
            )
        });
        match result {
            Ok(Some(stats)) => {
                emit_progress(app, WorkClass::Similarity, None);
                let _ = app.emit("derived://similarity-updated", json!({}));
                logging::info(
                    "similarity rebuilt",
                    json!({ "groups": stats.groups, "items": stats.grouped_items }),
                );
                return Ok(true);
            }
            Ok(None) => return Ok(false),
            Err(error) => {
                SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
                return Err(error);
            }
        }
    }

    if !is_idle() {
        let _ = app.emit("derived://quiet", json!({}));
        return Ok(false);
    }

    let stop = || !is_idle() || cancelled();
    if !class_paused(WorkClass::Snapshots) && !cursors.snapshots.exhausted {
        let stats = if let Some(ffmpeg) = settings.ffmpeg.as_deref() {
            with_active(app, WorkClass::Snapshots, || {
                crate::video::derive_strips_pending(
                    &conn,
                    &cache,
                    ffmpeg,
                    &settings.temp_dir,
                    &settings.strip,
                    cursors.snapshots.after_hash.as_deref(),
                    &stop,
                    &progress(app, WorkClass::Snapshots),
                )
            })?
            .unwrap_or_default()
        } else {
            crate::video::StripDeriveStats::default()
        };
        if stats.attempted > 0 {
            cursors.snapshots.after_hash = stats.last_attempted_hash;
            return Ok(true);
        }
        if !stats.candidates_found {
            cursors.snapshots.exhausted = true;
        }
    }

    let whisper = whisper_model(&data_root);
    if !class_paused(WorkClass::Transcripts) && !cursors.transcripts.exhausted {
        if let (Some(model), Some(ffmpeg)) = (whisper.as_deref(), settings.ffmpeg.as_deref()) {
            let step = with_active(app, WorkClass::Transcripts, || {
                transcribe_next(
                    &conn,
                    &cache,
                    model,
                    ffmpeg,
                    app,
                    cursors.transcripts.after_hash.as_deref(),
                )
            })?
            .unwrap_or_default();
            if step.attempted_hash.is_some() {
                cursors.transcripts.after_hash = step.attempted_hash;
                return Ok(true);
            }
            cursors.transcripts.exhausted = step.exhausted;
        }
    }

    if !class_paused(WorkClass::Faces) && !cursors.faces.exhausted {
        if let Some((detector, emotion)) = settings.face_models.as_ref() {
            let stats = with_active(app, WorkClass::Faces, || {
                crate::face::face_scores_pending(
                    &conn,
                    &cache,
                    Some((detector.as_path(), emotion.as_path())),
                    |done, total| emit_progress(app, WorkClass::Faces, Some((done, total))),
                    cursors.faces.after_hash.as_deref(),
                    &stop,
                )
            })?
            .unwrap_or_default();
            if stats.attempted > 0 {
                cursors.faces.after_hash = stats.last_attempted_hash;
                return Ok(true);
            }
            if !stats.candidates_found {
                cursors.faces.exhausted = true;
            }
        }
    }

    let _ = app.emit("derived://quiet", json!({}));
    emit_state_changed(app);
    Ok(false)
}

pub fn priority_candidates(
    conn: &Connection,
    settings: &Settings,
    selected: Option<&str>,
    visible: &[String],
    section: Option<&SectionPriority>,
) -> Result<Vec<String>, String> {
    let mut hinted = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |hash: &str| {
        if seen.insert(hash.to_string()) {
            hinted.push(hash.to_string());
        }
    };
    if let Some(selected) = selected {
        push(selected);
    }
    for hash in visible {
        push(hash);
    }

    let mut hashes = pending_hint_hashes(conn, settings, &hinted)?;
    let mut seen: HashSet<String> = hashes.iter().cloned().collect();

    let Some(section) = section else {
        return Ok(hashes);
    };
    if !matches!(section.kind.as_str(), "image" | "video") {
        return Ok(hashes);
    }

    let (image_pending, video_pending) =
        crate::derived_state::preview_pending_predicates(settings.ffmpeg.is_some());
    let time_clause = if section.start_ms.is_some() {
        "AND l.resolved_utc_ms >= ?2 AND l.resolved_utc_ms < ?3"
    } else {
        "AND l.resolved_utc_ms IS NULL"
    };
    let sql = format!(
        "SELECT l.content_hash FROM logical_contents l \
         JOIN contents c ON c.hash = l.content_hash \
         WHERE l.kind = ?1 {time_clause} \
           AND ((l.kind = 'image' AND {image_pending}) \
                OR (l.kind = 'video' AND {video_pending})) \
         ORDER BY l.resolved_utc_ms, l.representative_path_id \
         LIMIT {SECTION_HINT_LIMIT}"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let section_hashes: Vec<String> = match (section.start_ms, section.end_ms) {
        (Some(start), Some(end)) => statement
            .query_map(rusqlite::params![section.kind, start, end], |row| {
                row.get(0)
            })
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect(),
        _ => statement
            .query_map([&section.kind], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect(),
    };
    for hash in section_hashes {
        if seen.insert(hash.clone()) {
            hashes.push(hash);
        }
    }
    Ok(hashes)
}

fn pending_hint_hashes(
    conn: &Connection,
    settings: &Settings,
    hinted: &[String],
) -> Result<Vec<String>, String> {
    if hinted.is_empty() {
        return Ok(Vec::new());
    }
    let values = (1..=hinted.len())
        .map(|index| format!("(?{index}, {})", index - 1))
        .collect::<Vec<_>>()
        .join(", ");
    let (image_pending, video_pending) =
        crate::derived_state::preview_pending_predicates(settings.ffmpeg.is_some());
    let sql = format!(
        "WITH hinted(hash, priority) AS (VALUES {values}) \
         SELECT h.hash FROM hinted h \
         JOIN contents c ON c.hash = h.hash \
         WHERE EXISTS (SELECT 1 FROM paths p \
                       WHERE p.content_hash = c.hash AND p.missing = 0) \
           AND ((c.kind = 'image' AND {image_pending}) \
                OR (c.kind = 'video' AND {video_pending})) \
         ORDER BY h.priority"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(hinted), |row| row.get(0))
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

fn emit_progress(app: &AppHandle, class: WorkClass, counts: Option<(u64, u64)>) {
    record_progress(app, class, counts);
}

pub fn notify_item_update(
    app: &AppHandle,
    conn: &Connection,
    class: &str,
    previous_hash: &str,
    hash: &str,
) {
    match crate::queries::item_by_hash(conn, hash) {
        Ok(Some(item)) => {
            let _ = app.emit(
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

fn notify_image_changes(app: &AppHandle, conn: &Connection, changes: &[(String, String)]) {
    for (previous, current) in changes {
        notify_item_update(app, conn, "previews", previous, current);
    }
}

fn notify_video_changes(app: &AppHandle, conn: &Connection, hashes: &[String]) {
    for hash in hashes {
        notify_item_update(app, conn, "video-posters", hash, hash);
    }
}

fn progress(app: &AppHandle, class: WorkClass) -> impl Fn(u64, u64) + '_ {
    move |done, total| emit_progress(app, class, Some((done, total)))
}

fn whisper_model(data_root: &Path) -> Option<PathBuf> {
    crate::binaries_manager::spec_of("whisper-large-v3-turbo").and_then(|spec| {
        let state = crate::binaries_manager::state_of(data_root, spec);
        (state.status != crate::binaries::BinaryStatus::NotInstalled)
            .then(|| crate::binaries_manager::installed_path(data_root, spec))
    })
}

#[derive(Default)]
struct TranscriptStep {
    attempted_hash: Option<String>,
    exhausted: bool,
}

fn transcribe_next(
    conn: &Connection,
    cache: &CachePaths,
    model: &Path,
    ffmpeg: &Path,
    app: &AppHandle,
    after_hash: Option<&str>,
) -> Result<TranscriptStep, String> {
    let rows = crate::derived_state::transcript_candidates(
        conn,
        after_hash,
        crate::derived_state::TRANSCRIPT_CANDIDATE_PAGE_SIZE,
    )?;
    if rows.is_empty() {
        return Ok(TranscriptStep {
            exhausted: true,
            ..TranscriptStep::default()
        });
    }

    for (hash, path) in rows {
        if crate::derived_state::transcript_result(conn, cache, &hash)?.status
            == crate::derived_state::READY
        {
            return Ok(TranscriptStep {
                attempted_hash: Some(hash),
                exhausted: false,
            });
        }
        if !is_idle() {
            return Ok(TranscriptStep::default());
        }
        let claim = match crate::transcription::claim() {
            Ok(claim) => claim,
            Err(error) if error == crate::transcription::TRANSCRIPTION_BUSY => {
                return Ok(TranscriptStep::default())
            }
            Err(error) => return Err(error),
        };
        emit_progress(app, WorkClass::Transcripts, None);
        // Audio extraction and model loading happen before Whisper's first
        // percentage callback; publish ownership now so an open video never
        // looks pending while its expensive work is already underway.
        let _ = app.emit(
            "transcribe://progress",
            json!({ "hash": hash, "percent": 0 }),
        );
        let finished = std::sync::Arc::new(AtomicBool::new(false));
        let watch = std::thread::spawn({
            let finished = std::sync::Arc::clone(&finished);
            move || loop {
                if finished.load(Ordering::SeqCst) {
                    return;
                }
                if !is_idle() || cancelled() {
                    crate::transcription::request_cancel();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
        let result = crate::transcription::transcribe_to_cache_claimed(
            &claim,
            cache,
            Some(model),
            Some(ffmpeg),
            Path::new(&path),
            &hash,
            {
                let progress_handle = app.clone();
                let progress_hash = hash.clone();
                move |percent| {
                    let percent = percent.clamp(0, 100);
                    record_progress(
                        &progress_handle,
                        WorkClass::Transcripts,
                        Some((percent as u64, 100)),
                    );
                    let _ = progress_handle.emit(
                        "transcribe://progress",
                        json!({ "hash": progress_hash, "percent": percent }),
                    );
                }
            },
        );
        finished.store(true, Ordering::SeqCst);
        let _ = watch.join();
        // The claim resets cancellation when dropped, so classify this run
        // while it still owns the Whisper slot.
        let was_cancelled = crate::transcription::is_cancelled();
        drop(claim);
        match result {
            Ok(text) => {
                crate::derived_state::record_transcript_success(
                    conn,
                    &hash,
                    &path,
                    !text.trim().is_empty(),
                )?;
                let _ = app.emit("transcribe://done", json!({ "hash": hash, "text": text }));
                return Ok(TranscriptStep {
                    attempted_hash: Some(hash),
                    exhausted: false,
                });
            }
            Err(error) => {
                if error == crate::scanner::CANCELLED || was_cancelled {
                    logging::debug(
                        "derived transcription stopped",
                        json!({ "hash": hash, "reason": "cancelled" }),
                    );
                    let _ = app.emit("transcribe://cancelled", json!({ "hash": hash }));
                    return Ok(TranscriptStep::default());
                }
                crate::derived_state::record_transcript_failure(conn, &hash, &path, &error)?;
                logging::debug(
                    "derived transcription failed",
                    json!({ "hash": hash, "error": { "message": error } }),
                );
                let _ = app.emit(
                    "transcribe://error",
                    json!({ "hash": hash, "message": error }),
                );
                return Ok(TranscriptStep {
                    attempted_hash: Some(hash),
                    exhausted: false,
                });
            }
        }
    }
    Ok(TranscriptStep::default())
}
