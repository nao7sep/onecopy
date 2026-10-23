use super::*;

#[test]
fn cancellation_that_owns_the_claim_boundary_prevents_publication() {
    let _serial = crate::scan_runtime::serial_test();
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

#[test]
fn early_whisper_failure_keeps_model_version_unknown() {
    let _serial = crate::scan_runtime::serial_test();
    let _claim = claim().unwrap();
    assert!(request_cancel());
    let dir = tempfile::tempdir().unwrap();

    let outcome = run_whisper_with_model(
        &dir.path().join("not-loaded.bin"),
        &[0.5],
        crate::ai_acceleration::Mode::None,
        |_| panic!("inference must not start before cancellation is checked"),
    );

    assert!(outcome.result.is_err());
    assert_eq!(outcome.model, crate::ai_dependencies::transcription_model());
    assert_eq!(outcome.model.version, "unknown");
}
