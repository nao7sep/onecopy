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
        .query_row("SELECT COUNT(*) FROM active_issues", [], |r| r.get(0))
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

    let source = dir.path().join("clip.mov");
    std::fs::write(&source, b"invalid-video").unwrap();
    conn.execute("UPDATE paths SET abs_path = ?1", [source.to_string_lossy().as_ref()]).unwrap();

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

#[cfg(unix)]
#[test]
fn poster_frames_stream_through_stdout_and_release_their_staging_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let conn = onecopy_lib::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let source = dir.path().join("clip.mov");
    std::fs::write(&source, b"video-source").unwrap();
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('v1', 12, 'video')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES (?1, ?2, 'clip.mov', 'video', 'v1', 0)",
        rusqlite::params![source.to_string_lossy(), dir.path().to_string_lossy()],
    ).unwrap();
    let frame = dir.path().join("frame.jpg");
    image::DynamicImage::new_rgb8(8, 8).save(&frame).unwrap();
    let ffmpeg = dir.path().join("ffmpeg");
    std::fs::write(
        &ffmpeg,
        format!(
            "#!/bin/sh\nfor last do :; done\nif [ \"$last\" = pipe:1 ]; then\ncat '{}'\nelse\necho 'Duration: 00:00:10.00, start: 0' >&2\nexit 1\nfi\n",
            frame.display(),
        ),
    ).unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let temp = dir.path().join("temp");

    let stats = derive_videos_pending(&conn, &cache, Some(&ffmpeg), &temp, 320, 1600).unwrap();

    assert_eq!((stats.derived, stats.failed), (1, 0));
    assert!(cache.preview("v1").is_file());
    assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn incomplete_snapshot_attempts_remove_only_their_published_frames() {
    use std::os::unix::fs::PermissionsExt;
    for terminal in ["extract", "decode", "record", "success"] {
        let dir = tempfile::tempdir().unwrap();
        let conn = onecopy_lib::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let source = dir.path().join("clip.mov");
        std::fs::write(&source, b"video-source").unwrap();
        conn.execute(
            "INSERT INTO contents (hash, byte_size, kind, duration_ms, derived_at_utc, derived_version)
             VALUES ('v1', 12, 'video', 10000, '2026-01-01T00:00:00.000Z', ?1)",
            [onecopy_lib::derived_state::DERIVE_VERSION],
        ).unwrap();
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
             VALUES (?1, ?2, 'clip.mov', 'video', 'v1', 0)",
            rusqlite::params![source.to_string_lossy(), dir.path().to_string_lossy()],
        ).unwrap();
        if terminal == "record" {
            conn.execute_batch("CREATE TRIGGER refuse_strip_receipt BEFORE UPDATE OF strip_frames ON contents
                WHEN NEW.strip_frames > 0 BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;").unwrap();
        }
        let frame = dir.path().join("frame.jpg");
        image::DynamicImage::new_rgb8(8, 8).save(&frame).unwrap();
        let ffmpeg = dir.path().join("ffmpeg");
        let later = match terminal {
            "extract" => "echo 'damaged later frame' >&2; exit 1",
            "decode" => "printf 'invalid image'; exit 0",
            _ => ":",
        };
        std::fs::write(&ffmpeg, format!(
            "#!/bin/sh\nif [ -e \"$0.calls\" ]; then\necho later >> \"$0.calls\"\n{later}\nfi\necho frame >> \"$0.calls\"\ncat '{}'\n", frame.display(),
        )).unwrap();
        std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));
        // Other completed sets, and paths this attempt never writes, survive.
        let prior = strip_path(&cache, "prior", 0);
        let untouched = strip_path(&cache, "v1", 8);
        for path in [&prior, &untouched] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"retained previous output").unwrap();
        }
        let temp = dir.path().join("temp");
        let result = derive_strips_pending(&conn, &cache, &ffmpeg, &temp,
            &StripConfig { seconds_per_frame: 20, min_frames: 2, max_frames: 2 },
            64, &[], &|_| {}, &|_| {}, None, &|| false, &|_, _| {});
        if terminal == "record" { assert!(result.unwrap_err().contains("injected receipt failure")); }
        else {
            let stats = result.unwrap();
            assert_eq!(stats.attempted, 1);
            assert_eq!(stats.completed, u64::from(terminal == "success"));
            assert_eq!(stats.failed, u64::from(terminal != "success"));
        }
        assert!(std::fs::read_to_string(ffmpeg.with_extension("calls")).unwrap().contains("later"),
            "the first frame must succeed before testing the later terminal path");
        for index in 0..2 {
            assert_eq!(strip_path(&cache, "v1", index).exists(), terminal == "success", "{terminal}, frame {index}");
        }
        assert_eq!(std::fs::read(prior).unwrap(), b"retained previous output");
        assert_eq!(std::fs::read(untouched).unwrap(), b"retained previous output");
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
    }
}

#[test]
fn unavailable_video_sources_request_polling_but_missing_tools_do_not() {
    let dir = tempfile::tempdir().unwrap();
    let conn = onecopy_lib::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch("INSERT INTO contents(hash, byte_size, kind) VALUES ('v1', 10, 'video');
        INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash)
        VALUES ('/unavailable/clip.mov', '/unavailable', 'clip.mov', 'video', 'v1');").unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let temp = dir.path().join("temp");
    let ffmpeg = dir.path().join("unused-ffmpeg");
    let blocked = derive_videos_pending(&conn, &cache, None, &temp, 32, 64).unwrap();
    assert!(!blocked.waiting_source);
    let offline = derive_videos_pending(&conn, &cache, Some(&ffmpeg), &temp, 32, 64).unwrap();
    assert!(offline.waiting_source);
    assert_eq!((offline.derived, offline.failed), (0, 0));
    conn.execute("UPDATE contents SET derived_at_utc = '2026-01-01T00:00:00.000Z', derived_version = ?1, duration_ms = 10000",
        [onecopy_lib::derived_state::DERIVE_VERSION]).unwrap();
    let strips = derive_strips_pending(&conn, &cache, &ffmpeg, &temp, &config(), 64,
        &[], &|_| {}, &|_| {}, None, &|| false, &|_, _| {}).unwrap();
    assert!(strips.waiting_source);
    assert_eq!(strips.attempted, 0);
    conn.execute("UPDATE paths SET missing = 1", []).unwrap();
    let absent = derive_strips_pending(&conn, &cache, &ffmpeg, &temp, &config(), 64,
        &[], &|_| {}, &|_| {}, None, &|| false, &|_, _| {}).unwrap();
    assert!(!absent.waiting_source);
    assert!(!absent.candidates_found);
}
