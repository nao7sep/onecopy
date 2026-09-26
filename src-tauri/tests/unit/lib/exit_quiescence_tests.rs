use super::{await_exit_readiness, await_other_joins_with_deadline};
use std::time::Duration;

#[test]
fn a_signal_before_the_deadline_finishes_without_killing_anything() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    tx.send(()).unwrap();
    assert!(await_other_joins_with_deadline(rx, Duration::from_secs(5)));
}

// The exit-join task (Phase 5): quitting must not wait past its deadline for
// a job that never reaches its own cancellation point.
#[test]
fn no_signal_by_the_deadline_gives_up_waiting() {
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    let started = std::time::Instant::now();
    assert!(!await_other_joins_with_deadline(rx, Duration::from_millis(50)));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "must give up at the deadline, not hang"
    );
}

// The Phase 5 fix: a mutation still in flight must never be terminated by
// the exit deadline (`specs/file-operations.md`, "Normal exit and abnormal
// termination"). A stuck derived join (never signals `other_rx`) is
// abandoned at its short deadline, but exit readiness must still wait for
// the long-running fake mutation step to finish, with no deadline of its
// own. This fails against the pre-fix code, which force-exited the process
// after a single shared 10 s deadline regardless of mutation state.
#[test]
fn mutation_quiescence_has_no_deadline_while_a_stuck_derived_join_is_abandoned() {
    let (_other_tx, other_rx) = std::sync::mpsc::channel::<()>();
    let (mutation_tx, mutation_rx) = std::sync::mpsc::channel::<()>();
    let deadline = Duration::from_millis(50);
    let mutation_delay = Duration::from_millis(300);

    std::thread::spawn(move || {
        std::thread::sleep(mutation_delay);
        let _ = mutation_tx.send(());
    });

    let started = std::time::Instant::now();
    await_exit_readiness(other_rx, mutation_rx, deadline);
    let elapsed = started.elapsed();

    assert!(
        elapsed >= mutation_delay,
        "exit readiness must wait for mutation quiescence past the bounded \
         deadline, not exit early: waited {elapsed:?}"
    );
}
