use onecopy_lib::activity::{
    ActivityDraft, ActivityKind, ActivityOwner, ActivityReason, ActivityRecorder, ActivityState,
    ActivitySubject,
};
use onecopy_lib::{activity_history, index_store, visibility::Policy, visibility_index};

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
        ActivityRecorder::new(session.to_string(), temp.path().join("records.sqlite3")).unwrap();
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
fn projection_reads_use_seek_indexes() {
    let (temp, recorder) = recorder("one");
    drop(recorder);
    let conn = rusqlite::Connection::open(temp.path().join("records.sqlite3")).unwrap();
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
    conn.pragma_update(None, "user_version", 2).unwrap();
    drop(conn);
    assert!(onecopy_lib::records::open(&temp.path().join("records.sqlite3")).is_err());
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
fn the_purge_drops_old_start_and_progress_events_and_keeps_every_operation_readable() {
    let (temp, recorder) = recorder("one");
    let at = |day: u32| format!("2026-06-{day:02}T00:00:00.000Z");
    let mut work = draft(Some("old"));
    work.owner = ActivityOwner::ManagedTools;
    recorder.record_at(work.clone(), at(1), 1).unwrap();
    let mut tick = work.clone();
    tick.kind = ActivityKind::Progressed;
    tick.done = Some(1);
    tick.total = Some(2);
    recorder.record_at(tick, at(2), 2).unwrap();
    let mut end = work.clone();
    end.kind = ActivityKind::Completed;
    end.current = Some(ActivityState::Succeeded);
    recorder.record_at(end, at(3), 3).unwrap();
    let mut crashed = draft(Some("crashed"));
    crashed.owner = ActivityOwner::ManagedTools;
    recorder.record_at(crashed, at(4), 4).unwrap();
    let mut recent = draft(Some("recent"));
    recent.owner = ActivityOwner::ManagedTools;
    recorder.record_at(recent, at(28), 5).unwrap();

    let now = chrono::DateTime::parse_from_rfc3339("2026-09-25T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
    let records = onecopy_lib::records::open(&temp.path().join("records.sqlite3")).unwrap();
    assert_eq!(onecopy_lib::records::purge_transient(&records, now).unwrap(), 3);

    let page = recorder.operations(None, None, 100).unwrap();
    let ids = page.operations.iter().map(|row| row.latest.draft.operation_id.clone().unwrap()).collect::<Vec<_>>();
    assert_eq!(ids, ["recent", "old"]);
    let old = &page.operations[1];
    assert_eq!(old.first.draft.kind, ActivityKind::Completed);
    assert_eq!(old.event_count, 1);
    assert!(old.started.is_none());
    assert_eq!(old.progress.as_ref().map(|event| event.draft.kind), Some(ActivityKind::Completed));
    assert_eq!(recorder.events(old.id, None, 100).unwrap().0.len(), 1);
    assert!(page.operations[0].started.is_some());
}

// R4.4 E5: a target hidden entirely by review-visibility policy resolves to
// "unavailable" (no Target, even though the event still carries the hash it
// happened to), and the target reappears once the policy lifts.
#[test]
fn a_hidden_only_activity_target_resolves_to_unavailable() {
    let (_temp, recorder) = recorder("session-one");
    let mut with_target = draft(Some("op"));
    with_target.target_hash = Some("h".to_string());
    recorder
        .record_at(with_target, "2026-09-07T00:00:00.000Z".to_string(), 0)
        .unwrap();

    let index_dir = tempfile::tempdir().unwrap();
    let index = index_store::open(&index_dir.path().join("index.sqlite3")).unwrap();
    index
        .execute_batch(
            "INSERT INTO contents(hash, kind, byte_size) VALUES ('h', 'image', 1);
            INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash)
              VALUES ('/root/photo.jpg', '/root', 'photo.jpg', 'image', 'h');",
        )
        .unwrap();
    visibility_index::apply_policy(
        &index,
        &Policy::from_config(&serde_json::json!({"ignoredFileNames": ["photo.jpg"]})).unwrap(),
    )
    .unwrap();

    let mut page = recorder.operations(None, None, 100).unwrap();
    activity_history::resolve_targets(&index, &mut page.operations).unwrap();
    assert_eq!(page.operations[0].target_hash.as_deref(), Some("h"));
    assert!(
        page.operations[0].target.is_none(),
        "a hidden-only target must resolve as unavailable, not disappear"
    );

    visibility_index::apply_policy(&index, &Policy::from_config(&serde_json::json!({})).unwrap())
        .unwrap();
    let mut page = recorder.operations(None, None, 100).unwrap();
    activity_history::resolve_targets(&index, &mut page.operations).unwrap();
    assert_eq!(page.operations[0].target.as_ref().unwrap().name, "photo.jpg");
}
