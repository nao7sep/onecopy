use onecopy_lib::index_store;

fn revision_twelve_fixture(path: &std::path::Path) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.create_collation("onecopy_nocase", |left, right| left.to_lowercase().cmp(&right.to_lowercase())).unwrap();
    conn.execute_batch(include_str!("fixtures/index-v12.sql")).unwrap();
    conn
}

#[test]
fn revision_twelve_visibility_upgrade_preserves_history_caches_and_copy_evidence() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    conn.execute_batch("INSERT INTO contents (hash, byte_size, kind, derived_at_utc) VALUES ('photo', 3, 'image', 'ready');
        INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, resolved_source, resolved_utc_ms)
          VALUES ('/root/a.jpg', '/root', 'a.jpg', 'image', 'photo', 'filename', 1000),
                 ('/root/b.jpg', '/root', 'b.jpg', 'image', 'photo', 'filename', 2000);").unwrap();
    index_store::upsert_issue(&conn, Some("/root/a.jpg"), "read-error", "retained").unwrap();
    index_store::dismiss_issues(&conn, None).unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    assert_eq!(conn.query_row("SELECT live_copy_count, visible_copy_count, resolved_utc_ms FROM logical_contents", [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))).unwrap(), (2, 2, 1000));
    assert_eq!(conn.query_row("SELECT derived_at_utc FROM contents", [], |row| row.get::<_, String>(0)).unwrap(), "ready");
    assert_eq!(conn.query_row("SELECT closure FROM issues", [], |row| row.get::<_, String>(0)).unwrap(), "dismissed");
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM paths WHERE visibility_checked = 0", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
}

fn restore_legacy_issue_shape(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "DROP VIEW active_issues;
         ALTER TABLE issues RENAME TO fixture_history_shape;
         DROP INDEX idx_issues_live_identity;
         DROP INDEX idx_issues_first_seen;
         CREATE TABLE issues (
           id INTEGER PRIMARY KEY, path TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL, message TEXT,
           first_seen_utc TEXT NOT NULL, last_seen_utc TEXT NOT NULL,
           occurrence_count INTEGER NOT NULL DEFAULT 1, UNIQUE(kind, path)
         );
         INSERT INTO issues SELECT id, path, kind, message, first_seen_utc, last_seen_utc, occurrence_count FROM fixture_history_shape;
         DROP TABLE fixture_history_shape;
         CREATE INDEX idx_issues_first_seen ON issues(first_seen_utc, id);"
    ).unwrap();
}

#[test]
fn revision_nine_upgrade_preserves_library_and_diagnostics_across_concurrent_openers() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
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
    restore_legacy_issue_shape(&conn);
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
fn revision_ten_upgrade_retains_issue_identity_and_allows_new_occurrences_after_dismissal() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    index_store::upsert_issue(&conn, Some("/photo.jpg"), "read-error", "retained detail").unwrap();
    index_store::upsert_issue(&conn, Some("/photo.jpg"), "read-error", "retained detail").unwrap();
    restore_legacy_issue_shape(&conn);
    let before: (i64, String, String, i64) = conn
        .query_row(
            "SELECT id, first_seen_utc, last_seen_utc, occurrence_count FROM issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    conn.pragma_update(None, "user_version", 10).unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    let after = conn
        .query_row(
            "SELECT id, first_seen_utc, last_seen_utc, occurrence_count FROM active_issues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(before, after);
    index_store::dismiss_issues(&conn, Some(before.0)).unwrap();
    index_store::upsert_issue(&conn, Some("/photo.jpg"), "read-error", "new attempt").unwrap();
    assert_eq!(onecopy_lib::queries::issues(&conn, 10).unwrap().0, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn failed_history_upgrade_rolls_back_the_whole_migration() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    index_store::upsert_issue(&conn, None, "read-error", "retained").unwrap();
    restore_legacy_issue_shape(&conn);
    // A malformed old schema fails after the rename, while copying records.
    conn.execute_batch(
        "ALTER TABLE issues DROP COLUMN message;
        ALTER TABLE paths DROP COLUMN hash_attempt_failed;
        ALTER TABLE paths DROP COLUMN metadata_attempt_failed;
        PRAGMA user_version = 9;",
    )
    .unwrap();
    drop(conn);
    assert!(index_store::open(&db).is_err());
    let conn = rusqlite::Connection::open(&db).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('paths') WHERE name = 'hash_attempt_failed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'issues_before_history'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn revision_eleven_preserves_archived_closures_and_accepts_new_attempt_reasons() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("index.sqlite3");
    let conn = revision_twelve_fixture(&db);
    index_store::upsert_issue(&conn, None, "test", "dismissed detail").unwrap();
    index_store::dismiss_issues(&conn, None).unwrap();
    index_store::upsert_issue(&conn, None, "test", "live detail").unwrap();
    let old: (i64, String, String) = conn
        .query_row(
            "SELECT id, closed_at_utc, closure FROM issues WHERE closure IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    conn.execute_batch("PRAGMA user_version = 11;").unwrap();
    drop(conn);
    let conn = index_store::open(&db).unwrap();
    let kept = conn
        .query_row(
            "SELECT id, closed_at_utc, closure FROM issues WHERE closure IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(old, kept);
    index_store::begin_issue_run(&conn).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'app-restart'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'dismissed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
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
