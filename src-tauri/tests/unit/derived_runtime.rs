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
    let preview = job(WorkClass::Previews, false);
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
    // Heavy work lets a background index owner in while it computes instead.
    assert!(!job_cancelled(&runtime, &automatic, false, background));
    assert!(job_cancelled(&runtime, &preview, false, foreground));
    assert!(job_cancelled(&runtime, &preview, false, background));
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
fn every_background_index_owner_starts_within_a_long_preview_jobs_safe_point() {
    let _serial = crate::scan_runtime::serial_test();
    for owner in ["watcher", "source check", "file information"] {
        let (worker, running) = long_shared_job(ShareRank::Ordinary, WorkClass::Previews);
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
        // New photos or a started check do not wait out the preview pass.
        assert!(waited < Duration::from_secs(2), "{owner} waited {waited:?}");
        assert!(worker.join().unwrap().is_some(), "{owner}");
        // The job was already stopped when the owner took the index.
        assert!(!crate::scan_runtime::background_pending());
    }
}

/// Runs one background index owner of `kind` around `work`.
fn background_owner<T>(kind: &str, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match kind {
        "watcher" => crate::scan_runtime::with_watcher_claim(|| false, work),
        "source check" => crate::scan_runtime::with_source_check_claim(|| false, |_| {}, work),
        _ => crate::scan_runtime::with_owner(
            crate::scan_runtime::Owner::FileInformation,
            || false,
            work,
        ),
    }
}

#[test]
fn a_background_index_owner_runs_while_heavy_work_computes_and_the_work_publishes_after_it() {
    let _serial = crate::scan_runtime::serial_test();
    for owner in ["watcher", "source check", "file information"] {
        let order = std::sync::Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let (computing_tx, computing_rx) = mpsc::channel();
        let (entered_tx, entered_rx) = mpsc::channel();
        let heavy_order = order.clone();
        let heavy = thread::spawn(move || {
            crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || {
                let _job = ActiveGuard::begin(None, WorkClass::VideoTranscripts).unwrap().unwrap();
                let computed = crate::scan_runtime::outside_share(|| {
                    computing_tx.send(()).unwrap();
                    // The owner takes the index while the model still runs.
                    entered_rx
                        .recv_timeout(Duration::from_secs(5))
                        .expect("the owner waited out the transcription");
                    assert!(!cancelled(), "the owner stopped the transcription");
                    heavy_order.lock().unwrap().push("computed");
                    "transcript"
                });
                heavy_order.lock().unwrap().push("published");
                computed
            })
        });
        computing_rx.recv().unwrap();
        let owner_order = order.clone();
        background_owner(owner, move || {
            entered_tx.send(()).unwrap();
            // Still writing when the transcription finishes computing.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !owner_order.lock().unwrap().contains(&"computed") {
                assert!(Instant::now() < deadline, "the transcription never finished");
                thread::sleep(Duration::from_millis(2));
            }
            thread::sleep(Duration::from_millis(50));
            owner_order.lock().unwrap().push(owner);
            Ok(())
        })
        .unwrap();
        // The pass was never stopped, and published only after the owner.
        assert_eq!(heavy.join().unwrap(), Some(Some("transcript")), "{owner}");
        assert_eq!(*order.lock().unwrap(), vec!["computed", owner, "published"]);
        assert!(!crate::scan_runtime::background_pending());
    }
}

#[test]
fn heavy_work_that_keeps_retaking_its_share_never_starves_a_waiting_index_owner() {
    let _serial = crate::scan_runtime::serial_test();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let heavy_done = done.clone();
    let (running_tx, running_rx) = mpsc::channel();
    let heavy = thread::spawn(move || {
        crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || {
            let _job = ActiveGuard::begin(None, WorkClass::Faces).unwrap().unwrap();
            let mut items = 0u32;
            while !heavy_done.load(std::sync::atomic::Ordering::SeqCst) {
                // Each item computes outside the share, then reads and
                // publishes under it again.
                let computed =
                    crate::scan_runtime::outside_share(|| thread::sleep(Duration::from_millis(1)));
                assert!(computed.is_some(), "an index owner discarded the face");
                thread::sleep(Duration::from_millis(1));
                items += 1;
                if items == 1 {
                    running_tx.send(()).unwrap();
                }
            }
            items
        })
    });
    running_rx.recv().unwrap();
    for owner in ["watcher", "source check", "file information", "watcher"] {
        let asked = Instant::now();
        let waited = background_owner(owner, || Ok(asked.elapsed())).unwrap();
        assert!(waited < Duration::from_secs(2), "{owner} waited {waited:?}");
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(heavy.join().unwrap().unwrap() > 1);
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
fn preview_lane_reports_closed_while_a_requested_preview_holds_it() {
    // Phase 10 fresh review of Phase 3: `derived_work::run_preview_pass` must
    // not contend for the index admission's urgent share — which preempts a
    // background index owner's turn (e.g. file-information completion) at
    // its safe point — when the preview lane cannot actually admit automatic
    // work anyway. A requested preview occupying the lane is exactly that
    // case: preempting an owner's turn to then find no slot free wastes the
    // turn for nothing.
    let _serial = crate::scan_runtime::serial_test();
    assert!(
        preview_lane_open_for_automatic(),
        "the lane starts open with nothing running"
    );

    let requested = requested_preview(None, "photo").unwrap();
    assert!(
        !preview_lane_open_for_automatic(),
        "a requested preview fills the lane for automatic work"
    );
    // `ActiveGuard::begin` (what a preview pass would actually call once it
    // held the urgent share) agrees: it also declines.
    assert!(ActiveGuard::begin(None, WorkClass::Previews).unwrap().is_none());

    drop(requested);
    assert!(
        preview_lane_open_for_automatic(),
        "the lane reopens once the requested preview finishes"
    );
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

fn seed_content(conn: &rusqlite::Connection, hash: &str, path: &str, size: i64) {
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES (?1, ?2, 'audio')",
        rusqlite::params![hash, size],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, size, mtime_ms, content_hash, missing)
         VALUES (?1, '/', ?1, 'audio', ?2, 1, ?3, 0)",
        rusqlite::params![path, size, hash],
    )
    .unwrap();
}

fn remove_content(conn: &rusqlite::Connection, hash: &str) {
    conn.execute("DELETE FROM paths WHERE content_hash = ?1", [hash]).unwrap();
    conn.execute("DELETE FROM contents WHERE hash = ?1", [hash]).unwrap();
}

/// One automatic transcription under an ordinary share whose inference runs
/// `meanwhile` as the index work that happens while it computes.
fn transcribe_while(
    conn: &rusqlite::Connection,
    root: &std::path::Path,
    hash: &str,
    meanwhile: impl FnOnce(),
) -> crate::derived_work::TranscriptionAttemptOutcome {
    let cache = crate::preview::CachePaths::new(root.join("cache"));
    crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || {
        let _job = ActiveGuard::begin(None, WorkClass::AudioTranscripts).unwrap().unwrap();
        crate::derived_work::complete_transcription_attempt_with_inference(
            crate::derived_work::TranscriptionAttempt {
                conn,
                cache: &cache,
                data_root: root,
                temp_dir: root.join("temp"),
                source_hash: hash,
                source_path: "/song.m4a",
                replace_existing: false,
                acceleration: crate::ai_acceleration::Mode::None,
                cancel_when: Some(Box::new(cancelled)),
            },
            |_| {},
            |_| {},
            |_, _| {},
            |_| {
                meanwhile();
                Ok(crate::transcription::Transcript {
                    language: Some("en".to_string()),
                    segments: vec![crate::transcription::Segment {
                        start_ms: 0,
                        end_ms: 1_000,
                        text: "spoken".to_string(),
                    }],
                })
            },
        )
        .unwrap()
    })
    .unwrap()
}

fn has_transcript(conn: &rusqlite::Connection, hash: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM transcripts WHERE content_hash = ?1)",
        [hash],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn heavy_work_publishes_only_for_the_content_the_index_still_names() {
    let _serial = crate::scan_runtime::serial_test();
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = crate::index_store::open(&db).unwrap();
    let watcher_turn = |change: &dyn Fn(&rusqlite::Connection)| {
        let watcher_conn = crate::index_store::open(&db).unwrap();
        crate::scan_runtime::with_watcher_claim(|| false, || {
            change(&watcher_conn);
            Ok(())
        })
        .unwrap();
    };

    // A watcher batch about another file: the transcript is published.
    seed_content(&conn, "kept", "/song.m4a", 10);
    let outcome = transcribe_while(&conn, root.path(), "kept", || {
        watcher_turn(&|conn| seed_content(conn, "other", "/other.m4a", 10))
    });
    assert!(matches!(outcome, crate::derived_work::TranscriptionAttemptOutcome::Completed { .. }));
    assert!(has_transcript(&conn, "kept"));

    // The file was removed while the model ran: nothing is published.
    seed_content(&conn, "removed", "/removed.m4a", 10);
    let outcome = transcribe_while(&conn, root.path(), "removed", || {
        watcher_turn(&|conn| remove_content(conn, "removed"))
    });
    assert_eq!(
        outcome,
        crate::derived_work::TranscriptionAttemptOutcome::Cancelled { hash: "removed".to_string() }
    );
    assert!(!has_transcript(&conn, "removed"));

    // A foreground action, such as a rebuild, overlapped the compute: the
    // result is discarded although the content row looks the same.
    seed_content(&conn, "rebuilt", "/rebuilt.m4a", 10);
    let outcome = transcribe_while(&conn, root.path(), "rebuilt", || {
        drop(crate::scan_runtime::admit_foreground(None, None, &|| false, &mut || {}).unwrap())
    });
    assert_eq!(
        outcome,
        crate::derived_work::TranscriptionAttemptOutcome::Cancelled { hash: "rebuilt".to_string() }
    );
    assert!(!has_transcript(&conn, "rebuilt"));

    // A provisional file replaced in place keeps its key `p<path_id>` but is
    // another file now: its result is discarded.
    seed_content(&conn, "p1", "/clip.m4a", 10);
    let published = crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || {
        crate::derived_state::compute_outside_share(&conn, "p1", || {
            watcher_turn(&|conn| {
                remove_content(conn, "p1");
                seed_content(conn, "p1", "/clip.m4a", 20);
            })
        })
        .unwrap()
    })
    .unwrap();
    assert!(published.is_none());
    let unchanged = crate::scan_runtime::with_derived_share(ShareRank::Ordinary, || {
        crate::derived_state::compute_outside_share(&conn, "p1", || ()).unwrap()
    })
    .unwrap();
    assert!(unchanged.is_some());
}
