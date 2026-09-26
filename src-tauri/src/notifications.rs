//! Restart-persistent notification history plus the process-local notices that
//! are currently projected above OneCopy's viewing surfaces.
//!
//! Recent history belongs in the reconstructible index database. Live notices
//! do not: they are presentation state for this process and disappear at
//! restart, while diagnostic history is retained independently of the inbox.

use std::sync::{LazyLock, Mutex};

use chrono::{Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::AppHandle;

const RECENT_LIMIT: i64 = 500;
const RECENT_DAYS: i64 = 30;

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
    /// shows. Kept for restart-persistent history and for a row with no
    /// descriptor; a window that redraws its live notices while this one is
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

fn parse_level(value: &str) -> NotificationLevel {
    match value {
        "error" => NotificationLevel::Error,
        "warning" => NotificationLevel::Warning,
        _ => NotificationLevel::Info,
    }
}

fn parse_presentation(value: &str) -> NotificationPresentation {
    if value == "persistent" {
        NotificationPresentation::Persistent
    } else {
        NotificationPresentation::Timed
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

fn record_recent(
    conn: &Connection,
    request: &NotificationRequest,
) -> Result<NotificationRecord, String> {
    validate(request)?;
    let now = crate::logging::now_iso_millis();
    let cutoff = (Utc::now() - Duration::days(RECENT_DAYS))
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let transaction = conn
        .unchecked_transaction()
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
    let record = transaction
        .query_row(
            "INSERT INTO recent_notifications
               (kind, path, level, presentation, message, message_key, message_values,
                first_seen_utc, last_seen_utc, occurrence_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 1)
             ON CONFLICT (kind, path, level, presentation, message) DO UPDATE SET
               last_seen_utc = excluded.last_seen_utc,
               message_key = excluded.message_key,
               message_values = excluded.message_values,
               occurrence_count = recent_notifications.occurrence_count + 1
             RETURNING id, kind, path, level, presentation, message, message_key,
                       message_values, first_seen_utc, last_seen_utc, occurrence_count",
            params![
                request.kind,
                request.path.as_deref().unwrap_or(""),
                level_name(request.level),
                presentation_name(request.presentation),
                request.message,
                request.message_key,
                message_values_json,
                now,
            ],
            |row| {
                let path: String = row.get(2)?;
                let level: String = row.get(3)?;
                let presentation: String = row.get(4)?;
                let message_values: Option<String> = row.get(7)?;
                Ok(NotificationRecord {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    path: (!path.is_empty()).then_some(path),
                    level: parse_level(&level),
                    presentation: parse_presentation(&presentation),
                    message: row.get(5)?,
                    message_key: row.get(6)?,
                    message_values: message_values.and_then(|json| serde_json::from_str(&json).ok()),
                    first_seen_utc: row.get(8)?,
                    last_seen_utc: row.get(9)?,
                    occurrence_count: row.get::<_, i64>(10)?.max(1) as u64,
                })
            },
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM recent_notifications WHERE last_seen_utc < ?1",
            [cutoff],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM recent_notifications
             WHERE id NOT IN (
               SELECT id FROM recent_notifications
               ORDER BY last_seen_utc DESC, id DESC LIMIT ?1
             )",
            [RECENT_LIMIT],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(record)
}

fn remember_active(record: NotificationRecord) {
    let mut active = ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = active.iter_mut().find(|item| item.id == record.id) {
        *existing = record;
    } else {
        active.push(record);
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
    let _ = record_recent(&conn, &fallback)?;
    Ok(())
}

pub fn publish(app: &AppHandle, request: NotificationRequest) -> Result<NotificationRecord, String> {
    let root = crate::paths::data_root()?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let record = record_recent(&conn, &request)?;
    remember_active(record.clone());
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
    record_recent(&conn, &request)
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

pub fn recent(conn: &Connection, limit: u32) -> Result<(u64, Vec<NotificationRecord>), String> {
    let total = conn
        .query_row("SELECT COUNT(*) FROM recent_notifications", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|error| error.to_string())?;
    let mut statement = conn
        .prepare(
            "SELECT id, kind, path, level, presentation, message, message_key, message_values,
                    first_seen_utc, last_seen_utc, occurrence_count
             FROM recent_notifications
             ORDER BY last_seen_utc DESC, id DESC LIMIT ?1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([limit], |row| {
            let path: String = row.get(2)?;
            let level: String = row.get(3)?;
            let presentation: String = row.get(4)?;
            let message_values: Option<String> = row.get(7)?;
            Ok(NotificationRecord {
                id: row.get(0)?,
                kind: row.get(1)?,
                path: (!path.is_empty())
                    .then(|| crate::winpath::for_display(&path).into_owned()),
                level: parse_level(&level),
                presentation: parse_presentation(&presentation),
                message: row.get(5)?,
                message_key: row.get(6)?,
                message_values: message_values.and_then(|json| serde_json::from_str(&json).ok()),
                first_seen_utc: row.get(8)?,
                last_seen_utc: row.get(9)?,
                occurrence_count: row.get::<_, i64>(10)?.max(1) as u64,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok((total.max(0) as u64, rows))
}

#[cfg(test)]
// EXCEPTION (tests-folder conventions): retention constants and the
// process-local live owner are private implementation state.
#[path = "../tests/unit/notifications.rs"]
mod tests;
