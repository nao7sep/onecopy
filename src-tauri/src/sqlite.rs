//! Connection setup shared by the independently owned SQLite stores.
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::{path::{Path, PathBuf}, sync::Mutex, time::Duration};

/// Sets aside a store SQLite reports as not a database or corrupt, and, when
/// `unmarked_unreadable`, one whose schema has no format marker, renaming it
/// with its write-ahead log and index to `<stem>-<yyyymmdd-hhmmss-utc>.invalid`
/// without replacing anything, so the launch starts a fresh store and every
/// byte is kept. An absent store, a readable one, one a newer OneCopy wrote and
/// one that cannot be opened right now (I/O, permission) are left as they are.
/// Runs before anything else opens the store. A rename that fails is an error
/// naming the file.
pub(crate) fn set_aside_if_unreadable(
    path: &Path,
    unmarked_unreadable: bool,
) -> Result<Option<crate::storage::QuarantineRecord>, String> {
    use rusqlite::ErrorCode;
    if let Some(intent) = read_quarantine_intent(path)? {
        return resume_quarantine(path, &intent, &mut crate::fs_publish::rename_no_replace).map(Some);
    }
    let malformed = |error: &rusqlite::Error| {
        matches!(
            error.sqlite_error_code(),
            Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
        )
    };
    if !path.try_exists().map_err(|error| quarantine_error(path, error))? { // data root
        return Ok(None);
    }
    let unreadable = {
        // Read-only: a read-write probe of a damaged store lets SQLite discard
        // its write-ahead log, which may hold the newest data.
        let Ok(connection) = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return Ok(None);
        };
        // A store held busy is not damaged; waiting would only delay launch.
        if connection.busy_timeout(Duration::from_millis(100)).is_err() {
            return Ok(None);
        }
        match connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0)) {
            Err(error) => malformed(&error),
            Ok(version) if version < 0 => unmarked_unreadable,
            Ok(version) if version > 0 => false,
            Ok(_) => match connection.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row.get::<_, i64>(0)) {
                Err(error) => malformed(&error),
                Ok(objects) => unmarked_unreadable && objects > 0,
            },
        }
    };
    if !unreadable {
        return Ok(None);
    }
    let intent = prepare_quarantine(path)?;
    resume_quarantine(path, &intent, &mut crate::fs_publish::rename_no_replace).map(Some)
}

// This small recovery record must reach disk before any member moves. Startup
// resumes it before probing or opening the canonical database, even when that
// database has already moved. A malformed intent blocks replacement writes.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarantineIntent {
    target: String,
    wal: bool,
    shm: bool,
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn quarantine_intent_path(path: &Path) -> PathBuf {
    with_suffix(path, ".quarantine.json")
}

fn quarantine_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!("could not preserve the unreadable {}: {error}", crate::native_path::label(path))
}

fn read_quarantine_intent(path: &Path) -> Result<Option<QuarantineIntent>, String> {
    let intent_path = quarantine_intent_path(path);
    use std::io::Read;
    let file = match std::fs::File::open(&intent_path) { // data root
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(quarantine_error(&intent_path, error)),
    };
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).map_err(|error| quarantine_error(&intent_path, error))?;
    if bytes.len() > 4096 { return Err(quarantine_error(&intent_path, "oversized recovery record")); }
    let intent: QuarantineIntent = serde_json::from_slice(&bytes)
        .map_err(|error| quarantine_error(&intent_path, error))?;
    let target = crate::native_path::from_text(&intent.target);
    // Recovery names only a sibling, never an arbitrary path from stored text.
    if target.file_name() != Some(target.as_os_str()) || target == path.file_name().unwrap_or_default() {
        return Err(quarantine_error(&intent_path, "invalid recovery target"));
    }
    Ok(Some(intent))
}

fn prepare_quarantine(path: &Path) -> Result<QuarantineIntent, String> {
    use std::io::Write;
    let target = crate::storage::quarantine_name(path);
    for suffix in ["", "-wal", "-shm"] {
        if with_suffix(&target, suffix).try_exists().map_err(|error| quarantine_error(path, error))? {
            return Err(quarantine_error(path, "the recovery destination is already occupied"));
        }
    }
    let intent = QuarantineIntent {
        target: crate::native_path::encode(target.file_name().ok_or("store has no file name")?).into_owned(),
        wal: with_suffix(path, "-wal").try_exists().map_err(|error| quarantine_error(path, error))?,
        shm: with_suffix(path, "-shm").try_exists().map_err(|error| quarantine_error(path, error))?,
    };
    let bytes = serde_json::to_vec(&intent).map_err(|error| quarantine_error(path, error))?;
    let mut options = std::fs::OpenOptions::new(); // data root
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let intent_path = quarantine_intent_path(path);
    let mut file = options.open(&intent_path).map_err(|error| quarantine_error(&intent_path, error))?;
    file.write_all(&bytes).map_err(|error| quarantine_error(&intent_path, error))?;
    Ok(intent)
}

fn resume_quarantine(
    path: &Path,
    intent: &QuarantineIntent,
    rename: &mut dyn FnMut(&Path, &Path) -> std::io::Result<()>,
) -> Result<crate::storage::QuarantineRecord, String> {
    let parent = path.parent().ok_or("store has no parent")?;
    let target = parent.join(crate::native_path::from_text(&intent.target));
    let intent_path = quarantine_intent_path(path);
    // Also applies on retry after a failed intent flush, before any rename.
    std::fs::OpenOptions::new().read(true).write(true).open(&intent_path) // data root
        .and_then(|file| file.sync_all()).map_err(|error| quarantine_error(&intent_path, error))?;
    crate::fs_publish::sync_directory(parent).map_err(|error| quarantine_error(&intent_path, error))?;
    for (suffix, present) in [("", true), ("-wal", intent.wal), ("-shm", intent.shm)] {
        let source = with_suffix(path, suffix);
        let destination = with_suffix(&target, suffix);
        let source_exists = source.try_exists().map_err(|error| quarantine_error(&source, error))?;
        let destination_exists = destination.try_exists().map_err(|error| quarantine_error(&destination, error))?;
        match (present, source_exists, destination_exists) {
            (true, true, false) => rename(&source, &destination).map_err(|error| quarantine_error(&source, error))?,
            (true, false, true) | (false, false, false) => {}
            _ => return Err(quarantine_error(&source, "recovery files changed; kept every remaining file")),
        }
    }
    // Commit the entire family before retiring its recovery obligation. Only
    // then may startup create a replacement database at the canonical name.
    crate::fs_publish::sync_directory(parent).map_err(|error| quarantine_error(path, error))?;
    std::fs::remove_file(quarantine_intent_path(path)).map_err(|error| quarantine_error(path, error))?; // data root
    crate::fs_publish::sync_directory(parent).map_err(|error| quarantine_error(path, error))?;
    Ok(crate::storage::QuarantineRecord {
        file: crate::native_path::encode(path.file_name().ok_or("store has no file name")?).into_owned(),
        quarantined_to: crate::native_path::encode(&target).into_owned(),
    })
}

// transaction-and-external-effect-conventions. With an attached records store,
// taking its write lock before reading prevents a concurrent logger commit
// from invalidating a snapshot that this transaction later needs to write.
pub(crate) fn write_transaction(conn: &Connection) -> rusqlite::Result<Transaction<'_>> {
    Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
}

pub(crate) struct JournalSetup(Mutex<()>);

impl JournalSetup {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(()))
    }

    pub(crate) fn configure(&self, conn: &Connection, timeout: Duration) -> Result<(), String> {
        conn.busy_timeout(timeout)
            .map_err(|error| error.to_string())?;
        // Journal-mode conversion cannot run inside the schema transaction.
        // Concurrent read-to-exclusive lock upgrades may return SQLITE_BUSY
        // without invoking SQLite's busy handler. Serialize this short setup
        // boundary, not database reads/writes, and never rewrite an existing WAL.
        let _setup = self
            .0
            .lock()
            .map_err(|_| "SQLite journal setup is unavailable.")?;
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|error| format!("read SQLite journal mode: {error}"))?;
        if mode != "wal" {
            let mode: String = conn
                .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
                .map_err(|error| format!("enable SQLite WAL: {error}"))?;
            if mode != "wal" {
                return Err(format!("SQLite WAL is unavailable (journal mode {mode})."));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises journal configuration of a
// module that is private to the crate.
#[path = "../tests/unit/sqlite.rs"]
mod tests;

/// SQLite's Unix filename API accepts native bytes. Binding them as a blob
/// and casting inside SQL avoids Rust's UTF-8 text conversion. Windows uses
/// SQLite's UTF-8 API, so reject unpaired UTF-16 rather than opening a substitute.
pub(crate) fn filename_bytes(path: &Path) -> Result<&[u8], String> {
    #[cfg(windows)]
    if path.to_str().is_none() {
        return Err("SQLite cannot open a path containing unpaired UTF-16 units.".to_string());
    }
    Ok(path.as_os_str().as_encoded_bytes())
}
