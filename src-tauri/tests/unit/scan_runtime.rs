use super::*;

#[test]
fn cancellation_is_revalidated_after_the_projection_lock_is_owned() {
    let ran = std::cell::Cell::new(false);

    let result = with_owner(
        Owner::Watcher,
        || true,
        || {
            ran.set(true);
            Ok(())
        },
    );

    assert_eq!(result.unwrap_err(), crate::scanner::CANCELLED);
    assert!(!ran.get());
    assert_eq!(ACTIVE_OWNER.load(Ordering::SeqCst), 0);
    assert!(!crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst));
}
