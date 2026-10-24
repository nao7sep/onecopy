//! Connection-local admission for a bounded directory recheck. The scope is
//! captured before retries reopen and never expands to unrelated pending work.
use rusqlite::Connection;

#[derive(Clone, Copy)]
pub(crate) enum Scope {
    Library,
    AdmittedDirectories,
}

impl Scope {
    pub(crate) fn predicate(self) -> &'static str {
        match self {
            Self::Library => "1",
            Self::AdmittedDirectories => "dir_path IN (SELECT dir_path FROM temp.admitted_information_dirs)",
        }
    }
}

pub(crate) struct Directories<'a>(&'a Connection);

impl<'a> Directories<'a> {
    pub(crate) fn new(conn: &'a Connection, dirs: &[String]) -> Result<Self, String> {
        conn.execute_batch("CREATE TEMP TABLE admitted_information_dirs (dir_path TEXT PRIMARY KEY COLLATE BINARY) WITHOUT ROWID")
            .map_err(|error| error.to_string())?;
        let owner = Self(conn);
        let mut insert = conn.prepare("INSERT OR IGNORE INTO admitted_information_dirs VALUES (?1)")
            .map_err(|error| error.to_string())?;
        for dir in dirs { insert.execute([dir]).map_err(|error| error.to_string())?; }
        Ok(owner)
    }
}

impl Drop for Directories<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.0.execute_batch("DROP TABLE temp.admitted_information_dirs") {
            crate::logging::warn("information scope cleanup failed", serde_json::json!({"error": error.to_string()}));
        }
    }
}
