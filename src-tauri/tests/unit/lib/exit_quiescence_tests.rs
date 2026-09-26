use super::await_quiescence_with_deadline;
use std::time::Duration;

#[test]
fn a_signal_before_the_deadline_finishes_without_killing_anything() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    tx.send(()).unwrap();
    assert!(await_quiescence_with_deadline(rx, Duration::from_secs(5)));
}

// The exit-join task (Phase 5): quitting must not wait past its deadline for
// a job that never reaches its own cancellation point.
#[test]
fn no_signal_by_the_deadline_gives_up_waiting() {
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    let started = std::time::Instant::now();
    assert!(!await_quiescence_with_deadline(rx, Duration::from_millis(50)));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "must give up at the deadline, not hang"
    );
}
