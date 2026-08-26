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

use rusqlite::Connection;

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
    let run = crate::subprocess::run_bounded(cmd, &crate::derived_work::cancelled)?;
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
    let run = crate::subprocess::run_bounded(cmd, &crate::derived_work::cancelled)?;
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
    pub changed_hashes: Vec<String>,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct StripDeriveStats {
    pub completed: u64,
    pub failed: u64,
    pub attempted: u64,
    pub candidates_found: bool,
    pub last_attempted_hash: Option<String>,
}

/// The video half of the SCAN derive pass: duration + poster only (through
/// the image pipeline, so thumb/preview land in the shared cache). Scene
/// strips deliberately do NOT happen here (Phase 33): a grid without posters
/// reads as broken, so posters block the scan, but strips are many frames per
/// video and belong to idle derived work — `derive_strips_pending` finds them
/// through the NULL `strip_frames` this pass leaves behind.
pub fn derive_videos_pending(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    temp_dir: &Path,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<VideoDeriveStats, String> {
    derive_videos_pending_limit(
        conn,
        cache,
        ffmpeg,
        temp_dir,
        thumb_edge,
        preview_long_edge,
        None,
        None,
    )
}

/// Runs at most one video-poster job for the derived-work coordinator.
pub(crate) fn derive_next_video(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    temp_dir: &Path,
    thumb_edge: u32,
    preview_long_edge: u32,
) -> Result<VideoDeriveStats, String> {
    derive_videos_pending_limit(
        conn,
        cache,
        ffmpeg,
        temp_dir,
        thumb_edge,
        preview_long_edge,
        Some(1),
        None,
    )
}

/// Runs one pending poster only when it matches `hash`.
pub(crate) fn derive_video_hash(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    temp_dir: &Path,
    thumb_edge: u32,
    preview_long_edge: u32,
    hash: &str,
) -> Result<VideoDeriveStats, String> {
    derive_videos_pending_limit(
        conn,
        cache,
        ffmpeg,
        temp_dir,
        thumb_edge,
        preview_long_edge,
        Some(1),
        Some(hash),
    )
}

fn derive_videos_pending_limit(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    temp_dir: &Path,
    thumb_edge: u32,
    preview_long_edge: u32,
    limit: Option<usize>,
    only_hash: Option<&str>,
) -> Result<VideoDeriveStats, String> {
    let mut stats = VideoDeriveStats::default();
    let Some(ffmpeg) = ffmpeg else {
        stats.skipped_no_ffmpeg = true;
        return Ok(stats);
    };
    // not recorded: ffmpeg frame staging (temp/, wiped at launch); the WebP
    // results land through preview.rs's own unrecorded cache writes.
    std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;

    let rows = crate::derived_state::video_candidates(conn, true, limit, only_hash)?;

    for (hash, path) in rows {
        let src = Path::new(&path);
        let result = (|| -> Result<u64, String> {
            let duration_ms = probe_duration_ms(ffmpeg, src)?;

            // Poster at 15% through the shared image pipeline.
            let staged = temp_dir.join(format!("poster-{}.jpg", crate::nanoid::generate()));
            let poster_result = extract_frame(ffmpeg, src, duration_ms * 15 / 100, &staged)
                .and_then(|()| {
                    // The staged poster is a plain JPEG, so the image crate
                    // opens it directly — no ffmpeg needed for this half.
                    preview::generate_for_image(
                        &staged,
                        &hash,
                        cache,
                        thumb_edge,
                        preview_long_edge,
                        None,
                    )
                    .map(|_| ())
                });
            let _ = std::fs::remove_file(&staged);
            poster_result?;
            Ok(duration_ms)
        })();

        match result {
            Ok(duration_ms) => {
                stats.derived += 1;
                crate::derived_state::record_poster_success(
                    conn,
                    &hash,
                    &path,
                    duration_ms,
                )?;
                stats.changed_hashes.push(hash);
            }
            Err(err) if err.starts_with(crate::scanner::CANCELLED) => {
                return Err(crate::scanner::CANCELLED.to_string());
            }
            Err(err) => {
                stats.failed += 1;
                crate::derived_state::record_poster_failure(conn, &hash, &path, &err)?;
            }
        }
    }

    Ok(stats)
}

/// The idle derived-work half: scene strips for videos the poster pass already
/// postered, found through their NULL `strip_frames`. Runs only while the app
/// is idle — `stop` is consulted between videos, so the user's return waits
/// at most one video's strip extraction. Returns one page's work statistics.
pub fn derive_strips_pending(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: &Path,
    temp_dir: &Path,
    strip: &StripConfig,
    after_hash: Option<&str>,
    stop: &dyn Fn() -> bool,
    progress: &dyn Fn(u64, u64),
) -> Result<StripDeriveStats, String> {
    // not recorded: ffmpeg strip-frame staging lives in temp/ and produces
    // reconstructible binary cache entries.
    std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;
    let rows = crate::derived_state::strip_candidates(
        conn,
        after_hash,
        crate::derived_state::SNAPSHOT_CANDIDATE_PAGE_SIZE,
    )?;
    let mut stats = StripDeriveStats {
        candidates_found: !rows.is_empty(),
        ..StripDeriveStats::default()
    };
    let total = rows.len() as u64;
    for (hash, duration_ms, path) in rows {
        if stop() {
            break;
        }
        stats.attempted += 1;
        stats.last_attempted_hash = Some(hash.clone());
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
                crate::derived_state::record_strip_success(conn, &hash, &path, count)?;
                stats.completed += 1;
                progress(stats.attempted, total);
            }
            Err(err) if err.starts_with(crate::scanner::CANCELLED) => {
                for index in 0..count {
                    let _ = std::fs::remove_file(strip_path(cache, &hash, index));
                }
                return Err(crate::scanner::CANCELLED.to_string());
            }
            Err(err) => {
                // -1 = strips failed: keeps the row out of every later pass
                // (the churn the image pass's 'failed' marker exists to stop —
                // retrying a broken video's N frame extractions every idle
                // tick could eat whole minutes per pass). A rescan after a
                // DERIVE_VERSION bump re-derives; the issue row carries the
                // reason meanwhile.
                crate::derived_state::record_strip_failure(conn, &hash, &path, &err)?;
                stats.failed += 1;
                progress(stats.attempted, total);
            }
        }
    }
    Ok(stats)
}
