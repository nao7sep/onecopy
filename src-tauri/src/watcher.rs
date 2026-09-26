//! The filesystem watcher — a core feature, ON by default (the Camera Roll
//! inflow case): `notify` events mark the affected DIRECTORIES dirty; a
//! debounced background pass re-stats exactly those directories, runs the
//! pending pipeline stages over whatever changed, and tells the UI. Correctness
//! never depends on it — app-owned mutations update the index synchronously,
//! and a watcher overflow ("events lost") flags roots as rescan-needed in the
//! UI instead of failing silently.
//!
//! One watcher thread per app run; events are collected into a dirty set and
//! drained every couple of seconds. While a full scan is running the drain
//! simply waits — the scan's own walk covers the changes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// The bounded queue's capacity between the `notify` callback and the drain
/// loop. Large enough for an ordinary import burst; a queue this deep filling
/// up while ingestion is blocked on `INDEXING` (W-B1) means the run is
/// already going to need a full recheck, so overflow is treated the same as
/// `notify`'s own `need_rescan()` rather than growing without bound (W-L4).
const EVENT_QUEUE_CAPACITY: usize = 4096;

use notify::Watcher;
use serde_json::json;

use crate::logging;
use crate::scanner::{self, ScanLists};

static GENERATION: AtomicU64 = AtomicU64::new(0);
static WORKERS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

fn generation_is_live(current: u64, owned: u64, shutting_down: bool) -> bool {
    !shutting_down && current == owned
}

fn owns_generation(generation: u64) -> bool {
    generation_is_live(
        GENERATION.load(Ordering::SeqCst),
        generation,
        crate::app_lifecycle::shutting_down(),
    )
}

pub fn restart_from_config(app: tauri::AppHandle) -> Result<(), String> {
    let data_root = crate::paths::data_root(&app)?;
    let source_dirs = crate::storage::load_config_source_dirs(&data_root)?;
    start(app, source_dirs).map(|_| ())
}

/// Re-stats ONE directory (non-recursive): upserts its current files and marks
/// rows for vanished files missing — the walk logic scoped to a single dir.
pub fn restat_dir(
    conn: &rusqlite::Connection,
    dir: &Path,
    lists: &ScanLists,
    source_roots: &[String],
    data_root: &Path,
) -> Result<u64, String> {
    if crate::trash::is_trash_path(dir) || crate::paths::is_within_data_root(dir, data_root) {
        return Ok(0);
    }
    // notify reports ordinary Windows paths even when the full scan stores
    // their long-path spelling. Normalize at the boundary so a watcher pass
    // cannot turn one physical file into a second database row.
    let dir = crate::winpath::for_fs(dir);
    let dir = dir.as_ref();
    let roots = crate::visibility_index::source_root_spellings(conn, source_roots)?;
    let root = crate::visibility_index::root_for(&roots, dir).ok_or("Changed directory is outside configured sources")?;
    // Checked up front, before `DirectoryFacts::refresh` (which itself
    // `stat`s every ancestor down to `dir` and would surface the same
    // vanished condition as an `Err` there instead): a directory deleted
    // after its events arrived is normal (Shift+Del, `rm -r`), not a walk
    // failure. Every row under it is marked missing instead, contained to
    // this one dirty entry rather than aborting the whole watcher batch.
    if matches!(std::fs::symlink_metadata(dir), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        let dir_str = dir.to_string_lossy().to_string();
        return scanner::mark_missing_under(conn, &dir_str);
    }
    let mut visibility_directories = crate::visibility_index::DirectoryFacts::default();
    let inherited = visibility_directories.refresh(conn, &root, dir)?;
    let mut changed = visibility_directories.changed_files as u64;
    let mut present: HashSet<String> = HashSet::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A race after the check above (deleted between the two calls):
            // same treatment.
            let dir_str = dir.to_string_lossy().to_string();
            let changed = scanner::mark_missing_under(conn, &dir_str)?;
            return Ok(changed);
        }
        Err(error) => {
            crate::index_store::upsert_issue(
                conn,
                Some(dir.to_string_lossy().as_ref()),
                scanner::WALK_ERROR,
                &error.to_string(),
            )?;
            return Err(error.to_string());
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                crate::index_store::upsert_issue(
                    conn,
                    Some(dir.to_string_lossy().as_ref()),
                    scanner::WALK_ERROR,
                    &error.to_string(),
                )?;
                return Err(error.to_string());
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                crate::index_store::upsert_issue(
                    conn,
                    Some(path.to_string_lossy().as_ref()),
                    scanner::STAT_ERROR,
                    &error.to_string(),
                )?;
                return Err(error.to_string());
            }
        };
        if !file_type.is_file() {
            continue;
        }
        let abs = path.to_string_lossy().to_string();
        if crate::trash::is_trash_path(&path)
            || crate::paths::is_within_data_root(&path, data_root)
            || crate::scanner::is_apple_double_sidecar(&path)
        {
            continue;
        }
        present.insert(abs.clone());
        match scanner::upsert_file(conn, &path, lists, inherited) {
            Ok(scanner::Upsert::Unchanged) => {}
            Ok(_) => changed += 1,
            Err(error) => {
                crate::index_store::upsert_issue(
                    conn,
                    Some(&abs),
                    scanner::STAT_ERROR,
                    &error,
                )?;
                return Err(error);
            }
        }
        crate::index_store::clear_issues(
            conn,
            &abs,
            &[scanner::STAT_ERROR, scanner::WALK_ERROR],
        )?;
    }
    crate::index_store::clear_issues(
        conn,
        &dir.to_string_lossy(),
        &[scanner::WALK_ERROR],
    )?;

    // Rows directly in this dir whose files are gone → missing.
    let dir_str = dir.to_string_lossy().to_string();
    let mut stmt = conn
        .prepare("SELECT abs_path FROM paths WHERE dir_path = ?1 AND missing = 0")
        .map_err(|e| e.to_string())?;
    let known: Vec<String> = stmt
        .query_map([&dir_str], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);
    for path in known {
        if !present.contains(&path) {
            scanner::mark_path_missing(conn, &path)?;
            changed += 1;
        }
    }

    Ok(changed)
}

/// Starts the watcher thread over the configured source roots. Best-effort:
/// a watcher that cannot start logs one warn and the app continues (rescan
/// remains the manual path).
pub fn start(app: tauri::AppHandle, source_dirs: Vec<String>) -> Result<bool, String> {
    let mut workers = WORKERS
        .lock()
        .map_err(|_| "watcher worker state is unavailable".to_string())?;
    if crate::app_lifecycle::shutting_down() {
        return Ok(false);
    }
    join_finished(&mut workers);
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::Watcher);
    if source_dirs.is_empty() {
        return Ok(false);
    }
    let handle = app.clone();
    let started = std::thread::Builder::new()
        .name("onecopy-watcher".to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(handle.clone(), source_dirs, generation)
            }));
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(error)) if owns_generation(generation) => {
                    report_failure(&handle, &error)
                }
                Ok(Err(_)) => {}
                Err(payload) => {
                    let error = payload
                        .downcast_ref::<&str>()
                        .map(|value| (*value).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "watcher stopped unexpectedly".to_string());
                    if owns_generation(generation) {
                        report_failure(&handle, &error);
                    } else {
                        logging::error(
                            "watcher failed after its generation retired",
                            json!({ "error": { "message": error } }),
                        );
                    }
                }
            }
        });
    let worker = started.map_err(|error| format!("could not start watcher thread: {error}"))?;
    workers.push(worker);
    Ok(true)
}

fn run(app: tauri::AppHandle, source_dirs: Vec<String>, generation: u64) -> Result<(), String> {
    // Resolved once: a source containing the data root must never make the
    // watcher index or churn the app's own storage (R6-02).
    let data_root = crate::paths::data_root(&app)?;
    let (tx, rx) = mpsc::sync_channel::<notify::Result<notify::Event>>(EVENT_QUEUE_CAPACITY);
    // A full queue means ingestion cannot keep up (typically an `INDEXING`
    // holder running for a long time, W-B1). `try_send` never blocks the
    // `notify` callback thread; a full queue instead flags the same overflow
    // path the drain loop already uses for `need_rescan()`, so the affected
    // roots are rechecked instead of the process either stalling or growing
    // an unbounded backlog.
    let overflowed_while_blocked = Arc::new(AtomicBool::new(false));
    let handler_overflow = overflowed_while_blocked.clone();
    let handler = move |event: notify::Result<notify::Event>| {
        forward_or_flag_overflow(&tx, &handler_overflow, event);
    };
    let mut watcher = notify::recommended_watcher(handler).map_err(|error| error.to_string())?;
    let mut watched = 0usize;
    for root in &source_dirs {
        if !owns_generation(generation) {
            return Ok(());
        }
        if let Err(err) = watcher.watch(Path::new(root), notify::RecursiveMode::Recursive) {
            if !owns_generation(generation) {
                return Ok(());
            }
            logging::warn(
                "watcher could not watch a root",
                json!({ "root": root, "error": { "message": err.to_string() } }),
            );
            record_root_condition(&app, root, Some(&err.to_string()))?;
        } else {
            if !owns_generation(generation) {
                return Ok(());
            }
            record_root_condition(&app, root, None)?;
            watched += 1;
        }
    }
    if watched == 0 {
        return Err("none of the configured source folders could be watched".to_string());
    }
    crate::failure_runtime::clear(&app, "watcher-failed", None)?;
    logging::info("watcher started", json!({ "roots": source_dirs.len() }));

    let mut dirty: HashSet<PathBuf> = HashSet::new();
    let mut overflowed = false;
    loop {
        if !owns_generation(generation) {
            return Ok(());
        }
        // Wake periodically so a settings-driven watcher replacement can
        // retire this generation even on a completely quiet filesystem.
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => collect(event, &mut dirty, &mut overflowed, &data_root),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("watcher event channel disconnected".to_string())
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while let Ok(event) =
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            collect(event, &mut dirty, &mut overflowed, &data_root);
        }

        if !owns_generation(generation) {
            return Ok(());
        }

        if overflowed_while_blocked.swap(false, Ordering::SeqCst) {
            overflowed = true;
        }
        if overflowed {
            overflowed = false;
            dirty.clear();
            record_activity(
                crate::activity::ActivityKind::Failed,
                generation,
                crate::activity::ActivityState::Failed,
                Some(crate::activity::ActivityReason::Error),
                None,
            );
            crate::failure_runtime::emit_or_record(
                &app,
                "watch://rescan-needed",
                json!({ "reason": "event overflow" }),
            );
            logging::warn("watcher overflow; roots flagged rescan-needed", json!({}));
            continue;
        }
        if dirty.is_empty() {
            continue;
        }
        let dirs: Vec<PathBuf> = dirty.drain().collect();
        let outcome = process_dirty(&app, &dirs, generation);
        if !owns_generation(generation) {
            return Ok(());
        }
        match outcome {
            Ok(pass) => {
                // A directory that could not be re-read leaves its part of the
                // library stale until a source check, so the pass reports the
                // watcher failure and asks for a recheck instead of clearing it.
                match pass.failure() {
                    Some(failure) => report_failure(&app, &failure),
                    None => crate::failure_runtime::clear(&app, "watcher-failed", None)?,
                }
                if pass.changed == 0 {
                    continue;
                }
                let changed = pass.changed;
                record_activity(
                    crate::activity::ActivityKind::Changed,
                    generation,
                    crate::activity::ActivityState::Succeeded,
                    Some(crate::activity::ActivityReason::Completion),
                    Some(changed),
                );
                crate::failure_runtime::emit_or_record(
                    &app,
                    "watch://updated",
                    json!({ "changed": changed }),
                );
            }
            Err(err) => {
                logging::warn(
                    "watcher pass failed",
                    json!({ "error": { "message": &err } }),
                );
                report_failure(&app, &err);
            }
        }
    }
}

/// Closes watcher admission and invalidates every generation before the app's
/// shutdown owner joins their retained handles.
pub fn shutdown() {
    let _workers = match WORKERS.lock() {
        Ok(workers) => workers,
        Err(_) => {
            logging::error("watcher worker state is unavailable", json!({}));
            GENERATION.fetch_add(1, Ordering::SeqCst);
            crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::Watcher);
            return;
        }
    };
    GENERATION.fetch_add(1, Ordering::SeqCst);
    crate::scan_runtime::request_cancel(crate::scan_runtime::Owner::Watcher);
}

pub fn join() {
    let workers = match WORKERS.lock() {
        Ok(mut workers) => workers.drain(..).collect::<Vec<_>>(),
        Err(_) => {
            logging::error("watcher worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in workers {
        if worker.join().is_err() {
            logging::error("watcher worker join failed", json!({}));
        }
    }
}

fn join_finished(workers: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            if worker.join().is_err() {
                logging::error("watcher worker join failed", json!({}));
            }
        } else {
            index += 1;
        }
    }
}

fn report_failure(app: &tauri::AppHandle, error: &str) {
    record_activity(
        crate::activity::ActivityKind::Failed,
        GENERATION.load(Ordering::SeqCst),
        crate::activity::ActivityState::Failed,
        Some(crate::activity::ActivityReason::Error),
        None,
    );
    logging::error("watcher failed", json!({ "error": { "message": error } }));
    crate::scan_runtime::record_runtime_failure(app, "watcher-failed", error);
    for event in ["watch://failed", "watch://rescan-needed"] {
        crate::failure_runtime::emit_or_record(app, event, json!({ "reason": error }));
    }
}

fn record_activity(
    kind: crate::activity::ActivityKind,
    generation: u64,
    current: crate::activity::ActivityState,
    reason: Option<crate::activity::ActivityReason>,
    changed: Option<u64>,
) {
    let _ = crate::activity::record(crate::activity::ActivityDraft {
        kind,
        owner: crate::activity::ActivityOwner::Watcher,
        subject: None,
        operation_id: Some(format!("watcher:{generation}")),
        cause_id: None,
        generation: Some(generation),
        previous: None,
        current: Some(current),
        reason,
        lane: None,
        item_count: changed,
        queued: None,
        done: None,
        total: None,
        target_hash: None,
    });
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// `generation_is_live` and `restat_batch`; promoting them would widen the
// crate's API only for these tests.
#[path = "../tests/unit/watcher.rs"]
mod lifecycle_tests;

fn record_root_condition(
    app: &tauri::AppHandle,
    root: &str,
    error: Option<&str>,
) -> Result<(), String> {
    if let Some(message) = error {
        crate::failure_runtime::report(app, "watcher-root-failed", Some(root), message)
    } else {
        crate::failure_runtime::clear(app, "watcher-root-failed", Some(root))
    }
}

/// The `notify` callback: forwards into the bounded channel, or flags
/// overflow instead of blocking the callback thread when it is full (W-L4).
///
/// `pub` for the tests: a real full-queue scenario needs `INDEXING` held for
/// the channel's whole capacity, which a unit test has no reason to spin up.
pub fn forward_or_flag_overflow(
    tx: &mpsc::SyncSender<notify::Result<notify::Event>>,
    overflowed: &AtomicBool,
    event: notify::Result<notify::Event>,
) {
    if tx.try_send(event).is_err() {
        overflowed.store(true, Ordering::SeqCst);
    }
}

/// Folds one watcher event into the dirty-directory set.
///
/// `pub` for the tests: a file event must map to its PARENT directory, since
/// the drain calls `read_dir` on whatever lands here — inserting the file path
/// instead makes that call fail silently and new photos never appear.
pub fn collect(
    event: notify::Result<notify::Event>,
    dirty: &mut HashSet<PathBuf>,
    overflowed: &mut bool,
    data_root: &Path,
) {
    match event {
        Ok(event) => {
            if event.need_rescan() {
                *overflowed = true;
                return;
            }
            for path in event.paths {
                if crate::trash::is_trash_path(&path)
                    || crate::paths::is_within_data_root(&path, data_root)
                    || crate::scanner::is_apple_double_sidecar(&path)
                {
                    continue;
                }
                let dir = if path.is_dir() {
                    path
                } else {
                    match path.parent() {
                        Some(parent) => parent.to_path_buf(),
                        None => continue,
                    }
                };
                dirty.insert(dir);
            }
        }
        Err(_) => *overflowed = true,
    }
}

/// Re-stats the dirty directories and leaves durable index debt for the
/// independent file-information owner. The shared index claim retains this
/// event batch until any active projection reaches a safe boundary.
/// One watcher batch: how many rows changed and the directories that could
/// not be re-read, each with its error.
pub(crate) struct WatchPass {
    changed: u64,
    failed: Vec<(PathBuf, String)>,
}

impl WatchPass {
    fn failure(&self) -> Option<String> {
        let (dir, error) = self.failed.first()?;
        Some(format!(
            "{} changed folder(s) could not be updated; recheck the source folders. {}: {error}",
            self.failed.len(),
            dir.to_string_lossy(),
        ))
    }
}

/// Re-stats each dirty directory. File-local containment: one unreadable or
/// vanished directory fails only its own entry, recorded in the pass, not the
/// directories after it. `between` runs before each directory and ends the
/// batch when it fails.
pub(crate) fn restat_batch(
    conn: &rusqlite::Connection,
    dirs: &[PathBuf],
    settings: &scanner::ScanSettings,
    between: &dyn Fn() -> Result<(), String>,
) -> Result<WatchPass, String> {
    let mut pass = WatchPass { changed: 0, failed: Vec::new() };
    let data_root = settings.data_root();
    for dir in dirs {
        between()?;
        match restat_dir(conn, dir, &settings.lists, &settings.source_dirs, data_root) {
            Ok(count) => pass.changed += count,
            Err(error) => {
                logging::warn(
                    "watcher directory failed; continuing with the rest of the batch",
                    json!({ "dir": dir.to_string_lossy(), "error": { "message": &error } }),
                );
                pass.failed.push((dir.clone(), error));
            }
        }
    }
    Ok(pass)
}

fn process_dirty(
    app: &tauri::AppHandle,
    dirs: &[PathBuf],
    generation: u64,
) -> Result<WatchPass, String> {
    crate::scan_runtime::with_watcher_claim(
        move || !owns_generation(generation),
        || process_dirty_claimed(app, dirs, generation),
    )
}

fn process_dirty_claimed(
    app: &tauri::AppHandle,
    dirs: &[PathBuf],
    generation: u64,
) -> Result<WatchPass, String> {
    let _awake = crate::sleep_prevention::begin_work();
    let data_root = crate::paths::data_root(app)?;
    // Config only, through the same unserialized-reader path every other
    // background worker uses. `storage::load_app_data` drains the pending
    // quarantine list for the frontend's `load_from_root` to publish; the
    // watcher has no reporting surface for it and would otherwise consume
    // (and drop) a quarantine notice meant for Main.
    let config = crate::storage::read_config_for_setup(&data_root)?;
    let settings = scanner::settings_from_config(
        config.as_ref(),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let affected_dirs: Vec<String> = dirs
        .iter()
        .map(|dir| crate::winpath::for_fs(dir).to_string_lossy().to_string())
        .collect();
    let repair_roots = scanner::begin_scoped_index_repair(&conn, &affected_dirs)?;

    let WatchPass { changed, failed } = restat_batch(&conn, dirs, &settings, &|| {
        if !owns_generation(generation) {
            return Err(scanner::CANCELLED.to_string());
        }
        // A pending foreground action takes the index between directories.
        crate::scan_runtime::yield_to_foreground().map(|_| ())
    })?;
    if !owns_generation(generation) {
        return Err(scanner::CANCELLED.to_string());
    }
    if changed > 0 {
        logging::info(
            "watcher pass",
            json!({ "dirs": dirs.len(), "changed": changed }),
        );
        crate::file_information_runtime::wake(app.clone());
    } else if failed.is_empty() {
        scanner::complete_scoped_index_repair(&conn, &repair_roots)?;
    }
    Ok(WatchPass { changed, failed })
}
