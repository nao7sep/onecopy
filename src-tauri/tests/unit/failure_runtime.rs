use super::{condition_message_key, interface_notice, presentation_for, InterfaceFailure};

#[test]
fn runtime_diagnostics_are_not_user_presentation() {
    let hostile =
        "Error invoking remote method: EACCES /private/tmp/HOSTILE-SENTINEL";
    let presentation = presentation_for("file-operation-state-failed");

    assert!(!presentation.contains(hostile));
    assert!(!presentation.contains("EACCES"));
    assert!(!presentation.contains("/private/tmp"));
    assert!(!presentation.contains("Error invoking remote method"));
    // Both conditions have a sentence of their own, not the generic one an
    // unrecognised kind falls back to.
    let generic = presentation_for("an-unnamed-condition-nothing-maps-to");
    let derived = presentation_for("derived-worker-failed");
    assert_ne!(presentation, generic);
    assert_ne!(derived, generic);
    assert_ne!(presentation, derived);
}

#[test]
fn every_condition_message_key_exists_in_every_embedded_catalogue() {
    // These are the keys a notice or Issue's descriptor names; a typo here
    // would fall back to `t()`'s key-echo behavior instead of a sentence, in
    // every language at once.
    let kinds = [
        "sleep-prevention-failed", "derived-worker-failed", "config-save-failed",
        "state-save-failed", "source-check-failed", "watcher-failed", "watcher-root-failed",
        crate::watcher::WATCHER_RECOVERY_FAILED,
        "file-information-state-failed", "background-work-state-failed",
        "file-operation-state-failed", "trash-empty-entry-failed", "external-open-failed",
        "text-preview-failed", "dependency-install-failed", "update-check-failed",
        "instance-activation-failed", "instance-listener-failed", "media-use-state-failed",
        "transcription-worker-failed", "shutdown-media-release-failed",
        "shutdown-worker-failed",
        "source-check-feedback-failed", "event-delivery-failed", "interface-failed",
        "an-unnamed-condition-nothing-maps-to",
    ];
    for language in crate::i18n::LANGUAGES {
        let text = crate::i18n::catalogue(language);
        for kind in kinds {
            let key = condition_message_key(kind);
            assert!(text.has(key), "{language} lacks {key} (from kind {kind})");
        }
    }
}

#[test]
fn an_interface_failure_keeps_its_own_sentence_and_the_real_error_without_a_window() {
    let mut values = serde_json::Map::new();
    values.insert("name".to_string(), serde_json::json!("Preview"));
    let notice = interface_notice(InterfaceFailure {
        message_key: "app.appearanceUpdateFailed".to_string(),
        message_values: Some(values.clone()),
        detail: "x".to_string(),
    });
    // The notice names a recognised condition, so its Issue reads that
    // condition's sentence rather than the generic fallback.
    assert_ne!(
        condition_message_key(&notice.kind),
        condition_message_key("an-unnamed-condition-nothing-maps-to")
    );
    assert_eq!(notice.message_key.as_deref(), Some("app.appearanceUpdateFailed"));
    assert_eq!(notice.message_values, Some(values));
    assert_eq!(notice.message, "x");
    assert_eq!(notice.path, None);
}
