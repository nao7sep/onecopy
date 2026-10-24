use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[test]
fn identical_requested_previews_share_one_result() {
    let key = format!("test-preview-{}", crate::nanoid::generate().unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let first_key = key.clone();
    let leader = std::thread::spawn(move || {
        coalesce_requested_preview(&first_key, || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok("canonical".to_string())
        })
    });
    started_rx.recv().unwrap();

    let observed = REQUESTED_PREVIEWS
        .lock()
        .unwrap()
        .get(&key)
        .unwrap()
        .clone();
    let executions = Arc::new(AtomicUsize::new(0));
    let follower_executions = executions.clone();
    let follower_key = key.clone();
    let follower = std::thread::spawn(move || {
        coalesce_requested_preview(&follower_key, || {
            follower_executions.fetch_add(1, Ordering::SeqCst);
            Ok("wrong".to_string())
        })
    });

    let deadline = Instant::now() + Duration::from_secs(1);
    while Arc::strong_count(&observed) < 4 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let joined = Arc::strong_count(&observed) >= 4;
    release_tx.send(()).unwrap();
    assert!(joined, "follower did not join the active request");

    assert_eq!(leader.join().unwrap().unwrap(), ("canonical".to_string(), false));
    assert_eq!(follower.join().unwrap().unwrap(), ("canonical".to_string(), true));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[test]
fn a_panicking_leader_settles_its_followers_and_retires_the_flight() {
    let key = format!("test-preview-{}", crate::nanoid::generate().unwrap());
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let leader_key = key.clone();
    let leader = std::thread::spawn(move || {
        coalesce_requested_preview(&leader_key, || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            panic!("decoder panicked on a malformed image");
        })
    });
    started_rx.recv().unwrap();
    let observed = REQUESTED_PREVIEWS.lock().unwrap().get(&key).unwrap().clone();
    let follower_key = key.clone();
    let follower = std::thread::spawn(move || {
        coalesce_requested_preview(&follower_key, || Ok("wrong".to_string()))
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while Arc::strong_count(&observed) < 4 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    release_tx.send(()).unwrap();

    assert!(leader.join().is_err(), "the panic still reaches the command boundary");
    assert!(follower.join().unwrap().is_err(), "the follower was settled with a failure");
    assert!(REQUESTED_PREVIEWS.lock().unwrap().get(&key).is_none());
    // A later request derives again instead of waiting on the dead flight.
    assert_eq!(
        coalesce_requested_preview(&key, || Ok("canonical".to_string())).unwrap(),
        ("canonical".to_string(), false)
    );
}

fn report_event(report: &TranscriptionReport) -> Option<(&'static str, serde_json::Value)> {
    report.event.clone()
}

#[test]
fn requested_and_automatic_runs_publish_completion_the_same_way() {
    let outcome = TranscriptionAttemptOutcome::Completed {
        hash: "exact".to_string(),
        text: "hello".to_string(),
        issues_changed: false,
    };
    for (run, replacement) in [
        (TranscriptionRun::Automatic, false),
        (TranscriptionRun::Requested { replacement: false }, false),
        (TranscriptionRun::Requested { replacement: true }, true),
    ] {
        let report = transcription_report(run, &outcome);
        assert_eq!(report.item_hash.as_deref(), Some("exact"));
        assert_eq!(report.pause_message, None);
        assert_eq!(
            report_event(&report),
            Some((
                TRANSCRIBE_DONE,
                serde_json::json!({ "hash": "exact", "text": "hello", "replacement": replacement })
            ))
        );
    }
}

#[test]
fn cancelled_and_failed_runs_reproject_the_item_for_every_caller() {
    for run in [
        TranscriptionRun::Automatic,
        TranscriptionRun::Requested { replacement: false },
    ] {
        let cancelled = transcription_report(
            run,
            &TranscriptionAttemptOutcome::Cancelled { hash: "exact".to_string() },
        );
        assert_eq!(cancelled.item_hash.as_deref(), Some("exact"));
        assert_eq!(report_event(&cancelled).unwrap().0, TRANSCRIBE_CANCELLED);

        let failed = transcription_report(
            run,
            &TranscriptionAttemptOutcome::Failed {
                hash: "exact".to_string(),
                message: "broken".to_string(),
                issues_changed: true,
            },
        );
        assert_eq!(failed.item_hash.as_deref(), Some("exact"));
        let (event, payload) = report_event(&failed).unwrap();
        assert_eq!(event, TRANSCRIBE_ERROR);
        assert_eq!(payload["hash"], "exact");
        assert_eq!(payload["message"], "broken");
    }
}

#[test]
fn only_a_requested_run_reports_a_missing_tool_or_a_pause_to_its_requester() {
    let unavailable = TranscriptionAttemptOutcome::Unavailable {
        hash: "exact".to_string(),
        message: "install it".to_string(),
    };
    let paused = TranscriptionAttemptOutcome::ResourceSafety {
        hash: "exact".to_string(),
        message: "memory".to_string(),
    };
    let automatic = transcription_report(TranscriptionRun::Automatic, &unavailable);
    assert_eq!(automatic.event, None);
    assert_eq!(automatic.item_hash.as_deref(), Some("exact"));
    let requested =
        transcription_report(TranscriptionRun::Requested { replacement: false }, &unavailable);
    assert_eq!(report_event(&requested).unwrap().0, TRANSCRIBE_ERROR);

    for run in [
        TranscriptionRun::Automatic,
        TranscriptionRun::Requested { replacement: true },
    ] {
        let report = transcription_report(run, &paused);
        assert_eq!(report.pause_message.as_deref(), Some("memory"));
        assert_eq!(report.item_hash, None);
        assert_eq!(report.event.is_some(), run != TranscriptionRun::Automatic);
    }
}

// The derived worker's own panic boundary. `worker_termination` is
// the pure outcome-to-action mapping `derived_worker` executes against a live
// `AppHandle`; testing it directly proves a panicking or failing worker is
// caught and correctly routed to `Failed` (which `derived_worker` reports as
// `WORKER_FAILED`, publishes as `derived://worker-failed`, and leaves as an
// Issue) rather than propagating or being silently swallowed.
#[test]
fn a_clean_return_needs_no_failure_reporting() {
    let outcome = std::panic::catch_unwind(|| -> Result<(), String> { Ok(()) });
    assert_eq!(worker_termination(outcome, false), WorkerTermination::Clean);
}

#[test]
fn a_returned_error_during_ordinary_operation_is_a_reportable_failure() {
    let outcome: std::thread::Result<Result<(), String>> = Ok(Err("index unavailable".into()));
    assert_eq!(
        worker_termination(outcome, false),
        WorkerTermination::Failed("index unavailable".into())
    );
}

#[test]
fn a_returned_error_while_shutting_down_is_logged_only() {
    let outcome: std::thread::Result<Result<(), String>> = Ok(Err("index unavailable".into()));
    assert_eq!(
        worker_termination(outcome, true),
        WorkerTermination::DuringShutdown("index unavailable".into())
    );
}

#[test]
fn a_real_panic_is_caught_and_routed_as_a_failure_with_its_message() {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
        panic!("derived worker exploded");
    }));
    assert_eq!(
        worker_termination(outcome, false),
        WorkerTermination::Failed("derived worker exploded".into())
    );
}

#[test]
fn a_panic_while_shutting_down_is_logged_only_not_reported() {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
        panic!("derived worker exploded during shutdown");
    }));
    assert_eq!(
        worker_termination(outcome, true),
        WorkerTermination::DuringShutdown("derived worker exploded during shutdown".into())
    );
}

#[test]
fn a_transcription_class_follows_its_content_kind() {
    assert_eq!(
        WorkClass::transcription_for_kind("video"),
        Some(WorkClass::VideoTranscripts)
    );
    assert_eq!(
        WorkClass::transcription_for_kind("audio"),
        Some(WorkClass::AudioTranscripts)
    );
    assert_eq!(WorkClass::transcription_for_kind("image"), None);
}

#[test]
fn a_model_load_failure_keeps_transcription_retryable_and_coalesces_one_class_issue() {
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(temp.path().join("cache"));
    let attempt = TranscriptionAttempt {
        conn: &conn, cache: &cache, data_root: temp.path(), temp_dir: temp.path().join("temp"),
        source_hash: "media", source_path: "/media/video.mp4", replace_existing: false,
        acceleration: crate::ai_acceleration::Mode::None, cancel_when: None,
    };
    let error = crate::ai_dependencies::model_load_error("invalid model header");
    let outcome = finish_transcription_attempt(&attempt, "media".into(), crate::ai_dependencies::ModelOutcome { model: crate::ai_dependencies::transcription_model(), result: Err(error.clone()) }).unwrap();
    assert!(matches!(outcome, TranscriptionAttemptOutcome::ModelUnavailable { .. }));
    let failed: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE path != ''", [], |r| r.get(0)).unwrap();
    assert_eq!(failed, 0, "a dependency failure must never blame the media file");
    let class_issues = || -> i64 {
        conn.query_row("SELECT count(*) FROM active_issues WHERE path = ''", [], |r| r.get(0)).unwrap()
    };
    for _ in 0..2 {
        record_model_failure(&conn, WorkClass::VideoTranscripts, &error).unwrap();
    }
    assert_eq!(class_issues(), 1, "repeated failures of one class coalesce");
    record_model_failure(&conn, WorkClass::AudioTranscripts, &error).unwrap();
    assert_eq!(class_issues(), 2, "each class keeps its own Issue");
    let automatic = transcription_report(TranscriptionRun::Automatic, &outcome);
    assert_eq!(automatic.pause_message.as_deref(), Some(error.as_str()));
    assert!(automatic.event.is_none(), "no per-file failure event for automatic work");
    let requested = transcription_report(TranscriptionRun::Requested { replacement: false }, &outcome);
    assert_eq!(requested.event.unwrap().0, TRANSCRIBE_ERROR);
    let paused = crate::derived_runtime::changed_pause_classes(0, Some(WorkClass::VideoTranscripts.id()), true).unwrap();
    assert_eq!(paused & WorkClass::VideoTranscripts.bit(), WorkClass::VideoTranscripts.bit());
    assert_eq!(paused & WorkClass::Similarity.bit(), 0);
    assert_eq!(crate::derived_runtime::changed_pause_classes(paused, Some(WorkClass::VideoTranscripts.id()), false).unwrap(), 0);
}

#[test]
fn a_transcription_panic_becomes_a_durable_item_failure() {
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES ('media', 1, 'audio')", []).unwrap();
    let cache = CachePaths::new(temp.path().join("cache"));
    let attempt = TranscriptionAttempt {
        conn: &conn, cache: &cache, data_root: temp.path(), temp_dir: temp.path().join("temp"),
        source_hash: "media", source_path: "/media/audio.wav", replace_existing: false,
        acceleration: crate::ai_acceleration::Mode::None, cancel_when: None,
    };
    let result = crate::failure_runtime::contain_item(|| panic!("synthetic transcription panic"));
    let outcome = finish_transcription_attempt(&attempt, "media".into(), crate::ai_dependencies::ModelOutcome { model: crate::ai_dependencies::transcription_model(), result }).unwrap();
    assert!(matches!(outcome, TranscriptionAttemptOutcome::Failed { .. }));
    assert_eq!(crate::derived_state::transcript_result(&conn, "media").unwrap().status, crate::derived_state::FAILED);
}

#[test]
fn settled_global_preview_search_waits_for_new_debt_but_denied_turns_do_not_settle() {
    let mut cursors = CandidateCursors::default();
    assert!(!cursors.run_preview_search(|| Ok((false, false))).unwrap());
    assert!(cursors.run_preview_search(|| Ok((true, true))).unwrap());
    assert!(!cursors.run_preview_search(|| Ok((false, true))).unwrap());
    for _ in 0..10 {
        assert!(!cursors.run_preview_search(|| panic!("idle wake searched a settled library")).unwrap());
    }
    cursors.invalidate();
    assert!(cursors.run_preview_search(|| Ok((true, true))).unwrap());
    assert!(cursors.run_preview_search(|| Err("interrupted".to_string())).is_err());
    assert!(cursors.run_preview_search(|| Ok((true, true))).unwrap());
}

#[test]
fn transcription_receipts_keep_loaded_identity_and_failures_do_not_retry_for_a_different_version() {
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(temp.path().join("cache"));
    let model = crate::ai_dependencies::transcription_model_with_version(Some("a".repeat(64)));
    for hash in ["success", "failure"] {
        conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 1, 'audio')", [hash]).unwrap();
        let attempt = TranscriptionAttempt {
            conn: &conn, cache: &cache, data_root: temp.path(), temp_dir: temp.path().join("temp"),
            source_hash: hash, source_path: "/media/audio.wav", replace_existing: false,
            acceleration: crate::ai_acceleration::Mode::None, cancel_when: None,
        };
        let result = if hash == "success" { Ok(crate::transcription::Transcript::default()) }
            else { Err("inference failed".into()) };
        finish_transcription_attempt(&attempt, hash.into(), crate::ai_dependencies::ModelOutcome {
            model: model.clone(), result,
        }).unwrap();
        let sql = if hash == "success" {
            "SELECT model_version FROM transcripts WHERE content_hash = ?1"
        } else {
            "SELECT model_version FROM records.analysis_events WHERE content_hash = ?1 AND event = 'failed'"
        };
        let version: String = conn.query_row(sql, [hash], |r| r.get(0)).unwrap();
        assert_eq!(version, model.version);
    }
    assert_eq!(crate::derived_state::transcript_result(&conn, "failure").unwrap().status, crate::derived_state::FAILED);
}

#[test]
fn storage_pauses_are_shared_and_only_requested_runs_report_to_the_item() {
    let message = crate::resource_limits::cache_write_error(
        "transcription staging", std::io::Error::from(std::io::ErrorKind::QuotaExceeded));
    let outcome = TranscriptionAttemptOutcome::StorageUnavailable {
        hash: "media".into(), message: message.clone(),
    };
    let root = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&root.path().join("index.sqlite3")).unwrap();
    for _ in 0..2 {
        record_storage_failure(&conn, WorkClass::AudioTranscripts, &message).unwrap();
    }
    let issues: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE path = '' AND kind = 'cache-storage-unavailable-audio-transcripts'", [], |row| row.get(0)).unwrap();
    assert_eq!(issues, 1, "one shared Issue explains the paused class");
    for run in [TranscriptionRun::Automatic, TranscriptionRun::Requested { replacement: false },
                TranscriptionRun::Requested { replacement: true }] {
        let report = transcription_report(run, &outcome);
        assert_eq!(report.pause_message.as_deref(), Some(message.as_str()));
        assert!(report.item_hash.is_none());
        assert_eq!(report.event.is_some(), run != TranscriptionRun::Automatic);
    }
}


#[test]
fn idle_budget_uses_monotonic_elapsed_time_and_resets_after_input() {
    let input = Instant::now();
    assert!(idle_since(None, input));
    assert!(!idle_since(Some(input), input));
    assert!(!idle_since(Some(input), input + IDLE_AFTER - Duration::from_nanos(1)));
    assert!(idle_since(Some(input), input + IDLE_AFTER));
    let later_input = input + Duration::from_secs(24 * 60 * 60);
    assert!(idle_since(Some(input), later_input));
    assert!(!idle_since(Some(later_input), later_input));
    assert!(idle_since(Some(later_input), later_input + IDLE_AFTER));
}

#[test]
fn committed_previews_reopen_only_their_dependent_cursors() {
    const CHILD_ROOT: &str = "ONECOPY_PREVIEW_DEBT_TEST_ROOT";
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        // Publication revisions are process-wide; isolate this proof from
        // unrelated preview fixtures publishing on other test threads.
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "derived_work::tests::committed_previews_reopen_only_their_dependent_cursors", "--nocapture"])
            .env(CHILD_ROOT, root.path()).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(root.path().join("verified").is_file());
        return;
    };
    let root = PathBuf::from(root);
    let conn = crate::index_store::open(&root.join("index.sqlite3")).unwrap();
    let revisions = || (IMAGE_PREVIEW_REVISION.load(Ordering::SeqCst), VIDEO_POSTER_REVISION.load(Ordering::SeqCst));
    let wake_generation = || *WAKE.get_or_init(|| (Mutex::new(0), Condvar::new())).0.lock().unwrap();
    let debt = DEBT_REVISION.load(Ordering::SeqCst);
    for video in [false, true] {
        for exhausted in [false, true] {
            let hash = format!("a-{video}-{exhausted}");
            let kind = if video { "video" } else { "image" };
            conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES (?1, 1, ?2)", rusqlite::params![hash, kind]).unwrap();
            conn.execute("INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash)
                          VALUES (?1, '/', ?1, ?2, ?1)", rusqlite::params![hash, kind]).unwrap();
            let mut cursors = CandidateCursors::default();
            cursors.refresh_prerequisites();
            cursors.previews_exhausted = true;
            for cursor in [&mut cursors.faces, &mut cursors.snapshots, &mut cursors.video_transcripts, &mut cursors.audio_transcripts] {
                cursor.after_hash = Some("z".into());
                cursor.exhausted = exhausted;
            }
            cursors.section.initialized = true;
            let before = wake_generation();
            if video {
                crate::derived_state::record_poster_success(&conn, &hash, &hash, Some(1000)).unwrap();
            } else {
                crate::derived_state::record_preview_success(&conn, &hash, &hash, 10, 10, 1.0, 1).unwrap();
            }
            assert!(wake_generation() > before, "publication wakes the waiting worker");
            cursors.refresh_prerequisites();
            assert!(cursors.previews_exhausted);
            assert_eq!(cursors.audio_transcripts.after_hash.as_deref(), Some("z"));
            assert_eq!(cursors.audio_transcripts.exhausted, exhausted);
            assert!(!cursors.section.initialized, "section work may also lie behind its cursor");
            if video {
                assert!(cursors.snapshots.after_hash.is_none());
                assert!(!cursors.snapshots.exhausted);
                assert!(cursors.video_transcripts.after_hash.is_none());
                assert!(!cursors.video_transcripts.exhausted);
                assert_eq!(cursors.faces.after_hash.as_deref(), Some("z"));
                assert_eq!(cursors.faces.exhausted, exhausted);
                assert!(crate::derived_state::strip_candidates(&conn, cursors.snapshots.after_hash.as_deref(), 10)
                    .unwrap().iter().any(|(candidate, _, _)| candidate == &hash));
            } else {
                assert!(cursors.faces.after_hash.is_none());
                assert!(!cursors.faces.exhausted);
                assert_eq!(cursors.snapshots.after_hash.as_deref(), Some("z"));
                assert_eq!(cursors.snapshots.exhausted, exhausted);
                assert_eq!(cursors.video_transcripts.after_hash.as_deref(), Some("z"));
                assert_eq!(cursors.video_transcripts.exhausted, exhausted);
                assert!(crate::derived_state::face_candidates(&conn, cursors.faces.after_hash.as_deref(), 10)
                    .unwrap().iter().any(|(candidate, _)| candidate == &hash));
            }
            assert_eq!(DEBT_REVISION.load(Ordering::SeqCst), debt, "unrelated debt stays settled");
        }
    }
    let before = (revisions(), wake_generation());
    crate::derived_state::record_preview_success(&conn, "absent", "absent", 10, 10, 1.0, 1).unwrap();
    crate::derived_state::record_poster_success(&conn, "absent", "absent", Some(1000)).unwrap();
    assert_eq!((revisions(), wake_generation()), before, "no receipt creates no debt");
    conn.execute_batch("CREATE TEMP TRIGGER reject_preview BEFORE UPDATE ON contents
                        BEGIN SELECT RAISE(ABORT, 'fixture publication refused'); END;").unwrap();
    assert!(crate::derived_state::record_preview_success(&conn, "a-false-false", "a-false-false", 10, 10, 1.0, 1).is_err());
    assert!(crate::derived_state::record_poster_success(&conn, "a-true-false", "a-true-false", Some(1000)).is_err());
    assert_eq!((revisions(), wake_generation()), before, "failed publication cannot advertise debt");
    std::fs::write(root.join("verified"), b"verified").unwrap();
}
