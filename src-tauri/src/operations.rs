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
            DeleteMode::Trash => trash::trash_file_with_before_claim(
                file,
                app_root,
                content_hash.as_deref(),
                |path| before_source_claim(mode, path),
            )
            .map(|_| ()),
            DeleteMode::Permanent => permanently_delete_file(file, |path| {
                before_source_claim(mode, path)
            }),
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
                crate::index_store::upsert_issue(conn, Some(&abs_path), "delete-error", &err)?;
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

fn permanently_delete_file(
    file: &Path,
    before_claim: impl FnOnce(&Path),
) -> Result<(), String> {
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
    let mut existing_target_proofs = Vec::new();
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
    if let Some(proof) = delivery.existing_target {
        existing_target_proofs.push(proof);
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
        if let Some(proof) = companion.existing_target {
            existing_target_proofs.push(proof);
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

    // An identical file that was already present is only delivery authority
    // while its public name still identifies the exact descriptor we hashed.
    // Revalidate every such proof immediately before any source post-action.
    for proof in &mut existing_target_proofs {
        if !proof.revalidate() {
            outcome
                .conflicts
                .push(proof.path.to_string_lossy().into_owned());
        }
    }
    if !outcome.conflicts.is_empty() {
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
    existing_target: Option<ExistingTargetProof>,
}

struct ExistingTargetProof {
    path: std::path::PathBuf,
    identity: crate::file_identity::FileIdentity,
    file: std::fs::File,
    verified_hash: String,
}

impl ExistingTargetProof {
    fn revalidate(&mut self) -> bool {
        if !crate::file_identity::path_names(&self.path, self.identity) {
            return false;
        }
        let unchanged = crate::hashing::full_hash_file(&mut self.file)
            .is_ok_and(|hash| hash == self.verified_hash);
        unchanged && crate::file_identity::path_names(&self.path, self.identity)
    }
}

/// Delivers one file (trying each listed copy in order) to `target`.
fn deliver_one(
    conn: &Connection,
    copies: &[(i64, String, Option<String>)],
    expected_hash: Option<&str>,
    target: &Path,
    outcome: &mut MoveOutOutcome,
) -> Result<Delivery, String> {
    match inspect_existing_target(target, |_| {}) {
        Ok(Some(existing)) => {
            let matches = match expected_hash {
                Some(expected) => existing.proof.verified_hash == expected,
                // Unhashed item: compare against the first copy's actual bytes.
                None => match copies.first() {
                    Some((_, path, _)) => {
                        crate::hashing::full_hash(Path::new(path)).map_err(|e| e.to_string())?
                            == existing.proof.verified_hash
                    }
                    None => false,
                },
            };
            if matches {
                outcome.skipped_identical += 1;
                return Ok(Delivery {
                    ok: true,
                    real_hash: Some(existing.proof.verified_hash.clone()),
                    existing_target: Some(existing.proof),
                }); // already delivered
            }
            outcome.conflicts.push(target.to_string_lossy().to_string());
            return Ok(Delivery {
                ok: false,
                real_hash: None,
                existing_target: None,
            });
        }
        Ok(None) => {}
        Err(_) => {
            // A symlink, non-regular occupant, unreadable file, or replacement
            // during descriptor hashing is a conflict, never proof that a
            // destructive post-action is safe.
            outcome.conflicts.push(target.to_string_lossy().to_string());
            return Ok(Delivery {
                ok: false,
                real_hash: None,
                existing_target: None,
            });
        }
    }

    for (_, source_path, _) in copies {
        let source = Path::new(source_path);
        // Complete and verify beside the target under a unique private name.
        // Only the final exclusive rename makes bytes public, so a crash cannot
        // leave a partial file masquerading as the requested output.
        let staged = output_stage_path(target);
        match crate::hashing::hash_while_copying(source, &staged) {
            Ok((streamed_hash, _bytes, staged_id)) => {
                // Source verification is free: the tee hashed what was read.
                if let Some(expected) = expected_hash {
                    if streamed_hash != expected {
                        crate::file_identity::remove_private_if_owned(&staged, staged_id);
                        record_rot_issue(conn, source_path, expected, &streamed_hash)?;
                        continue; // the redundant copies pay off: try the next
                    }
                }
                // Move the private name once more and verify what actually
                // moved before it is eligible for public publication. This
                // closes the replace-between-verification-and-rename edge.
                let claimed = match crate::file_identity::claim_private(&staged, staged_id) {
                    Ok(claimed) => claimed,
                    Err(_) => {
                        outcome.conflicts.push(target.to_string_lossy().to_string());
                        return Ok(Delivery {
                            ok: false,
                            real_hash: None,
                            existing_target: None,
                        });
                    }
                };
                match crate::fs_publish::rename_no_replace(&claimed, target) {
                    Ok(()) => {
                        // A successful syscall committed this exact inode. Verify
                        // the name still identifies it before authorizing the
                        // destructive post-action; a later external winner is
                        // preserved and reported as a collision.
                        if !crate::file_identity::path_names(target, staged_id) {
                            outcome.conflicts.push(target.to_string_lossy().to_string());
                            return Ok(Delivery {
                                ok: false,
                                real_hash: None,
                                existing_target: None,
                            });
                        }
                        if let Some(parent) = target.parent() {
                            crate::fs_publish::sync_directory(parent)
                                .map_err(|e| format!("could not durably publish {}: {e}", target.display()))?;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                        crate::file_identity::remove_private_if_owned(&claimed, staged_id);
                        outcome.conflicts.push(target.to_string_lossy().to_string());
                        return Ok(Delivery {
                            ok: false,
                            real_hash: None,
                            existing_target: None,
                        });
                    }
                    Err(err) => {
                        crate::file_identity::remove_private_if_owned(&claimed, staged_id);
                        logging::warn(
                            "copy-out publication failed for one source",
                            json!({ "path": source_path, "target": target.to_string_lossy(), "error": { "message": err.to_string() } }),
                        );
                        crate::index_store::upsert_issue(
                            conn,
                            Some(source_path),
                            "copy-error",
                            &err.to_string(),
                        )?;
                        continue;
                    }
                }
                outcome.exported += 1;
                return Ok(Delivery {
                    ok: true,
                    real_hash: Some(streamed_hash),
                    existing_target: None,
                });
            }
            Err(err) => {
                // The copy helper removes only a private stage whose physical
                // identity it captured. If identity capture itself failed, its
                // nanoid name is safe crash debris; never unlink an unowned
                // pathname merely because this attempt failed.
                logging::warn(
                    "copy-out failed for one source",
                    json!({ "path": source_path, "error": { "message": err.to_string() } }),
                );
                crate::index_store::upsert_issue(
                    conn,
                    Some(source_path),
                    "copy-error",
                    &err.to_string(),
                )?;
            }
        }
    }
    Ok(Delivery {
        ok: false,
        real_hash: None,
        existing_target: None,
    })
}

struct ExistingTarget {
    proof: ExistingTargetProof,
}

fn inspect_existing_target(
    target: &Path,
    after_hash: impl FnOnce(&Path),
) -> std::io::Result<Option<ExistingTarget>> {
    let (mut file, identity) = match crate::file_identity::open_regular_nofollow(target) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let hash = crate::hashing::full_hash_file(&mut file)?;
    after_hash(target);
    if !crate::file_identity::path_names(target, identity) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "destination was replaced while being verified",
        ));
    }
    Ok(Some(ExistingTarget {
        proof: ExistingTargetProof {
            path: target.to_path_buf(),
            identity,
            file,
            verified_hash: hash,
        },
    }))
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

        assert!(inspect_existing_target(&link, |_| {}).is_err());
        assert_eq!(std::fs::read(&real).unwrap(), b"identical");
    }

    #[test]
    fn existing_destination_replacement_at_hash_boundary_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jpg");
        let held = dir.path().join("held.jpg");
        std::fs::write(&target, b"identical").unwrap();

        let result = inspect_existing_target(&target, |path| {
            std::fs::rename(path, &held).unwrap();
            std::fs::write(path, b"replacement").unwrap();
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&held).unwrap(), b"identical");
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
    }

    #[test]
    fn existing_destination_replacement_before_post_action_revokes_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jpg");
        let held = dir.path().join("held.jpg");
        std::fs::write(&target, b"identical").unwrap();

        let mut existing = inspect_existing_target(&target, |_| {})
            .unwrap()
            .expect("existing regular target");
        std::fs::rename(&target, &held).unwrap();
        std::fs::write(&target, b"replacement").unwrap();

        assert!(!existing.proof.revalidate());
        assert_eq!(std::fs::read(&held).unwrap(), b"identical");
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
    }

    #[test]
    fn existing_destination_same_inode_rewrite_before_post_action_revokes_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.jpg");
        std::fs::write(&target, b"identical").unwrap();

        let mut existing = inspect_existing_target(&target, |_| {})
            .unwrap()
            .expect("existing regular target");
        let identity = existing.proof.identity;
        std::fs::write(&target, b"rewritten").unwrap();

        assert!(
            crate::file_identity::path_names(&target, identity),
            "the mutation keeps the same public physical file"
        );
        assert!(!existing.proof.revalidate());
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
            .query_row("SELECT content_hash FROM paths LIMIT 1", [], |row| row.get(0))
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
