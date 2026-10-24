//! A disk-backed membership snapshot keeps hashing pages stable while landed
//! identities change which paths qualify for pending discovery.
use rusqlite::{params, Connection, OptionalExtension};

// Require the size seek: SQLite otherwise chooses the content-hash OR
// branches and sorts every pending path again for each distinct size.
pub(crate) fn size_query(after: bool, scope: crate::path_scope::Scope) -> String {
    let seek = if after { "AND size > ?1 AND id <= ?2" } else { "AND id <= ?1" };
    format!("SELECT size FROM paths INDEXED BY idx_paths_pending_size WHERE missing = 0 AND hash_attempt_failed = 0
             AND (content_hash IS NULL OR content_hash GLOB 'p*') {seek} AND {}
             ORDER BY size LIMIT 1", scope.predicate())
}
pub(crate) const KNOWN_SIZE: &str =
    "SELECT EXISTS(SELECT 1 FROM contents WHERE byte_size = ?1 AND NOT hash GLOB 'p*')";

pub(crate) struct Row {
    pub(crate) id: i64,
    pub(crate) abs: String,
    pub(crate) size: i64,
    pub(crate) kind: String,
    pub(crate) provisional: Option<String>,
    pub(crate) prehash: Option<String>,
    pub(crate) complete_version: Option<crate::file_identity::FileVersion>,
}

pub(crate) struct Cohort<'a> {
    conn: &'a Connection,
}

impl<'a> Cohort<'a> {
    pub(crate) fn new(conn: &'a Connection) -> Result<Self, String> {
        conn.execute_batch(
            "CREATE TEMP TABLE onecopy_hash_cohort (
               id INTEGER PRIMARY KEY,
               prehash TEXT,
               complete_version TEXT,
               landed_hash TEXT
             );",
        ).map_err(|error| error.to_string())?;
        // Own the table before subsequent fallible setup so failed setup also
        // removes its private membership snapshot.
        let cohort = Self { conn };
        conn.execute_batch(
            "CREATE INDEX temp.onecopy_hash_cohort_prehash
             ON onecopy_hash_cohort(prehash, id);",
        ).map_err(|error| error.to_string())?;
        Ok(cohort)
    }

    #[cfg(test)]
    pub(crate) fn select_size(&self, size: i64, upper_id: i64) -> Result<(), String> {
        self.select_size_in_scope(size, upper_id, crate::path_scope::Scope::Library)
    }

    pub(crate) fn select_size_in_scope(&self, size: i64, upper_id: i64, scope: crate::path_scope::Scope) -> Result<(), String> {
        self.conn.execute("DELETE FROM temp.onecopy_hash_cohort", [])
            .map_err(|error| error.to_string())?;
        self.conn.execute(
            &format!("INSERT INTO temp.onecopy_hash_cohort (id, prehash)
             SELECT id, prehash FROM paths INDEXED BY idx_paths_pending_size
             WHERE missing = 0 AND hash_attempt_failed = 0
               AND (content_hash IS NULL OR content_hash GLOB 'p*')
               AND size = ?1 AND id <= ?2 AND {}", scope.predicate()),
            params![size, upper_id],
        ).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) fn len(&self) -> Result<u64, String> {
        self.conn.query_row("SELECT COUNT(*) FROM temp.onecopy_hash_cohort", [], |row| Ok(row.get::<_, i64>(0)? as u64))
            .map_err(|error| error.to_string())
    }

    pub(crate) fn rows(&self, after_id: i64, prehash: Option<&str>) -> Result<Vec<Row>, String> {
        let sql = match prehash {
            None => "SELECT p.id, p.abs_path, p.size, p.kind, p.content_hash, p.prehash, c.complete_version
                     FROM temp.onecopy_hash_cohort c JOIN paths p ON p.id = c.id
                     WHERE c.id > ?1 ORDER BY c.id LIMIT 128",
            Some(_) => "SELECT p.id, p.abs_path, p.size, p.kind, p.content_hash, p.prehash, c.complete_version
                        FROM temp.onecopy_hash_cohort c JOIN paths p ON p.id = c.id
                        WHERE c.id > ?1 AND c.prehash = ?2 ORDER BY c.id LIMIT 128",
        };
        let mut statement = self.conn.prepare(sql).map_err(|error| error.to_string())?;
        let read = |row: &rusqlite::Row<'_>| Ok(Row {
            id: row.get(0)?,
            abs: row.get(1)?,
            size: row.get(2)?,
            kind: row.get(3)?,
            provisional: row.get(4)?,
            prehash: row.get(5)?,
            complete_version: row.get::<_, Option<String>>(6)?.map(|value| {
                serde_json::from_str(&value).map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                    6, rusqlite::types::Type::Text, Box::new(error),
                ))
            }).transpose()?,
        });
        let rows = match prehash {
            None => statement.query_map(params![after_id], read),
            Some(prehash) => statement.query_map(params![after_id, prehash], read),
        }.map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())
    }

    pub(crate) fn next_prehash(&self, after: Option<&str>) -> Result<Option<String>, String> {
        match after {
            None => self.conn.query_row(
                "SELECT prehash FROM temp.onecopy_hash_cohort
                 WHERE prehash IS NOT NULL ORDER BY prehash LIMIT 1",
                [], |row| row.get(0),
            ),
            Some(after) => self.conn.query_row(
                "SELECT prehash FROM temp.onecopy_hash_cohort
                 WHERE prehash > ?1 ORDER BY prehash LIMIT 1",
                [after], |row| row.get(0),
            ),
        }.optional().map_err(|error| error.to_string())
    }

    pub(crate) fn group_len(&self, prehash: &str) -> Result<u64, String> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM temp.onecopy_hash_cohort WHERE prehash = ?1",
            [prehash], |row| Ok(row.get::<_, i64>(0)? as u64),
        ).map_err(|error| error.to_string())
    }

    pub(crate) fn set_prehash(&self, id: i64, prehash: &str) -> Result<(), String> {
        self.conn.execute(
            "UPDATE temp.onecopy_hash_cohort SET prehash = ?2 WHERE id = ?1",
            params![id, prehash],
        ).map(|_| ()).map_err(|error| error.to_string())
    }

    pub(crate) fn set_complete_version(&self, id: i64, version: crate::file_identity::FileVersion) -> Result<(), String> {
        let value = serde_json::to_string(&version).map_err(|error| error.to_string())?;
        self.conn.execute("UPDATE temp.onecopy_hash_cohort SET complete_version = ?2 WHERE id = ?1", params![id, value])
            .map(|_| ()).map_err(|error| error.to_string())
    }

    pub(crate) fn discard(&self, id: i64) -> Result<(), String> {
        self.conn.execute("DELETE FROM temp.onecopy_hash_cohort WHERE id = ?1", [id])
            .map(|_| ()).map_err(|error| error.to_string())
    }

    pub(crate) fn set_hash(&self, id: i64, hash: &str) -> Result<(), String> {
        self.conn.execute(
            "UPDATE temp.onecopy_hash_cohort SET landed_hash = ?2 WHERE id = ?1",
            params![id, hash],
        ).map(|_| ()).map_err(|error| error.to_string())
    }

    pub(crate) fn distinct_hashes(&self, prehash: &str) -> Result<u64, String> {
        self.conn.query_row(
            "SELECT COUNT(DISTINCT landed_hash) FROM temp.onecopy_hash_cohort WHERE prehash = ?1",
            [prehash], |row| Ok(row.get::<_, i64>(0)? as u64),
        ).map_err(|error| error.to_string())
    }
}

impl Drop for Cohort<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.conn.execute_batch("DROP TABLE temp.onecopy_hash_cohort") {
            crate::logging::warn("hash cohort cleanup failed", serde_json::json!({"error": error.to_string()}));
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/hash_cohort.rs"]
mod tests;
