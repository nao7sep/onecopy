//! Runtime ownership for the one index pipeline: claim, worker lifetime,
//! startup resume probes, progress events, cooperative cancellation, and join.
//! Scanner owns index semantics; this module owns only ephemeral execution.

use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter};

static RUNNING: AtomicBool = AtomicBool::new(false);
static WORKER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
// Every index-producing workflow shares this one ephemeral owner. The scan
// worker, watcher repair, section rescan, and settings re-resolution may use
// different connections and threads, but their semantic pipelines must never
// interleave. SQLite serializes individual writes; this serializes the whole
// fact-to-projection operation.
static INDEXING: Mutex<()> = Mutex::new(());
static ACTIVE_RECHECK_ISSUE: Mutex<Option<i64>> = Mutex::new(None);

struct RunningClaim<'a>(&'a AtomicBool);

impl Drop for RunningClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
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
    if RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    let prepared = (|| -> Result<(), String> {
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
            let _running = RunningClaim(&RUNNING);
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
            let last_phase = Cell::new(None::<crate::scanner::ScanPhase>);
            let last_emit = Cell::new(Instant::now() - Duration::from_secs(1));
            let emit_progress = |progress: crate::scanner::ScanProgress| {
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
            };
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
        Err(error) => {
            RUNNING.store(false, Ordering::SeqCst);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{with_index_claim, RunningClaim};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn running_claim_releases_during_unwind() {
        let running = AtomicBool::new(true);
        let _ = std::panic::catch_unwind(|| {
            let _claim = RunningClaim(&running);
            panic!("worker stopped unexpectedly");
        });
        assert!(!running.load(Ordering::SeqCst));
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
}

pub fn request_cancel() -> bool {
    let was_running = running();
    crate::scanner::SCAN_CANCEL.store(true, Ordering::Relaxed);
    was_running
}

pub fn join() {
    if let Some(worker) = WORKER.lock().ok().and_then(|mut slot| slot.take()) {
        let _ = worker.join();
    }
}
