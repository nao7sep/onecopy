//! The idle backfill scheduler (Phase 33): derived data the user should never
//! wait for — video scene strips, transcripts, face scores — fills in by
//! itself whenever the tools are installed and the app is idle. Rows without
//! their tool stay PENDING, never failed (the needs-ffmpeg pattern), so the
//! app is fully usable with nothing installed and quietly completes itself
//! when something arrives.
//!
//! ONE busy rule: the user is busy if they touched the app in the last
//! `IDLE_AFTER` (the frontend pings on input, throttled), or a scan is
//! running, or a heavy operation (move/copy/delete/empty) is in flight. The
//! input signal deliberately subsumes surface states like "a modal is open":
//! an open modal with no input for a minute means the user walked away, which
//! is exactly when working is fine — the between-items check (and whisper's
//! abort callback) hands the machine back within a second of their return.
//!
//! Priorities, hard-coded by design (not configuration): strips → transcripts
//! → faces. Strips are what the scenes modal needs to be useful at all;
//! transcripts are the MAIN video-culling signal (audio tells more than
//! frames — a mid-sentence cut means a retake follows); faces are a
//! nice-to-have ordering refinement and opt-in besides. Within a class,
//! newest-resolved first — recent media is what the user is culling.

use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use rusqlite::Connection;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::logging;
use crate::preview::CachePaths;

/// Milliseconds since the epoch of the last user input (frontend pings).
/// Starts at 0 = "never touched", which reads as idle — a launch that sits
/// unattended starts filling in immediately.
static LAST_ACTIVITY_MS: AtomicI64 = AtomicI64::new(0);

/// Heavy operations in flight (move/copy/delete/empty-trash). The backfill
/// must never compete with them for the disk.
static HEAVY_OPS: AtomicUsize = AtomicUsize::new(0);

const IDLE_AFTER_MS: i64 = 60_000;
/// How often the loop re-checks for idleness and work.
const TICK_SECONDS: u64 = 15;

pub fn note_activity() {
    LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
}

/// RAII guard a heavy command holds for its whole body.
pub struct HeavyOp;
pub fn heavy_op() -> HeavyOp {
    HEAVY_OPS.fetch_add(1, Ordering::SeqCst);
    HeavyOp
}
impl Drop for HeavyOp {
    fn drop(&mut self) {
        HEAVY_OPS.fetch_sub(1, Ordering::SeqCst);
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

/// The scheduler thread, spawned once at setup. Never touches the UI thread;
/// its own connection per pass (WAL carries the concurrency).
pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(TICK_SECONDS));
        if !is_idle() {
            continue;
        }
        match run_one_pass(&app) {
            Ok(did_work) => {
                if !did_work {
                    // Nothing pending anywhere: nothing to narrate either.
                    let _ = app.emit("backfill://quiet", json!({}));
                }
            }
            Err(err) => {
                logging::warn("backfill pass failed", json!({ "error": { "message": err } }));
            }
        }
    });
}

/// One bounded pass: picks the highest-priority class with work and runs it
/// until done or until the user comes back. Returns whether anything ran.
fn run_one_pass(app: &AppHandle) -> Result<bool, String> {
    let data_root = crate::DATA_ROOT
        .get()
        .ok_or("data root unset")?
        .clone();
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = crate::scanner::settings_from_config(config.as_ref(), &data_root, 0);
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let cache = CachePaths::new(settings.cache_root.clone());
    let stop = || !is_idle();

    // Strips first: the scan derives only the poster now, and the scenes
    // modal is useless without its frames.
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
            return Ok(true);
        }
    }

    // Transcripts: the main video-culling signal.
    let whisper = whisper_model(&data_root);
    if let (Some(model), Some(ffmpeg)) = (whisper.as_deref(), settings.ffmpeg.as_deref()) {
        if transcribe_next(&conn, &cache, model, ffmpeg, app)? {
            return Ok(true);
        }
    }

    // Faces: opt-in, and models only count when the user enabled scoring.
    if let Some((detector, emotion)) = settings.face_models.as_ref() {
        let stats = crate::face::face_scores_pending(
            &conn,
            &cache,
            Some((detector.as_path(), emotion.as_path())),
            |done, total| {
                let _ = app.emit(
                    "backfill://progress",
                    json!({ "class": "faces", "done": done, "total": total }),
                );
            },
            &stop,
        )?;
        if stats.scored > 0 || stats.failed > 0 {
            return Ok(true);
        }
    }

    Ok(false)
}

fn progress<'a>(app: &'a AppHandle, class: &'a str) -> impl Fn(u64, u64) + 'a {
    move |done, total| {
        let _ = app.emit(
            "backfill://progress",
            json!({ "class": class, "done": done, "total": total }),
        );
    }
}

fn whisper_model(data_root: &Path) -> Option<std::path::PathBuf> {
    crate::binaries_manager::spec_of("whisper-large-v3-turbo").and_then(|spec| {
        let state = crate::binaries_manager::state_of(data_root, spec);
        (state.status != crate::binaries::BinaryStatus::NotInstalled)
            .then(|| crate::binaries_manager::installed_path(data_root, spec))
    })
}

/// Transcribes ONE pending video (newest resolved first), so the loop's
/// between-items idle check sits between files, and whisper's own abort
/// callback covers the minutes inside one. Returns whether one ran.
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
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
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
        let _ = app.emit(
            "backfill://progress",
            json!({ "class": "transcripts", "hash": hash }),
        );
        // The user coming back mid-file cancels through whisper's abort
        // callback — the same flag the scenes modal's Cancel sets. A
        // cancelled run writes no cache entry, so the file simply stays
        // pending for the next idle stretch.
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watch = std::thread::spawn({
            let finished = std::sync::Arc::clone(&finished);
            let flag_check = std::time::Duration::from_millis(500);
            move || loop {
                if finished.load(Ordering::SeqCst) {
                    return;
                }
                if !is_idle() {
                    crate::transcription::request_cancel();
                    return;
                }
                std::thread::sleep(flag_check);
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
        // Whatever happened, release the watcher before the claim. Keeping
        // those lifetimes nested prevents this watcher touching a later run.
        finished.store(true, Ordering::SeqCst);
        let _ = watch.join();
        drop(claim);
        match result {
            Ok(text) => {
                let _ = app.emit(
                    "transcribe://done",
                    json!({ "hash": hash, "text": text }),
                );
                return Ok(true);
            }
            Err(err) => {
                // Cancelled-by-return reads as an error from the engine; a
                // real failure is logged and the file is NOT retried this
                // pass (the next pass will — transcription has no failed
                // marker, deliberately: a transient ffmpeg audio error must
                // not strand a video forever).
                logging::debug(
                    "backfill transcription stopped",
                    json!({ "hash": hash, "error": { "message": err } }),
                );
                return Ok(false);
            }
        }
    }
    Ok(false)
}
