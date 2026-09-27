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
            files_already_present: 0,
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
            files_already_present: 0,
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

// R3-07, R1-14: the volume-substitution gate at mutation admission is scoped
// to the configured roots the accepted batch actually sits under, so a
// substituted or unverifiable source never blocks a batch that touches only
// another, unaffected root.
#[test]
fn touched_source_dirs_scopes_to_roots_the_accepted_batch_actually_sits_under() {
    let source_dirs = vec![
        "/Volumes/A".to_string(),
        "/Volumes/B".to_string(),
        "/Volumes/C".to_string(),
    ];
    let accepted = crate::operations::AcceptedFiles::for_test_paths([
        "/Volumes/A/2024/IMG_0001.jpg",
        "/Volumes/A/2024/IMG_0001.xmp",
    ]);
    let touched = touched_source_dirs(&source_dirs, &Touches::Files(&accepted));
    assert_eq!(touched, vec!["/Volumes/A".to_string()]);
}

#[test]
fn touched_source_dirs_is_empty_when_the_batch_touches_no_configured_source() {
    let source_dirs = vec!["/Volumes/A".to_string()];
    let accepted =
        crate::operations::AcceptedFiles::for_test_paths(["/Volumes/Elsewhere/photo.jpg"]);
    assert!(touched_source_dirs(&source_dirs, &Touches::Files(&accepted)).is_empty());
}

// A restore's receipt keeps "already there" apart from failures and from
// outcome-unknown renames.
#[test]
fn a_restore_receipt_counts_restored_already_there_failed_unknown_and_unstarted() {
    let outcome = crate::restore::RestoreOutcome {
        restored: vec!["/r/a.jpg".to_string(), "/r/b.jpg".to_string()],
        already_present: 1,
        failed: 2,
        unknown: 1,
        unstarted: 3,
        files_total: 8,
        ..Default::default()
    };
    assert_eq!(
        restore_summary(&outcome),
        ResultSummary {
            items_completed: 2,
            items_partial: 0,
            items_unstarted: 3,
            files_completed: 2,
            files_failed: 1,
            files_unknown: 1,
            files_unstarted: 3,
            files_already_present: 1,
            trash_available: false,
            error: None,
        }
    );
}

// Restore touches one configured root: a source containing it is gated, a
// destination root never is (blueprint case 8: the manifest travels with its
// own drive).
#[test]
fn a_restore_gates_only_the_source_its_root_lies_in() {
    let source_dirs = vec!["/Volumes/A".to_string(), "/Volumes/B".to_string()];
    assert_eq!(
        touched_source_dirs(&source_dirs, &Touches::Root("/Volumes/B")),
        vec!["/Volumes/B".to_string()]
    );
    assert_eq!(
        touched_source_dirs(&source_dirs, &Touches::Root("/Volumes/A/nested")),
        vec!["/Volumes/A".to_string()]
    );
    assert!(touched_source_dirs(&source_dirs, &Touches::Root("/Volumes/Backup")).is_empty());
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
