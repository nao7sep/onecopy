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
    pub message: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: i64,
    pub kind: String,
    pub path: Option<String>,
    pub level: NotificationLevel,
    pub presentation: NotificationPresentation,
    pub message: String,
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
    if request.message.trim().is_empty() {
        return Err("notification message is required".to_string());
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
    if request.level != NotificationLevel::Info {
        crate::index_store::upsert_issue(
            &transaction, request.path.as_deref(), &request.kind, &request.message,
        )?;
    }
    let record = transaction
        .query_row(
            "INSERT INTO recent_notifications
               (kind, path, level, presentation, message, first_seen_utc,
                last_seen_utc, occurrence_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 1)
             ON CONFLICT (kind, path, level, presentation, message) DO UPDATE SET
               last_seen_utc = excluded.last_seen_utc,
               occurrence_count = recent_notifications.occurrence_count + 1
             RETURNING id, kind, path, level, presentation, message,
                       first_seen_utc, last_seen_utc, occurrence_count",
            params![
                request.kind,
                request.path.as_deref().unwrap_or(""),
                level_name(request.level),
                presentation_name(request.presentation),
                request.message,
                now,
            ],
            |row| {
                let path: String = row.get(2)?;
                let level: String = row.get(3)?;
                let presentation: String = row.get(4)?;
                Ok(NotificationRecord {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    path: (!path.is_empty()).then_some(path),
                    level: parse_level(&level),
                    presentation: parse_presentation(&presentation),
                    message: row.get(5)?,
                    first_seen_utc: row.get(6)?,
                    last_seen_utc: row.get(7)?,
                    occurrence_count: row.get::<_, i64>(8)?.max(1) as u64,
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

fn record_delivery_failure(app: &AppHandle, event: &str, error: &str) -> Result<(), String> {
    crate::logging::error(
        "notification event delivery failed",
        json!({ "event": event, "error": { "message": error } }),
    );
    let root = crate::paths::data_root(app)?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let fallback = NotificationRequest {
        kind: "event-delivery-failed".to_string(),
        path: Some(event.to_string()),
        level: NotificationLevel::Error,
        presentation: NotificationPresentation::Persistent,
        message: "OneCopy could not update part of the interface. Reload the window before continuing.".to_string(),
    };
    let _ = record_recent(&conn, &fallback)?;
    Ok(())
}

pub fn publish(app: &AppHandle, request: NotificationRequest) -> Result<NotificationRecord, String> {
    let root = crate::paths::data_root(app)?;
    let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let record = record_recent(&conn, &request)?;
    remember_active(record.clone());
    if let Err(message) =
        crate::failure_runtime::emit_checked(app, "notification://published", &record)
    {
        record_delivery_failure(app, "notification://published", &message)?;
    }
    Ok(record)
}

pub fn record_history(
    app: &AppHandle,
    request: NotificationRequest,
) -> Result<NotificationRecord, String> {
    let root = crate::paths::data_root(app)?;
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
            "SELECT id, kind, path, level, presentation, message,
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
            Ok(NotificationRecord {
                id: row.get(0)?,
                kind: row.get(1)?,
                path: (!path.is_empty())
                    .then(|| crate::winpath::for_display(&path).into_owned()),
                level: parse_level(&level),
                presentation: parse_presentation(&presentation),
                message: row.get(5)?,
                first_seen_utc: row.get(6)?,
                last_seen_utc: row.get(7)?,
                occurrence_count: row.get::<_, i64>(8)?.max(1) as u64,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok((total.max(0) as u64, rows))
}

#[cfg(test)]
mod tests {
    // EXCEPTION (tests-folder conventions): retention constants and the
    // process-local live owner are private implementation state.
    use super::*;

    #[test]
    fn repeated_recent_notice_coalesces_with_times_and_count() {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
        let request = NotificationRequest {
            kind: "read-failed".to_string(),
            path: Some("/photos/a.jpg".to_string()),
            level: NotificationLevel::Error,
            presentation: NotificationPresentation::Persistent,
            message: "Could not read the file.".to_string(),
        };
        let first = record_recent(&conn, &request).unwrap();
        let second = record_recent(&conn, &request).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.occurrence_count, 2);
        assert_eq!(recent(&conn, 500).unwrap().0, 1);
        let issues = crate::queries::issues(&conn, 10).unwrap();
        assert_eq!(issues.0, 1);
        assert_eq!(issues.1[0].occurrence_count, 2, "one occurrence per publication, not one per presentation channel");
    }

    #[test]
    fn informational_notices_are_not_issues_and_dismissal_does_not_erase_history() {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
        let mut request = NotificationRequest {
            kind: "operation".into(), path: None, level: NotificationLevel::Info,
            presentation: NotificationPresentation::Timed, message: "Source folders checked.".into(),
        };
        record_recent(&conn, &request).unwrap();
        assert_eq!(crate::queries::issues(&conn, 10).unwrap().0, 0);
        request.level = NotificationLevel::Warning;
        request.message = "Some files could not be checked.".into();
        record_recent(&conn, &request).unwrap();
        let old_id = crate::queries::issues(&conn, 10).unwrap().1[0].id;
        crate::index_store::dismiss_issues(&conn, None).unwrap();
        record_recent(&conn, &request).unwrap();
        let issues = crate::queries::issues(&conn, 10).unwrap();
        assert_ne!(issues.1[0].id, old_id);
        assert_eq!(issues.1[0].occurrence_count, 1);
        assert_eq!(recent(&conn, 10).unwrap().0, 2);
    }

    #[test]
    fn notification_and_issue_recording_fail_as_one_transaction() {
        for blocked in ["issues", "recent_notifications"] {
            let directory = tempfile::tempdir().unwrap();
            let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
            conn.execute_batch(&format!("CREATE TRIGGER reject_insert BEFORE INSERT ON {blocked} BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;")).unwrap();
            assert!(record_recent(&conn, &NotificationRequest {
                kind: "failed".into(), path: None, level: NotificationLevel::Error,
                presentation: NotificationPresentation::Persistent, message: "Failed action.".into(),
            }).is_err());
            assert_eq!(crate::queries::issues(&conn, 10).unwrap().0, 0);
            assert_eq!(recent(&conn, 10).unwrap().0, 0);
        }
    }

    #[test]
    fn recent_history_prunes_old_and_over_limit_rows_after_a_write() {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&directory.path().join("index.sqlite3")).unwrap();
        let now = crate::logging::now_iso_millis();
        let transaction = conn.unchecked_transaction().unwrap();
        transaction
            .execute(
                "INSERT INTO recent_notifications
                   (kind, path, level, presentation, message, first_seen_utc,
                    last_seen_utc, occurrence_count)
                 VALUES ('old', '', 'warning', 'timed', 'old',
                         '2000-01-01T00:00:00.000Z', '2000-01-01T00:00:00.000Z', 1)",
                [],
            )
            .unwrap();
        for index in 0..500 {
            transaction
                .execute(
                    "INSERT INTO recent_notifications
                       (kind, path, level, presentation, message, first_seen_utc,
                        last_seen_utc, occurrence_count)
                     VALUES (?1, '', 'info', 'timed', ?1,
                             ?2, ?2, 1)",
                    params![format!("notice-{index}"), now],
                )
                .unwrap();
        }
        transaction.commit().unwrap();

        record_recent(
            &conn,
            &NotificationRequest {
                kind: "newest".to_string(),
                path: None,
                level: NotificationLevel::Info,
                presentation: NotificationPresentation::Timed,
                message: "newest".to_string(),
            },
        )
        .unwrap();

        assert_eq!(recent(&conn, 500).unwrap().0, 500);
        let old: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM recent_notifications WHERE kind = 'old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old, 0);
    }
}
