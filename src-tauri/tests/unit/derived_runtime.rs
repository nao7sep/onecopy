use super::*;

#[test]
fn shutdown_releases_every_manual_ticket_from_its_queue_wait() {
    let runtime = RuntimeState {
        exclusive: true,
        active: Some(ActiveWorkSnapshot {
            class: WorkClass::Previews,
            manual: true,
            done: None,
            total: None,
        }),
        serving_manual_ticket: 2,
        ..RuntimeState::default()
    };
    assert!(manual_waits(&runtime, 7, false));
    assert!(!manual_waits(&runtime, 7, true));
}

#[test]
fn shutdown_is_a_cancellation_condition_for_active_derived_work() {
    let runtime = RuntimeState::default();
    assert!(!runtime_cancelled(&runtime, false));
    assert!(runtime_cancelled(&runtime, true));
}
