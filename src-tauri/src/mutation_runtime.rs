//! Ephemeral ownership and transport for user-requested item mutations.
//!
//! `operations` owns plans, filesystem semantics, and results. This module
//! owns only one live claim, its identity and cancellation flag, plus the
//! coalesced progress/terminal events shared by delete and destination
//! batches. Nothing here survives a process exit or represents durable intent.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Active {
    id: u64,
    cancelled: Arc<AtomicBool>,
}

static ACTIVE: LazyLock<Mutex<Option<Active>>> = LazyLock::new(|| Mutex::new(None));

struct Claim {
    id: u64,
    cancelled: Arc<AtomicBool>,
}

impl Claim {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE.lock() {
            if active.as_ref().map(|entry| entry.id) == Some(self.id) {
                *active = None;
            }
        }
    }
}

fn begin() -> Result<Claim, String> {
    let mut active = ACTIVE
        .lock()
        .map_err(|_| "file-operation state is unavailable".to_string())?;
    if active.is_some() {
        return Err("Another file operation is already running.".to_string());
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let cancelled = Arc::new(AtomicBool::new(false));
    *active = Some(Active {
        id,
        cancelled: cancelled.clone(),
    });
    Ok(Claim { id, cancelled })
}

pub(crate) fn request_cancel(id: u64) -> bool {
    let Ok(active) = ACTIVE.lock() else {
        return false;
    };
    let Some(active) = active.as_ref().filter(|active| active.id == id) else {
        return false;
    };
    active.cancelled.store(true, Ordering::SeqCst);
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Planning,
    Deleting,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    operation_id: u64,
    kind: Kind,
    phase: Phase,
    items_done: u64,
    items_total: u64,
    files_done: u64,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
    failures: u64,
    next_phase: Option<Phase>,
}

struct Publisher {
    app: AppHandle,
    last_emit: Instant,
    last_phase: Option<Phase>,
    last_failures: u64,
}

impl Publisher {
    fn new(app: &AppHandle) -> Self {
        Self {
            app: app.clone(),
            last_emit: Instant::now() - Duration::from_secs(1),
            last_phase: None,
            last_failures: 0,
        }
    }

    fn progress(&mut self, progress: &Progress) {
        let now = Instant::now();
        let phase_changed = self.last_phase != Some(progress.phase);
        let failure_changed = self.last_failures != progress.failures;
        let completed = progress.phase == Phase::Complete
            || (progress.items_done == progress.items_total
                && progress.files_done == progress.files_total);
        if phase_changed
            || failure_changed
            || completed
            || now.duration_since(self.last_emit) >= Duration::from_millis(125)
        {
            self.last_emit = now;
            self.last_phase = Some(progress.phase);
            self.last_failures = progress.failures;
            let _ = self.app.emit("mutation://progress", progress);
        }
    }

    fn done(&mut self, progress: &Progress, cancelled: bool) {
        self.progress(progress);
        let _ = self.app.emit(
            "mutation://done",
            json!({ "progress": progress, "cancelled": cancelled }),
        );
    }

    fn error(&self, operation_id: u64, kind: Kind, error: &str) {
        let _ = self.app.emit(
            "mutation://error",
            json!({ "operationId": operation_id, "kind": kind, "error": error }),
        );
    }
}

/// Runs the delete command under this runtime's ephemeral lifecycle. The
/// operation module still owns planning/execution semantics; this function is
/// the application-edge orchestration kept out of the Tauri bootstrap.
pub(crate) fn delete_items(
    app: &AppHandle,
    mut items: Vec<crate::operations::ItemIdentity>,
    permanent: bool,
) -> Result<crate::operations::DeleteBatchOutcome, String> {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
    let mutation = begin()?;
    let operation_id = mutation.id();
    let mut publisher = Publisher::new(app);
    let mut last_progress = Progress {
        operation_id,
        kind: Kind::Delete,
        phase: Phase::Planning,
        items_done: 0,
        items_total: items.len() as u64,
        files_done: 0,
        files_total: 0,
        bytes_done: 0,
        bytes_total: 0,
        failures: 0,
        next_phase: Some(Phase::Deleting),
    };
    publisher.progress(&last_progress);
    let result = crate::logging::boundary(
        "delete_items",
        json!({ "items": items.len(), "permanent": permanent, "operationId": operation_id }),
        || {
            if items.is_empty() {
                return Ok(crate::operations::DeleteBatchOutcome::default());
            }
            let keys = items
                .iter()
                .map(crate::operations::ItemIdentity::media_key)
                .collect::<Result<Vec<_>, _>>()?;
            let _media = crate::media_use::begin(app, &keys)?;
            crate::ensure_sources_present(app)?;
            let data_root = crate::paths::data_root(app)?;
            let conn = crate::index_store::open(
                &data_root.join(crate::storage::INDEX_DB_FILE_NAME),
            )?;
            let cache = crate::preview::CachePaths::new(
                data_root.join(crate::storage::CACHE_DIR_NAME),
            );
            let mode = if permanent {
                crate::operations::DeleteMode::Permanent
            } else {
                crate::operations::DeleteMode::Trash
            };
            crate::operations::delete_batch(
                &conn,
                &data_root,
                &cache,
                &items,
                mode,
                &|| mutation.cancelled(),
                |progress| {
                    last_progress = match progress {
                        crate::operations::DeleteBatchProgress::Planning {
                            items_done,
                            items_total,
                            files_total,
                            bytes_total,
                        } => Progress {
                            operation_id,
                            kind: Kind::Delete,
                            phase: Phase::Planning,
                            items_done,
                            items_total,
                            files_done: 0,
                            files_total,
                            bytes_done: 0,
                            bytes_total,
                            failures: 0,
                            next_phase: Some(Phase::Deleting),
                        },
                        crate::operations::DeleteBatchProgress::Deleting {
                            items_done,
                            items_total,
                            files_done,
                            files_total,
                            bytes_done,
                            bytes_total,
                            failures,
                        } => Progress {
                            operation_id,
                            kind: Kind::Delete,
                            phase: Phase::Deleting,
                            items_done,
                            items_total,
                            files_done,
                            files_total,
                            bytes_done,
                            bytes_total,
                            failures,
                            next_phase: Some(Phase::Complete),
                        },
                    };
                    publisher.progress(&last_progress);
                },
            )
        },
        |outcome| {
            json!({
                "cancelled": outcome.cancelled,
                "items": outcome.items.len(),
                "deleted": outcome.deleted_files,
                "failed": outcome.failed_files,
            })
        },
    );
    match &result {
        Ok(outcome) => {
            let terminal = Progress {
                operation_id,
                kind: Kind::Delete,
                phase: Phase::Complete,
                items_done: outcome.items.len() as u64,
                items_total: last_progress.items_total,
                files_done: outcome.deleted_files.saturating_add(outcome.failed_files),
                files_total: outcome.files_total,
                bytes_done: if last_progress.phase == Phase::Deleting {
                    last_progress.bytes_done
                } else {
                    0
                },
                bytes_total: outcome.bytes_total,
                failures: outcome.failed_files,
                next_phase: None,
            };
            publisher.done(&terminal, outcome.cancelled);
        }
        Err(error) => publisher.error(operation_id, Kind::Delete, error),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_bound_to_one_claim_identity() {
        let first = begin().unwrap();
        let first_id = first.id();
        assert!(begin().is_err());
        assert!(request_cancel(first_id));
        assert!(first.cancelled());
        drop(first);

        let second = begin().unwrap();
        assert_ne!(second.id(), first_id);
        assert!(!request_cancel(first_id));
        assert!(!second.cancelled());
    }
}
