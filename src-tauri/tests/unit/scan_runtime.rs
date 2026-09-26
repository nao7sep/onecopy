use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

fn holder_present() -> bool {
    index_state().holder.is_some()
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition never became true");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn cancellation_is_revalidated_after_the_projection_lock_is_owned() {
    let _serial = serial_test();
    let ran = std::cell::Cell::new(false);

    let result = with_owner(
        Owner::Watcher,
        || true,
        || {
            ran.set(true);
            Ok(())
        },
    );

    assert_eq!(result.unwrap_err(), crate::scanner::CANCELLED);
    assert!(!ran.get());
    assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), 0);
    assert!(!crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst));
}

#[test]
fn a_foreground_wait_shows_itself_answers_busy_at_its_deadline_and_ends_on_cancel() {
    let _serial = serial_test();
    // Another foreground action holds the claim and cannot be preempted.
    let first = admit_foreground(None, None, &|| false, &mut || {}).unwrap();

    let deadline = Instant::now() + Duration::from_millis(150);
    let busy = admit_foreground(None, Some(deadline), &|| false, &mut || {});
    assert!(matches!(busy, Err(Refusal::Busy)));
    assert!(Instant::now() < deadline + Duration::from_millis(500));

    let cancel = AtomicBool::new(false);
    let shown = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            admit_foreground(None, None, &|| cancel.load(Ordering::SeqCst), &mut || {
                shown.store(true, Ordering::SeqCst)
            })
            .map(|_| ())
        });
        wait_until(|| shown.load(Ordering::SeqCst));
        let requested = Instant::now();
        cancel.store(true, Ordering::SeqCst);
        assert!(matches!(waiter.join().unwrap(), Err(Refusal::Cancelled)));
        assert!(requested.elapsed() < Duration::from_millis(500));
    });
    drop(first);
    assert!(!holder_present());
    assert!(!foreground_pending());
}

#[test]
fn a_parked_source_walk_continues_after_foreground_work_and_restarts_after_a_reset() {
    let _serial = serial_test();
    for reset in [false, true] {
        let parked = Arc::new(AtomicBool::new(false));
        let observed = parked.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let walker = std::thread::spawn(move || {
            with_source_check_claim(
                || false,
                move |waiting| observed.store(waiting, Ordering::SeqCst),
                || {
                    entered_tx.send(()).unwrap();
                    let mut steps = 0u32;
                    while steps < 100 {
                        yield_at_safe_point()?;
                        steps += 1;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Ok(steps)
                },
            )
        });
        entered_rx.recv().unwrap();

        let requested = Instant::now();
        let foreground = admit_foreground(
            None,
            Some(requested + Duration::from_secs(5)),
            &|| false,
            &mut || {},
        )
        .unwrap();
        assert!(requested.elapsed() < Duration::from_secs(1));
        assert!(parked.load(Ordering::SeqCst));
        assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), Owner::Foreground as u8);
        if reset {
            restart_source_walks();
        }
        drop(foreground);

        let result = walker.join().unwrap();
        if reset {
            assert_eq!(result.unwrap_err(), RESTART);
        } else {
            // The walk kept its progress instead of starting over.
            assert_eq!(result.unwrap(), 100);
        }
        assert!(!parked.load(Ordering::SeqCst));
        assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), 0);
        assert!(!holder_present());
    }
}

#[test]
fn a_parked_owner_takes_the_claim_back_before_another_background_owner() {
    let _serial = serial_test();
    let released = Arc::new(AtomicBool::new(false));
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let (entered_tx, entered_rx) = mpsc::channel();
    let walker_released = released.clone();
    let walker_order = order.clone();
    let walker = std::thread::spawn(move || {
        let lingered = AtomicBool::new(false);
        with_source_check_claim(
            // Slow to notice that the foreground action finished, so any
            // other waiting owner has every chance to take the claim first.
            move || {
                if walker_released.load(Ordering::SeqCst) && !lingered.swap(true, Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(150));
                }
                false
            },
            |_| {},
            move || {
                entered_tx.send(()).unwrap();
                while !yield_at_safe_point()? {
                    std::thread::sleep(Duration::from_millis(2));
                }
                walker_order.lock().unwrap().push("walker");
                Ok(())
            },
        )
    });
    entered_rx.recv().unwrap();
    let foreground = admit_foreground(
        None,
        Some(Instant::now() + Duration::from_secs(5)),
        &|| false,
        &mut || {},
    )
    .unwrap();

    let watcher_order = order.clone();
    let watcher = std::thread::spawn(move || {
        with_watcher_claim(
            || false,
            move || {
                watcher_order.lock().unwrap().push("watcher");
                Ok(())
            },
        )
    });
    std::thread::sleep(Duration::from_millis(100));
    released.store(true, Ordering::SeqCst);
    drop(foreground);

    walker.join().unwrap().unwrap();
    watcher.join().unwrap().unwrap();
    // Nothing writes between the parked walk's prefix and the rest of it.
    assert_eq!(*order.lock().unwrap(), vec!["walker", "watcher"]);
    assert!(!holder_present());
    assert!(index_state().parked.is_none());
}

#[test]
fn a_file_removed_while_its_walk_was_parked_is_absent_not_a_failure() {
    let _serial = serial_test();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("index.sqlite3");
    let sources = dir.path().join("sources");
    std::fs::create_dir(&sources).unwrap();
    // The walk's first entry is this file, so it parks with the entry
    // already listed, exactly as it does mid-directory.
    let photo = sources.join("photo.jpg");
    std::fs::write(&photo, b"data").unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let walk_db = db.clone();
    let walk_root = photo.clone();
    let walker = std::thread::spawn(move || {
        let conn = crate::index_store::open(&walk_db).unwrap();
        let lists = crate::scanner::ScanLists {
            images: vec!["jpg".into()],
            videos: vec![],
            audio: vec![],
            companions: vec![],
        };
        with_source_check_claim(
            || false,
            |_| {},
            || {
                entered_tx.send(()).unwrap();
                go_rx.recv().unwrap();
                crate::scanner::walk_root(&conn, &walk_root, &lists)
            },
        )
    });
    entered_rx.recv().unwrap();
    let removed = photo.clone();
    let foreground = std::thread::spawn(move || {
        let guard = admit_foreground(
            None,
            Some(Instant::now() + Duration::from_secs(5)),
            &|| false,
            &mut || {},
        )
        .unwrap();
        // The foreground action deletes the listed file.
        std::fs::remove_file(&removed).unwrap();
        drop(guard);
    });
    wait_until(foreground_pending);
    go_tx.send(()).unwrap();

    let stats = walker.join().unwrap().unwrap();
    foreground.join().unwrap();
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.seen, 0);
    let conn = crate::index_store::open(&db).unwrap();
    let issues: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM issues WHERE kind = ?1 AND closed_at_utc IS NULL",
            [crate::scanner::STAT_ERROR],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(issues, 0);
}

#[test]
fn the_share_admission_and_the_reported_availability_give_one_answer() {
    let _serial = serial_test();
    assert!(derived_work_admissible());
    assert!(ordinary_share_free());
    assert_eq!(with_derived_share(ShareRank::Ordinary, || ()), Some(()));

    let foreground = admit_foreground(None, None, &|| false, &mut || {}).unwrap();
    // A held index claim declines an ordinary turn, and availability says so.
    assert!(!ordinary_share_free());
    assert_eq!(with_derived_share(ShareRank::Ordinary, || ()), None);
    drop(foreground);

    FOREGROUND_WAITERS.fetch_add(1, Ordering::SeqCst);
    // A waiting foreground action closes derived admission for every rank.
    assert!(!derived_work_admissible());
    assert!(!ordinary_share_free());
    assert_eq!(with_derived_share(ShareRank::Urgent, || ()), None);
    FOREGROUND_WAITERS.fetch_sub(1, Ordering::SeqCst);
    assert!(ordinary_share_free());
}
