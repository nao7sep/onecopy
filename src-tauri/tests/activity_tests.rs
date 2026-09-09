use onecopy_lib::activity::{
    ActivityDraft, ActivityKind, ActivityOwner, ActivityReason, ActivityRecorder, ActivityState,
    ActivitySubject,
};

fn draft(operation_id: Option<&str>) -> ActivityDraft {
    ActivityDraft {
        kind: ActivityKind::Started,
        owner: ActivityOwner::Section,
        subject: None,
        operation_id: operation_id.map(str::to_string),
        cause_id: None,
        generation: Some(2),
        previous: Some(ActivityState::Idle),
        current: Some(ActivityState::Running),
        reason: Some(ActivityReason::User),
        lane: None,
        item_count: Some(4),
        queued: None,
        done: None,
        total: None,
        target_hash: None,
    }
}

fn recorder(session: &str) -> (tempfile::TempDir, ActivityRecorder) {
    let temp = tempfile::tempdir().unwrap();
    let recorder =
        ActivityRecorder::new(session.to_string(), temp.path().join("activity.sqlite3")).unwrap();
    (temp, recorder)
}

#[test]
fn recorder_assigns_one_session_and_monotonic_sequence() {
    let (_temp, recorder) = recorder("session-one");
    let first = recorder
        .record_at(
            draft(Some("section:1")),
            "2026-09-07T00:00:00.000Z".to_string(),
            10,
        )
        .unwrap();
    let second = recorder
        .record_at(
            draft(Some("section:2")),
            "2026-09-07T00:00:00.010Z".to_string(),
            20,
        )
        .unwrap();

    assert_eq!(first.session_id, "session-one");
    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    let (events, cursor) = recorder.page(None, 10).unwrap();
    assert_eq!(events, vec![second, first]);
    assert_eq!(cursor, None);
}

#[test]
fn recorder_persists_every_event_and_pages_with_a_stable_cursor() {
    let (_temp, recorder) = recorder("session-one");
    for sequence in 1..=3 {
        recorder
            .record_at(
                draft(Some(&format!("section:{sequence}"))),
                "2026-09-07T00:00:00.000Z".to_string(),
                sequence,
            )
            .unwrap();
    }
    let (newest, cursor) = recorder.page(None, 2).unwrap();
    assert_eq!(
        newest
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![3, 2]
    );
    let (older, end) = recorder.page(cursor, 2).unwrap();
    assert_eq!(
        older.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(end, None);
}

#[test]
fn recorder_rejects_free_form_identifiers_and_invalid_progress() {
    let (_temp, recorder) = recorder("session-one");
    assert!(recorder
        .record_at(
            draft(Some("/Users/person/private.jpg")),
            "2026-09-07T00:00:00.000Z".to_string(),
            0,
        )
        .is_err());

    let mut invalid_progress = draft(None);
    invalid_progress.done = Some(3);
    invalid_progress.total = Some(2);
    assert!(recorder
        .record_at(invalid_progress, "2026-09-07T00:00:00.000Z".to_string(), 0,)
        .is_err());
}

#[test]
fn serialized_events_omit_absent_optional_fields() {
    let (_temp, recorder) = recorder("session-one");
    let mut minimal = draft(None);
    minimal.generation = None;
    minimal.previous = None;
    minimal.reason = None;
    minimal.item_count = None;
    let event = recorder
        .record_at(minimal, "2026-09-07T00:00:00.000Z".to_string(), 10)
        .unwrap();

    let value = serde_json::to_value(event).unwrap();
    assert_eq!(value["current"], "running");
    for absent in [
        "operationId",
        "subject",
        "causeId",
        "generation",
        "previous",
        "reason",
        "lane",
        "itemCount",
        "queued",
        "done",
        "total",
    ] {
        assert!(value.get(absent).is_none(), "{absent} should be omitted");
    }
}

#[test]
fn background_subjects_are_closed_and_human_readable() {
    let (_temp, recorder) = recorder("session-one");
    let mut event = draft(Some("backgroundWork:one"));
    event.owner = ActivityOwner::BackgroundWork;
    event.subject = Some(ActivitySubject::VideoTranscription);
    let value = serde_json::to_value(
        recorder
            .record_at(event, "2026-09-07T00:00:00.000Z".to_string(), 10)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["subject"], "videoTranscription");
}

#[test]
fn concrete_timing_owners_serialize_without_free_form_payloads() {
    let (_temp, recorder) = recorder("session-one");
    for (owner, expected) in [
        (ActivityOwner::Anchor, "anchor"),
        (ActivityOwner::ManagedTools, "managedTools"),
        (ActivityOwner::Media, "media"),
        (ActivityOwner::Watcher, "watcher"),
        (ActivityOwner::Identity, "identity"),
        (ActivityOwner::Delivery, "delivery"),
    ] {
        let mut event = draft(None);
        event.owner = owner;
        let value = serde_json::to_value(
            recorder
                .record_at(event, "2026-09-07T00:00:00.000Z".to_string(), 10)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(value["owner"], expected);
        assert!(value.get("path").is_none());
        assert!(value.get("payload").is_none());
    }
}

#[test]
fn operation_pages_keep_start_and_duration_outside_the_latest_raw_slice() {
    let (_temp, recorder) = recorder("one");
    recorder
        .record_at(draft(Some("long")), "2026-09-07T00:00:00.000Z".into(), 10)
        .unwrap();
    for i in 1..=250 {
        let mut progress = draft(Some("long"));
        progress.kind = ActivityKind::Progressed;
        progress.done = Some(i);
        progress.total = Some(250);
        recorder
            .record_at(progress, "2026-09-07T00:00:01.000Z".into(), i + 10)
            .unwrap();
    }
    recorder
        .record_at(draft(Some("new")), "2026-09-07T00:00:02.000Z".into(), 300)
        .unwrap();
    let mut terminal = draft(Some("long"));
    terminal.kind = ActivityKind::Completed;
    terminal.current = Some(ActivityState::Succeeded);
    terminal.item_count = None;
    recorder
        .record_at(terminal, "2026-09-07T00:00:03.000Z".into(), 400)
        .unwrap();
    let page = recorder.operations(None, None, 1).unwrap();
    assert_eq!(
        page.operations[0].first.draft.operation_id.as_deref(),
        Some("new")
    );
    let older = recorder.operations(page.next_cursor, None, 1).unwrap();
    let row = &older.operations[0];
    assert_eq!(row.event_count, 252);
    assert_eq!(row.started.as_ref().unwrap().monotonic_ms, 10);
    assert_eq!(row.latest.monotonic_ms, 400);
    assert_eq!(row.latest.draft.current, Some(ActivityState::Succeeded));
    assert_eq!(row.progress.as_ref().unwrap().draft.done, Some(250));
    let (events, cursor) = recorder.events(row.id, None, 100).unwrap();
    assert_eq!(events.len(), 100);
    assert!(cursor.is_some());
    assert!(events
        .iter()
        .all(|event| event.draft.operation_id.as_deref() == Some("long")));
    let mut all = events;
    let mut before = cursor;
    while before.is_some() {
        let (events, next) = recorder.events(row.id, before, 100).unwrap();
        all.extend(events);
        before = next;
    }
    assert_eq!(all.len(), 252);
    assert_eq!(all.last().unwrap().event_id, row.first.event_id);
}

#[test]
fn projection_reads_use_seek_indexes_and_reject_corrupt_upgrades_without_partial_changes() {
    let (temp, recorder) = recorder("one");
    drop(recorder);
    let conn = rusqlite::Connection::open(temp.path().join("activity.sqlite3")).unwrap();
    for (query, expected) in [
        ("SELECT * FROM activity_operations WHERE last_id > 1 ORDER BY last_id LIMIT 101", "activity_operations_changed"),
        ("SELECT * FROM activity_operations WHERE first_id < 100 ORDER BY first_id DESC LIMIT 101", "INTEGER PRIMARY KEY"),
        ("SELECT * FROM activity_events WHERE session_id = 'one' AND operation_id = 'work:1' AND id < 100 ORDER BY id DESC LIMIT 101", "activity_events_operation"),
    ] {
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {query}")).unwrap();
        let details = statement.query_map([], |row| row.get::<_, String>(3)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap().join(" ");
        assert!(details.contains(expected), "{details}");
        assert!(!details.contains("SCAN "), "{details}");
    }
    conn.execute_batch("DROP TRIGGER activity_project_insert; DROP TABLE activity_operations;
        ALTER TABLE activity_events DROP COLUMN user_visible; PRAGMA user_version = 0;
        INSERT INTO activity_events(session_id, sequence, event_time_utc, monotonic_ms, owner, kind, draft_json)
        VALUES('old',1,'2026-09-09T00:00:00.000Z',0,'settings','started','broken');").unwrap();
    assert!(onecopy_lib::activity_history::initialize(&conn).is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM activity_events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('activity_events') WHERE name = 'user_visible'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn forward_operation_cursor_catches_every_burst_and_changes_to_old_rows() {
    let (_temp, recorder) = recorder("one");
    recorder
        .record_at(draft(Some("old")), "2026-09-07T00:00:00.000Z".into(), 0)
        .unwrap();
    let mut revision = recorder.operations(None, None, 100).unwrap().revision;
    for i in 0..251 {
        recorder
            .record_at(
                draft(Some(&format!("burst:{i}"))),
                "2026-09-07T00:00:01.000Z".into(),
                i + 1,
            )
            .unwrap();
    }
    let mut end = draft(Some("old"));
    end.kind = ActivityKind::Failed;
    end.current = Some(ActivityState::Failed);
    recorder
        .record_at(end, "2026-09-07T00:00:02.000Z".into(), 999)
        .unwrap();
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = recorder.operations(None, Some(revision), 100).unwrap();
        assert!(page.operations.len() <= 100);
        for row in page.operations {
            assert!(seen.insert(row.id));
        }
        revision = page.revision;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(seen.len(), 252);
    assert!(seen.contains(&1));
    assert!(recorder
        .operations(None, Some(revision), 100)
        .unwrap()
        .operations
        .is_empty());
}

#[test]
fn operation_projection_upgrade_keeps_old_events_and_separates_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("activity.sqlite3");
    let original = ActivityRecorder::new("one".into(), db.clone()).unwrap();
    let mut work = draft(Some("same"));
    work.owner = ActivityOwner::ManagedTools;
    original
        .record_at(work, "2026-09-07T00:00:00.000Z".into(), 2)
        .unwrap();
    original
        .record_at(
            draft(Some("internal")),
            "2026-09-07T00:00:00.001Z".into(),
            3,
        )
        .unwrap();
    drop(original);
    // Restore the original history schema, retaining its event exactly.
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(
        "DROP TRIGGER activity_project_insert; DROP TABLE activity_operations;
        ALTER TABLE activity_events DROP COLUMN user_visible; PRAGMA user_version = 0;",
    )
    .unwrap();
    drop(conn);
    let next = ActivityRecorder::new("two".into(), db).unwrap();
    next.record_at(draft(Some("same")), "2026-09-08T00:00:00.000Z".into(), 1)
        .unwrap();
    let page = next.operations(None, None, 100).unwrap();
    assert_eq!(page.operations.len(), 2);
    assert_eq!(page.operations[1].first.session_id, "one");
    assert_eq!(page.operations[1].first.sequence, 1);
    assert_eq!(page.operations[1].first.monotonic_ms, 2);
    assert_eq!(
        next.page(None, 100).unwrap().0.len(),
        3,
        "internal raw history is retained without ordinary rows"
    );
}
