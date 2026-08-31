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

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
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
    let targets = collect_delete_targets(conn, item)?;
    delete_targets(
        conn,
        app_root,
        cache,
        &targets,
        mode,
        &mut |_, _| {},
    )
}

fn delete_targets(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    targets: &[DeleteTarget],
    mode: DeleteMode,
    on_attempt: &mut (impl FnMut(u64, bool) + ?Sized),
) -> Result<DeleteOutcome, String> {
    let mut outcome = DeleteOutcome::default();
    let mut removed_hashes: Vec<Option<String>> = Vec::new();

    for target in targets {
        let file = Path::new(&target.abs_path);
        let result = match mode {
            DeleteMode::Trash => trash::trash_file(file, app_root, target.content_hash.as_deref())
                .map(|_| ()),
            DeleteMode::Permanent => permanently_delete_file(file),
        };

        match result {
            Ok(()) => {
                outcome.deleted_files += 1;
                // A partially completed Move may deliver a main file before a
                // companion output. Detach surviving companions so the main
                // row can leave without discarding or misrepresenting them.
                conn.execute(
                    "UPDATE paths SET companion_of = NULL WHERE companion_of = ?1",
                    [target.path_id],
                )
                .map_err(|e| e.to_string())?;
                let current_hash = conn
                    .query_row(
                        "SELECT content_hash FROM paths WHERE id = ?1",
                        [target.path_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .or_else(|| target.content_hash.clone());
                conn.execute("DELETE FROM evidence WHERE path_id = ?1", [target.path_id])
                    .map_err(|e| e.to_string())?;
                conn.execute("DELETE FROM paths WHERE id = ?1", [target.path_id])
                    .map_err(|e| e.to_string())?;
                outcome.removed_rows += 1;
                removed_hashes.push(current_hash);
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
        // Only live main copies keep a logical content identity alive. Counting missing rows too
        // meant one copy on an absent drive pinned the contents row and every
        // cache entry for that hash forever — a leak that accumulates across a
        // cull session and that no sweep reclaims, since startup_sweep only
        // drops cache whose hash is absent from contents.
        let live: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM paths WHERE content_hash = ?1 AND missing = 0 \
                   AND companion_of IS NULL",
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
                 WHERE content_hash = ?1 AND missing = 0 \
                   AND companion_of IS NULL ORDER BY id",
                params![hash],
            )?,
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE companion_of IN (\
                   SELECT id FROM paths WHERE content_hash = ?1 AND missing = 0 \
                     AND companion_of IS NULL\
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
/// changes. Cancellation is observed while planning and between physical file
/// actions; filesystem failures remain per-file Issues.
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
        let mut outcome = DeleteOutcome::default();
        let mut completed_unit = true;
        for target in &unit.targets {
            if cancelled() {
                batch.cancelled = true;
                completed_unit = false;
                break;
            }
            let step = delete_targets(
                conn,
                app_root,
                cache,
                std::slice::from_ref(target),
                mode,
                &mut |bytes, failed| {
                    files_done = files_done.saturating_add(1);
                    bytes_done = bytes_done.saturating_add(bytes);
                    on_progress(DeleteBatchProgress::Deleting {
                        items_done,
                        items_total,
                        files_done,
                        files_total: plan.files_total,
                        bytes_done,
                        bytes_total: plan.bytes_total,
                        failures: batch.failed_files
                            + outcome.failed_files
                            + u64::from(failed),
                    });
                },
            );
            let step = match step {
                Ok(step) => step,
                Err(error) => {
                    logging::warn(
                        "delete batch stopped inside one logical item",
                        json!({ "error": { "message": error } }),
                    );
                    batch.error = Some(error);
                    completed_unit = false;
                    break;
                }
            };
            outcome.deleted_files = outcome.deleted_files.saturating_add(step.deleted_files);
            outcome.failed_files = outcome.failed_files.saturating_add(step.failed_files);
            outcome.removed_rows = outcome.removed_rows.saturating_add(step.removed_rows);
        }
        batch.deleted_files = batch.deleted_files.saturating_add(outcome.deleted_files);
        batch.failed_files = batch.failed_files.saturating_add(outcome.failed_files);
        batch.removed_rows = batch.removed_rows.saturating_add(outcome.removed_rows);
        if !completed_unit {
            break;
        }
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

fn permanently_delete_file(file: &Path) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(crate::winpath::for_fs(file).as_ref()) {
        Ok(metadata) => metadata,
        // Already gone from disk: the index intent still applies; the walk
        // would have marked it missing anyway.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("not a regular file: {}", file.display()));
    }
    std::fs::remove_file(crate::winpath::for_fs(file).as_ref()).map_err(|error| error.to_string())
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

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DestinationConflictPolicy {
    Rename,
    Overwrite,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DestinationRenameStyle {
    SpaceNumber,
    ParenthesizedNumber,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DestinationConflict {
    pub path: String,
    pub incoming_bytes: u64,
    pub existing_bytes: Option<u64>,
    pub within_selection: bool,
    pub preserved_paths: Vec<String>,
}

#[derive(Clone, Serialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MoveOutOutcome {
    pub exported: u64,
    pub skipped_identical: u64,
    /// Conflicts that appeared after the reviewed plan was accepted. Expected
    /// conflicts are resolved before execution and never enter this result.
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
    pub plan_token: Option<String>,
    pub requires_conflict_choice: bool,
    pub plan_changed: bool,
    pub overwrite_allowed: bool,
    pub reviewed_conflicts: Vec<DestinationConflict>,
}

#[derive(Clone, Debug)]
struct DeliverySource {
    path_id: i64,
    abs_path: String,
    content_hash: Option<String>,
    bytes: u64,
}

#[derive(Clone, Debug)]
struct DeliveryPlan {
    target: std::path::PathBuf,
    sources: Vec<DeliverySource>,
    bytes: u64,
    primary: bool,
    replacement_family: Vec<ReviewedDestinationFile>,
    rename_required: bool,
}

#[derive(Clone, Debug)]
struct MoveUnit {
    item: ItemIdentity,
    deliveries: Vec<DeliveryPlan>,
    provisional_hash: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct MovePlan {
    units: Vec<MoveUnit>,
    files_total: u64,
    bytes_total: u64,
}

#[derive(Clone, Debug)]
struct ReviewedDestinationFile {
    path: std::path::PathBuf,
    bytes: u64,
    hash: String,
}

#[derive(Clone, Debug)]
enum DestinationObservation {
    Absent,
    Regular { bytes: u64, hash: String },
    Other { bytes: u64 },
}

#[derive(Clone, Debug)]
struct ReviewedDestination {
    path: std::path::PathBuf,
    state: DestinationObservation,
}

#[derive(Clone, Debug)]
struct DestinationReview {
    conflicts: Vec<DestinationConflict>,
    observations: Vec<ReviewedDestination>,
    overwrite_allowed: bool,
}

impl Default for DestinationReview {
    fn default() -> Self {
        Self {
            conflicts: Vec::new(),
            observations: Vec::new(),
            overwrite_allowed: true,
        }
    }
}

/// Moves or copies one logical item out to `dest_dir`: the primary plus one
/// instance of each distinct companion. Each output is copied privately from
/// the first currently readable source, read back, and published without
/// overwrite. A verified output releases only the sources represented by that
/// output; later output failures do not roll it back.
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
    if batch.requires_conflict_choice {
        return Err("destination conflicts require a reviewed batch decision".to_string());
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
/// publication. Cancellation is honored during private streaming and between
/// bounded output-publication and physical source actions; completed steps are
/// reported and never rolled back.
pub fn move_batch(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    dest_dir: &Path,
    mode: MoveOutMode,
    cancelled: &dyn Fn() -> bool,
    on_progress: impl FnMut(MoveBatchProgress),
) -> Result<MoveBatchOutcome, String> {
    move_batch_reviewed(
        conn,
        app_root,
        cache,
        items,
        dest_dir,
        mode,
        None,
        None,
        DestinationRenameStyle::SpaceNumber,
        cancelled,
        on_progress,
    )
}

pub fn move_batch_reviewed(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    dest_dir: &Path,
    mode: MoveOutMode,
    conflict_policy: Option<DestinationConflictPolicy>,
    expected_plan_token: Option<&str>,
    rename_style: DestinationRenameStyle,
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
        let unit = collect_move_unit(conn, item, dest_dir)?;
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
                .saturating_add(
                    unit.deliveries
                        .iter()
                        .map(|delivery| delivery.sources.len() as u64)
                        .sum::<u64>(),
                );
            plan.bytes_total = plan.bytes_total.saturating_add(
                unit.deliveries
                    .iter()
                    .flat_map(|delivery| &delivery.sources)
                    .map(|source| source.bytes)
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
    let review = match review_destination_conflicts(&mut plan, cancelled) {
        Ok(review) => review,
        Err(error) if error == crate::scanner::CANCELLED => {
            return Ok(MoveBatchOutcome {
                cancelled: true,
                files_total: plan.files_total,
                bytes_total: plan.bytes_total,
                ..MoveBatchOutcome::default()
            });
        }
        Err(error) => return Err(error),
    };
    let plan_token = move_plan_token(&plan, mode, &review);
    if expected_plan_token.is_some_and(|expected| expected != plan_token) {
        return Ok(MoveBatchOutcome {
            plan_token: Some(plan_token),
            requires_conflict_choice: !review.conflicts.is_empty(),
            plan_changed: true,
            overwrite_allowed: review.overwrite_allowed,
            reviewed_conflicts: review.conflicts,
            files_total: plan.files_total,
            bytes_total: plan.bytes_total,
            ..MoveBatchOutcome::default()
        });
    }
    if !review.conflicts.is_empty() && conflict_policy.is_none() {
        return Ok(MoveBatchOutcome {
            plan_token: Some(plan_token),
            requires_conflict_choice: true,
            overwrite_allowed: review.overwrite_allowed,
            reviewed_conflicts: review.conflicts,
            files_total: plan.files_total,
            bytes_total: plan.bytes_total,
            ..MoveBatchOutcome::default()
        });
    }
    if !review.conflicts.is_empty()
        && conflict_policy.is_some()
        && expected_plan_token.is_none()
    {
        return Err("destination conflict policy requires the reviewed plan token".to_string());
    }
    if conflict_policy == Some(DestinationConflictPolicy::Overwrite)
        && !review.overwrite_allowed
    {
        return Err(
            "overwrite cannot preserve every selected file in this conflict set; use Rename"
                .to_string(),
        );
    }
    if conflict_policy == Some(DestinationConflictPolicy::Rename) {
        apply_conflict_renames(&mut plan, rename_style)?;
    }
    let mut batch = MoveBatchOutcome {
        files_total: plan.files_total,
        bytes_total: plan.bytes_total,
        plan_token: Some(plan_token),
        overwrite_allowed: review.overwrite_allowed,
        reviewed_conflicts: review.conflicts,
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
            conflict_policy,
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
        let (outcome, unit_cancelled) = match execution {
            Ok(MoveUnitResult::Completed(outcome)) => (outcome, false),
            Ok(MoveUnitResult::Cancelled(outcome)) => (outcome, true),
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
        let has_effect = outcome.exported > 0
            || outcome.skipped_identical > 0
            || !outcome.conflicts.is_empty()
            || !outcome.undelivered.is_empty()
            || outcome.post_action.deleted_files > 0
            || outcome.post_action.failed_files > 0;
        let stopped_by_conflict = !outcome.conflicts.is_empty();
        if !unit_cancelled || has_effect {
            batch.items.push(MoveBatchItemResult {
                item: unit.item,
                outcome,
            });
        }
        if unit_cancelled {
            batch.cancelled = true;
            break;
        }
        if stopped_by_conflict {
            break;
        }
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

fn move_plan_token(
    plan: &MovePlan,
    mode: MoveOutMode,
    review: &DestinationReview,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(match mode {
        MoveOutMode::MoveTrashRest => b"move-trash-rest",
        MoveOutMode::MoveDeleteRest => b"move-delete-rest",
        MoveOutMode::CopyKeepAll => b"copy",
    });
    let mut field = |value: &[u8]| {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value);
    };
    for unit in &plan.units {
        match &unit.item.hash {
            Some(hash) => field(hash.as_bytes()),
            None => field(&unit.item.path_id.unwrap_or_default().to_le_bytes()),
        }
        for delivery in &unit.deliveries {
            field(delivery.target.as_os_str().as_encoded_bytes());
            field(&[u8::from(delivery.primary)]);
            for source in &delivery.sources {
                field(&source.path_id.to_le_bytes());
                field(source.abs_path.as_bytes());
            }
        }
    }
    for observation in &review.observations {
        field(observation.path.as_os_str().as_encoded_bytes());
        match &observation.state {
            DestinationObservation::Absent => field(b"absent"),
            DestinationObservation::Regular { bytes, hash } => {
                field(b"regular");
                field(&bytes.to_le_bytes());
                field(hash.as_bytes());
            }
            DestinationObservation::Other { bytes } => {
                field(b"other");
                field(&bytes.to_le_bytes());
            }
        }
    }
    for delivery in plan.units.iter().flat_map(|unit| &unit.deliveries) {
        for member in &delivery.replacement_family {
            field(member.path.as_os_str().as_encoded_bytes());
            field(&member.bytes.to_le_bytes());
            field(member.hash.as_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
}

fn review_destination_conflicts(
    plan: &mut MovePlan,
    cancelled: &dyn Fn() -> bool,
) -> Result<DestinationReview, String> {
    let mut review = DestinationReview::default();
    let mut claimed = HashSet::new();
    for delivery in plan
        .units
        .iter_mut()
        .flat_map(|unit| &mut unit.deliveries)
    {
        if cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        if !claimed.insert(delivery.target.clone()) {
            delivery.rename_required = true;
            review.conflicts.push(DestinationConflict {
                path: delivery.target.to_string_lossy().into_owned(),
                incoming_bytes: delivery.bytes,
                existing_bytes: None,
                within_selection: true,
                preserved_paths: Vec::new(),
            });
            review.overwrite_allowed = false;
            continue;
        }
        let observation = observe_destination(&delivery.target, cancelled)?;
        review.observations.push(ReviewedDestination {
            path: delivery.target.clone(),
            state: observation.clone(),
        });
        match observation {
            DestinationObservation::Absent => {}
            DestinationObservation::Other { bytes } => {
                delivery.rename_required = true;
                review.overwrite_allowed = false;
                review.conflicts.push(DestinationConflict {
                    path: delivery.target.to_string_lossy().into_owned(),
                    incoming_bytes: delivery.bytes,
                    existing_bytes: Some(bytes),
                    within_selection: false,
                    preserved_paths: vec![delivery.target.to_string_lossy().into_owned()],
                });
            }
            DestinationObservation::Regular { bytes, hash } => {
                if delivery_matches_hash(delivery, &hash, bytes, cancelled)? {
                    continue;
                }
                delivery.rename_required = true;
                let (family, preserved_paths, replaceable) = reviewed_replacement_family(
                    &delivery.target,
                    delivery.primary,
                    bytes,
                    hash,
                    cancelled,
                )?;
                delivery.replacement_family = family;
                review.overwrite_allowed &= replaceable;
                review.conflicts.push(DestinationConflict {
                    path: delivery.target.to_string_lossy().into_owned(),
                    incoming_bytes: delivery.bytes,
                    existing_bytes: Some(bytes),
                    within_selection: false,
                    preserved_paths,
                });
            }
        }
    }
    Ok(review)
}

fn observe_destination(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<DestinationObservation, String> {
    let metadata = match std::fs::symlink_metadata(crate::winpath::for_fs(path).as_ref()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DestinationObservation::Absent)
        }
        Err(error) => {
            return Err(format!(
                "could not inspect destination {}: {error}",
                path.display()
            ))
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Ok(DestinationObservation::Other {
            bytes: metadata.len(),
        });
    }
    let (mut file, _) = crate::file_identity::open_regular_nofollow(path).map_err(|error| {
        format!(
            "could not inspect destination {}: {error}",
            path.display()
        )
    })?;
    let bytes = file.metadata().map_err(|error| error.to_string())?.len();
    let hash = crate::hashing::full_hash_file_cancellable(
        &mut file,
        bytes,
        cancelled,
        &mut |_, _| {},
    )
    .map_err(|error| error.to_string())?;
    Ok(DestinationObservation::Regular { bytes, hash })
}

fn delivery_matches_hash(
    delivery: &DeliveryPlan,
    destination_hash: &str,
    destination_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<bool, String> {
    for source in &delivery.sources {
        let (mut source_file, _) = match crate::file_identity::open_regular_nofollow(Path::new(
            &source.abs_path,
        )) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let source_bytes = source_file.metadata().map_err(|error| error.to_string())?.len();
        if source_bytes != destination_bytes {
            return Ok(false);
        }
        let source_hash = crate::hashing::full_hash_file_cancellable(
            &mut source_file,
            source_bytes,
            cancelled,
            &mut |_, _| {},
        )
        .map_err(|error| error.to_string())?;
        return Ok(source_hash == destination_hash);
    }
    Ok(false)
}

fn reviewed_replacement_family(
    target: &Path,
    include_companions: bool,
    target_bytes: u64,
    target_hash: String,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<ReviewedDestinationFile>, Vec<String>, bool), String> {
    let mut paths = vec![target.to_path_buf()];
    if include_companions {
        let parent = target
            .parent()
            .ok_or_else(|| format!("destination has no parent: {}", target.display()))?;
        let stem = target
            .file_stem()
            .ok_or_else(|| format!("destination has no file name: {}", target.display()))?;
        for entry in std::fs::read_dir(crate::winpath::for_fs(parent).as_ref())
            .map_err(|error| format!("could not inspect destination companions: {error}"))?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path == target || path.file_stem() != Some(stem) {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if crate::extensions::COMPANION_EXTENSIONS.contains(&extension.as_str()) {
                paths.push(path);
            }
        }
    }
    paths.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    let mut reviewed = Vec::new();
    let mut presented = Vec::new();
    let mut replaceable = true;
    for path in paths {
        presented.push(path.to_string_lossy().into_owned());
        if path == target {
            reviewed.push(ReviewedDestinationFile {
                path,
                bytes: target_bytes,
                hash: target_hash.clone(),
            });
            continue;
        }
        match observe_destination(&path, cancelled)? {
            DestinationObservation::Regular { bytes, hash } => {
                reviewed.push(ReviewedDestinationFile { path, bytes, hash });
            }
            DestinationObservation::Other { .. } => replaceable = false,
            DestinationObservation::Absent => {}
        }
    }
    Ok((reviewed, presented, replaceable))
}

fn apply_conflict_renames(
    plan: &mut MovePlan,
    style: DestinationRenameStyle,
) -> Result<(), String> {
    let needs_rename = plan
        .units
        .iter()
        .map(|unit| {
            unit.deliveries
                .iter()
                .any(|delivery| delivery.rename_required)
        })
        .collect::<Vec<_>>();
    let mut reserved = HashSet::new();
    for (unit, rename) in plan.units.iter().zip(&needs_rename) {
        if !rename {
            reserved.extend(unit.deliveries.iter().map(|delivery| delivery.target.clone()));
        }
    }
    for (unit, rename) in plan.units.iter_mut().zip(needs_rename) {
        if !rename {
            continue;
        }
        let chosen = (2..=1_000_000u32).find_map(|number| {
            let candidates = unit
                .deliveries
                .iter()
                .map(|delivery| renamed_target(&delivery.target, number, style))
                .collect::<Option<Vec<_>>>()?;
            let available = candidates.iter().all(|candidate| {
                !reserved.contains(candidate)
                    && std::fs::symlink_metadata(crate::winpath::for_fs(candidate).as_ref())
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            });
            available.then_some(candidates)
        });
        let chosen = chosen.ok_or_else(|| {
            "could not find an available destination name for the selected family".to_string()
        })?;
        for (delivery, target) in unit.deliveries.iter_mut().zip(chosen) {
            delivery.target = target.clone();
            delivery.replacement_family.clear();
            delivery.rename_required = false;
            reserved.insert(target);
        }
    }
    Ok(())
}

fn renamed_target(
    target: &Path,
    number: u32,
    style: DestinationRenameStyle,
) -> Option<std::path::PathBuf> {
    let stem = target.file_stem()?.to_str()?;
    let suffix = match style {
        DestinationRenameStyle::SpaceNumber => format!(" {number}"),
        DestinationRenameStyle::ParenthesizedNumber => format!(" ({number})"),
    };
    let mut name = format!("{stem}{suffix}");
    if let Some(extension) = target.extension().and_then(|value| value.to_str()) {
        name.push('.');
        name.push_str(extension);
    }
    Some(target.with_file_name(name))
}

fn collect_move_unit(
    conn: &Connection,
    item: ItemIdentity,
    dest_dir: &Path,
) -> Result<MoveUnit, String> {
    let (primary_rows, companion_rows): (Vec<_>, Vec<_>) = match item.item_ref()? {
        ItemRef::Hash(hash) => (
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 AND companion_of IS NULL \
                 ORDER BY resolved_utc_ms IS NULL, resolved_utc_ms, \
                          abs_path COLLATE onecopy_nocase, abs_path",
                params![hash],
            )?,
            collect4(
                conn,
                "SELECT comp.id, comp.abs_path, comp.content_hash, comp.size \
                 FROM paths comp JOIN paths pri ON comp.companion_of = pri.id \
                 WHERE pri.content_hash = ?1 AND pri.missing = 0 \
                   AND pri.companion_of IS NULL AND comp.missing = 0 \
                 ORDER BY pri.resolved_utc_ms IS NULL, pri.resolved_utc_ms, \
                          pri.abs_path COLLATE onecopy_nocase, pri.abs_path, \
                          comp.abs_path COLLATE onecopy_nocase, comp.abs_path",
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
    let mut deliveries = Vec::new();
    if let Some(first) = primary_sources.first() {
        let name = file_name(&first.abs_path)?;
        deliveries.push(DeliveryPlan {
            target: dest_dir.join(name),
            bytes: first.bytes,
            sources: primary_sources,
            primary: true,
            replacement_family: Vec::new(),
            rename_required: false,
        });
    }

    let mut companions = Vec::<(String, Vec<DeliverySource>)>::new();
    for source in delivery_sources(companion_rows) {
        let name = file_name(&source.abs_path)?;
        if let Some((_, sources)) = companions.iter_mut().find(|(existing, _)| *existing == name) {
            sources.push(source);
        } else {
            companions.push((name, vec![source]));
        }
    }
    for (name, sources) in companions {
        let first = &sources[0];
        deliveries.push(DeliveryPlan {
            target: dest_dir.join(name),
            bytes: first.bytes,
            sources,
            primary: false,
            replacement_family: Vec::new(),
            rename_required: false,
        });
    }
    Ok(MoveUnit {
        item,
        deliveries,
        provisional_hash,
    })
}

fn delivery_sources(rows: Vec<(i64, String, Option<String>, Option<i64>)>) -> Vec<DeliverySource> {
    rows.into_iter()
        .map(|(path_id, abs_path, content_hash, indexed_bytes)| {
            let bytes =
                std::fs::symlink_metadata(crate::winpath::for_fs(Path::new(&abs_path)).as_ref())
                    .ok()
                    .filter(|metadata| metadata.file_type().is_file())
                    .map(|metadata| metadata.len())
                    .unwrap_or_else(|| indexed_bytes.unwrap_or(0).max(0) as u64);
            DeliverySource {
                path_id,
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

enum MoveUnitResult {
    Completed(MoveOutOutcome),
    Cancelled(MoveOutOutcome),
}

struct NaturalTarget {
    identity: crate::file_identity::FileIdentity,
    delivered: bool,
}

fn execute_move_unit(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    unit: &MoveUnit,
    mode: MoveOutMode,
    conflict_policy: Option<DestinationConflictPolicy>,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<MoveUnitResult, String> {
    let mut outcome = MoveOutOutcome::default();
    let mut natural_targets = Vec::<NaturalTarget>::new();
    let mut primary_promoted = false;
    for delivery in &unit.deliveries {
        if cancelled() {
            return Ok(MoveUnitResult::Cancelled(outcome));
        }
        let output = match stage_delivery(conn, delivery, cancelled, on_progress)? {
            StageResult::Ready(output) => output,
            StageResult::Cancelled => return Ok(MoveUnitResult::Cancelled(outcome)),
            StageResult::Failed => {
                outcome
                    .undelivered
                    .push(delivery.target.to_string_lossy().into_owned());
                on_progress(MoveUnitProgress::Attempt {
                    bytes: delivery.bytes,
                    failed: true,
                });
                continue;
            }
        };
        if output.primary {
            if !primary_promoted {
                if let Some(stored) = &unit.provisional_hash {
                    crate::scanner::promote_identity(conn, cache, stored, &output.hash)?;
                }
                primary_promoted = true;
            }
        }

        // From publication through this output group's source cleanup,
        // cancellation is deliberately deferred. The next output is the next
        // safe boundary.
        let claimed = match crate::file_identity::claim_private(&output.staged, output.identity) {
            Ok(claimed) => claimed,
            Err(error) => {
                crate::file_identity::remove_private_if_owned(&output.staged, output.identity);
                return Err(format!("private output changed before publication: {error}"));
            }
        };
        let delivered = match publish_claimed(conn, &claimed, &output.target, output.identity) {
            Ok(true) => {
                outcome.exported = outcome.exported.saturating_add(1);
                natural_targets.push(NaturalTarget {
                    identity: output.identity,
                    delivered: true,
                });
                true
            }
            Ok(false) => {
                let existing = crate::file_identity::open_regular_nofollow(&output.target);
                let delivered = match existing {
                    Ok((mut file, identity)) => {
                        if let Some(previous) = natural_targets
                            .iter()
                            .find(|target| target.identity == identity)
                        {
                            previous.delivered
                        } else {
                            let total = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                            let hash = crate::hashing::full_hash_file_cancellable(
                                &mut file,
                                total,
                                cancelled,
                                &mut |done, total| {
                                    on_progress(MoveUnitProgress::Stream { done, total })
                                },
                            );
                            if hash.as_ref().is_err_and(|error| {
                                error.kind() == std::io::ErrorKind::Interrupted && cancelled()
                            }) {
                                return Ok(MoveUnitResult::Cancelled(outcome));
                            }
                            let same = hash.is_ok_and(|hash| hash == output.hash);
                            natural_targets.push(NaturalTarget {
                                identity,
                                delivered: same,
                            });
                            if same {
                                crate::file_identity::remove_private_if_owned(
                                    &claimed,
                                    output.identity,
                                );
                                outcome.skipped_identical =
                                    outcome.skipped_identical.saturating_add(1);
                                true
                            } else if conflict_policy
                                == Some(DestinationConflictPolicy::Overwrite)
                            {
                                drop(file);
                                preserve_reviewed_destination_family(
                                    conn,
                                    &delivery.replacement_family,
                                    app_root,
                                )?;
                                if !publish_claimed(
                                    conn,
                                    &claimed,
                                    &output.target,
                                    output.identity,
                                )?
                                {
                                    return Err(format!(
                                        "a new destination conflict appeared at {}",
                                        output.target.display()
                                    ));
                                }
                                outcome.exported = outcome.exported.saturating_add(1);
                                true
                            } else {
                                crate::file_identity::remove_private_if_owned(
                                    &claimed,
                                    output.identity,
                                );
                                false
                            }
                        }
                    }
                    Err(_) => {
                        crate::file_identity::remove_private_if_owned(&claimed, output.identity);
                        false
                    }
                };
                if !delivered {
                    outcome
                        .conflicts
                        .push(output.target.to_string_lossy().into_owned());
                }
                delivered
            }
            Err(error) => {
                crate::file_identity::remove_private_if_owned(&claimed, output.identity);
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
                false
            }
        };
        on_progress(MoveUnitProgress::Attempt {
            bytes: output.bytes,
            failed: !delivered,
        });

        if delivered && mode != MoveOutMode::CopyKeepAll {
            let targets = delivery
                .sources
                .iter()
                .map(|source| DeleteTarget {
                    path_id: source.path_id,
                    abs_path: source.abs_path.clone(),
                    content_hash: source.content_hash.clone(),
                    bytes: source.bytes,
                })
                .collect::<Vec<_>>();
            let delete_mode = if mode == MoveOutMode::MoveTrashRest {
                DeleteMode::Trash
            } else {
                DeleteMode::Permanent
            };
            for target in &targets {
                if cancelled() {
                    return Ok(MoveUnitResult::Cancelled(outcome));
                }
                let cleanup = delete_targets(
                    conn,
                    app_root,
                    cache,
                    std::slice::from_ref(target),
                    delete_mode,
                    &mut |bytes, failed| {
                        on_progress(MoveUnitProgress::Attempt { bytes, failed })
                    },
                )?;
                outcome.post_action.deleted_files = outcome
                    .post_action
                    .deleted_files
                    .saturating_add(cleanup.deleted_files);
                outcome.post_action.failed_files = outcome
                    .post_action
                    .failed_files
                    .saturating_add(cleanup.failed_files);
                outcome.post_action.removed_rows = outcome
                    .post_action
                    .removed_rows
                    .saturating_add(cleanup.removed_rows);
            }
        }
        if !outcome.conflicts.is_empty() {
            break;
        }
    }

    Ok(MoveUnitResult::Completed(outcome))
}

fn publish_claimed(
    conn: &Connection,
    claimed: &Path,
    target: &Path,
    identity: crate::file_identity::FileIdentity,
) -> Result<bool, String> {
    match crate::fs_publish::rename_no_replace(claimed, target) {
        Ok(()) => {
            if !crate::file_identity::path_names(target, identity) {
                return Err(format!(
                    "published output was replaced before completion: {}",
                    target.display()
                ));
            }
            if let Some(parent) = target.parent() {
                if let Err(error) = crate::fs_publish::sync_directory(parent) {
                    crate::index_store::upsert_issue(
                        conn,
                        Some(target.to_string_lossy().as_ref()),
                        "copy-error",
                        &format!(
                            "output was published but its directory could not be synced: {error}"
                        ),
                    )?;
                    return Err(format!(
                        "could not durably publish {}: {error}",
                        target.display()
                    ));
                }
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn preserve_reviewed_destination_family(
    conn: &Connection,
    family: &[ReviewedDestinationFile],
    app_root: &Path,
) -> Result<(), String> {
    if family.is_empty() {
        return Err("a new unreviewed destination conflict appeared".to_string());
    }
    for member in family {
        match observe_destination(&member.path, &|| false)? {
            DestinationObservation::Regular { bytes, hash }
                if bytes == member.bytes && hash == member.hash => {}
            _ => {
                return Err(format!(
                    "the reviewed destination changed before replacement: {}",
                    member.path.display()
                ))
            }
        }
    }
    for member in family {
        crate::trash::trash_file(&member.path, app_root, None).map_err(|error| {
            let message = format!(
                "could not preserve the existing destination {} in OneCopy Trash: {error}",
                member.path.display()
            );
            let _ = crate::index_store::upsert_issue(
                conn,
                Some(member.path.to_string_lossy().as_ref()),
                "copy-error",
                &message,
            );
            message
        })?;
    }
    Ok(())
}

fn stage_delivery(
    conn: &Connection,
    delivery: &DeliveryPlan,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<StageResult, String> {
    for source in &delivery.sources {
        let staged = output_stage_path(&delivery.target)?;
        let copied = crate::hashing::hash_while_copying_cancellable_detailed(
            Path::new(&source.abs_path),
            &staged,
            cancelled,
            &mut |done, total| on_progress(MoveUnitProgress::Stream { done, total }),
        );
        match copied {
            Ok((hash, bytes, identity)) => {
                return Ok(StageResult::Ready(StagedOutput {
                    target: delivery.target.clone(),
                    staged,
                    identity,
                    hash,
                    bytes,
                    primary: delivery.primary,
                }));
            }
            Err(crate::hashing::CopyFailure::Cancelled) => return Ok(StageResult::Cancelled),
            Err(crate::hashing::CopyFailure::Source(error)) => {
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
            Err(crate::hashing::CopyFailure::Destination(error)) => {
                let message = format!(
                    "destination could not accept {}: {error}",
                    delivery.target.display()
                );
                logging::warn(
                    "copy-out destination failed",
                    json!({ "target": delivery.target.to_string_lossy(), "error": { "message": error.to_string() } }),
                );
                crate::index_store::upsert_issue(
                    conn,
                    Some(delivery.target.to_string_lossy().as_ref()),
                    "copy-error",
                    &message,
                )?;
                return Err(message);
            }
        }
    }
    Ok(StageResult::Failed)
}

fn output_stage_path(target: &Path) -> Result<std::path::PathBuf, String> {
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    Ok(target.with_file_name(format!(
        "{stem}-{}.tmp",
        crate::nanoid::generate()?
    )))
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
