// The Records window's reads of records.sqlite3: every table merged newest
// first, keyset paging across kinds, the filters, one record whole, and the
// launches the filter offers.

use onecopy_lib::records;
use onecopy_lib::records_view::{
    detail, open_reader, page, sources, Cursor, LevelFilter, RecordKind, RecordLevel, RecordsQuery, PAGE_SIZE,
};
use rusqlite::{params, Connection};
use serde_json::json;

const OLD: &str = "2026-10-01T08:00:00.000Z";
const NOW: &str = "2026-10-02T08:00:00.000Z";

struct Records {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    writer: Connection,
}

fn records() -> Records {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(records::RECORDS_DB_FILE_NAME);
    let writer = records::open(&path).unwrap();
    Records { _dir: dir, path, writer }
}

impl Records {
    fn reader(&self) -> Connection {
        open_reader(&self.path).unwrap()
    }

    fn log(&self, session: &str, time: &str, level: &str, message: &str, fields: serde_json::Value) -> i64 {
        let mut line = json!({ "time": time, "level": level, "message": message, "sessionId": session });
        line.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        self.writer
            .execute(
                "INSERT INTO log_lines (session_id, time_utc, level, message, line) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session, time, level, message, line.to_string()],
            )
            .unwrap();
        self.writer.last_insert_rowid()
    }

    fn activity(&self, time: &str, kind: &str) -> i64 {
        let draft = json!({ "kind": kind, "owner": "mutation", "subject": "deleteFiles", "operationId": "op-1" });
        self.writer
            .execute(
                "INSERT INTO activity_events (session_id, sequence, event_time_utc, monotonic_ms, operation_id, owner, kind, draft_json, user_visible)
                 VALUES (?1, (SELECT COUNT(*) + 1 FROM activity_events), ?2, 5, 'op-1', '\"mutation\"', ?3, ?4, 1)",
                params![NOW, time, format!("\"{kind}\""), draft.to_string()],
            )
            .unwrap();
        self.writer.last_insert_rowid()
    }

    fn trash(&self, time: &str, action: &str) -> i64 {
        self.writer
            .execute(
                "INSERT INTO trash_actions (session_id, time_utc, action, operation_id, content_hash, original_path, stored_path, detail_json)
                 VALUES (?1, ?2, ?3, 'op-1', 'hash-1', '/photos/a.jpg', '/photos/.onecopy-trash/a.jpg', '{\"reason\":\"user\"}')",
                params![NOW, time, action],
            )
            .unwrap();
        self.writer.last_insert_rowid()
    }

    fn analysis(&self, time: &str, event: &str) -> i64 {
        self.writer
            .execute(
                "INSERT INTO analysis_events (session_id, time_utc, content_hash, class, event, model, model_version, path, message)
                 VALUES (?1, ?2, 'hash-1', 'faces', ?3, 'ultraface', 'sha', '/photos/a.jpg', 'decode failed')",
                params![NOW, time, event],
            )
            .unwrap();
        self.writer.last_insert_rowid()
    }

    fn issue(&self, session: Option<&str>, time: &str, event: &str) -> i64 {
        self.writer
            .execute(
                "INSERT INTO issue_events (session_id, time_utc, kind, path, event, message, message_key, message_values)
                 VALUES (?1, ?2, 'preview-failed', '/photos/a.jpg', ?3, 'no decoder', 'notice.previewFailed', '{\"name\":\"a.jpg\"}')",
                params![session, time, event],
            )
            .unwrap();
        self.writer.last_insert_rowid()
    }

    fn notice(&self, time: &str, level: &str) -> i64 {
        self.writer
            .execute(
                "INSERT INTO notices (session_id, time_utc, kind, path, level, presentation, message, message_key, message_values)
                 VALUES (?1, ?2, 'preview-failed', NULL, ?3, 'persistent', '', 'notice.previewFailed', NULL)",
                params![NOW, time, level],
            )
            .unwrap();
        self.writer.last_insert_rowid()
    }
}

fn at(second: u32) -> String {
    format!("2026-10-02T08:00:{second:02}.000Z")
}

fn query() -> RecordsQuery {
    RecordsQuery::default()
}

fn keys(records: &[onecopy_lib::records_view::RecordSummary]) -> Vec<(RecordKind, i64)> {
    records.iter().map(|record| (record.kind, record.id)).collect()
}

#[test]
fn every_kind_is_listed_newest_first() {
    let store = records();
    let log = store.log(NOW, &at(1), "info", "app startup", json!({ "op": "startup" }));
    let activity = store.activity(&at(2), "started");
    let trash = store.trash(&at(3), "trashed");
    let analysis = store.analysis(&at(4), "failed");
    let issue = store.issue(Some(NOW), &at(5), "occurred");
    let notice = store.notice(&at(6), "warning");

    let result = page(&store.reader(), &query()).unwrap();

    assert_eq!(
        keys(&result.records),
        vec![
            (RecordKind::Notice, notice),
            (RecordKind::Issue, issue),
            (RecordKind::Analysis, analysis),
            (RecordKind::Trash, trash),
            (RecordKind::Activity, activity),
            (RecordKind::Log, log),
        ]
    );
    assert!(!result.more);
    let sentences: Vec<_> = result.records.iter().map(|record| record.message_key.as_deref()).collect();
    assert_eq!(
        sentences,
        vec![Some("notice.previewFailed"), Some("notice.previewFailed"), None, None, None, None]
    );
    assert_eq!(result.records[1].message_values.as_deref(), Some("{\"name\":\"a.jpg\"}"));
    let titles: Vec<_> = result.records.iter().map(|record| record.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["preview-failed", "preview-failed occurred", "faces failed", "trashed", "mutation started", "app startup"]
    );
    let texts: Vec<_> = result.records.iter().map(|record| record.text.as_deref()).collect();
    assert_eq!(
        texts,
        vec![
            None,
            Some("no decoder"),
            Some("decode failed"),
            Some("/photos/a.jpg"),
            Some("deleteFiles"),
            Some("startup"),
        ]
    );
}

#[test]
fn a_log_lines_error_is_its_summary_text() {
    let store = records();
    store.log(NOW, &at(1), "error", "boundary failed", json!({ "op": "trash", "error": "disk full" }));
    store.log(NOW, &at(2), "warn", "record write failed", json!({ "error": { "message": "locked" } }));

    let result = page(&store.reader(), &query()).unwrap();

    let texts: Vec<_> = result.records.iter().map(|record| record.text.as_deref()).collect();
    assert_eq!(texts, vec![Some("locked"), Some("disk full")]);
}

#[test]
fn levels_read_from_each_kinds_own_outcome() {
    let store = records();
    store.activity(&at(1), "failed");
    store.activity(&at(2), "completed");
    store.trash(&at(3), "trash-failed");
    store.trash(&at(4), "restored");
    store.analysis(&at(5), "failed");
    store.analysis(&at(6), "reopened");
    store.issue(Some(NOW), &at(7), "occurred");
    store.issue(Some(NOW), &at(8), "resolved");
    store.notice(&at(9), "warning");
    store.notice(&at(10), "error");
    store.notice(&at(11), "info");
    store.log(NOW, &at(12), "debug", "boundary start", json!({}));

    let result = page(&store.reader(), &query()).unwrap();

    let levels: Vec<_> = result.records.iter().map(|record| (record.kind, record.level)).collect();
    assert_eq!(
        levels,
        vec![
            (RecordKind::Log, RecordLevel::Debug),
            (RecordKind::Notice, RecordLevel::Info),
            (RecordKind::Notice, RecordLevel::Error),
            (RecordKind::Notice, RecordLevel::Warn),
            (RecordKind::Issue, RecordLevel::Info),
            (RecordKind::Issue, RecordLevel::Warn),
            (RecordKind::Analysis, RecordLevel::Info),
            (RecordKind::Analysis, RecordLevel::Error),
            (RecordKind::Trash, RecordLevel::Info),
            (RecordKind::Trash, RecordLevel::Error),
            (RecordKind::Activity, RecordLevel::Info),
            (RecordKind::Activity, RecordLevel::Error),
        ]
    );
}

#[test]
fn needs_attention_is_every_warning_and_error() {
    let store = records();
    let failed_call = store.analysis(&at(1), "failed");
    store.analysis(&at(2), "reopened");
    let warning = store.log(NOW, &at(3), "warn", "watcher lagged", json!({}));
    store.log(NOW, &at(4), "info", "app startup", json!({}));
    let issue = store.issue(Some(NOW), &at(5), "occurred");

    let attention = page(&store.reader(), &RecordsQuery { level: Some(LevelFilter::Attention), ..query() }).unwrap();
    let errors = page(&store.reader(), &RecordsQuery { level: Some(LevelFilter::Error), ..query() }).unwrap();

    assert_eq!(
        keys(&attention.records),
        vec![(RecordKind::Issue, issue), (RecordKind::Log, warning), (RecordKind::Analysis, failed_call)]
    );
    assert_eq!(keys(&errors.records), vec![(RecordKind::Analysis, failed_call)]);
}

#[test]
fn the_kind_launch_and_search_filters_narrow_the_list() {
    let store = records();
    let old = store.log(OLD, OLD, "info", "app startup", json!({}));
    let now = store.log(NOW, &at(1), "info", "app startup", json!({ "op": "50%_done" }));
    store.log(NOW, &at(2), "info", "something else", json!({ "op": "50 percent" }));
    let trash = store.trash(&at(3), "trashed");

    let reader = store.reader();
    let launch = page(&reader, &RecordsQuery { session: Some(OLD.into()), ..query() }).unwrap();
    let kind = page(&reader, &RecordsQuery { kind: Some(RecordKind::Trash), ..query() }).unwrap();
    // `%` and `_` match themselves, not any text.
    let search = page(&reader, &RecordsQuery { search: " 50%_ ".into(), ..query() }).unwrap();
    let path = page(&reader, &RecordsQuery { search: "A.JPG".into(), ..query() }).unwrap();

    assert_eq!(keys(&launch.records), vec![(RecordKind::Log, old)]);
    assert_eq!(keys(&kind.records), vec![(RecordKind::Trash, trash)]);
    assert_eq!(keys(&search.records), vec![(RecordKind::Log, now)]);
    assert_eq!(keys(&path.records), vec![(RecordKind::Trash, trash)]);
}

#[test]
fn pages_follow_on_from_the_cursor_across_kinds_and_equal_times() {
    let store = records();
    let mut expected = Vec::new();
    for second in 0..60 {
        let time = at(second);
        expected.push((RecordKind::Log, store.log(NOW, &time, "info", "line", json!({}))));
        expected.push((RecordKind::Issue, store.issue(Some(NOW), &time, "occurred")));
        expected.push((RecordKind::Log, store.log(NOW, &time, "info", "line", json!({}))));
    }
    expected.sort_by(|a, b| {
        let time = |key: &(RecordKind, i64)| expected_time(key, &store);
        time(b).cmp(&time(a)).then(b.0.as_str().cmp(a.0.as_str())).then(b.1.cmp(&a.1))
    });

    let reader = store.reader();
    let first = page(&reader, &query()).unwrap();
    let last = first.records.last().unwrap();
    let second = page(
        &reader,
        &RecordsQuery { after: Some(Cursor { time: last.time.clone(), kind: last.kind, id: last.id }), ..query() },
    )
    .unwrap();

    assert_eq!(first.records.len(), PAGE_SIZE);
    assert!(first.more);
    assert_eq!(second.records.len(), 80);
    assert!(!second.more);
    let all: Vec<_> = keys(&first.records).into_iter().chain(keys(&second.records)).collect();
    assert_eq!(all, expected);
}

fn expected_time(key: &(RecordKind, i64), store: &Records) -> String {
    let table = if key.0 == RecordKind::Log { "log_lines" } else { "issue_events" };
    store
        .writer
        .query_row(&format!("SELECT time_utc FROM {table} WHERE id = ?1"), [key.1], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_record_is_read_whole_with_every_stored_column() {
    let store = records();
    let id = store.trash(&at(1), "remove-failed");

    let record = detail(&store.reader(), RecordKind::Trash, id).unwrap().unwrap();

    assert_eq!(record.summary.level, RecordLevel::Error);
    assert_eq!(record.summary.title, "remove-failed");
    let fields: Vec<_> = record.fields.iter().map(|field| (field.name.as_str(), field.value.clone())).collect();
    assert_eq!(
        fields,
        vec![
            ("session_id", json!(NOW)),
            ("time_utc", json!(at(1))),
            ("action", json!("remove-failed")),
            ("operation_id", json!("op-1")),
            ("content_hash", json!("hash-1")),
            ("original_path", json!("/photos/a.jpg")),
            ("stored_path", json!("/photos/.onecopy-trash/a.jpg")),
            ("detail_json", json!("{\"reason\":\"user\"}")),
        ]
    );
    assert_eq!(detail(&store.reader(), RecordKind::Trash, id + 1).unwrap(), None);
}

#[test]
fn an_activity_events_numbers_stay_numbers() {
    let store = records();
    let id = store.activity(&at(1), "started");

    let record = detail(&store.reader(), RecordKind::Activity, id).unwrap().unwrap();

    let value = |name: &str| record.fields.iter().find(|field| field.name == name).unwrap().value.clone();
    assert_eq!(value("monotonic_ms"), json!(5));
    assert_eq!(value("user_visible"), json!(1));
    assert_eq!(value("kind"), json!("\"started\""));
}

#[test]
fn the_launch_filter_offers_every_launch_newest_first() {
    let store = records();
    store.log(OLD, OLD, "info", "app startup", json!({}));
    store.log(NOW, &at(1), "info", "app startup", json!({}));
    store.issue(None, &at(2), "occurred");
    store.issue(Some("2026-10-03T08:00:00.000Z"), "2026-10-03T08:00:00.000Z", "occurred");

    let result = sources(&store.reader(), Some(NOW)).unwrap();

    assert_eq!(result.current_session.as_deref(), Some(NOW));
    assert_eq!(result.sessions, vec!["2026-10-03T08:00:00.000Z".to_string(), NOW.to_string(), OLD.to_string()]);
}

#[test]
fn the_reader_cannot_write() {
    let store = records();
    let reader = store.reader();

    let insert = reader.execute(
        "INSERT INTO log_lines (session_id, time_utc, level, message, line) VALUES ('s', 't', 'info', 'm', '{}')",
        [],
    );

    assert!(insert.is_err());
}

#[test]
fn records_from_a_newer_onecopy_are_not_read() {
    let store = records();
    store.writer.pragma_update(None, "user_version", 99).unwrap();

    assert!(open_reader(&store.path).is_err());
}

#[test]
fn new_records_record_format_version_1() {
    let store = records();
    let version: i64 = store
        .writer
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, onecopy_lib::formats::RECORDS);
    assert_eq!(onecopy_lib::formats::RECORDS, 1);
}

#[test]
fn records_from_a_newer_onecopy_are_named_and_left_as_they_are() {
    let store = records();
    store.writer.pragma_update(None, "user_version", 2).unwrap();
    store.writer.pragma_update(None, "journal_mode", "DELETE").unwrap();

    let error = records::open(&store.path).expect_err("newer records are not opened for writing");
    assert!(error.contains(&store.path.to_string_lossy().into_owned()) && error.contains("newer"), "{error}");
    let journal: String = store
        .writer
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "delete", "the newer store's journal mode is not converted");
    let version: i64 = store
        .writer
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
}

#[test]
fn records_without_their_marker_are_unreadable_and_left_as_they_are() {
    let store = records();
    store.writer.pragma_update(None, "user_version", 0).unwrap();

    let error = records::open(&store.path).expect_err("unmarked records are not opened");
    assert!(error.contains("no format version"), "{error}");
    assert!(open_reader(&store.path).is_err());
    let version: i64 = store
        .writer
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0);
}
