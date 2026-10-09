//! The history of protected text (data-backup conventions). It owns one
//! SQLite file, `backups.sqlite3`, directly under onecopy's storage root
//! (`ONECOPY_DATA_DIR` or `~/.onecopy`, resolved in one place by
//! `paths::data_root` — never a hardcoded path), holding the last version of
//! each protected file saved in each session (one launch). Protected files
//! reach it through `storage::write_atomic` after their rename lands. There is
//! no startup scan, no periodic pass and no restore path.
//!
//! Recording never holds up a save: `record` hands the exact bytes to one
//! writer thread and returns. That thread applies them in save order, so an
//! earlier version never replaces a later one. Ordinary quit gives pending
//! writes a short bound (`drain`); the OS ending the session skips them.
//!
//! SQLite binding: `rusqlite` with the `bundled` feature, which compiles
//! SQLite into the binary, so the store adds no packaging churn. Content is a
//! BLOB of raw bytes, so CR/LF, a BOM and non-UTF-8 bytes stay byte-identical.
//!
//! Best effort: any failure (the store cannot open, a write fails) is logged
//! once at `warn` and swallowed; a successful record logs nothing. A lost
//! record heals on the file's next save.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::logging;

/// The store's on-disk file name under the storage root, named in exactly one
/// place (pinned by the storage_file_names integration test).
pub const BACKUPS_DB_FILE_NAME: &str = "backups.sqlite3";

/// One row per protected path per session. `content` is a BLOB of the exact
/// bytes written. `written_at_utc` is the serialized ISO-8601-ms form
/// (`2026-07-06T04:05:12.345Z`), a data value — NEVER the
/// `yyyymmdd-hhmmss-utc` filename stamp. `session_id` is the launch's log
/// session (its start time); rows from before sessions existed have none and
/// stay as earlier history. The `(path, id)` index serves the latest-row
/// lookup; the unique `(path, session_id)` index owns the session's rows.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS backups (
  id             INTEGER PRIMARY KEY,
  path           TEXT NOT NULL,
  content        BLOB NOT NULL,
  content_sha256 TEXT NOT NULL,
  byte_size      INTEGER NOT NULL,
  written_at_utc TEXT NOT NULL,
  session_id     TEXT
);
CREATE INDEX IF NOT EXISTS idx_backups_path_id ON backups (path, id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_backups_path_session ON backups (path, session_id);
";

enum Job {
    Record(String, Vec<u8>),
    Drained(Sender<()>),
}

/// The writer thread's mailbox, `None` when recording is off (never started,
/// or closed). Rows are written only on that thread.
struct Writer {
    sender: Option<Sender<Job>>,
    #[cfg(test)]
    worker: Option<std::thread::JoinHandle<()>>,
}

fn writer() -> &'static Mutex<Writer> {
    static WRITER: OnceLock<Mutex<Writer>> = OnceLock::new();
    WRITER.get_or_init(|| {
        Mutex::new(Writer {
            sender: None,
            #[cfg(test)]
            worker: None,
        })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Writer> {
    // A prior panic elsewhere must not wedge recording shut; nothing here
    // panics while holding the lock.
    writer().lock().unwrap_or_else(|p| p.into_inner())
}

/// Starts the writer for this launch's session. The store opens on the writer
/// thread; if it cannot (a newer or unmarked history included), one `warn` is
/// logged and every later record is dropped for the session. Startup never
/// waits on it.
pub fn init(store_file: PathBuf, session_id: String) {
    let (sender, receiver) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("onecopy-backups".to_string())
        .spawn(move || run_writer(&store_file, &session_id, receiver));
    let mut writer = lock();
    match spawned {
        Ok(_worker) => {
            writer.sender = Some(sender);
            #[cfg(test)]
            {
                writer.worker = Some(_worker);
            }
        }
        Err(error) => logging::warn(
            "backup store: could not start its writer; recording disabled for this session",
            json!({ "error": { "message": error.to_string() } }),
        ),
    }
}

fn run_writer(store_file: &Path, session_id: &str, jobs: Receiver<Job>) {
    let mut conn = match open(store_file) {
        Ok(conn) => Some(conn),
        Err(err) => {
            logging::warn(
                "backup store: could not open; recording disabled for this session",
                json!({ "file": store_file.to_string_lossy(), "error": { "message": err } }),
            );
            None
        }
    };
    while let Ok(first) = jobs.recv() {
        // Take everything already waiting. Under pressure only the newest
        // version of each path matters: it is what the session's row keeps.
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        let mut pending: Vec<(String, Vec<u8>)> = Vec::new();
        let mut drained = Vec::new();
        for job in batch {
            match job {
                Job::Record(path, bytes) => {
                    pending.retain(|(queued, _)| *queued != path);
                    pending.push((path, bytes));
                }
                Job::Drained(done) => drained.push(done),
            }
        }
        if let Some(conn) = conn.as_mut() {
            for (path, bytes) in pending {
                if let Err(err) = try_record(conn, session_id, &path, &bytes) {
                    logging::warn(
                        "backup store: failed to record a managed write",
                        json!({ "file": path, "error": { "message": err.to_string() } }),
                    );
                }
            }
        }
        for done in drained {
            let _ = done.send(());
        }
    }
}

fn open(store_file: &Path) -> Result<Connection, String> {
    open_before_setup(store_file, || {})
}

fn open_before_setup(store_file: &Path, before_setup: impl FnOnce()) -> Result<Connection, String> {
    // not recorded: backups.sqlite3 is the store itself — binary, and written by
    // this backup layer, not through the managed-text atomic-write path — so it
    // never records itself.
    // The first writer under the root does the `mkdir -p`; the store may be the
    // first thing written on a fresh root.
    if let Some(parent) = store_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // Only the opener that exclusively created the file may initialize an
    // empty history. An existing unmarked file is unreadable provenance.
    let fresh = match std::fs::OpenOptions::new().write(true).create_new(true).open(store_file) { // data root
        Ok(file) => { drop(file); true }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.to_string()),
    };
    let mut conn = Connection::open(store_file).map_err(|e| e.to_string())?;
    // Short, as the convention asks: the instance lock leaves this writer
    // alone with the file, so contention means something is wrong.
    conn.busy_timeout(Duration::from_millis(100))
        .map_err(|e| e.to_string())?;
    // A history a newer OneCopy wrote, or one without its marker, is left
    // exactly as it is: recording stays disabled for the session
    // (store-recovery conventions).
    match crate::formats::sqlite_marker(&conn, store_file, crate::formats::BACKUPS)? {
        crate::formats::SqliteMarker::Newer(newer) => return Err(newer.to_string()),
        crate::formats::SqliteMarker::Missing => return Err(crate::formats::missing_marker(store_file)),
        crate::formats::SqliteMarker::New if !fresh => return Err(crate::formats::missing_marker(store_file)),
        crate::formats::SqliteMarker::New | crate::formats::SqliteMarker::Current => {}
    }
    static JOURNAL: crate::sqlite::JournalSetup = crate::sqlite::JournalSetup::new();
    JOURNAL.configure(&conn, Duration::from_millis(100))?;
    before_setup();
    // The schema and its marker commit together.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    match crate::formats::sqlite_marker(&tx, store_file, crate::formats::BACKUPS)? {
        crate::formats::SqliteMarker::Newer(newer) => return Err(newer.to_string()),
        crate::formats::SqliteMarker::Missing => return Err(crate::formats::missing_marker(store_file)),
        crate::formats::SqliteMarker::New if fresh => {}
        crate::formats::SqliteMarker::New => return Err(crate::formats::missing_marker(store_file)),
        crate::formats::SqliteMarker::Current => {
            // Format 1 is edited in place while OneCopy's data is not durable
            // yet (`formats`). A history from before sessions gains the
            // column, and its rows stay as earlier history.
            let has_sessions = tx
                .prepare("SELECT 1 FROM pragma_table_info('backups') WHERE name = 'session_id'")
                .and_then(|mut statement| statement.exists([]))
                .map_err(|e| e.to_string())?;
            if !has_sessions {
                tx.execute_batch("ALTER TABLE backups ADD COLUMN session_id TEXT;")
                    .map_err(|e| e.to_string())?;
            }
            tx.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            return Ok(conn);
        }
    }
    tx.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    tx.pragma_update(None, "user_version", crate::formats::BACKUPS)
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(conn)
}

/// SHA-256 of the exact bytes, lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Queues one protected write: `absolute_path` is the FULL absolute path of
/// the file as written; `bytes` is the exact raw bytes just written (the caller
/// already holds them — never a re-read of the file). Returns at once.
pub fn record(absolute_path: &Path, bytes: &[u8]) {
    let writer = lock();
    if let Some(sender) = writer.sender.as_ref() {
        let _ = sender.send(Job::Record(
            absolute_path.to_string_lossy().into_owned(),
            bytes.to_vec(),
        ));
    }
}

/// Waits up to `timeout` for every queued record to be written. Ordinary quit
/// calls this; the OS ending the session does not.
pub fn drain(timeout: Duration) -> bool {
    let (done, finished) = mpsc::channel();
    let sent = lock()
        .sender
        .as_ref()
        .is_some_and(|sender| sender.send(Job::Drained(done)).is_ok());
    !sent || finished.recv_timeout(timeout).is_ok()
}

/// The session's row for a path holds its last saved version: the first save
/// in a session inserts it, unless its content equals the path's latest row
/// from an earlier session, and later saves replace it.
fn try_record(
    conn: &mut Connection,
    session_id: &str,
    path: &str,
    bytes: &[u8],
) -> Result<(), rusqlite::Error> {
    let hash = sha256_hex(bytes);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (latest_hash, latest_session): (Option<String>, Option<String>) = match tx.query_row(
        "SELECT content_sha256, session_id FROM backups WHERE path = ?1 ORDER BY id DESC LIMIT 1",
        [path],
        |row| Ok((Some(row.get(0)?), row.get(1)?)),
    ) {
        Ok(latest) => latest,
        Err(rusqlite::Error::QueryReturnedNoRows) => (None, None),
        Err(other) => return Err(other),
    };
    let unchanged = latest_hash.as_deref() == Some(hash.as_str());
    if unchanged && latest_session.as_deref() != Some(session_id) {
        // Nothing new since an earlier session's version.
        tx.commit()?;
        return Ok(());
    }
    tx.execute(
        "INSERT INTO backups (path, content, content_sha256, byte_size, written_at_utc, session_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (path, session_id) DO UPDATE SET content = excluded.content, \
           content_sha256 = excluded.content_sha256, byte_size = excluded.byte_size, \
           written_at_utc = excluded.written_at_utc",
        rusqlite::params![
            path,
            bytes,
            hash,
            bytes.len() as i64,
            logging::now_iso_millis(),
            session_id
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// Stops the writer after it has written everything queued, so a test can
/// read the store and the next `init` starts clean.
#[cfg(test)]
pub fn close_for_test() {
    let (sender, worker) = {
        let mut writer = lock();
        (writer.sender.take(), writer.worker.take())
    };
    drop(sender);
    if let Some(worker) = worker {
        let _ = worker.join();
    }
}

#[cfg(test)]
// EXCEPTION to the tests-live-in-tests/ rule (tests-folder
// conventions, Rust form): these tests exercise genuinely private
// internals that cannot reasonably be promoted — promoting them
// would widen the module's surface just to test through it.
// The store singleton is process-global, so every test that touches it is
// marked `#[serial(backup_store)]`. `cargo test` runs tests in parallel threads
// within one process; the shared `backup_store` key makes this group (plus the
// lib.rs atomic-write test, which reaches `record` through `write_atomic`)
// mutually exclusive, so no test resets/reopens the singleton out from under
// another. Each test opens a fresh throwaway store file, exercises it, then
// closes it so the next test re-opens cleanly.
#[path = "../tests/unit/backup_store.rs"]
mod tests;
