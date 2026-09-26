// Video preparation through the managed ffmpeg: every container and codec
// OneCopy accepts yields an upright poster, its recorded duration, and its
// snapshot strip. The corpus includes a transport stream whose timeline starts
// above zero and short clips whose last strip timestamp falls after the final
// frame, both of which have broken frame extraction before.

use std::path::Path;

use onecopy_lib::extensions::{lowercase_ext, VIDEO_EXTENSIONS};
use onecopy_lib::video;

use super::heavy_support::{corpus_file, library, manifest};

/// Container durations agree with ffprobe's to within this many milliseconds.
const DURATION_TOLERANCE_MS: i64 = 100;
/// Posters are scaled, so their aspect ratio matches the video's to within
/// rounding of the scaled edge.
const ASPECT_TOLERANCE: f64 = 0.02;

/// The aspect ratio a viewer sees: rotation metadata turns the stored frame.
fn displayed_aspect(stream: &serde_json::Value) -> f64 {
    let width = stream["width"].as_f64().unwrap();
    let height = stream["height"].as_f64().unwrap();
    let quarter_turn = stream["side_data_list"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|side| side["rotation"].as_i64())
        .any(|rotation| rotation.rem_euclid(180) == 90);
    if quarter_turn {
        height / width
    } else {
        width / height
    }
}

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn every_accepted_video_format_yields_an_upright_poster_duration_and_strip() {
    let videos: Vec<serde_json::Value> = manifest()
        .into_iter()
        .filter(|entry| {
            let path = entry["path"].as_str().unwrap();
            path.starts_with("video/codecs/")
                && VIDEO_EXTENSIONS.contains(&lowercase_ext(path).as_str())
        })
        .collect();
    let files: Vec<_> = videos
        .iter()
        .map(|entry| corpus_file(entry["path"].as_str().unwrap()))
        .collect();
    let library = library(
        "videos",
        &files,
        serde_json::json!({ "videoSnapshotsEnabled": true }),
    );
    let contents: i64 = library
        .conn
        .query_row(
            "SELECT COUNT(DISTINCT content_hash) FROM paths WHERE kind = 'video'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let settings = &library.settings;
    let posters = video::derive_videos_pending(
        &library.conn,
        &library.cache,
        Some(library.ffmpeg()),
        &settings.temp_dir,
        settings.thumb_edge,
        settings.preview_long_edge,
    )
    .unwrap();
    assert_eq!((posters.derived, posters.failed), (contents as u64, 0));

    let (mut completed, mut failed, mut after) = (0, 0, None);
    loop {
        let strips = video::derive_strips_pending(
            &library.conn,
            &library.cache,
            library.ffmpeg(),
            &settings.temp_dir,
            &settings.strip,
            settings.preview_long_edge,
            &[],
            &|_| {},
            &|_| {},
            after.as_deref(),
            &|| false,
            &|_, _| {},
        )
        .unwrap();
        completed += strips.completed;
        failed += strips.failed;
        if !strips.candidates_found {
            break;
        }
        after = strips.last_attempted_hash;
    }
    assert_eq!((completed, failed), (contents as u64, 0));

    for entry in &videos {
        let path = entry["path"].as_str().unwrap();
        let file_name = Path::new(path).file_name().unwrap().to_str().unwrap();
        let hash = library.hash_of(file_name);
        assert!(library.cache.thumb(&hash).is_file(), "{path} has a poster thumbnail");
        assert!(library.cache.preview(&hash).is_file(), "{path} has a poster preview");

        let (duration_ms, strip_frames): (i64, i64) = library
            .conn
            .query_row(
                "SELECT duration_ms, strip_frames FROM contents WHERE hash = ?1",
                [&hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let recorded_ms = (entry["probe"]["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            * 1000.0)
            .round() as i64;
        assert!(
            (duration_ms - recorded_ms).abs() <= DURATION_TOLERANCE_MS,
            "{path}: duration {duration_ms} ms, recorded {recorded_ms} ms"
        );

        let stream = entry["probe"]["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .unwrap();
        // A poster smaller than the preview edge keeps its staged JPEG bytes
        // rather than being re-encoded, so the format is read from content.
        let (width, height) = image::ImageReader::open(library.cache.preview(&hash))
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        let poster_aspect = f64::from(width) / f64::from(height);
        let expected_aspect = displayed_aspect(stream);
        assert!(
            (poster_aspect - expected_aspect).abs() <= ASPECT_TOLERANCE * expected_aspect,
            "{path}: poster is {width}×{height}, displayed aspect {expected_aspect:.3}"
        );

        let expected_frames = video::strip_frame_count(duration_ms as u64, &settings.strip);
        assert_eq!(strip_frames, i64::from(expected_frames), "{path}");
        for index in 0..expected_frames {
            assert!(
                video::strip_path(&library.cache, &hash, index).is_file(),
                "{path}: strip frame {index}"
            );
        }
    }
}

/// Writes a synthetic video through `ffmpeg`, its output to `target`.
fn synthesize(ffmpeg: &Path, args: &[&str], target: &Path) {
    let output = std::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    if args.last() == Some(&"pipe:1") {
        std::fs::write(target, output.stdout).unwrap();
    }
}

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn an_8k_video_and_one_without_a_container_duration_both_get_posters() {
    let staging = library("synthetic-staging", &[], serde_json::json!({}));
    let made = staging.home.path().join("made");
    std::fs::create_dir_all(&made).unwrap();
    // Its full frame exceeds the in-process decode ceiling on every platform.
    let eight_k = made.join("eight-k.mov");
    synthesize(
        staging.ffmpeg(),
        &["-f", "lavfi", "-i", "testsrc=size=7680x4320:rate=1:duration=2", "-c:v", "mjpeg", "-q:v", "8", "-y", eight_k.to_str().unwrap()],
        &eight_k,
    );
    // Written live, as a browser recorder does, so it reports no duration.
    let live = made.join("live.mkv");
    synthesize(
        staging.ffmpeg(),
        &["-f", "lavfi", "-i", "testsrc=size=320x240:rate=5:duration=2", "-c:v", "mjpeg", "-f", "matroska", "pipe:1"],
        &live,
    );
    let library = library(
        "synthetic-videos",
        &[eight_k, live],
        serde_json::json!({ "videoSnapshotsEnabled": true }),
    );
    let settings = &library.settings;
    let posters = video::derive_videos_pending(
        &library.conn,
        &library.cache,
        Some(library.ffmpeg()),
        &settings.temp_dir,
        settings.thumb_edge,
        settings.preview_long_edge,
    )
    .unwrap();
    assert_eq!((posters.derived, posters.failed), (2, 0));

    let eight_k = library.hash_of("eight-k.mov");
    let (width, height) = image::ImageReader::open(library.cache.preview(&eight_k))
        .unwrap()
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap();
    assert_eq!(width.max(height), settings.preview_long_edge);

    let live = library.hash_of("live.mkv");
    assert!(library.cache.preview(&live).is_file());
    let (duration_ms, strip_frames): (Option<i64>, Option<i64>) = library
        .conn
        .query_row(
            "SELECT duration_ms, strip_frames FROM contents WHERE hash = ?1",
            [&live],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // No snapshots can be placed on an unknown timeline, and nothing failed.
    assert_eq!((duration_ms, strip_frames), (None, Some(0)));
    let candidates = onecopy_lib::derived_state::transcript_candidates(&library.conn, "video", None, 10).unwrap();
    assert!(candidates.iter().any(|(hash, _)| *hash == live));
}
