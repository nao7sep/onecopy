//! Notices: each one is a record in `records.sqlite3`, written in the same
//! transaction as the Issue it raises, plus the process-local live notices
//! currently projected above OneCopy's viewing surfaces. Live notices are
//! presentation state for this process and clear each session.

use std::sync::{LazyLock, Mutex};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::AppHandle;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NotificationPresentation {
    Timed,
    Persistent,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRequest {
    pub kind: String,
    pub path: Option<String>,
    pub level: NotificationLevel,
    pub presentation: NotificationPresentation,
    /// The rendered sentence, in whatever language this window currently
    /// shows. Kept in the record and for a row with no descriptor; a window that redraws its live notices while this one is
    /// still active renders `message_key` instead, so they follow a later
    /// language change (R5.5 D-L12).
    pub message: String,
    /// A catalogue key the frontend can re-render in the current interface
    /// language; `None` for a condition the caller does not (yet) key.
    pub message_key: Option<String>,
    /// The key's interpolation values, as a JSON object; `None` when it
    /// takes none.
    pub message_values: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: i64,
    pub kind: String,
    pub path: Option<String>,
    pub level: NotificationLevel,
    pub presentation: NotificationPresentation,
    pub message: String,
    pub message_key: Option<String>,
    pub message_values: Option<serde_json::Map<String, serde_json::Value>>,
    pub first_seen_utc: String,
    pub last_seen_utc: String,
    pub occurrence_count: u64,
}

static ACTIVE: LazyLock<Mutex<Vec<NotificationRecord>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

fn level_name(level: NotificationLevel) -> &'static str {
    match level {
        NotificationLevel::Info => "info",
        NotificationLevel::Warning => "warning",
        NotificationLevel::Error => "error",
    }
}

fn presentation_name(presentation: NotificationPresentation) -> &'static str {
    match presentation {
        NotificationPresentation::Timed => "timed",
        NotificationPresentation::Persistent => "persistent",
    }
}

fn validate(request: &NotificationRequest) -> Result<(), String> {
    if request.kind.trim().is_empty() {
        return Err("notification kind is required".to_string());
    }
    // A row must say SOMETHING: either real recorded detail, or a message
    // key the frontend renders into a sentence. A keyed row with nothing else
    // to add legitimately sends an empty message (R5.5 D-L12).
    if request.message.trim().is_empty() && request.message_key.is_none() {
        return Err("notification message or message key is required".to_string());
    }
    Ok(())
}

fn record_notice(
    conn: &Connection,
    request: &NotificationRequest,
) -> Result<NotificationRecord, String> {
    validate(request)?;
    let now = crate::logging::now_iso_millis();
    let transaction = crate::sqlite::write_transaction(conn)
        .map_err(|error| error.to_string())?;
    let message_values_json = request
        .message_values
        .as_ref()
        .map(|values| serde_json::Value::Object(values.clone()).to_string());
    if request.level != NotificationLevel::Info {
        crate::index_store::upsert_issue_with_descriptor(
            &transaction,
            request.path.as_deref(),
            &request.kind,
            request.message_key.as_deref(),
            message_values_json.as_deref(),
            &request.message,
        )?;
    }
    transaction
        .execute(
            &format!(
                "INSERT INTO records.notices
                   (session_id, time_utc, kind, path, level, presentation, message, message_key, message_values)
                 VALUES ({}, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                crate::records::session_sql()
            ),
            params![
                now,
                request.kind,
                request.path,
                level_name(request.level),
                presentation_name(request.presentation),
                request.message,
                request.message_key,
                message_values_json,
            ],
        )
        .map_err(|error| error.to_string())?;
    crate::records::wrote(&transaction);
    let id = transaction.last_insert_rowid();
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(NotificationRecord {
        id,
        kind: request.kind.clone(),
        path: request.path.clone(),
        level: request.level,
        presentation: request.presentation,
        message: request.message.clone(),
        message_key: request.message_key.clone(),
        message_values: request.message_values.clone(),
        first_seen_utc: now.clone(),
        last_seen_utc: now,
        occurrence_count: 1,
    })
}

/// Adds a published notice to the live list. An equal notice still showing
/// takes the new occurrence instead of stacking a second one.
fn remember_active(record: NotificationRecord) -> NotificationRecord {
    let mut active = ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = active.iter_mut().find(|item| {
        item.kind == record.kind
            && item.path == record.path
            && item.level == record.level
            && item.presentation == record.presentation
            && item.message == record.message
    }) {
        existing.last_seen_utc = record.last_seen_utc;
        existing.message_key = record.message_key;
        existing.message_values = record.message_values;
        existing.occurrence_count += 1;
        existing.clone()
    } else {
        active.push(record.clone());
        record
    }
}

fn record_delivery_failure(event: &str, error: &str) -> Result<(), String> {
    crate::logging::error(
        "notification event delivery failed",
        json!({ "event": event, "error": { "message": error } }),
    );
    let root = crate::paths::data_root()?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let fallback = NotificationRequest {
        kind: "event-delivery-failed".to_string(),
        path: Some(event.to_string()),
        level: NotificationLevel::Error,
        presentation: NotificationPresentation::Persistent,
        message: "OneCopy could not update part of the interface. Reload the window before continuing.".to_string(),
        message_key: Some(crate::failure_runtime::condition_message_key("event-delivery-failed").to_string()),
        message_values: None,
    };
    let _ = record_notice(&conn, &fallback)?;
    Ok(())
}

pub fn publish(app: &AppHandle, request: NotificationRequest) -> Result<NotificationRecord, String> {
    let root = crate::paths::data_root()?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let record = remember_active(record_notice(&conn, &request)?);
    if let Err(message) =
        crate::failure_runtime::emit_checked(app, "notification://published", &record)
    {
        record_delivery_failure("notification://published", &message)?;
    }
    Ok(record)
}

pub fn record_history(
    request: NotificationRequest,
) -> Result<NotificationRecord, String> {
    let root = crate::paths::data_root()?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    record_notice(&conn, &request)
}

pub fn active() -> Vec<NotificationRecord> {
    let mut records = ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    records.sort_by(|left, right| {
        right
            .last_seen_utc
            .cmp(&left.last_seen_utc)
            .then_with(|| right.id.cmp(&left.id))
    });
    records
}

pub fn dismiss(app: &AppHandle, id: i64) -> Result<bool, String> {
    let exists = ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .any(|record| record.id == id);
    if !exists {
        return Ok(false);
    }
    crate::failure_runtime::emit_checked(app, "notification://dismissed", json!({ "id": id }))?;
    let mut active = ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    active.retain(|record| record.id != id);
    Ok(true)
}

pub fn clear_active(app: &AppHandle) -> Result<(), String> {
    crate::failure_runtime::emit_checked(app, "notification://cleared", ())?;
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    Ok(())
}

#[cfg(test)]
// EXCEPTION (tests-folder conventions): the record writer and the
// process-local live owner are private implementation state.
#[path = "../tests/unit/notifications.rs"]
mod tests;
