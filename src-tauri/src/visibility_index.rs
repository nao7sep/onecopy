//! Indexed visibility facts and their review projection. This never changes
//! content identity, attempt receipts, or physical-copy membership.

use crate::visibility::{self, Policy};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn begin_projection_batch(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS visibility_changed (content_hash TEXT PRIMARY KEY);
        DELETE FROM visibility_changed; INSERT INTO logical_projection_batch VALUES (1);",
    )
    .map_err(|error| error.to_string())
}

fn finish_projection_batch(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("DELETE FROM logical_contents WHERE content_hash IN (SELECT content_hash FROM visibility_changed);
        INSERT INTO logical_contents
          (content_hash, kind, date_state, resolved_utc_ms, representative_path_id, live_copy_count, visible_copy_count)
          SELECT content_hash, kind, date_state, resolved_utc_ms, representative_path_id, live_copy_count, visible_copy_count
          FROM logical_content_projection WHERE content_hash IN (SELECT content_hash FROM visibility_changed);
        DELETE FROM logical_projection_batch; DELETE FROM visibility_changed;")
        .map_err(|error| error.to_string())
}

pub fn apply_policy(conn: &Connection, policy: &Policy) -> Result<(), String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    apply_policy_in_transaction(&tx, policy)?;
    tx.commit().map_err(|error| error.to_string())
}

pub(crate) fn apply_policy_in_transaction(
    conn: &Connection,
    policy: &Policy,
) -> Result<(), String> {
    let flags: i64 = conn
        .query_row("SELECT hidden_flags FROM visibility_policy", [], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())?;
    let mut names = conn
        .prepare("SELECT name FROM visibility_ignored_names")
        .map_err(|error| error.to_string())?;
    let current = names
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<std::collections::HashSet<_>, _>>()
        .map_err(|error| error.to_string())?;
    if flags == policy.hidden_flags && current == policy.ignored_file_names {
        return Ok(());
    }
    conn.execute(
        "UPDATE visibility_policy SET hidden_flags = ?1",
        [policy.hidden_flags],
    )
    .map_err(|error| error.to_string())?;
    conn.execute("DELETE FROM visibility_ignored_names", [])
        .map_err(|error| error.to_string())?;
    for name in &policy.ignored_file_names {
        conn.execute("INSERT INTO visibility_ignored_names VALUES (?1)", [name])
            .map_err(|error| error.to_string())?;
    }
    // One projection publication per affected identity, not per duplicate.
    begin_projection_batch(conn)?;
    conn.execute_batch(
        "INSERT OR IGNORE INTO visibility_changed
           SELECT content_hash FROM paths WHERE content_hash IS NOT NULL AND review_visible != (
             (visibility_flags & (SELECT hidden_flags FROM visibility_policy)) = 0
             AND NOT EXISTS (SELECT 1 FROM visibility_ignored_names WHERE name = paths.file_name));
         UPDATE paths SET review_visible =
           (visibility_flags & (SELECT hidden_flags FROM visibility_policy)) = 0
           AND NOT EXISTS (SELECT 1 FROM visibility_ignored_names WHERE name = paths.file_name)
         WHERE review_visible != (
           (visibility_flags & (SELECT hidden_flags FROM visibility_policy)) = 0
           AND NOT EXISTS (SELECT 1 FROM visibility_ignored_names WHERE name = paths.file_name));",
    )
    .map_err(|error| error.to_string())?;
    finish_projection_batch(conn)
}

/// Cache belongs to one discovery pass. Each directory is statted once, with
/// inheritance ending at the explicitly configured root, never its ancestors.
#[derive(Default)]
pub struct DirectoryFacts {
    flags: HashMap<PathBuf, i64>,
    pub changed_files: usize,
}

impl DirectoryFacts {
    pub fn refresh(&mut self, conn: &Connection, root: &Path, dir: &Path) -> Result<i64, String> {
        if let Some(flags) = self.flags.get(dir) {
            return Ok(*flags);
        }
        if !dir.starts_with(root) {
            return Err("Visibility directory is outside its source root".into());
        }
        let (parent, own, inherited) = if dir == root {
            (None, 0, 0)
        } else {
            let parent = dir.parent().ok_or("Visibility directory has no parent")?;
            let inherited = self.refresh(conn, root, parent)?;
            let metadata = std::fs::metadata(crate::winpath::for_fs(dir).as_ref())
                .map_err(|error| error.to_string())?;
            (
                Some(parent.to_string_lossy().into_owned()),
                visibility::entry_flags(dir, &metadata),
                inherited,
            )
        };
        self.record(conn, dir, parent.as_deref(), own, inherited)?;
        Ok(own | inherited)
    }

    fn record(
        &mut self,
        conn: &Connection,
        dir: &Path,
        parent: Option<&str>,
        own: i64,
        inherited: i64,
    ) -> Result<(), String> {
        let abs = dir.to_string_lossy();
        let flags = own | inherited;
        let previous = conn.query_row("SELECT parent_path, own_flags, flags FROM visibility_directories WHERE abs_path = ?1", [&abs],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?)))
            .optional().map_err(|error| error.to_string())?;
        if previous
            .as_ref()
            .is_none_or(|old| old != &(parent.map(str::to_string), own, flags))
        {
            let tx = conn
                .unchecked_transaction()
                .map_err(|error| error.to_string())?;
            tx.execute("INSERT INTO visibility_directories (abs_path, parent_path, own_flags, flags) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT(abs_path) DO UPDATE SET parent_path=excluded.parent_path, own_flags=excluded.own_flags, flags=excluded.flags",
                params![abs, parent, own, flags]).map_err(|error| error.to_string())?;
            // An attribute change on a folder also affects already indexed
            // descendants whose bytes and own attributes have not changed.
            tx.execute("WITH RECURSIVE subtree(abs_path, flags) AS (
                SELECT abs_path, flags FROM visibility_directories WHERE abs_path = ?1
                UNION ALL SELECT child.abs_path, child.own_flags | subtree.flags
                FROM visibility_directories child JOIN subtree ON child.parent_path = subtree.abs_path)
                UPDATE visibility_directories SET flags = (SELECT flags FROM subtree WHERE subtree.abs_path = visibility_directories.abs_path)
                WHERE abs_path IN (SELECT abs_path FROM subtree)", [&abs]).map_err(|error| error.to_string())?;
            begin_projection_batch(&tx)?;
            tx.execute("WITH RECURSIVE subtree(abs_path) AS (
                SELECT abs_path FROM visibility_directories WHERE abs_path = ?1
                UNION ALL SELECT child.abs_path FROM visibility_directories child JOIN subtree ON child.parent_path = subtree.abs_path)
                INSERT OR IGNORE INTO visibility_changed SELECT content_hash FROM paths
                WHERE content_hash IS NOT NULL AND dir_path IN (SELECT abs_path FROM subtree)
                  AND visibility_flags != (own_visibility_flags |
                    (SELECT flags FROM visibility_directories WHERE abs_path = paths.dir_path))", [&abs]).map_err(|error| error.to_string())?;
            self.changed_files += tx.execute("WITH RECURSIVE subtree(abs_path) AS (
                SELECT abs_path FROM visibility_directories WHERE abs_path = ?1
                UNION ALL SELECT child.abs_path FROM visibility_directories child JOIN subtree ON child.parent_path = subtree.abs_path)
                UPDATE paths SET visibility_flags = own_visibility_flags |
                    (SELECT flags FROM visibility_directories WHERE abs_path = paths.dir_path)
                WHERE dir_path IN (SELECT abs_path FROM subtree)
                  AND visibility_flags != (own_visibility_flags |
                    (SELECT flags FROM visibility_directories WHERE abs_path = paths.dir_path))", [&abs]).map_err(|error| error.to_string())?;
            finish_projection_batch(&tx)?;
            tx.commit().map_err(|error| error.to_string())?;
        }
        self.flags.insert(dir.to_path_buf(), flags);
        Ok(())
    }
}

pub fn root_for(roots: &[String], path: &Path) -> Option<PathBuf> {
    let depth = roots
        .iter()
        .map(|root| crate::winpath::for_fs(Path::new(root)).into_owned())
        .filter(|root| {
            let mut components = path.components();
            root.components().all(|root_part| {
                components.next().is_some_and(|part| {
                    if cfg!(target_os = "windows") {
                        part.as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&root_part.as_os_str().to_string_lossy())
                    } else {
                        part == root_part
                    }
                })
            })
        })
        .map(|root| root.components().count())
        .max()?;
    // Retain the discovered spelling, including the native long-path prefix.
    Some(path.components().take(depth).collect())
}

pub fn source_root_spellings(conn: &Connection, roots: &[String]) -> Result<Vec<String>, String> {
    let mut spellings = Vec::new();
    for root in roots {
        // Watcher events may use the configured alias while the index uses
        // the scanner's settled spelling. Both name the configured boundary.
        spellings.push(
            crate::winpath::for_fs(Path::new(root))
                .to_string_lossy()
                .into_owned(),
        );
        match crate::scanner::settled_root(conn, Path::new(root)) {
            Ok(path) => spellings.push(path.to_string_lossy().into_owned()),
            Err(error) => crate::index_store::upsert_issue(
                conn,
                Some(root),
                crate::scanner::WALK_ERROR,
                &error,
            )?,
        }
    }
    spellings.sort();
    spellings.dedup();
    Ok(spellings)
}

/// Upgrade debt is stat-only, checkpointed, and shares information-work
/// cancellation. It never reopens a failed content/metadata attempt.
pub fn complete_missing_facts(conn: &Connection, roots: &[String]) -> Result<(), String> {
    let pending: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM paths WHERE missing = 0 AND visibility_checked = 0)",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !pending {
        return Ok(());
    }
    let roots = source_root_spellings(conn, roots)?;
    let mut directories = DirectoryFacts::default();
    let mut after = 0;
    loop {
        let mut statement = conn.prepare("SELECT id, abs_path FROM paths WHERE id > ?1 AND missing = 0 AND visibility_checked = 0 ORDER BY id LIMIT 256")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([after], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if rows.is_empty() {
            break;
        }
        for (id, abs) in rows {
            if crate::scanner::cancelled() {
                return Err(crate::scanner::CANCELLED.into());
            }
            after = id;
            let path = Path::new(&abs);
            let Some(root) = root_for(&roots, path) else {
                // Old inventory outside today's configured sources is not
                // authority to stat personal files or keep this worker busy.
                conn.execute(
                    "UPDATE paths SET visibility_checked = -1 WHERE id = ?1",
                    [id],
                )
                .map_err(|error| error.to_string())?;
                continue;
            };
            let result = (|| {
                let inherited =
                    directories.refresh(conn, &root, path.parent().ok_or("File has no parent")?)?;
                let metadata = std::fs::metadata(crate::winpath::for_fs(path).as_ref())
                    .map_err(|error| error.to_string())?;
                let own = visibility::entry_flags(path, &metadata);
                conn.execute("UPDATE paths SET own_visibility_flags = ?2, visibility_flags = ?3, visibility_checked = 1 WHERE id = ?1",
                    params![id, own, own | inherited]).map_err(|error| error.to_string())?;
                Ok::<(), String>(())
            })();
            if let Err(error) = result {
                crate::index_store::upsert_issue(
                    conn,
                    Some(&abs),
                    crate::scanner::STAT_ERROR,
                    &error,
                )?;
                // A failed fact read settles this run. Restart/recheck admit
                // another stat through the existing information boundary.
                conn.execute(
                    "UPDATE paths SET visibility_checked = -1 WHERE id = ?1",
                    [id],
                )
                .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}
