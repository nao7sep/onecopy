// Per-session JSON Lines logger. The privileged Rust core owns the log file;
// the sandboxed webview frontend forwards structured log objects to it (see
// `emit_forwarded` and the `log_event` command in lib.rs). Self-contained and
// dependency-free by design, per the logging conventions.
//
// Design (mirrors ~/code/company/conventions/...-logging-conventions.md):
//   - One file per process launch: ~/.onecopy/logs/<yyyymmdd-hhmmss-fff-utc.log>.
//     Strictly that stamp — no pid or id suffix. Two launches in the same UTC
//     millisecond collide on the name; the file is opened with exclusive
//     create, so the second launch's open fails and it degrades to the
//     stderr fallback rather than interleaving two sessions into one file.
//   - One JSON object per line: { time, level, message, ...fields }.
//   - `time` is UTC ISO 8601 with milliseconds and `Z`, generated here without a
//     date crate (no new heavy deps) via a hand-rolled civil-time conversion.
//   - Four levels. `debug` is developer-only and never written unless the debug
//     gate is on (a dev build, or ONECOPY_DEBUG=1).
//   - Every line is written straight through to the OS (unbuffered), so the
//     convention's "last lines before a crash must reach disk" holds for free:
//     once a line is logged the OS has it, surviving a panic, SIGKILL, or any
//     signal — no buffer can strand it, and there is no flush to forget. (Log
//     volume is human-paced and IO-bounded, so per-line writes cost nothing
//     meaningful; only a kernel panic or power loss, which no userspace flush
//     would prevent either, can lose an unsynced page.)
//   - A mandatory, non-destructive redactor replaces the value of any field whose
//     name (exact, case-insensitive) is in the denied set; it never edits prose.
//   - If the file cannot be opened or written, it degrades to stderr and never
//     panics — the app must never crash because logging failed. A mid-session
//     write failure permanently switches to the stderr fallback (the dead
//     handle is dropped and never retried), and the line that failed to write
//     is re-emitted to stderr so its content is never lost.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    fn parse(s: &str) -> Option<Level> {
        match s {
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

// --- Time (UTC ISO 8601 ms + the filename stamp), hand-rolled, no date crate ---

fn now_unix_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        // A clock set before the epoch is not a real case; stay total anyway.
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

// Howard Hinnant's days-from-civil inverse: `z` is days since 1970-01-01.
// Returns (year, month [1..12], day [1..31]).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// Breaks a UTC instant (unix millis) into calendar parts. `div_euclid` /
// `rem_euclid` keep this correct for instants before the epoch as well.
fn parts_from_millis(ms: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
    let days = ms.div_euclid(86_400_000);
    let rem = ms.rem_euclid(86_400_000); // [0, 86_400_000)
    let (year, month, day) = civil_from_days(days);
    let secs = rem / 1_000;
    let milli = (rem % 1_000) as u32;
    let hour = (secs / 3_600) as u32;
    let minute = ((secs % 3_600) / 60) as u32;
    let second = (secs % 60) as u32;
    (year, month, day, hour, minute, second, milli)
}

fn iso_millis(ms: i64) -> String {
    let (y, mo, d, h, mi, s, ms3) = parts_from_millis(ms);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms3:03}Z")
}

// The current instant as the serialized ISO-8601 UTC-with-milliseconds form
// (`2026-07-06T04:05:12.345Z`) — the timestamp-conventions' internal/serialized
// shape, a data value. Reuses the same `iso_millis` formatter the log lines use
// so there is one time formatter, never a fourth. The data-backup store stamps
// its `written_at_utc` column with this — NEVER the `yyyymmdd-hhmmss-fff-utc`
// filename stamp (`filename_stamp` above), which belongs to file names only.
pub fn now_iso_millis() -> String {
    iso_millis(now_unix_millis())
}

fn filename_stamp(ms: i64) -> String {
    let (y, mo, d, h, mi, s, ms3) = parts_from_millis(ms);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{ms3:03}-utc")
}

// `<yyyymmdd-hhmmss-fff-utc.log>` for the current launch — the plain UTC stamp
// (with milliseconds) and no other suffix; a same-millisecond collision is
// accepted rather than engineered around.
pub fn session_filename() -> String {
    format!("{}.log", filename_stamp(now_unix_millis()))
}

// --- Redaction: non-destructive, key-name based, recursive, total ---

fn default_denied() -> HashSet<String> {
    ["apikey", "authorization", "token", "password", "secret"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// Replaces the value of any field whose key (lowercased) is denied with the
// fixed marker; recurses into objects and arrays. Never inspects string content,
// never edits `message` (it is not a denied key), cannot drop fields or throw.
fn redact_in_place(value: &mut Value, denied: &HashSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if denied.contains(&key.to_ascii_lowercase()) {
                    *child = Value::String("[redacted]".to_string());
                } else {
                    redact_in_place(child, denied);
                }
            }
        }
        Value::Array(items) => {
            for child in items.iter_mut() {
                redact_in_place(child, denied);
            }
        }
        _ => {}
    }
}

// --- The logger itself ---

struct Inner {
    // Unbuffered: each line is written straight to the file so a crash or signal
    // can never strand buffered lines. `None` means file logging failed at open
    // and we degrade to stderr.
    writer: Option<File>,
}

pub struct Logger {
    inner: Mutex<Inner>,
    debug_enabled: bool,
    denied: HashSet<String>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

// Opens the session file (creating ~/.onecopy/logs/ if needed) and installs the
// process-global logger. On any failure it installs a logger that writes to
// stderr instead, so logging calls always have somewhere to go. Call once.
pub fn init(file_path: &Path, debug_enabled: bool) {
    let writer = open_writer(file_path);
    let logger = Logger {
        inner: Mutex::new(Inner { writer }),
        debug_enabled,
        denied: default_denied(),
    };
    if LOGGER.set(logger).is_err() {
        eprintln!("[onecopy:logging] logger already initialized; ignoring re-init");
    }
}

fn open_writer(file_path: &Path) -> Option<File> {
    if let Some(parent) = file_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "[onecopy:logging] could not create {}: {e}; logging to stderr",
                parent.display()
            );
            return None;
        }
    }
    // Exclusive create: a session file is always fresh, never appended into.
    // Two launches landing on the same millisecond stamp are the one case this
    // can legitimately fail on live filesystems; the second one loses the race
    // and falls through to the stderr fallback below rather than interleaving
    // both sessions into a single file.
    match OpenOptions::new().create_new(true).write(true).open(file_path) {
        Ok(file) => Some(file),
        Err(e) => {
            eprintln!(
                "[onecopy:logging] could not open {}: {e}; logging to stderr",
                file_path.display()
            );
            None
        }
    }
}

fn global() -> Option<&'static Logger> {
    LOGGER.get()
}

pub fn debug_enabled() -> bool {
    global().map(|l| l.debug_enabled).unwrap_or(false)
}

impl Logger {
    // Redacts, serializes, and writes one envelope as a single line. Both emit()
    // and emit_forwarded() funnel through here, so every line in the file passes
    // the identical redact + write contract.
    fn write_envelope(&self, obj: Map<String, Value>) {
        let mut value = Value::Object(obj);
        redact_in_place(&mut value, &self.denied);
        let mut line = match serde_json::to_string(&value) {
            Ok(line) => line,
            Err(e) => {
                eprintln!("[onecopy:logging] serialize failed: {e}");
                return;
            }
        };
        line.push('\n');
        // Recover from a poisoned mutex: a prior panic-while-writing must not
        // wedge logging shut, least of all the panic hook trying to record it.
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match inner.writer.as_mut() {
            // One write_all per line; the file is opened via exclusive create
            // and this is the only writer in the process, so lines never
            // interleave. The bytes reach the OS immediately — no buffer,
            // nothing to flush.
            Some(writer) => {
                if let Err(e) = writer.write_all(line.as_bytes()) {
                    // The handle is dead (disk full, permissions revoked, the
                    // device went away). Never retry it: drop it permanently so
                    // every later call takes the `None` branch below, and
                    // re-emit this very line to stderr right now so its content
                    // is degraded-to, not silently swallowed.
                    eprintln!(
                        "[onecopy:logging] write failed: {e}; switching to stderr fallback"
                    );
                    inner.writer = None;
                    eprint!("{line}");
                }
            }
            None => eprint!("{line}"),
        }
    }

    // Builds the envelope from a Rust-side event and writes it. `fields` is
    // merged in; the envelope keys (time/level/message) always win.
    fn emit(&self, level: Level, message: &str, fields: Value) {
        if level == Level::Debug && !self.debug_enabled {
            return;
        }
        let mut obj = Map::new();
        obj.insert(
            "time".to_string(),
            Value::String(iso_millis(now_unix_millis())),
        );
        obj.insert(
            "level".to_string(),
            Value::String(level.as_str().to_string()),
        );
        obj.insert("message".to_string(), Value::String(message.to_string()));
        if let Value::Object(extra) = fields {
            for (key, val) in extra {
                if key != "time" && key != "level" && key != "message" {
                    obj.insert(key, val);
                }
            }
        }
        self.write_envelope(obj);
    }

    // Writes an object the frontend already shaped (it stamped `time` at the
    // event instant). We re-apply the debug gate and the redactor so every line
    // in the file went through the same writer contract.
    fn emit_forwarded(&self, value: Value) {
        let mut obj = match value {
            Value::Object(map) => map,
            other => {
                // Defensive: a non-object payload is wrapped, never dropped.
                let mut map = Map::new();
                map.insert("forwarded".to_string(), other);
                map
            }
        };
        let level = obj
            .get("level")
            .and_then(|v| v.as_str())
            .and_then(Level::parse)
            .unwrap_or(Level::Info);
        if level == Level::Debug && !self.debug_enabled {
            return;
        }
        // Keep the frontend's `time`/`message` (the event instant and wording),
        // but always normalize `level` to the value we actually gated on — so an
        // unrecognized or missing level can never make the written level disagree
        // with how the line was handled.
        obj.entry("time".to_string())
            .or_insert_with(|| Value::String(iso_millis(now_unix_millis())));
        obj.insert(
            "level".to_string(),
            Value::String(level.as_str().to_string()),
        );
        obj.entry("message".to_string())
            .or_insert_with(|| Value::String(String::new()));
        self.write_envelope(obj);
    }
}

// --- Free functions over the process-global logger (used by lib.rs) ---

fn emit(level: Level, message: &str, fields: Value) {
    if let Some(logger) = global() {
        logger.emit(level, message, fields);
    } else if level != Level::Debug {
        // No logger yet (e.g. a panic during early startup): best effort.
        eprintln!("[onecopy:logging:{}] {message} {fields}", level.as_str());
    }
}

pub fn debug(message: &str, fields: Value) {
    emit(Level::Debug, message, fields);
}

pub fn info(message: &str, fields: Value) {
    emit(Level::Info, message, fields);
}

// One `warn` line is what the data-backup store logs when it cannot open
// (recording disabled for the session) or when a single record fails — the
// convention's "logs only failures, one warn line" contract. The frontend's
// warnings also arrive pre-leveled through `emit_forwarded`.
pub fn warn(message: &str, fields: Value) {
    emit(Level::Warn, message, fields);
}

pub fn error(message: &str, fields: Value) {
    emit(Level::Error, message, fields);
}

pub fn emit_forwarded(value: Value) {
    if let Some(logger) = global() {
        logger.emit_forwarded(value);
    }
}

// --- Boundary instrumentation (logging conventions) ---

// Wraps an external-boundary operation (file / database / IPC command) with the
// standard logging: a `debug` line at the start, then exactly one `info` line on
// success or one `error` line on failure, each carrying the elapsed duration.
// This is what keeps "log every boundary crossing" to one info line per crossing.
pub fn boundary<T>(
    op: &str,
    params: Value,
    body: impl FnOnce() -> Result<T, String>,
    summarize: impl FnOnce(&T) -> Value,
) -> Result<T, String> {
    let started = std::time::Instant::now();

    let mut start_fields = into_map(params);
    start_fields.insert("op".to_string(), Value::String(op.to_string()));
    emit(Level::Debug, "boundary start", Value::Object(start_fields));

    let result = body();
    let ms = started.elapsed().as_millis() as u64;

    match &result {
        Ok(value) => {
            let mut fields = into_map(summarize(value));
            fields.insert("op".to_string(), Value::String(op.to_string()));
            fields.insert("ms".to_string(), Value::from(ms));
            emit(Level::Info, "boundary ok", Value::Object(fields));
        }
        Err(err) => {
            let mut fields = Map::new();
            fields.insert("op".to_string(), Value::String(op.to_string()));
            fields.insert("ms".to_string(), Value::from(ms));
            fields.insert("error".to_string(), Value::String(err.clone()));
            emit(Level::Error, "boundary failed", Value::Object(fields));
        }
    }

    result
}

// A non-object params/summary value is wrapped, never lost.
fn into_map(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        other => {
            let mut map = Map::new();
            map.insert("value".to_string(), other);
            map
        }
    }
}

// The current instant as the filename stamp (`yyyymmdd-hhmmss-fff-utc`) — for
// derived sibling names whose discriminator is a moment (a quarantined corrupt
// store), per the derived-filename grammar. File names only; data values use
// `now_iso_millis`.
pub fn filename_stamp_now() -> String {
    filename_stamp(now_unix_millis())
}

#[cfg(test)]
mod tests {
    // EXCEPTION to the tests-live-in-tests/ rule (tests-folder
    // conventions, Rust form): these tests exercise genuinely private
    // internals that cannot reasonably be promoted — promoting them
    // would widen the module's surface just to test through it.
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
        assert!(lines[0]["time"].as_str().unwrap().ends_with('Z'));
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
}

/// Retention (Phase 33): one file per launch and nothing ever pruned meant
/// the logs folder grew for the life of the install — and it now has a menu
/// item pointing at it. Age by modification time, 30 days, best-effort: a
/// failure to prune must never touch startup, and the CURRENT session's file
/// is naturally the newest. Called once from setup, after init.
pub fn prune_old_logs(logs_dir: &Path, keep_days: u64) {
    let Ok(entries) = std::fs::read_dir(logs_dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        - std::time::Duration::from_secs(keep_days * 24 * 60 * 60);
    let mut removed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if old && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        info(
            "old logs pruned",
            serde_json::json!({ "removed": removed, "keepDays": keep_days }),
        );
    }
}
