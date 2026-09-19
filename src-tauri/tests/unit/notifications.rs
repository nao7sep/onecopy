use super::*;

#[test]
fn repeated_recent_notice_coalesces_with_times_and_count() {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    let request = NotificationRequest {
        kind: "read-failed".to_string(),
        path: Some("/photos/a.jpg".to_string()),
        level: NotificationLevel::Error,
        presentation: NotificationPresentation::Persistent,
        message: "Could not read the file.".to_string(),
    };
    let first = record_recent(&conn, &request).unwrap();
    let second = record_recent(&conn, &request).unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.occurrence_count, 2);
    assert_eq!(recent(&conn, 500).unwrap().0, 1);
    let issues = crate::queries::issues(&conn, 10).unwrap();
    assert_eq!(issues.0, 1);
    assert_eq!(issues.1[0].occurrence_count, 2, "one occurrence per publication, not one per presentation channel");
}

#[test]
fn informational_notices_are_not_issues_and_dismissal_does_not_erase_history() {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    let mut request = NotificationRequest {
        kind: "operation".into(), path: None, level: NotificationLevel::Info,
        presentation: NotificationPresentation::Timed, message: "Source folders checked.".into(),
    };
    record_recent(&conn, &request).unwrap();
    assert_eq!(crate::queries::issues(&conn, 10).unwrap().0, 0);
    request.level = NotificationLevel::Warning;
    request.message = "Some files could not be checked.".into();
    record_recent(&conn, &request).unwrap();
    let old_id = crate::queries::issues(&conn, 10).unwrap().1[0].id;
    crate::index_store::dismiss_issues(&conn, None).unwrap();
    record_recent(&conn, &request).unwrap();
    let issues = crate::queries::issues(&conn, 10).unwrap();
    assert_ne!(issues.1[0].id, old_id);
    assert_eq!(issues.1[0].occurrence_count, 1);
    assert_eq!(recent(&conn, 10).unwrap().0, 2);
}

#[test]
fn notification_and_issue_recording_fail_as_one_transaction() {
    for blocked in ["issues", "recent_notifications"] {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
        conn.execute_batch(&format!("CREATE TRIGGER reject_insert BEFORE INSERT ON {blocked} BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;")).unwrap();
        assert!(record_recent(&conn, &NotificationRequest {
            kind: "failed".into(), path: None, level: NotificationLevel::Error,
            presentation: NotificationPresentation::Persistent, message: "Failed action.".into(),
        }).is_err());
        assert_eq!(crate::queries::issues(&conn, 10).unwrap().0, 0);
        assert_eq!(recent(&conn, 10).unwrap().0, 0);
    }
}

#[test]
fn recent_history_prunes_old_and_over_limit_rows_after_a_write() {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    let now = crate::logging::now_iso_millis();
    let transaction = conn.unchecked_transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO recent_notifications
               (kind, path, level, presentation, message, first_seen_utc,
                last_seen_utc, occurrence_count)
             VALUES ('old', '', 'warning', 'timed', 'old',
                     '2000-01-01T00:00:00.000Z', '2000-01-01T00:00:00.000Z', 1)",
            [],
        )
        .unwrap();
    for index in 0..500 {
        transaction
            .execute(
                "INSERT INTO recent_notifications
                   (kind, path, level, presentation, message, first_seen_utc,
                    last_seen_utc, occurrence_count)
                 VALUES (?1, '', 'info', 'timed', ?1,
                         ?2, ?2, 1)",
                params![format!("notice-{index}"), now],
            )
            .unwrap();
    }
    transaction.commit().unwrap();

    record_recent(
        &conn,
        &NotificationRequest {
            kind: "newest".to_string(),
            path: None,
            level: NotificationLevel::Info,
            presentation: NotificationPresentation::Timed,
            message: "newest".to_string(),
        },
    )
    .unwrap();

    assert_eq!(recent(&conn, 500).unwrap().0, 500);
    let old: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM recent_notifications WHERE kind = 'old'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old, 0);
}
