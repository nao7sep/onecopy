//! `records.sqlite3`: what happened, kept as evidence (data-lifecycle
//! conventions). The app process is its only writer.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::activity::ActivityKind;

pub const RECORDS_DB_FILE_NAME: &str = "records.sqlite3";
const SCHEMA_VERSION: i64 = 1;
/// The data-lifecycle conventions' transient-record age, fixed in a desktop app.
const TRANSIENT_DAYS: i64 = 90;

/// Activity kinds nobody reads three months later: the operation's outcome,
/// failures and changes stay, while its start and progress ticks go.
const TRANSIENT_ACTIVITY_KINDS: [ActivityKind; 2] = [ActivityKind::Started, ActivityKind::Progressed];

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    static JOURNAL: crate::sqlite::JournalSetup = crate::sqlite::JournalSetup::new();
    JOURNAL.configure(&connection, std::time::Duration::from_secs(5))?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| error.to_string())?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version > SCHEMA_VERSION {
        return Err("The records were written by a newer OneCopy version.".into());
    }
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS activity_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL,
        sequence INTEGER NOT NULL,
        event_time_utc TEXT NOT NULL,
        monotonic_ms INTEGER NOT NULL,
        operation_id TEXT,
        owner TEXT NOT NULL,
        kind TEXT NOT NULL,
        draft_json TEXT NOT NULL,
        user_visible INTEGER NOT NULL,
        UNIQUE(session_id, sequence)
    );
    CREATE INDEX IF NOT EXISTS activity_events_operation ON activity_events(session_id, operation_id, id);
    CREATE INDEX IF NOT EXISTS activity_events_time ON activity_events(id DESC);
    CREATE INDEX IF NOT EXISTS activity_events_kind_time ON activity_events(kind, event_time_utc);
    CREATE TABLE IF NOT EXISTS activity_operations (
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
    CREATE INDEX IF NOT EXISTS activity_operations_changed ON activity_operations(last_id);
    CREATE TRIGGER IF NOT EXISTS activity_project_insert AFTER INSERT ON activity_events WHEN NEW.user_visible = 1 BEGIN
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
    PRAGMA user_version = 1;";

/// Deletes transient records older than the fixed age, and clears the
/// operation projection's links to the activity events it deleted.
pub fn purge_transient(connection: &Connection, now: chrono::DateTime<chrono::Utc>) -> Result<usize, String> {
    let cutoff = (now - chrono::Duration::days(TRANSIENT_DAYS))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();
    let kinds = TRANSIENT_ACTIVITY_KINDS
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let transaction = connection.unchecked_transaction().map_err(|error| error.to_string())?;
    transaction
        .execute(
            "CREATE TEMP TABLE purged_activity AS
             SELECT id, session_id, operation_id FROM activity_events
             WHERE kind IN (?1, ?2) AND event_time_utc < ?3",
            params![kinds[0], kinds[1], cutoff],
        )
        .map_err(|error| error.to_string())?;
    let purged = transaction
        .execute("DELETE FROM activity_events WHERE id IN (SELECT id FROM temp.purged_activity)", [])
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "DELETE FROM activity_operations
               WHERE operation_id IS NULL AND first_id IN (SELECT id FROM temp.purged_activity);
             CREATE TEMP TABLE affected_operations AS
               SELECT DISTINCT o.first_id FROM activity_operations o
               JOIN temp.purged_activity p ON p.session_id = o.session_id AND p.operation_id = o.operation_id;
             DELETE FROM activity_operations
               WHERE first_id IN (SELECT first_id FROM temp.affected_operations)
               AND NOT EXISTS (SELECT 1 FROM activity_events e
                 WHERE e.session_id = activity_operations.session_id
                 AND e.operation_id = activity_operations.operation_id AND e.user_visible = 1);
             UPDATE activity_operations SET
               started_id = CASE WHEN started_id IN (SELECT id FROM temp.purged_activity) THEN NULL ELSE started_id END,
               progress_id = CASE WHEN progress_id IN (SELECT id FROM temp.purged_activity) THEN NULL ELSE progress_id END,
               first_id = (SELECT MIN(e.id) FROM activity_events e WHERE e.session_id = activity_operations.session_id
                 AND e.operation_id = activity_operations.operation_id AND e.user_visible = 1),
               last_id = (SELECT MAX(e.id) FROM activity_events e WHERE e.session_id = activity_operations.session_id
                 AND e.operation_id = activity_operations.operation_id AND e.user_visible = 1),
               event_count = (SELECT COUNT(*) FROM activity_events e WHERE e.session_id = activity_operations.session_id
                 AND e.operation_id = activity_operations.operation_id AND e.user_visible = 1)
               WHERE first_id IN (SELECT first_id FROM temp.affected_operations);
             DROP TABLE temp.purged_activity;
             DROP TABLE temp.affected_operations;",
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(purged)
}
