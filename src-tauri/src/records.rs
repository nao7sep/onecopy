//! `records.sqlite3`: what happened, kept as evidence (data-lifecycle
//! conventions). The app process is its only writer.

use std::path::Path;
use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

use rusqlite::{params, Connection};
use serde_json::{json, Value as JsonValue};

use crate::activity::ActivityKind;

pub const RECORDS_DB_FILE_NAME: &str = "records.sqlite3";
/// The data-lifecycle conventions' transient-record age, fixed in a desktop app.
const TRANSIENT_DAYS: i64 = 90;

/// Activity kinds nobody reads three months later: the operation's outcome,
/// failures and changes stay, while its start and progress ticks go.
const TRANSIENT_ACTIVITY_KINDS: [ActivityKind; 2] = [ActivityKind::Started, ActivityKind::Progressed];

struct Store {
    connection: Connection,
    session_id: String,
}

/// The process's writer for every record kind except activity, whose
/// recorder keeps its own connection to the same database.
static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Opens the records at launch and deletes the transient ones.
pub fn init(path: &Path, session_id: &str) {
    let opened = open(path).and_then(|connection| {
        purge_transient(&connection, chrono::Utc::now())?;
        Ok(connection)
    });
    match opened {
        Ok(connection) => {
            *STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Store {
                connection,
                session_id: session_id.to_string(),
            });
        }
        Err(error) => crate::logging::warn("records unavailable", json!({ "error": { "message": error } })),
    }
}

/// Writes one record. A record the database cannot take is kept as a log
/// line instead (data-lifecycle conventions).
fn write(
    record: &'static str,
    entry: JsonValue,
    insert: impl FnOnce(&Connection, &str, &str) -> rusqlite::Result<usize>,
) {
    let time = crate::logging::now_iso_millis();
    let result = match STORE.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
        Some(store) => insert(&store.connection, &store.session_id, &time)
            .map(|_| wrote(&store.connection))
            .map_err(|error| error.to_string()),
        None => Err("records are not open".to_string()),
    };
    if let Err(error) = result {
        crate::logging::warn(
            "record write failed",
            json!({ "record": record, "timeUtc": time, "entry": entry, "error": { "message": error } }),
        );
    }
}

/// One file moved to, restored from, or removed from Deleted files.
pub struct TrashAction<'a> {
    /// `trashed`, `trash-failed`, `restored`, `removed` or `remove-failed`.
    pub action: &'static str,
    pub operation_id: Option<&'a str>,
    pub content_hash: Option<&'a str>,
    pub original_path: Option<&'a str>,
    pub stored_path: &'a str,
    pub detail: JsonValue,
}

pub fn trash_action(action: TrashAction<'_>) {
    let entry = json!({
        "action": action.action,
        "operationId": action.operation_id,
        "contentHash": action.content_hash,
        "originalPath": action.original_path,
        "storedPath": action.stored_path,
        "detail": action.detail,
    });
    write("trash", entry, |connection, session_id, time| {
        connection.execute(
            "INSERT INTO trash_actions (session_id, time_utc, action, operation_id, content_hash, original_path, stored_path, detail_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id,
                time,
                action.action,
                action.operation_id,
                action.content_hash,
                action.original_path,
                action.stored_path,
                action.detail.to_string(),
            ],
        )
    });
}

/// Called after each commit that stored a record. Set once, at app setup.
static STORED: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

thread_local! {
    /// This thread wrote a record inside a transaction that has not yet
    /// committed. A transaction lives on the thread that runs it; one that
    /// rolls back leaves the mark to the thread's next commit, which then
    /// signals once more than needed.
    static UNCOMMITTED: Cell<bool> = const { Cell::new(false) };
}

/// Sets what runs after each commit that stores a record: the Records
/// window's live signal. The listener must not log, because a log line is
/// itself a stored record.
pub fn on_stored(listener: impl Fn() + Send + Sync + 'static) {
    if STORED.set(Box::new(listener)).is_err() {
        eprintln!("[onecopy:records] the stored-record listener is already set");
    }
}

fn signal_stored() {
    if let Some(listener) = STORED.get() {
        listener();
    }
}

/// Called by the code that just wrote a record through `connection`. Outside
/// a transaction the write has committed, so the record is readable now;
/// inside one it is readable once `commit` commits that transaction.
pub fn wrote(connection: &Connection) {
    if connection.is_autocommit() {
        signal_stored();
    } else {
        UNCOMMITTED.with(|uncommitted| uncommitted.set(true));
    }
}

/// Commits a transaction that may have written records, then signals them
/// stored. A transaction that wrote none signals nothing.
pub fn commit(transaction: rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.commit()?;
    if UNCOMMITTED.with(|uncommitted| uncommitted.replace(false)) {
        signal_stored();
    }
    Ok(())
}

/// Sets aside a records store that is malformed or has a schema without its
/// format marker, so the launch starts a fresh one (store-recovery
/// conventions). Records are history and diagnostics, not needed for
/// OneCopy's own job, and the rename keeps every byte. A store a newer OneCopy
/// wrote, an absent one, and one that merely cannot be opened right now (I/O,
/// permission) are left exactly as they are. Runs before anything else opens
/// the store. A rename that fails stops the launch naming the file.
pub fn set_aside_if_unreadable(path: &Path) -> Result<Option<crate::storage::QuarantineRecord>, String> {
    use rusqlite::ErrorCode;
    let malformed = |error: &rusqlite::Error| {
        matches!(
            error.sqlite_error_code(),
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
        )
    };
    if !path.exists() { // data root
        return Ok(None);
    }
    let unreadable = {
        // Read-only: a read-write probe of a damaged store lets SQLite discard
        // its write-ahead log, which may hold the newest records.
        let Ok(connection) = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return Ok(None);
        };
        match connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)) {
            Err(error) => malformed(&error),
            Ok(version) if version < 0 => true,
            Ok(version) if version > 0 => false,
            Ok(_) => match connection.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get::<_, i64>(0)) {
                Err(error) => malformed(&error),
                Ok(objects) => objects > 0,
            },
        }
    };
    if !unreadable {
        return Ok(None);
    }
    let set_aside = crate::storage::quarantine_name(path);
    // The write-ahead log and its index belong to the set-aside store: left
    // here, they would be read into the fresh one.
    for suffix in ["", "-wal", "-shm"] {
        let from = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
        if suffix.is_empty() || from.exists() { // data root
            let to = std::path::PathBuf::from(format!("{}{suffix}", set_aside.display()));
            crate::fs_publish::rename_no_replace(&from, &to).map_err(|error| {
                format!("could not set aside the unreadable {}: {error}", from.display())
            })?;
        }
    }
    Ok(Some(crate::storage::QuarantineRecord {
        file: RECORDS_DB_FILE_NAME.to_string(),
        quarantined_to: set_aside.to_string_lossy().into_owned(),
    }))
}

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    // Read before anything is set or written: records a newer OneCopy wrote
    // are left exactly as they are (store-recovery conventions).
    match crate::formats::sqlite_marker(&connection, path, crate::formats::RECORDS)? {
        crate::formats::SqliteMarker::Newer(newer) => return Err(newer.to_string()),
        crate::formats::SqliteMarker::Missing => return Err(crate::formats::missing_marker(path)),
        crate::formats::SqliteMarker::New | crate::formats::SqliteMarker::Current => {}
    }
    static JOURNAL: crate::sqlite::JournalSetup = crate::sqlite::JournalSetup::new();
    JOURNAL.configure(&connection, std::time::Duration::from_secs(5))?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| error.to_string())?;
    // The schema and its marker commit together, so no reader ever sees one
    // without the other.
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(SCHEMA)
        .map_err(|error| error.to_string())?;
    transaction
        .pragma_update(None, "user_version", crate::formats::RECORDS)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
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
    CREATE TABLE IF NOT EXISTS trash_actions (
        id INTEGER PRIMARY KEY,
        session_id TEXT NOT NULL,
        time_utc TEXT NOT NULL,
        action TEXT NOT NULL,
        operation_id TEXT,
        content_hash TEXT,
        original_path TEXT,
        stored_path TEXT NOT NULL,
        detail_json TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS trash_actions_content ON trash_actions(content_hash);
    CREATE INDEX IF NOT EXISTS trash_actions_time ON trash_actions(time_utc, id);
    -- Written through the index connection that attaches this database, in
    -- the same transaction as the index change it belongs to.
    CREATE TABLE IF NOT EXISTS analysis_events (
        id INTEGER PRIMARY KEY,
        session_id TEXT,
        time_utc TEXT NOT NULL,
        content_hash TEXT NOT NULL,
        class TEXT NOT NULL,
        -- 'failed', or 'reopened' when the user asked for a new attempt.
        event TEXT NOT NULL,
        model TEXT,
        model_version TEXT,
        path TEXT,
        message TEXT
    );
    CREATE INDEX IF NOT EXISTS analysis_events_latest ON analysis_events(content_hash, class, session_id, id);
    CREATE INDEX IF NOT EXISTS analysis_events_time ON analysis_events(time_utc, id);
    -- An Issue is the run of these events for one (kind, path) in one launch:
    -- each occurrence, then how it closed. Written like `analysis_events`.
    CREATE TABLE IF NOT EXISTS issue_events (
        id INTEGER PRIMARY KEY,
        session_id TEXT,
        time_utc TEXT NOT NULL,
        kind TEXT NOT NULL,
        -- '' for a condition with no file.
        path TEXT NOT NULL,
        -- 'occurred', or how the Issue closed: 'resolved', 'dismissed',
        -- 'rechecked' or 'rebuilt'.
        event TEXT NOT NULL,
        message TEXT,
        message_key TEXT,
        message_values TEXT
    );
    CREATE INDEX IF NOT EXISTS issue_events_identity ON issue_events(session_id, kind, path, id);
    CREATE INDEX IF NOT EXISTS issue_events_time ON issue_events(time_utc, id);
    -- Every log line, as the logging conventions shape it, kept for good.
    CREATE TABLE IF NOT EXISTS log_lines (
        id INTEGER PRIMARY KEY,
        session_id TEXT NOT NULL,
        time_utc TEXT NOT NULL,
        level TEXT NOT NULL,
        message TEXT NOT NULL,
        line TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS log_lines_session ON log_lines(session_id, id);
    CREATE INDEX IF NOT EXISTS log_lines_time ON log_lines(time_utc, id);
    -- Every notice OneCopy published or recorded, in the transaction of the
    -- Issue it raises.
    CREATE TABLE IF NOT EXISTS notices (
        id INTEGER PRIMARY KEY,
        session_id TEXT,
        time_utc TEXT NOT NULL,
        kind TEXT NOT NULL,
        path TEXT,
        level TEXT NOT NULL,
        presentation TEXT NOT NULL,
        message TEXT NOT NULL,
        message_key TEXT,
        message_values TEXT
    );
    CREATE INDEX IF NOT EXISTS notices_time ON notices(time_utc, id);
    -- The Records window reads every table newest first through these.
    CREATE INDEX IF NOT EXISTS activity_events_event_time ON activity_events(event_time_utc, id);";

/// Attaches the records beside an index database to its connection as
/// `records`, so an index transaction can write the records it produces.
pub fn attach(connection: &Connection, path: &Path) -> Result<(), String> {
    static READY: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());
    {
        let mut ready = READY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let known = ready.iter().any(|known| known == path);
        // A store deleted while OneCopy runs is created again with its schema,
        // not attached as an empty file that every records query then fails on.
        if !known || !path.exists() { // data root
            drop(open(path)?);
            if !known {
                ready.push(path.to_path_buf());
            }
        }
    }
    connection
        .execute("ATTACH DATABASE ?1 AS records", [path.to_string_lossy()])
        .map_err(|error| format!("attach records: {error}"))?;
    // This launch's open Issues: each (kind, path) whose latest event is an
    // occurrence, identified by the first occurrence since it last closed.
    connection
        .execute_batch(&format!(
            "CREATE TEMP VIEW IF NOT EXISTS active_issues AS
             WITH events AS (
               SELECT * FROM records.issue_events WHERE session_id IS {session}
             ), closed AS (
               SELECT kind, path, MAX(id) AS id FROM events WHERE event <> 'occurred' GROUP BY kind, path
             ), open AS (
               SELECT e.kind, e.path, MIN(e.id) AS id, MAX(e.id) AS latest, COUNT(*) AS occurrence_count,
                      MIN(e.time_utc) AS first_seen_utc, MAX(e.time_utc) AS last_seen_utc
               FROM events e LEFT JOIN closed c ON c.kind = e.kind AND c.path = e.path
               WHERE e.event = 'occurred' AND e.id > COALESCE(c.id, 0)
               GROUP BY e.kind, e.path
             )
             SELECT o.id, o.path, o.kind, l.message, l.message_key, l.message_values,
                    o.first_seen_utc, o.last_seen_utc, o.occurrence_count
             FROM open o JOIN records.issue_events l ON l.id = o.latest;",
            session = session_sql()
        ))
        .map_err(|error| format!("records views: {error}"))
}

/// This launch's session as an SQL literal, for statements that write or
/// read records through an attached index connection.
pub fn session_sql() -> String {
    sql_text(crate::logging::session_id())
}

pub fn sql_text(value: Option<&str>) -> String {
    value.map_or_else(|| "NULL".to_string(), |text| format!("'{}'", text.replace('\'', "''")))
}

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
    let transaction = crate::sqlite::write_transaction(connection).map_err(|error| error.to_string())?;
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
