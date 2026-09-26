//! Lifecycle owner for completing missing hashes, metadata, dates, and
//! companion relationships. Durable index debt is the queue; wake requests
//! only ensure that one worker looks at it.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

static RUNNING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
static PREEMPTED: AtomicBool = AtomicBool::new(false);
/// An unexpected terminal failure holds the queued work like a pause, but is
/// shown as failed until the user retries it.
static FAILED: AtomicBool = AtomicBool::new(false);
static WORKERS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
// Cached in place of a live `pending_index_work_exists` probe (up to six
// EXISTS queries, one a correlated NOT EXISTS scan over every media row).
// Conservatively true until `run_requested` proves otherwise, so pause/resume
// and startup admission (all plain main-thread commands) can read it without
// touching the index (D-M1, W-M4). `wake()` is called at every point new
// debt might exist — scan-admission release, watcher activity, explicit
// resume — so setting it true there keeps it a faithful hint.
static PENDING_WORK_HINT: AtomicBool = AtomicBool::new(true);

enum WorkerAdmission {
    Started,
    AlreadyRunning,
    ShuttingDown,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    running: bool,
    paused: bool,
    failed: bool,
    stopping: bool,
    queued: bool,
    event_sequence: u64,
}

pub fn snapshot() -> Snapshot {
    let queued = PENDING_WORK_HINT.load(Ordering::SeqCst);
    Snapshot {
        running: running(),
        paused: PAUSED.load(Ordering::SeqCst),
        failed: FAILED.load(Ordering::SeqCst),
        stopping: running() && (PAUSED.load(Ordering::SeqCst) || PREEMPTED.load(Ordering::SeqCst)),
        queued,
        event_sequence: EVENT_SEQUENCE.load(Ordering::SeqCst),
    }
}

pub(crate) fn next_event_sequence() -> u64 {
    EVENT_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// The launch source decision reached its terminal boundary: file
/// information completion may run, and the automatic media queue opens.
pub fn admit_background_completion(app: AppHandle) {
    wake(app);
    crate::derived_work::admit_automatic();
}

pub fn wake(app: AppHandle) {
    if crate::app_lifecycle::shutting_down() {
        REQUESTED.store(false, Ordering::SeqCst);
        return;
    }
    REQUESTED.store(true, Ordering::SeqCst);
    PENDING_WORK_HINT.store(true, Ordering::SeqCst);
    if PAUSED.load(Ordering::SeqCst) {
        emit_state(&app);
        return;
    }
    if crate::source_check_runtime::running() {
        emit_state(&app);
        return;
    }
    if crate::scan_runtime::foreground_pending() {
        emit_state(&app);
        return;
    }
    match start_worker(app.clone()) {
        Ok(WorkerAdmission::Started | WorkerAdmission::AlreadyRunning) => {}
        Ok(WorkerAdmission::ShuttingDown) => {
            REQUESTED.store(false, Ordering::SeqCst);
        }
        Err(error) => {
            hold_failed();
            fail(&app, &error);
            emit_state(&app);
            emit_done(&app, json!({ "error": error }));
        }
    }
}

fn start_worker(app: AppHandle) -> Result<WorkerAdmission, String> {
    let mut workers = WORKERS
        .lock()
        .map_err(|_| "file-information worker state is unavailable".to_string())?;
    if crate::app_lifecycle::shutting_down() {
        return Ok(WorkerAdmission::ShuttingDown);
    }
    join_finished(&mut workers);
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(WorkerAdmission::AlreadyRunning);
    }
    PREEMPTED.store(false, Ordering::SeqCst);
    let handle = app.clone();
    let (release, wait_for_registration) = std::sync::mpsc::sync_channel(0);
    let worker = std::thread::Builder::new()
        .name("onecopy-file-information".to_string())
        .spawn(move || {
            if wait_for_registration.recv().is_ok() {
                worker_entry(handle);
            }
        })
        .map_err(|error| {
            RUNNING.store(false, Ordering::SeqCst);
            format!("could not start file-information completion: {error}")
        })?;
    workers.push(worker);
    drop(workers);
    emit_state(&app);
    if release.send(()).is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        emit_state(&app);
        return Err("file-information worker could not leave its start gate".to_string());
    }
    Ok(WorkerAdmission::Started)
}

fn worker_entry(app: AppHandle) {
    let handle = app.clone();
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| worker(handle))) {
        RUNNING.store(false, Ordering::SeqCst);
        PREEMPTED.store(false, Ordering::SeqCst);
        REQUESTED.store(true, Ordering::SeqCst);
        hold_failed();
        let error = crate::failure_runtime::panic_message(payload);
        if crate::app_lifecycle::shutting_down() {
            REQUESTED.store(false, Ordering::SeqCst);
            crate::logging::error(
                "file-information worker failed during shutdown",
                json!({ "error": { "message": error } }),
            );
            return;
        }
        fail(&app, &error);
        emit_state(&app);
        emit_done(&app, json!({ "error": error }));
    }
}

fn worker(app: AppHandle) {
    let outcome = catch_unwind(AssertUnwindSafe(|| run_requested(&app)));
    if crate::app_lifecycle::shutting_down() {
        match outcome {
            Ok(Err(error)) if error != crate::scanner::CANCELLED => crate::logging::error(
                "file-information worker failed during shutdown",
                json!({ "error": { "message": error } }),
            ),
            Err(payload) => crate::logging::error(
                "file-information worker failed during shutdown",
                json!({
                    "error": { "message": crate::failure_runtime::panic_message(payload) }
                }),
            ),
            _ => {}
        }
        RUNNING.store(false, Ordering::SeqCst);
        PREEMPTED.store(false, Ordering::SeqCst);
        REQUESTED.store(false, Ordering::SeqCst);
        PAUSED.store(true, Ordering::SeqCst);
        return;
    }
    let terminal = match outcome {
        Ok(Ok(summary)) => {
            if summary.is_some() {
                crate::derived_work::wake();
                crate::logging::info(
                    "file-information completion complete",
                    json!({ "summary": &summary }),
                );
            }
            json!({ "summary": summary })
        }
        Ok(Err(error)) if error == crate::scanner::CANCELLED => {
            REQUESTED.store(true, Ordering::SeqCst);
            json!({ "paused": PAUSED.load(Ordering::SeqCst), "preempted": true })
        }
        Ok(Err(error)) => {
            REQUESTED.store(true, Ordering::SeqCst);
            hold_failed();
            fail(&app, &error);
            json!({ "error": error })
        }
        Err(payload) => {
            REQUESTED.store(true, Ordering::SeqCst);
            hold_failed();
            let error = crate::failure_runtime::panic_message(payload);
            fail(&app, &error);
            json!({ "error": error })
        }
    };
    RUNNING.store(false, Ordering::SeqCst);
    PREEMPTED.store(false, Ordering::SeqCst);
    emit_state(&app);
    emit_done(&app, terminal);
    if REQUESTED.load(Ordering::SeqCst)
        && !PAUSED.load(Ordering::SeqCst)
        && !crate::scan_runtime::foreground_pending()
        && !crate::source_check_runtime::running()
    {
        match start_worker(app.clone()) {
            Ok(WorkerAdmission::Started | WorkerAdmission::AlreadyRunning) => {}
            Ok(WorkerAdmission::ShuttingDown) => {
                REQUESTED.store(false, Ordering::SeqCst);
            }
            Err(error) => {
                hold_failed();
                fail(&app, &error);
                emit_state(&app);
                emit_done(&app, json!({ "error": error }));
            }
        }
    }
    crate::derived_work::wake();
}

fn run_requested(app: &AppHandle) -> Result<Option<crate::scanner::ScanSummary>, String> {
    if PAUSED.load(Ordering::SeqCst) || crate::app_lifecycle::shutting_down() {
        return Ok(None);
    }
    REQUESTED.store(false, Ordering::SeqCst);
    let data_root = crate::paths::data_root()?;
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = crate::scanner::settings_from_config(
        config.as_ref(),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let visibility =
        crate::visibility::Policy::from_config(config.as_ref().unwrap_or(&json!({})))?;
    let db_file = data_root.join(crate::storage::INDEX_DB_FILE_NAME);
    let progress = crate::scan_runtime::progress_emitter(
        app.clone(),
        "file-information://progress",
        next_event_sequence,
    );
    let summary = crate::scan_runtime::with_owner(
        crate::scan_runtime::Owner::FileInformation,
        || PAUSED.load(Ordering::SeqCst) || PREEMPTED.load(Ordering::SeqCst),
        || -> Result<Option<crate::scanner::ScanSummary>, String> {
            let conn = crate::index_store::open(&db_file)?;
            let pending = |conn: &rusqlite::Connection| -> Result<bool, String> {
                Ok(crate::library_settings::owed(conn, &settings, &visibility)?
                    || crate::scanner::pending_index_work_exists(conn)?)
            };
            complete_pending(&conn, pending, |conn| {
                let mut summary = crate::scanner::ScanSummary::default();
                let mut trace = crate::activity::WorkTrace::begin(crate::activity::ActivityOwner::FileInformation, None, None);
                let report = trace.progress_reporter();
                let report_progress = |value: crate::scanner::ScanProgress| {
                    report(value.done, value.total);
                    progress(value);
                };
                // Saved library settings a busy Settings apply left owed.
                let result = crate::library_settings::apply(conn, &settings, &visibility, &report_progress)
                    .and_then(|_| crate::scanner::run_index_tail(conn, &settings, &report_progress, &mut summary));
                if result.is_ok() && summary.failures > 0 {
                    trace.finish(crate::activity::ActivityState::Failed, None);
                } else { trace.result(&result); }
                result?;
                Ok(summary)
            })
        },
    )?;
    if crate::app_lifecycle::shutting_down() {
        return Ok(None);
    }
    crate::failure_runtime::clear("file-information-failed", None)?;
    Ok(summary)
}

/// Runs `work` over the durable debt when there is any and leaves the
/// pending-work hint matching what remains afterwards, so a completed run is
/// not shown as still queued.
fn complete_pending(
    conn: &rusqlite::Connection,
    pending: impl Fn(&rusqlite::Connection) -> Result<bool, String>,
    work: impl FnOnce(&rusqlite::Connection) -> Result<crate::scanner::ScanSummary, String>,
) -> Result<Option<crate::scanner::ScanSummary>, String> {
    let owed = pending(conn)?;
    PENDING_WORK_HINT.store(owed, Ordering::SeqCst);
    if !owed {
        return Ok(None);
    }
    let summary = work(conn)?;
    PENDING_WORK_HINT.store(pending(conn).unwrap_or(true), Ordering::SeqCst);
    Ok(Some(summary))
}

/// Holds the queued work after an unexpected terminal failure, so it does
/// not retry in a loop, until the user retries it with Resume.
fn hold_failed() {
    FAILED.store(true, Ordering::SeqCst);
    PAUSED.store(true, Ordering::SeqCst);
}

/// Pause, Resume, or retry after a failure; every explicit choice ends the
/// failed presentation.
pub fn set_paused(app: AppHandle, paused: bool) {
    if crate::app_lifecycle::shutting_down() {
        PAUSED.store(true, Ordering::SeqCst);
        REQUESTED.store(false, Ordering::SeqCst);
        return;
    }
    FAILED.store(false, Ordering::SeqCst);
    PAUSED.store(paused, Ordering::SeqCst);
    if paused {
        REQUESTED.store(true, Ordering::SeqCst);
        crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::FileInformation);
        emit_state(&app);
    } else {
        wake(app);
    }
}

pub(crate) fn preempt() {
    if running() {
        PREEMPTED.store(true, Ordering::SeqCst);
        REQUESTED.store(true, Ordering::SeqCst);
        crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::FileInformation);
    }
}

pub fn shutdown() {
    if WORKERS.lock().is_err() {
        crate::logging::error("file-information worker state is unavailable", json!({}));
    }
    PAUSED.store(true, Ordering::SeqCst);
    REQUESTED.store(false, Ordering::SeqCst);
    crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::FileInformation);
}

pub fn join() {
    let workers = match WORKERS.lock() {
        Ok(mut workers) => workers.drain(..).collect::<Vec<_>>(),
        Err(_) => {
            crate::logging::error("file-information worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in workers {
        if worker.join().is_err() {
            crate::logging::error("file-information worker join failed", json!({}));
        }
    }
}

fn join_finished(workers: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            if worker.join().is_err() {
                crate::logging::error("file-information worker join failed", json!({}));
            }
        } else {
            index += 1;
        }
    }
}

fn fail(app: &AppHandle, error: &str) {
    crate::logging::error(
        "file-information completion failed",
        json!({ "error": { "message": error } }),
    );
    crate::scan_runtime::record_runtime_failure(app, "file-information-failed", error);
}

fn emit_state(app: &AppHandle) {
    let sequence = next_event_sequence();
    let mut state = snapshot();
    state.event_sequence = sequence;
    emit(app, "file-information://state", state);
}

fn emit_done(app: &AppHandle, mut payload: serde_json::Value) {
    payload["eventSequence"] = json!(next_event_sequence());
    emit(app, "file-information://done", payload);
}

fn emit<T: Clone + Serialize>(app: &AppHandle, event: &str, payload: T) {
    crate::failure_runtime::emit_or_record(app, event, payload);
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private pending-work
// hint; promoting it would widen the crate's API only for this test.
#[path = "../tests/unit/file_information_runtime.rs"]
mod tests;
