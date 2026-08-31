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
//! item's display time becomes the earliest acceptable resolved time only
//! after every live path has completed date checking.

use std::path::Path;

use rusqlite::Connection;

// This is an idempotent schema-generation stamp, not a migration ladder. The
// index is reconstructible and pre-release: bumping the stamp makes the next
// open apply the complete current schema once, while ordinary read commands
// avoid replaying DDL and replacing triggers on every connection.
const SCHEMA_REVISION: i64 = 6;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS volumes (
  id               INTEGER PRIMARY KEY,
  identity         TEXT NOT NULL UNIQUE,
  label            TEXT,
  last_seen_at_utc TEXT
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
  -- CLIP image embedding (f32 LE blob), present once the similarity model
  -- has seen this content; the cross-device pairing signal.
  embedding       BLOB,
  -- Face score (face.rs's contract): NULL never scored, 0.0 scored faceless,
  -- > 0 the smile-weighted best-face confidence. Orders groups ahead of
  -- sharpness for face-bearing members.
  face_score      REAL,
  strip_frames    INTEGER,
  derived_at_utc  TEXT,
  -- The DERIVE_VERSION that produced this row's cache entries. Both derive
  -- passes treat a row stamped with an older version as pending, so bumping
  -- the constant re-derives the library without touching a user file. Without
  -- it, a derive that completed with wrong or missing output stayed
  -- checkpointed for the life of the index and no rescan could fix it.
  derived_version INTEGER NOT NULL DEFAULT 0
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
CREATE INDEX IF NOT EXISTS idx_paths_companion ON paths (companion_of);
CREATE INDEX IF NOT EXISTS idx_paths_media_repair_by_id ON paths (missing, id);
CREATE INDEX IF NOT EXISTS idx_paths_unhashed_other_section
  ON paths (resolved_utc_ms, id)
  WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL
    AND kind NOT IN ('image', 'video');

-- The UI reads logical items, not physical paths. Keeping this one-row summary
-- beside the source tables lets opening a small month seek that month instead
-- of regrouping the whole library. It is a projection, never independent
-- truth: the triggers below rebuild only the content touched by a path write.
CREATE TABLE IF NOT EXISTS logical_contents (
  content_hash           TEXT PRIMARY KEY REFERENCES contents(hash),
  kind                   TEXT NOT NULL,
  date_state             TEXT NOT NULL
                           CHECK (date_state IN ('pending', 'dated', 'undated')),
  resolved_utc_ms        INTEGER,
  representative_path_id INTEGER NOT NULL REFERENCES paths(id),
  live_copy_count        INTEGER NOT NULL,
  CHECK (
    (date_state = 'dated' AND resolved_utc_ms IS NOT NULL) OR
    (date_state IN ('pending', 'undated') AND resolved_utc_ms IS NULL)
  )
);
CREATE INDEX IF NOT EXISTS idx_logical_contents_section
  ON logical_contents (kind, resolved_utc_ms, content_hash);
CREATE INDEX IF NOT EXISTS idx_logical_contents_work
  ON logical_contents (kind, content_hash);

-- Only the current trigger definitions may maintain the projection.
DROP TRIGGER IF EXISTS paths_logical_after_insert;
DROP TRIGGER IF EXISTS paths_logical_after_update;
DROP TRIGGER IF EXISTS paths_logical_after_delete;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_insert_v2
AFTER INSERT ON paths
WHEN NEW.content_hash IS NOT NULL
BEGIN
  INSERT OR REPLACE INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count)
  SELECT c.hash,
         CASE WHEN c.kind IN ('image', 'video') THEN c.kind ELSE 'other' END,
         CASE
           WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) > 0
             THEN 'pending'
           WHEN MIN(p.resolved_utc_ms) IS NULL THEN 'undated'
           ELSE 'dated'
         END,
         CASE
           WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) = 0
             THEN MIN(p.resolved_utc_ms)
           ELSE NULL
         END,
         (SELECT ranked.id FROM paths ranked
          WHERE ranked.content_hash = c.hash
            AND ranked.missing = 0 AND ranked.companion_of IS NULL
          ORDER BY ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                   ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
         LIMIT 1),
         (SELECT COUNT(*) FROM paths cp
          WHERE cp.content_hash = c.hash AND cp.missing = 0
            AND cp.companion_of IS NULL)
  FROM contents c JOIN paths p ON p.content_hash = c.hash
  WHERE c.hash = NEW.content_hash AND p.missing = 0 AND p.companion_of IS NULL
  GROUP BY c.hash, c.kind;
END;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_update_v2
AFTER UPDATE OF content_hash, resolved_utc_ms, resolved_source, missing,
                companion_of, file_name ON paths
BEGIN
  DELETE FROM logical_contents
  WHERE content_hash IN (OLD.content_hash, NEW.content_hash);

  INSERT OR REPLACE INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count)
  SELECT c.hash,
         CASE WHEN c.kind IN ('image', 'video') THEN c.kind ELSE 'other' END,
         CASE
           WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) > 0
             THEN 'pending'
           WHEN MIN(p.resolved_utc_ms) IS NULL THEN 'undated'
           ELSE 'dated'
         END,
         CASE
           WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) = 0
             THEN MIN(p.resolved_utc_ms)
           ELSE NULL
         END,
         (SELECT ranked.id FROM paths ranked
          WHERE ranked.content_hash = c.hash
            AND ranked.missing = 0 AND ranked.companion_of IS NULL
          ORDER BY ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                   ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
         LIMIT 1),
         (SELECT COUNT(*) FROM paths cp
          WHERE cp.content_hash = c.hash AND cp.missing = 0
            AND cp.companion_of IS NULL)
  FROM contents c JOIN paths p ON p.content_hash = c.hash
  WHERE c.hash IN (OLD.content_hash, NEW.content_hash)
    AND p.missing = 0 AND p.companion_of IS NULL
  GROUP BY c.hash, c.kind;
END;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_delete_v2
AFTER DELETE ON paths
WHEN OLD.content_hash IS NOT NULL
BEGIN
  DELETE FROM logical_contents WHERE content_hash = OLD.content_hash;

  INSERT OR REPLACE INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count)
  SELECT c.hash,
         CASE WHEN c.kind IN ('image', 'video') THEN c.kind ELSE 'other' END,
         CASE
           WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) > 0
             THEN 'pending'
           WHEN MIN(p.resolved_utc_ms) IS NULL THEN 'undated'
           ELSE 'dated'
         END,
         CASE
           WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) = 0
             THEN MIN(p.resolved_utc_ms)
           ELSE NULL
         END,
         (SELECT ranked.id FROM paths ranked
          WHERE ranked.content_hash = c.hash
            AND ranked.missing = 0 AND ranked.companion_of IS NULL
          ORDER BY ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                   ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
         LIMIT 1),
         (SELECT COUNT(*) FROM paths cp
          WHERE cp.content_hash = c.hash AND cp.missing = 0
            AND cp.companion_of IS NULL)
  FROM contents c JOIN paths p ON p.content_hash = c.hash
  WHERE c.hash = OLD.content_hash AND p.missing = 0 AND p.companion_of IS NULL
  GROUP BY c.hash, c.kind;
END;

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
CREATE INDEX IF NOT EXISTS idx_evidence_source_raw ON evidence (source, raw);

CREATE TABLE IF NOT EXISTS similar_groups (
  id             INTEGER PRIMARY KEY,
  created_at_utc TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS similar_group_members (
  group_id     INTEGER NOT NULL REFERENCES similar_groups(id),
  content_hash TEXT NOT NULL REFERENCES contents(hash),
  PRIMARY KEY (group_id, content_hash)
);
CREATE INDEX IF NOT EXISTS idx_similar_members_content
  ON similar_group_members (content_hash);

-- Issues are CURRENT-STATE diagnostics. Identity is (kind, path): a recurrence
-- UPDATES the row, so a condition persisting for weeks is one line, not one
-- per scan. `path` is ''
-- when the condition has no file anchor (a rootless walk error); NULLs would
-- break the unique identity, which is why the column is NOT NULL.
CREATE TABLE IF NOT EXISTS issues (
  id             INTEGER PRIMARY KEY,
  path           TEXT NOT NULL DEFAULT '',
  kind           TEXT NOT NULL,
  message        TEXT,
  first_seen_utc TEXT NOT NULL,
  last_seen_utc  TEXT NOT NULL,
  occurrence_count INTEGER NOT NULL DEFAULT 1,
  UNIQUE (kind, path)
);
CREATE INDEX IF NOT EXISTS idx_issues_first_seen
  ON issues (first_seen_utc, id);

-- Recent is restart-persistent notification history, not an operation plan
-- or permanent ledger. Equal notices coalesce and the owning publisher prunes
-- the table to the approved age/count window after each write.
CREATE TABLE IF NOT EXISTS recent_notifications (
  id               INTEGER PRIMARY KEY,
  kind             TEXT NOT NULL,
  path             TEXT NOT NULL DEFAULT '',
  level            TEXT NOT NULL CHECK (level IN ('info', 'warning', 'error')),
  presentation     TEXT NOT NULL CHECK (presentation IN ('timed', 'persistent')),
  message          TEXT NOT NULL,
  first_seen_utc   TEXT NOT NULL,
  last_seen_utc    TEXT NOT NULL,
  occurrence_count INTEGER NOT NULL DEFAULT 1,
  UNIQUE (kind, path, level, presentation, message)
);
CREATE INDEX IF NOT EXISTS idx_recent_notifications_latest
  ON recent_notifications (last_seen_utc DESC, id DESC);

-- Fixed-class output receipts, never jobs. NULL means the class is pending;
-- ready and failed are durable results, while running/paused/waiting belong
-- to the coordinator's ephemeral snapshot.
CREATE TABLE IF NOT EXISTS analysis_receipts (
  content_hash              TEXT PRIMARY KEY REFERENCES contents(hash),
  face_state                TEXT CHECK (face_state IN ('ready', 'failed')),
  face_updated_at_utc       TEXT,
  transcript_state          TEXT CHECK (
                              transcript_state IN
                                ('ready-text', 'ready-empty', 'failed')
                            ),
  transcript_updated_at_utc TEXT
);
CREATE TRIGGER IF NOT EXISTS contents_analysis_after_delete
AFTER DELETE ON contents
BEGIN
  DELETE FROM analysis_receipts WHERE content_hash = OLD.hash;
END;

CREATE TABLE IF NOT EXISTS scan_dirs (
  id                    INTEGER PRIMARY KEY,
  root                  TEXT NOT NULL UNIQUE,
  volume_id             INTEGER REFERENCES volumes(id),
  last_completed_at_utc TEXT,
  dirty                 INTEGER NOT NULL DEFAULT 0,
  relationship_dirty    INTEGER NOT NULL DEFAULT 0
);
";

/// Opens (creating if needed) the index DB with the fleet's SQLite posture:
/// WAL so the scanner writes while the UI reads, and a busy timeout so a
/// contended write waits rather than failing with SQLITE_BUSY.
pub fn open(db_file: &Path) -> Result<Connection, String> {
    // not recorded: index.sqlite3 is a binary, reconstructible scan cache.
    if let Some(parent) = db_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_file).map_err(|e| e.to_string())?;
    conn.create_collation("onecopy_nocase", |left, right| {
        left.to_lowercase().cmp(&right.to_lowercase())
    })
    .map_err(|error| error.to_string())?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    let schema_revision = conn
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| error.to_string())?;
    let logical_table_exists = conn
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM sqlite_master
               WHERE type = 'table' AND name = 'logical_contents')",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?
        != 0;
    let needs_logical_hydration = !logical_table_exists;
    if schema_revision > SCHEMA_REVISION {
        return Err(format!(
            "index schema revision {schema_revision} is newer than this app supports ({SCHEMA_REVISION})"
        ));
    }
    if schema_revision > 0 && schema_revision < SCHEMA_REVISION {
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE IF EXISTS analysis_receipts;
             DROP TABLE IF EXISTS similar_group_members;
             DROP TABLE IF EXISTS similar_groups;
             DROP TABLE IF EXISTS evidence;
             DROP TABLE IF EXISTS logical_contents;
             DROP TABLE IF EXISTS paths;
             DROP TABLE IF EXISTS contents;
             DROP TABLE IF EXISTS scan_dirs;
             DROP TABLE IF EXISTS issues;
             DROP TABLE IF EXISTS recent_notifications;
             DROP TABLE IF EXISTS volumes;
             PRAGMA user_version = 0;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(|error| error.to_string())?;
    }
    if schema_revision < SCHEMA_REVISION || needs_logical_hydration {
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let setup = (|| {
            conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
            // Existing development indexes predate the logical read model.
            // Hydrate inside the table-creation transaction so a crash cannot
            // leave a permanently partial projection; later opens are O(1).
            if needs_logical_hydration {
                conn.execute_batch(
                    "INSERT INTO logical_contents
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
            live_copy_count)
         SELECT c.hash,
                CASE WHEN c.kind IN ('image', 'video') THEN c.kind ELSE 'other' END,
                CASE
                  WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) > 0
                    THEN 'pending'
                  WHEN MIN(p.resolved_utc_ms) IS NULL THEN 'undated'
                  ELSE 'dated'
                END,
                CASE
                  WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) = 0
                    THEN MIN(p.resolved_utc_ms)
                  ELSE NULL
                END,
                (SELECT ranked.id FROM paths ranked
                 WHERE ranked.content_hash = c.hash
                   AND ranked.missing = 0 AND ranked.companion_of IS NULL
                 ORDER BY ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                          ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
                LIMIT 1),
                (SELECT COUNT(*) FROM paths cp
                 WHERE cp.content_hash = c.hash AND cp.missing = 0
                   AND cp.companion_of IS NULL)
         FROM contents c JOIN paths p ON p.content_hash = c.hash
         WHERE p.missing = 0 AND p.companion_of IS NULL
         GROUP BY c.hash, c.kind",
                )
                .map_err(|e| e.to_string())?;
            }
            conn.pragma_update(None, "user_version", SCHEMA_REVISION)
                .map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        })();
        match setup {
            Ok(()) => conn.execute_batch("COMMIT").map_err(|e| e.to_string())?,
            Err(error) => {
                if let Err(rollback_error) = conn.execute_batch("ROLLBACK") {
                    crate::logging::error(
                        "index setup rollback failed",
                        serde_json::json!({
                            "failure": { "message": &error },
                            "error": { "message": rollback_error.to_string() },
                        }),
                    );
                    return Err(format!(
                        "{error}; index setup rollback also failed: {rollback_error}"
                    ));
                }
                return Err(error);
            }
        }
    }
    Ok(conn)
}

// EXCEPTION (tests-folder conventions): the schema-shape test stays in-file
// because it asserts the private SCHEMA constant's effect on a fresh file,
/// Records (or refreshes) one issue. Identity is (kind, path): a recurrence
/// updates the message and last-seen stamp, never inserts a second row, so a
/// condition persisting across many scans stays ONE line. `path` None anchors
/// to '' — the rootless case.
pub fn upsert_issue(
    conn: &Connection,
    path: Option<&str>,
    kind: &str,
    message: &str,
) -> Result<(), String> {
    let now = crate::logging::now_iso_millis();
    conn.execute(
        "INSERT INTO issues (path, kind, message, first_seen_utc, last_seen_utc) \
         VALUES (?1, ?2, ?3, ?4, ?4) \
         ON CONFLICT (kind, path) DO UPDATE \
         SET message = excluded.message,
             last_seen_utc = excluded.last_seen_utc,
             occurrence_count = issues.occurrence_count + 1",
        rusqlite::params![path.unwrap_or(""), kind, message, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Clears an issue whose condition a scan has just found RESOLVED — the
/// success counterpart of `upsert_issue`, which is what makes scan-derived
/// issues current-state rather than a log: a fixed file's row disappears the
/// next time the pipeline touches it. Clearing something never recorded is a
/// no-op by design.
pub fn clear_issues(conn: &Connection, path: &str, kinds: &[&str]) -> Result<(), String> {
    for kind in kinds {
        conn.execute(
            "DELETE FROM issues WHERE kind = ?1 AND path = ?2",
            rusqlite::params![kind, path],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether any issue rows exist at all — the walk and the passes consult this
/// once so a clean index never pays a per-file DELETE for conditions that were
/// never recorded.
pub fn any_issues(conn: &Connection) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM issues)", [], |r| r.get(0))
        .map_err(|error| error.to_string())
}

/// Clears only reconstructible library facts. Durable configuration, managed
/// tools, and authored similarity exclusions live in separate stores and are
/// deliberately outside this transaction.
pub fn clear_reconstructible(conn: &Connection) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute_batch(
            "DELETE FROM analysis_receipts;
             DELETE FROM similar_group_members;
             DELETE FROM similar_groups;
             DELETE FROM evidence;
             DELETE FROM logical_contents;
             DELETE FROM paths;
             DELETE FROM contents;
             DELETE FROM scan_dirs;
             DELETE FROM issues;
             DELETE FROM recent_notifications;
             DELETE FROM volumes;",
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

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
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_REVISION
        );
        // All v0 tables exist.
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // Set EQUALITY, not a subset, so any table change is deliberate.
        let mut expected = vec![
            "analysis_receipts",
            "contents",
            "evidence",
            "issues",
            "logical_contents",
            "paths",
            "recent_notifications",
            "scan_dirs",
            "similar_group_members",
            "similar_groups",
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
        upsert_issue(&conn, Some("/x"), "test", "x").unwrap();
    }

    #[test]
    fn reopening_a_current_index_does_not_publish_schema_writes() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-index-read-open-")
            .tempdir()
            .unwrap();
        let db = dir.path().join("index.sqlite3");
        let observer = open(&db).unwrap();
        let before: i64 = observer
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .unwrap();

        drop(open(&db).unwrap());

        let after: i64 = observer
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .unwrap();
        assert_eq!(
            after, before,
            "an ordinary connection open must not invalidate read caches"
        );
    }

    #[test]
    fn open_hydrates_an_existing_index_when_the_read_model_is_first_added() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-index-upgrade-")
            .tempdir()
            .unwrap();
        let db = dir.path().join("index.sqlite3");
        let conn = open(&db).unwrap();
        conn.execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('h1', 10, 'image');
             INSERT INTO paths
               (abs_path, dir_path, file_name, kind, content_hash, resolved_utc_ms,
                resolved_source)
             VALUES ('/a.jpg', '/', 'a.jpg', 'image', 'h1', 1000, 'metadata');
             DROP TABLE logical_contents;",
        )
        .unwrap();
        drop(conn);

        let conn = open(&db).unwrap();
        let summary: (String, i64, i64) = conn
            .query_row(
                "SELECT date_state, resolved_utc_ms, live_copy_count FROM logical_contents \
                 WHERE content_hash = 'h1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(summary, ("dated".to_string(), 1000, 1));
    }
}
