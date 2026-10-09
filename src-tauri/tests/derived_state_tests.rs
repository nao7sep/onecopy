// Fixed-class output receipts and explicit attempt boundaries.

use onecopy_lib::preview::CachePaths;
use onecopy_lib::transcription::{Segment, Transcript};
use onecopy_lib::{derived_state, index_store, queries};
use derived_state::FailedOutputScope;

fn spoken(text: &str) -> Transcript {
    Transcript {
        language: Some("en".to_string()),
        segments: vec![Segment { start_ms: 0, end_ms: 1_000, text: text.to_string() }],
    }
}

fn whisper() -> onecopy_lib::ai_dependencies::ModelIdentity {
    onecopy_lib::ai_dependencies::transcription_model()
}

/// The face and transcript states the item projections read for `hash`.
fn analysis_states(conn: &rusqlite::Connection, hash: &str) -> (Option<String>, Option<String>) {
    conn.query_row(
        &format!(
            "SELECT {}, {} FROM contents c WHERE c.hash = ?1",
            derived_state::face_state_sql("c"),
            derived_state::transcript_state_sql("c")
        ),
        [hash],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .unwrap()
}

fn seeded() -> (tempfile::TempDir, rusqlite::Connection) {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derived-state-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents
           (hash, byte_size, kind, derived_at_utc, derive_outcome, strip_frames)
         VALUES ('image', 1, 'image', NULL, 'failed', NULL),
                ('poster', 1, 'video', NULL, 'failed', NULL),
                ('strip', 1, 'video', 'ready', NULL, -1),
                ('face', 1, 'image', 'ready', NULL, NULL),
                ('speech', 1, 'video', 'ready', NULL, NULL),
                ('delete', 1, 'image', 'ready', NULL, NULL);
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
fn failure_recording_keeps_a_competing_log_write_out_until_commit() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{Arc, Mutex};

    for class in ["preview", "face"] {
        let (dir, conn) = seeded();
        let records_path = dir.path().join("records.sqlite3");
        let logger = onecopy_lib::records::open(&records_path).unwrap();
        // Probe the held lock, not how long the logger waits for it.
        logger.busy_timeout(std::time::Duration::ZERO).unwrap();
        let outcome = Arc::new(Mutex::new(None));
        let observed = outcome.clone();
        let table = if class == "preview" { "issue_events" } else { "analysis_events" };
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            if context.database_name == Some("records")
                && matches!(context.action, AuthAction::Insert { table_name } if table_name == table)
            {
                let mut result = observed.lock().unwrap();
                if result.is_none() {
                    *result = Some(logger.execute(
                        "INSERT INTO log_lines (session_id, time_utc, level, message, line)
                         VALUES ('test', 'now', 'warn', 'decoder failed', '{}')", [],
                    ));
                }
            }
            Authorization::Allow
        })).unwrap();

        let recorded = if class == "preview" {
            derived_state::record_preview_failure(&conn, "image", "/image.jpg", "decode")
        } else {
            derived_state::record_face_failure(&conn, "face", "/face.jpg", "decode")
        };
        assert!(recorded.is_ok(), "{class}: {recorded:?}");
        let probe = outcome.lock().unwrap().take().expect("competing writer was exercised");
        assert_eq!(probe.unwrap_err().sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseBusy));

        // After the result commits, the independent logging connection can write.
        let logger = onecopy_lib::records::open(&records_path).unwrap();
        logger.execute(
            "INSERT INTO log_lines (session_id, time_utc, level, message, line)
             VALUES ('test', 'now', 'warn', 'decoder failed', '{}')", [],
        ).unwrap();
    }
}

#[test]
fn an_unsaved_failure_still_propagates_and_rolls_back_its_result() {
    let (_dir, conn) = seeded();
    conn.execute_batch(
        "UPDATE contents SET derive_outcome = NULL WHERE hash = 'image';
         CREATE TEMP TRIGGER reject_issue BEFORE INSERT ON records.issue_events
         BEGIN SELECT RAISE(ABORT, 'injected records write failure'); END;",
    ).unwrap();
    let result = derived_state::record_preview_failure(&conn, "image", "/image.jpg", "decode");
    assert!(result.unwrap_err().contains("injected records write failure"));
    let outcome: Option<String> = conn.query_row(
        "SELECT derive_outcome FROM contents WHERE hash = 'image'", [], |row| row.get(0),
    ).unwrap();
    assert_eq!(outcome, None);
}

#[test]
fn explicit_attempt_boundary_reopens_failures_without_using_or_erasing_issues() {
    let (_dir, conn) = seeded();
    // Dismissal/history cannot decide whether an output is eligible again.
    index_store::dismiss_issues(&conn, None).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc, derive_outcome, strip_frames)
         VALUES ('waiting', 1, 'image', NULL, 'needs-ffmpeg', NULL),
                ('ready', 1, 'video', 'ready', NULL, 8);
         INSERT INTO transcripts (content_hash, model, model_version, text, segments, created_at_utc)
         VALUES ('ready', 'm', 'v', '', '[]', 'now');
         INSERT INTO face_checks (content_hash, model, model_version, face_count, checked_at_utc)
         VALUES ('ready', 'm', 'v', 0, 'now');",
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
    let preserved: (String, i64, String) = conn.query_row(
        "SELECT c.derived_at_utc, c.strip_frames,
          (SELECT derive_outcome FROM contents WHERE hash = 'waiting')
         FROM contents c WHERE c.hash = 'ready'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    assert_eq!(preserved, ("ready".into(), 8, "needs-ffmpeg".into()));
    assert_eq!(
        analysis_states(&conn, "ready"),
        (Some("ready".into()), Some("ready-empty".into()))
    );
    assert_eq!(analysis_states(&conn, "face"), (None, None));
    assert_eq!(queries::issues(&conn, 20, None).unwrap().0, 0);
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
                kind: onecopy_lib::queries::SectionKind::Image,
                bounds: Some((100, 200))
            }
        )
        .unwrap(),
        1
    );
    let states: (Option<String>, String) = conn
        .query_row(
            "SELECT (SELECT derive_outcome FROM contents WHERE hash = 'image'),
          (SELECT derive_outcome FROM contents WHERE hash = 'poster')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(states, (None, "failed".into()));
    assert_eq!(analysis_states(&conn, "face").0.as_deref(), Some("failed"));
    assert_eq!(
        queries::issues(&conn, 20, None).unwrap().0,
        6,
        "reopening work does not dismiss its diagnostics"
    );
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: onecopy_lib::queries::SectionKind::Video,
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
                kind: onecopy_lib::queries::SectionKind::Image,
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
                kind: onecopy_lib::queries::SectionKind::Other,
                bounds: None
            }
        )
        .unwrap(),
        1
    );
    assert_eq!(analysis_states(&conn, "audio").1, None);
    assert_eq!(analysis_states(&conn, "speech").1.as_deref(), Some("failed"));
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
        queries::section_dirs(&conn, onecopy_lib::queries::SectionKind::Video, "undated", chrono_tz::UTC).unwrap();
        assert_eq!(analysis_states(&conn, "speech").1.as_deref(), Some("failed"));
    }
    assert_eq!(
        derived_state::reset_failed_outputs(
            &conn,
            FailedOutputScope::Section {
                kind: onecopy_lib::queries::SectionKind::Video,
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
    let before = queries::issues(&conn, 20, None).unwrap().0;
    for _ in 0..2 {
        let error =
            onecopy_lib::preview::derive_one(&conn, &cache, 32, 64, None, "image").unwrap_err();
        assert!(error.contains("Recheck this section"));
    }
    assert!(!cache.preview("image").exists());
    assert_eq!(queries::issues(&conn, 20, None).unwrap().0, before);
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
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", &spoken("kept"), whisper()).unwrap();
    derived_state::record_transcript_replacement_failure(
        &conn,
        "/speech.mov",
        "replacement failed",
    )
    .unwrap();
    derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap();
    assert_eq!(analysis_states(&conn, "speech").1.as_deref(), Some("ready-text"));
    assert!(queries::issues(&conn, 20, None)
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

    let (_, rows) = queries::issues(&conn, 20, None).unwrap();
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
    derived_state::record_face_success(&conn, "face", "/face.jpg", &[], &onecopy_lib::ai_dependencies::face_model()).unwrap();
    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", &Transcript::default(), whisper()).unwrap();

    assert_eq!(analysis_states(&conn, "face").0.as_deref(), Some("ready"));
    assert_eq!(analysis_states(&conn, "speech").1.as_deref(), Some("ready-empty"));
    let (_, rows) = queries::issues(&conn, 20, None).unwrap();
    assert!(!rows.iter().any(|row| matches!(
        row.kind.as_str(),
        derived_state::FACE_ERROR | derived_state::TRANSCRIPT_ERROR
    )));
}

#[test]
fn preview_poster_and_snapshot_transitions_retire_their_current_issue() {
    let (_dir, conn) = seeded();

    // Each success here resolves the matching Issue seeded above: the
    // Issues surface must be told even though nothing in this batch failed
    // (C-M3), so the record_* calls report the resolution back to the caller.
    assert!(
        derived_state::record_preview_success(&conn, "image", "/image.jpg", 4000, 3000, 12.5, 42)
            .unwrap()
    );
    assert!(derived_state::record_poster_success(&conn, "poster", "/poster.mov", Some(30_000)).unwrap());
    assert!(derived_state::record_strip_success(&conn, "strip", "/strip.mov", 8).unwrap());

    let state: (Option<String>, i64, Option<String>, i64, i64) = conn
        .query_row(
            "SELECT
               (SELECT derived_at_utc FROM contents WHERE hash = 'image' AND derive_outcome IS NULL),
               (SELECT derived_version FROM contents WHERE hash = 'image'),
               (SELECT derived_at_utc FROM contents WHERE hash = 'poster' AND derive_outcome IS NULL),
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

    let (_, issues) = queries::issues(&conn, 20, None).unwrap();
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
    // `seeded()` already opened each of these Issues, so a repeated failure
    // only bumps the existing entry's occurrence count: nothing the Issues
    // surface has not already shown, so no reload is needed (C-M3).
    assert!(!derived_state::record_preview_failure(&conn, "image", "/image.jpg", "decode").unwrap());
    assert!(
        !derived_state::record_poster_failure(&conn, "poster", "/poster.mov", "poster").unwrap()
    );
    assert!(!derived_state::record_strip_failure(&conn, "strip", "/strip.mov", "strip").unwrap());

    let state: (String, String, i64) = conn
        .query_row(
            "SELECT
               (SELECT derive_outcome FROM contents WHERE hash = 'image'),
               (SELECT derive_outcome FROM contents WHERE hash = 'poster'),
               (SELECT strip_frames FROM contents WHERE hash = 'strip')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(state, ("failed".into(), "failed".into(), -1));

    let (_, issues) = queries::issues(&conn, 20, None).unwrap();
    for kind in [
        derived_state::PREVIEW_ERROR,
        derived_state::VIDEO_POSTER_ERROR,
        derived_state::VIDEO_STRIP_ERROR,
    ] {
        assert!(issues.iter().any(|row| row.kind == kind));
    }
}

#[test]
fn a_success_with_no_matching_issue_reports_no_issues_change() {
    let (_dir, conn) = seeded();
    // "/delete.jpg" only ever had a "delete-error" Issue (unrelated to
    // preview derivation), so resolving PREVIEW_ERROR here is a genuine
    // no-op: nothing for the Issues surface to reload (C-M3).
    assert!(!derived_state::record_preview_success(
        &conn, "delete", "/delete.jpg", 100, 100, 1.0, 7
    )
    .unwrap());
    let (_, issues) = queries::issues(&conn, 20, None).unwrap();
    assert!(issues
        .iter()
        .any(|row| row.path.as_deref() == Some("/delete.jpg") && row.kind == "delete-error"));
}

#[test]
fn transcript_reads_distinguish_pending_failed_empty_and_ready_output() {
    let (_dir, conn) = seeded();

    let failed = derived_state::transcript_result(&conn, "speech").unwrap();
    assert_eq!(failed.status, "failed");
    // The Issue's `message` is now the raw recorded diagnostic; OneCopy's own
    // sentence lives behind the Issue's `message_key` instead, so this field
    // carries whatever `record_transcript_failure` was given (R5.5 D-L12,
    // D-L13). The frontend transcript surface does not read this field for
    // display — it shows its own translated `transcript.couldNotFinish`.
    assert_eq!(failed.message.as_deref(), Some("decoder failed"));

    let pending = derived_state::transcript_result(&conn, "poster").unwrap();
    assert_eq!(pending.status, "pending");

    // A transcript kept through a rebuild meets its content again.
    conn.execute(
        "INSERT INTO transcripts (content_hash, model, model_version, language, text, segments, created_at_utc)
         VALUES ('poster', 'whisper-large-v3-turbo', 'v', 'en', 'kept',
           '[{\"startMs\":1000,\"endMs\":2000,\"text\":\"kept\"}]', '2026-10-02T00:00:00.000Z')",
        [],
    )
    .unwrap();
    let adopted = derived_state::transcript_result(&conn, "poster").unwrap();
    assert_eq!(adopted.status, "ready");
    assert_eq!(adopted.text.as_deref(), Some("[0:01] kept\n"));

    derived_state::record_transcript_success(&conn, "speech", "/speech.mov", &Transcript::default(), whisper()).unwrap();
    let empty = derived_state::transcript_result(&conn, "speech").unwrap();
    assert_eq!(empty.status, "ready");
    assert_eq!(empty.text.as_deref(), Some(""));

}

#[test]
fn derived_failure_issues_keep_hostile_diagnostics_out_of_onecopys_own_sentence() {
    // OneCopy's own sentence is a static catalogue entry, named by a key the
    // diagnostic can never influence; the diagnostic itself is kept, but only
    // as recorded detail shown after that sentence, never blended into it
    // (text that stays as recorded is shown as recorded).
    let (_dir, conn) = seeded();
    let hostile =
        "DecoderError EACCES /private/tmp/HOSTILE-SENTINEL [52, 49, 46, 46]";

    derived_state::record_preview_failure(&conn, "image", "/image.jpg", hostile).unwrap();
    derived_state::record_poster_failure(&conn, "poster", "/poster.mov", hostile).unwrap();
    derived_state::record_strip_failure(&conn, "strip", "/strip.mov", hostile).unwrap();
    derived_state::record_face_failure(&conn, "face", "/face.jpg", hostile).unwrap();
    derived_state::record_transcript_failure(&conn, "speech", "/speech.mov", hostile).unwrap();

    let (_, issues) = queries::issues(&conn, 20, None).unwrap();
    let expected_keys = [
        (derived_state::PREVIEW_ERROR, "notice.previewFailed"),
        (derived_state::VIDEO_POSTER_ERROR, "notice.videoPosterFailed"),
        (derived_state::VIDEO_STRIP_ERROR, "notice.videoStripFailed"),
        (derived_state::FACE_ERROR, "notice.faceScoreFailed"),
        (derived_state::TRANSCRIPT_ERROR, "notice.transcriptFailed"),
    ];
    let english = onecopy_lib::i18n::catalogue("en");
    for (kind, expected_key) in expected_keys {
        let row = issues.iter().find(|row| row.kind == kind).unwrap();
        let key = row.message_key.as_deref().unwrap();
        assert_eq!(key, expected_key);
        let sentence = english.text(key, "OneCopy");
        assert!(sentence.starts_with("OneCopy could not"));
        assert!(sentence.contains("original file was not changed"));
        assert!(!sentence.contains("HOSTILE-SENTINEL"));
        assert!(!sentence.contains("EACCES"));
        assert!(!sentence.contains("/private/tmp"));
        assert!(!sentence.contains("DecoderError"));
        // The diagnostic is kept, exactly, as recorded detail alongside the
        // translated sentence — never discarded, never merged into it.
        assert_eq!(row.message.as_deref(), Some(hostile));
    }
}

#[test]
fn a_poster_for_a_video_without_a_container_duration_settles_snapshots_and_admits_transcription() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('live', 1, 'video');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('/live.webm', '/', 'live.webm', 'video', 'live');",
    )
    .unwrap();
    assert!(derived_state::transcript_candidates(&conn, "video", None, 10)
        .unwrap()
        .is_empty());

    derived_state::record_poster_success(&conn, "live", "/live.webm", None).unwrap();

    let (duration_ms, strip_frames): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT duration_ms, strip_frames FROM contents WHERE hash = 'live'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    // No snapshot can be placed on an unknown timeline; nothing failed.
    assert_eq!((duration_ms, strip_frames), (None, Some(0)));
    assert!(derived_state::strip_candidates(&conn, None, 10).unwrap().is_empty());
    assert_eq!(
        derived_state::transcript_candidates(&conn, "video", None, 10).unwrap(),
        [("live".to_string(), "/live.webm".to_string())]
    );
    let (_, issues) = queries::issues(&conn, 20, None).unwrap();
    assert!(issues.is_empty());
}

#[test]
fn a_failure_after_a_success_keeps_the_success_time_and_a_reset_restores_it() {
    let (_dir, conn) = seeded();
    derived_state::record_preview_success(&conn, "image", "/image.jpg", 4000, 3000, 12.5, 42)
        .unwrap();
    let succeeded = |conn: &rusqlite::Connection| -> (Option<String>, Option<String>, Option<f64>) {
        conn.query_row(
            "SELECT derived_at_utc, derive_outcome, sharpness FROM contents WHERE hash = 'image'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    };
    let (success_time, outcome, sharpness) = succeeded(&conn);
    assert!(success_time.is_some());
    assert_eq!((outcome, sharpness), (None, Some(12.5)));

    derived_state::record_preview_failure(&conn, "image", "/image.jpg", "decode").unwrap();
    assert_eq!(
        succeeded(&conn),
        (success_time.clone(), Some("failed".into()), Some(12.5)),
        "the failure is its own fact; the success's time stays a time"
    );

    derived_state::reset_failed_outputs(&conn, FailedOutputScope::Library).unwrap();
    assert_eq!(succeeded(&conn), (success_time, None, Some(12.5)));
}
