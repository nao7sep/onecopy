use super::*;

#[test]
fn cancellation_that_owns_the_claim_boundary_prevents_publication() {
    let claim = claim().unwrap();
    assert!(request_cancel());
    let published = std::cell::Cell::new(false);

    let result = publish_if_active(&claim, || {
        published.set(true);
        Ok(())
    })
    .unwrap();

    assert!(result.is_none());
    assert!(!published.get());
}
