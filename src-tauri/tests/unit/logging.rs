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

#[test]
fn session_filename_is_the_plain_utc_stamp_with_milliseconds() {
    let filename = session_filename();
    // Strictly yyyymmdd-hhmmss-fff-utc.log — no pid or id suffix.
    assert!(
        filename.ends_with("-utc.log") && !filename.contains("-p"),
        "filename {filename} must be the plain yyyymmdd-hhmmss-fff-utc.log form"
    );
    let stamp = filename.strip_suffix(".log").unwrap();
    let parts: Vec<&str> = stamp.split('-').collect();
    assert_eq!(
        parts.len(),
        4,
        "stamp {stamp} must split on '-' into 4 parts: yyyymmdd, hhmmss, fff, utc"
    );
    assert_eq!(parts[3], "utc");
    assert_eq!(parts[2].len(), 3, "millisecond part must be zero-padded to 3 digits");
}

#[test]
fn redact_matches_exact_key_case_insensitively() {
    let denied = default_denied();
    let mut value = json!({
        "token": "abc",
        "tokenCount": 5,
        "broken": true,
        "nested": { "PASSWORD": "x", "ok": 1 },
        "list": [{ "secret": "y" }, { "fine": "z" }],
    });
    redact_in_place(&mut value, &denied);
    assert_eq!(
        value,
        json!({
            "token": "[redacted]",
            "tokenCount": 5,
            "broken": true,
            "nested": { "PASSWORD": "[redacted]", "ok": 1 },
            "list": [{ "secret": "[redacted]" }, { "fine": "z" }],
        })
    );
}

#[test]
fn redact_replaces_whole_object_value() {
    let denied = default_denied();
    let mut value = json!({ "authorization": { "scheme": "Bearer", "creds": "xyz" } });
    redact_in_place(&mut value, &denied);
    assert_eq!(value, json!({ "authorization": "[redacted]" }));
}

#[test]
fn redact_never_touches_message_prose() {
    let denied = default_denied();
    let mut value = json!({ "message": "token=abc password=def", "level": "info" });
    redact_in_place(&mut value, &denied);
    assert_eq!(value["message"], json!("token=abc password=def"));
}

// --- Writer behavior: unbuffered durability, gating, level normalization ---

use std::sync::atomic::{AtomicU32, Ordering};

fn temp_logger(debug_enabled: bool) -> (Logger, std::path::PathBuf) {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "onecopy-log-test-{}-{}.log",
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_file(&path);
    let writer = open_writer(&path);
    assert!(writer.is_some(), "temp log file should open");
    let logger = Logger {
        inner: Mutex::new(Inner { writer }),
        debug_enabled,
        denied: default_denied(),
        session_id: "test-session".to_string(),
    };
    (logger, path)
}

fn read_lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("each log line is valid JSON"))
        .collect()
}

#[test]
fn line_is_on_disk_immediately_without_an_explicit_flush() {
    // The logger is unbuffered: a line is readable right after emit with no
    // flush call — this is what makes it survive a crash or signal.
    let (logger, path) = temp_logger(false);
    logger.emit(Level::Info, "started", json!({ "n": 3 }));
    let lines = read_lines(&path);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["level"], json!("info"));
    assert_eq!(lines[0]["message"], json!("started"));
    assert_eq!(lines[0]["n"], json!(3));
    assert_eq!(lines[0]["sessionId"], json!("test-session"));
    assert!(lines[0]["time"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn session_identity_is_owned_by_the_writer() {
    let (logger, path) = temp_logger(false);
    logger.emit(
        Level::Info,
        "started",
        json!({ "sessionId": "caller-supplied" }),
    );
    logger.emit_forwarded(json!({
        "message": "frontend",
        "sessionId": "frontend-supplied",
    }));
    let lines = read_lines(&path);
    assert_eq!(lines[0]["sessionId"], json!("test-session"));
    assert_eq!(lines[1]["sessionId"], json!("test-session"));
}

#[test]
fn redaction_applies_to_the_written_line() {
    let (logger, path) = temp_logger(false);
    logger.emit(
        Level::Info,
        "creds",
        json!({ "apiKey": "sk-secret", "count": 1 }),
    );
    let lines = read_lines(&path);
    assert_eq!(lines[0]["apiKey"], json!("[redacted]"));
    assert_eq!(lines[0]["count"], json!(1));
}

#[test]
fn nested_error_message_is_preserved_as_a_field() {
    let (logger, path) = temp_logger(false);
    logger.emit(
        Level::Warn,
        "read failed",
        json!({ "path": "/x.json", "error": { "message": "bad json" } }),
    );
    let lines = read_lines(&path);
    assert_eq!(lines[0]["message"], json!("read failed"));
    assert_eq!(lines[0]["error"], json!({ "message": "bad json" }));
}

#[test]
fn rust_debug_is_dropped_when_the_gate_is_off() {
    let (logger, path) = temp_logger(false);
    logger.emit(Level::Debug, "noise", json!({}));
    assert!(read_lines(&path).is_empty());
}

#[test]
fn forwarded_unknown_level_is_normalized_to_the_gated_level() {
    // A forwarded level Level::parse cannot recognize is written as the level
    // we actually gated/handled it as (info), never kept verbatim.
    let (logger, path) = temp_logger(false);
    logger.emit_forwarded(json!({
        "time": "2026-06-10T03:15:42.123Z",
        "level": "warning",
        "message": "odd",
    }));
    let lines = read_lines(&path);
    assert_eq!(lines[0]["level"], json!("info"));
    assert_eq!(lines[0]["message"], json!("odd"));
    // The frontend's own event time is preserved.
    assert_eq!(lines[0]["time"], json!("2026-06-10T03:15:42.123Z"));
}

#[test]
fn forwarded_missing_envelope_fields_are_filled() {
    let (logger, path) = temp_logger(false);
    logger.emit_forwarded(json!({ "detail": 1 }));
    let lines = read_lines(&path);
    assert_eq!(lines[0]["level"], json!("info"));
    assert_eq!(lines[0]["message"], json!(""));
    assert!(lines[0]["time"].as_str().unwrap().ends_with('Z'));
    assert_eq!(lines[0]["detail"], json!(1));
}

#[test]
fn forwarded_debug_respects_the_gate() {
    let (off, off_path) = temp_logger(false);
    off.emit_forwarded(json!({ "level": "debug", "message": "frame" }));
    assert!(read_lines(&off_path).is_empty());

    let (on, on_path) = temp_logger(true);
    on.emit_forwarded(json!({ "level": "debug", "message": "frame" }));
    assert_eq!(read_lines(&on_path)[0]["level"], json!("debug"));
}

#[test]
fn forwarded_warn_keeps_its_level() {
    let (logger, path) = temp_logger(false);
    logger.emit_forwarded(json!({ "level": "warn", "message": "careful" }));
    assert_eq!(read_lines(&path)[0]["level"], json!("warn"));
}

// --- Exclusive create: same-path second open degrades to the fallback ---

#[test]
fn exclusive_create_second_open_of_same_path_falls_back_to_stderr() {
    let path = std::env::temp_dir().join(format!(
        "onecopy-log-test-exclusive-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);

    let first = open_writer(&path);
    assert!(first.is_some(), "the first open should create the file exclusively");

    // A second open of the exact same path (the same-millisecond-clash case
    // in practice) must not append into or truncate the first session's
    // file — `create_new` makes it fail outright, and `open_writer` turns
    // that failure into the `None` stderr-fallback sentinel.
    let second = open_writer(&path);
    assert!(
        second.is_none(),
        "a same-path second open must fail over to the stderr fallback, not interleave"
    );

    let _ = std::fs::remove_file(&path);
}

// --- Mid-session write failure: permanent fallback, dead handle dropped ---

#[test]
fn write_failure_permanently_falls_back_and_stops_touching_the_dead_handle() {
    // Induce a real, deterministic write failure without abusing fd
    // ownership (closing a live fd out from under an open `File` trips
    // Rust's IO-safety double-close abort on the eventual second close).
    // A file opened read-only is a legitimate, valid handle whose own
    // close() always succeeds — but every write against it genuinely fails
    // at the OS level (EBADF/"access denied"), the same shape `write_all`
    // sees on a real mid-session failure (disk full, permissions revoked).
    let path = std::env::temp_dir().join(format!(
        "onecopy-log-test-write-fail-{}.log",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"").expect("create temp file");
    let readonly = File::open(&path).expect("open temp file read-only");

    let logger = Logger {
        inner: Mutex::new(Inner {
            writer: Some(readonly),
        }),
        debug_enabled: false,
        denied: default_denied(),
        session_id: "test-session".to_string(),
    };

    // The failing write must not panic, and must permanently drop the dead
    // handle rather than retry it on the next call.
    logger.emit(Level::Warn, "during-failure", json!({}));
    {
        let inner = logger.inner.lock().unwrap();
        assert!(
            inner.writer.is_none(),
            "a write failure must permanently switch the logger to the stderr fallback"
        );
    }

    // A later line must take the same `None` fallback branch as the failed
    // line rather than retrying the dead handle — provable because the file
    // on disk (never successfully written through the read-only handle)
    // stays empty.
    logger.emit(Level::Info, "after-failure", json!({}));
    assert!(
        read_lines(&path).is_empty(),
        "no line should ever reach the dead (read-only) file handle"
    );

    let _ = std::fs::remove_file(&path);
}
