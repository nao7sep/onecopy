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
                // The issues table is the user surface; the session log is
                // the debugging record — mirror the failure where it is
                // raised, with its context intact.
                logging::warn(
                    "delete failed for one copy",
                    json!({ "path": abs_path, "error": { "message": err } }),
                );
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoveOutMode {
    /// Plain drag: one copy moves out, the remaining copies go to trash.
    MoveTrashRest,
    /// Shift: one copy moves out, the remaining copies are deleted permanently.
    MoveDeleteRest,
    /// Cmd/Ctrl: a copy is exported; nothing else is touched.
    CopyKeepAll,
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveOutOutcome {
    pub exported: u64,
    pub skipped_identical: u64,
    /// Destination names that exist with DIFFERENT content — surfaced, never
    /// auto-suffixed; the post-action does not run when a conflict blocks the
    /// primary.
    pub conflicts: Vec<String>,
    pub post_action: DeleteOutcome,
}

/// Moves or copies one logical item out to `dest_dir`: the primary plus one
/// instance of each distinct companion, with the verified-copy pipeline —
/// tee-hash against the indexed hash (a mismatch means the chosen copy rotted;
/// the next copy is tried and the rotted one becomes an issue), then a
/// read-back verify of the destination. Collisions: identical content →
/// skip-as-delivered; different content → conflict, reported, post-action
/// withheld.
pub fn move_out(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    item: ItemRef,
    dest_dir: &Path,
    mode: MoveOutMode,
) -> Result<MoveOutOutcome, String> {
    if !dest_dir.is_dir() {
        return Err(format!("destination is not a directory: {}", dest_dir.display()));
    }

    let (copies, expected_hash) = match &item {
        ItemRef::Hash(hash) => (
            collect(
                conn,
                "SELECT id, abs_path, content_hash FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 ORDER BY id",
                params![*hash],
            )?,
            Some(hash.to_string()),
        ),
        ItemRef::PathId(id) => (
            collect(
                conn,
                "SELECT id, abs_path, content_hash FROM paths WHERE id = ?1 AND missing = 0",
                params![*id],
            )?,
            None,
        ),
    };
    let Some(first) = copies.first() else {
        return Ok(MoveOutOutcome::default());
    };

    let mut outcome = MoveOutOutcome::default();

    // The primary lands under its file name; companions land beside it, one
    // instance per distinct companion file name.
    let primary_name = Path::new(&first.1)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "copy has no file name".to_string())?
        .to_string();
    // A provisional identity has never been fully read: there is no source
    // claim to verify against, so delivery degrades honestly to
    // "verified against the source read" (tee + read-back), and the tee's
    // hash — the file's FIRST full hash — promotes the identity afterwards.
    let provisional = expected_hash
        .as_deref()
        .map(crate::scanner::is_provisional)
        .unwrap_or(false);
    let verify_hash = if provisional { None } else { expected_hash.as_deref() };
    let delivery = deliver_one(
        conn,
        &copies,
        verify_hash,
        &dest_dir.join(&primary_name),
        &mut outcome,
    )?;
    let mut promoted_to: Option<String> = None;
    if provisional && delivery.ok {
        if let (Some(stored), Some(real)) = (expected_hash.as_deref(), &delivery.real_hash) {
            crate::scanner::promote_identity(conn, cache, stored, real)?;
            promoted_to = Some(real.clone());
        }
    }
    if !delivery.ok {
        // Conflict (or every copy failed): report and leave the world alone.
        return Ok(outcome);
    }

    // Companions, grouped by file name (each group's members are copies of
    // one another in a synced collection; one instance is delivered).
    let id_list = copies
        .iter()
        .map(|(id, _, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let companions: Vec<(i64, String, Option<String>)> = collect(
        conn,
        &format!(
            "SELECT id, abs_path, content_hash FROM paths \
             WHERE companion_of IN ({id_list}) AND missing = 0 ORDER BY id"
        ),
        params![],
    )?;
    let mut by_name: std::collections::HashMap<String, Vec<(i64, String, Option<String>)>> =
        std::collections::HashMap::new();
    for companion in companions {
        let name = Path::new(&companion.1)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("companion")
            .to_lowercase();
        by_name.entry(name).or_default().push(companion);
    }
    for (_name, group) in by_name {
        let target_name = Path::new(&group[0].1)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("companion")
            .to_string();
        let expected = group[0].2.clone();
        // Companions are other-file tier — never provisional.
        let _ = deliver_one(
            conn,
            &group,
            expected.as_deref(),
            &dest_dir.join(target_name),
            &mut outcome,
        )?;
    }

    // Post-action over the originals (the item + companions as one unit).
    match mode {
        MoveOutMode::CopyKeepAll => {}
        MoveOutMode::MoveTrashRest | MoveOutMode::MoveDeleteRest => {
            let delete_mode = if mode == MoveOutMode::MoveTrashRest {
                DeleteMode::Trash
            } else {
                DeleteMode::Permanent
            };
            // A promotion during delivery repointed the rows: the post-action
            // must act on the REAL identity, not the retired provisional key.
            let post_item = match (&promoted_to, &item) {
                (Some(real), _) => ItemRef::Hash(real),
                (None, ItemRef::Hash(hash)) => ItemRef::Hash(hash),
                (None, ItemRef::PathId(id)) => ItemRef::PathId(*id),
            };
            outcome.post_action = delete_item(conn, app_root, cache, post_item, delete_mode)?;
        }
    }

    logging::info(
        "move out",
        json!({
            "mode": match mode {
                MoveOutMode::MoveTrashRest => "move+trash",
                MoveOutMode::MoveDeleteRest => "move+delete",
                MoveOutMode::CopyKeepAll => "copy",
            },
            "exported": outcome.exported,
            "conflicts": outcome.conflicts.len(),
        }),
    );

    Ok(outcome)
}

/// What one delivery attempt produced: whether the destination ends up
/// holding the expected content, and — when a full read happened along the
/// way — the content's REAL hash (what promotes a provisional identity).
struct Delivery {
    ok: bool,
    real_hash: Option<String>,
}

/// Delivers one file (trying each listed copy in order) to `target`.
fn deliver_one(
    conn: &Connection,
    copies: &[(i64, String, Option<String>)],
    expected_hash: Option<&str>,
    target: &Path,
    outcome: &mut MoveOutOutcome,
) -> Result<Delivery, String> {
    if target.exists() {
        let existing = crate::hashing::full_hash(target).map_err(|e| e.to_string())?;
        let matches = match expected_hash {
            Some(expected) => existing == expected,
            // Unhashed item: compare against the first copy's actual bytes.
            None => match copies.first() {
                Some((_, path, _)) => {
                    crate::hashing::full_hash(Path::new(path)).map_err(|e| e.to_string())?
                        == existing
                }
                None => false,
            },
        };
        if matches {
            outcome.skipped_identical += 1;
            return Ok(Delivery { ok: true, real_hash: Some(existing) }); // already delivered
        }
        outcome
            .conflicts
            .push(target.to_string_lossy().to_string());
        return Ok(Delivery { ok: false, real_hash: None });
    }

    for (_, source_path, _) in copies {
        let source = Path::new(source_path);
        match crate::hashing::hash_while_copying(source, target) {
            Ok((streamed_hash, _bytes)) => {
                // Source verification is free: the tee hashed what was read.
                if let Some(expected) = expected_hash {
                    if streamed_hash != expected {
                        let _ = std::fs::remove_file(target);
                        record_rot_issue(conn, source_path, expected, &streamed_hash)?;
                        continue; // the redundant copies pay off: try the next
                    }
                }
                // Read-back verify of the destination.
                let read_back = crate::hashing::full_hash(target).map_err(|e| e.to_string())?;
                if read_back != streamed_hash {
                    let _ = std::fs::remove_file(target);
                    return Err(format!(
                        "destination read-back mismatch at {} (failing storage?)",
                        target.display()
                    ));
                }
                outcome.exported += 1;
                return Ok(Delivery { ok: true, real_hash: Some(streamed_hash) });
            }
            Err(err) => {
                let _ = std::fs::remove_file(target);
                logging::warn(
                    "copy-out failed for one source",
                    json!({ "path": source_path, "error": { "message": err.to_string() } }),
                );
                conn.execute(
                    "INSERT INTO issues (path, kind, message, created_at_utc) \
                     VALUES (?1, 'copy-error', ?2, ?3)",
                    params![source_path, err.to_string(), logging::now_iso_millis()],
                )
                .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(Delivery { ok: false, real_hash: None })
}

fn record_rot_issue(
    conn: &Connection,
    source_path: &str,
    expected: &str,
    actual: &str,
) -> Result<(), String> {
    logging::warn(
        "source copy failed tee verification (rot or divergence)",
        json!({ "path": source_path, "expected": expected, "actual": actual }),
    );
    conn.execute(
        "INSERT INTO issues (path, kind, message, created_at_utc) \
         VALUES (?1, 'copy-verify-mismatch', ?2, ?3)",
        params![
            source_path,
            format!("indexed {expected} but read {actual} — bit rot or external change"),
            logging::now_iso_millis()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
        scanner::hash_pending(&f.conn, &f.cache).unwrap();
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
    fn move_out_delivers_primary_and_companion_then_trashes_the_rest() {
        let f = fixture("moveout");
        for sub in ["a", "b"] {
            std::fs::create_dir_all(f.root.join(sub)).unwrap();
            std::fs::write(f.root.join(sub).join("x.jpg"), b"same-bytes").unwrap();
            std::fs::write(f.root.join(sub).join("x.arw"), b"raw-bytes").unwrap();
        }
        scan(&f);
        let dest = f._dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let hash: String = f
            .conn
            .query_row(
                "SELECT content_hash FROM paths WHERE file_name = 'x.jpg' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();

        let outcome = move_out(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            &dest,
            MoveOutMode::MoveTrashRest,
        )
        .unwrap();

        assert_eq!(outcome.exported, 2, "primary + one companion instance");
        assert!(outcome.conflicts.is_empty());
        assert_eq!(std::fs::read(dest.join("x.jpg")).unwrap(), b"same-bytes");
        assert_eq!(std::fs::read(dest.join("x.arw")).unwrap(), b"raw-bytes");
        // All four originals left their places (post-action trashed them).
        assert_eq!(outcome.post_action.deleted_files, 4);
        assert!(!f.root.join("a").join("x.jpg").exists());
        assert!(!f.root.join("b").join("x.arw").exists());
        let rows: i64 = f
            .conn
            .query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "inbox-zero: nothing remains in the index");
    }

    #[test]
    fn copy_mode_exports_and_leaves_everything_untouched() {
        let f = fixture("copy-mode");
        std::fs::write(f.root.join("keep.jpg"), b"kept-bytes").unwrap();
        scan(&f);
        let dest = f._dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let outcome = move_out(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            &dest,
            MoveOutMode::CopyKeepAll,
        )
        .unwrap();
        assert_eq!(outcome.exported, 1);
        assert_eq!(outcome.post_action.deleted_files, 0);
        assert!(f.root.join("keep.jpg").exists(), "copy mode never deletes");
        assert!(dest.join("keep.jpg").exists());
    }

    #[test]
    fn identical_destination_skips_but_still_runs_the_post_action() {
        let f = fixture("identical");
        std::fs::write(f.root.join("dup.jpg"), b"dup-bytes").unwrap();
        scan(&f);
        let dest = f._dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("dup.jpg"), b"dup-bytes").unwrap(); // already delivered
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let outcome = move_out(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            &dest,
            MoveOutMode::MoveTrashRest,
        )
        .unwrap();
        assert_eq!(outcome.skipped_identical, 1);
        assert_eq!(outcome.exported, 0);
        assert_eq!(outcome.post_action.deleted_files, 1, "post-action proceeds");
        assert!(!f.root.join("dup.jpg").exists());
    }

    #[test]
    fn conflicting_destination_blocks_and_withholds_the_post_action() {
        let f = fixture("conflict");
        std::fs::write(f.root.join("clash.jpg"), b"mine").unwrap();
        scan(&f);
        let dest = f._dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("clash.jpg"), b"theirs - different").unwrap();
        let hash: String = f
            .conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let outcome = move_out(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            &dest,
            MoveOutMode::MoveTrashRest,
        )
        .unwrap();
        assert_eq!(outcome.conflicts.len(), 1);
        assert_eq!(outcome.exported, 0);
        assert_eq!(outcome.post_action.deleted_files, 0, "no destructive follow-up");
        assert!(f.root.join("clash.jpg").exists(), "originals untouched");
        assert_eq!(
            std::fs::read(dest.join("clash.jpg")).unwrap(),
            b"theirs - different".as_slice(),
            "the conflicting file is never overwritten"
        );
    }

    #[test]
    fn a_rotted_copy_is_skipped_and_the_next_copy_delivers() {
        let f = fixture("rot");
        for sub in ["a", "b"] {
            std::fs::create_dir_all(f.root.join(sub)).unwrap();
            std::fs::write(f.root.join(sub).join("r.jpg"), b"healthy-bytes").unwrap();
        }
        scan(&f);
        let hash: String = f
            .conn
            .query_row(
                "SELECT content_hash FROM paths WHERE file_name = 'r.jpg' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Rot copy a AFTER indexing: same length, different bytes.
        std::fs::write(f.root.join("a").join("r.jpg"), b"rotten!-bytes").unwrap();

        let dest = f._dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let outcome = move_out(
            &f.conn,
            &f.app_root,
            &f.cache,
            ItemRef::Hash(&hash),
            &dest,
            MoveOutMode::CopyKeepAll,
        )
        .unwrap();

        assert_eq!(outcome.exported, 1);
        // The delivered bytes are the healthy ones, never the rotted ones.
        assert_eq!(std::fs::read(dest.join("r.jpg")).unwrap(), b"healthy-bytes");
        let issues: i64 = f
            .conn
            .query_row(
                "SELECT COUNT(*) FROM issues WHERE kind = 'copy-verify-mismatch'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(issues, 1, "the rotted copy is surfaced");
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
