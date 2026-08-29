//! Lifecycle owner for completing missing hashes, metadata, dates, and
//! companion relationships. Durable index debt is the queue; wake requests
//! only ensure that one worker looks at it.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use serde_json::json;
use tauri::AppHandle;

static RUNNING: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
static PREEMPTED: AtomicBool = AtomicBool::new(false);
static WORKERS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    running: bool,
    paused: bool,
    stopping: bool,
    queued: bool,
}

pub fn snapshot(data_root: &std::path::Path) -> Snapshot {
    let queued = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))
        .and_then(|conn| crate::scanner::pending_index_work_exists(&conn))
        .unwrap_or(true);
    Snapshot {
        running: running(),
        paused: PAUSED.load(Ordering::SeqCst),
        stopping: running() && (PAUSED.load(Ordering::SeqCst) || PREEMPTED.load(Ordering::SeqCst)),
        queued,
    }
}

pub fn running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn wake(app: AppHandle) {
    REQUESTED.store(true, Ordering::SeqCst);
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
    if let Err(error) = start_worker(app.clone()) {
        PAUSED.store(true, Ordering::SeqCst);
        fail(&app, &error);
        emit_state(&app);
        emit(&app, "file-information://done", json!({ "error": error }));
    }
}

fn start_worker(app: AppHandle) -> Result<(), String> {
    if RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }
    PREEMPTED.store(false, Ordering::SeqCst);
    join_finished();
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
    let mut workers = WORKERS.lock().map_err(|_| {
        RUNNING.store(false, Ordering::SeqCst);
        "file-information worker state is unavailable".to_string()
    })?;
    workers.push(worker);
    drop(workers);
    emit_state(&app);
    if release.send(()).is_err() {
        RUNNING.store(false, Ordering::SeqCst);
        emit_state(&app);
        return Err("file-information worker could not leave its start gate".to_string());
    }
    Ok(())
}

fn worker_entry(app: AppHandle) {
    let handle = app.clone();
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| worker(handle))) {
        RUNNING.store(false, Ordering::SeqCst);
        PREEMPTED.store(false, Ordering::SeqCst);
        REQUESTED.store(true, Ordering::SeqCst);
        PAUSED.store(true, Ordering::SeqCst);
        let error = crate::failure_runtime::panic_message(payload);
        fail(&app, &error);
        emit_state(&app);
        emit(&app, "file-information://done", json!({ "error": error }));
    }
}

fn worker(app: AppHandle) {
    let outcome = catch_unwind(AssertUnwindSafe(|| run_requested(&app)));
    let terminal = match outcome {
        Ok(Ok(summary)) => {
            if summary.is_some() {
                crate::derived_work::wake(true);
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
            PAUSED.store(true, Ordering::SeqCst);
            fail(&app, &error);
            json!({ "error": error })
        }
        Err(payload) => {
            REQUESTED.store(true, Ordering::SeqCst);
            PAUSED.store(true, Ordering::SeqCst);
            let error = crate::failure_runtime::panic_message(payload);
            fail(&app, &error);
            json!({ "error": error })
        }
    };
    RUNNING.store(false, Ordering::SeqCst);
    PREEMPTED.store(false, Ordering::SeqCst);
    emit_state(&app);
    emit(&app, "file-information://done", terminal);
    if REQUESTED.load(Ordering::SeqCst)
        && !PAUSED.load(Ordering::SeqCst)
        && !crate::scan_runtime::foreground_pending()
        && !crate::source_check_runtime::running()
    {
        if let Err(error) = start_worker(app.clone()) {
            PAUSED.store(true, Ordering::SeqCst);
            fail(&app, &error);
            emit_state(&app);
            emit(&app, "file-information://done", json!({ "error": error }));
        }
    }
}

fn run_requested(app: &AppHandle) -> Result<Option<crate::scanner::ScanSummary>, String> {
    if PAUSED.load(Ordering::SeqCst) {
        return Ok(None);
    }
    REQUESTED.store(false, Ordering::SeqCst);
    let data_root = crate::paths::data_root(app)?;
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = crate::scanner::settings_from_config(
        config.as_ref(),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let db_file = data_root.join(crate::storage::INDEX_DB_FILE_NAME);
    let progress =
        crate::scan_runtime::progress_emitter(app.clone(), "file-information://progress");
    let summary = crate::scan_runtime::with_owner(
        crate::scan_runtime::Owner::FileInformation,
        PAUSED.load(Ordering::SeqCst) || PREEMPTED.load(Ordering::SeqCst),
        || -> Result<Option<crate::scanner::ScanSummary>, String> {
            let conn = crate::index_store::open(&db_file)?;
            if !crate::scanner::pending_index_work_exists(&conn)? {
                return Ok(None);
            }
            let mut summary = crate::scanner::ScanSummary::default();
            crate::scanner::run_index_tail(&conn, &settings, &progress, &mut summary)?;
            Ok(Some(summary))
        },
    )?;
    crate::failure_runtime::clear(app, "file-information-failed", None)?;
    Ok(summary)
}

pub fn set_paused(app: AppHandle, paused: bool) {
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

pub fn shutdown(app: &AppHandle) {
    PAUSED.store(true, Ordering::SeqCst);
    REQUESTED.store(true, Ordering::SeqCst);
    crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::FileInformation);
    emit_state(app);
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
            crate::logging::error("file-information worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in finished {
        if worker.join().is_err() {
            crate::logging::error("file-information worker join failed", json!({}));
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
    let Ok(data_root) = crate::paths::data_root(app) else {
        return;
    };
    emit(app, "file-information://state", snapshot(&data_root));
}

fn emit<T: Clone + Serialize>(app: &AppHandle, event: &str, payload: T) {
    crate::failure_runtime::emit_or_record(app, event, payload);
}
