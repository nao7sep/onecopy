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
    let output = std::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(src)
        .output()
        .map_err(|e| e.to_string())?;
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    parse_duration_ms(&stderr_text)
        .ok_or_else(|| format!("no Duration in ffmpeg output for {}", src.display()))
}

/// Extracts one frame at `at_ms` as a staged JPEG (real content extension —
/// ffmpeg infers the muxer from it; the storage-path conventions' documented
/// staging exception).
fn extract_frame(ffmpeg: &Path, src: &Path, at_ms: u64, staged_jpg: &Path) -> Result<(), String> {
    log_invocation("extract-frame", src);
    let seconds = format!("{}.{:03}", at_ms / 1000, at_ms % 1000);
    let status = std::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss", &seconds, "-i"])
        .arg(src)
        .args(["-frames:v", "1", "-q:v", "3", "-update", "1", "-y"])
        .arg(staged_jpg)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
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

/// The video half of the derive pass: duration + poster (through the image
/// pipeline, so thumb/preview land in the shared cache) + strip frames.
pub fn derive_videos_pending(
    conn: &Connection,
    cache: &CachePaths,
    ffmpeg: Option<&Path>,
    temp_dir: &Path,
    thumb_edge: u32,
    preview_long_edge: u32,
    strip: &StripConfig,
) -> Result<VideoDeriveStats, String> {
    let mut stats = VideoDeriveStats::default();
    let Some(ffmpeg) = ffmpeg else {
        stats.skipped_no_ffmpeg = true;
        return Ok(stats);
    };
    // not recorded: ffmpeg frame staging (temp/, wiped at launch); the WebP
    // results land through preview.rs's own unrecorded cache writes.
    std::fs::create_dir_all(temp_dir).map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(
            "SELECT c.hash, (SELECT p.abs_path FROM paths p \
             WHERE p.content_hash = c.hash AND p.missing = 0 LIMIT 1) \
             FROM contents c WHERE c.kind = 'video' AND c.derived_at_utc IS NULL",
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
        let src = Path::new(&path);
        let result = (|| -> Result<u64, String> {
            let duration_ms = probe_duration_ms(ffmpeg, src)?;

            // Poster at 15% through the shared image pipeline.
            let staged = temp_dir.join(format!("poster-{}.jpg", crate::nanoid::generate()));
            extract_frame(ffmpeg, src, duration_ms * 15 / 100, &staged)?;
            let poster_result =
                preview::generate_for_image(&staged, &hash, cache, thumb_edge, preview_long_edge);
            let _ = std::fs::remove_file(&staged);
            poster_result?;

            // The strip.
            let count = strip_frame_count(duration_ms, strip);
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
            Ok(duration_ms)
        })();

        match result {
            Ok(duration_ms) => {
                stats.derived += 1;
                conn.execute(
                    "UPDATE contents SET duration_ms = COALESCE(duration_ms, ?2), \
                     strip_frames = ?3, derived_at_utc = ?4 WHERE hash = ?1",
                    params![
                        hash,
                        duration_ms as i64,
                        strip_frame_count(duration_ms, strip) as i64,
                        logging::now_iso_millis()
                    ],
                )
                .map_err(|e| e.to_string())?;
            }
            Err(err) => {
                stats.failed += 1;
                conn.execute(
                    "INSERT INTO issues (path, kind, message, created_at_utc) \
                     VALUES (?1, 'video-derive-error', ?2, ?3)",
                    params![path, err, logging::now_iso_millis()],
                )
                .map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> StripConfig {
        StripConfig {
            seconds_per_frame: 20,
            min_frames: 5,
            max_frames: 20,
        }
    }

    #[test]
    fn duration_parsing_reads_the_banner_form() {
        let stderr_text = "Input #0, mov,mp4 …\n  Duration: 00:01:02.34, start: 0.0, bitrate: 1000 kb/s";
        assert_eq!(parse_duration_ms(stderr_text), Some(62_340));
        assert_eq!(parse_duration_ms("Duration: 01:00:00.00, x"), Some(3_600_000));
        assert_eq!(parse_duration_ms("no duration here"), None);
        assert_eq!(parse_duration_ms("Duration: N/A, start"), None);
    }

    #[test]
    fn strip_count_scales_with_duration_and_clamps() {
        let c = config();
        assert_eq!(strip_frame_count(10_000, &c), 5); // 10 s → min
        assert_eq!(strip_frame_count(200_000, &c), 10); // 200 s → 10
        assert_eq!(strip_frame_count(3_600_000, &c), 20); // 1 h → max
    }

    #[test]
    fn strip_timestamps_are_interior_and_even() {
        let times = strip_timestamps_ms(100_000, 4);
        assert_eq!(times, vec![20_000, 40_000, 60_000, 80_000]);
        assert!(times.first().copied().unwrap() > 0);
        assert!(times.last().copied().unwrap() < 100_000);
    }

    // Live end-to-end: installs (or reuses) ffmpeg, synthesizes a test clip
    // with lavfi testsrc, and derives poster + strip into the cache. Run with
    // `cargo test live_video_derive -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[serial_test::serial(backup_store)]
    fn live_video_derive() {
        use crate::{binaries_manager, index_store};

        let dir = tempfile::Builder::new()
            .prefix("onecopy-video-live-")
            .tempdir()
            .unwrap();
        let root = dir.path();
        let facts = binaries_manager::install_or_update(root, |p, d| eprintln!("[{p}] {d}"))
            .expect("ffmpeg install");
        eprintln!("ffmpeg {:?}", facts.installed_version);
        let ffmpeg = binaries_manager::ffmpeg_path(root);

        // Synthesize a 30 s test clip.
        let clip = root.join("clip.mp4");
        let status = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=duration=30:size=640x360:rate=24")
            .args(["-pix_fmt", "yuv420p", "-y"])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(status.success(), "test clip synthesis");

        let conn = index_store::open(&root.join("index.sqlite3")).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('vid01', 1, 'video');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
               VALUES ('{}', '{}', 'clip.mp4', 'video', 'vid01');",
            clip.display(),
            root.display()
        ))
        .unwrap();

        let cache = CachePaths::new(root.join("cache"));
        let stats = derive_videos_pending(
            &conn,
            &cache,
            Some(&ffmpeg),
            &root.join("temp"),
            320,
            1600,
            &config(),
        )
        .unwrap();
        assert_eq!((stats.derived, stats.failed), (1, 0));

        assert!(cache.thumb("vid01").exists(), "poster thumb");
        assert!(cache.preview("vid01").exists(), "poster preview");
        // 30 s at 1/20 s clamps to min 5 frames.
        for i in 0..5 {
            assert!(strip_path(&cache, "vid01", i).exists(), "strip frame {i}");
        }
        let duration: i64 = conn
            .query_row("SELECT duration_ms FROM contents WHERE hash = 'vid01'", [], |r| r.get(0))
            .unwrap();
        assert!((29_000..31_500).contains(&duration), "duration {duration}");
    }
}
