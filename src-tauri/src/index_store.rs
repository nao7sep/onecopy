//! The scan index: one SQLite file, `index.sqlite3`, under the storage root.
//! Facts and caches only — everything here is re-derivable from the source
//! directories, so the file is never backed up and may be deleted between runs
//! at the cost of a full re-index (persisted-store-separation conventions).
//!
//! Schema v0, pre-release: evolved in place with `CREATE ... IF NOT EXISTS`
//! (plus a fresh file when a change is large) — no migration scaffolding until
//! release, per PLAYBOOK.
//!
//! The unit model: `contents` holds one row per unique content hash (the
//! logical file every view shows); `paths` holds one row per physical path,
//! N of which share a `content_hash` — the copy count is a COUNT over this
//! join. Timestamp evidence lands per path (filename and filesystem sources
//! differ per copy) with content-level EXIF evidence keyed by hash; a logical
//! item's display time is the earliest resolved time among its paths.

use std::path::Path;

use rusqlite::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS volumes (
  id               INTEGER PRIMARY KEY,
  identity         TEXT NOT NULL UNIQUE,
  label            TEXT,
  last_seen_at_utc TEXT
);

-- Per-source-directory volume identity, recorded at first sight and verified
-- at the session gate and before destructive operations: a substituted drive
-- with an identical directory tree must be caught, not just an absent one.
CREATE TABLE IF NOT EXISTS source_volumes (
  dir              TEXT PRIMARY KEY,
  identity         TEXT NOT NULL,
  recorded_at_utc  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS contents (
  hash            TEXT PRIMARY KEY,
  byte_size       INTEGER NOT NULL,
  kind            TEXT NOT NULL,
  phash           INTEGER,
  camera_make     TEXT,
  camera_model    TEXT,
  width           INTEGER,
  height          INTEGER,
  duration_ms     INTEGER,
  sharpness       REAL,
  strip_frames    INTEGER,
  derived_at_utc  TEXT
);

CREATE TABLE IF NOT EXISTS paths (
  id               INTEGER PRIMARY KEY,
  volume_id        INTEGER REFERENCES volumes(id),
  abs_path         TEXT NOT NULL UNIQUE,
  dir_path         TEXT NOT NULL,
  file_name        TEXT NOT NULL,
  stem             TEXT NOT NULL DEFAULT '',
  ext              TEXT NOT NULL DEFAULT '',
  kind             TEXT NOT NULL,
  size             INTEGER,
  mtime_ms         INTEGER,
  birthtime_ms     INTEGER,
  prehash          TEXT,
  content_hash     TEXT REFERENCES contents(hash),
  indexed_at_utc   TEXT,
  missing          INTEGER NOT NULL DEFAULT 0,
  companion_of     INTEGER REFERENCES paths(id),
  resolved_utc_ms  INTEGER,
  resolved_source  TEXT,
  date_only        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_paths_content_hash ON paths (content_hash);
CREATE INDEX IF NOT EXISTS idx_paths_dir ON paths (dir_path);
CREATE INDEX IF NOT EXISTS idx_paths_pairing ON paths (dir_path, stem);
CREATE INDEX IF NOT EXISTS idx_paths_resolved ON paths (kind, resolved_utc_ms);

CREATE TABLE IF NOT EXISTS evidence (
  id            INTEGER PRIMARY KEY,
  content_hash  TEXT REFERENCES contents(hash),
  path_id       INTEGER REFERENCES paths(id),
  source        TEXT NOT NULL,
  raw           TEXT,
  parsed_utc_ms INTEGER,
  offset_known  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_evidence_content ON evidence (content_hash);
CREATE INDEX IF NOT EXISTS idx_evidence_path ON evidence (path_id);

CREATE TABLE IF NOT EXISTS similar_groups (
  id             INTEGER PRIMARY KEY,
  created_at_utc TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS similar_group_members (
  group_id     INTEGER NOT NULL REFERENCES similar_groups(id),
  content_hash TEXT NOT NULL REFERENCES contents(hash),
  PRIMARY KEY (group_id, content_hash)
);

CREATE TABLE IF NOT EXISTS issues (
  id             INTEGER PRIMARY KEY,
  path           TEXT,
  kind           TEXT NOT NULL,
  message        TEXT,
  created_at_utc TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_issues_kind ON issues (kind);

CREATE TABLE IF NOT EXISTS scan_dirs (
  id                    INTEGER PRIMARY KEY,
  root                  TEXT NOT NULL UNIQUE,
  volume_id             INTEGER REFERENCES volumes(id),
  last_completed_at_utc TEXT,
  dirty                 INTEGER NOT NULL DEFAULT 0
);
";

/// Opens (creating if needed) the index DB with the fleet's SQLite posture:
/// WAL so the scanner writes while the UI reads, and a busy timeout so a
/// contended write waits rather than failing with SQLITE_BUSY.
pub fn open(db_file: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_file).map_err(|e| e.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(conn)
}

// EXCEPTION (tests-folder conventions): the schema-shape test stays in-file
// because it asserts the private SCHEMA constant's effect on a fresh file,
// which has no public seam. The copy-count semantics it used to sit beside
// moved to tests/queries_tests.rs, where they are asserted through the real
// query instead of a SELECT the test wrote itself.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_schema_and_is_idempotent() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-index-")
            .tempdir()
            .unwrap();
        let db = dir.path().join("index.sqlite3");

        let conn = open(&db).unwrap();
        // All v0 tables exist.
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // Set EQUALITY, not a subset: the previous list silently omitted
        // source_volumes, so that table could have been dropped while this
        // test still passed and verify_source_dirs only failed at runtime.
        // Equality also forces a NEW table to be declared here deliberately.
        let mut expected = vec![
            "contents",
            "evidence",
            "issues",
            "paths",
            "scan_dirs",
            "similar_group_members",
            "similar_groups",
            "source_volumes",
            "volumes",
        ];
        expected.sort_unstable();
        let actual: Vec<&str> = tables
            .iter()
            .map(String::as_str)
            .filter(|t| !t.starts_with("sqlite_"))
            .collect();
        assert_eq!(actual, expected, "the schema's table set changed");
        drop(stmt);
        drop(conn);

        // Re-opening an existing file is fine (IF NOT EXISTS schema).
        let conn = open(&db).unwrap();
        conn.execute(
            "INSERT INTO issues (kind, message, created_at_utc) VALUES ('test', 'x', '2026-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
    }

}
