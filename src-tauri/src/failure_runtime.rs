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

pub fn report(
    app: &AppHandle,
    kind: &str,
    path: Option<&str>,
    message: &str,
) -> Result<(), String> {
    record_active(app, kind, path, message)?;
    crate::notifications::publish(
        app,
        crate::notifications::NotificationRequest {
            kind: kind.to_string(),
            path: path.map(str::to_string),
            level: crate::notifications::NotificationLevel::Error,
            presentation: crate::notifications::NotificationPresentation::Persistent,
            message: message.to_string(),
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
    crate::index_store::upsert_issue(&conn, path, kind, message)
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
            &emit_error,
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
                message: emit_error.clone(),
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
    let direct = format!(
        "{message} OneCopy also could not save this failure to {record_name}: {save_error}"
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
