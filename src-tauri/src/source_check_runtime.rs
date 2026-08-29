//! Lifecycle owner for the finite `Check source folders` job.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static WORKER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    running: bool,
    stopping: bool,
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        running: running(),
        stopping: running() && STOP_REQUESTED.load(Ordering::SeqCst),
    }
}

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn start(app: AppHandle) -> Result<bool, String> {
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(false);
    }
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    // Discovery must not sit invisibly behind an hours-long metadata tail.
    // Completion yields at the scanner's existing safe checkpoints and keeps
    // its durable queue for the wake at this worker's terminal boundary.
    crate::file_information_runtime::preempt();
    join_finished();
    let handle = app.clone();
    let (release, wait_for_registration) = std::sync::mpsc::sync_channel(0);
    let worker = std::thread::Builder::new()
        .name("onecopy-source-check".to_string())
        .spawn(move || {
            if wait_for_registration.recv().is_ok() {
                worker(handle);
            }
        })
        .map_err(|error| {
            RUNNING.store(false, Ordering::SeqCst);
            crate::file_information_runtime::wake(app.clone());
            format!("could not start source-folder check: {error}")
        })?;
    let mut slot = WORKER.lock().map_err(|_| {
        RUNNING.store(false, Ordering::SeqCst);
        "source-folder worker state is unavailable".to_string()
    })?;
    *slot = Some(worker);
    drop(slot);
    emit_state(&app);
    if release.send(()).is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        emit_state(&app);
        return Err("source-folder worker could not leave its start gate".to_string());
    }
    Ok(true)
}

fn worker(app: AppHandle) {
    let outcome = catch_unwind(AssertUnwindSafe(|| run(&app)));
    RUNNING.store(false, Ordering::SeqCst);
    let terminal = match outcome {
        Ok(Ok(summary)) => {
            RESTART_REQUESTED.store(false, Ordering::SeqCst);
            crate::logging::info(
                "source-folder check complete",
                json!({ "summary": summary }),
            );
            if let Err(error) = crate::watcher::restart_from_config(app.clone()) {
                crate::scan_runtime::record_runtime_failure(&app, "watcher-failed", &error);
            }
            json!({ "summary": summary })
        }
        Ok(Err(error)) if error == crate::scanner::CANCELLED => {
            crate::logging::info("source-folder check stopped", json!({}));
            json!({
                "stopped": !RESTART_REQUESTED.load(Ordering::SeqCst),
                "preempted": RESTART_REQUESTED.load(Ordering::SeqCst),
            })
        }
        Ok(Err(error)) => {
            RESTART_REQUESTED.store(false, Ordering::SeqCst);
            fail(&app, &error);
            json!({ "error": error })
        }
        Err(payload) => {
            RESTART_REQUESTED.store(false, Ordering::SeqCst);
            let error = panic_message(payload);
            fail(&app, &error);
            json!({ "error": error })
        }
    };
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    emit_state(&app);
    emit(&app, "source-check://done", terminal);
    // A foreground action may have preempted this worker before it acquired
    // the index claim. In that order the foreground guard finishes first, so
    // its resume attempt sees this worker as still running. Retry here after
    // publishing the terminal state; user-requested Stop clears the flag.
    resume_if_requested(app.clone());
    // A stopped or failed walk may still have committed discoveries before
    // its last safe boundary. Completion owns those durable rows regardless
    // of how the source check ended. A resumed source check has priority, so
    // wake() leaves this request queued until that check reaches its terminal.
    crate::file_information_runtime::wake(app);
}

fn run(app: &AppHandle) -> Result<crate::scanner::ScanSummary, String> {
    let data_root = crate::paths::data_root(app)?;
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = crate::scanner::settings_from_config(
        config.as_ref(),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let db_file = data_root.join(crate::storage::INDEX_DB_FILE_NAME);
    let progress = crate::scan_runtime::progress_emitter(app.clone(), "source-check://progress");
    crate::scan_runtime::with_owner(
        crate::scan_runtime::Owner::SourceCheck,
        STOP_REQUESTED.load(Ordering::SeqCst),
        || {
            crate::index_store::open(&db_file)
                .and_then(|conn| crate::scanner::run_source_check(&conn, &settings, &progress))
        },
    )
}

pub fn stop(app: &AppHandle) -> bool {
    if !running() {
        return false;
    }
    STOP_REQUESTED.store(true, Ordering::SeqCst);
    RESTART_REQUESTED.store(false, Ordering::SeqCst);
    crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::SourceCheck);
    emit_state(app);
    true
}

pub(crate) fn preempt() {
    if running() {
        RESTART_REQUESTED.store(true, Ordering::SeqCst);
        STOP_REQUESTED.store(true, Ordering::SeqCst);
        crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::SourceCheck);
    }
}

pub(crate) fn resume_if_requested(app: AppHandle) {
    if RESTART_REQUESTED.swap(false, Ordering::SeqCst) && !running() {
        if let Err(error) = start(app.clone()) {
            fail(&app, &error);
            emit(&app, "source-check://done", json!({ "error": error }));
        }
    }
}

pub fn shutdown(app: &AppHandle) {
    let _ = stop(app);
}

pub fn join() {
    if let Some(worker) = WORKER.lock().ok().and_then(|mut slot| slot.take()) {
        if worker.join().is_err() {
            crate::logging::error("source-folder worker join failed", json!({}));
        }
    }
}

fn join_finished() {
    let finished = WORKER
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .filter(|worker| worker.is_finished());
    if let Some(worker) = finished {
        let _ = worker.join();
    }
}

fn fail(app: &AppHandle, error: &str) {
    crate::logging::error(
        "source-folder check failed",
        json!({ "error": { "message": error } }),
    );
    crate::scan_runtime::record_runtime_failure(app, "source-check-failed", error);
}

fn emit_state(app: &AppHandle) {
    emit(app, "source-check://state", snapshot());
}

fn emit<T: Clone + Serialize>(app: &AppHandle, event: &str, payload: T) {
    if let Err(error) = app.emit(event, payload) {
        crate::logging::warn(
            "source-folder event failed",
            json!({ "event": event, "error": { "message": error.to_string() } }),
        );
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|value| (*value).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "source-folder worker stopped unexpectedly".to_string())
}
