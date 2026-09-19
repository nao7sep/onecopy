use super::*;

#[test]
fn cancellation_is_bound_to_one_claim_identity() {
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
