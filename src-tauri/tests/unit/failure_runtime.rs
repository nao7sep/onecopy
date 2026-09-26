use super::{condition_message_key, presentation_for};

#[test]
fn runtime_diagnostics_are_not_user_presentation() {
    let hostile =
        "Error invoking remote method: EACCES /private/tmp/HOSTILE-SENTINEL";
    let presentation = presentation_for("file-operation-state-failed");

    assert!(!presentation.contains(hostile));
    assert!(!presentation.contains("EACCES"));
    assert!(!presentation.contains("/private/tmp"));
    assert!(!presentation.contains("Error invoking remote method"));
    assert!(presentation.contains("file operation"));
    assert!(presentation_for("derived-worker-failed").contains("Resume a row in Background work"));
}

#[test]
fn every_condition_message_key_exists_in_every_embedded_catalogue() {
    // These are the keys a notice or Issue's descriptor names; a typo here
    // would fall back to `t()`'s key-echo behavior instead of a sentence, in
    // every language at once (R5.5 D-L12).
    let kinds = [
        "sleep-prevention-failed", "derived-worker-failed", "config-save-failed",
        "state-save-failed", "source-check-failed", "watcher-failed", "watcher-root-failed",
        "file-information-state-failed", "background-work-state-failed",
        "file-operation-state-failed", "trash-empty-entry-failed", "external-open-failed",
        "text-preview-failed", "dependency-install-failed", "update-check-failed",
        "instance-activation-failed", "instance-listener-failed", "media-use-state-failed",
        "transcription-worker-failed", "shutdown-media-release-failed",
        "shutdown-window-recovery-failed", "shutdown-worker-failed",
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
