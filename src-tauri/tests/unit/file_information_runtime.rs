use super::*;

#[test]
fn a_completed_run_leaves_no_queued_work_behind() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let photo = dir.path().join("photo.jpg");
    std::fs::write(&photo, b"data").unwrap();
    let lists = crate::scanner::ScanLists {
        images: vec!["jpg".into()],
        videos: vec![],
        audio: vec![],
        companions: vec![],
    };
    crate::scanner::upsert_file(&conn, &photo, &std::fs::metadata(&photo).unwrap(), &lists, 0).unwrap();
    assert!(crate::scanner::pending_index_work_exists(&conn).unwrap());

    let summary = complete_pending(&conn, crate::scanner::pending_index_work_exists, |conn| {
        // The run settles every piece of debt it found.
        conn.execute("DELETE FROM paths", []).map_err(|error| error.to_string())?;
        Ok(crate::scanner::ScanSummary::default())
    })
    .unwrap();

    assert!(summary.is_some());
    assert!(!crate::scanner::pending_index_work_exists(&conn).unwrap());
    assert!(!snapshot().queued);
    assert!(complete_pending(&conn, crate::scanner::pending_index_work_exists, |_| unreachable!()).unwrap().is_none());
}

#[test]
fn urgent_preparation_ends_a_completion_turn_at_its_safe_point_and_requeues_it() {
    use std::time::{Duration, Instant};
    let _serial = crate::scan_runtime::serial_test();
    PAUSED.store(false, Ordering::SeqCst);
    PREEMPTED.store(false, Ordering::SeqCst);
    REQUESTED.store(false, Ordering::SeqCst);
    RUNNING.store(true, Ordering::SeqCst);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let completion = std::thread::spawn(move || {
        crate::scan_runtime::with_owner(
            crate::scan_runtime::Owner::FileInformation,
            || PAUSED.load(Ordering::SeqCst) || PREEMPTED.load(Ordering::SeqCst),
            || -> Result<(), String> {
                entered_tx.send(()).unwrap();
                // A long metadata tail whose safe points check the stop.
                let started = Instant::now();
                while !crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst) {
                    assert!(started.elapsed() < Duration::from_secs(20), "completion never yielded");
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(crate::scanner::CANCELLED.to_string())
            },
        )
    });
    entered_rx.recv().unwrap();

    let requested = Instant::now();
    let waited = crate::scan_runtime::with_derived_share(
        crate::scan_runtime::ShareRank::Urgent,
        || requested.elapsed(),
    )
    .unwrap();
    let ended = completion.join().unwrap();
    RUNNING.store(false, Ordering::SeqCst);

    assert!(waited < Duration::from_secs(2), "visible previews waited {waited:?}");
    assert_eq!(ended.unwrap_err(), crate::scanner::CANCELLED);
    // The rest of the turn stays queued for the worker's next turn.
    assert!(PREEMPTED.swap(false, Ordering::SeqCst));
    assert!(REQUESTED.swap(false, Ordering::SeqCst));
}

#[test]
fn a_terminal_failure_holds_queued_work_and_presents_as_failed_not_paused() {
    let _serial = crate::scan_runtime::serial_test();
    hold_failed();
    let state = snapshot();
    assert!(state.failed);
    // The queued work stays held so it does not retry in a loop.
    assert!(state.paused);
    FAILED.store(false, Ordering::SeqCst);
    PAUSED.store(false, Ordering::SeqCst);
}
