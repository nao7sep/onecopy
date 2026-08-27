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

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Serialize, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ItemIdentity {
    pub hash: Option<String>,
    pub path_id: Option<i64>,
}

impl ItemIdentity {
    fn item_ref(&self) -> Result<ItemRef<'_>, String> {
        match (&self.hash, self.path_id) {
            (Some(hash), None) if !hash.is_empty() => Ok(ItemRef::Hash(hash)),
            (None, Some(path_id)) => Ok(ItemRef::PathId(path_id)),
            _ => Err("each item needs exactly one non-empty hash or pathId".to_string()),
        }
    }

    pub fn media_key(&self) -> Result<String, String> {
        match self.item_ref()? {
            ItemRef::Hash(hash) => Ok(hash.to_string()),
            ItemRef::PathId(path_id) => Ok(format!("path-{path_id}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteBatchProgress {
    Planning {
        items_done: u64,
        items_total: u64,
        files_total: u64,
        bytes_total: u64,
    },
    Deleting {
        items_done: u64,
        items_total: u64,
        files_done: u64,
        files_total: u64,
        bytes_done: u64,
        bytes_total: u64,
        failures: u64,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteItemResult {
    pub item: ItemIdentity,
    pub deleted_files: u64,
    pub failed_files: u64,
    pub removed_rows: u64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteBatchOutcome {
    pub cancelled: bool,
    pub error: Option<String>,
    pub items: Vec<DeleteItemResult>,
    pub deleted_files: u64,
    pub failed_files: u64,
    pub removed_rows: u64,
    pub files_total: u64,
    pub bytes_total: u64,
}

#[derive(Clone, Debug)]
struct DeleteTarget {
    path_id: i64,
    abs_path: String,
    content_hash: Option<String>,
    bytes: u64,
}

#[derive(Clone, Debug)]
struct DeleteUnit {
    item: ItemIdentity,
    targets: Vec<DeleteTarget>,
}

#[derive(Clone, Debug, Default)]
struct DeletePlan {
    units: Vec<DeleteUnit>,
    files_total: u64,
    bytes_total: u64,
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
    delete_item_inner(conn, app_root, cache, item, mode, |_, _| {})
}

fn delete_item_inner(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    item: ItemRef,
    mode: DeleteMode,
    mut before_source_claim: impl FnMut(DeleteMode, &Path),
) -> Result<DeleteOutcome, String> {
    let targets = collect_delete_targets(conn, item)?;
    delete_targets(
        conn,
        app_root,
        cache,
        &targets,
        mode,
        &mut before_source_claim,
        &mut |_, _| {},
    )
}

fn delete_targets(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    targets: &[DeleteTarget],
    mode: DeleteMode,
    before_source_claim: &mut impl FnMut(DeleteMode, &Path),
    on_attempt: &mut (impl FnMut(u64, bool) + ?Sized),
) -> Result<DeleteOutcome, String> {
    let mut outcome = DeleteOutcome::default();
    let mut removed_hashes: Vec<Option<String>> = Vec::new();

    for target in targets {
        let file = Path::new(&target.abs_path);
        let result = match mode {
            DeleteMode::Trash => trash::trash_file_with_before_claim(
                file,
                app_root,
                target.content_hash.as_deref(),
                |path| before_source_claim(mode, path),
            )
            .map(|_| ()),
            DeleteMode::Permanent => {
                permanently_delete_file(file, |path| before_source_claim(mode, path))
            }
        };

        match result {
            Ok(()) => {
                outcome.deleted_files += 1;
                conn.execute("DELETE FROM evidence WHERE path_id = ?1", [target.path_id])
                    .map_err(|e| e.to_string())?;
                conn.execute("DELETE FROM paths WHERE id = ?1", [target.path_id])
                    .map_err(|e| e.to_string())?;
                outcome.removed_rows += 1;
                removed_hashes.push(target.content_hash.clone());
                on_attempt(target.bytes, false);
            }
            Err(err) => {
                outcome.failed_files += 1;
                // The issues table is the user surface; the session log is
                // the debugging record — mirror the failure where it is
                // raised, with its context intact.
                logging::warn(
                    "delete failed for one copy",
                    json!({ "path": target.abs_path, "error": { "message": err } }),
                );
                crate::index_store::upsert_issue(
                    conn,
                    Some(&target.abs_path),
                    "delete-error",
                    &err,
                )?;
                on_attempt(target.bytes, true);
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

fn collect_delete_targets(
    conn: &Connection,
    item: ItemRef<'_>,
) -> Result<Vec<DeleteTarget>, String> {
    // Target rows: the item's own copies… plus companions attached to any of
    // them. The companion query stays parameterized and constant-size even if
    // one logical item has an extreme number of copies.
    let (targets, mut companions): (Vec<_>, Vec<_>) = match item {
        ItemRef::Hash(hash) => (
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 ORDER BY id",
                params![hash],
            )?,
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE companion_of IN (\
                   SELECT id FROM paths WHERE content_hash = ?1 AND missing = 0\
                 ) AND missing = 0 ORDER BY id",
                params![hash],
            )?,
        ),
        ItemRef::PathId(id) => (
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE id = ?1 AND missing = 0",
                params![id],
            )?,
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE companion_of = ?1 AND missing = 0 ORDER BY id",
                params![id],
            )?,
        ),
    };
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    // Companions delete FIRST: their rows hold a foreign key to the primary
    // (`companion_of`), so the primary's row must outlive them.
    companions.extend(targets);
    Ok(companions
        .into_iter()
        .map(|(path_id, abs_path, content_hash, indexed_bytes)| {
            let bytes =
                std::fs::symlink_metadata(crate::winpath::for_fs(Path::new(&abs_path)).as_ref())
                    .ok()
                    .filter(|metadata| metadata.file_type().is_file())
                    .map(|metadata| metadata.len())
                    .unwrap_or_else(|| indexed_bytes.unwrap_or(0).max(0) as u64);
            DeleteTarget {
                path_id,
                abs_path,
                content_hash,
                bytes,
            }
        })
        .collect())
}

/// Deletes an ordered logical-item set under one already-acquired mutation and
/// media boundary. Target membership is resolved once before the first file
/// changes. Cancellation is observed while planning and only between complete
/// logical units during deletion; filesystem failures remain per-file Issues.
pub fn delete_batch(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    mode: DeleteMode,
    cancelled: &dyn Fn() -> bool,
    mut on_progress: impl FnMut(DeleteBatchProgress),
) -> Result<DeleteBatchOutcome, String> {
    let mut unique = HashSet::new();
    let mut ordered = Vec::new();
    for item in items {
        item.item_ref()?;
        if unique.insert(item.clone()) {
            ordered.push(item.clone());
        }
    }
    let items_total = ordered.len() as u64;
    on_progress(DeleteBatchProgress::Planning {
        items_done: 0,
        items_total,
        files_total: 0,
        bytes_total: 0,
    });

    let mut plan = DeletePlan::default();
    let mut claimed_paths = HashSet::new();
    for item in ordered {
        if cancelled() {
            return Ok(DeleteBatchOutcome {
                cancelled: true,
                files_total: plan.files_total,
                bytes_total: plan.bytes_total,
                ..DeleteBatchOutcome::default()
            });
        }
        let mut targets = collect_delete_targets(conn, item.item_ref()?)?;
        // A malformed caller can name overlapping identities. Physical rows
        // still belong to exactly one unit in this immutable plan.
        targets.retain(|target| claimed_paths.insert(target.path_id));
        plan.files_total = plan.files_total.saturating_add(targets.len() as u64);
        plan.bytes_total = plan
            .bytes_total
            .saturating_add(targets.iter().map(|target| target.bytes).sum::<u64>());
        plan.units.push(DeleteUnit { item, targets });
        on_progress(DeleteBatchProgress::Planning {
            items_done: plan.units.len() as u64,
            items_total,
            files_total: plan.files_total,
            bytes_total: plan.bytes_total,
        });
    }

    let mut batch = DeleteBatchOutcome {
        files_total: plan.files_total,
        bytes_total: plan.bytes_total,
        ..DeleteBatchOutcome::default()
    };
    let mut items_done = 0u64;
    let mut files_done = 0u64;
    let mut bytes_done = 0u64;
    on_progress(DeleteBatchProgress::Deleting {
        items_done,
        items_total,
        files_done,
        files_total: plan.files_total,
        bytes_done,
        bytes_total: plan.bytes_total,
        failures: 0,
    });

    for unit in plan.units {
        if cancelled() {
            batch.cancelled = true;
            break;
        }
        let failures_before = batch.failed_files;
        let mut failures_in_unit = 0u64;
        let outcome = delete_targets(
            conn,
            app_root,
            cache,
            &unit.targets,
            mode,
            &mut |_, _| {},
            &mut |bytes, failed| {
                files_done = files_done.saturating_add(1);
                bytes_done = bytes_done.saturating_add(bytes);
                failures_in_unit = failures_in_unit.saturating_add(u64::from(failed));
                on_progress(DeleteBatchProgress::Deleting {
                    items_done,
                    items_total,
                    files_done,
                    files_total: plan.files_total,
                    bytes_done,
                    bytes_total: plan.bytes_total,
                    failures: failures_before + failures_in_unit,
                });
            },
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                logging::warn(
                    "delete batch stopped inside one logical item",
                    json!({ "error": { "message": error } }),
                );
                batch.error = Some(error);
                break;
            }
        };
        batch.deleted_files = batch.deleted_files.saturating_add(outcome.deleted_files);
        batch.failed_files = batch.failed_files.saturating_add(outcome.failed_files);
        batch.removed_rows = batch.removed_rows.saturating_add(outcome.removed_rows);
        batch.items.push(DeleteItemResult {
            item: unit.item,
            deleted_files: outcome.deleted_files,
            failed_files: outcome.failed_files,
            removed_rows: outcome.removed_rows,
        });
        items_done = items_done.saturating_add(1);
        on_progress(DeleteBatchProgress::Deleting {
            items_done,
            items_total,
            files_done,
            files_total: plan.files_total,
            bytes_done,
            bytes_total: plan.bytes_total,
            failures: batch.failed_files,
        });
    }

    Ok(batch)
}

fn permanently_delete_file(file: &Path, before_claim: impl FnOnce(&Path)) -> Result<(), String> {
    let (_descriptor, identity) = match crate::file_identity::open_regular_nofollow(file) {
        Ok(opened) => opened,
        // Already gone from disk: the index intent still applies; the walk
        // would have marked it missing anyway.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    before_claim(file);
    let claimed = crate::file_identity::claim_private(file, identity)
        .map_err(|error| format!("source changed before permanent deletion: {error}"))?;
    if let Err(error) = std::fs::remove_file(crate::winpath::for_fs(&claimed).as_ref()) {
        let restore = crate::file_identity::restore_private_claim(&claimed, file, identity);
        return Err(match restore {
            Ok(()) => format!("permanent deletion failed; source restored: {error}"),
            Err(restore_error) => format!(
                "permanent deletion failed: {error}; source recovery also failed: {restore_error}"
            ),
        });
    }
    require_original_name_clear(file)
}

fn require_original_name_clear(file: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(crate::winpath::for_fs(file).as_ref()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "source was replaced during deletion; replacement preserved and index row retained: {}",
            file.display(),
        )),
        Err(error) => Err(format!(
            "could not revalidate deleted source {}; index row retained: {error}",
            file.display(),
        )),
    }
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

#[derive(Clone, Serialize, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveBatchProgress {
    Planning {
        items_done: u64,
        items_total: u64,
        files_total: u64,
        bytes_total: u64,
        current_file_bytes_done: Option<u64>,
        current_file_bytes_total: Option<u64>,
    },
    Delivering {
        items_done: u64,
        items_total: u64,
        files_done: u64,
        files_total: u64,
        bytes_done: u64,
        bytes_total: u64,
        failures: u64,
        current_file_bytes_done: Option<u64>,
        current_file_bytes_total: Option<u64>,
    },
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveBatchItemResult {
    pub item: ItemIdentity,
    pub outcome: MoveOutOutcome,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveBatchOutcome {
    pub cancelled: bool,
    pub error: Option<String>,
    pub items: Vec<MoveBatchItemResult>,
    pub exported: u64,
    pub skipped_identical: u64,
    pub conflicts: Vec<String>,
    pub undelivered: Vec<String>,
    pub post_action: DeleteOutcome,
    pub files_total: u64,
    pub bytes_total: u64,
}

#[derive(Clone, Debug)]
struct DeliverySource {
    abs_path: String,
    content_hash: Option<String>,
    bytes: u64,
}

#[derive(Clone, Debug)]
enum PlannedTarget {
    Missing,
    Existing {
        identity: crate::file_identity::FileIdentity,
        verified_hash: String,
    },
    Conflict,
}

#[derive(Clone, Debug)]
struct DeliveryPlan {
    target: std::path::PathBuf,
    sources: Vec<DeliverySource>,
    expected_hash: Option<String>,
    planned_target: PlannedTarget,
    bytes: u64,
    primary: bool,
}

#[derive(Clone, Debug)]
struct MoveUnit {
    item: ItemIdentity,
    deliveries: Vec<DeliveryPlan>,
    delete_targets: Vec<DeleteTarget>,
    preflight_conflicts: Vec<String>,
    provisional_hash: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct MovePlan {
    units: Vec<MoveUnit>,
    files_total: u64,
    bytes_total: u64,
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
    let identity = match item {
        ItemRef::Hash(hash) => ItemIdentity {
            hash: Some(hash.to_string()),
            path_id: None,
        },
        ItemRef::PathId(path_id) => ItemIdentity {
            hash: None,
            path_id: Some(path_id),
        },
    };
    let batch = move_batch(
        conn,
        app_root,
        cache,
        &[identity],
        dest_dir,
        mode,
        &|| false,
        |_| {},
    )?;
    if let Some(error) = batch.error {
        return Err(error);
    }
    Ok(batch
        .items
        .into_iter()
        .next()
        .map(|result| result.outcome)
        .unwrap_or_default())
}

/// Moves or copies one ordered logical-item set under one mutation/media
/// boundary. Membership and destination names are frozen before the first
/// publication. Cancellation is honored only while work remains private or
/// between logical items; once one item's publication starts, its required
/// source post-action runs to completion without consulting cancellation.
pub fn move_batch(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    dest_dir: &Path,
    mode: MoveOutMode,
    cancelled: &dyn Fn() -> bool,
    mut on_progress: impl FnMut(MoveBatchProgress),
) -> Result<MoveBatchOutcome, String> {
    if !dest_dir.is_dir() {
        return Err(format!(
            "destination is not a directory: {}",
            dest_dir.display()
        ));
    }

    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for item in items {
        item.item_ref()?;
        if seen.insert(item.clone()) {
            ordered.push(item.clone());
        }
    }
    let items_total = ordered.len() as u64;
    let mut plan = MovePlan::default();
    on_progress(MoveBatchProgress::Planning {
        items_done: 0,
        items_total,
        files_total: 0,
        bytes_total: 0,
        current_file_bytes_done: None,
        current_file_bytes_total: None,
    });

    for item in ordered {
        if cancelled() {
            return Ok(MoveBatchOutcome {
                cancelled: true,
                files_total: plan.files_total,
                bytes_total: plan.bytes_total,
                ..MoveBatchOutcome::default()
            });
        }
        let mut unit = collect_move_unit(conn, item, dest_dir, mode)?;
        for delivery in &mut unit.deliveries {
            let mut current = (None, None);
            delivery.planned_target =
                plan_existing_target(&delivery.target, cancelled, &mut |done, total| {
                    current = (Some(done), Some(total));
                    on_progress(MoveBatchProgress::Planning {
                        items_done: plan.units.len() as u64,
                        items_total,
                        files_total: plan.files_total,
                        bytes_total: plan.bytes_total,
                        current_file_bytes_done: current.0,
                        current_file_bytes_total: current.1,
                    });
                });
            if cancelled() {
                return Ok(MoveBatchOutcome {
                    cancelled: true,
                    files_total: plan.files_total,
                    bytes_total: plan.bytes_total,
                    ..MoveBatchOutcome::default()
                });
            }
            if let PlannedTarget::Existing { verified_hash, .. } = &delivery.planned_target {
                if delivery
                    .expected_hash
                    .as_ref()
                    .is_some_and(|expected| expected != verified_hash)
                {
                    delivery.planned_target = PlannedTarget::Conflict;
                }
            }
        }
        plan.files_total = plan
            .files_total
            .saturating_add(unit.deliveries.len() as u64);
        plan.bytes_total = plan.bytes_total.saturating_add(
            unit.deliveries
                .iter()
                .map(|delivery| delivery.bytes)
                .sum::<u64>(),
        );
        if mode != MoveOutMode::CopyKeepAll {
            plan.files_total = plan
                .files_total
                .saturating_add(unit.delete_targets.len() as u64);
            plan.bytes_total = plan.bytes_total.saturating_add(
                unit.delete_targets
                    .iter()
                    .map(|target| target.bytes)
                    .sum::<u64>(),
            );
        }
        plan.units.push(unit);
        on_progress(MoveBatchProgress::Planning {
            items_done: plan.units.len() as u64,
            items_total,
            files_total: plan.files_total,
            bytes_total: plan.bytes_total,
            current_file_bytes_done: None,
            current_file_bytes_total: None,
        });
    }
    reserve_batch_target_names(&mut plan.units);

    let mut batch = MoveBatchOutcome {
        files_total: plan.files_total,
        bytes_total: plan.bytes_total,
        ..MoveBatchOutcome::default()
    };
    let mut items_done = 0u64;
    let mut files_done = 0u64;
    let mut bytes_done = 0u64;
    let mut failures = 0u64;
    on_progress(MoveBatchProgress::Delivering {
        items_done,
        items_total,
        files_done,
        files_total: plan.files_total,
        bytes_done,
        bytes_total: plan.bytes_total,
        failures,
        current_file_bytes_done: None,
        current_file_bytes_total: None,
    });

    for unit in plan.units {
        if cancelled() {
            batch.cancelled = true;
            break;
        }
        let execution = execute_move_unit(
            conn,
            app_root,
            cache,
            &unit,
            mode,
            cancelled,
            &mut |progress| {
                let (current_done, current_total) = match progress {
                    MoveUnitProgress::Stream { done, total } => (Some(done), Some(total)),
                    MoveUnitProgress::Attempt { bytes, failed } => {
                        files_done = files_done.saturating_add(1);
                        bytes_done = bytes_done.saturating_add(bytes);
                        failures = failures.saturating_add(u64::from(failed));
                        (None, None)
                    }
                };
                on_progress(MoveBatchProgress::Delivering {
                    items_done,
                    items_total,
                    files_done,
                    files_total: plan.files_total,
                    bytes_done: bytes_done.saturating_add(
                        current_done
                            .zip(current_total)
                            .map(|(done, total)| done.min(total))
                            .unwrap_or(0),
                    ),
                    bytes_total: plan.bytes_total,
                    failures,
                    current_file_bytes_done: current_done,
                    current_file_bytes_total: current_total,
                });
            },
        );
        let outcome = match execution {
            Ok(Some(outcome)) => outcome,
            Ok(None) => {
                batch.cancelled = true;
                break;
            }
            Err(error) => {
                logging::warn(
                    "destination batch stopped inside one logical item",
                    json!({ "error": { "message": error } }),
                );
                batch.error = Some(error);
                break;
            }
        };
        // Undelivered files already incremented failures with their attempted
        // output. Preflight/publication conflicts did not attempt a file, so
        // they enter the aggregate here exactly once.
        failures = failures.saturating_add(outcome.conflicts.len() as u64);
        batch.exported = batch.exported.saturating_add(outcome.exported);
        batch.skipped_identical = batch
            .skipped_identical
            .saturating_add(outcome.skipped_identical);
        batch.conflicts.extend(outcome.conflicts.iter().cloned());
        batch
            .undelivered
            .extend(outcome.undelivered.iter().cloned());
        batch.post_action.deleted_files = batch
            .post_action
            .deleted_files
            .saturating_add(outcome.post_action.deleted_files);
        batch.post_action.failed_files = batch
            .post_action
            .failed_files
            .saturating_add(outcome.post_action.failed_files);
        batch.post_action.removed_rows = batch
            .post_action
            .removed_rows
            .saturating_add(outcome.post_action.removed_rows);
        batch.items.push(MoveBatchItemResult {
            item: unit.item,
            outcome,
        });
        items_done = items_done.saturating_add(1);
        on_progress(MoveBatchProgress::Delivering {
            items_done,
            items_total,
            files_done,
            files_total: plan.files_total,
            bytes_done,
            bytes_total: plan.bytes_total,
            failures,
            current_file_bytes_done: None,
            current_file_bytes_total: None,
        });
    }

    logging::info(
        "move out batch",
        json!({
            "mode": match mode {
                MoveOutMode::MoveTrashRest => "move+trash",
                MoveOutMode::MoveDeleteRest => "move+delete",
                MoveOutMode::CopyKeepAll => "copy",
            },
            "items": batch.items.len(),
            "exported": batch.exported,
            "conflicts": batch.conflicts.len(),
            "cancelled": batch.cancelled,
        }),
    );
    Ok(batch)
}

fn collect_move_unit(
    conn: &Connection,
    item: ItemIdentity,
    dest_dir: &Path,
    mode: MoveOutMode,
) -> Result<MoveUnit, String> {
    let item_ref = item.item_ref()?;
    if let ItemRef::Hash(hash) = item_ref {
        let distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT lower(file_name)) FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 AND companion_of IS NULL",
                [hash],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if distinct > 1 {
            return Err(format!(
                "one selected item's copies carry {distinct} different names — reveal the copies and resolve the names first"
            ));
        }
    }

    let (primary_rows, companion_rows): (Vec<_>, Vec<_>) = match item.item_ref()? {
        ItemRef::Hash(hash) => (
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 AND companion_of IS NULL ORDER BY id",
                params![hash],
            )?,
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE companion_of IN (SELECT id FROM paths WHERE content_hash = ?1 AND missing = 0 AND companion_of IS NULL) \
                   AND missing = 0 ORDER BY id",
                params![hash],
            )?,
        ),
        ItemRef::PathId(path_id) => (
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE id = ?1 AND missing = 0 AND companion_of IS NULL",
                params![path_id],
            )?,
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE companion_of = ?1 AND missing = 0 ORDER BY id",
                params![path_id],
            )?,
        ),
    };
    let primary_sources = delivery_sources(primary_rows);
    let provisional_hash = item
        .hash
        .as_ref()
        .filter(|hash| crate::scanner::is_provisional(hash.as_str()))
        .cloned();
    let primary_expected = if provisional_hash.is_some() {
        None
    } else {
        item.hash.clone()
    };
    let mut deliveries = Vec::new();
    if let Some(first) = primary_sources.first() {
        let name = file_name(&first.abs_path)?;
        deliveries.push(DeliveryPlan {
            target: dest_dir.join(name),
            bytes: first.bytes,
            sources: primary_sources,
            expected_hash: primary_expected,
            planned_target: PlannedTarget::Missing,
            primary: true,
        });
    }

    let mut companions = std::collections::BTreeMap::<String, Vec<DeliverySource>>::new();
    for source in delivery_sources(companion_rows) {
        companions
            .entry(file_name(&source.abs_path)?.to_lowercase())
            .or_default()
            .push(source);
    }
    for sources in companions.into_values() {
        let first = &sources[0];
        deliveries.push(DeliveryPlan {
            target: dest_dir.join(file_name(&first.abs_path)?),
            bytes: first.bytes,
            expected_hash: first.content_hash.clone(),
            sources,
            planned_target: PlannedTarget::Missing,
            primary: false,
        });
    }
    let delete_targets = if mode == MoveOutMode::CopyKeepAll {
        Vec::new()
    } else {
        collect_delete_targets(conn, item.item_ref()?)?
    };
    Ok(MoveUnit {
        item,
        deliveries,
        delete_targets,
        preflight_conflicts: Vec::new(),
        provisional_hash,
    })
}

fn delivery_sources(rows: Vec<(i64, String, Option<String>, Option<i64>)>) -> Vec<DeliverySource> {
    rows.into_iter()
        .map(|(_path_id, abs_path, content_hash, indexed_bytes)| {
            let bytes =
                std::fs::symlink_metadata(crate::winpath::for_fs(Path::new(&abs_path)).as_ref())
                    .ok()
                    .filter(|metadata| metadata.file_type().is_file())
                    .map(|metadata| metadata.len())
                    .unwrap_or_else(|| indexed_bytes.unwrap_or(0).max(0) as u64);
            DeliverySource {
                abs_path,
                content_hash,
                bytes,
            }
        })
        .collect()
}

fn file_name(path: &str) -> Result<String, String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("copy has no file name: {path}"))
}

fn plan_existing_target(
    target: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
) -> PlannedTarget {
    plan_existing_target_with_after_hash(target, cancelled, progress, |_| {})
}

fn plan_existing_target_with_after_hash(
    target: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(u64, u64),
    after_hash: impl FnOnce(&Path),
) -> PlannedTarget {
    let (mut file, identity) = match crate::file_identity::open_regular_nofollow(target) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PlannedTarget::Missing
        }
        Err(_) => return PlannedTarget::Conflict,
    };
    let total = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let hash = crate::hashing::full_hash_file_cancellable(
        &mut file,
        total,
        cancelled,
        &mut |done, total| progress(done, total),
    );
    after_hash(target);
    match hash {
        Ok(verified_hash) if crate::file_identity::path_names(target, identity) => {
            PlannedTarget::Existing {
                identity,
                verified_hash,
            }
        }
        _ => PlannedTarget::Conflict,
    }
}

fn reserve_batch_target_names(units: &mut [MoveUnit]) {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for unit in units.iter() {
        for delivery in &unit.deliveries {
            let key = delivery
                .target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_lowercase();
            *counts.entry(key).or_default() += 1;
        }
    }
    for unit in units {
        for target in unit.deliveries.iter().map(|delivery| &delivery.target) {
            let key = target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_lowercase();
            if counts.get(&key).copied().unwrap_or(0) > 1 {
                unit.preflight_conflicts
                    .push(target.to_string_lossy().into_owned());
            }
        }
        unit.preflight_conflicts.sort();
        unit.preflight_conflicts.dedup();
    }
}

struct StagedOutput {
    target: std::path::PathBuf,
    staged: std::path::PathBuf,
    identity: crate::file_identity::FileIdentity,
    hash: String,
    bytes: u64,
    primary: bool,
}

enum MoveUnitProgress {
    Stream { done: u64, total: u64 },
    Attempt { bytes: u64, failed: bool },
}

enum StageResult {
    Ready(StagedOutput),
    Cancelled,
    Failed,
}

fn execute_move_unit(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    unit: &MoveUnit,
    mode: MoveOutMode,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<Option<MoveOutOutcome>, String> {
    let mut outcome = MoveOutOutcome::default();
    if !unit.preflight_conflicts.is_empty() {
        outcome.conflicts = unit.preflight_conflicts.clone();
        return Ok(Some(outcome));
    }
    for delivery in &unit.deliveries {
        if matches!(delivery.planned_target, PlannedTarget::Conflict) {
            outcome
                .conflicts
                .push(delivery.target.to_string_lossy().into_owned());
        }
    }
    if !outcome.conflicts.is_empty() {
        return Ok(Some(outcome));
    }

    let mut staged = VecDeque::<StagedOutput>::new();
    let mut existing = Vec::<ExistingTargetProof>::new();
    let mut primary_real_hash = None;
    for delivery in &unit.deliveries {
        if cancelled() {
            cleanup_staged(&mut staged);
            return Ok(None);
        }
        match &delivery.planned_target {
            PlannedTarget::Missing => match stage_delivery(conn, delivery, cancelled, on_progress)?
            {
                StageResult::Ready(output) => {
                    if output.primary {
                        primary_real_hash = Some(output.hash.clone());
                    }
                    staged.push_back(output);
                }
                StageResult::Cancelled => {
                    cleanup_staged(&mut staged);
                    return Ok(None);
                }
                StageResult::Failed => {
                    cleanup_staged(&mut staged);
                    outcome
                        .undelivered
                        .push(delivery.target.to_string_lossy().into_owned());
                    on_progress(MoveUnitProgress::Attempt {
                        bytes: delivery.bytes,
                        failed: true,
                    });
                    return Ok(Some(outcome));
                }
            },
            PlannedTarget::Existing {
                identity,
                verified_hash,
            } => {
                if delivery.expected_hash.is_none() {
                    let staged_comparison =
                        match stage_delivery(conn, delivery, cancelled, on_progress)? {
                            StageResult::Ready(output) => output,
                            StageResult::Cancelled => {
                                cleanup_staged(&mut staged);
                                return Ok(None);
                            }
                            StageResult::Failed => {
                                cleanup_staged(&mut staged);
                                outcome
                                    .undelivered
                                    .push(delivery.target.to_string_lossy().into_owned());
                                on_progress(MoveUnitProgress::Attempt {
                                    bytes: delivery.bytes,
                                    failed: true,
                                });
                                return Ok(Some(outcome));
                            }
                        };
                    let matches = staged_comparison.hash == *verified_hash;
                    if delivery.primary {
                        primary_real_hash = Some(staged_comparison.hash.clone());
                    }
                    crate::file_identity::remove_private_if_owned(
                        &staged_comparison.staged,
                        staged_comparison.identity,
                    );
                    if !matches {
                        cleanup_staged(&mut staged);
                        outcome
                            .conflicts
                            .push(delivery.target.to_string_lossy().into_owned());
                        return Ok(Some(outcome));
                    }
                } else if delivery.primary {
                    primary_real_hash = Some(verified_hash.clone());
                }
                let proof = match open_existing_target(
                    &delivery.target,
                    *identity,
                    verified_hash,
                ) {
                    Ok(Some(proof)) => proof,
                    Ok(None) | Err(_) => {
                        cleanup_staged(&mut staged);
                        outcome
                            .conflicts
                            .push(delivery.target.to_string_lossy().into_owned());
                        return Ok(Some(outcome));
                    }
                };
                existing.push(proof);
            }
            PlannedTarget::Conflict => unreachable!("conflicts return before staging"),
        }
    }

    if cancelled() {
        cleanup_staged(&mut staged);
        return Ok(None);
    }

    for proof in &mut existing {
        match proof.revalidate_cancellable(cancelled, on_progress) {
            Ok(true) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::Interrupted && cancelled() =>
            {
                cleanup_staged(&mut staged);
                return Ok(None);
            }
            Ok(false) | Err(_) => {
                cleanup_staged(&mut staged);
                outcome
                    .conflicts
                    .push(proof.path.to_string_lossy().into_owned());
                return Ok(Some(outcome));
            }
        }
    }
    if cancelled() {
        cleanup_staged(&mut staged);
        return Ok(None);
    }

    // Commit boundary: from here through the already-planned source action,
    // cancellation is intentionally ignored. A failed exclusive publication
    // keeps every source and leaves any earlier successful publication as a
    // safe extra copy that a retry will recognize.
    outcome.skipped_identical = existing.len() as u64;
    for proof in &existing {
        let bytes = unit
            .deliveries
            .iter()
            .find(|delivery| delivery.target == proof.path)
            .map(|delivery| delivery.bytes)
            .unwrap_or(0);
        on_progress(MoveUnitProgress::Attempt {
            bytes,
            failed: false,
        });
    }

    while let Some(output) = staged.pop_front() {
        let claimed = match crate::file_identity::claim_private(&output.staged, output.identity) {
            Ok(claimed) => claimed,
            Err(_) => {
                crate::file_identity::remove_private_if_owned(&output.staged, output.identity);
                cleanup_staged(&mut staged);
                outcome
                    .conflicts
                    .push(output.target.to_string_lossy().into_owned());
                return Ok(Some(outcome));
            }
        };
        match crate::fs_publish::rename_no_replace(&claimed, &output.target) {
            Ok(()) => {
                if !crate::file_identity::path_names(&output.target, output.identity) {
                    cleanup_staged(&mut staged);
                    outcome
                        .conflicts
                        .push(output.target.to_string_lossy().into_owned());
                    return Ok(Some(outcome));
                }
                if let Some(parent) = output.target.parent() {
                    if let Err(error) = crate::fs_publish::sync_directory(parent) {
                        cleanup_staged(&mut staged);
                        return Err(format!(
                            "could not durably publish {}: {error}",
                            output.target.display()
                        ));
                    }
                }
                outcome.exported = outcome.exported.saturating_add(1);
                on_progress(MoveUnitProgress::Attempt {
                    bytes: output.bytes,
                    failed: false,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                crate::file_identity::remove_private_if_owned(&claimed, output.identity);
                cleanup_staged(&mut staged);
                outcome
                    .conflicts
                    .push(output.target.to_string_lossy().into_owned());
                return Ok(Some(outcome));
            }
            Err(error) => {
                crate::file_identity::remove_private_if_owned(&claimed, output.identity);
                cleanup_staged(&mut staged);
                logging::warn(
                    "copy-out publication failed",
                    json!({ "target": output.target.to_string_lossy(), "error": { "message": error.to_string() } }),
                );
                crate::index_store::upsert_issue(
                    conn,
                    Some(output.target.to_string_lossy().as_ref()),
                    "copy-error",
                    &error.to_string(),
                )?;
                outcome
                    .undelivered
                    .push(output.target.to_string_lossy().into_owned());
                on_progress(MoveUnitProgress::Attempt {
                    bytes: output.bytes,
                    failed: true,
                });
                return Ok(Some(outcome));
            }
        }
    }

    if let (Some(stored), Some(real)) = (&unit.provisional_hash, &primary_real_hash) {
        crate::scanner::promote_identity(conn, cache, stored, real)?;
    }
    if mode != MoveOutMode::CopyKeepAll {
        let delete_mode = if mode == MoveOutMode::MoveTrashRest {
            DeleteMode::Trash
        } else {
            DeleteMode::Permanent
        };
        outcome.post_action = delete_targets(
            conn,
            app_root,
            cache,
            &unit.delete_targets,
            delete_mode,
            &mut |_, _| {},
            &mut |bytes, failed| on_progress(MoveUnitProgress::Attempt { bytes, failed }),
        )?;
    }
    Ok(Some(outcome))
}

fn stage_delivery(
    conn: &Connection,
    delivery: &DeliveryPlan,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<StageResult, String> {
    for source in &delivery.sources {
        let staged = output_stage_path(&delivery.target);
        let copied = crate::hashing::hash_while_copying_cancellable(
            Path::new(&source.abs_path),
            &staged,
            cancelled,
            &mut |done, total| on_progress(MoveUnitProgress::Stream { done, total }),
        );
        match copied {
            Ok((hash, bytes, identity)) => {
                if delivery
                    .expected_hash
                    .as_ref()
                    .is_some_and(|expected| expected != &hash)
                {
                    crate::file_identity::remove_private_if_owned(&staged, identity);
                    record_rot_issue(
                        conn,
                        &source.abs_path,
                        delivery.expected_hash.as_deref().unwrap_or_default(),
                        &hash,
                    )?;
                    continue;
                }
                return Ok(StageResult::Ready(StagedOutput {
                    target: delivery.target.clone(),
                    staged,
                    identity,
                    hash,
                    bytes,
                    primary: delivery.primary,
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted && cancelled() => {
                return Ok(StageResult::Cancelled)
            }
            Err(error) => {
                logging::warn(
                    "copy-out staging failed for one source",
                    json!({ "path": source.abs_path, "target": delivery.target.to_string_lossy(), "error": { "message": error.to_string() } }),
                );
                crate::index_store::upsert_issue(
                    conn,
                    Some(&source.abs_path),
                    "copy-error",
                    &error.to_string(),
                )?;
            }
        }
    }
    Ok(StageResult::Failed)
}

fn open_existing_target(
    target: &Path,
    planned_identity: crate::file_identity::FileIdentity,
    planned_hash: &str,
) -> Result<Option<ExistingTargetProof>, String> {
    let (file, identity) = match crate::file_identity::open_regular_nofollow(target) {
        Ok(opened) => opened,
        Err(_) => return Ok(None),
    };
    if identity != planned_identity {
        return Ok(None);
    }
    if !crate::file_identity::path_names(target, identity) {
        return Ok(None);
    }
    Ok(Some(ExistingTargetProof {
        path: target.to_path_buf(),
        identity,
        file,
        verified_hash: planned_hash.to_string(),
    }))
}

fn cleanup_staged(staged: &mut VecDeque<StagedOutput>) {
    for output in staged.drain(..) {
        crate::file_identity::remove_private_if_owned(&output.staged, output.identity);
    }
}

struct ExistingTargetProof {
    path: std::path::PathBuf,
    identity: crate::file_identity::FileIdentity,
    file: std::fs::File,
    verified_hash: String,
}

impl ExistingTargetProof {
    fn revalidate_cancellable(
        &mut self,
        cancelled: &dyn Fn() -> bool,
        on_progress: &mut dyn FnMut(MoveUnitProgress),
    ) -> std::io::Result<bool> {
        if !crate::file_identity::path_names(&self.path, self.identity) {
            return Ok(false);
        }
        let total = self.file.metadata()?.len();
        let hash = crate::hashing::full_hash_file_cancellable(
            &mut self.file,
            total,
            cancelled,
            &mut |done, total| on_progress(MoveUnitProgress::Stream { done, total }),
        )?;
        Ok(hash == self.verified_hash
            && crate::file_identity::path_names(&self.path, self.identity))
    }

    #[cfg(test)]
    fn revalidate(&mut self) -> bool {
        if !crate::file_identity::path_names(&self.path, self.identity) {
            return false;
        }
        let unchanged = crate::hashing::full_hash_file(&mut self.file)
            .is_ok_and(|hash| hash == self.verified_hash);
        unchanged && crate::file_identity::path_names(&self.path, self.identity)
    }
}

fn output_stage_path(target: &Path) -> std::path::PathBuf {
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    target.with_file_name(format!("{stem}-{}.tmp", crate::nanoid::generate()))
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
    crate::index_store::upsert_issue(
        conn,
        Some(source_path),
        "copy-verify-mismatch",
        &format!("indexed {expected} but read {actual} — bit rot or external change"),
    )?;
    Ok(())
}

fn collect4(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<(i64, String, Option<String>, Option<i64>)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

#[cfg(test)]
mod boundary_tests {
    // EXCEPTION to tests-folder conventions: these callbacks are private
    // exact-boundary seams and must not widen the shipped command API.
    use super::*;

    #[cfg(unix)]
    #[test]
    fn existing_destination_symlink_is_never_proof_of_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.jpg");
        let link = dir.path().join("link.jpg");
        std::fs::write(&real, b"identical").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(matches!(
            plan_existing_target(&link, &|| false, &mut |_, _| {}),
            PlannedTarget::Conflict
        ));
        assert_eq!(std::fs::read(&real).unwrap(), b"identical");
    }

    #[test]
    fn existing_destination_replacement_at_hash_boundary_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jpg");
        let held = dir.path().join("held.jpg");
        std::fs::write(&target, b"identical").unwrap();

        let result =
            plan_existing_target_with_after_hash(&target, &|| false, &mut |_, _| {}, |path| {
                std::fs::rename(path, &held).unwrap();
                std::fs::write(path, b"replacement").unwrap();
            });

        assert!(matches!(result, PlannedTarget::Conflict));
        assert_eq!(std::fs::read(&held).unwrap(), b"identical");
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
    }

    #[test]
    fn existing_destination_replacement_before_post_action_revokes_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jpg");
        let held = dir.path().join("held.jpg");
        std::fs::write(&target, b"identical").unwrap();

        let PlannedTarget::Existing {
            identity,
            verified_hash,
        } = plan_existing_target(&target, &|| false, &mut |_, _| {})
        else {
            panic!("existing regular target");
        };
        let mut existing = open_existing_target(&target, identity, &verified_hash)
            .unwrap()
            .expect("opened target proof");
        std::fs::rename(&target, &held).unwrap();
        std::fs::write(&target, b"replacement").unwrap();

        assert!(!existing.revalidate());
        assert_eq!(std::fs::read(&held).unwrap(), b"identical");
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
    }

    #[test]
    fn existing_destination_same_inode_rewrite_before_post_action_revokes_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jpg");
        std::fs::write(&target, b"identical").unwrap();

        let PlannedTarget::Existing {
            identity,
            verified_hash,
        } = plan_existing_target(&target, &|| false, &mut |_, _| {})
        else {
            panic!("existing regular target");
        };
        let mut existing = open_existing_target(&target, identity, &verified_hash)
            .unwrap()
            .expect("opened target proof");
        std::fs::write(&target, b"rewritten").unwrap();

        assert!(
            crate::file_identity::path_names(&target, identity),
            "the mutation keeps the same public physical file"
        );
        assert!(!existing.revalidate());
        assert_eq!(std::fs::read(&target).unwrap(), b"rewritten");
    }

    #[test]
    fn permanent_delete_replacement_keeps_the_index_row() {
        replacement_during_source_claim_keeps_the_row(DeleteMode::Permanent);
    }

    #[test]
    fn trash_delete_replacement_keeps_the_index_row() {
        replacement_during_source_claim_keeps_the_row(DeleteMode::Trash);
    }

    fn replacement_during_source_claim_keeps_the_row(mode: DeleteMode) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let app_root = dir.path().join("app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&app_root).unwrap();
        let source = root.join("photo.jpg");
        let held = root.join("held.jpg");
        std::fs::write(&source, b"original").unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let lists = crate::scanner::ScanLists {
            images: crate::extensions::IMAGE_EXTENSIONS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            videos: crate::extensions::VIDEO_EXTENSIONS
                .iter()
                .map(|value| value.to_string())
                .collect(),
            companions: crate::extensions::COMPANION_EXTENSIONS
                .iter()
                .map(|value| value.to_string())
                .collect(),
        };
        let cache = CachePaths::new(dir.path().join("cache"));
        crate::scanner::walk_root(&conn, &root, &lists).unwrap();
        crate::scanner::hash_pending(&conn, &cache).unwrap();
        let hash: String = conn
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();

        let outcome = delete_item_inner(
            &conn,
            &app_root,
            &cache,
            ItemRef::Hash(&hash),
            mode,
            |_, path| {
                std::fs::rename(path, &held).unwrap();
                std::fs::write(path, b"replacement").unwrap();
            },
        )
        .unwrap();

        assert_eq!(outcome.deleted_files, 0);
        assert_eq!(outcome.failed_files, 1);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM paths", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1, "mismatched source keeps its live index row");
        assert_eq!(std::fs::read(&source).unwrap(), b"replacement");
        assert_eq!(std::fs::read(&held).unwrap(), b"original");
    }
}
