use super::*;

fn notices(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM records.notices", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_repeated_live_notice_coalesces_and_every_occurrence_is_a_record() {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    let request = NotificationRequest {
        kind: "read-failed-coalesce-test".to_string(),
        path: Some("/photos/a.jpg".to_string()),
        level: NotificationLevel::Error,
        presentation: NotificationPresentation::Persistent,
        message: "Could not read the file.".to_string(),
        message_key: None,
        message_values: None,
    };
    let first = remember_active(record_notice(&conn, &request).unwrap());
    let second = remember_active(record_notice(&conn, &request).unwrap());
    assert_eq!(first.id, second.id);
    assert_eq!(second.occurrence_count, 2);
    assert_eq!(second.first_seen_utc, first.first_seen_utc);
    assert_eq!(notices(&conn), 2);
    let issues = crate::queries::issues(&conn, 10, None).unwrap();
    assert_eq!(issues.0, 1);
    assert_eq!(issues.1[0].occurrence_count, 2, "one occurrence per publication, not one per presentation channel");
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|record| record.kind != request.kind);
}

#[test]
fn informational_notices_are_not_issues_and_dismissal_does_not_erase_records() {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    let mut request = NotificationRequest {
        kind: "operation".into(), path: None, level: NotificationLevel::Info,
        presentation: NotificationPresentation::Timed, message: "Source folders checked.".into(),
        message_key: None, message_values: None,
    };
    record_notice(&conn, &request).unwrap();
    assert_eq!(crate::queries::issues(&conn, 10, None).unwrap().0, 0);
    request.level = NotificationLevel::Warning;
    request.message = "Some files could not be checked.".into();
    record_notice(&conn, &request).unwrap();
    let old_id = crate::queries::issues(&conn, 10, None).unwrap().1[0].id;
    crate::index_store::dismiss_issues(&conn, None).unwrap();
    record_notice(&conn, &request).unwrap();
    let issues = crate::queries::issues(&conn, 10, None).unwrap();
    assert_ne!(issues.1[0].id, old_id);
    assert_eq!(issues.1[0].occurrence_count, 1);
    assert_eq!(notices(&conn), 3);
}

#[test]
fn notification_and_issue_recording_fail_as_one_transaction() {
    for blocked in ["issue_events", "notices"] {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
        conn.execute_batch(&format!("CREATE TRIGGER records.reject_insert BEFORE INSERT ON {blocked} BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;")).unwrap();
        assert!(record_notice(&conn, &NotificationRequest {
            kind: "failed".into(), path: None, level: NotificationLevel::Error,
            presentation: NotificationPresentation::Persistent, message: "Failed action.".into(),
            message_key: None, message_values: None,
        }).is_err());
        assert_eq!(crate::queries::issues(&conn, 10, None).unwrap().0, 0);
        assert_eq!(notices(&conn), 0);
    }
}
