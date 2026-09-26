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
