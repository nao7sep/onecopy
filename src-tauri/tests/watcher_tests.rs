// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::scanner::ScanLists;
use onecopy_lib::watcher::*;
use onecopy_lib::extensions;
use onecopy_lib::index_store;
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

#[test]
fn source_issues_coalesce_and_clear_on_return_or_configuration_removal() {
    let dir = tempfile::tempdir().unwrap();
    let roots = vec!["/offline/photos".to_string(), "/offline/videos".to_string()];
    for _ in 0..3 { reconcile_source_conditions(dir.path(), &roots, &roots, &[]).unwrap(); }
    let conn = index_store::open(&dir.path().join(onecopy_lib::storage::INDEX_DB_FILE_NAME)).unwrap();
    let count = || conn.query_row("SELECT count(*) FROM active_issues WHERE kind = 'source-unavailable'", [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count(), 2);
    reconcile_source_conditions(dir.path(), &roots, &roots[..1], &[]).unwrap();
    assert_eq!(count(), 1);
    reconcile_source_conditions(dir.path(), &roots, &[], &roots[..1]).unwrap();
    assert_eq!(count(), 1, "substitution remains one actionable condition");
    reconcile_source_conditions(dir.path(), &[], &[], &[]).unwrap();
    assert_eq!(count(), 0);
}

#[test]
fn a_folder_whose_drive_cannot_be_identified_keeps_one_issue_until_it_can() {
    let dir = tempfile::tempdir().unwrap();
    let roots = vec!["/Volumes/share/photos".to_string(), "/Users/me/Pictures".to_string()];
    for _ in 0..2 { onecopy_lib::watcher::reconcile_unidentified_sources(dir.path(), &roots, &roots[..1]).unwrap(); }
    let conn = index_store::open(&dir.path().join(onecopy_lib::storage::INDEX_DB_FILE_NAME)).unwrap();
    let count = || conn.query_row("SELECT count(*) FROM active_issues WHERE kind = 'source-identity-unavailable'", [], |row| row.get::<_, i64>(0)).unwrap();
    assert_eq!(count(), 1);
    onecopy_lib::watcher::reconcile_unidentified_sources(dir.path(), &roots, &[]).unwrap();
    assert_eq!(count(), 0);
}

#[test]
fn healthy_directory_updates_do_not_reopen_walk_issues_for_an_offline_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("healthy");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("photo.jpg"), b"photo").unwrap();
    let offline = dir.path().join("offline");
    let roots = vec![root.to_string_lossy().into_owned(), offline.to_string_lossy().into_owned()];
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for _ in 0..2 { restat_dir(&conn, &root, &lists(), &roots, &no_data_root()).unwrap(); }
    assert_eq!(conn.query_row("SELECT count(*) FROM active_issues", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}
