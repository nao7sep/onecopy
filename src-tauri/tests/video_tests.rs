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
    let facts = binaries_manager::install_entry(root, "ffmpeg", |p, d| eprintln!("[{p}] {d}"))
        .expect("ffmpeg install");
    eprintln!("ffmpeg {:?}", facts.latest_known_version);
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
    )
    .unwrap();
    assert_eq!((stats.derived, stats.failed), (1, 0));

    assert!(cache.thumb("vid01").exists(), "poster thumb");
    assert!(cache.preview("vid01").exists(), "poster preview");
    // The scan half leaves strips PENDING (Phase 33: they are the idle
    // coordinator's idle job) — strip_frames stays NULL until that pass runs.
    let pending: Option<i64> = conn
        .query_row("SELECT strip_frames FROM contents WHERE hash = 'vid01'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, None, "strips are pending, not scan work");

    let done = derive_strips_pending(
        &conn,
        &cache,
        &ffmpeg,
        &root.join("temp"),
        &config(),
        None,
        &|| false,
        &|_, _| {},
    )
    .unwrap();
    assert_eq!(done.completed, 1);
    // 30 s at 1/20 s clamps to min 5 frames.
    for i in 0..5 {
        assert!(strip_path(&cache, "vid01", i).exists(), "strip frame {i}");
    }
    let duration: i64 = conn
        .query_row("SELECT duration_ms FROM contents WHERE hash = 'vid01'", [], |r| r.get(0))
        .unwrap();
    assert!((29_000..31_500).contains(&duration), "duration {duration}");
}

#[test]
fn videos_wait_when_ffmpeg_is_absent_and_never_get_checkpointed() {
    // The ffmpeg-skippable contract the wizard's offer rests on: a video the
    // app cannot derive must be BLOCKED, never failed and never checkpointed,
    // so installing ffmpeg later still picks it up. Needs no ffmpeg, so unlike
    // the four #[ignore]d live tests this runs on every `cargo test`.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-video-noffmpeg-")
        .tempdir()
        .unwrap();
    let conn = onecopy_lib::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 100, 'video')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing) \
         VALUES ('/root/clip.mov', '/root', 'clip.mov', 'video', 'v1', 0)",
        [],
    )
    .unwrap();

    let run = || {
        derive_videos_pending(
            &conn,
            &cache,
            None,
            &dir.path().join("temp"),
            320,
            1600,
        )
        .unwrap()
    };

    let stats = run();
    assert!(stats.skipped_no_ffmpeg, "the skip must be reported honestly");
    assert_eq!((stats.derived, stats.failed), (0, 0), "nothing failed");

    let derived_at: Option<String> = conn
        .query_row("SELECT derived_at_utc FROM contents WHERE hash = 'v1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        derived_at, None,
        "a blocked video must stay pending, not be checkpointed"
    );
    let issues: i64 = conn
        .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
        .unwrap();
    assert_eq!(issues, 0, "nothing is wrong with the file — no issue row");

    // Idempotent: a second pass behaves identically.
    let again = run();
    assert!(again.skipped_no_ffmpeg);
    assert_eq!((again.derived, again.failed), (0, 0));
    assert_eq!(issues, 0);
}
