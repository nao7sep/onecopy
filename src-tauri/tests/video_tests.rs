// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::preview::CachePaths;
use onecopy_lib::video::*;

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
    use onecopy_lib::{binaries_manager, index_store};

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
