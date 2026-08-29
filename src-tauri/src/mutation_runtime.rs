//! Ephemeral ownership and transport for user-requested item mutations.
//!
//! `operations` owns plans, filesystem semantics, and results. This module
//! owns only one live claim, its identity and cancellation flag, plus the
//! coalesced progress/terminal events shared by delete and destination
//! batches. Nothing here survives a process exit or represents durable intent.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Active {
    id: u64,
    cancelled: Arc<AtomicBool>,
}

struct Runtime {
    active: Mutex<Option<Active>>,
    idle: Condvar,
    shutting_down: AtomicBool,
}

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime {
    active: Mutex::new(None),
    idle: Condvar::new(),
    shutting_down: AtomicBool::new(false),
});

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
        if let Ok(mut active) = RUNTIME.active.lock() {
            if active.as_ref().map(|entry| entry.id) == Some(self.id) {
                *active = None;
                RUNTIME.idle.notify_all();
            }
        }
    }
}

fn begin() -> Result<Claim, String> {
    if RUNTIME.shutting_down.load(Ordering::SeqCst) {
        return Err("OneCopy is closing; no new file operation can start.".to_string());
    }
    let mut active = RUNTIME
        .active
        .lock()
        .map_err(|_| "file-operation state is unavailable".to_string())?;
    if RUNTIME.shutting_down.load(Ordering::SeqCst) {
        return Err("OneCopy is closing; no new file operation can start.".to_string());
    }
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

pub(crate) fn begin_rebuild() -> Result<impl Drop, String> {
    begin()
}

pub(crate) fn request_cancel(id: u64) -> bool {
    let Ok(active) = RUNTIME.active.lock() else {
        return false;
    };
    let Some(active) = active.as_ref().filter(|active| active.id == id) else {
        return false;
    };
    active.cancelled.store(true, Ordering::SeqCst);
    true
}

pub(crate) fn request_shutdown() {
    RUNTIME.shutting_down.store(true, Ordering::SeqCst);
    if let Ok(active) = RUNTIME.active.lock() {
        if let Some(active) = active.as_ref() {
            active.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

pub(crate) fn wait_for_idle() {
    let Ok(mut active) = RUNTIME.active.lock() else {
        return;
    };
    while active.is_some() {
        match RUNTIME.idle.wait(active) {
            Ok(next) => active = next,
            Err(_) => return,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Delete,
    DestinationCopy,
    DestinationMove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    Planning,
    Deleting,
    Delivering,
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
    current_file_bytes_done: Option<u64>,
    current_file_bytes_total: Option<u64>,
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
            crate::failure_runtime::emit_or_record(&self.app, "mutation://progress", progress);
        }
    }

    fn done(&mut self, progress: &Progress, cancelled: bool) {
        self.progress(progress);
        crate::failure_runtime::emit_or_record(
            &self.app,
            "mutation://done",
            json!({ "progress": progress, "cancelled": cancelled }),
        );
    }

    fn error(&self, operation_id: u64, kind: Kind, error: &str) {
        crate::failure_runtime::emit_or_record(
            &self.app,
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
    let _index = crate::scan_runtime::begin_foreground(app);
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
        current_file_bytes_done: None,
        current_file_bytes_total: None,
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
            let data_root = crate::paths::data_root(app)?;
            let conn =
                crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
            let cache =
                crate::preview::CachePaths::new(data_root.join(crate::storage::CACHE_DIR_NAME));
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
                            current_file_bytes_done: None,
                            current_file_bytes_total: None,
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
                            current_file_bytes_done: None,
                            current_file_bytes_total: None,
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
                current_file_bytes_done: None,
                current_file_bytes_total: None,
                next_phase: None,
            };
            publisher.done(&terminal, outcome.cancelled);
        }
        Err(error) => publisher.error(operation_id, Kind::Delete, error),
    }
    result
}

pub(crate) fn move_items_out(
    app: &AppHandle,
    mut items: Vec<crate::operations::ItemIdentity>,
    dest_dir: String,
    mode: String,
) -> Result<crate::operations::MoveBatchOutcome, String> {
    let mode = match mode.as_str() {
        "move-trash-rest" => crate::operations::MoveOutMode::MoveTrashRest,
        "move-delete-rest" => crate::operations::MoveOutMode::MoveDeleteRest,
        "copy" => crate::operations::MoveOutMode::CopyKeepAll,
        other => return Err(format!("unknown move-out mode: {other}")),
    };
    let kind = if mode == crate::operations::MoveOutMode::CopyKeepAll {
        Kind::DestinationCopy
    } else {
        Kind::DestinationMove
    };
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
    let mutation = begin()?;
    let _index = crate::scan_runtime::begin_foreground(app);
    let operation_id = mutation.id();
    let mut publisher = Publisher::new(app);
    let mut last_progress = Progress {
        operation_id,
        kind,
        phase: Phase::Planning,
        items_done: 0,
        items_total: items.len() as u64,
        files_done: 0,
        files_total: 0,
        bytes_done: 0,
        bytes_total: 0,
        failures: 0,
        current_file_bytes_done: None,
        current_file_bytes_total: None,
        next_phase: Some(Phase::Delivering),
    };
    publisher.progress(&last_progress);
    let result = crate::logging::boundary(
        "move_items_out",
        json!({
            "items": items.len(),
            "destDir": dest_dir,
            "mode": mode_string(mode),
            "operationId": operation_id,
        }),
        || {
            if items.is_empty() {
                return Ok(crate::operations::MoveBatchOutcome::default());
            }
            let keys = items
                .iter()
                .map(crate::operations::ItemIdentity::media_key)
                .collect::<Result<Vec<_>, _>>()?;
            let _media = crate::media_use::begin(app, &keys)?;
            let data_root = crate::paths::data_root(app)?;
            let config = crate::storage::read_config_for_setup(&data_root)?;
            let settings = crate::scanner::settings_from_config(config.as_ref(), &data_root, 0);
            let destination = std::path::Path::new(&dest_dir);
            for source in &settings.source_dirs {
                if crate::path_identity::directory_is_within(
                    destination,
                    std::path::Path::new(source),
                )? {
                    return Err(format!(
                        "destination {dest_dir} lies inside the scanned directory {source}; move-out targets must be outside every source directory"
                    ));
                }
            }
            let conn =
                crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
            let cache =
                crate::preview::CachePaths::new(data_root.join(crate::storage::CACHE_DIR_NAME));
            crate::operations::move_batch(
                &conn,
                &data_root,
                &cache,
                &items,
                destination,
                mode,
                &|| mutation.cancelled(),
                |progress| {
                    last_progress = match progress {
                        crate::operations::MoveBatchProgress::Planning {
                            items_done,
                            items_total,
                            files_total,
                            bytes_total,
                            current_file_bytes_done,
                            current_file_bytes_total,
                        } => Progress {
                            operation_id,
                            kind,
                            phase: Phase::Planning,
                            items_done,
                            items_total,
                            files_done: 0,
                            files_total,
                            bytes_done: 0,
                            bytes_total,
                            failures: 0,
                            current_file_bytes_done,
                            current_file_bytes_total,
                            next_phase: Some(Phase::Delivering),
                        },
                        crate::operations::MoveBatchProgress::Delivering {
                            items_done,
                            items_total,
                            files_done,
                            files_total,
                            bytes_done,
                            bytes_total,
                            failures,
                            current_file_bytes_done,
                            current_file_bytes_total,
                        } => Progress {
                            operation_id,
                            kind,
                            phase: Phase::Delivering,
                            items_done,
                            items_total,
                            files_done,
                            files_total,
                            bytes_done,
                            bytes_total,
                            failures,
                            current_file_bytes_done,
                            current_file_bytes_total,
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
                "exported": outcome.exported,
                "conflicts": outcome.conflicts.len(),
                "undelivered": outcome.undelivered.len(),
            })
        },
    );
    match &result {
        Ok(outcome) => {
            let terminal = Progress {
                operation_id,
                kind,
                phase: Phase::Complete,
                items_done: outcome.items.len() as u64,
                items_total: last_progress.items_total,
                files_done: last_progress.files_done,
                files_total: outcome.files_total,
                bytes_done: last_progress.bytes_done,
                bytes_total: outcome.bytes_total,
                failures: last_progress.failures,
                current_file_bytes_done: None,
                current_file_bytes_total: None,
                next_phase: None,
            };
            publisher.done(&terminal, outcome.cancelled);
        }
        Err(error) => publisher.error(operation_id, kind, error),
    }
    result
}

fn mode_string(mode: crate::operations::MoveOutMode) -> &'static str {
    match mode {
        crate::operations::MoveOutMode::MoveTrashRest => "move-trash-rest",
        crate::operations::MoveOutMode::MoveDeleteRest => "move-delete-rest",
        crate::operations::MoveOutMode::CopyKeepAll => "copy",
    }
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
