use onecopy_lib::activity::{
    ActivityDraft, ActivityKind, ActivityOwner, ActivityReason, ActivityRecorder, ActivityState,
};

fn draft(operation_id: Option<&str>) -> ActivityDraft {
    ActivityDraft {
        kind: ActivityKind::Started,
        owner: ActivityOwner::Section,
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

#[test]
fn recorder_assigns_one_session_and_monotonic_sequence() {
    let recorder = ActivityRecorder::new("session-one".to_string(), 3);
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
    assert_eq!(recorder.snapshot(), vec![first, second]);
}

#[test]
fn recorder_retains_only_its_bounded_tail() {
    let recorder = ActivityRecorder::new("session-one".to_string(), 2);
    for sequence in 1..=3 {
        recorder
            .record_at(
                draft(Some(&format!("section:{sequence}"))),
                "2026-09-07T00:00:00.000Z".to_string(),
                sequence,
            )
            .unwrap();
    }
    let events = recorder.snapshot();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence, 2);
    assert_eq!(events[1].sequence, 3);
}

#[test]
fn recorder_rejects_free_form_identifiers_and_invalid_progress() {
    let recorder = ActivityRecorder::new("session-one".to_string(), 2);
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
    let recorder = ActivityRecorder::new("session-one".to_string(), 1);
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
fn concrete_timing_owners_serialize_without_free_form_payloads() {
    let recorder = ActivityRecorder::new("session-one".to_string(), 8);
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
