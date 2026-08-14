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
        // Only LIVE copies keep an identity alive. Counting missing rows too
        // meant one copy on an absent drive pinned the contents row and every
        // cache entry for that hash forever — a leak that accumulates across a
        // cull session and that no sweep reclaims, since startup_sweep only
        // drops cache whose hash is absent from contents.
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE content_hash = ?1 AND missing = 0",
                [&hash],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if live == 0 {
            // Missing rows are files that are not on disk; they may not hold a
            // foreign key into a contents row that is about to go.
            conn.execute("DELETE FROM paths WHERE content_hash = ?1", [&hash])
                .map_err(|e| e.to_string())?;
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
    /// Files that could not be written at all — every source copy failed to
    /// read or the destination refused the write (a full disk is the common
    /// case). Distinct from `conflicts`, which means the destination already
    /// holds different content; this failure leaves NOTHING at the target and
    /// previously had no way to be expressed at all.
    pub undelivered: Vec<String>,
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
        let target = dest_dir.join(&target_name);
        let companion = deliver_one(conn, &group, expected.as_deref(), &target, &mut outcome)?;
        if !companion.ok {
            // A conflict already recorded itself; a total copy failure did not,
            // so record it here — otherwise this branch reports nothing at all.
            if !outcome
                .conflicts
                .iter()
                .any(|c| c == &target.to_string_lossy())
            {
                outcome.undelivered.push(target.to_string_lossy().to_string());
            }
        }
    }

    // The item and its companions move as ONE unit, so an undelivered
    // companion withholds the post-action exactly as an undelivered primary
    // does. Deleting the source of a file that never reached the destination
    // destroys it, and companions (RAW, sidecars) are never their own grid
    // row — the loss would be invisible in the UI and unrecoverable under
    // MoveDeleteRest.
    if !outcome.conflicts.is_empty() || !outcome.undelivered.is_empty() {
        return Ok(outcome);
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
