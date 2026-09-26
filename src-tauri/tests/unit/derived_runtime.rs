use super::*;
use crate::scan_runtime::ShareRank;
use std::sync::mpsc;
use std::thread;

fn job(class: WorkClass, manual: bool) -> Job {
    Job {
        id: 1,
        class,
        manual,
        urgent: false,
        hash: Some("hash".to_string()),
        stop: false,
        done: None,
        total: None,
    }
}

fn set_hash(id: u64, hash: &str) {
    let mut runtime = RUNTIME.0.lock().unwrap();
    runtime
        .jobs_mut()
        .find(|job| job.id == id)
        .unwrap()
        .hash = Some(hash.to_string());
}

/// Runs an automatic heavy job for `hash` on its own thread until its safe
/// point reports a stop, like a long transcription polling `cancelled`.
fn long_automatic_job(
    class: WorkClass,
    hash: &'static str,
) -> (thread::JoinHandle<Duration>, mpsc::Receiver<()>) {
    let (running_tx, running_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let job = ActiveGuard::begin(None, class).unwrap().unwrap();
        set_hash(job.id, hash);
        running_tx.send(()).unwrap();
        let started = Instant::now();
        while !cancelled() {
            assert!(started.elapsed() < Duration::from_secs(20), "the job was never stopped");
            thread::sleep(Duration::from_millis(5));
        }
        started.elapsed()
    });
    (worker, running_rx)
}

#[test]
fn shutdown_releases_every_manual_ticket_from_its_queue_wait() {
    let runtime = RuntimeState {
        claim: Some(Claim { keys: Vec::new() }),
        heavy: Some(job(WorkClass::VideoTranscripts, true)),
        serving_manual_ticket: 2,
        ..RuntimeState::default()
    };
    assert!(manual_waits(&runtime, 7, Some("other"), false));
    assert!(!manual_waits(&runtime, 7, Some("other"), true));
}

#[test]
fn preemption_stops_automatic_work_by_rank_and_never_requested_work() {
    let runtime = RuntimeState::default();
    let automatic = job(WorkClass::VideoTranscripts, false);
    let urgent = Job {
        urgent: true,
        ..job(WorkClass::Previews, false)
    };
    let requested = job(WorkClass::VideoTranscripts, true);
    let none = Preemption::default();
    let foreground = Preemption {
        foreground: true,
        background: false,
    };
    let background = Preemption {
        foreground: false,
        background: true,
    };
    assert!(!job_cancelled(&runtime, &automatic, false, none));
    assert!(job_cancelled(&runtime, &automatic, false, foreground));
    assert!(job_cancelled(&runtime, &automatic, false, background));
    assert!(job_cancelled(&runtime, &urgent, false, foreground));
    assert!(!job_cancelled(&runtime, &urgent, false, background));
    assert!(!job_cancelled(&runtime, &requested, false, foreground));
    assert!(!job_cancelled(&runtime, &requested, false, background));
    assert!(job_cancelled(&runtime, &requested, true, none));
}

/// Runs a long automatic job under a derived share on its own thread until
/// its safe point reports a stop, like a transcription polling `cancelled`.
/// The receiver reports when the job is running.
fn long_shared_job(
    rank: ShareRank,
    class: WorkClass,
) -> (thread::JoinHandle<Option<Duration>>, mpsc::Receiver<()>) {
    let (running_tx, running_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        crate::scan_runtime::with_derived_share(rank, || {
            let _job = ActiveGuard::begin(None, class).unwrap().unwrap();
            running_tx.send(()).unwrap();
            let started = Instant::now();
            while !cancelled() {
                assert!(started.elapsed() < Duration::from_secs(20), "the job never yielded");
                thread::sleep(Duration::from_millis(5));
            }
            started.elapsed()
        })
    });
    (worker, running_rx)
}

#[test]
fn a_file_operation_starts_within_a_long_automatic_jobs_safe_point() {
    let _serial = crate::scan_runtime::serial_test();
    let (worker, running) = long_shared_job(ShareRank::Ordinary, WorkClass::VideoTranscripts);
    running.recv().unwrap();

    let requested = Instant::now();
    let admitted = crate::scan_runtime::admit_foreground(
        None,
        Some(requested + Duration::from_secs(5)),
        &|| false,
        &mut || {},
    );
    assert!(admitted.is_ok(), "{:?}", admitted.err());
    assert!(requested.elapsed() < Duration::from_secs(2));
    drop(admitted);
    assert!(worker.join().unwrap().is_some());
}

#[test]
fn every_background_index_owner_starts_within_a_long_automatic_jobs_safe_point() {
    let _serial = crate::scan_runtime::serial_test();
    for owner in ["watcher", "source check", "file information"] {
        let (worker, running) = long_shared_job(ShareRank::Ordinary, WorkClass::VideoTranscripts);
        running.recv().unwrap();
        let requested = Instant::now();
        let work = || Ok::<_, String>(requested.elapsed());
        let waited = match owner {
            "watcher" => crate::scan_runtime::with_watcher_claim(|| false, work),
            "source check" => crate::scan_runtime::with_source_check_claim(|| false, |_| {}, work),
            _ => crate::scan_runtime::with_owner(
                crate::scan_runtime::Owner::FileInformation,
                || false,
                work,
            ),
        }
        .unwrap();
        // New photos or a started check do not wait out the transcription.
        assert!(waited < Duration::from_secs(2), "{owner} waited {waited:?}");
        assert!(worker.join().unwrap().is_some(), "{owner}");
        // The job was already stopped when the owner took the index.
        assert!(!crate::scan_runtime::background_pending());
    }
}

#[test]
fn an_ordinary_share_declines_while_an_index_owner_holds_or_waits() {
    let _serial = crate::scan_runtime::serial_test();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let watcher = thread::spawn(move || {
        crate::scan_runtime::with_watcher_claim(
            || false,
            || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            },
        )
    });
    entered_rx.recv().unwrap();
    let asked = Instant::now();
    assert!(crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || ()).is_none());
    assert!(asked.elapsed() < Duration::from_millis(500), "an ordinary share never waits");
    release_tx.send(()).unwrap();
    watcher.join().unwrap().unwrap();
    assert!(crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || ()).is_some());
}

#[test]
fn preview_work_runs_beside_a_heavy_job_without_stopping_it() {
    let _serial = crate::scan_runtime::serial_test();
    let (heavy, running) = long_shared_job(ShareRank::Ordinary, WorkClass::VideoTranscripts);
    running.recv().unwrap();
    for rank in [ShareRank::Ordinary, ShareRank::Urgent] {
        let admitted = Instant::now();
        let ran = crate::scan_runtime::with_derived_share(rank, || {
            let preview = ActiveGuard::begin(None, WorkClass::Previews).unwrap();
            assert!(preview.is_some(), "{rank:?} preview work was refused beside a transcription");
            assert!(!cancelled());
        });
        assert!(ran.is_some(), "{rank:?}");
        assert!(admitted.elapsed() < Duration::from_millis(500));
    }
    assert!(
        !heavy.is_finished(),
        "preview work stopped the running transcription"
    );
    // Only a foreground action ends the transcription here.
    drop(crate::scan_runtime::admit_foreground(None, None, &|| false, &mut || {}).unwrap());
    assert!(heavy.join().unwrap().is_some());
}

#[test]
fn urgent_preparation_is_not_stopped_by_a_waiting_index_owner_and_the_owner_follows_it() {
    let _serial = crate::scan_runtime::serial_test();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let urgent = thread::spawn(move || {
        crate::scan_runtime::with_derived_share(ShareRank::Urgent, || {
            let _job = ActiveGuard::begin(None, WorkClass::Previews).unwrap().unwrap();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            // A watcher batch has been waiting all along.
            assert!(crate::scan_runtime::background_pending());
            cancelled()
        })
    });
    entered_rx.recv().unwrap();
    let watcher = thread::spawn(|| {
        crate::scan_runtime::with_watcher_claim(|| false, || Ok(Instant::now()))
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !crate::scan_runtime::background_pending() {
        assert!(Instant::now() < deadline, "the watcher never waited");
        thread::sleep(Duration::from_millis(2));
    }
    thread::sleep(Duration::from_millis(100));
    assert!(!watcher.is_finished(), "the watcher wrote beside urgent preparation");
    let released = Instant::now();
    release_tx.send(()).unwrap();
    assert_eq!(urgent.join().unwrap(), Some(false), "urgent preparation was stopped");
    assert!(watcher.join().unwrap().unwrap() >= released);
}

#[test]
fn urgent_preparation_interleaves_with_a_source_walk_that_keeps_its_progress() {
    let _serial = crate::scan_runtime::serial_test();
    let parked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = parked.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let walker = thread::spawn(move || {
        crate::scan_runtime::with_source_check_claim(
            || false,
            move |waiting| observed.store(waiting, std::sync::atomic::Ordering::SeqCst),
            || {
                entered_tx.send(()).unwrap();
                let mut steps = 0u32;
                while steps < 100 {
                    crate::scan_runtime::yield_at_safe_point()?;
                    steps += 1;
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(steps)
            },
        )
    });
    entered_rx.recv().unwrap();
    let requested = Instant::now();
    let during = crate::scan_runtime::with_derived_share(ShareRank::Urgent, || {
        let job = ActiveGuard::begin(None, WorkClass::Previews).unwrap();
        (
            requested.elapsed(),
            job.is_some(),
            parked.load(std::sync::atomic::Ordering::SeqCst),
        )
    })
    .unwrap();
    assert!(during.0 < Duration::from_secs(1), "waited {:?}", during.0);
    assert!(during.1, "the visible preview was refused");
    assert!(during.2, "the walk wrote beside the visible preview");
    // The walk continues from where it parked instead of starting over.
    assert_eq!(walker.join().unwrap().unwrap(), 100);
}

#[test]
fn an_external_open_never_stops_requested_work_and_stops_automatic_work_only_on_its_file() {
    let _serial = crate::scan_runtime::serial_test();
    let requested = requested_heavy(None, WorkClass::VideoTranscripts, "movie").unwrap();
    let opened = exclusive_claim(None, &["movie".to_string()], ClaimPolicy::ExternalOpen, &|| false);
    assert!(opened.is_ok());
    assert!(!cancelled(), "opening the file externally stopped the requested transcription");
    drop(opened);
    drop(requested);

    let (worker, running) = long_automatic_job(WorkClass::Faces, "photo");
    running.recv().unwrap();
    let elsewhere = Instant::now();
    drop(exclusive_claim(None, &["other".to_string()], ClaimPolicy::ExternalOpen, &|| false).unwrap());
    assert!(elsewhere.elapsed() < Duration::from_secs(1));
    let same = exclusive_claim(None, &["photo".to_string()], ClaimPolicy::ExternalOpen, &|| false);
    assert!(same.is_ok());
    drop(same);
    worker.join().unwrap();
}

#[test]
fn a_file_operation_stops_requested_work_on_its_own_file() {
    let _serial = crate::scan_runtime::serial_test();
    let (running_tx, running_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _job = requested_heavy(None, WorkClass::AudioTranscripts, "song").unwrap();
        running_tx.send(()).unwrap();
        let started = Instant::now();
        while !cancelled() {
            assert!(started.elapsed() < Duration::from_secs(20), "the job was never stopped");
            thread::sleep(Duration::from_millis(5));
        }
    });
    running_rx.recv().unwrap();
    let claimed = exclusive_claim(None, &["song".to_string()], ClaimPolicy::Mutation, &|| false);
    assert!(claimed.is_ok());
    drop(claimed);
    worker.join().unwrap();
}

#[test]
fn a_requested_preview_runs_beside_a_transcription() {
    let _serial = crate::scan_runtime::serial_test();
    let transcription = requested_heavy(None, WorkClass::VideoTranscripts, "movie").unwrap();
    let requested = Instant::now();
    let preview = requested_preview(None, "photo");
    assert!(preview.is_ok(), "{:?}", preview.err());
    assert!(requested.elapsed() < Duration::from_secs(1));
    assert!(!cancelled());
    drop(preview);
    // Automatic preview preparation is not stopped by it either.
    let automatic = ActiveGuard::begin(None, WorkClass::Previews).unwrap();
    assert!(automatic.is_some());
    drop(automatic);
    drop(transcription);
}

#[test]
fn requested_previews_share_the_lane_but_never_derive_the_same_item_at_once() {
    let mut running = job(WorkClass::Previews, true);
    running.hash = Some("photo".to_string());
    let runtime = RuntimeState {
        previews: vec![running],
        preview_queue: VecDeque::from([0]),
        ..RuntimeState::default()
    };
    assert!(preview_admits(&runtime, 0, "other", 4));
    assert!(!preview_admits(&runtime, 0, "photo", 4));
}

#[test]
fn a_second_request_for_an_item_waits_for_the_first() {
    let _serial = crate::scan_runtime::serial_test();
    let first = requested_preview(None, "photo").unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let second = thread::spawn(move || {
        let admitted = requested_preview(None, "photo").map(|_| ());
        done_tx.send(()).unwrap();
        admitted
    });
    assert!(done_rx.recv_timeout(Duration::from_millis(150)).is_err());
    drop(first);
    second.join().unwrap().unwrap();
}

#[test]
fn a_file_operation_stops_requested_work_whose_identity_was_promoted() {
    let _serial = crate::scan_runtime::serial_test();
    let (running_tx, running_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        // Main asked with the provisional key; identifying the file promotes it.
        let _job = requested_heavy(None, WorkClass::VideoTranscripts, "p7").unwrap();
        assert!(set_active_item(WorkClass::VideoTranscripts, "c0ffee07"));
        running_tx.send(()).unwrap();
        let started = Instant::now();
        while !cancelled() {
            assert!(started.elapsed() < Duration::from_secs(20), "the job was never stopped");
            thread::sleep(Duration::from_millis(5));
        }
    });
    running_rx.recv().unwrap();
    // Main now knows the item by its exact identity.
    let claimed = exclusive_claim(None, &["c0ffee07".to_string()], ClaimPolicy::Mutation, &|| false);
    assert!(claimed.is_ok());
    drop(claimed);
    worker.join().unwrap();
}
