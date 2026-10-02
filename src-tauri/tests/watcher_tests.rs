// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::scanner::ScanLists;
use onecopy_lib::watcher::*;
use onecopy_lib::extensions;
use onecopy_lib::index_store;
use std::collections::HashSet;
use std::path::PathBuf;

fn lists() -> ScanLists {
    let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    ScanLists {
        images: owned(extensions::IMAGE_EXTENSIONS),
        videos: owned(extensions::VIDEO_EXTENSIONS),
        audio: owned(extensions::AUDIO_EXTENSIONS),
        companions: owned(extensions::COMPANION_EXTENSIONS),
    }
}

fn no_data_root() -> PathBuf {
    // A path these tests never write under, so restat_dir's data-root
    // exclusion (R6-02) never fires unless a test deliberately targets it.
    PathBuf::from("/onecopy-test-data-root-never-used")
}

#[test]
fn restat_upserts_new_files_and_marks_vanished_missing() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-watch-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("new.jpg"), b"fresh").unwrap();

    let changed = restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap();
    assert_eq!(changed, 1);
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM paths WHERE missing = 0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);

    // Unchanged re-stat: nothing to do.
    assert_eq!(restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap(), 0);

    // Vanished file: marked missing, row kept.
    std::fs::remove_file(root.join("new.jpg")).unwrap();
    assert_eq!(restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap(), 1);
    let missing: i64 = conn
        .query_row("SELECT COUNT(*) FROM paths WHERE missing = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(missing, 1);
}

#[test]
fn restat_skips_apple_double_sidecars_beside_their_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("IMG_0001.jpg"), b"photo").unwrap();
    std::fs::write(root.join("._IMG_0001.jpg"), b"resource fork").unwrap();

    let changed = restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap();
    assert_eq!(changed, 1, "only the real file is indexed");
    let names: Vec<String> = {
        let mut stmt = conn.prepare("SELECT file_name FROM paths").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    assert_eq!(names, vec!["IMG_0001.jpg".to_string()]);

    // A row left over from before this exclusion (or from some other path)
    // leaves cleanly on the next re-stat: marked missing, no Issue.
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, stem, kind, size, mtime_ms, missing) \
         VALUES (?1, ?2, '._IMG_0001.jpg', '._img_0001', 'other', 0, 0, 0)",
        rusqlite::params![
            root.join("._IMG_0001.jpg").to_string_lossy().to_string(),
            root.to_string_lossy().to_string()
        ],
    )
    .unwrap();
    let changed = restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap();
    assert_eq!(changed, 1);
    let missing: i64 = conn
        .query_row(
            "SELECT missing FROM paths WHERE file_name = '._IMG_0001.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 1);
    let issues: i64 = conn.query_row("SELECT COUNT(*) FROM active_issues", [], |r| r.get(0)).unwrap();
    assert_eq!(issues, 0, "an excluded path is absent, never a failure");

    // The real file gone leaves the sidecar as ordinary content: it is
    // indexed like any other file, not treated as metadata forever.
    std::fs::remove_file(root.join("IMG_0001.jpg")).unwrap();
    restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap();
    let missing: i64 = conn
        .query_row(
            "SELECT missing FROM paths WHERE file_name = '._IMG_0001.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 0, "a lone ._name with no sibling is ordinary content");
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

// (W-M2) A vanished directory PROVES absence — `read_dir` failing with
// `NotFound` is the normal shape of "this folder was deleted after its
// events arrived" (Shift+Del, `rm -r`), not an unreadable directory. Every
// row known to live under it is marked missing instead of raising a
// walk-error Issue for an expected condition.
#[test]
fn a_vanished_directory_marks_every_row_under_it_missing() {
    // A SUBDIRECTORY vanishing, not the configured source root itself: the
    // root staying put keeps `source_root_spellings`'s own "root unreachable"
    // signal out of this scenario, isolating the one this test targets.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-watch-vanished-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("known.jpg"), b"known").unwrap();
    let stored_root = onecopy_lib::winpath::for_fs(&root).into_owned();
    let roots = [stored_root.to_string_lossy().into_owned()];
    restat_dir(&conn, &sub, &lists(), &roots, &no_data_root()).unwrap();

    std::fs::remove_dir_all(&sub).unwrap();
    let changed = restat_dir(&conn, &sub, &lists(), &roots, &no_data_root())
        .expect("a vanished directory is not a walk failure");
    assert_eq!(changed, 1, "the one known row under it is marked missing");
    let missing: i64 = conn
        .query_row("SELECT missing FROM paths", [], |row| row.get(0))
        .unwrap();
    assert_eq!(missing, 1);
    let issues: i64 = conn
        .query_row("SELECT COUNT(*) FROM active_issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(issues, 0, "an expected condition raises no walk-error Issue");

    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("known.jpg"), b"known").unwrap();
    assert_eq!(restat_dir(&conn, &sub, &lists(), &roots, &no_data_root()).unwrap(), 1);
    let state: (i64, i64) = conn
        .query_row(
            "SELECT (SELECT missing FROM paths), (SELECT COUNT(*) FROM active_issues)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, (0, 0), "the file's return is seen on the next pass");
}

// A genuinely unreadable-but-present directory (permission denied) cannot
// prove absence the way a vanished one can, so it keeps the original
// contract: an `Err`, no missing rows, and a recheckable walk-error Issue.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_never_turns_known_files_into_missing_rows() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-watch-unreadable-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("known.jpg"), b"known").unwrap();
    let stored_root = onecopy_lib::winpath::for_fs(&root).into_owned();
    restat_dir(&conn, &root, &lists(), &[stored_root.to_string_lossy().into_owned()], &no_data_root()).unwrap();

    let original = std::fs::metadata(&root).unwrap().permissions();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = restat_dir(&conn, &root, &lists(), &[stored_root.to_string_lossy().into_owned()], &no_data_root());
    // Root test runners can read past the mode bits; skip rather than assert
    // a false failure in that environment.
    if result.is_ok() {
        std::fs::set_permissions(&root, original).unwrap();
        return;
    }

    let missing: i64 = conn
        .query_row("SELECT missing FROM paths", [], |row| row.get(0))
        .unwrap();
    assert_eq!(missing, 0, "failed enumeration proves nothing about absence");
    let issues: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM active_issues WHERE kind = 'walk-error' AND path = ?1",
            [onecopy_lib::winpath::for_fs(&root).to_string_lossy().as_ref()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(issues, 1, "the failure must remain visible and recheckable");

    std::fs::set_permissions(&root, original).unwrap();
    assert_eq!(restat_dir(&conn, &root, &lists(), &[stored_root.to_string_lossy().into_owned()], &no_data_root()).unwrap(), 0);
    let state: (i64, i64) = conn
        .query_row(
            "SELECT (SELECT missing FROM paths), (SELECT COUNT(*) FROM active_issues)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, (0, 0), "success retires the current condition");
}

#[cfg(windows)]
#[test]
fn restat_uses_the_same_windows_spelling_as_a_full_scan() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-watch-winpath-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("new.jpg"), b"fresh").unwrap();

    let stored_root = onecopy_lib::winpath::for_fs(&root).into_owned();
    onecopy_lib::scanner::walk_root(&conn, &stored_root, &lists()).unwrap();
    assert_eq!(restat_dir(&conn, &root, &lists(), &[root.to_string_lossy().into_owned()], &no_data_root()).unwrap(), 0);

    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "the watcher must not fork the full-scan row");
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
        .join(onecopy_lib::trash::TRASH_DIR_NAME)
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
    let (dirty, overflowed) = fold(vec![dir.path().join(".onecopy-trash")]);
    assert!(!overflowed);
    assert!(dirty.is_empty());

    let lookalike = dir.path().join(".onecopy-trash-notes");
    std::fs::create_dir(&lookalike).unwrap();
    let file = lookalike.join("photo.jpg");
    std::fs::write(&file, b"ordinary").unwrap();
    let (dirty, _) = fold(vec![file]);
    assert_eq!(dirty, HashSet::from([lookalike]));
}

#[test]
fn restat_keeps_trash_lookalikes_but_never_opens_deleted_storage() {
    let dir = tempfile::tempdir().unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let lookalike = dir.path().join(".onecopy-trash-notes");
    std::fs::create_dir(&lookalike).unwrap();
    std::fs::write(lookalike.join(".onecopy-trash.jpg"), b"ordinary").unwrap();
    assert_eq!(restat_dir(&conn, &lookalike, &lists(), &[lookalike.to_string_lossy().into_owned()], &no_data_root()).unwrap(), 1);
    // Intentionally absent: an excluded location needs no enumeration and
    // must not produce an inaccessible-directory Issue.
    let excluded = lookalike.join(".onecopy-trash").join("day");
    assert_eq!(restat_dir(&conn, &excluded, &lists(), &[excluded.to_string_lossy().into_owned()], &no_data_root()).unwrap(), 0);
    let counts: (i64, i64) = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM paths), (SELECT COUNT(*) FROM active_issues)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(counts, (1, 0));
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

// (W-L4) The channel between `notify`'s callback and the drain loop is
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


// (R7-02) A vanished directory marks its rows missing through the
// projection-batch publisher, page by page, never through the per-row
// projection trigger that holds the write lock for the whole subtree.
#[test]
fn a_vanished_directory_is_marked_missing_through_the_batch_publisher() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-watch-vanished-batch-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    for index in 0..600 {
        std::fs::write(sub.join(format!("{index}.jpg")), b"known").unwrap();
    }
    let stored_root = onecopy_lib::winpath::for_fs(&root).into_owned();
    let roots = [stored_root.to_string_lossy().into_owned()];
    restat_dir(&conn, &sub, &lists(), &roots, &no_data_root()).unwrap();
    conn.execute_batch(
        "CREATE TEMP TABLE unguarded_path_updates (id INTEGER);
         CREATE TEMP TRIGGER count_unguarded_path_updates AFTER UPDATE ON main.paths
         WHEN NOT EXISTS (SELECT 1 FROM main.logical_projection_batch)
         BEGIN INSERT INTO unguarded_path_updates VALUES (NEW.id); END;",
    )
    .unwrap();

    std::fs::remove_dir_all(&sub).unwrap();
    assert_eq!(restat_dir(&conn, &sub, &lists(), &roots, &no_data_root()).unwrap(), 600);
    let (missing, unguarded): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM paths WHERE missing = 1),
                    (SELECT COUNT(*) FROM unguarded_path_updates)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(missing, 600);
    assert_eq!(unguarded, 0, "every row was published under the batch guard");
}

// Each root is watched in its own bounded registration: a root whose drive
// does not answer fails within the bound and never keeps another unwatched.
#[test]
fn a_root_that_does_not_answer_never_keeps_another_root_unwatched() {
    use onecopy_lib::volume_io::{FakeStallingVolume, Op};
    let stalled = tempfile::tempdir().unwrap();
    let healthy = tempfile::tempdir().unwrap();
    let volume = FakeStallingVolume::mount(stalled.path(), std::time::Duration::from_millis(300));
    volume.stall(&[Op::Watch], None);

    let started = std::time::Instant::now();
    let refused = watch_root(stalled.path(), |_event: notify::Result<notify::Event>| {});
    assert!(refused.is_err_and(|error| onecopy_lib::volume_io::is_not_responding(&error)));
    let (events, received) = std::sync::mpsc::channel();
    let _watching = watch_root(healthy.path(), move |event| {
        let _ = events.send(event);
    })
    .expect("the healthy root is watched");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));

    std::fs::write(healthy.path().join("new.jpg"), b"x").unwrap();
    assert!(received
        .recv_timeout(std::time::Duration::from_secs(10))
        .is_ok());
    volume.release();
    assert!(volume.wait_until_settled(std::time::Duration::from_secs(5)));
}
