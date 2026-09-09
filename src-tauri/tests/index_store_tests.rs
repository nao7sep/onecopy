use onecopy_lib::index_store;

#[test]
fn revision_nine_upgrade_preserves_library_and_diagnostics_across_concurrent_openers() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc) VALUES ('kept', 4, 'image', 'ready');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES ('/kept.jpg', '/', 'kept.jpg', 'image', 'kept');
         INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc) VALUES ('/kept.jpg', 'read-error', 'diagnostic', '2026-09-09T00:00:00.000Z', '2026-09-09T00:00:00.000Z');
         INSERT INTO recent_notifications (kind, path, level, presentation, message, first_seen_utc, last_seen_utc)
         VALUES ('read-error', '/kept.jpg', 'error', 'persistent', 'retained', '2026-09-09T00:00:00.000Z', '2026-09-09T00:00:00.000Z');
         ALTER TABLE paths DROP COLUMN hash_attempt_failed;
         ALTER TABLE paths DROP COLUMN metadata_attempt_failed;
         PRAGMA user_version = 9;"
    ).unwrap();
    drop(conn);
    let start = std::sync::Arc::new(std::sync::Barrier::new(4));
    let workers = (0..4)
        .map(|_| {
            let db = db.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                drop(index_store::open(&db).unwrap());
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let conn = index_store::open(&db).unwrap();
    for table in [
        "contents",
        "paths",
        "logical_contents",
        "issues",
        "recent_notifications",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "{table}"
        );
    }
    assert_eq!(
        conn.query_row(
            "SELECT hash_attempt_failed + metadata_attempt_failed FROM paths",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT derived_at_utc FROM contents", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "ready"
    );
}

#[test]
fn newer_unknown_schema_is_not_destructively_downgraded() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = index_store::open(&db).unwrap();
    conn.execute_batch("INSERT INTO contents (hash, byte_size, kind) VALUES ('retained', 1, 'image'); PRAGMA user_version = 999;").unwrap();
    drop(conn);
    assert!(index_store::open(&db).is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM contents", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        999
    );
}

#[test]
fn rebuild_clears_reconstructible_library_facts_and_issues() {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('hash', 4, 'image');
         INSERT INTO paths
           (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES ('/photos/a.jpg', '/photos', 'a.jpg', 'image', 'hash', 0);
         INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc)
         VALUES ('/photos/a.jpg', 'read-error', 'failed', 'now', 'now');
         INSERT INTO recent_notifications
           (kind, path, level, presentation, message, first_seen_utc, last_seen_utc)
         VALUES ('read-error', '/photos/a.jpg', 'error', 'persistent', 'failed',
                 '2026-08-31T00:00:00.000Z', '2026-08-31T00:00:00.000Z');
         INSERT INTO scan_dirs (root, last_completed_at_utc)
         VALUES ('/photos', 'now');",
    )
    .unwrap();

    index_store::clear_reconstructible(&conn).unwrap();

    for table in [
        "contents",
        "paths",
        "logical_contents",
        "similar_groups",
        "similar_group_members",
        "similarity_dirty_buckets",
        "similarity_state",
        "issues",
        "recent_notifications",
        "scan_dirs",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}
