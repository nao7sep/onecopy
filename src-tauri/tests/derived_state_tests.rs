// Fixed-class output receipts and explicit attempt boundaries.

use onecopy_lib::preview::CachePaths;
use onecopy_lib::{derived_state, index_store, queries};
use derived_state::FailedOutputScope;

fn seeded() -> (tempfile::TempDir, rusqlite::Connection) {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-state-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents
           (hash, byte_size, kind, derived_at_utc, strip_frames)
         VALUES ('image', 1, 'image', 'failed', NULL),
                ('poster', 1, 'video', 'failed', NULL),
                ('strip', 1, 'video', 'ready', -1),
                ('face', 1, 'image', 'ready', NULL),
                ('speech', 1, 'video', 'ready', NULL),
                ('delete', 1, 'image', 'ready', NULL);
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
         VALUES ('/image.jpg', '/', 'image.jpg', 'image', 'image'),
                ('/poster.mov', '/', 'poster.mov', 'video', 'poster'),
                ('/strip.mov', '/', 'strip.mov', 'video', 'strip'),
                ('/face.jpg', '/', 'face.jpg', 'image', 'face'),
                ('/speech.mov', '/', 'speech.mov', 'video', 'speech'),
                ('/delete.jpg', '/', 'delete.jpg', 'image', 'delete');",
    )
    .unwrap();
    derived_state::record_face_failure(&conn, "face", "/face.jpg", "out of memory").unwrap();
    derived_state::record_transcript_failure(&conn, "speech", "/speech.mov", "decoder failed")
        .unwrap();
    for (path, kind) in [
        ("/image.jpg", "decode-error"),
        ("/poster.mov", derived_state::VIDEO_POSTER_ERROR),
        ("/strip.mov", derived_state::VIDEO_STRIP_ERROR),
        ("/delete.jpg", "delete-error"),
    ] {
        index_store::upsert_issue(&conn, Some(path), kind, "failed").unwrap();
    }
    (dir, conn)
}

#[test]
fn explicit_attempt_boundary_reopens_failures_without_using_or_erasing_issues() {
    let (_dir, conn) = seeded();
    // Dismissal/history cannot decide whether an output is eligible again.
    conn.execute("DELETE FROM issues", []).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc, strip_frames)
         VALUES ('waiting', 1, 'image', 'needs-ffmpeg', NULL),
                ('ready', 1, 'video', 'ready', 8);
         INSERT INTO analysis_receipts (content_hash, transcript_state, face_state)
         VALUES ('ready', 'ready-empty', 'ready');",
    )
    .unwrap();
    assert_eq!(
        derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap(),
        5
    );
    assert_eq!(
        derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap(),
        0
    );
    let preserved: (String, i64, String, String, String) = conn.query_row(
        "SELECT c.derived_at_utc, c.strip_frames, r.transcript_state, r.face_state,
          (SELECT derived_at_utc FROM contents WHERE hash = 'waiting')
         FROM contents c JOIN analysis_receipts r ON r.content_hash = c.hash WHERE c.hash = 'ready'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    ).unwrap();
    assert_eq!(
        preserved,
        (
            "ready".into(),
            8,
            "ready-empty".into(),
            "ready".into(),
            "needs-ffmpeg".into()
        )
    );
    assert_eq!(queries::issues(&conn, 20).unwrap().0, 0);
}

#[test]
fn section_attempt_reset_uses_logical_kind_and_half_open_dates_not_shared_folders() {
    let (_dir, conn) = seeded();
    conn.execute_batch(
        "UPDATE paths SET resolved_source = 'filename', resolved_utc_ms = 100 WHERE content_hash IN ('image', 'face');
         UPDATE paths SET resolved_source = 'filename', resolved_utc_ms = 200 WHERE content_hash = 'face';",
    ).unwrap();
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: "image",
                bounds: Some((100, 200))
            }
        )
        .unwrap(),
        1
    );
    let states: (Option<String>, String, String) = conn
        .query_row(
            "SELECT (SELECT derived_at_utc FROM contents WHERE hash = 'image'),
          (SELECT face_state FROM analysis_receipts WHERE content_hash = 'face'),
          (SELECT derived_at_utc FROM contents WHERE hash = 'poster')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(states, (None, "failed".into(), "failed".into()));
    assert_eq!(
        queries::issues(&conn, 20).unwrap().0,
        6,
        "reopening work does not dismiss its diagnostics"
    );
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: "video",
                bounds: None
            }
        )
        .unwrap(),
        3
    );
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: "image",
                bounds: None
            }
        )
        .unwrap(),
        0
    );
}

#[test]
fn audio_transcription_is_reopened_by_its_other_files_section() {
    let (_dir, conn) = seeded();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('audio', 1, 'audio');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
         VALUES ('/speech.wav', '/', 'speech.wav', 'audio', 'audio');",
    )
    .unwrap();
    derived_state::record_transcript_failure(&conn, "audio", "/speech.wav", "failed").unwrap();
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: "other",
                bounds: None
            }
        )
        .unwrap(),
        1
    );
    let states: (Option<String>, String) = conn
        .query_row(
            "SELECT (SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'audio'),
          (SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'speech')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(states, (None, "failed".into()));
}

#[test]
fn opening_database_and_querying_section_do_not_repeat_a_failed_new_attempt() {
    let (dir, conn) = seeded();
    derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap();
    derived_state::record_transcript_failure(&conn, "speech", "/speech.mov", "new attempt failed")
        .unwrap();
    drop(conn);
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for _ in 0..3 {
        queries::section_dirs(&conn, "video", "undated", chrono_tz::UTC).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'speech'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "failed"
        );
    }
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: "video",
                bounds: None
            }
        )
        .unwrap(),
        1
    );
}

#[test]
fn requested_preview_honors_failure_until_explicit_reset_even_if_file_is_now_valid() {
    let (dir, conn) = seeded();
    let path = dir.path().join("fixed.png");
    image::RgbImage::new(2, 2).save(&path).unwrap();
    conn.execute(
        "UPDATE paths SET abs_path = ?1 WHERE content_hash = 'image'",
        [path.to_string_lossy().as_ref()],
    )
    .unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let before = queries::issues(&conn, 20).unwrap().0;
    for _ in 0..2 {
        let error =
            onecopy_lib::preview::derive_one(&conn, &cache, 32, 64, None, "image").unwrap_err();
        assert!(error.contains("Recheck this section"));
    }
    assert!(!cache.preview("image").exists());
    assert_eq!(queries::issues(&conn, 20).unwrap().0, before);
    derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap();
    assert_eq!(
        onecopy_lib::preview::derive_one(&conn, &cache, 32, 64, None, "image").unwrap(),
        "image"
    );
    assert!(cache.preview("image").exists());
}

#[test]
fn failed_replacement_keeps_its_completed_transcript_across_reattempt_boundaries() {
    let (_dir, conn) = seeded();
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", true).unwrap();
    derived_state::record_transcript_replacement_failure(
        &conn,
        "/speech.mov",
        "replacement failed",
    )
    .unwrap();
    derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'speech'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "ready-text"
    );
    assert!(queries::issues(&conn, 20)
        .unwrap()
        .1
        .iter()
        .any(|row| row.kind == derived_state::TRANSCRIPT_ERROR));
}

#[test]
fn resource_safety_issue_is_not_attached_to_one_file() {
    let (_dir, conn) = seeded();
    index_store::upsert_issue(
        &conn,
        None,
        "resource-limit-video-transcripts",
        "Transcription needs more available memory",
    )
    .unwrap();

    let (_, rows) = queries::issues(&conn, 20).unwrap();
    let issue = rows
        .iter()
        .find(|row| row.kind == "resource-limit-video-transcripts")
        .unwrap();
    assert!(issue.path.is_none());
    assert!(issue.message.as_ref().unwrap().contains("memory"));
}

#[test]
fn successful_analysis_records_value_or_empty_and_retires_its_issue() {
    let (_dir, conn) = seeded();
    derived_state::record_face_success(&conn, "face", "/face.jpg", 0.0).unwrap();
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", false).unwrap();

    let receipt: (String, String) = conn
        .query_row(
            "SELECT a.face_state, b.transcript_state FROM analysis_receipts a
             JOIN analysis_receipts b ON b.content_hash = 'speech'
             WHERE a.content_hash = 'face'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, ("ready".to_string(), "ready-empty".to_string()));
    let (_, rows) = queries::issues(&conn, 20).unwrap();
    assert!(!rows.iter().any(|row| matches!(
        row.kind.as_str(),
        derived_state::FACE_ERROR | derived_state::TRANSCRIPT_ERROR
    )));
}

#[test]
fn preview_poster_and_snapshot_transitions_retire_their_current_issue() {
    let (_dir, conn) = seeded();

    derived_state::record_preview_success(&conn, "image", "/image.jpg", 4000, 3000, 12.5, 42)
        .unwrap();
    derived_state::record_poster_success(&conn, "poster", "/poster.mov", 30_000).unwrap();
    derived_state::record_strip_success(&conn, "strip", "/strip.mov", 8).unwrap();

    let state: (Option<String>, i64, Option<String>, i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT derived_at_utc FROM contents WHERE hash = 'image'),
               (SELECT derived_version FROM contents WHERE hash = 'image'),
               (SELECT derived_at_utc FROM contents WHERE hash = 'poster'),
               (SELECT duration_ms FROM contents WHERE hash = 'poster'),
               (SELECT strip_frames FROM contents WHERE hash = 'strip')",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert!(state.0.is_some());
    assert_eq!(state.1, derived_state::DERIVE_VERSION);
    assert!(state.2.is_some());
    assert_eq!((state.3, state.4), (30_000, 8));

    let (_, issues) = queries::issues(&conn, 20).unwrap();
    assert!(!issues.iter().any(|row| matches!(
        row.kind.as_str(),
        derived_state::PREVIEW_ERROR
            | derived_state::VIDEO_POSTER_ERROR
            | derived_state::VIDEO_STRIP_ERROR
    )));
}

#[test]
fn preview_poster_and_snapshot_failures_checkpoint_once_for_retry() {
    let (_dir, conn) = seeded();
    derived_state::record_preview_failure(&conn, "image", "/image.jpg", "decode").unwrap();
    derived_state::record_poster_failure(&conn, "poster", "/poster.mov", "poster").unwrap();
    derived_state::record_strip_failure(&conn, "strip", "/strip.mov", "strip").unwrap();

    let state: (String, String, i64) = conn
        .query_row(
            "SELECT
               (SELECT derived_at_utc FROM contents WHERE hash = 'image'),
               (SELECT derived_at_utc FROM contents WHERE hash = 'poster'),
               (SELECT strip_frames FROM contents WHERE hash = 'strip')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, ("failed".into(), "failed".into(), -1));

    let (_, issues) = queries::issues(&conn, 20).unwrap();
    for kind in [
        derived_state::PREVIEW_ERROR,
        derived_state::VIDEO_POSTER_ERROR,
        derived_state::VIDEO_STRIP_ERROR,
    ] {
        assert!(issues.iter().any(|row| row.kind == kind));
    }
}

#[test]
fn transcript_reads_distinguish_pending_failed_empty_and_missing_output() {
    let (dir, conn) = seeded();
    let cache = CachePaths::new(dir.path().join("cache"));

    let failed = derived_state::transcript_result(&conn, &cache, "speech").unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.message.as_deref(),
        Some(
            "OneCopy could not transcribe this media file. The original file was not changed. Recheck its section to try again."
        )
    );

    let pending = derived_state::transcript_result(&conn, &cache, "poster").unwrap();
    assert_eq!(pending.status, "pending");

    let legacy = cache.transcript("poster");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "[0:01] kept\n").unwrap();
    let adopted = derived_state::transcript_result(&conn, &cache, "poster").unwrap();
    assert_eq!(adopted.status, "ready");
    assert_eq!(adopted.text.as_deref(), Some("[0:01] kept\n"));

    let target = cache.transcript("speech");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "").unwrap();
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", false).unwrap();
    let empty = derived_state::transcript_result(&conn, &cache, "speech").unwrap();
    assert_eq!(empty.status, "ready");
    assert_eq!(empty.text.as_deref(), Some(""));

    std::fs::remove_file(target).unwrap();
    let repaired = derived_state::transcript_result(&conn, &cache, "speech").unwrap();
    assert_eq!(repaired.status, "pending");
    let state: Option<String> = conn
        .query_row(
            "SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'speech'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, None);
}

#[test]
fn derived_failure_issues_keep_hostile_diagnostics_out_of_user_copy() {
    let (_dir, conn) = seeded();
    let hostile =
        "DecoderError EACCES /private/tmp/HOSTILE-SENTINEL [52, 49, 46, 46]";

    derived_state::record_preview_failure(&conn, "image", "/image.jpg", hostile).unwrap();
    derived_state::record_poster_failure(&conn, "poster", "/poster.mov", hostile).unwrap();
    derived_state::record_strip_failure(&conn, "strip", "/strip.mov", hostile).unwrap();
    derived_state::record_face_failure(&conn, "face", "/face.jpg", hostile).unwrap();
    derived_state::record_transcript_failure(&conn, "speech", "/speech.mov", hostile).unwrap();

    let (_, issues) = queries::issues(&conn, 20).unwrap();
    for kind in [
        derived_state::PREVIEW_ERROR,
        derived_state::VIDEO_POSTER_ERROR,
        derived_state::VIDEO_STRIP_ERROR,
        derived_state::FACE_ERROR,
        derived_state::TRANSCRIPT_ERROR,
    ] {
        let message = issues
            .iter()
            .find(|row| row.kind == kind)
            .unwrap()
            .message
            .as_deref()
            .unwrap();
        assert!(message.starts_with("OneCopy could not"));
        assert!(message.contains("original file was not changed"));
        assert!(!message.contains("HOSTILE-SENTINEL"));
        assert!(!message.contains("EACCES"));
        assert!(!message.contains("/private/tmp"));
        assert!(!message.contains("DecoderError"));
    }
}
