//! The write-through data-backup store (data-backup conventions). It owns one
//! add-only SQLite file, `backups.sqlite3`, directly under onecopy's storage
//! root (`ONECOPY_HOME` or `~/.onecopy`, resolved in one place by
//! `paths::data_root` — never a hardcoded path). Every managed *text* save
//! records the exact bytes it just wrote here, strictly AFTER its atomic rename
//! lands (see `write_atomic` in lib.rs), so the history is always as current as
//! the last save. There is no startup scan, no periodic pass, no restore path.
//!
//! SQLite binding: `rusqlite` with the `bundled` feature. Bundled compiles
//! SQLite from source into the binary, so the store needs no system libsqlite3
//! and adds no packaging churn — the lowest-friction binding for the Rust core,
//! per the convention's "built-in or lowest-friction binding" rule. It is
//! synchronous, exactly what a record-after-rename hook wants, and stores/reads
//! a BLOB as raw `&[u8]`/`Vec<u8>` so CR/LF, a BOM, and non-UTF-8 bytes are
//! byte-identical.
//!
//! Two absolute musts drive every line below (they are not best-effort
//! aspirations):
//!
//!  - It never breaks a save and never crashes the app. The save has already
//!    succeeded — the file is on disk before `record` is called — so any failure
//!    here (the DB is locked, the disk is full, an insert fails) is caught,
//!    logged once at `warn`, and swallowed. A lost record self-heals on the next
//!    save of that file, whose content will differ from the last recorded row.
//!    Nothing here ever panics or propagates an error to the caller.
//!  - It logs only failures. A successful record logs NOTHING; a line per save
//!    would flood the log.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::logging;

/// The store's on-disk file name under the storage root, named in exactly one
/// place (pinned by the storage_file_names integration test).
pub const BACKUPS_DB_FILE_NAME: &str = "backups.sqlite3";

/// The one add-only table. `content` is a BLOB of the exact bytes written —
/// never decoded text, so CR/LF, a BOM, and non-UTF-8 bytes are stored
/// byte-identically. `written_at_utc` is the serialized ISO-8601-ms form
/// (`2026-07-06T04:05:12.345Z`), a data value — NEVER the
/// `yyyymmdd-hhmmss-fff-utc` filename stamp. The `(path, id)` index serves the
/// latest-row-per-path dedup lookup.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS backups (
  id             INTEGER PRIMARY KEY,
  path           TEXT NOT NULL,
  content        BLOB NOT NULL,
  content_sha256 TEXT NOT NULL,
  byte_size      INTEGER NOT NULL,
  written_at_utc TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_backups_path_id ON backups (path, id);
";

/// Session state for the store singleton.
///
/// `initialized` records that `init` has run; `conn` is `Some` when the store is
/// open and `None` when it could not be opened (a single warn was already logged,
/// and every later `record` becomes a no-op rather than retrying a broken open on
/// every save). Wrapped in a `Mutex` because `record` is called synchronously
/// from the atomic-write path, which may run on any thread, and a
/// `rusqlite::Connection` is not `Sync`.
struct StoreState {
    conn: Option<Connection>,
    initialized: bool,
}

fn store() -> &'static Mutex<StoreState> {
    static STORE: OnceLock<Mutex<StoreState>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(StoreState {
            conn: None,
            initialized: false,
        })
    })
}

/// Open and initialize the store once, at startup, best-effort. Creates the
/// `backups.sqlite3` file's parent directory if needed, opens the connection,
/// switches on WAL, sets a busy timeout, and creates the table + index. On any
/// failure it logs ONE `warn`, leaves recording disabled for the session, and
/// never panics — startup is never blocked by a backup-store problem.
///
/// WAL is what lets the tolerated two-instance case serialize safely without a
/// cross-process lock; the short `busy_timeout` gives a concurrent writer one
/// scheduling beat, then drops the best-effort record instead of making an
/// ordinary app save visibly wait.
pub fn init(store_file: PathBuf) {
    let mut state = lock();
    state.initialized = true;
    match open(&store_file) {
        Ok(conn) => state.conn = Some(conn),
        Err(err) => {
            logging::warn(
                "backup store: could not open; recording disabled for this session",
                json!({ "file": store_file.to_string_lossy(), "error": { "message": err } }),
            );
            state.conn = None;
        }
    }
}

fn open(store_file: &Path) -> Result<Connection, String> {
    // not recorded: backups.sqlite3 is the store itself — binary, and written by
    // this backup layer, not through the managed-text atomic-write path — so it
    // never records itself. No recursion, no special case (data-backup
    // conventions: "A binary store, excluded from itself").
    // The first writer under the root does the `mkdir -p`; the store may be the
    // first thing written on a fresh root.
    if let Some(parent) = store_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(store_file).map_err(|e| e.to_string())?;
    // WAL for the tolerated two-instance case; contention may delay a save by
    // at most 100 ms before this best-effort record is dropped and warned.
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 100)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(conn)
}

/// SHA-256 of the exact bytes, lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Record one managed-text write: `absolute_path` is the FULL absolute path of
/// the file as written; `bytes` is the exact raw bytes just written (the caller
/// already holds them — never a re-read of the file).
///
/// Dedup by content hash per path: the new content's SHA-256 is compared against
/// the latest row for the same `path`, and the insert is SKIPPED when they are
/// equal. This collapses consecutive identical saves (an autosave with no real
/// change writes no row) while still recording every genuinely distinct version —
/// including a revert, whose content differs from the immediately preceding row.
///
/// Best-effort and silent on success; any failure is caught, logged once at
/// `warn` (file + reason), and swallowed. It never panics, never crashes the app,
/// and never breaks the save.
pub fn record(absolute_path: &Path, bytes: &[u8]) {
    let state = lock();
    let Some(conn) = state.conn.as_ref() else {
        // Store never opened (open failed at startup, or init hasn't run under a
        // test that doesn't exercise it): disabled for the session, already warned
        // once if it was an open failure. No-op.
        return;
    };
    let path = absolute_path.to_string_lossy();
    if let Err(err) = try_record(conn, &path, bytes) {
        logging::warn(
            "backup store: failed to record a managed write",
            json!({ "file": path, "error": { "message": err.to_string() } }),
        );
    }
}

/// The fallible core of `record`, factored out so the one `warn` site in `record`
/// catches every failure path uniformly.
fn try_record(conn: &Connection, path: &str, bytes: &[u8]) -> Result<(), rusqlite::Error> {
    let hash = sha256_hex(bytes);
    // Compare against the latest row for this same path only — a cheap,
    // append-only check with no full-history scan (served by the (path, id)
    // index). No prior row (QueryReturnedNoRows) means never captured -> record.
    let latest: Option<String> = match conn.query_row(
        "SELECT content_sha256 FROM backups WHERE path = ?1 ORDER BY id DESC LIMIT 1",
        [path],
        |row| row.get::<_, String>(0),
    ) {
        Ok(h) => Some(h),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(other) => return Err(other),
    };
    if latest.as_deref() == Some(hash.as_str()) {
        return Ok(()); // unchanged since the last recorded version — dedup skip
    }
    conn.execute(
        "INSERT INTO backups (path, content, content_sha256, byte_size, written_at_utc) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![path, bytes, hash, bytes.len() as i64, logging::now_iso_millis()],
    )?;
    Ok(())
}

fn lock() -> std::sync::MutexGuard<'static, StoreState> {
    // Recover from a poisoned mutex: a prior panic elsewhere must not wedge the
    // store shut. (Nothing in this module panics while holding the lock, so the
    // recovered state is consistent.)
    store().lock().unwrap_or_else(|p| p.into_inner())
}

/// Close the store and reset the singleton (best-effort). For tests that need to
/// release the file handle between throwaway roots so the next `init` re-opens
/// against the current root; the app itself lets the process exit close it.
#[cfg(test)]
pub fn close_for_test() {
    let mut state = lock();
    // Dropping the Connection closes it.
    state.conn = None;
    state.initialized = false;
}

#[cfg(test)]
mod tests {
    // EXCEPTION to the tests-live-in-tests/ rule (tests-folder
    // conventions, Rust form): these tests exercise genuinely private
    // internals that cannot reasonably be promoted — promoting them
    // would widen the module's surface just to test through it.
    use super::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicU32, Ordering};

    // The store singleton is process-global, so every test that touches it is
    // marked `#[serial(backup_store)]`. `cargo test` runs tests in parallel threads
    // within one process; the shared `backup_store` key makes this group (plus the
    // lib.rs atomic-write test, which reaches `record` through `write_atomic`)
    // mutually exclusive, so no test resets/reopens the singleton out from under
    // another. Each test opens a fresh throwaway store file, exercises it, then
    // closes it so the next test re-opens cleanly.

    fn unique_store_file(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "onecopy-backupstore-test-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("backups.sqlite3")
    }

    // Opens a throwaway store, runs `body` against a direct connection to the same
    // file for assertions, and always closes the singleton afterward.
    // Serialization is provided by `#[serial(backup_store)]` on each caller, not by
    // an in-module lock, so this group is also mutually exclusive with the lib.rs
    // atomic-write test that shares the same key.
    fn with_store<F: FnOnce(&Path)>(label: &str, body: F) {
        let file = unique_store_file(label);
        init(file.clone());
        body(&file);
        close_for_test();
    }

    // A read-only view of every row for a path, in insert order, for assertions.
    fn rows_for(file: &Path, path: &str) -> Vec<(Vec<u8>, String, i64, String)> {
        let conn = Connection::open(file).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT content, content_sha256, byte_size, written_at_utc \
                 FROM backups WHERE path = ?1 ORDER BY id ASC",
            )
            .unwrap();
        let rows = stmt
            .query_map([path], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows
    }

    #[test]
    #[serial(backup_store)]
    fn content_blob_is_byte_identical_including_crlf_and_non_utf8() {
        with_store("blob-fidelity", |file| {
            // A CR/LF pair, a UTF-8 BOM, and a lone 0xFF byte (invalid UTF-8):
            // proves the BLOB stores raw bytes, never decoded/normalized text.
            let raw: &[u8] = &[0xEF, 0xBB, 0xBF, b'a', b'\r', b'\n', b'b', 0xFF];
            let p = "/abs/doc.json";
            record(Path::new(p), raw);

            let rows = rows_for(file, p);
            assert_eq!(rows.len(), 1);
            let (content, hash, byte_size, _written) = &rows[0];
            assert_eq!(content.as_slice(), raw, "content BLOB must be byte-identical");
            assert_eq!(*byte_size, raw.len() as i64);
            assert_eq!(hash, &sha256_hex(raw), "hash is over the raw bytes");
        });
    }

    #[test]
    #[serial(backup_store)]
    fn written_at_utc_is_serialized_iso_ms_not_the_filename_stamp() {
        with_store("iso-shape", |file| {
            let p = "/abs/a.json";
            record(Path::new(p), b"x");
            let rows = rows_for(file, p);
            let written = &rows[0].3;
            // Serialized ISO-8601-ms shape: yyyy-mm-ddThh:mm:ss.fffZ.
            assert!(
                written.len() == 24
                    && &written[4..5] == "-"
                    && &written[7..8] == "-"
                    && &written[10..11] == "T"
                    && &written[13..14] == ":"
                    && &written[16..17] == ":"
                    && &written[19..20] == "."
                    && written.ends_with('Z'),
                "written_at_utc {written:?} must be serialized ISO-8601-ms (2026-07-06T04:05:12.345Z)"
            );
            // Must NOT be the yyyymmdd-hhmmss-fff-utc filename stamp.
            assert!(!written.ends_with("-utc"), "must not be the filename stamp");
            assert!(!written.contains("-utc"), "must not be the filename stamp");
        });
    }

    #[test]
    #[serial(backup_store)]
    fn dedup_skips_an_unchanged_re_save() {
        with_store("dedup", |file| {
            let p = "/abs/b.json";
            record(Path::new(p), b"same");
            record(Path::new(p), b"same"); // identical -> deduped, no new row
            assert_eq!(rows_for(file, p).len(), 1, "an unchanged re-save writes no row");
        });
    }

    #[test]
    #[serial(backup_store)]
    fn a_changed_save_and_a_revert_each_insert_a_row() {
        with_store("changed-and-revert", |file| {
            let p = "/abs/c.json";
            record(Path::new(p), b"v1");
            record(Path::new(p), b"v2"); // changed -> new row
            record(Path::new(p), b"v1"); // revert to v1: differs from the LATEST (v2) -> new row
            let rows = rows_for(file, p);
            assert_eq!(rows.len(), 3, "changed save and revert each insert a row");
            assert_eq!(rows[0].0, b"v1");
            assert_eq!(rows[1].0, b"v2");
            assert_eq!(rows[2].0, b"v1"); // the revert is recorded as the new version it is
        });
    }

    #[test]
    #[serial(backup_store)]
    fn dedup_is_per_path_not_global() {
        with_store("per-path", |file| {
            // Identical content under two different paths each records (dedup is
            // per-path against that path's latest row, never global).
            record(Path::new("/abs/x.json"), b"same");
            record(Path::new("/abs/y.json"), b"same");
            assert_eq!(rows_for(file, "/abs/x.json").len(), 1);
            assert_eq!(rows_for(file, "/abs/y.json").len(), 1);
        });
    }

    #[test]
    #[serial(backup_store)]
    fn record_is_a_silent_no_op_when_the_store_never_opened() {
        // Best-effort: with the store disabled (never init'd / closed), a record
        // call must not panic and must simply do nothing.
        close_for_test(); // ensure disabled state
        record(Path::new("/abs/whatever.json"), b"data"); // must not panic
    }

    #[test]
    #[serial(backup_store)]
    fn record_never_panics_on_a_broken_connection() {
        // Best-effort under a store failure: prove the no-throw contract by
        // pointing init at an un-creatable path — the open fails, recording is
        // disabled, and a subsequent record is a silent no-op (never a panic,
        // never a crash). A path whose parent is a FILE, so create_dir_all +
        // open must fail.
        let dir = std::env::temp_dir().join(format!(
            "onecopy-backupstore-badpath-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file_as_parent = dir.join("not-a-dir");
        std::fs::write(&file_as_parent, b"x").unwrap(); // a regular file
        let store_file = file_as_parent.join("backups.sqlite3"); // parent is a file -> mkdir fails

        init(store_file); // open fails -> disabled, one warn logged, no panic
        record(Path::new("/abs/whatever.json"), b"data"); // silent no-op, no panic
        close_for_test();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
