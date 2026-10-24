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
fn cancellation_cannot_carry_an_old_owners_bit_into_a_successor() {
    let _serial = serial_test();
    let old = enter(Owner::SourceCheck, &|| false);
    let (observed_tx, observed_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    let cancelling = std::thread::spawn(move || {
        cancel_at_transition(Owner::SourceCheck, || {
            observed_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
        });
    });
    observed_rx.recv().unwrap();

    // Cancellation has observed the old owner but has not written its bit.
    // The successor cannot retire or publish through this same boundary
    // until that write finishes; this checks the lock, not elapsed time.
    let transition_held = matches!(
        OWNER_TRANSITION.try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    );
    finish_tx.send(()).unwrap();
    cancelling.join().unwrap();
    assert!(transition_held);
    assert!(crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst));

    drop(old);
    let successor = enter(Owner::Foreground, &|| false);
    request_cancel(Owner::SourceCheck);
    assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), Owner::Foreground as u8);
    assert!(!crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst));
    drop(successor);
    assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), 0);
    assert!(!crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst));
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
fn a_file_removed_while_its_walk_was_parked_restarts_without_a_failure() {
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
        // Mutation admission invalidates even a producer that has not yet
        // delivered its first entry.
        restart_source_walks();
        std::fs::remove_file(&removed).unwrap();
        drop(guard);
    });
    wait_until(foreground_pending);
    go_tx.send(()).unwrap();

    assert_eq!(walker.join().unwrap().unwrap_err(), RESTART);
    foreground.join().unwrap();
    let conn = crate::index_store::open(&db).unwrap();
    let issues: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM active_issues WHERE kind = ?1",
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

#[cfg(unix)]
#[test]
fn section_recheck_finishes_healthy_directories_and_tail_after_a_read_refusal() {
    use std::os::unix::fs::PermissionsExt;
    let _serial = serial_test();
    crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    let locked = root.join("locked");
    let healthy = root.join("healthy");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::create_dir_all(&healthy).unwrap();
    std::fs::write(locked.join("kept.txt"), b"old file").unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({"sourceDirs": [root]})), &temp.path().join("data"), 0,
    );
    crate::scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
    std::fs::write(healthy.join("new.txt"), b"new healthy file").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let dirs: Vec<String> = [&locked, &healthy].iter().map(|path| crate::native_path::encode(path).into_owned()).collect();
    let result = recheck_section_dirs(&conn, &settings, &dirs, &|| Ok(()));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.unwrap_err().contains("locked"));
    let resolved: bool = conn.query_row(
        "SELECT resolved_source IS NOT NULL FROM paths WHERE file_name = 'new.txt' AND missing = 0", [], |row| row.get(0),
    ).unwrap();
    assert!(resolved, "healthy file reaches the information tail despite the other directory's failure");
    assert_eq!(conn.query_row("SELECT missing FROM paths WHERE file_name = 'kept.txt'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::WALK_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(conn.query_row("SELECT relationship_dirty FROM scan_dirs", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    recheck_section_dirs(&conn, &settings, &dirs, &|| Ok(())).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::WALK_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn section_recheck_cancellation_stops_before_directory_work() {
    let _serial = serial_test();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("new.txt"), b"untouched").unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({"sourceDirs": [root]})), &temp.path().join("data"), 0,
    );
    let dirs = [crate::native_path::encode(&root).into_owned()];
    assert_eq!(recheck_section_dirs(&conn, &settings, &dirs, &|| Err(crate::scanner::CANCELLED.into())).unwrap_err(), crate::scanner::CANCELLED);
    assert_eq!(conn.query_row("SELECT count(*) FROM paths", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn a_stalled_source_entry_releases_the_index_and_keeps_its_own_cancellation() {
    let _serial = serial_test();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("file.txt"), b"file").unwrap();
    let db = temp.path().join("index.sqlite3");
    let volume = crate::volume_io::FakeStallingVolume::mount(&root, Duration::from_secs(5));
    volume.stall(&[crate::volume_io::Op::List], None);
    let source = root.clone();
    let walker = std::thread::spawn(move || {
        let conn = crate::index_store::open(&db).unwrap();
        let settings = crate::scanner::settings_from_config(None, &temp.path().join("data"), 0);
        with_source_check_claim(|| false, |_| {}, || crate::scanner::walk_root(&conn, &source, &settings.lists))
    });
    assert!(volume.wait_until_held(1, Duration::from_secs(3)));
    // Admission completes while the physical listing is still held.
    let foreground = admit_foreground(None, Some(Instant::now() + Duration::from_secs(2)), &|| false, &mut || {}).unwrap();
    request_cancel(Owner::Foreground);
    assert!(crate::scanner::cancelled());
    volume.release();
    drop(foreground);
    let stats = walker.join().unwrap().unwrap();
    assert_eq!(stats.seen, 1);
    assert_eq!(stats.errors, 0);
    assert!(index_state().parked.is_none());
}

#[test]
fn a_source_snapshot_cannot_publish_buffered_files_or_absence_after_delete_and_restore() {
    let _serial = serial_test();
    for watcher in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        std::fs::create_dir(&root).unwrap();
        for name in ["first.txt", "second.txt", "third.txt"] {
            std::fs::write(root.join(name), b"old").unwrap();
        }
        let db = temp.path().join("index.sqlite3");
        let config = serde_json::json!({"sourceDirs": [root]});
        let settings = crate::scanner::settings_from_config(Some(&config), &temp.path().join("data"), 0);
        let conn = crate::index_store::open(&db).unwrap();
        crate::scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
        let (paused, observed) = mpsc::channel();
        let (resume, held) = mpsc::channel();
        let walk_db = db.clone();
        let walk_settings = crate::scanner::settings_from_config(Some(&config), &temp.path().join("data"), 0);
        let walker = std::thread::spawn(move || {
            let conn = crate::index_store::open(&walk_db).unwrap();
            let once = std::cell::Cell::new(false);
            let work = || crate::scanner::run_source_check(&conn, &walk_settings, &|value| {
                if value.discovered == Some(1) && !once.replace(true) {
                    paused.send(()).unwrap();
                    held.recv().unwrap();
                }
            });
            if watcher { with_watcher_claim(|| false, work) }
            else { with_source_check_claim(|| false, |_| {}, work) }
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let mutation_root = root.clone();
        let mutation_db = db.clone();
        let mutation_settings = crate::scanner::settings_from_config(Some(&config), &temp.path().join("data"), 0);
        let mutation = std::thread::spawn(move || {
            let foreground = admit_foreground(None, Some(Instant::now() + Duration::from_secs(5)), &|| false, &mut || {}).unwrap();
            // The mutation admission uses this invalidation before any effect.
            restart_source_walks();
            for name in ["first.txt", "second.txt", "third.txt"] {
                std::fs::remove_file(mutation_root.join(name)).unwrap();
            }
            std::fs::write(mutation_root.join("restored.txt"), b"restored").unwrap();
            let conn = crate::index_store::open(&mutation_db).unwrap();
            conn.execute("DELETE FROM paths", []).unwrap();
            crate::scanner::walk_root(&conn, &std::fs::canonicalize(&mutation_root).unwrap(), &mutation_settings.lists).unwrap();
            drop(foreground);
        });
        wait_until(foreground_pending);
        resume.send(()).unwrap();
        assert_eq!(walker.join().unwrap().unwrap_err(), RESTART);
        mutation.join().unwrap();
        let names: Vec<(String, bool)> = conn.prepare("SELECT file_name, missing FROM paths").unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(names, vec![("restored.txt".into(), false)]);
        with_source_check_claim(|| false, |_| {}, || crate::scanner::run_source_check(&conn, &settings, &|_| {})).unwrap();
        assert_eq!(conn.query_row("SELECT count(*) FROM paths WHERE missing = 0", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }
}

#[test]
fn unwinding_a_source_wait_releases_its_parked_reservation() {
    let _serial = serial_test();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_source_check_claim(|| false, |_| {}, || {
            wait_for_walk_entry::<()>(|_| panic!("injected source wait panic"))
        })
    }));
    assert!(result.is_err());
    assert!(index_state().holder.is_none());
    assert!(index_state().parked.is_none());
    let guard = admit_foreground(None, Some(Instant::now() + Duration::from_secs(1)), &|| false, &mut || {}).unwrap();
    drop(guard);
}

#[test]
fn section_recheck_admits_healthy_sources_without_resetting_or_draining_refused_sources() {
    let _serial = serial_test();
    for unreadable in [false, true] {
        crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
        let temp = tempfile::tempdir().unwrap();
        let healthy = temp.path().join("healthy");
        let refused = temp.path().join("refused");
        let data = temp.path().join("data");
        for root in [&healthy, &refused, &data] { std::fs::create_dir(root).unwrap(); }
        image::RgbImage::new(2, 2).save(healthy.join("photo.jpg")).unwrap();
        image::RgbImage::new(3, 3).save(refused.join("photo.jpg")).unwrap();
        let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
        let settings = crate::scanner::settings_from_config(Some(&serde_json::json!({"sourceDirs": [healthy, refused]})), &data,
            chrono::Utc::now().timestamp_millis());
        crate::scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
        crate::scanner::run_index_tail(&conn, &settings, &|_| {}, &mut Default::default()).unwrap();
        conn.execute_batch("UPDATE paths SET hash_attempt_failed = 1, metadata_attempt_failed = 1, indexed_at_utc = NULL,
                            resolved_utc_ms = NULL, resolved_source = NULL;
                            UPDATE contents SET derive_outcome = 'failed';").unwrap();
        image::RgbImage::new(4, 4).save(refused.join("pending.jpg")).unwrap();
        let healthy_dir = crate::native_path::encode(crate::winpath::for_fs(&std::fs::canonicalize(&healthy).unwrap()).as_ref()).into_owned();
        let refused_dir = crate::native_path::encode(crate::winpath::for_fs(&std::fs::canonicalize(&refused).unwrap()).as_ref()).into_owned();
        crate::watcher::restat_dir(&conn, crate::native_path::from_text(&refused_dir).as_ref(), &settings.lists, &settings.source_dirs, settings.data_root()).unwrap();
        let snapshot = || -> Vec<(String, Option<String>, i64, i64, Option<String>, Option<String>)> {
            conn.prepare("SELECT file_name, content_hash, hash_attempt_failed, metadata_attempt_failed, indexed_at_utc, resolved_source
                          FROM paths WHERE dir_path = ?1 ORDER BY id").unwrap().query_map([&refused_dir],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))
                .unwrap().collect::<rusqlite::Result<_>>().unwrap()
        };
        let before = snapshot();
        for root in &settings.source_dirs { crate::volume::check_identity(&data, root, "original").unwrap(); }
        let check = |roots: &[String]| {
            crate::volume::enforce_no_substitution_with(&data, roots, &|_| {
                if roots[0] == settings.source_dirs[1] {
                    if unreadable { None } else { Some("replacement".into()) }
                } else { Some("original".into()) }
            })
        };
        let error = recheck_section_with_admission(&conn, &settings, &[healthy_dir.clone(), refused_dir.clone()],
            crate::queries::SectionKind::Image, None, &check, &|| Ok(())).unwrap_err();
        assert!(error.contains("refused"), "{error}");
        assert_eq!(snapshot(), before, "refused retries and unrelated pending work stay untouched");
        assert!(conn.query_row("SELECT indexed_at_utc IS NOT NULL AND metadata_attempt_failed = 0 FROM paths WHERE dir_path = ?1",
            [&healthy_dir], |row| row.get::<_, bool>(0)).unwrap());
        assert_eq!(conn.query_row("SELECT derive_outcome FROM contents WHERE hash = (SELECT content_hash FROM paths WHERE dir_path = ?1 AND file_name = 'photo.jpg')",
            [&refused_dir], |row| row.get::<_, String>(0)).unwrap(), "failed");
        assert!(conn.query_row("SELECT derive_outcome IS NULL FROM contents WHERE hash = (SELECT content_hash FROM paths WHERE dir_path = ?1)",
            [&healthy_dir], |row| row.get::<_, bool>(0)).unwrap());
    }
}

#[test]
fn a_healthy_parent_cannot_admit_a_refused_nested_source_through_its_settled_alias() {
    let temp = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let encode = |path: std::path::PathBuf| crate::native_path::encode(crate::winpath::for_fs(&path).as_ref()).into_owned();
    let parent = encode(temp.path().join("configured"));
    let nested = encode(temp.path().join("configured").join("nested"));
    let settled = encode(temp.path().join("settled"));
    let nested_settled = encode(temp.path().join("settled").join("nested"));
    for (root, configured) in [(&settled, &parent), (&nested_settled, &nested)] {
        conn.execute("INSERT INTO scan_dirs (root, configured_root) VALUES (?1, ?2)", rusqlite::params![root, configured]).unwrap();
    }
    let healthy = encode(temp.path().join("settled").join("sibling"));
    let refused = encode(temp.path().join("settled").join("nested").join("photos"));
    let probes = std::cell::RefCell::new(Vec::new());
    let (admitted, failures) = admitted_section_dirs(&conn, &[parent.clone(), nested.clone()], &[healthy.clone(), refused.clone(), refused], &|roots| {
        probes.borrow_mut().push(roots[0].clone());
        if roots[0] == nested { Err("nested source refused".into()) } else { Ok(()) }
    }).unwrap();
    assert_eq!(admitted, [healthy]);
    assert_eq!(failures, ["nested source refused"]);
    assert_eq!(*probes.borrow(), [parent, nested]);
}
