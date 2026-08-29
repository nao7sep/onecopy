//! Shared admission for index-changing work.
//!
//! Source-folder checking and file-information completion own independent
//! lifecycles in their runtime modules. This module owns only the database
//! projection lock, scanner cancellation hand-off, foreground admission, and
//! issue-recheck identity used by read projections.

use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

static INDEXING: Mutex<()> = Mutex::new(());
static ACTIVE_OWNER: AtomicU8 = AtomicU8::new(0);
static FOREGROUND_WAITERS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_RECHECK_ISSUE: Mutex<Option<i64>> = Mutex::new(None);

struct ForegroundWait {
    app: AppHandle,
}

impl Drop for ForegroundWait {
    fn drop(&mut self) {
        FOREGROUND_WAITERS.fetch_sub(1, Ordering::SeqCst);
        crate::source_check_runtime::resume_if_requested(self.app.clone());
        crate::file_information_runtime::wake(self.app.clone());
    }
}

pub(crate) struct ForegroundGuard {
    active: Option<ActiveOwner>,
    index: Option<MutexGuard<'static, ()>>,
    waiting: Option<ForegroundWait>,
}

impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        self.active.take();
        self.index.take();
        self.waiting.take();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    SourceCheck = 1,
    FileInformation = 2,
    Foreground = 3,
    Watcher = 4,
}

struct ActiveOwner;

impl Drop for ActiveOwner {
    fn drop(&mut self) {
        crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
        ACTIVE_OWNER.store(0, Ordering::SeqCst);
    }
}

fn enter(owner: Owner, already_cancelled: bool) -> ActiveOwner {
    ACTIVE_OWNER.store(owner as u8, Ordering::SeqCst);
    crate::scanner::SCAN_CANCEL.store(already_cancelled, Ordering::SeqCst);
    ActiveOwner
}

pub(crate) fn request_cancel(owner: Owner) {
    if ACTIVE_OWNER.load(Ordering::SeqCst) == owner as u8 {
        crate::scanner::SCAN_CANCEL.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn with_owner<T>(owner: Owner, already_cancelled: bool, work: impl FnOnce() -> T) -> T {
    let _index = INDEXING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _active = enter(owner, already_cancelled);
    work()
}

pub(crate) fn with_watcher_claim<T>(work: impl FnOnce() -> T) -> T {
    with_owner(Owner::Watcher, false, work)
}

/// Runs a foreground index action after asking file-information completion to
/// yield. Source checking is the one documented conflict and returns busy
/// instead of placing the user's action into an invisible queue.
pub(crate) fn run_section<T>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if crate::source_check_runtime::running() {
        return Err(
            "Recheck this section is unavailable while OneCopy is checking all source folders."
                .to_string(),
        );
    }
    run_foreground(app, work)
}

pub(crate) fn run_foreground<T>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _foreground = begin_foreground(app);
    work()
}

pub(crate) fn begin_foreground(app: &AppHandle) -> ForegroundGuard {
    FOREGROUND_WAITERS.fetch_add(1, Ordering::SeqCst);
    let waiting = ForegroundWait { app: app.clone() };
    crate::source_check_runtime::preempt();
    crate::file_information_runtime::preempt();
    let index = INDEXING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let active = enter(Owner::Foreground, false);
    ForegroundGuard {
        active: Some(active),
        index: Some(index),
        waiting: Some(waiting),
    }
}

pub(crate) fn foreground_pending() -> bool {
    FOREGROUND_WAITERS.load(Ordering::SeqCst) != 0
}

struct RecheckClaim;

impl Drop for RecheckClaim {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_RECHECK_ISSUE.lock() {
            *active = None;
        }
    }
}

/// An issue recheck is deliberately non-queuing. The Issues surface already
/// has an explicit Busy result and can be retried after the active safe step.
pub fn try_with_recheck_claim<T>(issue_id: i64, work: impl FnOnce() -> T) -> Option<T> {
    if crate::source_check_runtime::running() {
        return None;
    }
    let _index = INDEXING.try_lock().ok()?;
    let _active_owner = enter(Owner::Foreground, false);
    let mut active = ACTIVE_RECHECK_ISSUE.lock().ok()?;
    *active = Some(issue_id);
    drop(active);
    let _active_recheck = RecheckClaim;
    Some(work())
}

pub fn active_recheck_issue() -> Option<i64> {
    ACTIVE_RECHECK_ISSUE.lock().ok().and_then(|active| *active)
}

pub fn running() -> bool {
    crate::source_check_runtime::running()
        || crate::file_information_runtime::running()
        || ACTIVE_OWNER.load(Ordering::SeqCst) != 0
}

pub(crate) fn progress_emitter(
    handle: AppHandle,
    event: &'static str,
) -> impl Fn(crate::scanner::ScanProgress) {
    let last_phase = Cell::new(None::<crate::scanner::ScanPhase>);
    let last_emit = Cell::new(Instant::now() - Duration::from_secs(1));
    move |progress: crate::scanner::ScanProgress| {
        let now = Instant::now();
        let phase_changed = last_phase.get() != Some(progress.phase);
        let completed = progress.done == progress.total;
        if phase_changed
            || completed
            || now.duration_since(last_emit.get()) >= Duration::from_millis(125)
        {
            last_phase.set(Some(progress.phase));
            last_emit.set(now);
            if let Err(error) = handle.emit(event, progress) {
                crate::logging::warn(
                    "index progress event failed",
                    serde_json::json!({ "event": event, "error": { "message": error.to_string() } }),
                );
            }
        }
    }
}

pub(crate) fn record_runtime_failure(app: &AppHandle, kind: &str, message: &str) {
    let saved = crate::paths::data_root(app)
        .and_then(|root| crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME)))
        .and_then(|conn| crate::index_store::upsert_issue(&conn, None, kind, message));
    if let Err(error) = saved {
        crate::logging::error(
            "background failure issue could not be saved",
            serde_json::json!({
                "kind": kind,
                "failure": { "message": message },
                "error": { "message": error },
            }),
        );
    }
}
