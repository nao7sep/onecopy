use super::generation_is_live;

#[test]
fn replacement_and_shutdown_each_retire_an_owned_generation() {
    assert!(generation_is_live(4, 4, false));
    assert!(!generation_is_live(5, 4, false));
    assert!(!generation_is_live(4, 4, true));
}

// A directory the watcher could not re-read leaves its part
// of the library stale, so the pass carries a failure to report instead of
// letting the loop clear the watcher-failed condition.
#[test]
fn a_directory_that_fails_to_restat_keeps_the_watcher_failure_visible() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    let outside = dir.path().join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("new.jpg"), b"fresh").unwrap();
    let stored_root = crate::winpath::for_fs(&root).to_string_lossy().into_owned();
    // A data root distinct from `root`/`outside` (the data-root exclusion would
    // otherwise treat these watched directories as the app's own storage,
    // since they would sit inside it).
    let app_data = dir.path().join("app-data");
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({ "sourceDirs": [stored_root] })),
        &app_data,
        0,
    );

    let pass = super::restat_batch(&conn, &[outside.clone(), root.clone()], &settings, &|| Ok(())).unwrap();

    assert_eq!(pass.changed, 1, "the other directory is still updated");
    let failure = pass.failure().expect("the failed directory is reported");
    assert!(failure.contains(&*outside.to_string_lossy()), "{failure}");

    let clean = super::restat_batch(&conn, &[root], &settings, &|| Ok(())).unwrap();
    assert!(clean.failure().is_none(), "a clean pass clears the condition");
}

#[test]
fn recovery_scope_uses_path_boundaries_and_includes_overlapping_configured_roots() {
    use std::path::PathBuf;
    let roots = vec!["/photos".to_string(), "/photos/sub".to_string(), "/photos-other".to_string()];
    assert_eq!(super::affected_roots(&roots, &[PathBuf::from("/photos/sub/day")]), roots[..2]);
    assert_eq!(super::affected_roots(&roots, &[PathBuf::from("/photos-extra")]), roots);
}

#[test]
fn watcher_section_scope_covers_descendants_but_not_sibling_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("photos");
    let sibling = temp.path().join("photos-other");
    std::fs::create_dir_all(root.join("child")).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    std::fs::write(root.join("child/a.txt"), b"a").unwrap();
    std::fs::write(sibling.join("b.txt"), b"b").unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let roots: Vec<String> = [&root, &sibling].iter().map(|path| crate::winpath::for_fs(path).to_string_lossy().into_owned()).collect();
    let settings = crate::scanner::settings_from_config(Some(&serde_json::json!({ "sourceDirs": roots })), &temp.path().join("app"), 0);
    crate::scanner::run_source_check(&conn, &settings, &|_| {}).unwrap();
    conn.execute("UPDATE paths SET resolved_utc_ms = CASE WHEN file_name = 'a.txt' THEN 1767225600000 ELSE 1769904000000 END", []).unwrap();
    let stored = vec![crate::scanner::settled_root(&conn, &root).unwrap().to_string_lossy().into_owned()];
    let sections = crate::queries::sections_under_directories(&conn, &stored, chrono_tz::UTC).unwrap();
    assert_eq!(sections.len(), 1);
    assert!(sections.contains(&crate::queries::SectionLocation { kind: "other".to_string(), month: "2026-01".to_string() }));
    // Capturing before and after a changed date retains both affected months.
    conn.execute("UPDATE paths SET resolved_utc_ms = 1772323200000 WHERE file_name = 'a.txt'", []).unwrap();
    let mut both = sections;
    both.extend(crate::queries::sections_under_directories(&conn, &stored, chrono_tz::UTC).unwrap());
    assert_eq!(both.len(), 2);
    assert!(both.contains(&crate::queries::SectionLocation { kind: "other".to_string(), month: "2026-03".to_string() }));
}

#[test]
fn a_file_stat_failure_preserves_its_row_and_allows_other_entries_to_update() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    std::fs::write(root.join("uncertain.jpg"), b"old").unwrap();
    let settings = crate::scanner::settings_from_config(Some(&serde_json::json!({"sourceDirs": [root]})), &temp.path().join("data"), 0);
    super::restat_dir(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root()).unwrap();
    std::fs::write(root.join("new.jpg"), b"new").unwrap();
    for type_unknown in [false, true] {
        super::restat_dir_with(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root(), &|path| {
            let mut entries = crate::volume_io::read_dir(path, true)?;
            for entry in &mut entries {
                if entry.file_name == "uncertain.jpg" {
                    entry.metadata = Some(Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
                    if type_unknown { entry.file_type = None; }
                }
            }
            Ok(entries)
        }).unwrap();
        let present: i64 = conn.query_row("SELECT count(*) FROM paths WHERE missing = 0", [], |row| row.get(0)).unwrap();
        assert_eq!(present, 2);
        let issues: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::STAT_ERROR], |row| row.get(0)).unwrap();
        assert_eq!(issues, 1);
    }
    super::restat_dir(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root()).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::STAT_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 0);
    std::fs::remove_dir_all(&root).unwrap();
    super::restat_dir(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root()).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM paths WHERE missing = 0", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
}

#[test]
fn sources_offline_at_launch_register_on_return_and_retry_failed_registration() {
    let roots = vec!["first".to_string(), "second".to_string()];
    let mut watches = vec![None, None];
    let mut calls = Vec::new();
    let mut register = |_: usize, root: &str| { calls.push(root.to_string()); Ok(root.to_string()) };
    assert!(super::reconcile_watches(&roots, &mut watches, &roots, &[], &[], false, &mut register).is_empty());
    let returned = super::reconcile_watches(&roots, &mut watches, &[], &[], &[], false, &mut register);
    assert_eq!(returned.len(), 2);
    assert!(returned.iter().all(|(_, result)| result.is_ok()));
    assert!(super::reconcile_watches(&roots, &mut watches, &[], &[], &[], false, &mut register).is_empty());
    super::reconcile_watches(&roots, &mut watches, &roots, &[], &[], false, &mut register);
    let refused = super::reconcile_watches(&roots, &mut watches, &[], &roots, &[], false, &mut register);
    assert!(refused.is_empty(), "a substituted drive never registers");
    assert!(super::reconcile_watches(&roots, &mut watches, &[], &[], &[], true, &mut register).is_empty());
    assert_eq!(calls, roots);
    let failed = super::reconcile_watches(&roots, &mut watches, &[], &[], &[], false, &mut |_, _| Err("busy".to_string()));
    assert!(failed.iter().all(|(_, result)| result.is_err()));
    assert!(watches.iter().all(Option::is_none));
    assert_eq!(super::reconcile_watches(&roots, &mut watches, &[], &[], &[], false, &mut |_, root| Ok(root.to_string())).len(), 2);
}

// The event fold and the callback's queue hand-off are private; these tests
// reach them here rather than widening their visibility.
mod events {
    use super::super::{collect, forward_or_flag_overflow};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn no_data_root() -> PathBuf {
        PathBuf::from("/onecopy-test-data-root-never-used")
    }

    #[test]
    fn collect_skips_a_change_event_on_an_apple_double_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("IMG_0001.jpg");
        let sidecar = dir.path().join("._IMG_0001.jpg");
        std::fs::write(&real, b"photo").unwrap();
        std::fs::write(&sidecar, b"resource fork").unwrap();

        let (dirty, overflowed) = fold(vec![sidecar]);
        assert!(!overflowed);
        assert!(
            dirty.is_empty(),
            "a sidecar's own change event never dirties its directory"
        );
    }

    fn fold(paths: Vec<PathBuf>) -> (HashSet<PathBuf>, bool) {
        let mut dirty = HashSet::new();
        let mut overflowed = false;
        let event = notify::Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
            paths,
            attrs: Default::default(),
        };
        collect(Ok(event), &mut dirty, &mut overflowed, &no_data_root());
        (dirty, overflowed)
    }

    #[test]
    fn a_file_event_marks_its_parent_directory_dirty() {
        // The drain calls read_dir on whatever lands in the set. A file path there
        // fails silently, so new photos would simply never appear.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("IMG_0001.jpg");
        std::fs::write(&file, b"x").unwrap();

        let (dirty, overflowed) = fold(vec![file]);
        assert!(!overflowed);
        assert_eq!(
            dirty.into_iter().collect::<Vec<_>>(),
            vec![dir.path().to_path_buf()],
            "a file event marks the directory, never the file"
        );
    }

    #[test]
    fn a_directory_event_marks_the_directory_itself() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        let (dirty, _) = fold(vec![sub.clone()]);
        assert_eq!(dirty.into_iter().collect::<Vec<_>>(), vec![sub]);
    }

    #[test]
    fn the_apps_own_trash_is_never_marked_dirty() {
        // Trashing is a same-volume rename INSIDE a watched root, so every delete
        // fires events here; re-indexing them would resurrect what was just culled.
        let dir = tempfile::tempdir().unwrap();
        let trashed = dir
            .path()
            .join(crate::trash::TRASH_DIR_NAME)
            .join("20260101-utc")
            .join("IMG_0001.jpg");
        std::fs::create_dir_all(trashed.parent().unwrap()).unwrap();
        std::fs::write(&trashed, b"x").unwrap();

        let (dirty, _) = fold(vec![trashed]);
        assert!(dirty.is_empty(), "the app's own trash is not source material");
    }

    #[test]
    fn trash_filter_uses_the_event_path_before_resolving_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        // A removed trash directory no longer answers is_dir(). It must not dirty
        // the ordinary parent just because the event refers to a vanished path.
        let (dirty, overflowed) = fold(vec![dir.path().join(crate::trash::TRASH_DIR_NAME)]);
        assert!(!overflowed);
        assert!(dirty.is_empty());

        let lookalike = dir.path().join(format!("{}-notes", crate::trash::TRASH_DIR_NAME));
        std::fs::create_dir(&lookalike).unwrap();
        let file = lookalike.join("photo.jpg");
        std::fs::write(&file, b"ordinary").unwrap();
        let (dirty, _) = fold(vec![file]);
        assert_eq!(dirty, HashSet::from([lookalike]));
    }

    #[test]
    fn a_lost_event_batch_flags_an_overflow() {
        // notify drops events under load; the flag is what turns that into a
        // visible "Rescan needed" instead of a silently incomplete index.
        let mut dirty = HashSet::new();
        let mut overflowed = false;
        collect(
            Err(notify::Error::generic("watch queue overflowed")),
            &mut dirty,
            &mut overflowed,
            &no_data_root(),
        );
        assert!(overflowed, "a watcher error must raise the rescan flag");
        assert!(dirty.is_empty());
    }

    // The channel between `notify`'s callback and the drain loop is
    // bounded: a full queue flags overflow instead of growing without bound
    // while ingestion is blocked (e.g. a long `INDEXING` hold).
    #[test]
    fn a_full_event_queue_flags_overflow_instead_of_blocking_the_callback() {
        let (tx, rx) = std::sync::mpsc::sync_channel::<notify::Result<notify::Event>>(1);
        let overflowed = std::sync::atomic::AtomicBool::new(false);
        let event = || {
            Ok(notify::Event {
                kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
                paths: vec![],
                attrs: Default::default(),
            })
        };

        forward_or_flag_overflow(&tx, &overflowed, event());
        assert!(
            !overflowed.load(std::sync::atomic::Ordering::SeqCst),
            "the first event fits inside capacity"
        );

        // The queue is now full (nothing has drained it): the next send cannot
        // block the notify callback thread, so it must flag overflow instead.
        forward_or_flag_overflow(&tx, &overflowed, event());
        assert!(
            overflowed.load(std::sync::atomic::Ordering::SeqCst),
            "a full queue must flag overflow rather than block or silently drop"
        );
        drop(rx);
    }

    #[test]
    fn callback_failure_marks_only_its_root_for_recovery_even_without_a_queued_event() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let affected_root = AtomicBool::new(false);
        let other_root = AtomicBool::new(false);
        forward_or_flag_overflow(&tx, &affected_root, Err(notify::Error::generic("events lost")));
        assert!(affected_root.load(Ordering::SeqCst));
        assert!(!other_root.load(Ordering::SeqCst));
        assert!(rx.try_recv().is_err(), "the root's flag drives recovery even on an idle queue");
    }
}

#[test]
fn uncertain_sources_retire_only_their_watch_and_recover_without_interrupting_healthy_roots() {
    let data = tempfile::tempdir().unwrap();
    let roots = vec!["uncertain".to_string(), "healthy".to_string(), "missing".to_string(), "substituted".to_string()];
    let mut watches = vec![Some(10), Some(20), Some(30), Some(40)];
    let uncertain = &roots[..1];
    let missing = &roots[2..3];
    let substituted = &roots[3..];
    assert!(super::reconcile_watches(&roots, &mut watches, missing, substituted, uncertain, false,
        &mut |_, _| panic!("healthy watch stays registered")).is_empty());
    assert_eq!(watches, vec![None, Some(20), None, None]);
    let unavailable: Vec<_> = missing.iter().chain(uncertain).cloned().collect();
    super::reconcile_source_conditions(data.path(), &roots, &unavailable, substituted).unwrap();
    let conn = crate::index_store::open(&data.path().join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();
    let affected = || {
        let mut statement = conn.prepare("SELECT path FROM active_issues WHERE kind = 'source-unavailable' ORDER BY path").unwrap();
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap();
        rows
    };
    assert_eq!(affected(), vec!["missing", "substituted", "uncertain"]);
    let recovered = super::reconcile_watches(&roots, &mut watches, missing, substituted, &[], false,
        &mut |index, _| Ok(index));
    assert_eq!(recovered, vec![(0, Ok(()))]);
    assert_eq!(watches, vec![Some(0), Some(20), None, None]);
    super::reconcile_source_conditions(data.path(), &roots, missing, substituted).unwrap();
    assert_eq!(affected(), vec!["missing", "substituted"]);
    super::reconcile_watches(&roots, &mut watches, &[], &[], &[], true,
        &mut |_, _| panic!("whole-store uncertainty disables all watches"));
    assert!(watches.iter().all(Option::is_none));
}

#[test]
fn a_nonempty_event_queue_stops_at_the_deadline_and_retains_its_backlog() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(3);
    for event in 0..3 { sender.send(event).unwrap(); }
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_secs(2);
    let mut ticks = 0;
    let mut collected = Vec::new();
    super::drain_events(&receiver, deadline, || {
        let now = start + std::time::Duration::from_secs(ticks);
        ticks += 1;
        now
    }, || true, |event| collected.push(event));
    assert_eq!(collected, vec![0, 1]);
    assert_eq!(receiver.try_recv().unwrap(), 2);
}

#[test]
fn a_retired_generation_stops_before_receiving_queued_events() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(2);
    sender.send(0).unwrap();
    sender.send(1).unwrap();
    let now = std::time::Instant::now();
    let mut checks = 0;
    let mut collected = Vec::new();
    super::drain_events(&receiver, now + std::time::Duration::from_secs(2), || now, || {
        checks += 1;
        checks == 1
    }, |event| collected.push(event));
    assert_eq!(collected, vec![0]);
    assert_eq!(receiver.try_recv().unwrap(), 1);
}

#[test]
fn stopping_a_batch_preserves_collected_overflow_and_queued_events() {
    let (sender, receiver) = std::sync::mpsc::sync_channel(2);
    sender.send(Err(notify::Error::generic("events lost"))).unwrap();
    sender.send(Err(notify::Error::generic("next batch"))).unwrap();
    let start = std::time::Instant::now();
    let mut ticks = 0;
    let mut dirty = std::collections::HashSet::new();
    let mut overflowed = false;
    super::drain_events(&receiver, start + std::time::Duration::from_secs(1), || {
        let now = start + std::time::Duration::from_secs(ticks);
        ticks += 1;
        now
    }, || true, |event| super::collect(event, &mut dirty, &mut overflowed, &std::path::PathBuf::from("/onecopy-test-data-root-never-used")));
    assert!(overflowed);
    assert!(dirty.is_empty());
    assert!(receiver.try_recv().unwrap().is_err());
}

#[test]
fn a_database_failure_stops_the_directory_batch_before_its_next_entry() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("new.txt"), b"new").unwrap();
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({"sourceDirs": [root]})), &temp.path().join("data"), 0,
    );
    conn.authorizer(Some(|context: AuthContext<'_>| {
        if matches!(context.action, AuthAction::Insert { table_name: "paths" }) { Authorization::Deny }
        else { Authorization::Allow }
    })).unwrap();
    let entered = std::cell::Cell::new(0);
    let result = super::restat_batch(&conn, &[root.clone(), root], &settings, &|| {
        entered.set(entered.get() + 1); Ok(())
    });
    assert!(result.is_err());
    assert_eq!(entered.get(), 1);
}

#[test]
fn repeated_substitution_probes_report_only_new_identity_conditions() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let data = root.path().join("data");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&data).unwrap();
    crate::storage::save_config(&data, &serde_json::json!({"sourceDirs": [source]})).unwrap();
    let mut previous = Vec::new();
    let mut reports = Vec::new();
    for identity in ["original", "other-a", "other-a", "other-b", "other-b", "original", "other-b"] {
        let status = crate::volume::verify_source_dirs_with(&data, &|_| Some(identity.to_string())).unwrap();
        assert_eq!(status.substituted.len(), usize::from(identity != "original"));
        super::report_substitution_changes(&mut previous, status.substitutions, |change| {
            reports.push((change.recorded.clone(), change.current.clone()));
        });
    }
    assert_eq!(reports, [
        ("original".to_string(), "other-a".to_string()),
        ("original".to_string(), "other-b".to_string()),
        ("original".to_string(), "other-b".to_string()),
    ]);
}

#[test]
fn clean_restat_issue_reads_scale_by_batch_instead_of_file() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    let measure = |files: usize| {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        std::fs::create_dir(&root).unwrap();
        for index in 0..files {
            std::fs::write(root.join(format!("{index}.txt")), b"unchanged").unwrap();
        }
        let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
        let settings = crate::scanner::settings_from_config(
            Some(&serde_json::json!({"sourceDirs": [root]})), &temp.path().join("data"), 0,
        );
        super::restat_dir(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root()).unwrap();
        let reads = Arc::new(AtomicUsize::new(0));
        let observed = reads.clone();
        conn.authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(context.action, AuthAction::Read { table_name: "issue_events", column_name: "event" }) {
                observed.fetch_add(1, Ordering::Relaxed);
            }
            Authorization::Allow
        })).unwrap();
        assert_eq!(super::restat_dir(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root()).unwrap(), 0);
        reads.load(Ordering::Relaxed)
    };
    let one = measure(1);
    let full = measure(super::RESTAT_ISSUE_BATCH_SIZE);
    let next = measure(super::RESTAT_ISSUE_BATCH_SIZE + 1);
    assert!(one > 0);
    assert_eq!(full, one, "a clean batch does not probe each file's absent conditions");
    assert!(next > full && next < full * 2, "only one additional membership query joins the directory-level check: {one}, {full}, {next}");
}

#[test]
fn restat_batches_close_recovered_conditions_but_keep_new_and_unrelated_failures() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("source");
    std::fs::create_dir(&root).unwrap();
    for index in 0..=super::RESTAT_ISSUE_BATCH_SIZE {
        std::fs::write(root.join(format!("{index:04}.txt")), b"file").unwrap();
    }
    let conn = crate::index_store::open(&temp.path().join("index.sqlite3")).unwrap();
    let settings = crate::scanner::settings_from_config(
        Some(&serde_json::json!({"sourceDirs": [root]})), &temp.path().join("data"), 0,
    );
    let first = crate::native_path::encode(crate::winpath::for_fs(&root.join("0000.txt")).as_ref()).into_owned();
    let last = crate::native_path::encode(crate::winpath::for_fs(&root.join(format!("{:04}.txt", super::RESTAT_ISSUE_BATCH_SIZE))).as_ref()).into_owned();
    for path in [&first, &last] {
        for kind in [crate::scanner::STAT_ERROR, crate::scanner::WALK_ERROR, crate::scanner::READ_ERROR] {
            crate::index_store::upsert_issue(&conn, Some(path), kind, "prior condition").unwrap();
        }
    }
    super::restat_dir_with(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root(), &|path| {
        let mut entries = crate::volume_io::read_dir(path, true)?;
        entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));
        for entry in &mut entries {
            if entry.file_name == "0128.txt" {
                entry.metadata = Some(Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
            }
        }
        Ok(entries)
    }).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::STAT_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::WALK_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::READ_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(conn.query_row("SELECT count(*) FROM records.issue_events WHERE event = 'resolved'", [], |row| row.get::<_, i64>(0)).unwrap(), 4);
    super::restat_dir(&conn, &root, &settings.lists, &settings.source_dirs, settings.data_root()).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::scanner::STAT_ERROR], |row| row.get::<_, i64>(0)).unwrap(), 0);
}
