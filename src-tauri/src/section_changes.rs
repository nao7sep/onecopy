//! Which Main sections an owner's own index writes touched, so the change
//! events it already publishes can name them and Main can skip re-reading a
//! section whose items did not change.
//!
//! TEMP triggers on the owner's connection record section facts as rows
//! change: a logical item's kind and display instant on both sides of each
//! projection write (so an item whose date moves names its old and its new
//! month), an unhashed other-file's facts on both sides of a path write, the
//! item a changed path, content fact, or companion belongs to, and each image
//! similarity cohort whose pending state changed. The triggers see only that
//! connection's writes and roll back with its transactions. Months are
//! bucketed in Rust under the display timezone, exactly as `queries` buckets
//! them for Main.

use std::cell::RefCell;
use std::collections::HashSet;

use chrono::TimeZone;
use chrono_tz::Tz;
use rusqlite::Connection;

use crate::queries::SectionLocation;

const TRACKING_SQL: &str = "
-- No uniqueness constraints: a trigger's own conflict clause gives way to
-- the conflict clause of the statement that fired it (an upsert's, for
-- example), so a duplicate here would fail the owner's write. Duplicates
-- are folded when the rows are drained.
CREATE TEMP TABLE IF NOT EXISTS section_change_facts (
  kind    TEXT NOT NULL,
  undated INTEGER NOT NULL,
  at      INTEGER NOT NULL
);
CREATE TEMP TABLE IF NOT EXISTS section_change_buckets (
  bucket TEXT NOT NULL
);

CREATE TEMP TRIGGER IF NOT EXISTS section_change_logical_insert
AFTER INSERT ON logical_contents WHEN NEW.visible_copy_count > 0
BEGIN
  INSERT INTO section_change_facts
  VALUES (NEW.kind, NEW.resolved_utc_ms IS NULL, IFNULL(NEW.resolved_utc_ms, 0));
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_logical_update
AFTER UPDATE ON logical_contents
BEGIN
  INSERT INTO section_change_facts
  SELECT OLD.kind, OLD.resolved_utc_ms IS NULL, IFNULL(OLD.resolved_utc_ms, 0)
  WHERE OLD.visible_copy_count > 0;
  INSERT INTO section_change_facts
  SELECT NEW.kind, NEW.resolved_utc_ms IS NULL, IFNULL(NEW.resolved_utc_ms, 0)
  WHERE NEW.visible_copy_count > 0;
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_logical_delete
AFTER DELETE ON logical_contents WHEN OLD.visible_copy_count > 0
BEGIN
  INSERT INTO section_change_facts
  VALUES (OLD.kind, OLD.resolved_utc_ms IS NULL, IFNULL(OLD.resolved_utc_ms, 0));
END;

-- An unhashed other-file is its own item; its row is its section fact.
CREATE TEMP TRIGGER IF NOT EXISTS section_change_other_insert
AFTER INSERT ON paths
WHEN NEW.missing = 0 AND NEW.review_visible = 1 AND NEW.companion_of IS NULL
 AND NEW.content_hash IS NULL AND NEW.kind NOT IN ('image', 'video')
BEGIN
  INSERT INTO section_change_facts
  VALUES ('other', NEW.resolved_utc_ms IS NULL, IFNULL(NEW.resolved_utc_ms, 0));
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_other_update
AFTER UPDATE OF missing, review_visible, companion_of, content_hash, kind, resolved_utc_ms,
  file_name, dir_path, size ON paths
WHEN (OLD.missing = 0 AND OLD.review_visible = 1 AND OLD.companion_of IS NULL
      AND OLD.content_hash IS NULL AND OLD.kind NOT IN ('image', 'video'))
  OR (NEW.missing = 0 AND NEW.review_visible = 1 AND NEW.companion_of IS NULL
      AND NEW.content_hash IS NULL AND NEW.kind NOT IN ('image', 'video'))
BEGIN
  INSERT INTO section_change_facts
  SELECT 'other', OLD.resolved_utc_ms IS NULL, IFNULL(OLD.resolved_utc_ms, 0)
  WHERE OLD.missing = 0 AND OLD.review_visible = 1 AND OLD.companion_of IS NULL
    AND OLD.content_hash IS NULL AND OLD.kind NOT IN ('image', 'video');
  INSERT INTO section_change_facts
  SELECT 'other', NEW.resolved_utc_ms IS NULL, IFNULL(NEW.resolved_utc_ms, 0)
  WHERE NEW.missing = 0 AND NEW.review_visible = 1 AND NEW.companion_of IS NULL
    AND NEW.content_hash IS NULL AND NEW.kind NOT IN ('image', 'video');
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_other_delete
AFTER DELETE ON paths
WHEN OLD.missing = 0 AND OLD.review_visible = 1 AND OLD.companion_of IS NULL
 AND OLD.content_hash IS NULL AND OLD.kind NOT IN ('image', 'video')
BEGIN
  INSERT INTO section_change_facts
  VALUES ('other', OLD.resolved_utc_ms IS NULL, IFNULL(OLD.resolved_utc_ms, 0));
END;

-- A copy's own facts (its folder, its name, whether it counts) reach the
-- item without always republishing the projection, so a write to one of
-- them on a hashed copy names the item. Triggers list the columns that
-- change what Main shows: a statement that sets only others (a prehash, an
-- attempt flag, an indexing time) neither runs nor compiles them.
CREATE TEMP TRIGGER IF NOT EXISTS section_change_copy_update
AFTER UPDATE OF content_hash, missing, review_visible, dir_path, file_name, resolved_utc_ms ON paths
WHEN OLD.content_hash IS NOT NULL OR NEW.content_hash IS NOT NULL
BEGIN
  INSERT INTO section_change_facts
  SELECT kind, resolved_utc_ms IS NULL, IFNULL(resolved_utc_ms, 0)
  FROM logical_contents
  WHERE content_hash IN (OLD.content_hash, NEW.content_hash) AND visible_copy_count > 0;
END;

-- A companion changes its primary's item (its companion flag), which no
-- projection write of the primary records.
CREATE TEMP TRIGGER IF NOT EXISTS section_change_companion_insert
AFTER INSERT ON paths WHEN NEW.companion_of IS NOT NULL
BEGIN
  INSERT INTO section_change_facts
  SELECT l.kind, l.resolved_utc_ms IS NULL, IFNULL(l.resolved_utc_ms, 0)
  FROM paths p JOIN logical_contents l ON l.content_hash = p.content_hash
  WHERE p.id = NEW.companion_of AND l.visible_copy_count > 0
  UNION ALL
  SELECT 'other', p.resolved_utc_ms IS NULL, IFNULL(p.resolved_utc_ms, 0)
  FROM paths p
  WHERE p.id = NEW.companion_of AND p.missing = 0 AND p.review_visible = 1
    AND p.companion_of IS NULL AND p.content_hash IS NULL AND p.kind NOT IN ('image', 'video');
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_companion_update
AFTER UPDATE OF companion_of ON paths WHEN OLD.companion_of IS NOT NULL OR NEW.companion_of IS NOT NULL
BEGIN
  INSERT INTO section_change_facts
  SELECT l.kind, l.resolved_utc_ms IS NULL, IFNULL(l.resolved_utc_ms, 0)
  FROM paths p JOIN logical_contents l ON l.content_hash = p.content_hash
  WHERE p.id IN (OLD.companion_of, NEW.companion_of) AND l.visible_copy_count > 0
  UNION ALL
  SELECT 'other', p.resolved_utc_ms IS NULL, IFNULL(p.resolved_utc_ms, 0)
  FROM paths p
  WHERE p.id IN (OLD.companion_of, NEW.companion_of) AND p.missing = 0 AND p.review_visible = 1
    AND p.companion_of IS NULL AND p.content_hash IS NULL AND p.kind NOT IN ('image', 'video');
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_companion_delete
AFTER DELETE ON paths WHEN OLD.companion_of IS NOT NULL
BEGIN
  INSERT INTO section_change_facts
  SELECT l.kind, l.resolved_utc_ms IS NULL, IFNULL(l.resolved_utc_ms, 0)
  FROM paths p JOIN logical_contents l ON l.content_hash = p.content_hash
  WHERE p.id = OLD.companion_of AND l.visible_copy_count > 0
  UNION ALL
  SELECT 'other', p.resolved_utc_ms IS NULL, IFNULL(p.resolved_utc_ms, 0)
  FROM paths p
  WHERE p.id = OLD.companion_of AND p.missing = 0 AND p.review_visible = 1
    AND p.companion_of IS NULL AND p.content_hash IS NULL AND p.kind NOT IN ('image', 'video');
END;

-- Content facts (dimensions, duration, size) are item columns too.
CREATE TEMP TRIGGER IF NOT EXISTS section_change_content_update
AFTER UPDATE ON contents
BEGIN
  INSERT INTO section_change_facts
  SELECT kind, resolved_utc_ms IS NULL, IFNULL(resolved_utc_ms, 0)
  FROM logical_contents WHERE content_hash = NEW.hash AND visible_copy_count > 0;
END;

-- A cohort's pending similarity is shown on every image in it, including
-- images whose own rows this connection never wrote.
CREATE TEMP TRIGGER IF NOT EXISTS section_change_bucket_insert
AFTER INSERT ON similarity_dirty_buckets
BEGIN
  INSERT INTO section_change_buckets VALUES (NEW.bucket);
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_bucket_update
AFTER UPDATE ON similarity_dirty_buckets
BEGIN
  INSERT INTO section_change_buckets VALUES (NEW.bucket);
END;
CREATE TEMP TRIGGER IF NOT EXISTS section_change_bucket_delete
AFTER DELETE ON similarity_dirty_buckets
BEGIN
  INSERT INTO section_change_buckets VALUES (OLD.bucket);
END;
";

/// Starts recording the sections `conn`'s own writes touch. Idempotent.
pub fn track(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(TRACKING_SQL).map_err(|error| error.to_string())
}

/// The sections recorded on `conn` since the previous drain, cleared as they
/// are read.
pub fn drain(conn: &Connection, display_tz: Tz) -> Result<HashSet<SectionLocation>, String> {
    let mut sections = HashSet::new();
    let mut facts = conn
        .prepare("SELECT DISTINCT kind, undated, at FROM temp.section_change_facts")
        .map_err(|error| error.to_string())?;
    let rows = facts
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, i64>(2)?))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (kind, undated, at) = row.map_err(|error| error.to_string())?;
        sections.insert(crate::queries::section_from_facts(kind, (!undated).then_some(at), display_tz)?);
    }
    let mut buckets = conn
        .prepare("SELECT DISTINCT bucket FROM temp.section_change_buckets")
        .map_err(|error| error.to_string())?;
    let rows = buckets
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    for bucket in rows {
        sections.extend(image_sections_for_bucket(&bucket.map_err(|error| error.to_string())?, display_tz)?);
    }
    conn.execute_batch(
        "DELETE FROM temp.section_change_facts; DELETE FROM temp.section_change_buckets;",
    )
    .map_err(|error| error.to_string())?;
    Ok(sections)
}

/// The image sections that show a similarity cohort. Cohorts are UTC months
/// (`'YYYY-MM'`, or `'undated'`) and sections are display-timezone months, so
/// a cohort reaches the months its first and last instants fall in as well as
/// its own: no offset spans a whole month.
pub fn image_sections_for_bucket(bucket: &str, display_tz: Tz) -> Result<Vec<SectionLocation>, String> {
    let image = |month: String| SectionLocation { kind: "image".to_string(), month };
    if bucket == "undated" {
        return Ok(vec![image("undated".to_string())]);
    }
    let invalid = || format!("Similarity cohort {bucket} is not a month");
    let (year, month) = bucket.split_once('-').ok_or_else(invalid)?;
    let year: i32 = year.parse().map_err(|_| invalid())?;
    let month: u32 = month.parse().map_err(|_| invalid())?;
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let start = chrono::Utc
        .with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .ok_or_else(invalid)?
        .timestamp_millis();
    let end = chrono::Utc
        .with_ymd_and_hms(next_year, next_month, 1, 0, 0, 0)
        .single()
        .ok_or_else(invalid)?
        .timestamp_millis()
        - 1;
    let mut sections = vec![image(bucket.to_string())];
    for instant in [start, end] {
        let section = crate::queries::section_from_facts("image".to_string(), Some(instant), display_tz)?;
        if !sections.contains(&section) {
            sections.push(section);
        }
    }
    Ok(sections)
}

/// One owner run's section scope: what its index writes touched since the
/// last published progress, and over the whole run. Tracking that could not
/// start or be read, and a connection whose tracking never finished, leave the
/// run unscoped: its events then carry no sections and refresh Main
/// unconditionally, which is never stale.
pub struct SectionLog {
    timezone: Tz,
    state: RefCell<LogState>,
}

#[derive(Default)]
struct LogState {
    pending: HashSet<SectionLocation>,
    run: HashSet<SectionLocation>,
    open: usize,
    unscoped: bool,
}

impl Default for SectionLog {
    fn default() -> Self {
        Self::new(crate::queries::display_timezone())
    }
}

impl SectionLog {
    pub fn new(timezone: Tz) -> Self {
        Self { timezone, state: RefCell::new(LogState::default()) }
    }

    /// Starts recording `conn`'s writes for this run; pair with `finish`.
    pub fn track(&self, conn: &Connection) {
        let mut state = self.state.borrow_mut();
        state.open += 1;
        if let Err(error) = track(conn) {
            unscope(&mut state, &error);
        }
    }

    /// Sections touched since the previous call. Writes inside a transaction
    /// that is still open stay recorded for a later call, so a section is
    /// never named before Main can read its change.
    pub fn changed_since_last(&self, conn: &Connection) -> Option<Vec<SectionLocation>> {
        if conn.is_autocommit() {
            self.collect(conn);
        }
        let mut state = self.state.borrow_mut();
        let pending = std::mem::take(&mut state.pending);
        (!state.unscoped).then(|| sorted(pending))
    }

    /// Ends recording on `conn`, reading what it still holds.
    pub fn finish(&self, conn: &Connection) {
        if conn.is_autocommit() {
            self.collect(conn);
        } else {
            unscope(&mut self.state.borrow_mut(), "a transaction was still open");
        }
        let mut state = self.state.borrow_mut();
        state.open = state.open.saturating_sub(1);
    }

    /// Every section the run touched, or `None` when the run is unscoped.
    pub fn sections(&self) -> Option<Vec<SectionLocation>> {
        let state = self.state.borrow();
        (!state.unscoped && state.open == 0).then(|| sorted(state.run.clone()))
    }

    fn collect(&self, conn: &Connection) {
        if self.state.borrow().unscoped {
            return;
        }
        let drained = drain(conn, self.timezone);
        let mut state = self.state.borrow_mut();
        match drained {
            Ok(sections) => {
                state.run.extend(sections.iter().cloned());
                state.pending.extend(sections);
            }
            Err(error) => unscope(&mut state, &error),
        }
    }
}

fn unscope(state: &mut LogState, reason: &str) {
    if !state.unscoped {
        crate::logging::warn(
            "changed sections unavailable; Main refreshes unscoped",
            serde_json::json!({ "error": { "message": reason } }),
        );
    }
    state.unscoped = true;
    state.pending.clear();
    state.run.clear();
}

fn sorted(sections: HashSet<SectionLocation>) -> Vec<SectionLocation> {
    let mut sections: Vec<_> = sections.into_iter().collect();
    sections.sort_by(|left, right| (&left.kind, &left.month).cmp(&(&right.kind, &right.month)));
    sections
}
