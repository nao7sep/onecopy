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
        result_summary(8, 4, 3, 12, 7, 2, true, None),
        ResultSummary {
            items_completed: 3,
            items_partial: 1,
            items_unstarted: 4,
            files_completed: 5,
            files_failed: 2,
            files_unstarted: 5,
            trash_available: true,
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
