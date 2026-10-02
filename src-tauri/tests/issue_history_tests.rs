use onecopy_lib::{attempt_boundaries, index_store, information_attempts, queries};

fn db() -> (tempfile::TempDir, rusqlite::Connection) {
    let root = tempfile::tempdir().unwrap();
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    (root, conn)
}

/// How many Issue events of `event` the records hold.
fn events(conn: &rusqlite::Connection, event: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM records.issue_events WHERE event = ?1",
        [event],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn dismiss_keeps_the_record_and_a_new_attempt_never_revives_it() {
    let (root, conn) = db();
    index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "original").unwrap();
    index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "latest detail").unwrap();
    let original = queries::issues(&conn, 10, None).unwrap().1.remove(0);
    index_store::dismiss_issues(&conn, Some(original.id)).unwrap();
    assert_eq!(queries::issues(&conn, 10, None).unwrap().0, 0);
    assert!(!index_store::any_issues(&conn).unwrap());
    drop(conn);
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    assert_eq!(queries::issues(&conn, 10, None).unwrap().0, 0);
    index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "new attempt").unwrap();
    let new = queries::issues(&conn, 10, None).unwrap().1.remove(0);
    assert_ne!(new.id, original.id);
    assert_eq!(new.occurrence_count, 1);
    let history: Vec<(String, Option<String>)> = conn
        .prepare("SELECT event, message FROM records.issue_events WHERE path = '/a.jpg' ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        history,
        [
            ("occurred".to_string(), Some("original".to_string())),
            ("occurred".to_string(), Some("latest detail".to_string())),
            ("dismissed".to_string(), None),
            ("occurred".to_string(), Some("new attempt".to_string())),
        ]
    );
    assert_eq!(original.occurrence_count, 2);
    assert_eq!(original.message.as_deref(), Some("latest detail"));
    index_store::clear_issues(&conn, "/a.jpg", &["read-error"]).unwrap();
    index_store::dismiss_issues(&conn, None).unwrap();
    assert_eq!((events(&conn, "dismissed"), events(&conn, "resolved")), (1, 1));
}

#[test]
fn dismiss_all_covers_the_full_live_inbox_and_preserves_other_history() {
    let (_root, conn) = db();
    for n in 0..520 {
        index_store::upsert_issue(&conn, Some(&format!("/{n}.jpg")), "read-error", "failed")
            .unwrap();
    }
    conn.execute_batch("INSERT INTO recent_notifications(kind, level, presentation, message, first_seen_utc, last_seen_utc) VALUES ('notice', 'error', 'timed', 'retained', '2026-09-09T00:00:00.000Z', '2026-09-09T00:00:00.000Z');").unwrap();
    assert_eq!(queries::issues(&conn, 2, None).unwrap().1.len(), 2);
    index_store::dismiss_issues(&conn, None).unwrap();
    assert_eq!(queries::issues(&conn, 2, None).unwrap().0, 0);
    assert_eq!(events(&conn, "dismissed"), 520);
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
    assert_eq!(queries::issues(&conn, 10, None).unwrap().0, 0);
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
    assert_eq!(events(&conn, "occurred"), 3);
}

#[test]
fn failed_dismissal_leaves_live_diagnostics_visible() {
    let (_root, conn) = db();
    index_store::upsert_issue(&conn, None, "read-error", "failed").unwrap();
    conn.execute_batch("CREATE TRIGGER records.reject_close BEFORE INSERT ON issue_events WHEN NEW.event = 'dismissed' BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
    assert!(index_store::dismiss_issues(&conn, None).is_err());
    assert_eq!(queries::issues(&conn, 10, None).unwrap().0, 1);
}

#[test]
fn each_launch_starts_a_fresh_inbox_and_keeps_earlier_launches_issues() {
    let (root, conn) = db();
    conn.execute(
        "INSERT INTO records.issue_events (session_id, time_utc, kind, path, event, message)
         VALUES ('an earlier launch', '2026-10-01T00:00:00.000Z', 'decode-error', '/a.jpg', 'occurred', 'failed')",
        [],
    )
    .unwrap();
    assert_eq!(queries::issues(&conn, 10, None).unwrap().0, 0);
    index_store::upsert_issue(&conn, Some("/a.jpg"), "decode-error", "failed").unwrap();
    drop(conn);
    let conn = index_store::open(&root.path().join("index.sqlite3")).unwrap();
    assert_eq!(
        queries::issues(&conn, 10, None).unwrap().0,
        1,
        "connection opens are not app starts"
    );
    assert_eq!(events(&conn, "occurred"), 2);
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
        attempt_boundaries::recheck_section(&conn, onecopy_lib::queries::SectionKind::Image, Some((0, 100))).unwrap(),
        1
    );
    let live = queries::issues(&conn, 20, None).unwrap();
    assert_eq!(live.0, 3);
    assert!(live.1.iter().any(|row| row.kind == "delete-error"));
    assert_eq!((events(&conn, "rechecked"), events(&conn, "resolved")), (3, 0));
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
    let fresh = queries::issues(&conn, 20, None)
        .unwrap()
        .1
        .into_iter()
        .find(|row| row.path.as_deref() == Some("/same/photo.jpg") && row.kind == "decode-error")
        .unwrap();
    assert_eq!(fresh.occurrence_count, 1);
    assert_eq!(events(&conn, "occurred"), 7);
}

#[test]
fn upsert_and_clear_issue_report_whether_they_actually_changed_a_row() {
    let (_root, conn) = db();
    // A brand-new failure opens a live entry.
    assert!(index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "first").unwrap());
    // A repeated failure of the same already-open issue only bumps its
    // occurrence count and last-seen time; that is not a change the Issues
    // surface needs to reload for (C-M3).
    assert!(!index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "again").unwrap());
    assert!(!index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "again").unwrap());

    // Resolving the open issue reports the change.
    assert!(index_store::clear_issues(&conn, "/a.jpg", &["read-error"]).unwrap());
    // Clearing an already-closed (or never-open) issue is a no-op.
    assert!(!index_store::clear_issues(&conn, "/a.jpg", &["read-error"]).unwrap());
    assert!(!index_store::clear_issues(&conn, "/never-failed.jpg", &["read-error"]).unwrap());

    // A new failure after resolution reopens the entry as a fresh identity.
    assert!(index_store::upsert_issue(&conn, Some("/a.jpg"), "read-error", "new attempt").unwrap());
}

#[test]
fn failed_attempt_admission_rolls_back_diagnostic_retirement_and_all_receipts() {
    let (_root, conn) = db();
    section_fixture(&conn);
    conn.execute_batch("CREATE TRIGGER reject_reset BEFORE UPDATE OF derived_at_utc ON contents BEGIN SELECT RAISE(ABORT, 'fixture reset failure'); END;").unwrap();
    assert!(attempt_boundaries::recheck_section(&conn, onecopy_lib::queries::SectionKind::Image, Some((0, 100))).is_err());
    assert!(attempt_boundaries::begin_run(&conn).is_err());
    assert_eq!(queries::issues(&conn, 20, None).unwrap().0, 6);
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
