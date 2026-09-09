//! Connection setup shared by the independently owned SQLite stores.
use rusqlite::Connection;
use std::{sync::Mutex, time::Duration};

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
#[path = "../tests/unit/sqlite.rs"]
mod tests;
