// The application logger. Each line is a record in `records.sqlite3`
// (logging conventions); the sandboxed webview frontend forwards structured
// log objects to it (see `emit_forwarded` and the `log_event` command in
// lib.rs). Self-contained by design, per the logging conventions.
//
//   - One JSON object per line: { time, level, message, sessionId, ...fields },
//     stored whole beside its time, level, message and session. The session
//     is this launch, named by its start time.
//   - `time` is UTC ISO 8601 with milliseconds and `Z`, generated here without a
//     date crate (no new heavy deps) via a hand-rolled civil-time conversion.
//   - Four levels. `debug` is developer-only and never written unless the debug
//     gate is on (a dev build, or ONECOPY_DEBUG=1).
//   - One writer thread owns the records connection, so a caller never waits
//     on a database lock, including one its own thread holds through an
//     attached index connection. `flush` waits, bounded, for queued lines.
//   - Overload omits whole diagnostic lines with an explicit count; operation
//     facts and Issues have independent writers and are not sent here.
//   - A line the records cannot take goes to this launch's text file under
//     `logs/`, then to stderr; logging never panics and never stops the app.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, OnceLock};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
// its `written_at_utc` column with this — NEVER the `yyyymmdd-hhmmss-utc`
// filename stamp (`filename_stamp` above), which belongs to file names only.
pub fn now_iso_millis() -> String {
    iso_millis(now_unix_millis())
}

// Seconds, the timestamp conventions' default: these names are one fallback
// log per launch and a set-aside store, at most once per store per launch,
// and set-asides are renamed without replacing anything.
fn filename_stamp(ms: i64) -> String {
    let (y, mo, d, h, mi, s, _) = parts_from_millis(ms);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-utc")
}

// --- The logger itself ---

// A small diagnostic burst buffer, not a durable delivery queue. Both slots
// and retained bytes are bounded; overload is recorded as an omitted count.
const QUEUE_SLOTS: usize = 1024;
const QUEUE_BYTES: usize = 1024 * 1024;

#[derive(Default)]
struct Backlog {
    bytes: AtomicUsize,
    omitted: AtomicU64,
}

enum Queued {
    Line(Line),
    Omitted,
    Flush(mpsc::Sender<()>),
}

struct Line {
    session_id: String,
    time: String,
    level: String,
    message: String,
    text: String,
}

impl Line {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.session_id.capacity() + self.time.capacity()
            + self.level.capacity() + self.message.capacity() + self.text.capacity()
    }

    fn omission(session: &str, count: u64) -> Self {
        let time = now_iso_millis();
        let message = "diagnostic lines omitted during logging overload";
        let text = serde_json::json!({
            "time": time, "level": "warn", "message": message,
            "sessionId": session, "omittedCount": count,
        }).to_string();
        Self { session_id: session.to_string(), time, level: "warn".into(), message: message.into(), text }
    }
}

pub struct Logger {
    sender: Option<mpsc::SyncSender<Queued>>,
    backlog: Arc<Backlog>,
    debug_enabled: bool,
    session_id: String,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// Writes queued lines into the records, falling back to the session's text
/// file and then stderr for a line the records cannot take.
struct Writer {
    records: Option<rusqlite::Connection>,
    fallback_path: PathBuf,
    fallback: Option<File>,
}

impl Writer {
    fn write(&mut self, line: &Line) {
        let stored = match &self.records {
            Some(connection) => connection
                .execute(
                    "INSERT INTO log_lines (session_id, time_utc, level, message, line)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![line.session_id, line.time, line.level, line.message, line.text],
                )
                .map(|_| crate::records::wrote(connection))
                .map_err(|error| error.to_string()),
            None => Err("records are unavailable".to_string()),
        };
        if let Err(error) = stored {
            self.write_fallback(line, &error);
        }
    }

    // not recorded: the fallback text file is append-mode diagnostic output.
    fn write_fallback(&mut self, line: &Line, reason: &str) {
        if self.fallback.is_none() {
            let opened = self
                .fallback_path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| OpenOptions::new().create(true).append(true).open(&self.fallback_path));
            match opened {
                Ok(file) => self.fallback = Some(file),
                Err(error) => {
                    eprintln!("[onecopy:logging] {reason}; {}: {error}", crate::native_path::label(&self.fallback_path));
                    eprintln!("{}", line.text);
                    return;
                }
            }
        }
        let written = self
            .fallback
            .as_mut()
            .map(|file| file.write_all(format!("{}\n", line.text).as_bytes()));
        if let Some(Err(error)) = written {
            self.fallback = None;
            eprintln!("[onecopy:logging] {reason}; {}: {error}", crate::native_path::label(&self.fallback_path));
            eprintln!("{}", line.text);
        }
    }
}

impl Logger {
    /// Starts the writer thread over `records_path`, with `fallback_path` for
    /// lines the records cannot take.
    fn start(records_path: &Path, fallback_path: PathBuf, debug_enabled: bool, session_id: String) -> Self {
        let records_path = records_path.to_path_buf();
        Self::start_writer(debug_enabled, session_id, move || {
            let records = crate::records::open(&records_path)
                .map_err(|error| eprintln!("[onecopy:logging] records unavailable: {error}"))
                .ok();
            let mut writer = Writer { records, fallback_path, fallback: None };
            move |line: &Line| writer.write(line)
        })
    }

    fn start_writer<W: FnMut(&Line) + 'static>(
        debug_enabled: bool,
        session_id: String,
        prepare: impl FnOnce() -> W + Send + 'static,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Queued>(QUEUE_SLOTS);
        let backlog = Arc::new(Backlog::default());
        let pending = Arc::clone(&backlog);
        let session = session_id.clone();
        let spawned = std::thread::Builder::new()
            .name("onecopy-log-writer".to_string())
            .spawn(move || {
                let mut write = prepare();
                for queued in receiver {
                    let acknowledgement = match queued {
                        Queued::Line(line) => {
                            let bytes = line.retained_bytes();
                            write(&line);
                            pending.bytes.fetch_sub(bytes, Ordering::Relaxed);
                            None
                        }
                        Queued::Omitted => None,
                        Queued::Flush(done) => Some(done),
                    };
                    let omitted = pending.omitted.swap(0, Ordering::Relaxed);
                    if omitted > 0 {
                        write(&Line::omission(&session, omitted));
                    }
                    if let Some(done) = acknowledgement {
                        let _ = done.send(());
                    }
                }
            });
        let sender = match spawned {
            Ok(_) => Some(sender),
            Err(error) => {
                eprintln!("[onecopy:logging] could not start the log writer: {error}; logging to stderr");
                None
            }
        };
        Logger { sender, backlog, debug_enabled, session_id }
    }

    /// Includes queued lines and overload summaries. Queue admission consumes
    /// the same deadline as the acknowledgement; a full queue cannot hang exit.
    fn flush(&self, timeout: Duration) -> bool {
        let Some(sender) = &self.sender else { return true };
        let started = std::time::Instant::now();
        let (done, finished) = mpsc::channel();
        let mut marker = Queued::Flush(done);
        loop {
            match sender.try_send(marker) {
                Ok(()) => return finished.recv_timeout(timeout.saturating_sub(started.elapsed())).is_ok(),
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
                Err(mpsc::TrySendError::Full(returned)) => marker = returned,
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() { return false; }
            std::thread::sleep(remaining.min(Duration::from_millis(1)));
        }
    }

}

// Installs the process-global logger for this launch: lines go to the
// records at `records_path`, and to a text file under `logs_dir` named by the
// launch when the records cannot take one. Call once.
pub fn init(records_path: &Path, logs_dir: &Path, debug_enabled: bool) {
    let started = now_unix_millis();
    let logger = Logger::start(
        records_path,
        logs_dir.join(format!("{}.log", filename_stamp(started))),
        debug_enabled,
        iso_millis(started),
    );
    if LOGGER.set(logger).is_err() {
        eprintln!("[onecopy:logging] logger already initialized; ignoring re-init");
    }
}

/// Waits, bounded, for admitted lines and summaries of omitted diagnostics.
pub fn flush(timeout: Duration) -> bool {
    global().is_none_or(|logger| logger.flush(timeout))
}

fn global() -> Option<&'static Logger> {
    LOGGER.get()
}

pub fn debug_enabled() -> bool {
    global().map(|l| l.debug_enabled).unwrap_or(false)
}

pub fn session_id() -> Option<&'static str> {
    global().map(|logger| logger.session_id.as_str())
}

impl Logger {
    // Serializes and queues one envelope as a single line. Both emit() and
    // emit_forwarded() funnel through here, so every line passes the
    // identical write contract.
    fn write_envelope(&self, obj: Map<String, Value>) {
        let mut obj = obj;
        obj.insert(
            "sessionId".to_string(),
            Value::String(self.session_id.clone()),
        );
        let text_of = |key: &str| obj.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let (time, level, message) = (text_of("time"), text_of("level"), text_of("message"));
        let text = match serde_json::to_string(&Value::Object(obj)) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("[onecopy:logging] serialize failed: {e}");
                return;
            }
        };
        let line = Line { session_id: self.session_id.clone(), time, level, message, text };
        let Some(sender) = &self.sender else {
            eprintln!("{}", line.text);
            return;
        };
        let bytes = line.retained_bytes();
        let reserved = self.backlog.bytes.try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(bytes).filter(|next| *next <= QUEUE_BYTES)
        }).is_ok();
        if !reserved {
            self.omit(sender);
            return;
        }
        match sender.try_send(Queued::Line(line)) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                self.backlog.bytes.fetch_sub(bytes, Ordering::Relaxed);
                self.omit(sender);
            }
            Err(mpsc::TrySendError::Disconnected(Queued::Line(line))) => {
                self.backlog.bytes.fetch_sub(bytes, Ordering::Relaxed);
                eprintln!("{}", line.text);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => unreachable!(),
        }
    }

    fn omit(&self, sender: &mpsc::SyncSender<Queued>) {
        self.backlog.omitted.fetch_add(1, Ordering::Relaxed);
        // If full, an already queued item will wake the writer. Otherwise
        // wake it even when an oversized event was the only incoming line.
        if let Err(mpsc::TrySendError::Disconnected(_)) = sender.try_send(Queued::Omitted) {
            eprintln!("[onecopy:logging] diagnostic line omitted; writer stopped");
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
    // event instant). We re-apply the debug gate so every line in the file
    // went through the same writer contract.
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

// The current instant as the filename stamp (`yyyymmdd-hhmmss-utc`) — for
// derived sibling names whose discriminator is a moment (a quarantined corrupt
// store), per the derived-filename grammar. File names only; data values use
// `now_iso_millis`.
pub fn filename_stamp_now() -> String {
    filename_stamp(now_unix_millis())
}

// Records the panic payload, location, and (when RUST_BACKTRACE is set) the
// backtrace, flushes, then defers to the previous hook so the process still
// aborts and prints as usual.
pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let backtrace = std::backtrace::Backtrace::capture();
        error(
            "panic",
            serde_json::json!({
                "error": {
                    "message": payload,
                    "location": location,
                    "backtrace": format!("{backtrace}"),
                }
            }),
        );
        // Let the error line reach the records before deferring to the
        // previous hook, so the process still aborts and prints as usual.
        flush(Duration::from_secs(2));
        default_hook(info);
    }));
}

#[cfg(test)]
// EXCEPTION to the tests-live-in-tests/ rule (tests-folder
// conventions, Rust form): these tests exercise genuinely private
// internals that cannot reasonably be promoted — promoting them
// would widen the module's surface just to test through it.
#[path = "../tests/unit/logging.rs"]
mod tests;
