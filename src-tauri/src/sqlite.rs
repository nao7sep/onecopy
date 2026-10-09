//! Connection setup shared by the independently owned SQLite stores.
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::{path::Path, sync::Mutex, time::Duration};

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
        // its write-ahead log, which may hold the newest data.
        let Ok(connection) = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            return Ok(None);
        };
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
    let set_aside = crate::storage::quarantine_name(path);
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
        file: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        quarantined_to: set_aside.to_string_lossy().into_owned(),
    }))
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
