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

#[test]
fn videos_wait_when_ffmpeg_is_absent_and_never_get_checkpointed() {
    // The ffmpeg-skippable contract the wizard's offer rests on: a video the
    // app cannot derive must be BLOCKED, never failed and never checkpointed,
    // so installing ffmpeg later still picks it up. This boundary needs no
    // external tool and runs in the ordinary integration suite.
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

#[test]
fn a_poster_failure_is_returned_as_an_item_transition() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-video-failure-transition-")
        .tempdir()
        .unwrap();
    let conn = onecopy_lib::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 100, 'video');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
           VALUES ('/root/clip.mov', '/root', 'clip.mov', 'video', 'v1', 0);",
    )
    .unwrap();

    let stats = derive_videos_pending(
        &conn,
        &CachePaths::new(dir.path().join("cache")),
        Some(&dir.path().join("missing-ffmpeg")),
        &dir.path().join("temp"),
        320,
        1600,
    )
    .unwrap();

    assert_eq!((stats.derived, stats.failed), (0, 1));
    assert_eq!(stats.changed_hashes, ["v1"]);
}
