//! Video derivatives via the managed ffmpeg: a poster frame at ~15% of
//! duration (skipping black/fade-in openings) that flows through the SAME
//! thumb/preview cache pipeline as images — so the grid and comparison
//! surfaces need no video-specific rendering — plus an evenly spaced snapshot
//! strip, duration-scaled (one frame per `videoStripSecondsPerFrame`, clamped
//! to the configured min/max). Duration comes from `ffmpeg -i` stderr parsing
//! (one managed executable, no separate ffprobe, per the
//! managed-runtime-dependencies conventions' one-binary rule).
//!
//! Frames are staged as JPEG (mjpeg is in every build; webp encoders are not)
//! and re-encoded through `preview.rs`'s own WebP writer, keeping one encode
//! path. When ffmpeg is absent, video derivation simply waits — rows stay
//! underived and the scan reports the skip; nothing fails.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::logging;
use crate::preview::{self, CachePaths};

pub struct StripConfig {
    pub seconds_per_frame: u32,
    pub min_frames: u32,
    pub max_frames: u32,
}

/// Strip cache entry: `strips/<h2>/<hash>-<index>.webp` beside the
/// thumbs/previews shards, served as `strip-<hash>-<index>` by mediacache.
pub fn strip_path(cache: &CachePaths, hash: &str, index: u32) -> PathBuf {
    cache
        .root_dir()
        .join("strips")
        .join(hash.get(0..2).unwrap_or("00"))
        .join(format!("{hash}-{index}.webp"))
}

/// Parses `Duration: HH:MM:SS.cc` out of `ffmpeg -i` stderr.
pub fn parse_duration_ms(stderr_text: &str) -> Option<u64> {
    let after = stderr_text.split("Duration: ").nth(1)?;
    let stamp: String = after.chars().take_while(|c| *c != ',').collect();
    let stamp = stamp.trim();
    let mut parts = stamp.split(':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if minutes >= 60 || !(0.0..3600.0).contains(&seconds.min(59.99)) {
        return None;
    }
    Some(hours * 3_600_000 + minutes * 60_000 + (seconds * 1000.0) as u64)
}

/// Frame count for a duration under the strip config.
pub fn strip_frame_count(duration_ms: u64, config: &StripConfig) -> u32 {
    let by_duration = (duration_ms / 1000 / u64::from(config.seconds_per_frame.max(1))) as u32;
    by_duration.clamp(config.min_frames, config.max_frames)
}

/// Evenly spaced timestamps (ms) for `count` frames — interior points, never
/// the exact start or end.
pub fn strip_timestamps_ms(duration_ms: u64, count: u32) -> Vec<u64> {
    (1..=u64::from(count))
        .map(|i| duration_ms * i / (u64::from(count) + 1))
        .collect()
}

// The subprocess boundary is logged (logging conventions): one debug line
// per invocation, and the caller's error path carries the result — a probe
// or extraction failure becomes an issue row, which record-time mirrors warn.
fn log_invocation(op: &str, src: &Path) {
    crate::logging::debug(
        "ffmpeg invocation",
        serde_json::json!({ "op": op, "src": src.to_string_lossy() }),
    );
}

fn probe_duration_ms(ffmpeg: &Path, src: &Path) -> Result<u64, String> {
    log_invocation("probe-duration", src);
    // `ffmpeg -i` with no output exits non-zero by design; stderr still
    // carries the stream banner we parse.
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-i"]).arg(src);
    let run = crate::subprocess::run_bounded(cmd, &crate::scanner::cancelled)?;
    parse_duration_ms(&run.stderr)
        .ok_or_else(|| format!("no Duration in ffmpeg output for {}", src.display()))
}

/// Extracts one frame at `at_ms` as a staged JPEG (real content extension —
/// ffmpeg infers the muxer from it; the storage-path conventions' documented
/// staging exception).
fn extract_frame(ffmpeg: &Path, src: &Path, at_ms: u64, staged_jpg: &Path) -> Result<(), String> {
    log_invocation("extract-frame", src);
    let seconds = format!("{}.{:03}", at_ms / 1000, at_ms % 1000);
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-ss", &seconds, "-i"])
        .arg(src)
        .args(["-frames:v", "1", "-q:v", "3", "-update", "1", "-y"])
        .arg(staged_jpg);
    let run = crate::subprocess::run_bounded(cmd, &crate::scanner::cancelled)?;
    if !run.status_ok {
        return Err(format!("frame extraction failed at {seconds}s for {}", src.display()));
    }
    if !staged_jpg.is_file() {
        return Err(format!("ffmpeg emitted no frame at {seconds}s for {}", src.display()));
    }
    Ok(())
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct VideoDeriveStats {
    pub derived: u64,
    pub failed: u64,
    pub skipped_no_ffmpeg: bool,
}

/// The video half of the SCAN derive pass: duration + poster only (through
/// the image pipeline, so thumb/preview land in the shared cache). Scene
/// strips deliberately do NOT happen here (Phase 33): a grid without posters
/// reads as broken, so posters block the scan, but strips are many frames per
/// video and belong to the idle backfill — `derive_strips_pending` finds them
/// through the NULL `strip_frames` this pass leaves behind.
pub fn derive_videos_pending(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    temp_dir: &Path,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<VideoDeriveStats, String> {
    let mut stats = VideoDeriveStats::default();
    let Some(ffmpeg) = ffmpeg else {
        stats.skipped_no_ffmpeg = true;
        return Ok(stats);
    };
    // not recorded: ffmpeg frame staging (temp/, wiped at launch); the WebP
    // results land through preview.rs's own unrecorded cache writes.
    std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;

    // A row stamped with an older DERIVE_VERSION is pending again, so bumping
    // the constant re-derives posters and strips without touching a user file.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT c.hash, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c WHERE c.kind = 'video' \
             AND (c.derived_at_utc IS NULL \
                  OR (c.derived_version < {} AND c.derived_at_utc != 'failed'))",
            crate::preview::DERIVE_VERSION
        ))
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    for (hash, path) in rows {
        let Some(path) = path else { continue };
        let src = Path::new(&path);
        let result = (|| -> Result<u64, String> {
            let duration_ms = probe_duration_ms(ffmpeg, src)?;

            // Poster at 15% through the shared image pipeline.
            let staged = temp_dir.join(format!("poster-{}.jpg", crate::nanoid::generate()));
            extract_frame(ffmpeg, src, duration_ms * 15 / 100, &staged)?;
            // The staged poster is a plain JPEG, so the image crate opens it
            // directly — no ffmpeg needed for the decode half.
            let poster_result = preview::generate_for_image(
                &staged,
                &hash,
                cache,
                thumb_edge,
                preview_long_edge,
                None,
            );
            let _ = std::fs::remove_file(&staged);
            poster_result?;
            Ok(duration_ms)
        })();

        match result {
            Ok(duration_ms) => {
                stats.derived += 1;
                conn.execute(
                    &format!(
                        "UPDATE contents SET duration_ms = COALESCE(duration_ms, ?2), \
                         derived_at_utc = ?3, \
                         derived_version = {} WHERE hash = ?1",
                        crate::preview::DERIVE_VERSION
                    ),
                    params![hash, duration_ms as i64, logging::now_iso_millis()],
                )
                .map_err(|e| e.to_string())?;
                // Current-state issues: a derive that now succeeds retires the
                // failure it recorded on an earlier pass.
                crate::index_store::clear_issues(conn, &path, &["video-derive-error"])?;
            }
            Err(err) => {
                stats.failed += 1;
                crate::index_store::upsert_issue(conn, Some(&path), "video-derive-error", &err)?;
                conn.execute(
                    "UPDATE contents SET derived_at_utc = 'failed' WHERE hash = ?1",
                    [&hash],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    Ok(stats)
}

/// The backfill half (Phase 33): scene strips for videos the scan already
/// postered, found through their NULL `strip_frames`. Runs only while the app
/// is idle — `stop` is consulted between videos, so the user's return waits
/// at most one video's strip extraction. Returns how many videos got strips.
pub fn derive_strips_pending(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: &Path,
    temp_dir: &Path,
    strip: &StripConfig,
    stop: &dyn Fn() -> bool,
    progress: &dyn Fn(u64, u64),
) -> Result<u64, String> {
    std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT c.hash, c.duration_ms, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c JOIN paths p2 ON p2.content_hash = c.hash \
             WHERE c.kind = 'video' AND c.strip_frames IS NULL \
               AND c.duration_ms IS NOT NULL \
               AND c.derived_at_utc IS NOT NULL AND c.derived_at_utc != 'failed' \
               AND p2.missing = 0 \
             GROUP BY c.hash \
             ORDER BY MIN(p2.resolved_utc_ms) DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, i64, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let total = rows.len() as u64;
    let mut done = 0u64;
    for (hash, duration_ms, path) in rows {
        if stop() {
            break;
        }
        let Some(path) = path else { continue };
        let src = Path::new(&path);
        let duration_ms = duration_ms.max(0) as u64;
        let count = strip_frame_count(duration_ms, strip);
        let result = (|| -> Result<(), String> {
            for (index, at_ms) in strip_timestamps_ms(duration_ms, count).iter().enumerate() {
                let staged = temp_dir.join(format!("strip-{}.jpg", crate::nanoid::generate()));
                let frame_result = extract_frame(ffmpeg, src, *at_ms, &staged).and_then(|()| {
                    let img = image::open(&staged).map_err(|e| e.to_string())?;
                    let target = strip_path(cache, &hash, index as u32);
                    preview::write_webp(&img, &target, 76.0)
                });
                let _ = std::fs::remove_file(&staged);
                frame_result?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute(
                    "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
                    params![hash, count as i64],
                )
                .map_err(|e| e.to_string())?;
                crate::index_store::clear_issues(conn, &path, &["video-derive-error"])?;
                done += 1;
                progress(done, total);
            }
            Err(err) => {
                // -1 = strips failed: keeps the row out of every later pass
                // (the churn the image pass's 'failed' marker exists to stop —
                // retrying a broken video's N frame extractions every idle
                // tick could eat whole minutes per pass). A rescan after a
                // DERIVE_VERSION bump re-derives; the issue row carries the
                // reason meanwhile.
                conn.execute(
                    "UPDATE contents SET strip_frames = -1 WHERE hash = ?1",
                    params![hash],
                )
                .map_err(|e| e.to_string())?;
                crate::index_store::upsert_issue(conn, Some(&path), "video-derive-error", &err)?;
                progress(done, total);
            }
        }
    }
    Ok(done)
}

