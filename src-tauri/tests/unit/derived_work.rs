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
                "transcribe://done",
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
        assert_eq!(report_event(&cancelled).unwrap().0, "transcribe://cancelled");

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
        assert_eq!(event, "transcribe://error");
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
    assert_eq!(report_event(&requested).unwrap().0, "transcribe://error");

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

// R4.4 E1: the derived worker's own panic boundary. `worker_termination` is
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
    let outcome = finish_transcription_attempt(&attempt, "media".into(), Err(error.clone())).unwrap();
    assert!(matches!(outcome, TranscriptionAttemptOutcome::ModelUnavailable { .. }));
    let failed: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE path != ''", [], |r| r.get(0)).unwrap();
    assert_eq!(failed, 0, "a dependency failure must never blame the media file");
    for _ in 0..2 {
        record_model_failure(&conn, WorkClass::VideoTranscripts, &error).unwrap();
    }
    let count: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE kind = 'model-unavailable-video-transcripts'", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
    let automatic = transcription_report(TranscriptionRun::Automatic, &outcome);
    assert_eq!(automatic.pause_message.as_deref(), Some(error.as_str()));
    assert!(automatic.event.is_none(), "no per-file failure event for automatic work");
    let requested = transcription_report(TranscriptionRun::Requested { replacement: false }, &outcome);
    assert_eq!(requested.event.unwrap().0, "transcribe://error");
    let paused = crate::derived_runtime::changed_pause_classes(0, Some(WorkClass::VideoTranscripts.id()), true).unwrap();
    assert_eq!(paused & WorkClass::VideoTranscripts.bit(), WorkClass::VideoTranscripts.bit());
    assert_eq!(paused & WorkClass::Similarity.bit(), 0);
    assert_eq!(crate::derived_runtime::changed_pause_classes(paused, Some(WorkClass::VideoTranscripts.id()), false).unwrap(), 0);
}
