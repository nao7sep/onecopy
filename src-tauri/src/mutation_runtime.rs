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
const STATE_UNAVAILABLE: &str =
    "File-operation state is unavailable. Restart OneCopy before changing files.";

struct Active {
    id: u64,
    cancelled: Arc<AtomicBool>,
}

struct Runtime {
    active: Mutex<Option<Active>>,
    idle: Condvar,
}

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| Runtime {
    active: Mutex::new(None),
    idle: Condvar::new(),
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
        let mut active = match RUNTIME.active.lock() {
            Ok(active) => active,
            Err(poisoned) => {
                crate::logging::error("file-operation state was recovered", json!({}));
                poisoned.into_inner()
            }
        };
        if active.as_ref().map(|entry| entry.id) == Some(self.id) {
            *active = None;
            RUNTIME.idle.notify_all();
        }
    }
}

fn begin() -> Result<Claim, String> {
    if crate::app_lifecycle::shutting_down() {
        return Err("OneCopy is closing; no new file operation can start.".to_string());
    }
    let mut active = RUNTIME
        .active
        .lock()
        .map_err(|_| STATE_UNAVAILABLE.to_string())?;
    if crate::app_lifecycle::shutting_down() {
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

fn begin_reported(app: &AppHandle) -> Result<Claim, String> {
    begin().map_err(|error| {
        if error == STATE_UNAVAILABLE {
            crate::failure_runtime::report(app, "file-operation-state-failed", None, &error)
                .err()
                .unwrap_or(error)
        } else {
            error
        }
    })
}

/// Rebuilds the library index. The mutation claim makes the contract
/// race-free: an active file operation rejects the rebuild, and no new one can
/// begin while the reconstructible database facts are cleared. Unlike a file
/// operation's [`admit`], the index and media claims answer busy when
/// background work does not stop within the foreground deadline, like other
/// Settings actions, and no source file is touched, so the volume-substitution
/// gate does not apply.
pub(crate) fn rebuild_index(
    app: &AppHandle,
    discard_previews: bool,
    discard_transcripts: bool,
) -> Result<(), String> {
    let _rebuild = begin_reported(app)?;
    crate::scan_runtime::run_foreground(app, || {
        let deadline = Instant::now() + crate::scan_runtime::FOREGROUND_DEADLINE;
        let _media = crate::media_use::begin(app, &[], &|| Instant::now() >= deadline).map_err(
            |error| {
                if error == crate::scanner::CANCELLED {
                    crate::scan_runtime::BUSY.to_string()
                } else {
                    error
                }
            },
        )?;
        crate::scan_runtime::restart_source_walks();
        let data_root = crate::paths::data_root()?;
        crate::preview::purge_for_rebuild(
            &crate::preview::CachePaths::new(data_root.join(crate::storage::CACHE_DIR_NAME)),
            discard_previews,
            discard_transcripts,
        )?;
        let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
        crate::index_store::clear_reconstructible(&conn)?;
        crate::notifications::clear_active(app)
    })?;
    let _ = crate::source_check_runtime::start(app.clone())?;
    Ok(())
}

/// Whether a file-changing operation or rebuild currently owns the claim.
pub(crate) fn active() -> bool {
    match RUNTIME.active.lock() {
        Ok(active) => active.is_some(),
        Err(poisoned) => poisoned.into_inner().is_some(),
    }
}

pub(crate) fn request_cancel(id: u64) -> Result<bool, String> {
    let active = match RUNTIME.active.lock() {
        Ok(active) => active,
        Err(poisoned) => {
            let active = poisoned.into_inner();
            if let Some(active) = active.as_ref().filter(|active| active.id == id) {
                active.cancelled.store(true, Ordering::SeqCst);
            }
            return Err(
                "file-operation state was recovered; cancellation was requested".to_string(),
            );
        }
    };
    let Some(active) = active.as_ref().filter(|active| active.id == id) else {
        return Ok(false);
    };
    active.cancelled.store(true, Ordering::SeqCst);
    Ok(true)
}

pub(crate) fn request_active_cancel() -> Result<bool, String> {
    let active = match RUNTIME.active.lock() {
        Ok(active) => active,
        Err(poisoned) => {
            let active = poisoned.into_inner();
            if let Some(active) = active.as_ref() {
                active.cancelled.store(true, Ordering::SeqCst);
            }
            return Err(
                "file-operation state was recovered; cancellation was requested".to_string(),
            );
        }
    };
    let Some(active) = active.as_ref() else {
        return Ok(false);
    };
    active.cancelled.store(true, Ordering::SeqCst);
    Ok(true)
}

pub(crate) fn request_shutdown() -> Result<(), String> {
    let (active, recovered) = match RUNTIME.active.lock() {
        Ok(active) => (active, false),
        Err(poisoned) => (poisoned.into_inner(), true),
    };
    if let Some(active) = active.as_ref() {
        active.cancelled.store(true, Ordering::SeqCst);
    }
    if recovered {
        Err("file-operation state was recovered during shutdown".to_string())
    } else {
        Ok(())
    }
}

/// The outcome of bounded exit-time mutation quiescence: either the claim
/// dropped (the operation reached a safe point, or none was active), or the
/// deadline passed first and the current file operation is given up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdleWait {
    Idle,
    TimedOut,
}

/// Waits up to `deadline` for the active mutation claim to drop, i.e. for the
/// current file operation to reach its own safe point (between physical
/// files, per `specs/file-operations.md` "Cancellation"). Normal exit calls
/// this after requesting cancellation, so the wait only lasts as long as the
/// current bounded filesystem step takes, up to `deadline`; on `IdleWait::TimedOut`
/// the caller gives up on that step (killing outstanding subprocesses) rather
/// than waiting longer, per "Normal exit and abnormal termination".
pub(crate) fn wait_for_idle(deadline: Duration) -> Result<IdleWait, String> {
    let cutoff = Instant::now() + deadline;
    let (mut active, mut recovered) = match RUNTIME.active.lock() {
        Ok(active) => (active, false),
        Err(poisoned) => (poisoned.into_inner(), true),
    };
    while active.is_some() {
        let remaining = cutoff.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return idle_wait_result(recovered, IdleWait::TimedOut);
        }
        let waited = match RUNTIME.idle.wait_timeout(active, remaining) {
            Ok(pair) => pair,
            Err(poisoned) => {
                recovered = true;
                poisoned.into_inner()
            }
        };
        active = waited.0;
        if active.is_some() && waited.1.timed_out() {
            return idle_wait_result(recovered, IdleWait::TimedOut);
        }
    }
    idle_wait_result(recovered, IdleWait::Idle)
}

fn idle_wait_result(recovered: bool, outcome: IdleWait) -> Result<IdleWait, String> {
    if recovered {
        Err("file-operation state was recovered while waiting for shutdown".to_string())
    } else {
        Ok(outcome)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Delete,
    DestinationCopy,
    DestinationMove,
    TrashEmpty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Phase {
    /// Background work is reaching its safe point before this operation
    /// can own the index and the media boundary.
    Waiting,
    Planning,
    Deleting,
    Delivering,
    Emptying,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultSummary {
    items_completed: u64,
    items_partial: u64,
    items_unstarted: u64,
    files_completed: u64,
    /// Files that failed with a known outcome.
    files_failed: u64,
    /// Files whose rename or removal was given up on while their drive was
    /// not responding: done or not, the next check settles them.
    files_unknown: u64,
    files_unstarted: u64,
    trash_available: bool,
    error: Option<String>,
}

struct Publisher {
    trace: Option<crate::activity::WorkTrace>,
    app: AppHandle,
    throttle: crate::progress_throttle::ProgressThrottle<(Phase, u64)>,
}

impl Publisher {
    fn new(app: &AppHandle) -> Self {
        Self {
            app: app.clone(),
            trace: None,
            throttle: Default::default(),
        }
    }

    fn progress(&mut self, progress: &Progress) {
        if self.trace.is_none() {
            use crate::activity::{ActivityOwner, ActivitySubject, WorkTrace};
            self.trace = Some(WorkTrace::begin(ActivityOwner::Mutation, Some(match progress.kind {
                Kind::Delete => ActivitySubject::DeleteFiles,
                Kind::DestinationCopy => ActivitySubject::CopyFiles,
                Kind::DestinationMove => ActivitySubject::MoveFiles,
                Kind::TrashEmpty => ActivitySubject::EmptyDeletedFiles,
            }), None));
        }
        if let Some(trace) = &self.trace { trace.progress(progress.files_done, progress.files_total); }
        let completed = progress.phase == Phase::Complete
            || (progress.items_done == progress.items_total
                && progress.files_done == progress.files_total);
        if self
            .throttle
            .admit((progress.phase, progress.failures), completed, Instant::now())
        {
            crate::failure_runtime::emit_or_record(&self.app, "mutation://progress", progress);
        }
    }

    fn done(&mut self, progress: &Progress, cancelled: bool, summary: Option<ResultSummary>) {
        self.progress(progress);
        if let Some(trace) = &mut self.trace {
            use crate::activity::ActivityState;
            trace.finish(if cancelled { ActivityState::Cancelled }
                else if summary.as_ref().is_some_and(|result| result.files_unknown > 0) { ActivityState::OutcomeUnknown }
                else if summary.as_ref().is_some_and(|result| result.files_failed > 0 || result.error.is_some()) { ActivityState::Failed }
                else if summary.is_none() { ActivityState::Idle }
                else { ActivityState::Succeeded }, None);
        }
        crate::failure_runtime::emit_or_record(
            &self.app,
            "mutation://done",
            json!({ "progress": progress, "cancelled": cancelled, "summary": summary }),
        );
    }

    fn error(&mut self, progress: &Progress, error: &str) {
        if let Some(trace) = &mut self.trace { trace.finish(crate::activity::ActivityState::Failed, None); }
        let started = progress.items_done.saturating_add(u64::from(
            progress.phase != Phase::Planning && progress.items_done < progress.items_total,
        ));
        let summary = result_summary(
            progress.items_total,
            started,
            progress.items_done,
            progress.files_total,
            progress.files_done,
            progress.failures,
            0,
            false,
            Some(error.to_string()),
        );
        crate::failure_runtime::emit_or_record(
            &self.app,
            "mutation://error",
            json!({
                "operationId": progress.operation_id,
                "kind": progress.kind,
                "error": error,
                "summary": summary,
            }),
        );
    }
}

fn result_summary(
    items_total: u64,
    items_started: u64,
    items_completed: u64,
    files_total: u64,
    files_done: u64,
    files_failed: u64,
    files_unknown: u64,
    trash_available: bool,
    error: Option<String>,
) -> ResultSummary {
    ResultSummary {
        items_completed,
        items_partial: items_started.saturating_sub(items_completed),
        items_unstarted: items_total.saturating_sub(items_started),
        files_completed: files_done.saturating_sub(files_failed),
        files_failed: files_failed.saturating_sub(files_unknown),
        files_unknown,
        files_unstarted: files_total.saturating_sub(files_done),
        trash_available,
        error,
    }
}

/// The index claim, then the media boundary on the operation's files. Both
/// wait as long as background work needs to reach its safe point; the
/// operation shows that it is waiting, and its Cancel ends the wait with no
/// filesystem work. Fields drop in order: media first, then the index.
struct Admitted {
    _media: crate::media_use::Guard,
    _index: crate::scan_runtime::ForegroundGuard,
}

/// The configured source directories an accepted batch's files actually sit
/// under, so the volume-substitution gate can be scoped to only the roots a
/// mutation touches (R3-07, R1-14): a source that failed verification never
/// blocks a batch that never reads or writes under it.
fn touched_source_dirs(
    source_dirs: &[String],
    accepted: &crate::operations::AcceptedFiles,
) -> Vec<String> {
    source_dirs
        .iter()
        .filter(|dir| {
            accepted
                .abs_paths()
                .any(|path| crate::scanner::directory_belongs_to_root(path, dir))
        })
        .cloned()
        .collect()
}

fn admit(
    app: &AppHandle,
    mutation: &Claim,
    keys: &[String],
    accepted: &crate::operations::AcceptedFiles,
    on_wait: &mut dyn FnMut(),
) -> Result<Option<Admitted>, String> {
    // The volume-substitution gate guards every destructive path it is
    // documented for, not only the frontend's own recheck (R1-14): a backup
    // drive swapped mid-session at the same mount path must not be walked or
    // mutated under the original drive's rows. A failed check keeps this gate
    // closed (R3-07) — the `?` below refuses admission rather than treating
    // an unreadable check as "nothing recorded". Scoped to the configured
    // roots this batch's accepted files actually sit under, so a source that
    // failed verification never blocks a batch that never touches it.
    let data_root = crate::paths::data_root()?;
    let source_dirs = crate::storage::load_config_source_dirs(&data_root)?;
    let touched_dirs = touched_source_dirs(&source_dirs, accepted);
    crate::volume::enforce_no_substitution(&data_root, &touched_dirs)?;
    let cancelled = || mutation.cancelled();
    let Some(index) = crate::scan_runtime::begin_admitted_mutation(app, &cancelled, on_wait)?
    else {
        return Ok(None);
    };
    match crate::media_use::begin(app, keys, &cancelled) {
        Ok(media) => Ok(Some(Admitted {
            _media: media,
            _index: index,
        })),
        Err(_) if mutation.cancelled() => Ok(None),
        Err(error) => Err(error),
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
    let mutation = begin_reported(app)?;
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
    let result = crate::logging::boundary(
        "delete_items",
        json!({ "items": items.len(), "permanent": permanent, "operationId": operation_id }),
        || {
            if items.is_empty() {
                publisher.progress(&last_progress);
                return Ok(crate::operations::DeleteBatchOutcome::default());
            }
            let keys = items
                .iter()
                .map(crate::operations::ItemIdentity::key)
                .collect::<Result<Vec<_>, _>>()?;
            // Accepting the confirmation freezes the physical files before
            // admission may wait for background work to yield.
            let data_root = crate::paths::data_root()?;
            let conn =
                crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
            let accepted = crate::operations::AcceptedFiles::capture(&conn, &items)?;
            let waiting = Progress {
                phase: Phase::Waiting,
                next_phase: Some(Phase::Planning),
                ..last_progress.clone()
            };
            let Some(_admitted) =
                admit(app, &mutation, &keys, &accepted, &mut || publisher.progress(&waiting))?
            else {
                return Ok(crate::operations::DeleteBatchOutcome {
                    cancelled: true,
                    ..Default::default()
                });
            };
            publisher.progress(&last_progress);
            let cache =
                crate::preview::CachePaths::new(data_root.join(crate::storage::CACHE_DIR_NAME));
            let mode = if permanent {
                crate::operations::DeleteMode::Permanent
            } else {
                crate::operations::DeleteMode::Trash
            };
            crate::operations::delete_accepted_batch(
                &conn,
                &data_root,
                &cache,
                &items,
                &accepted,
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
            let items_completed = if last_progress.phase == Phase::Deleting {
                last_progress.items_done
            } else {
                0
            };
            let terminal = Progress {
                operation_id,
                kind: Kind::Delete,
                phase: Phase::Complete,
                items_done: items_completed,
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
            publisher.done(
                &terminal,
                outcome.cancelled,
                Some(result_summary(
                    terminal.items_total,
                    outcome.items_started,
                    items_completed,
                    terminal.files_total,
                    terminal.files_done,
                    terminal.failures,
                    outcome.unknown_files,
                    !permanent && outcome.deleted_files > 0,
                    outcome.error.clone(),
                )),
            );
        }
        Err(error) => publisher.error(&last_progress, error),
    }
    result
}

pub(crate) fn move_items_out(
    app: &AppHandle,
    mut items: Vec<crate::operations::ItemIdentity>,
    dest_dir: String,
    mode: crate::operations::MoveOutMode,
    conflict_policy: Option<crate::operations::DestinationConflictPolicy>,
    plan_token: Option<String>,
) -> Result<crate::operations::MoveBatchOutcome, String> {
    let kind = if mode == crate::operations::MoveOutMode::CopyKeepAll {
        Kind::DestinationCopy
    } else {
        Kind::DestinationMove
    };
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
    let mutation = begin_reported(app)?;
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
    let result = crate::logging::boundary(
        "move_items_out",
        json!({
            "items": items.len(),
            "destDir": dest_dir,
            "mode": mode,
            "operationId": operation_id,
        }),
        || {
            if items.is_empty() {
                publisher.progress(&last_progress);
                return Ok(crate::operations::MoveBatchOutcome::default());
            }
            let keys = items
                .iter()
                .map(crate::operations::ItemIdentity::key)
                .collect::<Result<Vec<_>, _>>()?;
            // Accepting the confirmation freezes the physical files before
            // admission may wait for background work to yield.
            let data_root = crate::paths::data_root()?;
            let conn =
                crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
            let accepted = crate::operations::AcceptedFiles::capture(&conn, &items)?;
            let waiting = Progress {
                phase: Phase::Waiting,
                next_phase: Some(Phase::Planning),
                ..last_progress.clone()
            };
            let Some(_admitted) =
                admit(app, &mutation, &keys, &accepted, &mut || publisher.progress(&waiting))?
            else {
                return Ok(crate::operations::MoveBatchOutcome {
                    cancelled: true,
                    ..Default::default()
                });
            };
            publisher.progress(&last_progress);
            let config = crate::storage::read_config_for_setup(&data_root)?;
            let rename_style = match config
                .as_ref()
                .and_then(|value| value.get("destinationConflictRenameStyle"))
                .and_then(serde_json::Value::as_str)
            {
                Some("parenthesized-number") => {
                    crate::operations::DestinationRenameStyle::ParenthesizedNumber
                }
                _ => crate::operations::DestinationRenameStyle::SpaceNumber,
            };
            // Destination admission belongs to the operation itself.
            let destination = std::path::Path::new(&dest_dir);
            let cache =
                crate::preview::CachePaths::new(data_root.join(crate::storage::CACHE_DIR_NAME));
            crate::operations::move_batch_reviewed(
                &conn,
                &data_root,
                &cache,
                &items,
                &accepted,
                destination,
                mode,
                conflict_policy,
                plan_token.as_deref(),
                rename_style,
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
            let items_completed = if last_progress.phase == Phase::Delivering {
                last_progress.items_done
            } else {
                0
            };
            let terminal = Progress {
                operation_id,
                kind,
                phase: Phase::Complete,
                items_done: items_completed,
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
            let show_result = !outcome.requires_conflict_choice && !outcome.plan_changed;
            publisher.done(
                &terminal,
                outcome.cancelled,
                show_result.then(|| {
                    result_summary(
                        terminal.items_total,
                        outcome.items_started,
                        items_completed,
                        terminal.files_total,
                        terminal.files_done,
                        terminal.failures,
                        (outcome.unknown.len() as u64)
                            .saturating_add(outcome.post_action.unknown_files),
                        (mode == crate::operations::MoveOutMode::MoveTrashRest
                            && outcome.post_action.deleted_files > 0)
                            || outcome.trashed_destination_files > 0,
                        outcome.error.clone(),
                    )
                }),
            );
        }
        Err(error) => publisher.error(&last_progress, error),
    }
    result
}

pub(crate) fn empty_trash(
    app: &AppHandle,
    root: String,
    plan_token: String,
) -> Result<crate::trash::EmptyOutcome, String> {
    let mutation = begin_reported(app)?;
    let operation_id = mutation.id();
    let publisher = std::cell::RefCell::new(Publisher::new(app));
    let progress = std::cell::RefCell::new(Progress {
        operation_id,
        kind: Kind::TrashEmpty,
        phase: Phase::Planning,
        items_done: 0,
        items_total: 1,
        files_done: 0,
        files_total: 0,
        bytes_done: 0,
        bytes_total: 0,
        failures: 0,
        current_file_bytes_done: None,
        current_file_bytes_total: None,
        next_phase: Some(Phase::Emptying),
    });
    publisher.borrow_mut().progress(&progress.borrow());
    let result = crate::logging::boundary(
        "trash_empty",
        json!({ "root": root, "operationId": operation_id }),
        || {
            let data_root = crate::paths::data_root()?;
            // Scoped to the root being emptied, not every configured source:
            // a different source that failed verification never blocks
            // emptying deleted files under an unaffected root (R3-07, R1-14).
            crate::volume::enforce_no_substitution(&data_root, std::slice::from_ref(&root))?;
            let roots = crate::storage::load_config_file_roots(&data_root)?;
            let known = crate::trash::overview(&roots);
            if !known.iter().any(|candidate| candidate.root == root) {
                return Err("not a known trash root".to_string());
            }
            crate::trash::empty_root_with_progress(
                std::path::Path::new(&root),
                &plan_token,
                &mutation.cancelled,
                &|trash_progress| {
                    let next = Progress {
                        operation_id,
                        kind: Kind::TrashEmpty,
                        phase: Phase::Emptying,
                        items_done: 0,
                        items_total: 1,
                        files_done: trash_progress.done,
                        files_total: trash_progress.total,
                        bytes_done: trash_progress.bytes_done,
                        bytes_total: trash_progress.bytes_total,
                        failures: trash_progress.failures,
                        current_file_bytes_done: None,
                        current_file_bytes_total: None,
                        next_phase: Some(Phase::Complete),
                    };
                    *progress.borrow_mut() = next.clone();
                    publisher.borrow_mut().progress(&next);
                    crate::failure_runtime::emit_or_record(
                        app,
                        "trash://progress",
                        json!({ "root": root, "progress": trash_progress }),
                    );
                },
                &|path, error| {
                    crate::failure_runtime::record_active(
                        app,
                        "trash-empty-entry-failed",
                        Some(&path.to_string_lossy()),
                        &format!(
                            "OneCopy couldn’t permanently remove this item from Deleted files: {error}"
                        ),
                    )
                },
            )
        },
        |outcome| {
            json!({
                "cancelled": outcome.cancelled,
                "failures": outcome.failures,
                "planChanged": outcome.plan_changed,
            })
        },
    );
    match &result {
        Ok(outcome) => {
            let latest = progress.borrow();
            // A changed location did no filesystem work; it ends like a
            // cancelled one while the Deleted files surface asks for review.
            let stopped = outcome.cancelled || outcome.plan_changed;
            let completed = !stopped && outcome.failures == 0;
            let terminal = Progress {
                operation_id,
                kind: Kind::TrashEmpty,
                phase: Phase::Complete,
                items_done: u64::from(completed),
                items_total: 1,
                files_done: latest.files_done,
                files_total: latest.files_total,
                bytes_done: latest.bytes_done,
                bytes_total: latest.bytes_total,
                failures: outcome.failures,
                current_file_bytes_done: None,
                current_file_bytes_total: None,
                next_phase: None,
            };
            publisher.borrow_mut().done(
                &terminal,
                stopped,
                Some(result_summary(
                    1,
                    1,
                    terminal.items_done,
                    terminal.files_total,
                    terminal.files_done,
                    terminal.failures,
                    0,
                    false,
                    None,
                )),
            );
        }
        Err(error) => publisher.borrow_mut().error(&progress.borrow(), error),
    }
    result
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: the process-wide mutation claim
// is private lifecycle state and has no public application contract.
#[path = "../tests/unit/mutation_runtime.rs"]
mod tests;
