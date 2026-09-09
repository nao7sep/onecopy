use onecopy_lib::{attempt_boundaries, index_store, information_attempts, queries};

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
fn archived_failures_do_not_reopen_work() {
    let (_root, conn) = db();
    conn.execute_batch("INSERT INTO contents(hash, byte_size, kind, derived_at_utc) VALUES ('hash', 4, 'image', 'failed');
        INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash, hash_attempt_failed) VALUES ('/a.jpg', '/', 'a.jpg', 'image', 'hash', 1);").unwrap();
    for kind in [
        "decode-error",
        "resource-limit-preparation",
        onecopy_lib::derived_work::WORKER_FAILED,
    ] {
        index_store::upsert_issue(&conn, Some("/a.jpg"), kind, "failed").unwrap();
    }
    index_store::dismiss_issues(&conn, None).unwrap();
    assert_eq!(queries::issues(&conn, 10).unwrap().0, 0);
    assert_eq!(
        conn.query_row("SELECT derived_at_utc FROM contents", [], |row| row
            .get::<_, String>(0))
            .unwrap(),
        "failed"
    );
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

#[test]
fn only_startup_admission_begins_a_new_run_and_keeps_all_history() {
    let (root, conn) = db();
    index_store::upsert_issue(&conn, Some("/a.jpg"), "decode-error", "failed").unwrap();
    let old_id = queries::issues(&conn, 10).unwrap().1[0].id;
    drop(conn);
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    assert_eq!(
        queries::issues(&conn, 10).unwrap().0,
        1,
        "connection opens are not app starts"
    );
    attempt_boundaries::begin_run(&conn).unwrap();
    assert_eq!(queries::issues(&conn, 10).unwrap().0, 0);
    assert_eq!(
        conn.query_row(
            "SELECT closure FROM issues WHERE id = ?1",
            [old_id],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "app-restart"
    );
    index_store::upsert_issue(&conn, Some("/a.jpg"), "decode-error", "failed again").unwrap();
    assert_ne!(queries::issues(&conn, 10).unwrap().1[0].id, old_id);
}

fn section_fixture(conn: &rusqlite::Connection) {
    conn.execute_batch("INSERT INTO contents(hash, byte_size, kind, derived_at_utc) VALUES
        ('photo', 4, 'image', 'failed'), ('next-month', 4, 'image', 'failed'), ('video', 4, 'video', 'failed');
        INSERT INTO paths(id, abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms, resolved_source, metadata_attempt_failed) VALUES
        (1, '/same/photo.jpg', '/same', 'photo.jpg', 'image', 'photo', 10, 'filesystem', 1),
        (2, '/same/next.jpg', '/same', 'next.jpg', 'image', 'next-month', 100, 'filesystem', 1),
        (3, '/same/video.mp4', '/same', 'video.mp4', 'video', 'video', 10, 'filesystem', 1);
        INSERT INTO paths(id, abs_path, dir_path, file_name, kind, companion_of, metadata_attempt_failed) VALUES
        (4, '/same/photo.xmp', '/same', 'photo.xmp', 'companion', 1, 1);
        INSERT INTO paths(id, abs_path, dir_path, file_name, kind, resolved_utc_ms, resolved_source, hash_attempt_failed) VALUES
        (5, '/same/unidentified.jpg', '/same', 'unidentified.jpg', 'image', 10, 'filesystem', 1);").unwrap();
    for (path, kind) in [
        ("/same/photo.jpg", "decode-error"),
        ("/same/photo.xmp", "metadata-read-error"),
        ("/same/unidentified.jpg", "read-error"),
        ("/same/next.jpg", "decode-error"),
        ("/same/video.mp4", "transcription-error"),
        ("/same/photo.jpg", "delete-error"),
    ] {
        index_store::upsert_issue(conn, Some(path), kind, "failed").unwrap();
    }
}

#[test]
fn section_attempt_retires_only_its_preparation_failures_and_never_claims_repair() {
    let (_root, conn) = db();
    section_fixture(&conn);
    assert_eq!(
        attempt_boundaries::recheck_section(&conn, "image", Some((0, 100))).unwrap(),
        1
    );
    let live = queries::issues(&conn, 20).unwrap();
    assert_eq!(live.0, 3);
    assert!(live.1.iter().any(|row| row.kind == "delete-error"));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'rechecked'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        3
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM issues WHERE closure = 'resolved'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT SUM(metadata_attempt_failed + hash_attempt_failed) FROM paths",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    index_store::upsert_issue(
        &conn,
        Some("/same/photo.jpg"),
        "decode-error",
        "failed again",
    )
    .unwrap();
    let fresh = queries::issues(&conn, 20)
        .unwrap()
        .1
        .into_iter()
        .find(|row| row.path.as_deref() == Some("/same/photo.jpg") && row.kind == "decode-error")
        .unwrap();
    assert_eq!(fresh.occurrence_count, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        7
    );
}

#[test]
fn failed_attempt_admission_rolls_back_diagnostic_retirement_and_all_receipts() {
    let (_root, conn) = db();
    section_fixture(&conn);
    conn.execute_batch("CREATE TRIGGER reject_reset BEFORE UPDATE OF derived_at_utc ON contents BEGIN SELECT RAISE(ABORT, 'fixture reset failure'); END;").unwrap();
    assert!(attempt_boundaries::recheck_section(&conn, "image", Some((0, 100))).is_err());
    assert!(attempt_boundaries::begin_run(&conn).is_err());
    assert_eq!(queries::issues(&conn, 20).unwrap().0, 6);
    assert_eq!(
        conn.query_row(
            "SELECT SUM(metadata_attempt_failed + hash_attempt_failed) FROM paths",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        5
    );
}
