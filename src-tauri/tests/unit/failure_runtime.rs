use super::presentation_for;

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
