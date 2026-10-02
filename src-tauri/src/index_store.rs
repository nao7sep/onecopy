//! The scan index: one SQLite file, `index.sqlite3`, under the storage root.
//! Scan facts, derived caches, and expensive analysis results. Whole-file
//! lifecycle archives preserve this store; its Issues and analysis failures
//! are records in the attached `records.sqlite3`.
//!
//! An index written by an earlier schema revision is rebuilt from the files.
//!
//! The unit model: `contents` holds one row per unique content hash (the
//! logical file every view shows); `paths` holds one row per physical path,
//! N of which share a `content_hash` — the copy count is a COUNT over this
//! join. Timestamp evidence lands per path (filename and filesystem sources
//! differ per copy) with content-level EXIF evidence keyed by hash; a logical
//! item's display time becomes the earliest acceptable resolved time only
//! after every live path has completed date checking.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

// Ordinary reads do not replay DDL.
const SCHEMA_REVISION: i64 = 20;

const SCHEMA: &str = "
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
  hash_attempt_failed INTEGER NOT NULL DEFAULT 0,
  metadata_attempt_failed INTEGER NOT NULL DEFAULT 0,
  missing          INTEGER NOT NULL DEFAULT 0,
  companion_of     INTEGER REFERENCES paths(id),
  resolved_utc_ms  INTEGER,
  resolved_source  TEXT,
  date_only        INTEGER NOT NULL DEFAULT 0,
  visibility_flags INTEGER NOT NULL DEFAULT 0,
  own_visibility_flags INTEGER NOT NULL DEFAULT 0,
  visibility_checked INTEGER NOT NULL DEFAULT 1,
  review_visible   INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_paths_content_hash ON paths (content_hash);
CREATE INDEX IF NOT EXISTS idx_paths_dir ON paths (dir_path);
CREATE INDEX IF NOT EXISTS idx_paths_pairing ON paths (dir_path, stem);
CREATE INDEX IF NOT EXISTS idx_paths_resolved ON paths (kind, resolved_utc_ms);
CREATE INDEX IF NOT EXISTS idx_paths_companion ON paths (companion_of);
CREATE INDEX IF NOT EXISTS idx_paths_media_repair_by_id ON paths (missing, id);
CREATE INDEX IF NOT EXISTS idx_paths_visibility_pending ON paths (id)
  WHERE missing = 0 AND visibility_checked = 0;
CREATE INDEX IF NOT EXISTS idx_paths_unhashed_other_section
  ON paths (resolved_utc_ms, id)
  WHERE missing = 0 AND companion_of IS NULL AND content_hash IS NULL
    AND kind NOT IN ('image', 'video') AND review_visible = 1;

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
  visible_copy_count     INTEGER NOT NULL DEFAULT 1,
  CHECK (
    (date_state = 'dated' AND resolved_utc_ms IS NOT NULL) OR
    (date_state IN ('pending', 'undated') AND resolved_utc_ms IS NULL)
  )
);
CREATE INDEX IF NOT EXISTS idx_logical_contents_section
  ON logical_contents (kind, resolved_utc_ms, content_hash) WHERE visible_copy_count > 0;
CREATE INDEX IF NOT EXISTS idx_logical_contents_work
  ON logical_contents (kind, content_hash) WHERE visible_copy_count > 0;
CREATE VIEW IF NOT EXISTS review_contents AS
  SELECT * FROM logical_contents WHERE visible_copy_count > 0;

-- A path batch suppresses the row trigger while one transaction changes many
-- physical copies, then republishes each affected logical item once from the
-- canonical projection below. SQLite serializes writers, and the guard lives
-- in the same transaction as the path writes, so no other writer can observe
-- or join a half-published batch and rollback can never strand the guard.
CREATE TABLE IF NOT EXISTS logical_projection_batch (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1)
);

-- The library choices this index was built with, a stamp of `config.json`,
-- which alone owns them (`library_settings`): the visibility triggers read
-- the hidden flags here, and the resolution columns stay NULL until launch
-- adopts the saved settings. The ignored names below are refilled from the
-- settings whenever they differ.
CREATE TABLE IF NOT EXISTS library_choices (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  hidden_flags INTEGER NOT NULL,
  default_timezone TEXT,
  good_range_start_year INTEGER,
  pairing_enabled INTEGER
);
INSERT OR IGNORE INTO library_choices (singleton, hidden_flags) VALUES (1, 0);
CREATE TABLE IF NOT EXISTS visibility_ignored_names (
  name TEXT PRIMARY KEY COLLATE onecopy_nocase
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS visibility_directories (
  abs_path TEXT PRIMARY KEY,
  parent_path TEXT,
  own_flags INTEGER NOT NULL,
  flags INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_visibility_directories_parent
  ON visibility_directories (parent_path);

CREATE TRIGGER IF NOT EXISTS paths_visibility_after_insert
AFTER INSERT ON paths
BEGIN
  UPDATE paths SET review_visible =
    (visibility_flags & (SELECT hidden_flags FROM library_choices)) = 0
    AND NOT EXISTS (SELECT 1 FROM visibility_ignored_names WHERE name = paths.file_name)
  WHERE id = NEW.id;
END;
CREATE TRIGGER IF NOT EXISTS paths_visibility_after_update
AFTER UPDATE OF visibility_flags, file_name ON paths
BEGIN
  UPDATE paths SET review_visible =
    (visibility_flags & (SELECT hidden_flags FROM library_choices)) = 0
    AND NOT EXISTS (SELECT 1 FROM visibility_ignored_names WHERE name = paths.file_name)
  WHERE id = NEW.id;
END;

-- A section's kind follows the representative copy's own kind, not
-- `contents.kind` (fixed by whichever copy was hashed first): byte-identical
-- copies with different extensions must not have their section depend on
-- indexing order (R4.1 finding 3).
CREATE VIEW IF NOT EXISTS logical_content_projection AS
SELECT c.hash AS content_hash,
       CASE WHEN (SELECT ranked.kind FROM paths ranked
                  WHERE ranked.content_hash = c.hash
                    AND ranked.missing = 0 AND ranked.companion_of IS NULL
                  ORDER BY ranked.review_visible DESC, ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                           ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
                  LIMIT 1) IN ('image', 'video')
         THEN (SELECT ranked.kind FROM paths ranked
               WHERE ranked.content_hash = c.hash
                 AND ranked.missing = 0 AND ranked.companion_of IS NULL
               ORDER BY ranked.review_visible DESC, ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                        ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
               LIMIT 1)
         ELSE 'other'
       END AS kind,
       CASE
         WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) > 0
           THEN 'pending'
         WHEN MIN(p.resolved_utc_ms) IS NULL THEN 'undated'
         ELSE 'dated'
       END AS date_state,
       CASE
         WHEN SUM(CASE WHEN p.resolved_source IS NULL THEN 1 ELSE 0 END) = 0
           THEN MIN(p.resolved_utc_ms)
         ELSE NULL
       END AS resolved_utc_ms,
       (SELECT ranked.id FROM paths ranked
        WHERE ranked.content_hash = c.hash
          AND ranked.missing = 0 AND ranked.companion_of IS NULL
        ORDER BY ranked.review_visible DESC, ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                 ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
        LIMIT 1) AS representative_path_id,
       COUNT(*) AS live_copy_count,
       SUM(p.review_visible) AS visible_copy_count
FROM contents c JOIN paths p ON p.content_hash = c.hash
WHERE p.missing = 0 AND p.companion_of IS NULL
GROUP BY c.hash;

-- Similarity is published as complete UTC-month cohorts. Dirty buckets are
-- reconstructible invalidation facts, not jobs: a revision changes whenever
-- source facts affecting one cohort change, so stale computation can never
-- clear or replace a newer request.
CREATE TABLE IF NOT EXISTS similarity_dirty_buckets (
  bucket   TEXT PRIMARY KEY,
  revision INTEGER NOT NULL
);

-- The configuration fingerprint is the derivation version for similarity.
-- Keeping it beside the output makes a settings change survive restart and
-- invalidate every existing cohort exactly once.
CREATE TABLE IF NOT EXISTS similarity_state (
  singleton          INTEGER PRIMARY KEY CHECK (singleton = 1),
  config_fingerprint TEXT NOT NULL
);

-- Only the current trigger definitions may maintain the projection.
DROP TRIGGER IF EXISTS paths_logical_after_insert;
DROP TRIGGER IF EXISTS paths_logical_after_update;
DROP TRIGGER IF EXISTS paths_logical_after_delete;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_insert_v2
AFTER INSERT ON paths
WHEN NEW.content_hash IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM logical_projection_batch)
BEGIN
  DELETE FROM logical_contents WHERE content_hash = NEW.content_hash;

  INSERT INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count, visible_copy_count)
  SELECT content_hash, kind, date_state, resolved_utc_ms,
         representative_path_id, live_copy_count, visible_copy_count
  FROM logical_content_projection WHERE content_hash = NEW.content_hash;
END;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_update_v2
AFTER UPDATE OF content_hash, resolved_utc_ms, resolved_source, missing,
                companion_of, file_name, review_visible ON paths
WHEN NOT EXISTS (SELECT 1 FROM logical_projection_batch)
BEGIN
  DELETE FROM logical_contents
  WHERE content_hash IN (OLD.content_hash, NEW.content_hash);

  INSERT INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count, visible_copy_count)
  SELECT content_hash, kind, date_state, resolved_utc_ms,
         representative_path_id, live_copy_count, visible_copy_count
  FROM logical_content_projection
  WHERE content_hash IN (OLD.content_hash, NEW.content_hash);
END;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_delete_v2
AFTER DELETE ON paths
WHEN OLD.content_hash IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM logical_projection_batch)
BEGIN
  DELETE FROM logical_contents WHERE content_hash = OLD.content_hash;

  INSERT INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count, visible_copy_count)
  SELECT content_hash, kind, date_state, resolved_utc_ms,
         representative_path_id, live_copy_count, visible_copy_count
  FROM logical_content_projection WHERE content_hash = OLD.content_hash;
END;

CREATE TRIGGER IF NOT EXISTS logical_similarity_after_insert
AFTER INSERT ON logical_contents
WHEN NEW.kind = 'image'
BEGIN
  INSERT INTO similarity_dirty_buckets (bucket, revision)
  VALUES (
    COALESCE(strftime('%Y-%m', NEW.resolved_utc_ms / 1000.0, 'unixepoch'), 'undated'),
    1
  )
  ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1;
END;

CREATE TRIGGER IF NOT EXISTS logical_similarity_after_update
AFTER UPDATE OF kind, resolved_utc_ms ON logical_contents
BEGIN
  INSERT INTO similarity_dirty_buckets (bucket, revision)
  SELECT COALESCE(strftime('%Y-%m', OLD.resolved_utc_ms / 1000.0, 'unixepoch'), 'undated'),
         1
  WHERE OLD.kind = 'image'
  ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1;

  INSERT INTO similarity_dirty_buckets (bucket, revision)
  SELECT COALESCE(strftime('%Y-%m', NEW.resolved_utc_ms / 1000.0, 'unixepoch'), 'undated'),
         1
  WHERE NEW.kind = 'image'
  ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1;
END;

CREATE TRIGGER IF NOT EXISTS logical_similarity_after_delete
AFTER DELETE ON logical_contents
WHEN OLD.kind = 'image'
BEGIN
  INSERT INTO similarity_dirty_buckets (bucket, revision)
  VALUES (
    COALESCE(strftime('%Y-%m', OLD.resolved_utc_ms / 1000.0, 'unixepoch'), 'undated'),
    1
  )
  ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1;
END;

CREATE TRIGGER IF NOT EXISTS contents_similarity_after_update
AFTER UPDATE OF phash, camera_make, camera_model ON contents
BEGIN
  INSERT INTO similarity_dirty_buckets (bucket, revision)
  SELECT COALESCE(strftime('%Y-%m', l.resolved_utc_ms / 1000.0, 'unixepoch'), 'undated'),
         1
  FROM logical_contents l
  WHERE l.content_hash = NEW.hash AND l.kind = 'image'
  ON CONFLICT(bucket) DO UPDATE SET revision = revision + 1;
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
  bucket         TEXT NOT NULL,
  created_at_utc TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_similar_groups_bucket ON similar_groups (bucket);

CREATE TABLE IF NOT EXISTS similar_group_members (
  group_id     INTEGER NOT NULL REFERENCES similar_groups(id),
  content_hash TEXT NOT NULL REFERENCES contents(hash),
  PRIMARY KEY (group_id, content_hash)
);
CREATE INDEX IF NOT EXISTS idx_similar_members_content
  ON similar_group_members (content_hash);

-- Expensive results are kept by content hash with no reference to `contents`:
-- a rebuild clears `contents` and keeps them (`clear_reconstructible`), and
-- they go with their content only when it leaves the library.
CREATE TABLE IF NOT EXISTS transcripts (
  content_hash   TEXT PRIMARY KEY,
  model          TEXT NOT NULL,
  model_version  TEXT NOT NULL,
  language       TEXT,
  text           TEXT NOT NULL,
  -- JSON array of {startMs, endMs, text}.
  segments       TEXT NOT NULL,
  created_at_utc TEXT NOT NULL
);
-- Which face model checked a content and how many faces it found, so a
-- content with no faces differs from one never checked.
CREATE TABLE IF NOT EXISTS face_checks (
  content_hash   TEXT PRIMARY KEY,
  model          TEXT NOT NULL,
  model_version  TEXT NOT NULL,
  face_count     INTEGER NOT NULL,
  checked_at_utc TEXT NOT NULL
);
-- One row per face found: its relative corner box, the detector's
-- confidence, and the expression model's eight probabilities (NULL when the
-- face could not be read).
CREATE TABLE IF NOT EXISTS faces (
  id            INTEGER PRIMARY KEY,
  content_hash  TEXT NOT NULL,
  x1            REAL NOT NULL,
  y1            REAL NOT NULL,
  x2            REAL NOT NULL,
  y2            REAL NOT NULL,
  confidence    REAL NOT NULL,
  anger         REAL,
  contempt      REAL,
  disgust       REAL,
  fear          REAL,
  happiness     REAL,
  neutral       REAL,
  sadness       REAL,
  surprise      REAL,
  model         TEXT NOT NULL,
  model_version TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_faces_content ON faces (content_hash);
-- The face score (face.rs): 0 for a checked content with no faces, else the
-- best face's confidence weighted by how much it smiles.
CREATE VIEW IF NOT EXISTS face_scores AS
  SELECT k.content_hash,
         COALESCE((SELECT MAX(f.confidence * (0.5 + 0.5 * COALESCE(f.happiness, 0.0)))
                   FROM faces f WHERE f.content_hash = k.content_hash), 0.0) AS score
  FROM face_checks k;
CREATE TABLE IF NOT EXISTS rebuild_keeps_results (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1)
);
CREATE TRIGGER IF NOT EXISTS contents_results_after_delete
AFTER DELETE ON contents
WHEN NOT EXISTS (SELECT 1 FROM rebuild_keeps_results)
BEGIN
  DELETE FROM transcripts WHERE content_hash = OLD.hash;
  DELETE FROM faces WHERE content_hash = OLD.hash;
  DELETE FROM face_checks WHERE content_hash = OLD.hash;
END;

CREATE TABLE IF NOT EXISTS scan_dirs (
  id                    INTEGER PRIMARY KEY,
  root                  TEXT NOT NULL UNIQUE,
  last_completed_at_utc TEXT,
  dirty                 INTEGER NOT NULL DEFAULT 0,
  relationship_dirty    INTEGER NOT NULL DEFAULT 0,
  -- The configured spelling whose walk last settled to this root, so an
  -- unavailable root that resolves under another spelling is still known.
  configured_root       TEXT
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
        // Allocation-free case-insensitive compare: char::to_lowercase yields a
        // small stack iterator per character, so this never heap-allocates a
        // lowercased copy of either string (unlike `str::to_lowercase`).
        left.chars()
            .flat_map(char::to_lowercase)
            .cmp(right.chars().flat_map(char::to_lowercase))
    })
    .map_err(|error| error.to_string())?;
    static JOURNAL: crate::sqlite::JournalSetup = crate::sqlite::JournalSetup::new();
    JOURNAL.configure(&conn, std::time::Duration::from_secs(5))?;
    let schema_revision = conn
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|error| format!("read index schema revision: {error}"))?;
    if schema_revision != SCHEMA_REVISION {
        conn.execute_batch("PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE")
            .map_err(|error| format!("claim index upgrade: {error}"))?;
        let setup = (|| {
            // Another opener may have completed the upgrade while this one
            // waited for SQLite's write lock. Its observed version owns DDL.
            let current = conn
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .map_err(|error| error.to_string())?;
            match current {
                SCHEMA_REVISION => return Ok(()),
                current if current > SCHEMA_REVISION => {
                    return Err(format!("unsupported index schema revision: {current}"))
                }
                _ => {}
            }
            // An earlier revision is not upgraded: the index is rebuilt from
            // the files, starting empty.
            let objects = conn
                .prepare(
                    "SELECT type, name FROM sqlite_master
                     WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(|error| error.to_string())?;
            for (kind, name) in objects {
                let kind = if kind == "view" { "VIEW" } else { "TABLE" };
                conn.execute_batch(&format!("DROP {kind} IF EXISTS \"{}\"", name.replace('"', "\"\"")))
                    .map_err(|error| error.to_string())?;
            }
            conn.execute_batch(SCHEMA).map_err(|error| error.to_string())?;
            crate::visibility_index::apply_policy_in_transaction(
                &conn,
                &crate::visibility::Policy::from_config(&serde_json::json!({}))?,
            )?;
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
    crate::records::attach(
        &conn,
        &db_file.with_file_name(crate::records::RECORDS_DB_FILE_NAME),
    )?;
    // Enforced on every open, not only the connection that ran the upgrade:
    // a dangling `evidence` or `companion_of` row after a same-transaction
    // sibling delete (see delete_targets and forget_unconfigured_roots) must
    // fail the transaction rather than survive silently (R1-11).
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(|error| error.to_string())?;
    Ok(conn)
}

// EXCEPTION (tests-folder conventions): the schema-shape test stays in-file
// because it asserts the private SCHEMA constant's effect on a fresh file,
/// Whether this launch's Issue for (kind, path) is open: its latest event is
/// an occurrence.
fn issue_open(conn: &Connection, kind: &str, path: &str) -> Result<bool, String> {
    let latest: Option<String> = conn
        .query_row(
            &format!(
                "SELECT event FROM records.issue_events
                 WHERE session_id IS {} AND kind = ?1 AND path = ?2 ORDER BY id DESC LIMIT 1",
                crate::records::session_sql()
            ),
            rusqlite::params![kind, path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(latest.as_deref() == Some("occurred"))
}

fn record_issue_event(conn: &Connection, kind: &str, path: &str, event: &str) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO records.issue_events (session_id, time_utc, kind, path, event)
             VALUES ({}, ?1, ?2, ?3, ?4)",
            crate::records::session_sql()
        ),
        rusqlite::params![crate::logging::now_iso_millis(), kind, path, event],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Records one occurrence of a condition, identified by (kind, path). `path`
/// None anchors to '' so rootless conditions also coalesce. Returns whether
/// this call opened a new Issue — a repeated failure of an open one only adds
/// an occurrence, which callers must not treat as an Issues-surface change
/// (C-M3).
pub fn upsert_issue(
    conn: &Connection,
    path: Option<&str>,
    kind: &str,
    message: &str,
) -> Result<bool, String> {
    upsert_issue_with_descriptor(conn, path, kind, None, None, message)
}

/// Like `upsert_issue`, plus a message descriptor (a catalogue key the
/// frontend renders in the current interface language, with its
/// interpolation values as a JSON object) beside the recorded `message`.
/// `message` stays exactly what a caller with no descriptor already passed:
/// real, non-restatable detail (a system error, a count, a path), shown as
/// recorded after the translated sentence (R5.5 D-L12).
pub fn upsert_issue_with_descriptor(
    conn: &Connection,
    path: Option<&str>,
    kind: &str,
    message_key: Option<&str>,
    message_values_json: Option<&str>,
    message: &str,
) -> Result<bool, String> {
    let path = path.unwrap_or("");
    let opened = !issue_open(conn, kind, path)?;
    conn.execute(
        &format!(
            "INSERT INTO records.issue_events
               (session_id, time_utc, kind, path, event, message, message_key, message_values)
             VALUES ({}, ?1, ?2, ?3, 'occurred', ?4, ?5, ?6)",
            crate::records::session_sql()
        ),
        rusqlite::params![
            crate::logging::now_iso_millis(),
            kind,
            path,
            message,
            message_key,
            message_values_json
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(opened)
}

/// Resolves open conditions. Returns whether any Issue was actually
/// resolved, so callers can tell a real resolution from a no-op clear of an
/// already-closed or absent issue (C-M3).
pub fn clear_issues(conn: &Connection, path: &str, kinds: &[&str]) -> Result<bool, String> {
    let mut changed = false;
    for kind in kinds {
        if issue_open(conn, kind, path)? {
            record_issue_event(conn, kind, path, "resolved")?;
            changed = true;
        }
    }
    Ok(changed)
}

/// Closes every open Issue `predicate` (SQL over `kind` and `path`, with
/// `params`) selects, recording how it closed.
pub(crate) fn close_issues(
    conn: &Connection,
    closure: &str,
    predicate: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<(), String> {
    conn.execute(
        &format!(
            "INSERT INTO records.issue_events (session_id, time_utc, kind, path, event)
             SELECT {}, {}, kind, path, '{closure}' FROM active_issues WHERE {predicate}",
            crate::records::session_sql(),
            crate::records::sql_text(Some(&crate::logging::now_iso_millis())),
        ),
        params,
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Whether any Issue is open; closed records never cause cleanup work.
pub fn any_issues(conn: &Connection) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM active_issues)", [], |r| r.get(0))
        .map_err(|error| error.to_string())
}

/// Dismiss one open Issue or the complete inbox, not just a loaded page.
pub fn dismiss_issues(conn: &Connection, id: Option<i64>) -> Result<(), String> {
    close_issues(conn, "dismissed", "(?1 IS NULL OR id = ?1)", &[&id])
}

/// The only way to write more than one `paths` row's projection-affecting
/// columns in one statement. Suppresses the per-row logical-projection
/// triggers (`paths_logical_after_*_v2`) for the duration of `write`, then
/// republishes every hash `collect_hashes` recorded exactly once, all inside
/// one IMMEDIATE transaction. [`publish_paths_batch_in`] is the same
/// publication inside a transaction the caller already holds.
///
/// `collect_hashes` runs first, against the transaction, and inserts every
/// `content_hash` the coming write can affect into the temp table
/// `batch_touched_hashes`; it is free to insert nothing when `write` is a
/// full wipe with nothing left to republish (see `clear_reconstructible`).
/// `write` then performs the bulk UPDATE/DELETE.
pub fn publish_paths_batch(
    conn: &Connection,
    collect_hashes: impl FnOnce(&Connection) -> Result<(), String>,
    write: impl FnOnce(&Connection) -> Result<(), String>,
) -> Result<(), String> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    publish_paths_batch_in(&tx, collect_hashes, write)?;
    tx.commit().map_err(|error| error.to_string())
}

/// [`publish_paths_batch`] inside a write transaction the caller already
/// holds (the visibility apply).
pub(crate) fn publish_paths_batch_in(
    conn: &Connection,
    collect_hashes: impl FnOnce(&Connection) -> Result<(), String>,
    write: impl FnOnce(&Connection) -> Result<(), String>,
) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS batch_touched_hashes (content_hash TEXT PRIMARY KEY) WITHOUT ROWID;
         DELETE FROM batch_touched_hashes;",
    )
    .map_err(|error| error.to_string())?;
    collect_hashes(conn)?;
    // Drop the stale projection rows before `write` runs, not after: a
    // touched hash's `logical_contents.representative_path_id` can point at
    // a `paths` row `write` is about to delete, and that FK only tolerates
    // the delete once nothing still references it.
    conn.execute(
        "DELETE FROM logical_contents WHERE content_hash IN (SELECT content_hash FROM batch_touched_hashes)",
        [],
    )
    .map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO logical_projection_batch (singleton) VALUES (1)",
        [],
    )
    .map_err(|error| error.to_string())?;
    write(conn)?;
    conn.execute_batch(
        "INSERT INTO logical_contents
           (content_hash, kind, date_state, resolved_utc_ms, representative_path_id, live_copy_count, visible_copy_count)
           SELECT content_hash, kind, date_state, resolved_utc_ms, representative_path_id, live_copy_count, visible_copy_count
           FROM logical_content_projection WHERE content_hash IN (SELECT content_hash FROM batch_touched_hashes);
         DELETE FROM logical_projection_batch;
         DELETE FROM batch_touched_hashes;",
    )
    .map_err(|error| error.to_string())
}

/// Clears only reconstructible library facts and closes the open Issues;
/// transcripts and face results stay unless their discard flag is set. Durable configuration, managed tools, and retained
/// authored records live in separate stores and are deliberately outside this
/// transaction. Everything projection-affecting is
/// wiped in the same pass, so the batch guard only needs to suppress the
/// per-row triggers while it runs; there is nothing left to republish.
pub fn clear_reconstructible(
    conn: &Connection,
    discard_transcripts: bool,
    discard_faces: bool,
) -> Result<(), String> {
    publish_paths_batch(
        conn,
        |_tx| Ok(()),
        |tx| {
            if discard_transcripts {
                tx.execute("DELETE FROM transcripts", []).map_err(|error| error.to_string())?;
            }
            if discard_faces {
                tx.execute_batch("DELETE FROM faces; DELETE FROM face_checks;")
                    .map_err(|error| error.to_string())?;
            }
            crate::derived_state::reopen_session_analysis_failures(tx)?;
            close_issues(tx, "rebuilt", "1", &[])?;
            tx.execute_batch(
                "INSERT INTO rebuild_keeps_results (singleton) VALUES (1);
             DELETE FROM similar_group_members;
             DELETE FROM similar_groups;
             DELETE FROM evidence;
             DELETE FROM logical_contents;
             DELETE FROM paths;
             DELETE FROM visibility_directories;
             DELETE FROM contents;
             DELETE FROM similarity_dirty_buckets;
             DELETE FROM similarity_state;
             DELETE FROM scan_dirs;
             DELETE FROM rebuild_keeps_results;",
            )
            .map_err(|error| error.to_string())
        },
    )
}

// which has no public seam. The copy-count semantics it used to sit beside
// moved to tests/queries_tests.rs, where they are asserted through the real
// query instead of a SELECT the test wrote itself.
#[cfg(test)]
// EXCEPTION to tests-folder conventions: reads the private
// `SCHEMA_REVISION`; promoting it would widen the crate's API only for this
// test.
#[path = "../tests/unit/index_store.rs"]
mod tests;
