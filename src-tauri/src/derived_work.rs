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
        face_models: score_faces
            .then(|| installed("ultraface-rfb640").zip(installed("hsemotion-enet-b2")))
            .flatten(),
        temp_dir: data_root.join(crate::binaries_manager::TEMP_DIR_NAME),
    }
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
        && !crate::scan_running()
}

fn available() -> bool {
    HEAVY_OPS.load(Ordering::SeqCst) == 0 && !crate::scan_running()
}

/// Wakes the coordinator after index or dependency state changes. Index
/// changes also invalidate the process's similarity snapshot; a durable
/// revision receipt will replace this process-local bit in the receipts slice.
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
                        observed = current;
                    }
                }
            }
            run_again = false;
            if !available() {
                continue;
            }
            match run_one_pass(&app) {
                Ok(did_work) => run_again = did_work,
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
    data_root: &Path,
    config: Option<&serde_json::Value>,
    hash: &str,
) -> Result<String, String> {
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
fn run_one_pass(app: &AppHandle) -> Result<bool, String> {
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
        let image = {
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
            )?
        };
        if image.derived + image.failed + image.blocked_no_ffmpeg > 0 {
            if image.derived > 0 {
                SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
            }
            emit_progress(app, "previews", None);
            let _ = app.emit("derived://updated", json!({ "class": "previews" }));
            priority_done += 1;
        } else {
            let video = {
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
                )?
            };
            if video.derived + video.failed > 0 {
                emit_progress(app, "video-posters", None);
                let _ = app.emit("derived://updated", json!({ "class": "video-posters" }));
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
        let image = {
            let _claim = MEDIA_WORK
                .lock()
                .map_err(|_| "derived media owner is unavailable".to_string())?;
            crate::preview::derive_next_image(
                &conn,
                &cache,
                settings.thumb_edge,
                settings.preview_long_edge,
                settings.ffmpeg.as_deref(),
            )?
        };
        if image.derived + image.failed + image.blocked_no_ffmpeg == 0 {
            break;
        }
        if image.derived > 0 {
            SIMILARITY_DIRTY.store(true, Ordering::SeqCst);
        }
        emit_progress(app, "previews", None);
        let _ = app.emit("derived://updated", json!({ "class": "previews" }));
        if index + 1 == IMAGE_BATCH {
            image_budget_full = true;
        }
    }

    let mut video_budget_full = false;
    for index in 0..VIDEO_BATCH {
        if !available() {
            return Ok(false);
        }
        let video = {
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
            )?
        };
        if video.derived + video.failed == 0 {
            break;
        }
        emit_progress(app, "video-posters", None);
        let _ = app.emit("derived://updated", json!({ "class": "video-posters" }));
        if index + 1 == VIDEO_BATCH {
            video_budget_full = true;
        }
    }

    if image_budget_full || video_budget_full {
        return Ok(true);
    }

    if SIMILARITY_DIRTY.swap(false, Ordering::SeqCst) {
        let result = crate::similarity::rebuild_groups_for_root(
            &conn,
            &settings.similarity,
            &settings.data_root,
        );
        match result {
            Ok(stats) => {
                emit_progress(app, "similarity", None);
                let _ = app.emit("derived://updated", json!({ "class": "similarity" }));
                logging::info(
                    "similarity rebuilt",
                    json!({ "groups": stats.groups, "items": stats.grouped_items }),
                );
                return Ok(true);
            }
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

    let stop = || !is_idle();
    if let Some(ffmpeg) = settings.ffmpeg.as_deref() {
        let ran = crate::video::derive_strips_pending(
            &conn,
            &cache,
            ffmpeg,
            &settings.temp_dir,
            &settings.strip,
            &stop,
            &progress(app, "strips"),
        )?;
        if ran > 0 {
            return Ok(false);
        }
    }

    let whisper = whisper_model(&data_root);
    if let (Some(model), Some(ffmpeg)) = (whisper.as_deref(), settings.ffmpeg.as_deref()) {
        if transcribe_next(&conn, &cache, model, ffmpeg, app)? {
            return Ok(false);
        }
    }

    if let Some((detector, emotion)) = settings.face_models.as_ref() {
        let stats = crate::face::face_scores_pending(
            &conn,
            &cache,
            Some((detector.as_path(), emotion.as_path())),
            |done, total| emit_progress(app, "faces", Some((done, total))),
            &stop,
        )?;
        if stats.scored > 0 || stats.failed > 0 {
            return Ok(false);
        }
    }

    let _ = app.emit("derived://quiet", json!({}));
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

    let stale = format!(
        "c.derived_version < {} AND c.derived_at_utc NOT IN ('failed', '{}')",
        crate::preview::DERIVE_VERSION,
        crate::preview::NEEDS_FFMPEG,
    );
    let image_pending = if settings.ffmpeg.is_some() {
        format!(
            "(c.derived_at_utc IS NULL OR c.derived_at_utc = '{}' OR ({stale}))",
            crate::preview::NEEDS_FFMPEG,
        )
    } else {
        format!("(c.derived_at_utc IS NULL OR ({stale}))")
    };
    let video_pending = if settings.ffmpeg.is_some() {
        format!(
            "(c.derived_at_utc IS NULL OR \
             (c.derived_version < {} AND c.derived_at_utc != 'failed'))",
            crate::preview::DERIVE_VERSION,
        )
    } else {
        "0".to_string()
    };
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
    let stale = format!(
        "c.derived_version < {} AND c.derived_at_utc NOT IN ('failed', '{}')",
        crate::preview::DERIVE_VERSION,
        crate::preview::NEEDS_FFMPEG,
    );
    let image_pending = if settings.ffmpeg.is_some() {
        format!(
            "(c.derived_at_utc IS NULL OR c.derived_at_utc = '{}' OR ({stale}))",
            crate::preview::NEEDS_FFMPEG,
        )
    } else {
        format!("(c.derived_at_utc IS NULL OR ({stale}))")
    };
    let video_pending = if settings.ffmpeg.is_some() {
        format!(
            "(c.derived_at_utc IS NULL OR \
             (c.derived_version < {} AND c.derived_at_utc != 'failed'))",
            crate::preview::DERIVE_VERSION,
        )
    } else {
        "0".to_string()
    };
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

fn emit_progress(app: &AppHandle, class: &str, counts: Option<(u64, u64)>) {
    let (done, total) = counts.map_or((None, None), |(done, total)| (Some(done), Some(total)));
    let _ = app.emit(
        "derived://progress",
        json!({ "class": class, "done": done, "total": total }),
    );
}

fn progress<'a>(app: &'a AppHandle, class: &'a str) -> impl Fn(u64, u64) + 'a {
    move |done, total| emit_progress(app, class, Some((done, total)))
}

fn whisper_model(data_root: &Path) -> Option<PathBuf> {
    crate::binaries_manager::spec_of("whisper-large-v3-turbo").and_then(|spec| {
        let state = crate::binaries_manager::state_of(data_root, spec);
        (state.status != crate::binaries::BinaryStatus::NotInstalled)
            .then(|| crate::binaries_manager::installed_path(data_root, spec))
    })
}

fn transcribe_next(
    conn: &Connection,
    cache: &CachePaths,
    model: &Path,
    ffmpeg: &Path,
    app: &AppHandle,
) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.hash, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c JOIN paths p2 ON p2.content_hash = c.hash \
             WHERE c.kind = 'video' AND c.duration_ms IS NOT NULL \
               AND p2.missing = 0 \
             GROUP BY c.hash \
             ORDER BY MIN(p2.resolved_utc_ms) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|row| row.ok())
        .collect();
    drop(stmt);

    for (hash, path) in rows {
        let Some(path) = path else { continue };
        if cache.transcript(&hash).exists() {
            continue;
        }
        if !is_idle() {
            return Ok(false);
        }
        let claim = match crate::transcription::claim() {
            Ok(claim) => claim,
            Err(error) if error == crate::transcription::TRANSCRIPTION_BUSY => return Ok(false),
            Err(error) => return Err(error),
        };
        emit_progress(app, "transcripts", None);
        let finished = std::sync::Arc::new(AtomicBool::new(false));
        let watch = std::thread::spawn({
            let finished = std::sync::Arc::clone(&finished);
            move || loop {
                if finished.load(Ordering::SeqCst) {
                    return;
                }
                if !is_idle() {
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
            |_| {},
        );
        finished.store(true, Ordering::SeqCst);
        let _ = watch.join();
        drop(claim);
        match result {
            Ok(text) => {
                let _ = app.emit("transcribe://done", json!({ "hash": hash, "text": text }));
                return Ok(true);
            }
            Err(error) => {
                logging::debug(
                    "derived transcription stopped",
                    json!({ "hash": hash, "error": { "message": error } }),
                );
                return Ok(false);
            }
        }
    }
    Ok(false)
}
