use onecopy_lib::{derived_state, index_store, information_attempts, issue_recovery, queries};

fn db() -> (tempfile::TempDir, rusqlite::Connection) {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    (root, conn)
}

#[test]
fn dismiss_keeps_the_record_and_a_new_attempt_never_revives_it() {
    let (root, conn) = db();
    index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "original").unwrap();
    index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "latest detail").unwrap();
    let original = queries::issues(&conn, 10).unwrap().1.remove(0);
    index_store::dismiss_issues(&conn, Some(original.id)).unwrap();
    assert_eq!(queries::issues(&conn, 10).unwrap().0, 0);
    assert!(!index_store::any_issues(&conn).unwrap());
    drop(conn);
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    assert_eq!(queries::issues(&conn, 10).unwrap().0, 0);
    index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "new attempt").unwrap();
    let new = queries::issues(&conn, 10).unwrap().1.remove(0);
    assert_ne!(new.id, original.id);
    assert_eq!(new.occurrence_count, 1);
    let retained: (String, String, String, i64, String, String) = conn.query_row(
        "SELECT message, first_seen_utc, last_seen_utc, occurrence_count, closure, closed_at_utc FROM issues WHERE id = ?1",
        [original.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
    ).unwrap();
    assert_eq!(retained.0, "latest detail");
    assert_eq!(retained.1, original.first_seen_utc);
    assert_eq!(retained.2, original.last_seen_utc);
    assert_eq!(retained.3, 2);
    assert_eq!(retained.4, "dismissed");
    assert!(retained.5.ends_with('Z'));
    index_store::clear_issues(&conn, "/a.jpg", &["read-error"]).unwrap();
    index_store::dismiss_issues(&conn, None).unwrap();
    let closure: (String, String) = conn
        .query_row(
            "SELECT closure, closed_at_utc FROM issues WHERE id = ?1",
            [original.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(closure, (retained.4, retained.5));
    assert_eq!(
        conn.query_row(
            "SELECT closure FROM issues WHERE id = ?1",
            [new.id],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "resolved"
    );
}

#[test]
fn dismiss_all_covers_the_full_live_inbox_and_preserves_other_history() {
    let (_root, conn) = db();
    for n in 0..520 {
        index_store::upsert_issue(&conn, Some(&format!("/{n}.jpg")), "read-error", "failed")
            .unwrap();
    }
    conn.execute_batch("INSERT INTO recent_notifications(kind, level, presentation, message, first_seen_utc, last_seen_utc) VALUES ('notice', 'error', 'timed', 'retained', '2026-09-09T00:00:00.000Z', '2026-09-09T00:00:00.000Z');").unwrap();
    assert_eq!(queries::issues(&conn, 2).unwrap().1.len(), 2);
    index_store::dismiss_issues(&conn, None).unwrap();
    assert_eq!(queries::issues(&conn, 2).unwrap().0, 0);
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'dismissed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        520
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM recent_notifications", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
}

#[test]
fn archived_failures_do_not_supply_recovery_actions_or_reopen_work() {
    let (_root, conn) = db();
    conn.execute_batch("INSERT INTO contents(hash, byte_size, kind, derived_at_utc) VALUES ('hash', 4, 'image', 'failed');
        INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash, hash_attempt_failed) VALUES ('/a.jpg', '/', 'a.jpg', 'image', 'hash', 1);").unwrap();
    for kind in [
        "decode-error",
        "resource-limit-preparation",
        issue_recovery::DERIVED_WORKER_FAILED,
    ] {
        index_store::upsert_issue(&conn, Some("/a.jpg"), kind, "failed").unwrap();
    }
    let rows = queries::issues(&conn, 10).unwrap().1;
    index_store::dismiss_issues(&conn, None).unwrap();
    for row in rows {
        assert!(!issue_recovery::issue_has_kind(&conn, row.id, &row.kind).unwrap());
        assert!(!derived_state::retry_issue(&conn, row.id).unwrap());
    }
    assert!(!issue_recovery::contains_kind(&conn, issue_recovery::DERIVED_WORKER_FAILED).unwrap());
    assert_eq!(
        conn.query_row("SELECT hash_attempt_failed FROM paths", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(information_attempts::reset_library(&conn).unwrap(), 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn failed_dismissal_leaves_live_diagnostics_visible() {
    let (_root, conn) = db();
    index_store::upsert_issue(&conn, None, "read-error", "failed").unwrap();
    conn.execute_batch("CREATE TRIGGER reject_close BEFORE UPDATE OF closed_at_utc ON issues BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
    assert!(index_store::dismiss_issues(&conn, None).is_err());
    assert_eq!(queries::issues(&conn, 10).unwrap().0, 1);
}
