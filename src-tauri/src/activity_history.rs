//! Read projection of retained Activity events. It has no work-control state.
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::activity::ActivityEvent;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub id: i64,
    pub first: ActivityEvent,
    pub latest: ActivityEvent,
    pub started: Option<ActivityEvent>,
    pub progress: Option<ActivityEvent>,
    pub event_count: u64,
    pub target_hash: Option<String>,
    pub target: Option<Target>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub name: String,
    pub path: String,
}

/// Resolve presentation only on reads; private paths never enter Activity events.
pub fn resolve_targets(conn: &Connection, rows: &mut [Operation]) -> Result<(), String> {
    use rusqlite::OptionalExtension;
    let mut query = conn
        .prepare(
            "SELECT p.file_name, p.abs_path FROM review_contents l
        JOIN paths p ON p.id = l.representative_path_id WHERE l.content_hash = ?1",
        )
        .map_err(|e| e.to_string())?;
    for row in rows {
        if let Some(hash) = &row.target_hash {
            row.target = query
                .query_row([hash], |r| {
                    Ok(Target {
                        name: r.get(0)?,
                        path: r.get(1)?,
                    })
                })
                .optional()
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationPage {
    pub operations: Vec<Operation>,
    pub next_cursor: Option<i64>,
    pub revision: i64,
    pub has_more: bool,
    pub session_id: String,
    pub monotonic_now_ms: u64,
}

pub fn initialize(conn: &Connection) -> Result<(), String> {
    let revision: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if revision > 1 {
        return Err("Activity history was written by a newer OneCopy version.".into());
    }
    if revision == 1 {
        return Ok(());
    }
    // The original events remain untouched. Both backfill and the writer's
    // projection trigger commit together, so an interrupted upgrade is retriable.
    conn.execute_batch("BEGIN IMMEDIATE;
        ALTER TABLE activity_events ADD COLUMN user_visible INTEGER NOT NULL DEFAULT 1;
        UPDATE activity_events SET user_visible = 0
          WHERE json_extract(draft_json, '$.owner') NOT IN
            ('sourceCheck','fileInformation','mutation','managedTools','settings','transcript');
        CREATE TABLE activity_operations (
            first_id INTEGER PRIMARY KEY,
            last_id INTEGER NOT NULL,
            started_id INTEGER,
            progress_id INTEGER,
            event_count INTEGER NOT NULL,
            session_id TEXT NOT NULL,
            operation_id TEXT,
            target_hash TEXT,
            UNIQUE(session_id, operation_id)
        );
        CREATE INDEX activity_operations_changed ON activity_operations(last_id);
        INSERT INTO activity_operations
          SELECT MIN(id), MAX(id),
            MIN(CASE WHEN json_extract(draft_json, '$.kind') IN ('admitted','queued','started','opened') THEN id END),
            MAX(CASE WHEN json_extract(draft_json, '$.done') IS NOT NULL OR json_extract(draft_json, '$.itemCount') IS NOT NULL THEN id END),
            COUNT(*), session_id, operation_id, NULL
          FROM activity_events WHERE user_visible = 1
          GROUP BY session_id, operation_id, CASE WHEN operation_id IS NULL THEN id ELSE 0 END;
        CREATE TRIGGER activity_project_insert AFTER INSERT ON activity_events WHEN NEW.user_visible = 1 BEGIN
          INSERT INTO activity_operations(first_id, last_id, started_id, progress_id, event_count, session_id, operation_id, target_hash)
          VALUES (NEW.id, NEW.id,
            CASE WHEN json_extract(NEW.draft_json, '$.kind') IN ('admitted','queued','started','opened') THEN NEW.id END,
            CASE WHEN json_extract(NEW.draft_json, '$.done') IS NOT NULL OR json_extract(NEW.draft_json, '$.itemCount') IS NOT NULL THEN NEW.id END,
            1, NEW.session_id, NEW.operation_id, json_extract(NEW.draft_json, '$.targetHash'))
          ON CONFLICT(session_id, operation_id) DO UPDATE SET
            last_id = excluded.last_id,
            started_id = COALESCE(activity_operations.started_id, excluded.started_id),
            progress_id = COALESCE(excluded.progress_id, activity_operations.progress_id),
            event_count = activity_operations.event_count + 1,
            target_hash = COALESCE(excluded.target_hash, activity_operations.target_hash);
        END;
        PRAGMA user_version = 1;
        COMMIT;")
        .map_err(|error| {
            if !conn.is_autocommit() {
                if let Err(rollback) = conn.execute_batch("ROLLBACK") {
                    crate::logging::error("activity upgrade rollback failed", serde_json::json!({"error": rollback.to_string()}));
                }
            }
            error.to_string()
        })
}

pub fn event_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<ActivityEvent> {
    let draft: String = row.get(offset + 5)?;
    Ok(ActivityEvent {
        event_id: row.get(offset)?,
        session_id: row.get(offset + 1)?,
        sequence: row.get::<_, i64>(offset + 2)? as u64,
        event_time_utc: row.get(offset + 3)?,
        monotonic_ms: row.get::<_, i64>(offset + 4)? as u64,
        draft: serde_json::from_str(&draft).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                offset + 5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
    })
}

pub fn operations(
    conn: &Connection,
    before: Option<i64>,
    after: Option<i64>,
    limit: usize,
    session_id: String,
    monotonic_now_ms: u64,
) -> Result<OperationPage, String> {
    if before.is_some() && after.is_some() {
        return Err("Activity paging needs one direction.".into());
    }
    let limit = limit.clamp(1, 100);
    let (condition, order, cursor) = match after {
        Some(after) => ("o.last_id > ?1", "o.last_id ASC", after),
        None => (
            "o.first_id < ?1",
            "o.first_id DESC",
            before.unwrap_or(i64::MAX),
        ),
    };
    let sql = format!(
        "SELECT o.first_id, o.event_count, o.target_hash,
        f.id, f.session_id, f.sequence, f.event_time_utc, f.monotonic_ms, f.draft_json,
        l.id, l.session_id, l.sequence, l.event_time_utc, l.monotonic_ms, l.draft_json,
        s.id, s.session_id, s.sequence, s.event_time_utc, s.monotonic_ms, s.draft_json,
        p.id, p.session_id, p.sequence, p.event_time_utc, p.monotonic_ms, p.draft_json
        FROM activity_operations o
        JOIN activity_events f ON f.id = o.first_id
        JOIN activity_events l ON l.id = o.last_id
        LEFT JOIN activity_events s ON s.id = o.started_id
        LEFT JOIN activity_events p ON p.id = o.progress_id
        WHERE {condition} ORDER BY {order} LIMIT ?2"
    );
    let mut statement = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut operations = statement
        .query_map(params![cursor, (limit + 1) as i64], |row| {
            Ok(Operation {
                id: row.get(0)?,
                event_count: row.get::<_, i64>(1)? as u64,
                target_hash: row.get(2)?,
                target: None,
                first: event_from_row(row, 3)?,
                latest: event_from_row(row, 9)?,
                started: if row.get::<_, Option<i64>>(15)?.is_some() {
                    Some(event_from_row(row, 15)?)
                } else {
                    None
                },
                progress: if row.get::<_, Option<i64>>(21)?.is_some() {
                    Some(event_from_row(row, 21)?)
                } else {
                    None
                },
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let has_more = operations.len() > limit;
    operations.truncate(limit);
    let revision = if after.is_some() && has_more {
        operations
            .last()
            .map(|row| row.latest.event_id)
            .unwrap_or(cursor)
    } else {
        conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM activity_events",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?
    };
    let next_cursor = if after.is_none() && has_more {
        operations.last().map(|row| row.id)
    } else {
        None
    };
    Ok(OperationPage {
        operations,
        next_cursor,
        revision,
        has_more,
        session_id,
        monotonic_now_ms,
    })
}

pub fn events(
    conn: &Connection,
    operation: i64,
    before: Option<i64>,
    limit: usize,
) -> Result<(Vec<ActivityEvent>, Option<i64>), String> {
    let limit = limit.clamp(1, 100);
    // The operation-index seek includes events outside any loaded summary page.
    let mut statement = conn
        .prepare(
            "SELECT e.id, e.session_id, e.sequence, e.event_time_utc, e.monotonic_ms, e.draft_json
        FROM activity_operations o JOIN activity_events e
        ON e.session_id = o.session_id AND e.operation_id = o.operation_id
        WHERE o.first_id = ?1 AND e.id < ?2 AND e.user_visible = 1
        UNION ALL
        SELECT e.id, e.session_id, e.sequence, e.event_time_utc, e.monotonic_ms, e.draft_json
        FROM activity_operations o JOIN activity_events e ON e.id = o.first_id
        WHERE o.first_id = ?1 AND o.operation_id IS NULL AND e.id < ?2
        ORDER BY id DESC LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = statement
        .query_map(
            params![operation, before.unwrap_or(i64::MAX), (limit + 1) as i64],
            |row| event_from_row(row, 0),
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let cursor = if more {
        rows.last().map(|row| row.event_id)
    } else {
        None
    };
    Ok((rows, cursor))
}
