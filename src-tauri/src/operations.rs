//! Destructive operations over LOGICAL items: one decision deletes every
//! physical copy, and companions ride along (pair = one unit for every
//! action). Trash-delete is the default path; permanent delete exists for the
//! explicit Shift flows. Every operation is audit-logged, and partial failures
//! degrade per copy: a copy that cannot be moved keeps its index row and
//! records an issue — the app never pretends a file left the disk.
//!
//! Index consequences: deleted rows leave `paths` (and their evidence with
//! them); when the last row bearing a hash goes, the `contents` row goes too
//! and the cache entries are dropped synchronously (the GC's synchronous
//! half). Trash-side history lives in the day folders' manifests, not here.

use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::json;

use crate::logging;
use crate::preview::{self, CachePaths};
use crate::trash;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeleteMode {
    Trash,
    Permanent,
}

impl DeleteMode {
    fn as_str(self) -> &'static str {
        match self {
            DeleteMode::Trash => "trash",
            DeleteMode::Permanent => "permanent",
        }
    }
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteOutcome {
    pub deleted_files: u64,
    pub failed_files: u64,
    pub removed_rows: u64,
}

/// Identifies a logical item the way the grid does: by content hash, or by
/// path id for unhashed unique-size other-files.
pub enum ItemRef<'a> {
    Hash(&'a str),
    PathId(i64),
}

/// Deletes one logical item: every non-missing copy plus every companion
/// attached to any of those copies.
pub fn delete_item(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    item: ItemRef,
    mode: DeleteMode,
) -> Result<DeleteOutcome, String> {
    // Target rows: the item's own copies…
    let targets: Vec<(i64, String, Option<String>)> = match &item {
        ItemRef::Hash(hash) => collect(
            conn,
            "SELECT id, abs_path, content_hash FROM paths \
             WHERE content_hash = ?1 AND missing = 0",
            params![*hash],
        )?,
        ItemRef::PathId(id) => collect(
            conn,
            "SELECT id, abs_path, content_hash FROM paths WHERE id = ?1 AND missing = 0",
            params![*id],
        )?,
    };
    if targets.is_empty() {
        return Ok(DeleteOutcome::default());
    }
    // …plus companions attached to any of them (pair = one unit).
    let id_list = targets
        .iter()
        .map(|(id, _, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let mut companions: Vec<(i64, String, Option<String>)> = collect(
        conn,
        &format!(
            "SELECT id, abs_path, content_hash FROM paths \
             WHERE companion_of IN ({id_list}) AND missing = 0"
        ),
        params![],
    )?;
    // Companions delete FIRST: their rows hold a foreign key to the primary
    // (`companion_of`), so the primary's row must outlive them.
    companions.extend(targets);
    let targets = companions;

    let mut outcome = DeleteOutcome::default();
    let mut removed_hashes: Vec<Option<String>> = Vec::new();

    for (path_id, abs_path, content_hash) in targets {
        let file = Path::new(&abs_path);
        let result = match mode {
            DeleteMode::Trash => {
                trash::trash_file(file, app_root, content_hash.as_deref()).map(|_| ())
            }
            DeleteMode::Permanent => match std::fs::remove_file(file) {
                Ok(()) => Ok(()),
                // Already gone from disk: the index intent (drop the row)
                // still applies; the walk would have marked it missing anyway.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(err) => Err(err.to_string()),
            },
        };

        match result {
            Ok(()) => {
                outcome.deleted_files += 1;
                conn.execute("DELETE FROM evidence WHERE path_id = ?1", [path_id])
                    .map_err(|e| e.to_string())?;
                conn.execute("DELETE FROM paths WHERE id = ?1", [path_id])
                    .map_err(|e| e.to_string())?;
                outcome.removed_rows += 1;
                removed_hashes.push(content_hash);
            }
            Err(err) => {
                outcome.failed_files += 1;
                conn.execute(
                    "INSERT INTO issues (path, kind, message, created_at_utc) \
                     VALUES (?1, 'delete-error', ?2, ?3)",
                    params![abs_path, err, logging::now_iso_millis()],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }

    // Orphaned contents rows lose their cache entries synchronously.
    for hash in removed_hashes.into_iter().flatten() {
        let remaining: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE content_hash = ?1",
                [&hash],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if remaining == 0 {
            conn.execute(
                "DELETE FROM similar_group_members WHERE content_hash = ?1",
                [&hash],
            )
            .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM contents WHERE hash = ?1", [&hash])
                .map_err(|e| e.to_string())?;
            preview::remove_entries(cache, &hash);
        }
    }

    // The audit line — the op log is a logging concern, not a feature.
    logging::info(
        "delete",
        json!({
            "mode": mode.as_str(),
            "deletedFiles": outcome.deleted_files,
            "failedFiles": outcome.failed_files,
        }),
    );

    Ok(outcome)
}

fn collect(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<(i64, String, Option<String>)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_store;
    use crate::scanner::{self, ScanLists};
    use crate::extensions;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        app_root: std::path::PathBuf,
        cache: CachePaths,
        conn: Connection,
    }

    fn fixture(label: &str) -> Fixture {
        let dir = tempfile::Builder::new()
            .prefix(&format!("onecopy-ops-{label}-"))
            .tempdir()
            .unwrap();
        let root = dir.path().join("root");
        let app_root = dir.path().join("apphome");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&app_root).unwrap();
        let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));
        Fixture {
            _dir: dir,
            root,
            app_root,
            cache,
            conn,
        }
    }

    fn lists() -> ScanLists {
        let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
        ScanLists {
            images: owned(extensions::IMAGE_EXTENSIONS),
            videos: owned(extensions::VIDEO_EXTENSIONS),
            companions: owned(extensions::COMPANION_EXTENSIONS),
        }
    }

    fn scan(f: &Fixture) {
        scanner::walk_root(&f.conn, &f.root, &lists()).unwrap();
        scanner::hash_pending(&f.conn).unwrap();
        scanner::pair_companions(&f.conn).unwrap();
    }

    #[test]
    fn deleting_a_logical_item_trashes_every_copy_and_companion() {
        let f = fixture("cascade");
        for sub in ["a", "b"] {
            std::fs::create_dir_all(f.root.join(sub)).unwrap();
            std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
        }
        // A companion RAW beside copy a.
        std::fs::write(f.root.join("a").join("x.arw"), b"raw-bytes").unwrap();
        scan(&f);

        let hash: String = f
            .conn
            .query_row(
                "SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let outcome = delete_item(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            DeleteMode::Trash,
        )
        .unwrap();
        assert_eq!(outcome.deleted_files, 3, "two copies + one companion");
        assert_eq!(outcome.failed_files, 0);

        // Disk: originals gone, all three in the app-root trash.
        assert!(!f.root.join("a").join("x.jpg").exists());
        assert!(!f.root.join("b").join("x.jpg").exists());
        assert!(!f.root.join("a").join("x.arw").exists());

        // Index: no rows, no contents, no evidence remain.
        let rows: i64 = f
            .conn
            .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        let contents: i64 = f
            .conn
            .query_row("SELECT COUNT(*) FROM contents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(contents, 0);
    }

    #[test]
    fn cache_entries_go_when_the_last_copy_goes() {
        let f = fixture("cache-gc");
        std::fs::write(f.root.join("solo.jpg"), b"solo-bytes").unwrap();
        scan(&f);
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();
        // Simulate derived cache entries.
        for path in [f.cache.thumb(&hash), f.cache.preview(&hash)] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"webp").unwrap();
        }

        delete_item(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            DeleteMode::Trash,
        )
        .unwrap();
        assert!(!f.cache.thumb(&hash).exists());
        assert!(!f.cache.preview(&hash).exists());
    }

    #[test]
    fn permanent_delete_removes_without_trashing() {
        let f = fixture("permanent");
        std::fs::write(f.root.join("gone.jpg"), b"bytes").unwrap();
        scan(&f);
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();

        delete_item(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            DeleteMode::Permanent,
        )
        .unwrap();
        assert!(!f.root.join("gone.jpg").exists());
        // Nothing landed in any trash under the app root.
        assert!(!f.app_root.join("trash").exists());
    }

    #[test]
    fn unhashed_other_files_delete_by_path_id() {
        let f = fixture("by-path");
        std::fs::write(f.root.join("unique.bin"), vec![9u8; 77]).unwrap();
        scan(&f);
        let path_id: i64 = f
            .conn
            .query_row("SELECT id FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let outcome = delete_item(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::PathId(path_id),
            DeleteMode::Trash,
        )
        .unwrap();
        assert_eq!(outcome.deleted_files, 1);
        assert!(!f.root.join("unique.bin").exists());
    }

    #[test]
    fn a_failed_copy_keeps_its_row_and_records_an_issue() {
        let f = fixture("partial");
        std::fs::write(f.root.join("ok.jpg"), b"same").unwrap();
        std::fs::create_dir_all(f.root.join("b")).unwrap();
        std::fs::write(f.root.join("b").join("ok.jpg"), b"same").unwrap();
        scan(&f);
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();
        // Sabotage one copy: replace it with a directory so rename/remove fails.
        std::fs::remove_file(f.root.join("b").join("ok.jpg")).unwrap();
        std::fs::create_dir_all(f.root.join("b").join("ok.jpg")).unwrap();

        let outcome = delete_item(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            DeleteMode::Trash,
        )
        .unwrap();
        assert_eq!(outcome.deleted_files, 1);
        assert_eq!(outcome.failed_files, 1);

        // The failed copy's row survives; the contents row survives with it.
        let rows: i64 = f
            .conn
            .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        let issues: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM issues WHERE kind = 'delete-error'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(issues, 1);
    }
}
