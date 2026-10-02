use super::*;
use serde_json::json;

#[test]
fn iso_epoch() {
    assert_eq!(iso_millis(0), "1970-01-01T00:00:00.000Z");
}

#[test]
fn iso_one_second() {
    assert_eq!(iso_millis(1_000), "1970-01-01T00:00:01.000Z");
}

#[test]
fn iso_end_of_first_day() {
    assert_eq!(iso_millis(86_399_000), "1970-01-01T23:59:59.000Z");
}

#[test]
fn iso_rolls_to_second_day() {
    assert_eq!(iso_millis(86_400_000), "1970-01-02T00:00:00.000Z");
}

#[test]
fn iso_known_vector() {
    // Unix 1_700_000_000 = 2023-11-14T22:13:20Z.
    assert_eq!(iso_millis(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
}

#[test]
fn iso_preserves_milliseconds() {
    assert_eq!(iso_millis(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
}

#[test]
fn iso_handles_leap_day() {
    // Unix 951_782_400 = 2000-02-29T00:00:00Z (2000 is a leap year).
    assert_eq!(iso_millis(951_782_400_000), "2000-02-29T00:00:00.000Z");
}

#[test]
fn filename_stamp_matches_known_vector() {
    assert_eq!(filename_stamp(1_700_000_000_123), "20231114-221320-123-utc");
}

// --- Writer behavior: records, gating, level normalization, fallback ---

struct TempLogger {
    _dir: tempfile::TempDir,
    records: std::path::PathBuf,
    fallback: std::path::PathBuf,
}

fn temp_logger(debug_enabled: bool) -> (Logger, TempLogger) {
    let dir = tempfile::tempdir().unwrap();
    let records = dir.path().join("records.sqlite3");
    let fallback = dir.path().join("logs").join("fallback.log");
    let logger = Logger::start(&records, fallback.clone(), debug_enabled, "test-session".to_string());
    (logger, TempLogger { _dir: dir, records, fallback })
}

/// Every stored line, after the writer has taken what was queued.
fn read_lines(logger: &Logger, temp: &TempLogger) -> Vec<Value> {
    assert!(logger.flush(Duration::from_secs(5)));
    let conn = rusqlite::Connection::open(&temp.records).unwrap();
    let mut statement = conn.prepare("SELECT line FROM log_lines ORDER BY id").unwrap();
    let lines = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|line| serde_json::from_str::<Value>(&line.unwrap()).expect("each log line is valid JSON"))
        .collect();
    lines
}

#[test]
fn a_line_is_a_record_with_its_time_level_message_and_session() {
    let (logger, temp) = temp_logger(false);
    logger.emit(Level::Info, "started", json!({ "n": 3 }));
    let lines = read_lines(&logger, &temp);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["level"], json!("info"));
    assert_eq!(lines[0]["message"], json!("started"));
    assert_eq!(lines[0]["n"], json!(3));
    assert_eq!(lines[0]["sessionId"], json!("test-session"));
    assert!(lines[0]["time"].as_str().unwrap().ends_with('Z'));
    let conn = rusqlite::Connection::open(&temp.records).unwrap();
    let columns: (String, String, String) = conn
        .query_row("SELECT session_id, level, message FROM log_lines", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap();
    assert_eq!(columns, ("test-session".into(), "info".into(), "started".into()));
    assert!(!temp.fallback.exists(), "nothing fell back");
}

#[test]
fn a_line_the_records_cannot_take_goes_to_the_text_file() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the database belongs cannot be opened as one.
    let records = dir.path().join("records.sqlite3");
    std::fs::create_dir_all(&records).unwrap();
    let fallback = dir.path().join("logs").join("fallback.log");
    let logger = Logger::start(&records, fallback.clone(), false, "test-session".to_string());
    logger.emit(Level::Warn, "kept anyway", json!({ "n": 1 }));
    assert!(logger.flush(Duration::from_secs(5)));
    let text = std::fs::read_to_string(&fallback).unwrap();
    let line: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(line["message"], json!("kept anyway"));
    assert_eq!(line["n"], json!(1));
}

#[test]
fn session_identity_is_owned_by_the_writer() {
    let (logger, temp) = temp_logger(false);
    logger.emit(
        Level::Info,
        "started",
        json!({ "sessionId": "caller-supplied" }),
    );
    logger.emit_forwarded(json!({
        "message": "frontend",
        "sessionId": "frontend-supplied",
    }));
    let lines = read_lines(&logger, &temp);
    assert_eq!(lines[0]["sessionId"], json!("test-session"));
    assert_eq!(lines[1]["sessionId"], json!("test-session"));
}

#[test]
fn every_field_is_written_as_given() {
    let (logger, temp) = temp_logger(false);
    logger.emit(
        Level::Info,
        "creds",
        json!({ "apiKey": "sk-secret", "count": 1 }),
    );
    let lines = read_lines(&logger, &temp);
    assert_eq!(lines[0]["apiKey"], json!("sk-secret"));
    assert_eq!(lines[0]["count"], json!(1));
}

#[test]
fn nested_error_message_is_preserved_as_a_field() {
    let (logger, temp) = temp_logger(false);
    logger.emit(
        Level::Warn,
        "read failed",
        json!({ "path": "/x.json", "error": { "message": "bad json" } }),
    );
    let lines = read_lines(&logger, &temp);
    assert_eq!(lines[0]["message"], json!("read failed"));
    assert_eq!(lines[0]["error"], json!({ "message": "bad json" }));
}

#[test]
fn rust_debug_is_dropped_when_the_gate_is_off() {
    let (logger, temp) = temp_logger(false);
    logger.emit(Level::Debug, "noise", json!({}));
    assert!(read_lines(&logger, &temp).is_empty());
}

#[test]
fn forwarded_unknown_level_is_normalized_to_the_gated_level() {
    // A forwarded level Level::parse cannot recognize is written as the level
    // we actually gated/handled it as (info), never kept verbatim.
    let (logger, temp) = temp_logger(false);
    logger.emit_forwarded(json!({
        "time": "2026-06-10T03:15:42.123Z",
        "level": "warning",
        "message": "odd",
    }));
    let lines = read_lines(&logger, &temp);
    assert_eq!(lines[0]["level"], json!("info"));
    assert_eq!(lines[0]["message"], json!("odd"));
    // The frontend's own event time is preserved.
    assert_eq!(lines[0]["time"], json!("2026-06-10T03:15:42.123Z"));
}

#[test]
fn forwarded_missing_envelope_fields_are_filled() {
    let (logger, temp) = temp_logger(false);
    logger.emit_forwarded(json!({ "detail": 1 }));
    let lines = read_lines(&logger, &temp);
    assert_eq!(lines[0]["level"], json!("info"));
    assert_eq!(lines[0]["message"], json!(""));
    assert!(lines[0]["time"].as_str().unwrap().ends_with('Z'));
    assert_eq!(lines[0]["detail"], json!(1));
}

#[test]
fn forwarded_debug_respects_the_gate() {
    let (off, off_temp) = temp_logger(false);
    off.emit_forwarded(json!({ "level": "debug", "message": "frame" }));
    assert!(read_lines(&off, &off_temp).is_empty());

    let (on, on_temp) = temp_logger(true);
    on.emit_forwarded(json!({ "level": "debug", "message": "frame" }));
    assert_eq!(read_lines(&on, &on_temp)[0]["level"], json!("debug"));
}

#[test]
fn forwarded_warn_keeps_its_level() {
    let (logger, temp) = temp_logger(false);
    logger.emit_forwarded(json!({ "level": "warn", "message": "careful" }));
    assert_eq!(read_lines(&logger, &temp)[0]["level"], json!("warn"));
}
