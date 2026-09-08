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
