//! Shared last-resort reporting for failures that need user attention.
//!
//! The application log keeps technical history. The Issues table keeps one
//! restart-persistent current condition per `(kind, path)`. If that durable
//! record cannot be written, the affected owner must stop and this module
//! attempts a direct interface event instead of pretending the failure was
//! safely recorded.

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

/// User-facing failure copy is owned here, where arbitrary runtime diagnostics
/// cross into Issues, Recent, and the direct last-resort surface. Callers keep
/// supplying the complete diagnostic for the log; no exception prose is
/// persisted as presentation.
fn presentation_for(kind: &str) -> &'static str {
    match kind {
        "config-save-failed" | "state-save-failed" =>
            "OneCopy could not save an application setting. Your library files were not changed.",
        "source-check-failed" | "watcher-failed" | "watcher-root-failed" =>
            "OneCopy could not monitor a source folder. Check that the folder is available, then retry the scan.",
        "file-information-state-failed" | "background-work-state-failed" =>
            "OneCopy could not save background-work progress. Restart OneCopy before continuing.",
        "file-operation-state-failed" | "trash-empty-entry-failed" =>
            "OneCopy could not finish a file operation. Review the affected items in Issues before retrying.",
        "external-open-failed" =>
            "OneCopy could not open this item in another application. Check that the file is available, then try again.",
        "text-preview-failed" =>
            "OneCopy could not prepare this text preview. The original file was not changed.",
        "dependency-install-failed" | "update-check-failed" =>
            "OneCopy could not finish the managed-tool operation. Try again.",
        "instance-activation-failed" | "instance-listener-failed" =>
            "OneCopy could not connect this window to the running application. Restart OneCopy.",
        "media-use-state-failed" =>
            "OneCopy could not save media-use state. Restart OneCopy before continuing.",
        "transcription-worker-failed" =>
            "Transcription stopped unexpectedly. Restart OneCopy, then try again.",
        "shutdown-media-release-failed" | "shutdown-window-recovery-failed" | "shutdown-worker-failed" =>
            "OneCopy could not finish shutting down cleanly. Restart it before continuing.",
        "event-delivery-failed" =>
            "OneCopy could not update part of the interface. Reload the window before continuing.",
        "interface-failed" =>
            "This window could not finish an interface operation. Reload it before continuing.",
        "issue-recovery-failed" =>
            "OneCopy could not complete the selected recovery. Review Issues, then try again.",
        _ => "A OneCopy background operation stopped unexpectedly. Restart OneCopy before continuing.",
    }
}

pub fn report(
    app: &AppHandle,
    kind: &str,
    path: Option<&str>,
    message: &str,
) -> Result<(), String> {
    let presentation = presentation_for(kind);
    record_active(app, kind, path, message)?;
    crate::notifications::publish(
        app,
        crate::notifications::NotificationRequest {
            kind: kind.to_string(),
            path: path.map(str::to_string),
            level: crate::notifications::NotificationLevel::Error,
            presentation: crate::notifications::NotificationPresentation::Persistent,
            message: presentation.to_string(),
        },
    )
    .map_err(|error| present_unrecorded(app, kind, path, message, "Recent", &error))?;
    Ok(())
}

/// Records one unresolved condition without creating a per-input notification.
/// Large operations use this for detailed Active rows and publish one summary
/// through their owning result surface.
pub fn record_active(
    app: &AppHandle,
    kind: &str,
    path: Option<&str>,
    message: &str,
) -> Result<(), String> {
    let presentation = presentation_for(kind);
    crate::logging::error(
        "application failure",
        json!({
            "kind": kind,
            "path": path,
            "error": { "message": message },
        }),
    );
    let conn = crate::paths::data_root(app)
        .and_then(|root| crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME)))
        .map_err(|error| present_unrecorded(app, kind, path, message, "Issues", &error))?;
    crate::index_store::upsert_issue(&conn, path, kind, presentation)
        .map_err(|error| present_unrecorded(app, kind, path, message, "Issues", &error))?;
    if let Err(emit_error) = emit_checked(app, "failure://reported", json!({ "kind": kind })) {
        crate::logging::error(
            "failure notification event failed",
            json!({ "error": { "message": &emit_error } }),
        );
        crate::index_store::upsert_issue(
            &conn,
            Some("failure://reported"),
            "event-delivery-failed",
            presentation_for("event-delivery-failed"),
        )
        .map_err(|save_error| {
            present_unrecorded(
                app,
                "event-delivery-failed",
                Some("failure://reported"),
                &emit_error,
                "Issues",
                &save_error,
            )
        })?;
        crate::notifications::record_history(
            app,
            crate::notifications::NotificationRequest {
                kind: "event-delivery-failed".to_string(),
                path: Some("failure://reported".to_string()),
                level: crate::notifications::NotificationLevel::Error,
                presentation: crate::notifications::NotificationPresentation::Persistent,
                message: presentation_for("event-delivery-failed").to_string(),
            },
        )
        .map_err(|save_error| {
            present_unrecorded(
                app,
                "event-delivery-failed",
                Some("failure://reported"),
                &emit_error,
                "Recent",
                &save_error,
            )
        })?;
    }
    Ok(())
}

fn present_unrecorded(
    app: &AppHandle,
    kind: &str,
    path: Option<&str>,
    message: &str,
    record_name: &str,
    save_error: &str,
) -> String {
    let presentation = presentation_for(kind);
    let direct = format!(
        "{presentation} OneCopy also could not save this failure to {record_name}. Reload the window before continuing."
    );
    crate::logging::error(
        "application failure could not be saved",
        json!({
            "kind": kind,
            "path": path,
            "failure": { "message": message },
            "record": record_name,
            "error": { "message": save_error },
        }),
    );
    if let Err(emit_error) = emit_checked(app, "failure://direct", json!({ "message": direct })) {
        crate::logging::error(
            "direct failure event failed",
            json!({ "error": { "message": emit_error } }),
        );
    }
    direct
}

pub fn clear(app: &AppHandle, kind: &str, path: Option<&str>) -> Result<(), String> {
    let root = crate::paths::data_root(app)?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    crate::index_store::clear_issues(&conn, path.unwrap_or(""), &[kind])
}

pub fn emit_checked<T: Clone + Serialize>(
    app: &AppHandle,
    event: &str,
    payload: T,
) -> Result<(), String> {
    app.emit(event, payload)
        .map_err(|error| format!("could not publish {event}: {error}"))
}

pub fn emit_or_record<T: Clone + Serialize>(app: &AppHandle, event: &str, payload: T) {
    if let Err(error) = emit_checked(app, event, payload) {
        let _ = report(app, "event-delivery-failed", Some(event), &error);
    }
}

pub fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "worker stopped unexpectedly".to_string())
}

#[cfg(test)]
mod tests {
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
    }
}

pub fn spawn_reported(
    app: AppHandle,
    thread_name: &'static str,
    issue_kind: &'static str,
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let handle = app.clone();
    let started = std::thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let failure = match outcome {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error,
                Err(payload) => panic_message(payload),
            };
            let _ = report(&handle, issue_kind, None, &failure);
        });
    match started {
        Ok(_) => Ok(()),
        Err(error) => {
            let message = format!("could not start {thread_name}: {error}");
            let _ = report(&app, issue_kind, None, &message);
            Err(message)
        }
    }
}
