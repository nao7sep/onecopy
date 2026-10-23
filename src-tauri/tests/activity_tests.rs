use onecopy_lib::activity::{
    ActivityDraft, ActivityKind, ActivityOwner, ActivityReason, ActivityRecorder, ActivityState,
    ActivitySubject,
};
use onecopy_lib::records_view::{self, Cursor, RecordKind, RecordsQuery};

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
    let (temp, recorder) = recorder("session-one");
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
    let reader = records_view::open_reader(&temp.path().join("records.sqlite3")).unwrap();
    let page = records_view::page(&reader, &RecordsQuery { kind: Some(RecordKind::Activity), ..Default::default() }).unwrap();
    assert_eq!(page.records.iter().map(|row| row.id).collect::<Vec<_>>(), vec![second.event_id, first.event_id]);
    assert!(page.records.iter().all(|row| row.session.as_deref() == Some("session-one")));
    assert!(!page.more);
}

#[test]
fn recorder_persists_every_event_and_pages_with_a_stable_cursor() {
    let (temp, recorder) = recorder("session-one");
    let mut ids = Vec::new();
    for sequence in 1..=records_view::PAGE_SIZE + 1 {
        let event = recorder.record_at(
            draft(Some(&format!("section:{sequence}"))),
            "2026-09-07T00:00:00.000Z".to_string(),
            sequence as u64,
        ).unwrap();
        ids.push(event.event_id);
    }
    let reader = records_view::open_reader(&temp.path().join("records.sqlite3")).unwrap();
    let query = RecordsQuery { kind: Some(RecordKind::Activity), ..Default::default() };
    let newest = records_view::page(&reader, &query).unwrap();
    assert!(newest.more);
    assert_eq!(newest.records.iter().map(|row| row.id).collect::<Vec<_>>(), ids[1..].iter().rev().copied().collect::<Vec<_>>());
    let last = newest.records.last().unwrap();
    // New writes between pages cannot repeat or displace the cursor's older row.
    recorder.record_at(draft(Some("section:new")), "2026-09-07T00:00:01.000Z".into(), 1000).unwrap();
    let older = records_view::page(&reader, &RecordsQuery {
        after: Some(Cursor { time: last.time.clone(), kind: last.kind, id: last.id }),
        ..query
    }).unwrap();
    assert_eq!(older.records.iter().map(|row| row.id).collect::<Vec<_>>(), vec![ids[0]]);
    assert!(!older.more);
}

#[test]
fn records_reads_background_lifecycle_and_details_after_recorder_reopens() {
    use onecopy_lib::records_view::{self, RecordKind, RecordsQuery};
    let (temp, recorder) = recorder("background-session");
    let mut event = draft(Some("backgroundWork:one"));
    event.owner = ActivityOwner::BackgroundWork;
    event.subject = Some(ActivitySubject::VideoTranscription);
    event.item_count = None;
    recorder.record_at(event.clone(), "2026-09-07T00:00:00.000Z".into(), 0).unwrap();
    event.kind = ActivityKind::Progressed;
    event.done = Some(2);
    event.total = Some(3);
    let progress = recorder.record_at(event.clone(), "2026-09-07T00:00:01.000Z".into(), 1000).unwrap();
    event.kind = ActivityKind::Completed;
    event.current = Some(ActivityState::Succeeded);
    event.done = Some(3);
    recorder.record_at(event, "2026-09-07T00:00:02.000Z".into(), 2000).unwrap();
    drop(recorder);

    let path = temp.path().join("records.sqlite3");
    let _reopened = ActivityRecorder::new("next-session".into(), path.clone()).unwrap();
    let reader = records_view::open_reader(&path).unwrap();
    let page = records_view::page(&reader, &RecordsQuery {
        session: Some("background-session".into()),
        kind: Some(RecordKind::Activity),
        search: "backgroundWork:one".into(),
        ..Default::default()
    }).unwrap();
    assert_eq!(page.records.len(), 3);
    assert!(page.records.iter().all(|row| row.kind == RecordKind::Activity));
    let detail = records_view::detail(&reader, RecordKind::Activity, progress.event_id).unwrap().unwrap();
    let json = &detail.fields.iter().find(|field| field.name == "draft_json").unwrap().value;
    let draft: serde_json::Value = serde_json::from_str(json.as_str().unwrap()).unwrap();
    assert_eq!(draft["done"], 2);
    assert_eq!(draft["total"], 3);
    assert_eq!(
        serde_json::from_value::<ActivitySubject>(draft["subject"].clone()).unwrap(),
        ActivitySubject::VideoTranscription
    );
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
    assert_eq!(value["current"], serde_json::to_value(ActivityState::Running).unwrap());
    let fields = value.as_object().unwrap();
    for (name, field) in fields {
        assert!(!field.is_null(), "{name} is absent, so it should be omitted");
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
    assert!(value["subject"].is_string(), "{value}");
    assert_eq!(
        serde_json::from_value::<ActivitySubject>(value["subject"].clone()).unwrap(),
        ActivitySubject::VideoTranscription
    );
}

#[test]
fn concrete_timing_owners_serialize_without_free_form_payloads() {
    let (_temp, recorder) = recorder("session-one");
    let owners = [
        ActivityOwner::Anchor,
        ActivityOwner::ManagedTools,
        ActivityOwner::Media,
        ActivityOwner::Watcher,
        ActivityOwner::Identity,
        ActivityOwner::Delivery,
    ];
    let mut names = std::collections::HashSet::new();
    for owner in owners {
        let mut event = draft(None);
        event.owner = owner;
        let value = serde_json::to_value(
            recorder
                .record_at(event, "2026-09-07T00:00:00.000Z".to_string(), 10)
                .unwrap(),
        )
        .unwrap();
        assert!(value["owner"].is_string(), "{value}");
        assert_eq!(serde_json::from_value::<ActivityOwner>(value["owner"].clone()).unwrap(), owner);
        names.insert(value["owner"].as_str().unwrap().to_string());
        assert!(value.get("path").is_none());
        assert!(value.get("payload").is_none());
    }
    assert_eq!(names.len(), owners.len(), "each owner has its own name");
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

    let reader = records_view::open_reader(&temp.path().join("records.sqlite3")).unwrap();
    let page = records_view::page(&reader, &RecordsQuery { kind: Some(RecordKind::Activity), ..Default::default() }).unwrap();
    assert!(!page.more);
    assert_eq!(page.records.len(), 2);
    let drafts = page.records.iter().map(|row| {
        let detail = records_view::detail(&reader, row.kind, row.id).unwrap().unwrap();
        let value = &detail.fields.iter().find(|field| field.name == "draft_json").unwrap().value;
        serde_json::from_str::<ActivityDraft>(value.as_str().unwrap()).unwrap()
    }).collect::<Vec<_>>();
    assert_eq!(drafts[0].operation_id.as_deref(), Some("recent"));
    assert_eq!(drafts[1].operation_id.as_deref(), Some("old"));
    assert_eq!(drafts[1].kind, ActivityKind::Completed);
}
