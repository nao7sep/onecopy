//! Runtime ownership for the one index pipeline: claim, worker lifetime,
//! startup resume probes, progress events, cooperative cancellation, and join.
//! Scanner owns index semantics; this module owns only ephemeral execution.

use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter};

static RUNNING: AtomicBool = AtomicBool::new(false);
static RUNNING_WAIT: std::sync::LazyLock<(Mutex<()>, Condvar)> =
    std::sync::LazyLock::new(|| (Mutex::new(()), Condvar::new()));
static WORKER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
// Every index-producing workflow shares this one ephemeral owner. The scan
// worker, watcher repair, section rescan, and settings re-resolution may use
// different connections and threads, but their semantic pipelines must never
// interleave. SQLite serializes individual writes; this serializes the whole
// fact-to-projection operation.
static INDEXING: Mutex<()> = Mutex::new(());
static ACTIVE_RECHECK_ISSUE: Mutex<Option<i64>> = Mutex::new(None);

struct RunningClaim;

impl Drop for RunningClaim {
    fn drop(&mut self) {
        let _wait_guard = RUNNING_WAIT
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        RUNNING.store(false, Ordering::SeqCst);
        RUNNING_WAIT.1.notify_all();
    }
}

fn try_running_claim() -> Option<RunningClaim> {
    let _wait_guard = RUNNING_WAIT
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // Reset while reservation and cancellation share the wait lock, so a
        // Cancel accepted just after admission cannot be overwritten by the
        // operation's startup path.
        crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
        Some(RunningClaim)
    } else {
        None
    }
}

fn wait_running_claim() -> RunningClaim {
    loop {
        if let Some(claim) = try_running_claim() {
            return claim;
        }
        let mut guard = RUNNING_WAIT
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while running() {
            guard = RUNNING_WAIT
                .1
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn with_index_claim<T>(work: impl FnOnce() -> T) -> T {
    let _claim = INDEXING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    work()
}

struct RecheckClaim;

impl Drop for RecheckClaim {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_RECHECK_ISSUE.lock() {
            *active = None;
        }
    }
}

/// Runs one issue-anchored filesystem probe only when the index pipeline is
/// idle. Rechecks never wait in a shadow queue: a busy pipeline is an honest
/// `None`, while an admitted probe publishes its issue id for read projection.
pub fn try_with_recheck_claim<T>(issue_id: i64, work: impl FnOnce() -> T) -> Option<T> {
    if running() {
        return None;
    }
    let _index = INDEXING.try_lock().ok()?;
    // `start` publishes RUNNING before its worker tries the index claim. Check
    // again after admission so a recheck cannot slip ahead of that worker and
    // then lose the resume request when `start` honestly returns false.
    if running() {
        return None;
    }
    crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    if let Ok(mut active) = ACTIVE_RECHECK_ISSUE.lock() {
        *active = Some(issue_id);
    } else {
        return None;
    }
    let _active = RecheckClaim;
    Some(work())
}

pub fn active_recheck_issue() -> Option<i64> {
    ACTIVE_RECHECK_ISSUE
        .lock()
        .ok()
        .and_then(|active| *active)
}

/// `(resume_wanted, needs_walk)`. A cancelled walk can leave directories
/// without rows, so walk debt must be checked before row-level tail debt.
pub fn resume_plan(data_root: &Path) -> (bool, bool) {
    let Ok(conn) = crate::index_store::open(
        &data_root.join(crate::storage::INDEX_DB_FILE_NAME),
    ) else {
        return (false, false);
    };
    let roots = match crate::storage::load_config_source_dirs(data_root) {
        Ok(roots) => roots,
        Err(error) => {
            crate::logging::warn(
                "source dirs unreadable for resume",
                json!({ "error": { "message": error } }),
            );
            Vec::new()
        }
    };
    let needs_walk = !roots.is_empty()
        && match crate::scanner::walk_owed(&conn, &roots) {
            Ok(owed) => owed,
            Err(error) => {
                crate::logging::warn(
                    "walk-owed probe failed",
                    json!({ "error": { "message": error } }),
                );
                false
            }
        };
    if needs_walk {
        return (true, true);
    }
    match crate::scanner::pending_index_work_exists(&conn) {
        Ok(pending) => (pending, false),
        Err(error) => {
            crate::logging::warn(
                "pending-work probe failed",
                json!({ "error": { "message": error } }),
            );
            (false, false)
        }
    }
}

/// Starts the one worker. `include_walk` selects full walk+tail versus a
/// checkpointed tail resume; a second start is an honest `false` no-op.
pub fn start(app: AppHandle, include_walk: bool) -> Result<bool, String> {
    let Some(running_claim) = try_running_claim() else {
        return Ok(false);
    };
    let prepared = (move || -> Result<(), String> {
        let data_root = crate::paths::data_root(&app)?;
        let config = crate::storage::read_config_for_setup(&data_root)?;
        let settings = crate::scanner::settings_from_config(
            config.as_ref(),
            &data_root,
            chrono::Utc::now().timestamp_millis(),
        );
        let db_file = data_root.join(crate::storage::INDEX_DB_FILE_NAME);
        let handle = app.clone();

        let worker = std::thread::spawn(move || {
            // A worker-thread panic does not terminate the app. The claim must
            // therefore release on unwind as well as every ordinary outcome.
            let _running = running_claim;
            let _awake = settings.keep_awake.then(|| {
                keepawake::Builder::default()
                    .idle(true)
                    .sleep(true)
                    .reason("Indexing media")
                    .app_name("OneCopy")
                    .create()
                    .ok()
            });
            // The scanner may report each durable item and each streamed hash
            // chunk. Transport at human cadence while always publishing phase
            // boundaries and completed totals; this keeps the UI honest
            // without flooding the webview on a fast million-row pass.
            let emit_progress = progress_emitter(handle.clone());
            let outcome = with_index_claim(|| {
                crate::index_store::open(&db_file).and_then(|conn| {
                    if include_walk {
                        crate::scanner::run_full_scan(&conn, &settings, &emit_progress)
                    } else {
                        let mut summary = crate::scanner::ScanSummary::default();
                        crate::scanner::run_index_tail(
                            &conn,
                            &settings,
                            &emit_progress,
                            &mut summary,
                        )
                        .map(|()| summary)
                    }
                })
            });
            match outcome {
                Ok(summary) => {
                    crate::logging::info("scan complete", json!({ "summary": summary }));
                    let _ = handle.emit("scan://done", json!({ "summary": summary }));
                    crate::derived_work::wake(true);
                }
                Err(error) if error == crate::scanner::CANCELLED => {
                    crate::logging::info(
                        "scan cancelled",
                        json!({ "resumesAtNextLaunch": true }),
                    );
                    let _ = handle.emit("scan://done", json!({ "cancelled": true }));
                }
                Err(error) => {
                    crate::logging::error(
                        "scan failed",
                        json!({ "error": { "message": error.clone() } }),
                    );
                    let _ = handle.emit("scan://error", json!({ "message": error }));
                }
            }
        });
        if let Ok(mut slot) = WORKER.lock() {
            *slot = Some(worker);
        }
        Ok(())
    })();
    match prepared {
        Ok(()) => Ok(true),
        Err(error) => Err(error),
    }
}

/// Runs a user-started scoped index operation through the same reservation,
/// cancellation flag, coalesced progress transport, and terminal events as a
/// full scan. It waits behind an already-admitted scan instead of creating a
/// second queue or allowing settings/index repair to interleave with it.
pub fn run_inline<T>(
    app: &AppHandle,
    work: impl FnOnce(&dyn Fn(crate::scanner::ScanProgress)) -> Result<T, String>,
) -> Result<T, String> {
    let _running = wait_running_claim();
    let _ = app.emit("scan://waiting", json!({}));
    let emit_progress = progress_emitter(app.clone());
    let outcome = with_index_claim(|| work(&emit_progress));
    match &outcome {
        Ok(_) => {
            let _ = app.emit("scan://done", json!({}));
        }
        Err(error) if error == crate::scanner::CANCELLED => {
            let _ = app.emit("scan://done", json!({ "cancelled": true }));
        }
        Err(error) => {
            let _ = app.emit("scan://error", json!({ "message": error }));
        }
    }
    outcome
}

fn progress_emitter(handle: AppHandle) -> impl Fn(crate::scanner::ScanProgress) {
    // The scanner may report each durable item and each streamed hash chunk.
    // Transport at human cadence while always publishing phase boundaries and
    // completed totals, shared by full and scoped user-started index work.
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
            let _ = handle.emit("scan://progress", progress);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{running, try_running_claim, wait_running_claim, with_index_claim};

    static RESERVATION_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn running_claim_releases_during_unwind() {
        let _serial = RESERVATION_TEST.lock().unwrap();
        let claim = try_running_claim().unwrap();
        let _ = std::panic::catch_unwind(|| {
            let _claim = claim;
            panic!("worker stopped unexpectedly");
        });
        assert!(!running());
    }

    #[test]
    fn one_index_claim_serializes_whole_workflows() {
        let (first_entered_tx, first_entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (second_attempted_tx, second_attempted_rx) = std::sync::mpsc::channel();
        let (second_entered_tx, second_entered_rx) = std::sync::mpsc::channel();

        let first = std::thread::spawn(move || {
            with_index_claim(|| {
                first_entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        first_entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let second = std::thread::spawn(move || {
            second_attempted_tx.send(()).unwrap();
            with_index_claim(|| second_entered_tx.send(()).unwrap());
        });
        second_attempted_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert!(second_entered_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        release_tx.send(()).unwrap();
        second_entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        first.join().unwrap();
        second.join().unwrap();
    }

    #[test]
    fn inline_reservation_waits_without_racing_an_admitted_scan() {
        let _serial = RESERVATION_TEST.lock().unwrap();
        let first = try_running_claim().unwrap();
        let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _second = wait_running_claim();
            acquired_tx.send(()).unwrap();
        });

        assert!(acquired_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        drop(first);
        acquired_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        waiter.join().unwrap();
        assert!(!running());
    }

    #[test]
    fn cancellation_belongs_to_exactly_one_reservation() {
        let _serial = RESERVATION_TEST.lock().unwrap();
        let first = try_running_claim().unwrap();
        assert!(super::request_cancel());
        assert!(crate::scanner::SCAN_CANCEL.load(std::sync::atomic::Ordering::SeqCst));
        drop(first);

        let second = try_running_claim().unwrap();
        assert!(!crate::scanner::SCAN_CANCEL.load(std::sync::atomic::Ordering::SeqCst));
        drop(second);
        assert!(!super::request_cancel());
    }
}

pub fn request_cancel() -> bool {
    let _wait_guard = RUNNING_WAIT
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !running() {
        return false;
    }
    crate::scanner::SCAN_CANCEL.store(true, Ordering::Relaxed);
    true
}

pub fn join() {
    if let Some(worker) = WORKER.lock().ok().and_then(|mut slot| slot.take()) {
        let _ = worker.join();
    }
}
