//! What the Records window reads from `records.sqlite3`: a filtered page of
//! summaries, newest first, and one record whole. Each read opens its own
//! read-only connection, so a read never writes a record and never waits
//! behind a writer's connection. Nothing here logs on success: a log line is
//! itself a stored record, whose signal would start the next read.

use std::cmp::Ordering;
use std::path::Path;
use std::time::Duration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

pub const PAGE_SIZE: usize = 100;

/// The tables of `records.sqlite3`, one record kind each.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecordKind {
    Log,
    Activity,
    Trash,
    Analysis,
    Issue,
    Notice,
}

impl RecordKind {
    /// The kind's name as the list orders it: records at the same time sort
    /// by this name, newest first.
    pub fn as_str(self) -> &'static str {
        match self {
            RecordKind::Log => "log",
            RecordKind::Activity => "activity",
            RecordKind::Trash => "trash",
            RecordKind::Analysis => "analysis",
            RecordKind::Issue => "issue",
            RecordKind::Notice => "notice",
        }
    }

    fn table(self) -> &'static Table {
        TABLES
            .iter()
            .find(|table| table.kind == self)
            .expect("every record kind has a table")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RecordLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// What the level filter offers: a record's own level, or `attention`, every
/// record at `warn` or `error`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LevelFilter {
    Attention,
    Debug,
    Info,
    Warn,
    Error,
}

/// Where the next page starts: the last summary of the page before it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub time: String,
    pub kind: RecordKind,
    pub id: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordsQuery {
    /// A launch, named by its session.
    pub session: Option<String>,
    pub kind: Option<RecordKind>,
    pub level: Option<LevelFilter>,
    pub search: String,
    pub after: Option<Cursor>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordSummary {
    pub kind: RecordKind,
    pub id: i64,
    pub session: Option<String>,
    pub time: String,
    pub level: RecordLevel,
    pub title: String,
    pub text: Option<String>,
    /// An Issue's or a notification's sentence, as the key and the JSON values
    /// it was recorded with; the window renders it in the interface language.
    pub message_key: Option<String>,
    pub message_values: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordsPage {
    pub records: Vec<RecordSummary>,
    pub more: bool,
}

/// One stored column, as the database holds it.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordField {
    pub name: String,
    pub value: JsonValue,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordDetail {
    #[serde(flatten)]
    pub summary: RecordSummary,
    /// Every column of the row except its id, in table order.
    pub fields: Vec<RecordField>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordSources {
    pub current_session: Option<String>,
    /// Every launch that has records, newest first.
    pub sessions: Vec<String>,
}

/// How one table reads as a summary. Levels are the logging levels: a table
/// without a level of its own reads as `error` for a failure, `warn` for an
/// open Issue, and `info` otherwise.
struct Table {
    kind: RecordKind,
    name: &'static str,
    time: &'static str,
    level: &'static str,
    title: &'static str,
    text: &'static str,
    /// The recorded sentence's key and values, where the table has them.
    sentence: (&'static str, &'static str),
    searched: &'static [&'static str],
}

const NO_SENTENCE: (&str, &str) = ("NULL", "NULL");

const LOG_ERROR_TEXT: &str = "CASE WHEN json_valid(line) THEN CASE json_type(line, '$.error') \
     WHEN 'text' THEN json_extract(line, '$.error') \
     WHEN 'object' THEN json_extract(line, '$.error.message') \
     ELSE json_extract(line, '$.op') END END";

const TABLES: [Table; 6] = [
    Table {
        kind: RecordKind::Log,
        name: "log_lines",
        time: "time_utc",
        level: "level",
        title: "message",
        text: LOG_ERROR_TEXT,
        sentence: NO_SENTENCE,
        searched: &["message", "line"],
    },
    Table {
        kind: RecordKind::Activity,
        name: "activity_events",
        time: "event_time_utc",
        level: "CASE kind WHEN '\"failed\"' THEN 'error' ELSE 'info' END",
        title: "trim(owner, '\"') || ' ' || trim(kind, '\"')",
        text: "COALESCE(CASE WHEN json_valid(draft_json) THEN json_extract(draft_json, '$.subject') END, operation_id)",
        sentence: NO_SENTENCE,
        searched: &["owner", "kind", "operation_id", "draft_json"],
    },
    Table {
        kind: RecordKind::Trash,
        name: "trash_actions",
        time: "time_utc",
        level: "CASE WHEN action LIKE '%-failed' THEN 'error' ELSE 'info' END",
        title: "action",
        text: "COALESCE(original_path, stored_path)",
        sentence: NO_SENTENCE,
        searched: &["action", "operation_id", "content_hash", "original_path", "stored_path", "detail_json"],
    },
    Table {
        kind: RecordKind::Analysis,
        name: "analysis_events",
        time: "time_utc",
        level: "CASE event WHEN 'failed' THEN 'error' ELSE 'info' END",
        title: "class || ' ' || event",
        text: "COALESCE(message, path)",
        sentence: NO_SENTENCE,
        searched: &["class", "event", "model", "model_version", "path", "message", "content_hash"],
    },
    Table {
        kind: RecordKind::Issue,
        name: "issue_events",
        time: "time_utc",
        level: "CASE event WHEN 'occurred' THEN 'warn' ELSE 'info' END",
        title: "kind || ' ' || event",
        text: "COALESCE(NULLIF(message, ''), NULLIF(path, ''))",
        sentence: ("message_key", "message_values"),
        searched: &["kind", "path", "event", "message", "message_key", "message_values"],
    },
    Table {
        kind: RecordKind::Notice,
        name: "notices",
        time: "time_utc",
        level: "CASE level WHEN 'error' THEN 'error' WHEN 'warning' THEN 'warn' ELSE 'info' END",
        title: "kind",
        text: "COALESCE(NULLIF(message, ''), path)",
        sentence: ("message_key", "message_values"),
        searched: &["kind", "path", "level", "presentation", "message", "message_key", "message_values"],
    },
];

/// Opens the records for reading only, with a bounded wait on a lock.
pub fn open_reader(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    match crate::formats::sqlite_marker(&connection, path, crate::formats::RECORDS)? {
        crate::formats::SqliteMarker::Newer(newer) => return Err(newer.to_string()),
        crate::formats::SqliteMarker::Missing => return Err(crate::formats::missing_marker(path)),
        crate::formats::SqliteMarker::New | crate::formats::SqliteMarker::Current => {}
    }
    Ok(connection)
}

fn summary_select(table: &Table) -> String {
    format!(
        "SELECT id, session_id, {time}, {level}, COALESCE({title}, ''), CAST({text} AS TEXT), {key}, {values} FROM {name}",
        time = table.time,
        level = table.level,
        title = table.title,
        text = table.text,
        key = table.sentence.0,
        values = table.sentence.1,
        name = table.name,
    )
}

fn summary_from_row(kind: RecordKind, row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordSummary> {
    let level: String = row.get(3)?;
    Ok(RecordSummary {
        kind,
        id: row.get(0)?,
        session: row.get(1)?,
        time: row.get(2)?,
        level: match level.as_str() {
            "debug" => RecordLevel::Debug,
            "warn" => RecordLevel::Warn,
            "error" => RecordLevel::Error,
            _ => RecordLevel::Info,
        },
        title: row.get(4)?,
        text: row.get(5)?,
        message_key: row.get(6)?,
        message_values: row.get(7)?,
    })
}

fn like_pattern(search: &str) -> Option<String> {
    let trimmed = search.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut pattern = String::from("%");
    for character in trimmed.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    Some(pattern)
}

/// The newest summaries of one table that match the query, at most one more
/// than a page, read through the table's (time, id) index.
fn table_page(connection: &Connection, table: &Table, query: &RecordsQuery) -> Result<Vec<RecordSummary>, String> {
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<SqlValue> = Vec::new();
    if let Some(session) = &query.session {
        conditions.push("session_id = ?".into());
        params.push(SqlValue::Text(session.clone()));
    }
    match query.level {
        None => {}
        Some(LevelFilter::Attention) => conditions.push(format!("({}) IN ('warn', 'error')", table.level)),
        Some(level) => {
            conditions.push(format!("({}) = ?", table.level));
            let name = match level {
                LevelFilter::Debug => "debug",
                LevelFilter::Info => "info",
                LevelFilter::Warn => "warn",
                LevelFilter::Error | LevelFilter::Attention => "error",
            };
            params.push(SqlValue::Text(name.into()));
        }
    }
    if let Some(pattern) = like_pattern(&query.search) {
        let any = table
            .searched
            .iter()
            .map(|column| format!("{column} LIKE ? ESCAPE '\\'"))
            .collect::<Vec<_>>()
            .join(" OR ");
        conditions.push(format!("({any})"));
        params.extend(table.searched.iter().map(|_| SqlValue::Text(pattern.clone())));
    }
    if let Some(after) = &query.after {
        // The list's order is time, then kind name, then id, all newest
        // first; this table's rows after the cursor follow from where its
        // kind sorts against the cursor's.
        conditions.push(match table.kind.as_str().cmp(after.kind.as_str()) {
            Ordering::Less => format!("{} <= ?", table.time),
            Ordering::Equal => format!("({}, id) < (?, ?)", table.time),
            Ordering::Greater => format!("{} < ?", table.time),
        });
        params.push(SqlValue::Text(after.time.clone()));
        if table.kind == after.kind {
            params.push(SqlValue::Integer(after.id));
        }
    }
    params.push(SqlValue::Integer((PAGE_SIZE + 1) as i64));
    let filter = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let sql = format!(
        "{select}{filter} ORDER BY {time} DESC, id DESC LIMIT ?",
        select = summary_select(table),
        time = table.time,
    );
    let mut statement = connection.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(params), |row| summary_from_row(table.kind, row))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

/// The order the list shows records in, newest first.
pub fn newest_first(a: &RecordSummary, b: &RecordSummary) -> Ordering {
    b.time
        .cmp(&a.time)
        .then_with(|| b.kind.as_str().cmp(a.kind.as_str()))
        .then_with(|| b.id.cmp(&a.id))
}

/// One page of summaries across every table the query asks for. Each table
/// gives at most one more than a page, so the merged first page is exact.
pub fn page(connection: &Connection, query: &RecordsQuery) -> Result<RecordsPage, String> {
    let mut records = Vec::new();
    for table in TABLES.iter().filter(|table| query.kind.is_none_or(|kind| kind == table.kind)) {
        records.extend(table_page(connection, table, query)?);
    }
    records.sort_by(newest_first);
    let more = records.len() > PAGE_SIZE;
    records.truncate(PAGE_SIZE);
    Ok(RecordsPage { records, more })
}

fn json_of(value: SqlValue) -> JsonValue {
    match value {
        SqlValue::Null => JsonValue::Null,
        SqlValue::Integer(number) => JsonValue::from(number),
        SqlValue::Real(number) => JsonValue::from(number),
        SqlValue::Text(text) => JsonValue::String(text),
        SqlValue::Blob(bytes) => JsonValue::String(bytes.iter().map(|byte| format!("{byte:02x}")).collect()),
    }
}

/// One record whole: its summary, and every column as stored.
pub fn detail(connection: &Connection, kind: RecordKind, id: i64) -> Result<Option<RecordDetail>, String> {
    use rusqlite::OptionalExtension;
    let table = kind.table();
    let summary = connection
        .query_row(&format!("{} WHERE id = ?1", summary_select(table)), [id], |row| {
            summary_from_row(kind, row)
        })
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(summary) = summary else { return Ok(None) };
    let mut statement = connection
        .prepare(&format!("SELECT * FROM {} WHERE id = ?1", table.name))
        .map_err(|error| error.to_string())?;
    let names: Vec<String> = statement.column_names().into_iter().map(str::to_string).collect();
    let values = statement
        .query_row([id], |row| {
            (0..names.len()).map(|index| row.get::<_, SqlValue>(index)).collect::<rusqlite::Result<Vec<_>>>()
        })
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(values) = values else { return Ok(None) };
    let fields = names
        .into_iter()
        .zip(values)
        .filter(|(name, _)| name != "id")
        .map(|(name, value)| RecordField { name, value: json_of(value) })
        .collect();
    Ok(Some(RecordDetail { summary, fields }))
}

/// Every launch that has records, newest first. Read once per window.
pub fn sources(connection: &Connection, current_session: Option<&str>) -> Result<RecordSources, String> {
    let sql = TABLES
        .iter()
        .map(|table| format!("SELECT session_id FROM {} WHERE session_id IS NOT NULL", table.name))
        .collect::<Vec<_>>()
        .join(" UNION ");
    let mut statement = connection
        .prepare(&format!("{sql} ORDER BY 1 DESC"))
        .map_err(|error| error.to_string())?;
    let sessions = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(RecordSources {
        current_session: current_session.map(str::to_string),
        sessions,
    })
}
