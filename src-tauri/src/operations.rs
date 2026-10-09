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

use crate::file_names::{self, FolderNames, RenameStyle};
use crate::logging;
use crate::preview::{self, CachePaths};
use crate::trash;
use crate::volume_io;

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
    /// Files not deleted, including `unknown_files`.
    pub failed_files: u64,
    pub removed_rows: u64,
    /// Files whose removal was given up on while their volume was not
    /// responding: each may be gone or still in place. Their rows stay until
    /// the next source check settles them.
    pub unknown_files: u64,
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

    /// The item's boundary key (see `indexed_file::item_key`).
    pub fn key(&self) -> Result<String, String> {
        match self.item_ref()? {
            ItemRef::Hash(hash) => Ok(hash.to_string()),
            ItemRef::PathId(path_id) => Ok(crate::indexed_file::item_key(None, path_id)),
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
    pub unknown_files: u64,
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
    pub unknown_files: u64,
    pub files_total: u64,
    pub bytes_total: u64,
    pub items_started: u64,
}

#[derive(Clone, Debug)]
struct DeleteTarget {
    path_id: i64,
    abs_path: String,
    content_hash: Option<String>,
    bytes: u64,
    /// The frozen configured root for recoverable deletion. A copy whose owner
    /// cannot be established (its drive is away, or its source was removed)
    /// fails as that one file; it never stops planning.
    owning_root: Result<std::path::PathBuf, String>,
    /// The logical item's key and the file's role in it, for the
    /// deleted-file record.
    item: Option<String>,
    role: trash::TrashRole,
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

type PhysicalRow = (i64, String, Option<String>, Option<i64>);

/// The physical main copies and locally paired companions an accepted
/// logical-item batch covers. It is captured when the operation is accepted,
/// before admission may wait for background work, and planning after
/// admission acts only on captured files that still belong to their item at
/// the same path: discovery or reconciliation during the wait can narrow the
/// batch but never broaden it.
#[derive(Clone, Debug, Default)]
pub struct AcceptedFiles {
    files: HashSet<(i64, String)>,
}

impl AcceptedFiles {
    pub fn capture(conn: &Connection, items: &[ItemIdentity]) -> Result<Self, String> {
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Deferred)
                .map_err(|error| error.to_string())?;
        let mut files = HashSet::new();
        for item in items {
            let (mains, companions) = item_physical_rows(&transaction, item.item_ref()?)?;
            files.extend(
                mains
                    .into_iter()
                    .chain(companions)
                    .map(|(path_id, abs_path, _, _)| (path_id, abs_path)),
            );
        }
        crate::records::commit(transaction).map_err(|error| error.to_string())?;
        Ok(Self { files })
    }

    /// Inspect the whole accepted set before any destructive effect. A later
    /// disconnect still follows the operation's explicit partial-result contract.
    fn preflight(&self, cancelled: &dyn Fn() -> bool) -> Result<bool, String> {
        let mut paths = self.abs_paths().collect::<Vec<_>>();
        paths.sort_unstable();
        for path in paths {
            if cancelled() { return Ok(false); }
            crate::file_identity::open_regular_nofollow(Path::new(path))
                .map_err(|error| format!("Required source is unavailable: {path}: {error}"))?;
        }
        Ok(true)
    }

    fn retain_accepted(&self, rows: &mut Vec<PhysicalRow>) {
        rows.retain(|(path_id, abs_path, _, _)| self.accepts(*path_id, abs_path));
    }

    /// The absolute paths this accepted batch covers, so admission can scope
    /// the volume-substitution gate to only the configured roots the batch
    /// actually touches (R3-07, R1-14): a source that failed verification
    /// never blocks a batch that never reads or writes under it.
    pub fn abs_paths(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|(_, abs_path)| abs_path.as_str())
    }

    /// Test-only construction from bare paths, without a database: the
    /// volume-scoping decision only reads `abs_paths()`, so mutation_runtime's
    /// unit tests do not need a real index to exercise it.
    #[cfg(test)]
    pub(crate) fn for_test_paths(paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            files: paths
                .into_iter()
                .enumerate()
                .map(|(id, path)| (id as i64, path.into()))
                .collect(),
        }
    }

    fn accepts(&self, path_id: i64, abs_path: &str) -> bool {
        self.files.contains(&(path_id, abs_path.to_string()))
    }
}

/// An item's live main copies and the live companions paired with them.
fn item_physical_rows(
    conn: &Connection,
    item: ItemRef<'_>,
) -> Result<(Vec<PhysicalRow>, Vec<PhysicalRow>), String> {
    // The companion query stays parameterized and constant-size even if one
    // logical item has an extreme number of copies.
    Ok(match item {
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
    })
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
    let roots = crate::storage::configured_file_roots(app_root)?;
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
    let accepted = AcceptedFiles::capture(conn, std::slice::from_ref(&identity))?;
    accepted.preflight(&|| false)?;
    let targets = collect_delete_targets(conn, item, &roots, &accepted)?;
    let operation = crate::nanoid::generate()?;
    delete_targets(
        conn,
        cache,
        &targets,
        mode,
        &trash::TrashContext::new(trash::TrashKind::Delete, &operation),
        &mut |_, _| {},
    )
}

/// `trash` names the operation for the deleted-file records; each target
/// supplies its own item and role.
fn delete_targets(
    conn: &Connection,
    cache: &CachePaths,
    targets: &[DeleteTarget],
    mode: DeleteMode,
    trash: &trash::TrashContext,
    on_attempt: &mut (impl FnMut(u64, bool) + ?Sized),
) -> Result<DeleteOutcome, String> {
    let mut outcome = DeleteOutcome::default();

    for target in targets {
        let file = Path::new(&target.abs_path);
        let result = match mode {
            DeleteMode::Trash => match &target.owning_root {
                Ok(owning_root) => trash::trash_file(
                    file,
                    owning_root,
                    target.content_hash.as_deref(),
                    &trash.clone().item(target.item.clone()).role(target.role),
                )
                .map(|_| ()),
                Err(error) => Err(error.clone().into()),
            },
            DeleteMode::Permanent => permanently_delete_file(file),
        };

        match result {
            Ok(()) => {
                outcome.deleted_files += 1;
                // Row removal and orphan collection are one IMMEDIATE
                // transaction: a crash or DB error after this point can no
                // longer leak a `contents` row or cache file with no
                // surviving path (`reconcile_orphan_contents` is the
                // backstop for whatever still slips through). The physical
                // delete/trash already happened above, so a DB failure here
                // still reports the file as gone rather than silently
                // losing that fact.
                let tx = rusqlite::Transaction::new_unchecked(
                    conn,
                    rusqlite::TransactionBehavior::Immediate,
                )
                .map_err(|e| e.to_string())?;
                // A partially completed Move may deliver a main file before a
                // companion output. Detach surviving companions so the main
                // row can leave without discarding or misrepresenting them.
                tx.execute(
                    "UPDATE paths SET companion_of = NULL WHERE companion_of = ?1",
                    [target.path_id],
                )
                .map_err(|e| e.to_string())?;
                let current_hash = tx
                    .query_row(
                        "SELECT content_hash FROM paths WHERE id = ?1",
                        [target.path_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .or_else(|| target.content_hash.clone());
                tx.execute("DELETE FROM evidence WHERE path_id = ?1", [target.path_id])
                    .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM paths WHERE id = ?1", [target.path_id])
                    .map_err(|e| e.to_string())?;
                outcome.removed_rows += 1;

                // Only live files keep a content identity alive. Counting
                // missing rows too meant one copy on an absent drive pinned the
                // contents row and every cache entry for that hash forever. A
                // live companion counts: identical companions beside other main
                // copies (sidecars a Move left with their own main, a boilerplate
                // sidecar shared by two photos) must keep their rows.
                let mut orphaned = false;
                if let Some(hash) = &current_hash {
                    let live: i64 = tx
                        .query_row(
                            "SELECT COUNT(*) FROM paths WHERE content_hash = ?1 AND missing = 0",
                            [hash],
                            |r| r.get(0),
                        )
                        .map_err(|e| e.to_string())?;
                    if live == 0 {
                        // Missing rows are files that are not on disk; they may
                        // not hold a foreign key into a contents row that is
                        // about to go. Their evidence and any companion paired
                        // with them hold foreign keys to those rows in turn.
                        tx.execute(
                            "DELETE FROM evidence WHERE path_id IN \
                             (SELECT id FROM paths WHERE content_hash = ?1)",
                            [hash],
                        )
                        .map_err(|e| e.to_string())?;
                        tx.execute(
                            "UPDATE paths SET companion_of = NULL WHERE companion_of IN \
                             (SELECT id FROM paths WHERE content_hash = ?1)",
                            [hash],
                        )
                        .map_err(|e| e.to_string())?;
                        tx.execute("DELETE FROM paths WHERE content_hash = ?1", [hash])
                            .map_err(|e| e.to_string())?;
                        tx.execute(
                            "DELETE FROM similar_group_members WHERE content_hash = ?1",
                            [hash],
                        )
                        .map_err(|e| e.to_string())?;
                        tx.execute("DELETE FROM contents WHERE hash = ?1", [hash])
                            .map_err(|e| e.to_string())?;
                        orphaned = true;
                    }
                }
                crate::records::commit(tx).map_err(|e| e.to_string())?;
                if orphaned {
                    preview::remove_entries(cache, current_hash.as_deref().unwrap_or_default());
                }
                on_attempt(target.bytes, false);
            }
            Err(err) if err.outcome_unknown => {
                // Given up on while the volume was not responding: the file
                // may be gone or still in place. Its row stays; the next
                // source check finds it present, or missing (and in Deleted
                // files when it was trashed).
                outcome.failed_files += 1;
                outcome.unknown_files += 1;
                logging::warn(
                    "delete outcome unknown for one copy",
                    json!({ "path": target.abs_path, "error": { "message": err.message } }),
                );
                crate::index_store::upsert_issue_with_descriptor(
                    conn,
                    Some(&target.abs_path),
                    DELETE_OUTCOME_UNKNOWN,
                    Some("notice.deleteOutcomeUnknown"),
                    None,
                    &err.message,
                )?;
                on_attempt(target.bytes, true);
            }
            Err(err) => {
                outcome.failed_files += 1;
                // The issues table is the user surface; the session log is
                // the debugging record — mirror the failure where it is
                // raised, with its context intact.
                logging::warn(
                    "delete failed for one copy",
                    json!({ "path": target.abs_path, "error": { "message": err.message } }),
                );
                crate::index_store::upsert_issue_with_descriptor(
                    conn,
                    Some(&target.abs_path),
                    "delete-error",
                    Some("notice.deleteFailed"),
                    None,
                    &err.message,
                )?;
                on_attempt(target.bytes, true);
            }
        }
    }

    // The audit line — the op log is a logging concern, not a feature.
    logging::info(
        "delete",
        json!({
            "mode": mode.as_str(),
            "deletedFiles": outcome.deleted_files,
            "failedFiles": outcome.failed_files,
            "unknownFiles": outcome.unknown_files,
        }),
    );

    Ok(outcome)
}

fn collect_delete_targets(
    conn: &Connection,
    item: ItemRef<'_>,
    roots: &[std::path::PathBuf],
    accepted: &AcceptedFiles,
) -> Result<Vec<DeleteTarget>, String> {
    // Target rows: the item's own copies plus companions attached to any of
    // them, limited to the files the accepted batch captured.
    let item_key = match &item {
        ItemRef::Hash(hash) => hash.to_string(),
        ItemRef::PathId(path_id) => crate::indexed_file::item_key(None, *path_id),
    };
    let (mut targets, mut companions) = item_physical_rows(conn, item)?;
    accepted.retain_accepted(&mut targets);
    accepted.retain_accepted(&mut companions);
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    // Companions delete FIRST: their rows hold a foreign key to the primary
    // (`companion_of`), so the primary's row must outlive them.
    let roles = std::iter::repeat_n(trash::TrashRole::Companion, companions.len())
        .chain(std::iter::repeat_n(trash::TrashRole::Main, targets.len()))
        .collect::<Vec<_>>();
    companions.extend(targets);
    Ok(companions
        .into_iter()
        .zip(roles)
        .map(|((path_id, abs_path, content_hash, indexed_bytes), role)| {
            let owning_root = trash::root_for_file(Path::new(&abs_path), roots);
            let bytes = current_or_indexed_bytes(&abs_path, indexed_bytes);
            DeleteTarget {
                path_id,
                abs_path,
                content_hash,
                bytes,
                owning_root,
                item: Some(item_key.clone()),
                role,
            }
        })
        .collect())
}

/// Deletes an ordered logical-item set accepted now, under one
/// already-acquired mutation and media boundary. Target membership is resolved
/// once before the first file changes. Cancellation is observed while
/// planning and between physical file actions; filesystem failures remain
/// per-file Issues.
pub fn delete_batch(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    mode: DeleteMode,
    cancelled: &dyn Fn() -> bool,
    on_progress: impl FnMut(DeleteBatchProgress),
) -> Result<DeleteBatchOutcome, String> {
    let accepted = AcceptedFiles::capture(conn, items)?;
    delete_accepted_batch(conn, app_root, cache, items, &accepted, mode, cancelled, on_progress)
}

/// Deletes an ordered logical-item set whose physical files were captured
/// when the operation was accepted (`AcceptedFiles`).
#[allow(clippy::too_many_arguments)]
pub fn delete_accepted_batch(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    accepted: &AcceptedFiles,
    mode: DeleteMode,
    cancelled: &dyn Fn() -> bool,
    mut on_progress: impl FnMut(DeleteBatchProgress),
) -> Result<DeleteBatchOutcome, String> {
    if !accepted.preflight(cancelled)? {
        return Ok(DeleteBatchOutcome { cancelled: true, ..DeleteBatchOutcome::default() });
    }
    let roots = crate::storage::configured_file_roots(app_root)?;
    let trash_context =
        trash::TrashContext::new(trash::TrashKind::Delete, &crate::nanoid::generate()?);
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
        let mut targets = collect_delete_targets(conn, item.item_ref()?, &roots, accepted)?;
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
        batch.items_started = batch.items_started.saturating_add(1);
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
                cache,
                std::slice::from_ref(target),
                mode,
                &trash_context,
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
                        failures: batch.failed_files + outcome.failed_files + u64::from(failed),
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
            outcome.unknown_files = outcome.unknown_files.saturating_add(step.unknown_files);
        }
        batch.deleted_files = batch.deleted_files.saturating_add(outcome.deleted_files);
        batch.failed_files = batch.failed_files.saturating_add(outcome.failed_files);
        batch.removed_rows = batch.removed_rows.saturating_add(outcome.removed_rows);
        batch.unknown_files = batch.unknown_files.saturating_add(outcome.unknown_files);
        if !completed_unit {
            break;
        }
        batch.items.push(DeleteItemResult {
            item: unit.item,
            deleted_files: outcome.deleted_files,
            failed_files: outcome.failed_files,
            removed_rows: outcome.removed_rows,
            unknown_files: outcome.unknown_files,
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

fn permanently_delete_file(file: &Path) -> Result<(), trash::TrashError> {
    // A missing file fails as that one file, exactly as recoverable deletion
    // does: nothing was deleted, so the receipt must not count it.
    let metadata = volume_io::symlink_metadata(file)
        .map_err(|error| format!("file to delete is unavailable: {error}"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!("not a regular file: {}", file.display()).into());
    }
    volume_io::remove_file(file).map_err(|error| trash::TrashError {
        message: error.to_string(),
        outcome_unknown: volume_io::outcome_unknown(&error),
    })
}

/// Issue kind for a file whose delete or trash move was given up on while
/// its volume was not responding.
pub const DELETE_OUTCOME_UNKNOWN: &str = "delete-outcome-unknown";
/// Issue kind for an output whose publication was given up on while the
/// destination was not responding.
pub const COPY_OUTCOME_UNKNOWN: &str = "copy-outcome-unknown";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize, serde::Serialize)]
pub enum MoveOutMode {
    /// Plain drag: one copy moves out, the remaining copies go to trash.
    #[serde(rename = "move-trash-rest")]
    MoveTrashRest,
    /// Shift: one copy moves out, the remaining copies are deleted permanently.
    #[serde(rename = "move-delete-rest")]
    MoveDeleteRest,
    /// Cmd/Ctrl: a copy is exported; nothing else is touched.
    #[serde(rename = "copy")]
    CopyKeepAll,
}

impl MoveOutMode {
    /// The wire name, as the webview sends it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MoveTrashRest => "move-trash-rest",
            Self::MoveDeleteRest => "move-delete-rest",
            Self::CopyKeepAll => "copy",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DestinationConflictPolicy {
    Rename,
    Overwrite,
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
    /// Reviewed destination files preserved in Deleted files before overwrite.
    pub trashed_destination_files: u64,
    /// Conflicts that appeared after the reviewed plan was accepted. Expected
    /// conflicts are resolved before execution and never enter this result.
    pub conflicts: Vec<String>,
    /// Files that could not be written at all — every source copy failed to
    /// read or the destination refused the write (a full disk is the common
    /// case). Distinct from `conflicts`, which means the destination already
    /// holds different content; this failure leaves NOTHING at the target and
    /// previously had no way to be expressed at all.
    pub undelivered: Vec<String>,
    /// Outputs whose publication was given up on while the destination was
    /// not responding: each is either a complete file at its target or
    /// nothing. Their sources stay in place either way.
    pub unknown: Vec<String>,
    /// Companion sources whose content differs from the companion delivered
    /// under the same destination name (two edits of one photo, say). Copy
    /// does not deliver them; Move leaves each in place together with the
    /// main copy it belongs to and that copy's other companions.
    pub different_companions: Vec<String>,
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
    pub trashed_destination_files: u64,
    pub conflicts: Vec<String>,
    pub undelivered: Vec<String>,
    pub unknown: Vec<String>,
    pub different_companions: Vec<String>,
    pub post_action: DeleteOutcome,
    pub files_total: u64,
    pub bytes_total: u64,
    pub plan_token: Option<String>,
    pub requires_conflict_choice: bool,
    pub plan_changed: bool,
    pub overwrite_allowed: bool,
    pub reviewed_conflicts: Vec<DestinationConflict>,
    pub items_started: u64,
}

#[derive(Clone, Debug)]
struct DeliverySource {
    path_id: i64,
    abs_path: String,
    content_hash: Option<String>,
    bytes: u64,
    owning_root: Result<std::path::PathBuf, String>,
    /// For a companion source, the main copy it is paired with.
    beside: Option<i64>,
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

/// Moves or copies one ordered logical-item set, accepted now, under one
/// mutation/media boundary. Membership and destination names are frozen before the first
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
    let accepted = AcceptedFiles::capture(conn, items)?;
    move_batch_reviewed(
        conn,
        app_root,
        cache,
        items,
        &accepted,
        dest_dir,
        mode,
        None,
        None,
        RenameStyle::SpaceNumber,
        cancelled,
        on_progress,
    )
}

/// Moves or copies an ordered logical-item set whose physical files were
/// captured when the operation was accepted (`AcceptedFiles`), under the
/// reviewed destination-conflict decision when one was required.
#[allow(clippy::too_many_arguments)]
pub fn move_batch_reviewed(
    conn: &Connection,
    app_root: &Path,
    cache: &CachePaths,
    items: &[ItemIdentity],
    accepted: &AcceptedFiles,
    dest_dir: &Path,
    mode: MoveOutMode,
    conflict_policy: Option<DestinationConflictPolicy>,
    expected_plan_token: Option<&str>,
    rename_style: RenameStyle,
    cancelled: &dyn Fn() -> bool,
    mut on_progress: impl FnMut(MoveBatchProgress),
) -> Result<MoveBatchOutcome, String> {
    if mode != MoveOutMode::CopyKeepAll && !accepted.preflight(cancelled)? {
        return Ok(MoveBatchOutcome { cancelled: true, ..MoveBatchOutcome::default() });
    }
    let configured = crate::storage::configured_roots(app_root)?;
    let destination_root = admit_destination(dest_dir, &configured)?;
    // Ordinary Copy/Move staging lands flat in `dest_dir` (every delivery
    // target is `dest_dir.join(name)`), a folder the source walk never visits
    // unless it also happens to be a configured source. This is the one
    // place that sweeps it, so a crash's leftover staging does not linger
    // forever in a destination outside every source.
    crate::file_identity::sweep_private_tmp_leftovers(dest_dir);
    let roots = configured.all();
    let operation = crate::nanoid::generate()?;
    let names = FolderNames::for_directory(dest_dir);
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
        let mut unit = collect_move_unit(conn, item, dest_dir, &roots, accepted, names)?;
        if mode == MoveOutMode::CopyKeepAll {
            for delivery in &mut unit.deliveries {
                // Preserve the reviewed output name while choosing the first
                // reachable copy, without reporting an expected offline duplicate.
                if let Some(index) = delivery.sources.iter().position(|source|
                    crate::file_identity::open_regular_nofollow(Path::new(&source.abs_path)).is_ok()) {
                    delivery.sources.swap(0, index);
                    delivery.bytes = delivery.sources[0].bytes;
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
            plan.files_total = plan.files_total.saturating_add(
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
    let review = match review_destination_conflicts(&mut plan, names, cancelled) {
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
    if !review.conflicts.is_empty() && conflict_policy.is_some() && expected_plan_token.is_none() {
        return Err("destination conflict policy requires the reviewed plan token".to_string());
    }
    if conflict_policy == Some(DestinationConflictPolicy::Overwrite) && !review.overwrite_allowed {
        return Err(
            "overwrite cannot preserve every selected file in this conflict set; use Rename"
                .to_string(),
        );
    }
    if conflict_policy == Some(DestinationConflictPolicy::Rename) {
        apply_conflict_renames(&mut plan, rename_style, names)?;
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
        batch.items_started = batch.items_started.saturating_add(1);
        let execution = execute_move_unit(
            conn,
            cache,
            &unit,
            &destination_root,
            &operation,
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
        let mut stopped = None;
        let (outcome, unit_cancelled) = match execution {
            Ok(MoveUnitResult::Completed(outcome)) => (outcome, false),
            Ok(MoveUnitResult::Cancelled(outcome)) => (outcome, true),
            Ok(MoveUnitResult::Stopped(outcome, error)) => {
                stopped = Some(error);
                (outcome, false)
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
        batch.trashed_destination_files = batch
            .trashed_destination_files
            .saturating_add(outcome.trashed_destination_files);
        batch.conflicts.extend(outcome.conflicts.iter().cloned());
        batch
            .undelivered
            .extend(outcome.undelivered.iter().cloned());
        batch.unknown.extend(outcome.unknown.iter().cloned());
        batch
            .different_companions
            .extend(outcome.different_companions.iter().cloned());
        batch.post_action.unknown_files = batch
            .post_action
            .unknown_files
            .saturating_add(outcome.post_action.unknown_files);
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
            || !outcome.unknown.is_empty()
            || !outcome.different_companions.is_empty()
            || outcome.post_action.deleted_files > 0
            || outcome.post_action.failed_files > 0;
        let stopped_by_conflict = !outcome.conflicts.is_empty();
        if !unit_cancelled || has_effect {
            batch.items.push(MoveBatchItemResult {
                item: unit.item,
                outcome,
            });
        }
        if let Some(error) = stopped {
            logging::warn(
                "destination batch stopped: the destination is not responding",
                json!({ "error": { "message": error } }),
            );
            batch.error = Some(error);
            break;
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
            "differentCompanions": batch.different_companions.len(),
            "cancelled": batch.cancelled,
        }),
    );
    Ok(batch)
}

/// Destination admission at operation start: the destination must exist as a
/// directory, be a configured destination root or a folder beneath one, and
/// lie outside every configured source. Returns the most specific configured
/// destination root containing it, which owns overwrite displacement. A
/// configured root that is unavailable right now neither admits nor refuses
/// anything, so one unplugged drive never blocks another destination.
pub fn admit_destination(
    dest_dir: &Path,
    configured: &crate::storage::ConfiguredRoots,
) -> Result<std::path::PathBuf, String> {
    if !volume_io::is_dir(dest_dir).map_err(|error| {
        format!("could not inspect destination {}: {error}", dest_dir.display())
    })? {
        return Err(format!(
            "destination is not a directory: {}",
            dest_dir.display()
        ));
    }
    let mut owner: Option<(usize, &std::path::PathBuf)> = None;
    for root in &configured.destinations {
        if crate::path_identity::directory_is_within(dest_dir, root)? {
            let depth = volume_io::canonicalize(root)
                .map(|resolved| resolved.components().count())
                .unwrap_or(0);
            if owner.is_none_or(|(deepest, _)| depth > deepest) {
                owner = Some((depth, root));
            }
        }
    }
    let Some((_, destination_root)) = owner else {
        return Err(format!(
            "destination {} is not a configured destination root or one of its folders",
            dest_dir.display()
        ));
    };
    for source in &configured.sources {
        if crate::path_identity::directory_is_within(dest_dir, source)? {
            return Err(format!(
                "destination {} lies inside the scanned directory {}; move-out targets must be outside every source directory",
                dest_dir.display(),
                source.display()
            ));
        }
    }
    Ok(destination_root.clone())
}

fn move_plan_token(plan: &MovePlan, mode: MoveOutMode, review: &DestinationReview) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(mode.as_str().as_bytes());
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
    names: FolderNames,
    cancelled: &dyn Fn() -> bool,
) -> Result<DestinationReview, String> {
    let mut review = DestinationReview::default();
    let mut claimed = HashSet::new();
    for delivery in plan.units.iter_mut().flat_map(|unit| &mut unit.deliveries) {
        if cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        if !claimed.insert(names.key(&delivery.target)) {
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
                    names,
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
    let metadata = match volume_io::symlink_metadata(path) {
        Ok(metadata) => metadata,
        // A name the destination cannot hold holds nothing; publishing it
        // later fails as that one file.
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound || file_names::is_name_error(&error) =>
        {
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
    let (mut file, _) = crate::file_identity::open_regular_nofollow(path)
        .map_err(|error| format!("could not inspect destination {}: {error}", path.display()))?;
    let bytes = file.metadata().map_err(|error| error.to_string())?.len();
    let hash =
        crate::hashing::full_hash_file_cancellable(&mut file, bytes, cancelled, &mut |_, _| {})
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
        let (mut source_file, _) =
            match crate::file_identity::open_regular_nofollow(Path::new(&source.abs_path)) {
                Ok(file) => file,
                Err(_) => continue,
            };
        let source_bytes = source_file
            .metadata()
            .map_err(|error| error.to_string())?
            .len();
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
    names: FolderNames,
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
        for entry in volume_io::read_dir(parent, false)
            .map_err(|error| format!("could not inspect destination companions: {error}"))?
        {
            // `read_dir` preserves the `\\?\` filesystem prefix from its input
            // on Windows. Rebuild the child from the reviewed, user-facing
            // parent so recovery manifests and plan tokens keep one canonical
            // spelling while later filesystem calls can add the prefix again.
            let path = parent.join(entry.file_name);
            let same_stem = path
                .file_stem()
                .is_some_and(|candidate| names.same(Path::new(candidate), Path::new(stem)));
            if names.same(&path, target) || !same_stem {
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
    style: RenameStyle,
    names: FolderNames,
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
            reserved.extend(
                unit.deliveries
                    .iter()
                    .map(|delivery| names.key(&delivery.target)),
            );
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
                .map(|delivery| file_names::renamed(&delivery.target, number, style))
                .collect::<Option<Vec<_>>>()?;
            // A name the destination cannot hold (too long, invalid there)
            // is not occupied: it is planned and then fails at publication as
            // that one file, like any other refused final name.
            let available = candidates.iter().all(|candidate| {
                !reserved.contains(&names.key(candidate))
                    && volume_io::symlink_metadata(candidate)
                        .is_err_and(|error| {
                            error.kind() == std::io::ErrorKind::NotFound || file_names::is_name_error(&error)
                        })
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
            reserved.insert(names.key(&target));
        }
    }
    Ok(())
}

fn collect_move_unit(
    conn: &Connection,
    item: ItemIdentity,
    dest_dir: &Path,
    roots: &[std::path::PathBuf],
    accepted: &AcceptedFiles,
    names: FolderNames,
) -> Result<MoveUnit, String> {
    let (mut primary_rows, mut companion_rows): (Vec<_>, Vec<_>) = match item.item_ref()? {
        ItemRef::Hash(hash) => (
            collect4(
                conn,
                "SELECT id, abs_path, content_hash, size FROM paths \
                 WHERE content_hash = ?1 AND missing = 0 AND companion_of IS NULL \
                 ORDER BY review_visible DESC, resolved_utc_ms IS NULL, resolved_utc_ms, \
                          abs_path COLLATE onecopy_nocase, abs_path",
                params![hash],
            )?,
            collect_companions(
                conn,
                "SELECT comp.id, comp.abs_path, comp.content_hash, comp.size, comp.companion_of \
                 FROM paths comp JOIN paths pri ON comp.companion_of = pri.id \
                 WHERE pri.content_hash = ?1 AND pri.missing = 0 \
                   AND pri.companion_of IS NULL AND comp.missing = 0 \
                 ORDER BY pri.review_visible DESC, pri.resolved_utc_ms IS NULL, pri.resolved_utc_ms, \
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
            collect_companions(
                conn,
                "SELECT id, abs_path, content_hash, size, companion_of FROM paths \
                 WHERE companion_of = ?1 AND missing = 0 ORDER BY id",
                params![path_id],
            )?,
        ),
    };
    accepted.retain_accepted(&mut primary_rows);
    companion_rows.retain(|((path_id, abs_path, _, _), _)| accepted.accepts(*path_id, abs_path));
    let primary_sources = delivery_sources(primary_rows, roots);
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

    // Companions that would land on the same destination entry are one
    // output: on a case-insensitive destination `x.xmp` and `x.XMP` are one
    // name, and the companion beside the highest-ranked main copy supplies it.
    // Staging compares the others with it by content; one that differs is not
    // covered by this output (`stage_delivery`).
    let mut companions = Vec::<(String, Vec<DeliverySource>)>::new();
    let (companion_rows, companion_mains): (Vec<_>, Vec<_>) = companion_rows.into_iter().unzip();
    let companion_sources = delivery_sources(companion_rows, roots)
        .into_iter()
        .zip(companion_mains)
        .map(|(source, main)| DeliverySource {
            beside: Some(main),
            ..source
        });
    for source in companion_sources {
        let name = file_name(&source.abs_path)?;
        if let Some((_, sources)) = companions
            .iter_mut()
            .find(|(existing, _)| names.same(Path::new(existing), Path::new(&name)))
        {
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

fn delivery_sources(rows: Vec<PhysicalRow>, roots: &[std::path::PathBuf]) -> Vec<DeliverySource> {
    rows.into_iter()
        .map(|(path_id, abs_path, content_hash, indexed_bytes)| {
            let owning_root = trash::root_for_file(Path::new(&abs_path), roots);
            let bytes = current_or_indexed_bytes(&abs_path, indexed_bytes);
            DeliverySource {
                path_id,
                abs_path,
                content_hash,
                bytes,
                owning_root,
                beside: None,
            }
        })
        .collect()
}

/// A planned file's size for progress: its current size when it is a regular
/// file now, otherwise the indexed size (an unavailable copy still counts).
fn current_or_indexed_bytes(abs_path: &str, indexed_bytes: Option<i64>) -> u64 {
    volume_io::symlink_metadata(Path::new(abs_path))
        .ok()
        .filter(|metadata| metadata.file_type().is_file())
        .map(|metadata| metadata.len())
        .unwrap_or_else(|| indexed_bytes.unwrap_or(0).max(0) as u64)
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
    private: crate::file_identity::PrivateFile,
    hash: String,
    bytes: u64,
    primary: bool,
}

enum MoveUnitProgress {
    Stream { done: u64, total: u64 },
    Attempt { bytes: u64, failed: bool },
}

enum StageResult {
    /// The staged output, the planned sources it does not cover (main copies
    /// whose bytes no longer matched the item's recorded content, companions
    /// paired with such a copy, unreadable sources and companions whose
    /// content differs from the output), and the paths of those differing
    /// companions.
    Ready(StagedOutput, Vec<i64>, Vec<String>),
    Cancelled,
    /// No output; the planned sources left uncovered as in `Ready`.
    Failed(Vec<i64>),
}

enum MoveUnitResult {
    Completed(MoveOutOutcome),
    Cancelled(MoveOutOutcome),
    /// The destination stopped answering; nothing further is attempted.
    Stopped(MoveOutOutcome, String),
}

/// Delivers one logical item. Every output is first written and verified
/// under a private name; only then is any output published, so Overwrite
/// never displaces the destination family before the complete replacement
/// exists. Each staged output removes itself when dropped unpublished, so
/// every early return, failure, conflict and cancellation below abandons the
/// private output it still holds.
fn execute_move_unit(
    conn: &Connection,
    cache: &CachePaths,
    unit: &MoveUnit,
    destination_root: &Path,
    operation: &str,
    mode: MoveOutMode,
    conflict_policy: Option<DestinationConflictPolicy>,
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<MoveUnitResult, String> {
    let mut outcome = MoveOutOutcome::default();
    let mut staged = Vec::<(&DeliveryPlan, StagedOutput, Vec<i64>)>::new();
    let mut replacement_prepared = true;
    // Main copies whose bytes no longer match stay in place, and so do the
    // companions paired with them: no output covers that copy's family. The
    // main delivery is planned, and so staged, before every companion.
    let mut changed_mains = Vec::<i64>::new();
    // A main copy beside a companion whose content differs from the one
    // delivered under its name stays in place with all of its companions:
    // that companion belongs to it and is neither lost nor separated from it.
    // Settled before any publication below. (A companion that merely failed
    // to read stays in place on its own, as any uncovered source does.)
    let mut kept_mains = Vec::<i64>::new();
    for delivery in &unit.deliveries {
        if cancelled() {
            return Ok(MoveUnitResult::Cancelled(outcome));
        }
        // Move covers other copies only with bytes proven to be the item's
        // recorded content. Copy delivers the file as it currently exists,
        // and an item with no recorded full hash has nothing to prove against.
        let recorded_hash = unit
            .item
            .hash
            .as_deref()
            .filter(|hash| {
                delivery.primary
                    && mode != MoveOutMode::CopyKeepAll
                    && !crate::scanner::is_provisional(hash)
            });
        match stage_delivery(
            conn,
            delivery,
            recorded_hash,
            mode,
            &changed_mains,
            cancelled,
            on_progress,
        )? {
            StageResult::Ready(output, uncovered, different) => {
                if delivery.primary {
                    changed_mains.extend(&uncovered);
                }
                kept_mains.extend(
                    delivery
                        .sources
                        .iter()
                        .filter(|source| different.contains(&source.abs_path))
                        .filter_map(|source| source.beside),
                );
                outcome.different_companions.extend(different);
                staged.push((delivery, output, uncovered))
            }
            StageResult::Cancelled => return Ok(MoveUnitResult::Cancelled(outcome)),
            StageResult::Failed(uncovered) => {
                if delivery.primary {
                    changed_mains.extend(&uncovered);
                }
                replacement_prepared = false;
                outcome
                    .undelivered
                    .push(delivery.target.to_string_lossy().into_owned());
                on_progress(MoveUnitProgress::Attempt {
                    bytes: delivery.bytes,
                    failed: true,
                });
            }
        }
    }
    if let Some(stored) = &unit.provisional_hash {
        if let Some((_, primary, _)) = staged.iter().find(|(_, output, _)| output.primary) {
            crate::scanner::promote_identity(conn, cache, stored, &primary.hash)?;
        }
    }

    for (delivery, mut output, changed_sources) in staged {
        if cancelled() {
            return Ok(MoveUnitResult::Cancelled(outcome));
        }
        // From publication through this output group's source cleanup,
        // cancellation is deliberately deferred: each of those short steps
        // finishes within its own bound. The next output is the next safe
        // boundary.
        let delivered = match output.private.publish(&output.target) {
            Err(error) if volume_io::wait_failure(&error).is_some() => {
                return publication_given_up(conn, outcome, &output, &error, on_progress);
            }
            Ok(()) => {
                let durable = sync_published(conn, &output.target)?;
                if durable {
                    outcome.exported = outcome.exported.saturating_add(1);
                } else {
                    outcome
                        .undelivered
                        .push(output.target.to_string_lossy().into_owned());
                }
                durable
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing_hash = match crate::file_identity::open_regular_nofollow(&output.target)
                {
                    Ok((mut file, _)) => {
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
                        Some(hash.ok())
                    }
                    Err(_) => None,
                };
                match existing_hash {
                    Some(Some(hash)) if hash == output.hash => {
                        outcome.skipped_identical = outcome.skipped_identical.saturating_add(1);
                        true
                    }
                    Some(_)
                        if conflict_policy == Some(DestinationConflictPolicy::Overwrite)
                            && !replacement_prepared
                            && !delivery.replacement_family.is_empty() =>
                    {
                        crate::index_store::upsert_issue_with_descriptor(
                            conn,
                            Some(output.target.to_string_lossy().as_ref()),
                            "copy-error",
                            Some("notice.copyReplacementUnavailable"),
                            None,
                            "",
                        )?;
                        outcome
                            .undelivered
                            .push(output.target.to_string_lossy().into_owned());
                        false
                    }
                    Some(_) if conflict_policy == Some(DestinationConflictPolicy::Overwrite) => {
                        preserve_reviewed_destination_family(
                            conn,
                            &delivery.replacement_family,
                            &output.target,
                            destination_root,
                            operation,
                        )?;
                        outcome.trashed_destination_files = outcome
                            .trashed_destination_files
                            .saturating_add(delivery.replacement_family.len() as u64);
                        match output.private.publish(&output.target) {
                            Ok(()) => {}
                            Err(error) if volume_io::wait_failure(&error).is_some() => {
                                return publication_given_up(conn, outcome, &output, &error, on_progress);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                return Err(format!(
                                    "a new destination conflict appeared at {}",
                                    output.target.display()
                                ))
                            }
                            Err(error) => return Err(error.to_string()),
                        }
                        let durable = sync_published(conn, &output.target)?;
                        if durable {
                            outcome.exported = outcome.exported.saturating_add(1);
                        } else {
                            outcome
                                .undelivered
                                .push(output.target.to_string_lossy().into_owned());
                        }
                        durable
                    }
                    _ => {
                        outcome
                            .conflicts
                            .push(output.target.to_string_lossy().into_owned());
                        false
                    }
                }
            }
            // A failure at this one final name (too long or invalid for the
            // destination filesystem, or refused by it) fails only this file.
            Err(error) => {
                logging::warn(
                    "copy-out publication failed",
                    json!({ "target": output.target.to_string_lossy(), "error": { "message": error.to_string() } }),
                );
                crate::index_store::upsert_issue_with_descriptor(
                    conn,
                    Some(output.target.to_string_lossy().as_ref()),
                    "copy-error",
                    Some("notice.copyPublishFailed"),
                    None,
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
            // A source this output does not cover stays in place, and so does
            // the whole family of a main copy kept above.
            let item_key = unit.item.key()?;
            let targets = delivery
                .sources
                .iter()
                .filter(|source| {
                    !changed_sources.contains(&source.path_id)
                        && !kept_mains.contains(&source.beside.unwrap_or(source.path_id))
                })
                .map(|source| DeleteTarget {
                    path_id: source.path_id,
                    abs_path: source.abs_path.clone(),
                    content_hash: source.content_hash.clone(),
                    bytes: source.bytes,
                    owning_root: source.owning_root.clone(),
                    item: Some(item_key.clone()),
                    role: if source.beside.is_some() {
                        trash::TrashRole::Companion
                    } else {
                        trash::TrashRole::Main
                    },
                })
                .collect::<Vec<_>>();
            let cleanup_context = trash::TrashContext::new(trash::TrashKind::MoveCleanup, operation)
                .moved_to(Some(output.target.to_string_lossy().into_owned()));
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
                    cache,
                    std::slice::from_ref(target),
                    delete_mode,
                    &cleanup_context,
                    &mut |bytes, failed| on_progress(MoveUnitProgress::Attempt { bytes, failed }),
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
                outcome.post_action.unknown_files = outcome
                    .post_action
                    .unknown_files
                    .saturating_add(cleanup.unknown_files);
            }
        }
        if !outcome.conflicts.is_empty() {
            break;
        }
    }

    Ok(MoveUnitResult::Completed(outcome))
}

/// Ends a unit whose publication the destination did not answer. A call
/// given up on while it ran leaves the output either complete at its target
/// or nothing there (it settles on its worker); one refused at once because
/// the destination was already stalled did nothing. Either way the sources
/// stay in place and nothing further goes to this destination.
fn publication_given_up(
    conn: &Connection,
    mut outcome: MoveOutOutcome,
    output: &StagedOutput,
    error: &std::io::Error,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<MoveUnitResult, String> {
    on_progress(MoveUnitProgress::Attempt {
        bytes: output.bytes,
        failed: true,
    });
    let target = output.target.to_string_lossy().into_owned();
    if volume_io::outcome_unknown(error) {
        logging::warn(
            "copy-out publication outcome unknown",
            json!({ "target": target, "error": { "message": error.to_string() } }),
        );
        crate::index_store::upsert_issue_with_descriptor(
            conn,
            Some(&target),
            COPY_OUTCOME_UNKNOWN,
            Some("notice.copyOutcomeUnknown"),
            None,
            &error.to_string(),
        )?;
        outcome.unknown.push(target);
    } else {
        crate::index_store::upsert_issue_with_descriptor(
            conn,
            Some(&target),
            "copy-error",
            Some("notice.copyPublishFailed"),
            None,
            &error.to_string(),
        )?;
        outcome.undelivered.push(target);
    }
    Ok(MoveUnitResult::Stopped(outcome, error.to_string()))
}

/// Makes a just-published output durable. An output whose directory could not
/// be synced is reported as not delivered, so its sources stay in place.
fn sync_published(conn: &Connection, target: &Path) -> Result<bool, String> {
    let Some(parent) = target.parent() else {
        return Ok(true);
    };
    match crate::fs_publish::sync_directory(parent) {
        Ok(()) => Ok(true),
        Err(error) => {
            crate::index_store::upsert_issue_with_descriptor(
                conn,
                Some(target.to_string_lossy().as_ref()),
                "copy-error",
                Some("notice.copyDirectorySyncFailed"),
                None,
                &error.to_string(),
            )?;
            Ok(false)
        }
    }
}

fn preserve_reviewed_destination_family(
    conn: &Connection,
    family: &[ReviewedDestinationFile],
    replaced: &Path,
    destination_root: &Path,
    operation: &str,
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
    // The replaced file is the main one; the rest are its companions. The
    // reviewed hash travels into the record: it is what the user saw
    // replaced, and the stored file was just re-proven to hold it.
    let context = trash::TrashContext::new(trash::TrashKind::OverwriteDisplaced, operation);
    for member in family {
        let role = if member.path == replaced {
            trash::TrashRole::Main
        } else {
            trash::TrashRole::Companion
        };
        crate::trash::trash_file(
            &member.path,
            destination_root,
            Some(&member.hash),
            &context.clone().role(role),
        )
        .map_err(|error| {
            let message = format!(
                "could not preserve the existing destination {} in Deleted files: {error}",
                member.path.display()
            );
            let _ = crate::index_store::upsert_issue_with_descriptor(
                conn,
                Some(member.path.to_string_lossy().as_ref()),
                "copy-error",
                Some("notice.copyPreserveDestinationFailed"),
                None,
                &error.to_string(),
            );
            message
        })?;
    }
    Ok(())
}

fn stage_delivery(
    conn: &Connection,
    delivery: &DeliveryPlan,
    recorded_hash: Option<&str>,
    mode: MoveOutMode,
    changed_mains: &[i64],
    cancelled: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(MoveUnitProgress),
) -> Result<StageResult, String> {
    let mut changed_sources = delivery
        .sources
        .iter()
        .filter(|source| source.beside.is_some_and(|main| changed_mains.contains(&main)))
        .map(|source| source.path_id)
        .collect::<Vec<_>>();
    for (index, source) in delivery.sources.iter().enumerate() {
        if changed_sources.contains(&source.path_id) {
            continue;
        }
        let staged = output_stage_path(&delivery.target)?;
        let copied = crate::hashing::hash_while_copying_cancellable_detailed(
            Path::new(&source.abs_path),
            &staged,
            cancelled,
            &mut |done, total| on_progress(MoveUnitProgress::Stream { done, total }),
        );
        match copied {
            Ok((hash, _, private)) if recorded_hash.is_some_and(|recorded| recorded != hash) => {
                drop(private);
                logging::warn(
                    "move skipped a changed source copy",
                    json!({ "path": source.abs_path, "target": delivery.target.to_string_lossy() }),
                );
                crate::index_store::upsert_issue_with_descriptor(
                    conn,
                    Some(&source.abs_path),
                    "copy-error",
                    Some("notice.copySourceChanged"),
                    None,
                    "",
                )?;
                changed_sources.push(source.path_id);
            }
            Ok((hash, bytes, private)) => {
                let mut different = Vec::new();
                // A main delivery proves the other copies against the item's
                // recorded content. A companion delivery compares them with the
                // output itself: same-named companions of different main copies
                // need not be identical, and one that differs is not covered.
                // Copy removes nothing, so an unreadable companion (an offline
                // duplicate, typically) is simply not compared.
                let expected = if delivery.primary { recorded_hash } else { Some(hash.as_str()) };
                if let Some(recorded) = expected {
                    for remaining in &delivery.sources[index + 1..] {
                        if changed_sources.contains(&remaining.path_id) {
                            continue;
                        }
                        let verified = crate::file_identity::open_regular_nofollow(
                            Path::new(&remaining.abs_path),
                        )
                        .and_then(|(mut file, _)| {
                            crate::hashing::full_hash_file_cancellable(
                                &mut file,
                                remaining.bytes,
                                cancelled,
                                &mut |done, total| on_progress(MoveUnitProgress::Stream { done, total }),
                            )
                        });
                        let (descriptor, detail) = match verified {
                            Ok(current) if current == recorded => continue,
                            Ok(_) if !delivery.primary => {
                                logging::info(
                                    "a companion differs from the one delivered under its name",
                                    json!({ "path": remaining.abs_path, "target": delivery.target.to_string_lossy() }),
                                );
                                different.push(remaining.abs_path.clone());
                                changed_sources.push(remaining.path_id);
                                continue;
                            }
                            Ok(_) => ("notice.copySourceChanged", String::new()),
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted && cancelled() => {
                                return Ok(StageResult::Cancelled);
                            }
                            Err(_) if mode == MoveOutMode::CopyKeepAll => continue,
                            Err(error) => ("notice.copySourceReadFailed", error.to_string()),
                        };
                        logging::warn(
                            "move left an uncovered source copy in place",
                            json!({ "path": remaining.abs_path, "target": delivery.target.to_string_lossy(), "error": { "message": detail } }),
                        );
                        crate::index_store::upsert_issue_with_descriptor(
                            conn,
                            Some(&remaining.abs_path),
                            "copy-error",
                            Some(descriptor),
                            None,
                            &detail,
                        )?;
                        changed_sources.push(remaining.path_id);
                    }
                }
                return Ok(StageResult::Ready(
                    StagedOutput {
                        target: delivery.target.clone(),
                        private,
                        hash,
                        bytes,
                        primary: delivery.primary,
                    },
                    changed_sources,
                    different,
                ));
            }
            Err(crate::hashing::CopyFailure::Cancelled) => return Ok(StageResult::Cancelled),
            Err(crate::hashing::CopyFailure::Source(error)) => {
                changed_sources.push(source.path_id);
                logging::warn(
                    "copy-out staging failed for one source",
                    json!({ "path": source.abs_path, "target": delivery.target.to_string_lossy(), "error": { "message": error.to_string() } }),
                );
                crate::index_store::upsert_issue_with_descriptor(
                    conn,
                    Some(&source.abs_path),
                    "copy-error",
                    Some("notice.copySourceReadFailed"),
                    None,
                    &error.to_string(),
                )?;
            }
            // The private name has a short fixed length, so a failure to
            // write it concerns the destination itself (full, disconnected,
            // read-only, broken) and stops later writes there.
            Err(crate::hashing::CopyFailure::Destination(error)) => {
                let message = format!(
                    "destination could not accept {}: {error}",
                    delivery.target.display()
                );
                logging::warn(
                    "copy-out destination failed",
                    json!({ "target": delivery.target.to_string_lossy(), "error": { "message": error.to_string() } }),
                );
                crate::index_store::upsert_issue_with_descriptor(
                    conn,
                    Some(delivery.target.to_string_lossy().as_ref()),
                    "copy-error",
                    Some("notice.copyDestinationRefused"),
                    None,
                    &error.to_string(),
                )?;
                return Err(message);
            }
        }
    }
    Ok(StageResult::Failed(changed_sources))
}

/// A private name beside the final target with a short fixed length. It never
/// exceeds a final name longer than itself, so a name the destination accepts
/// can always be staged, and a name it refuses fails at publication as that
/// one file.
fn output_stage_path(target: &Path) -> Result<std::path::PathBuf, String> {
    Ok(target.with_file_name(crate::file_identity::private_stage_file_name()?))
}

/// Companion rows, each with the main copy it is paired with (the query's
/// fifth column).
fn collect_companions(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<(PhysicalRow, i64)>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |r| {
            Ok(((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?), r.get(4)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

fn collect4(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<PhysicalRow>, String> {
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    Ok(rows)
}
