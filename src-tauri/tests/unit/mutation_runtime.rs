use super::*;

#[test]
fn cancellation_is_bound_to_one_claim_identity() {
    let _serial = crate::scan_runtime::serial_test();
    let first = begin().unwrap();
    let first_id = first.id();
    assert!(begin().is_err());
    assert!(request_cancel(first_id).unwrap());
    assert!(first.cancelled());
    drop(first);
    assert!(!request_active_cancel().unwrap());

    let second = begin().unwrap();
    assert_ne!(second.id(), first_id);
    assert!(!request_cancel(first_id).unwrap());
    assert!(!second.cancelled());
    assert!(request_active_cancel().unwrap());
    assert!(second.cancelled());
}

#[test]
fn result_accounting_separates_complete_partial_and_unstarted_work() {
    assert_eq!(
        result_summary(8, 4, 3, 12, 7, 2, 0, true, None),
        ResultSummary {
            items_completed: 3,
            items_partial: 1,
            items_unstarted: 4,
            files_completed: 5,
            files_failed: 2,
            files_unknown: 0,
            files_unstarted: 5,
            trash_available: true,
            error: None,
        }
    );
}

// A file whose rename or removal was given up on is neither completed nor a
// known failure: it is reported on its own.
#[test]
fn result_accounting_reports_unknown_outcomes_apart_from_failures() {
    assert_eq!(
        result_summary(2, 2, 1, 4, 4, 3, 2, false, None),
        ResultSummary {
            items_completed: 1,
            items_partial: 1,
            items_unstarted: 0,
            files_completed: 1,
            files_failed: 1,
            files_unknown: 2,
            files_unstarted: 0,
            trash_available: false,
            error: None,
        }
    );
}

#[test]
fn section_recheck_answers_busy_while_a_file_operation_runs() {
    let _serial = crate::scan_runtime::serial_test();
    let operation = begin().unwrap();
    assert_eq!(
        crate::scan_runtime::section_admission().unwrap_err(),
        "Recheck this section is unavailable while a file operation is running."
    );
    drop(operation);
    assert!(crate::scan_runtime::section_admission().is_ok());
}

// Quitting must not wait past its deadline for a file operation that never
// reaches its own safe point (`specs/file-operations.md`, "Normal exit and
// abnormal termination"): it gives up on the current file, not on the whole
// exit sequence.
#[test]
fn wait_for_idle_gives_up_at_its_deadline_when_the_claim_never_drops() {
    let _serial = crate::scan_runtime::serial_test();
    let claim = begin().unwrap();
    let started = std::time::Instant::now();
    let outcome = wait_for_idle(std::time::Duration::from_millis(50));
    assert_eq!(outcome, Ok(IdleWait::TimedOut));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "must give up at the deadline, not hang"
    );
    drop(claim);
}

// A file operation that reaches its safe point (drops its claim) before the
// deadline lets exit proceed at once, without waiting out the full deadline.
#[test]
fn wait_for_idle_returns_as_soon_as_the_claim_drops_before_the_deadline() {
    let _serial = crate::scan_runtime::serial_test();
    let claim = begin().unwrap();
    let released = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(claim);
    });
    let started = std::time::Instant::now();
    let outcome = wait_for_idle(std::time::Duration::from_secs(10));
    released.join().unwrap();
    assert_eq!(outcome, Ok(IdleWait::Idle));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "must not wait out the full deadline once the claim already dropped"
    );
}

#[test]
fn a_rebuild_requested_during_a_file_operation_is_refused_at_once_and_never_queued() {
    // `begin_rebuild` takes this same claim, reporting only state failures.
    let _serial = crate::scan_runtime::serial_test();
    let operation = begin().unwrap();
    let requested = std::time::Instant::now();
    let refused = begin().map(|_| ()).unwrap_err();
    assert!(requested.elapsed() < std::time::Duration::from_millis(100));
    assert_eq!(refused, "Another file operation is already running.");
    drop(operation);
    // Nothing waited to run once the operation ended.
    assert!(!active());
}
