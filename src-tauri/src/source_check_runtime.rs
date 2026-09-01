//! Lifecycle owner for the finite `Check source folders` job.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static WORKERS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());
static LAST_RESULT: AtomicU8 = AtomicU8::new(SourceCheckResult::Stopped as u8);
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum SourceCheckResult {
    Stopped = 0,
    Completed = 1,
    Failed = 2,
}

impl SourceCheckResult {
    fn current() -> Self {
        match LAST_RESULT.load(Ordering::SeqCst) {
            1 => Self::Completed,
            2 => Self::Failed,
            _ => Self::Stopped,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    running: bool,
    stopping: bool,
    last_result: SourceCheckResult,
    event_sequence: u64,
}

pub fn snapshot() -> Snapshot {
    snapshot_at(EVENT_SEQUENCE.load(Ordering::SeqCst))
}

fn snapshot_at(event_sequence: u64) -> Snapshot {
    Snapshot {
        running: running(),
        stopping: running() && STOP_REQUESTED.load(Ordering::SeqCst),
        last_result: SourceCheckResult::current(),
        event_sequence,
    }
}

pub(crate) fn next_event_sequence() -> u64 {
    EVENT_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1
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
                worker_entry(handle);
            }
        })
        .map_err(|error| {
            RUNNING.store(false, Ordering::SeqCst);
            LAST_RESULT.store(SourceCheckResult::Failed as u8, Ordering::SeqCst);
            crate::admit_background_completion(app.clone());
            format!("could not start source-folder check: {error}")
        })?;
    let mut workers = WORKERS.lock().map_err(|_| {
        RUNNING.store(false, Ordering::SeqCst);
        LAST_RESULT.store(SourceCheckResult::Failed as u8, Ordering::SeqCst);
        crate::admit_background_completion(app.clone());
        "source-folder worker state is unavailable".to_string()
    })?;
    workers.push(worker);
    drop(workers);
    emit_state(&app);
    if release.send(()).is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        LAST_RESULT.store(SourceCheckResult::Failed as u8, Ordering::SeqCst);
        emit_state(&app);
        crate::admit_background_completion(app);
        return Err("source-folder worker could not leave its start gate".to_string());
    }
    Ok(true)
}

fn worker_entry(app: AppHandle) {
    let handle = app.clone();
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| worker(handle))) {
        RUNNING.store(false, Ordering::SeqCst);
        STOP_REQUESTED.store(false, Ordering::SeqCst);
        RESTART_REQUESTED.store(false, Ordering::SeqCst);
        LAST_RESULT.store(SourceCheckResult::Failed as u8, Ordering::SeqCst);
        let error = crate::failure_runtime::panic_message(payload);
        fail(&app, &error);
        emit_state(&app);
        emit_done(&app, json!({ "error": error }));
        crate::admit_background_completion(app);
    }
}

fn worker(app: AppHandle) {
    let outcome = catch_unwind(AssertUnwindSafe(|| run(&app)));
    let terminal = match outcome {
        Ok(Ok(summary)) => {
            RESTART_REQUESTED.store(false, Ordering::SeqCst);
            LAST_RESULT.store(SourceCheckResult::Completed as u8, Ordering::SeqCst);
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
            LAST_RESULT.store(SourceCheckResult::Stopped as u8, Ordering::SeqCst);
            crate::logging::info("source-folder check stopped", json!({}));
            json!({
                "stopped": !RESTART_REQUESTED.load(Ordering::SeqCst),
                "preempted": RESTART_REQUESTED.load(Ordering::SeqCst),
            })
        }
        Ok(Err(error)) => {
            RESTART_REQUESTED.store(false, Ordering::SeqCst);
            LAST_RESULT.store(SourceCheckResult::Failed as u8, Ordering::SeqCst);
            fail(&app, &error);
            json!({ "error": error })
        }
        Err(payload) => {
            RESTART_REQUESTED.store(false, Ordering::SeqCst);
            LAST_RESULT.store(SourceCheckResult::Failed as u8, Ordering::SeqCst);
            let error = crate::failure_runtime::panic_message(payload);
            fail(&app, &error);
            json!({ "error": error })
        }
    };
    RUNNING.store(false, Ordering::SeqCst);
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    emit_state(&app);
    emit_done(&app, terminal);
    // A foreground action may have preempted this worker before it acquired
    // the index claim. In that order the foreground guard finishes first, so
    // its resume attempt sees this worker as still running. Retry here after
    // publishing the terminal state; user-requested Stop clears the flag.
    resume_if_requested(app.clone());
    // A stopped or failed walk may still have committed discoveries before
    // its last safe boundary. Completion owns those durable rows regardless
    // of how the source check ended. A resumed source check has priority, so
    // wake() leaves this request queued until that check reaches its terminal.
    crate::admit_background_completion(app);
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
    let progress = crate::scan_runtime::progress_emitter(
        app.clone(),
        "source-check://progress",
        next_event_sequence,
    );
    let summary = crate::scan_runtime::with_owner(
        crate::scan_runtime::Owner::SourceCheck,
        STOP_REQUESTED.load(Ordering::SeqCst),
        || {
            crate::index_store::open(&db_file)
                .and_then(|conn| crate::scanner::run_source_check(&conn, &settings, &progress))
        },
    )?;
    crate::failure_runtime::clear(app, "source-check-failed", None)?;
    Ok(summary)
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
            emit_done(&app, json!({ "error": error }));
        }
    }
}

pub fn shutdown(app: &AppHandle) {
    let _ = stop(app);
}

pub fn join() {
    let workers = match WORKERS.lock() {
        Ok(mut workers) => workers.drain(..).collect::<Vec<_>>(),
        Err(_) => {
            crate::logging::error("source-folder worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in workers {
        if worker.join().is_err() {
            crate::logging::error("source-folder worker join failed", json!({}));
        }
    }
}

fn join_finished() {
    let finished = match WORKERS.lock() {
        Ok(mut workers) => {
            let mut finished = Vec::new();
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    finished.push(workers.swap_remove(index));
                } else {
                    index += 1;
                }
            }
            finished
        }
        Err(_) => {
            crate::logging::error("source-folder worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in finished {
        if worker.join().is_err() {
            crate::logging::error("source-folder worker join failed", json!({}));
        }
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
    let sequence = next_event_sequence();
    emit(app, "source-check://state", snapshot_at(sequence));
}

fn emit_done(app: &AppHandle, mut payload: serde_json::Value) {
    payload["eventSequence"] = json!(next_event_sequence());
    emit(app, "source-check://done", payload);
}

fn emit<T: Clone + Serialize>(app: &AppHandle, event: &str, payload: T) {
    crate::failure_runtime::emit_or_record(app, event, payload);
}
