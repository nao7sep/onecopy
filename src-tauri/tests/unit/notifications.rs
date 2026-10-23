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
    let mut active = ActiveState::default();
    let (first, first_snapshot) = active.remember(record_notice(&conn, &request).unwrap());
    let (second, second_snapshot) = active.remember(record_notice(&conn, &request).unwrap());
    assert_eq!(first_snapshot.revision, 1);
    assert_eq!(second_snapshot.revision, 2);
    assert_eq!(first_snapshot.active, vec![first.clone()]);
    assert_eq!(second_snapshot.active, vec![second.clone()]);
    assert_eq!(first.id, second.id);
    assert_eq!(second.occurrence_count, 2);
    assert_eq!(second.first_seen_utc, first.first_seen_utc);
    assert_eq!(notices(&conn), 2);
    let issues = crate::queries::issues(&conn, 10, None).unwrap();
    assert_eq!(issues.0, 1);
    assert_eq!(issues.1[0].occurrence_count, 2, "one occurrence per publication, not one per presentation channel");
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

#[test]
fn live_snapshots_capture_coherent_transitions_without_retaining_dismissed_records() {
    let directory = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
    let request = NotificationRequest {
        kind: "first".into(), path: None, level: NotificationLevel::Info,
        presentation: NotificationPresentation::Timed, message: "Done.".into(),
        message_key: None, message_values: None,
    };
    let mut active = ActiveState::default();
    assert_eq!(active.snapshot().revision, 0);
    let (first, published) = active.remember(record_notice(&conn, &request).unwrap());
    let mut different = request.clone();
    different.kind = "second".into();
    let (second, both) = active.remember(record_notice(&conn, &different).unwrap());
    assert_eq!(published.revision, 1);
    assert_eq!(both.revision, 2);
    assert_eq!(both.active, vec![first.clone(), second.clone()]);
    let dismissed = active.dismiss(first.id);
    assert_eq!(dismissed.revision, 3);
    assert_eq!(dismissed.active, vec![second]);
    assert_eq!(active.dismiss(first.id), dismissed, "missing dismissal returns current snapshot");
    let cleared = active.clear();
    assert_eq!(cleared.revision, 4);
    assert!(cleared.active.is_empty());
    assert!(active.records.is_empty());
    assert_eq!(published.active, vec![first], "captured snapshots remain independent");
    let (republished, snapshot) = active.remember(record_notice(&conn, &request).unwrap());
    assert_eq!(snapshot.revision, 5);
    assert_eq!(snapshot.active, vec![republished.clone()]);
    assert_eq!(republished.occurrence_count, 1);
    assert_eq!(notices(&conn), 3, "live dismissal and clear preserve history");
}
