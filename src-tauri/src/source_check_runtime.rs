//! Lifecycle owner for the finite `Check source folders` job.

use crate::source_check_state::{Request, ResultState, SourceCheckState};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

static STATE: LazyLock<Mutex<SourceCheckState>> =
    LazyLock::new(|| Mutex::new(SourceCheckState::default()));
static WORKERS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());
static EVENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const CLOSING: &str = "OneCopy is closing; source-folder checking cannot start.";

fn state() -> MutexGuard<'static, SourceCheckState> {
    match STATE.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            // Only infallible lifecycle transitions hold this lock, never I/O.
            // Retain the running claim but cancel it if an unexpected unwind occurs.
            let mut state = poisoned.into_inner();
            state.stop();
            STATE.clear_poison();
            crate::logging::error("source-check lifecycle interrupted", json!({}));
            crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::SourceCheck);
            state
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    running: bool,
    stopping: bool,
    last_result: ResultState,
    waiting: bool,
    event_sequence: u64,
}

pub fn snapshot() -> Snapshot {
    snapshot_at(EVENT_SEQUENCE.load(Ordering::SeqCst))
}

fn snapshot_at(event_sequence: u64) -> Snapshot {
    let state = state();
    Snapshot {
        running: state.running(),
        stopping: state.stopping(),
        waiting: state.waiting(),
        last_result: state.last_result,
        event_sequence,
    }
}

pub(crate) fn next_event_sequence() -> u64 {
    EVENT_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn running() -> bool {
    state().running()
}

pub fn start(app: AppHandle) -> Result<bool, String> {
    start_requested(app, Request::Automatic)
}

pub fn start_explicit(app: AppHandle) -> Result<bool, String> {
    start_requested(app, Request::Explicit)
}

fn start_requested(app: AppHandle, request: Request) -> Result<bool, String> {
    let mut workers = WORKERS
        .lock()
        .map_err(|_| "source-folder worker state is unavailable".to_string())?;
    if crate::app_lifecycle::shutting_down() {
        return Err(CLOSING.to_string());
    }
    join_finished(&mut workers);
    if !state().begin(request, crate::scan_runtime::foreground_pending()) {
        return Ok(false);
    }
    // Discovery must not sit invisibly behind an hours-long metadata tail.
    // Completion yields at the scanner's existing safe checkpoints and keeps
    // its durable queue for the wake at this worker's terminal boundary.
    crate::file_information_runtime::preempt();
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
            finish(ResultState::Failed);
            crate::admit_background_completion(app.clone());
            format!("could not start source-folder check: {error}")
        })?;
    workers.push(worker);
    drop(workers);
    emit_state(&app);
    if release.send(()).is_err() {
        finish(ResultState::Failed);
        emit_state(&app);
        crate::admit_background_completion(app);
        return Err("source-folder worker could not leave its start gate".to_string());
    }
    Ok(true)
}

fn worker_entry(app: AppHandle) {
    let handle = app.clone();
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| worker(handle))) {
        finish(ResultState::Failed);
        let error = crate::failure_runtime::panic_message(payload);
        if crate::app_lifecycle::shutting_down() {
            crate::logging::error(
                "source-folder worker failed during shutdown",
                json!({ "error": { "message": error } }),
            );
            return;
        }
        fail(&app, &error);
        emit_state(&app);
        emit_done(&app, json!({ "error": error }));
        crate::admit_background_completion(app);
    }
}

fn worker(app: AppHandle) {
    let outcome = catch_unwind(AssertUnwindSafe(|| run(&app)));
    if crate::app_lifecycle::shutting_down() {
        match outcome {
            Ok(Err(error)) if error != crate::scanner::CANCELLED => crate::logging::error(
                "source-folder worker failed during shutdown",
                json!({ "error": { "message": error } }),
            ),
            Err(payload) => crate::logging::error(
                "source-folder worker failed during shutdown",
                json!({
                    "error": { "message": crate::failure_runtime::panic_message(payload) }
                }),
            ),
            _ => {}
        }
        finish(ResultState::Stopped);
        return;
    }
    let terminal = match outcome {
        Ok(Ok(summary)) => {
            crate::logging::info(
                "source-folder check complete",
                json!({ "summary": summary }),
            );
            if let Err(error) = crate::watcher::restart_from_config(app.clone()) {
                crate::scan_runtime::record_runtime_failure(&app, "watcher-failed", &error);
            }
            (
                if summary.failures > 0 {
                    ResultState::CompletedWithIssues
                } else {
                    ResultState::Completed
                },
                json!({ "summary": summary }),
            )
        }
        Ok(Err(error)) if error == crate::scanner::CANCELLED => {
            crate::logging::info("source-folder check stopped", json!({}));
            (ResultState::Stopped, json!({ "stopped": true }))
        }
        Ok(Err(error)) => {
            fail(&app, &error);
            (ResultState::Failed, json!({ "error": error }))
        }
        Err(payload) => {
            let error = crate::failure_runtime::panic_message(payload);
            fail(&app, &error);
            (ResultState::Failed, json!({ "error": error }))
        }
    };
    let notify_completion = finish(terminal.0);
    emit_state(&app);
    emit_done(&app, terminal.1);
    if notify_completion {
        let incomplete = matches!(terminal.0, ResultState::CompletedWithIssues);
        let request = crate::notifications::NotificationRequest {
            kind: "source-check-completed".to_string(),
            path: None,
            level: if incomplete {
                crate::notifications::NotificationLevel::Warning
            } else {
                crate::notifications::NotificationLevel::Info
            },
            presentation: crate::notifications::NotificationPresentation::Timed,
            message: if incomplete {
                "Source-folder check finished. Some folders or files could not be checked."
            } else {
                "Source folders checked."
            }
            .to_string(),
        };
        if let Err(error) = crate::notifications::publish(&app, request) {
            crate::scan_runtime::record_runtime_failure(
                &app,
                "source-check-feedback-failed",
                &error,
            );
        }
    }
    // A foreground action may have preempted this worker before it acquired
    // the index claim. In that order the foreground guard finishes first, so
    // its resume attempt sees this worker as still running. Retry here after
    // publishing the terminal state; user-requested Stop retires the request.
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
        || state().cancelled(),
        || {
            let mut trace = crate::activity::WorkTrace::begin(crate::activity::ActivityOwner::SourceCheck, None, None);
            let report = trace.progress_reporter();
            let result = crate::index_store::open(&db_file)
                .and_then(|conn| crate::scanner::run_source_check(&conn, &settings, &|value| {
                    report(value.done, value.total);
                    progress(value);
                }));
            if result.as_ref().is_ok_and(|summary| summary.failures > 0) {
                trace.finish(crate::activity::ActivityState::Failed, None);
            } else { trace.result(&result); }
            result
        },
    )?;
    if crate::app_lifecycle::shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    crate::failure_runtime::clear(app, "source-check-failed", None)?;
    Ok(summary)
}

fn finish(result: ResultState) -> bool {
    state().finish(result)
}

pub fn stop(app: &AppHandle) -> bool {
    let stopped = {
        let mut state = state();
        let stopped = state.stop();
        if stopped {
            crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::SourceCheck);
        }
        stopped
    };
    if stopped {
        emit_state(app);
        crate::admit_background_completion(app.clone());
    }
    stopped
}

pub(crate) fn preempt() {
    // Couple cancellation to its lifecycle transition so a later Start cannot
    // be accidentally cancelled by this older foreground interruption.
    let mut state = state();
    state.preempt();
    if state.running() && state.cancelled() {
        crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::SourceCheck);
    }
}

pub(crate) fn resume_if_requested(app: AppHandle) {
    if crate::app_lifecycle::shutting_down() {
        return;
    }
    if let Err(error) = start_requested(app.clone(), Request::Resume) {
        fail(&app, &error);
        emit_done(&app, json!({ "error": error }));
    }
}

pub fn shutdown() {
    if WORKERS.lock().is_err() {
        crate::logging::error("source-folder worker state is unavailable", json!({}));
    }
    state().stop();
    crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::SourceCheck);
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

fn join_finished(workers: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            if worker.join().is_err() {
                crate::logging::error("source-folder worker join failed", json!({}));
            }
        } else {
            index += 1;
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
    let sequence = next_event_sequence();
    payload["eventSequence"] = json!(sequence);
    payload["sourceCheck"] = json!(snapshot_at(sequence));
    emit(app, "source-check://done", payload);
}

fn emit<T: Clone + Serialize>(app: &AppHandle, event: &str, payload: T) {
    crate::failure_runtime::emit_or_record(app, event, payload);
}
