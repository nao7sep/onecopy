-- Frozen schema from revision 12; migration tests must start with the shipped shape.

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
  hash_attempt_failed INTEGER NOT NULL DEFAULT 0,
  metadata_attempt_failed INTEGER NOT NULL DEFAULT 0,
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

-- A path batch suppresses the row trigger while one transaction changes many
-- physical copies, then republishes each affected logical item once from the
-- canonical projection below. SQLite serializes writers, and the guard lives
-- in the same transaction as the path writes, so no other writer can observe
-- or join a half-published batch and rollback can never strand the guard.
CREATE TABLE IF NOT EXISTS logical_projection_batch (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1)
);

CREATE VIEW IF NOT EXISTS logical_content_projection AS
SELECT c.hash AS content_hash,
       CASE WHEN c.kind IN ('image', 'video') THEN c.kind ELSE 'other' END AS kind,
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
        ORDER BY ranked.resolved_utc_ms IS NULL, ranked.resolved_utc_ms,
                 ranked.abs_path COLLATE onecopy_nocase, ranked.abs_path
        LIMIT 1) AS representative_path_id,
       COUNT(*) AS live_copy_count
FROM contents c JOIN paths p ON p.content_hash = c.hash
WHERE p.missing = 0 AND p.companion_of IS NULL
GROUP BY c.hash, c.kind;

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
     live_copy_count)
  SELECT content_hash, kind, date_state, resolved_utc_ms,
         representative_path_id, live_copy_count
  FROM logical_content_projection WHERE content_hash = NEW.content_hash;
END;

CREATE TRIGGER IF NOT EXISTS paths_logical_after_update_v2
AFTER UPDATE OF content_hash, resolved_utc_ms, resolved_source, missing,
                companion_of, file_name ON paths
WHEN NOT EXISTS (SELECT 1 FROM logical_projection_batch)
BEGIN
  DELETE FROM logical_contents
  WHERE content_hash IN (OLD.content_hash, NEW.content_hash);

  INSERT INTO logical_contents
    (content_hash, kind, date_state, resolved_utc_ms, representative_path_id,
     live_copy_count)
  SELECT content_hash, kind, date_state, resolved_utc_ms,
         representative_path_id, live_copy_count
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
     live_copy_count)
  SELECT content_hash, kind, date_state, resolved_utc_ms,
         representative_path_id, live_copy_count
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

CREATE TABLE IF NOT EXISTS issues (
  id             INTEGER PRIMARY KEY,
  path           TEXT NOT NULL DEFAULT '',
  kind           TEXT NOT NULL,
  message        TEXT,
  first_seen_utc TEXT NOT NULL,
  last_seen_utc  TEXT NOT NULL,
  occurrence_count INTEGER NOT NULL DEFAULT 1,
  closed_at_utc  TEXT,
  closure        TEXT CHECK (closure IN ('dismissed', 'resolved', 'app-restart', 'rechecked')),
  CHECK ((closed_at_utc IS NULL) = (closure IS NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_issues_live_identity
  ON issues (kind, path) WHERE closed_at_utc IS NULL;
CREATE INDEX IF NOT EXISTS idx_issues_first_seen
  ON issues (first_seen_utc, id) WHERE closed_at_utc IS NULL;
CREATE VIEW IF NOT EXISTS active_issues AS
  SELECT id, path, kind, message, first_seen_utc, last_seen_utc, occurrence_count
  FROM issues WHERE closed_at_utc IS NULL;

PRAGMA user_version = 12;
