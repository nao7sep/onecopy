//! The filesystem watcher — a core feature, ON by default (the Camera Roll
//! inflow case): `notify` events mark the affected DIRECTORIES dirty; a
//! debounced background pass re-stats exactly those directories, runs the
//! pending pipeline stages over whatever changed, and tells the UI. Correctness
//! never depends on it — app-owned mutations update the index synchronously,
//! and a watcher overflow ("events lost") rechecks the affected roots
//! automatically. Failed recovery stays in Issues.
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
    let data_root = crate::paths::data_root()?;
    let source_dirs = crate::storage::configured_source_dirs(&data_root)?;
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
    restat_dir_with(conn, dir, lists, source_roots, data_root, &|path| crate::volume_io::read_dir(path, true))
}

fn restat_dir_with(
    conn: &rusqlite::Connection, dir: &Path, lists: &ScanLists,
    source_roots: &[String], data_root: &Path,
    read_dir: &dyn Fn(&Path) -> std::io::Result<Vec<crate::volume_io::DirEntryInfo>>,
) -> Result<u64, String> {
    if crate::trash::is_trash_path(dir) || crate::paths::is_within_data_root(dir, data_root) {
        return Ok(0);
    }
    // notify reports ordinary Windows paths even when the full scan stores
    // their long-path spelling. Normalize at the boundary so a watcher pass
    // cannot turn one physical file into a second database row.
    let dir = crate::winpath::for_fs(dir);
    let dir = dir.as_ref();
    // Availability has its own source Issue. Restatting an independent folder
    // must not reopen walk failures for every disconnected configured root.
    let reachable_roots: Vec<String> = source_roots.iter()
        .filter(|root| crate::volume_io::is_dir(Path::new(root)).unwrap_or(false))
        .cloned().collect();
    let roots = crate::visibility_index::source_root_spellings(conn, &reachable_roots)?;
    let root = match crate::visibility_index::root_for(&roots, dir) {
        Some(root) => root,
        None if crate::visibility_index::root_for(source_roots, dir).is_some() => return Ok(0),
        None => return Err("Changed directory is outside configured sources".to_string()),
    };
    // Losing the configured root does not establish that any indexed file was deleted.
    if !crate::volume_io::is_dir(&root).unwrap_or(false) {
        return Ok(0);
    }
    // Checked up front, before `DirectoryFacts::refresh` (which itself
    // `stat`s every ancestor down to `dir` and would surface the same
    // vanished condition as an `Err` there instead): a directory deleted
    // after its events arrived is normal (Shift+Del, `rm -r`), not a walk
    // failure. Every row under it is marked missing instead, contained to
    // this one dirty entry rather than aborting the whole watcher batch.
    if matches!(crate::volume_io::symlink_metadata(dir), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        if !crate::volume_io::is_dir(&root).unwrap_or(false) { return Ok(0); }
        let dir_str = dir.to_string_lossy().to_string();
        return scanner::mark_missing_under(conn, &dir_str);
    }
    let mut visibility_directories = crate::visibility_index::DirectoryFacts::default();
    let inherited = visibility_directories.refresh(conn, &root, dir)?;
    let mut changed = visibility_directories.changed_files as u64;
    let mut present: HashSet<String> = HashSet::new();

    // One bounded listing that also reads each entry's metadata.
    let entries = match read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // A race after the check above (deleted between the two calls):
            // same treatment.
            if !crate::volume_io::is_dir(&root).unwrap_or(false) { return Ok(changed); }
            let dir_str = dir.to_string_lossy().to_string();
            let changed = scanner::mark_missing_under(conn, &dir_str)?;
            return Ok(changed);
        }
        Err(error) => {
            crate::index_store::upsert_issue_with_descriptor(
                conn,
                Some(dir.to_string_lossy().as_ref()),
                scanner::WALK_ERROR,
                Some(scanner::scan_issue_message_key(scanner::WALK_ERROR)),
                None,
                &error.to_string(),
            )?;
            return Err(error.to_string());
        }
    };
    for entry in entries {
        let path = entry.path;
        let Some(file_type) = entry.file_type else {
            let error = "could not read the entry's type".to_string();
            crate::index_store::upsert_issue_with_descriptor(
                conn,
                Some(path.to_string_lossy().as_ref()),
                scanner::STAT_ERROR,
                Some(scanner::scan_issue_message_key(scanner::STAT_ERROR)),
                None,
                &error,
            )?;
            present.insert(path.to_string_lossy().into_owned());
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let abs = path.to_string_lossy().to_string();
        if crate::trash::is_trash_path(&path)
            || crate::paths::is_within_data_root(&path, data_root)
            || crate::scanner::is_apple_double_sidecar(&path)
            || crate::file_identity::is_private_tmp_name(&path)
        {
            continue;
        }
        present.insert(abs.clone());
        let meta = match entry.metadata {
            Some(Ok(meta)) => meta,
            Some(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                present.remove(&abs);
                continue;
            }
            failure => {
                let error = match failure {
                    Some(Err(error)) => error.to_string(),
                    _ => "file metadata was not read".to_string(),
                };
                crate::index_store::upsert_issue_with_descriptor(
                    conn, Some(&abs), scanner::STAT_ERROR,
                    Some(scanner::scan_issue_message_key(scanner::STAT_ERROR)), None, &error,
                )?;
                continue;
            }
        };
        // Database failures still stop the pass; only per-file reads are isolated.
        match scanner::upsert_file(conn, &path, &meta, lists, inherited)? {
            scanner::Upsert::Unchanged => {},
            _ => changed += 1,
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

    // A disconnection during the listing cannot prove any file absent.
    if !crate::volume_io::is_dir(&root).unwrap_or(false) { return Ok(changed); }
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
                run(handle.clone(), source_dirs.clone(), generation)
            }));
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(error)) if owns_generation(generation) => {
                    report_failure(&handle, &error);
                    recover_roots(&handle, &source_dirs, generation);
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
                        recover_roots(&handle, &source_dirs, generation);
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
    let data_root = crate::paths::data_root()?;
    let (tx, rx) = mpsc::sync_channel::<notify::Result<notify::Event>>(EVENT_QUEUE_CAPACITY);
    // A full queue means ingestion cannot keep up (typically an `INDEXING`
    // holder running for a long time, W-B1). `try_send` never blocks the
    // `notify` callback thread; a full queue instead flags the same overflow
    // path the drain loop already uses for `need_rescan()`, so the affected
    // roots are rechecked instead of the process either stalling or growing
    // an unbounded backlog.
    let recovery_flags: Vec<_> = source_dirs.iter()
        .map(|root| (root.clone(), Arc::new(AtomicBool::new(false)))).collect();
    let mut watchers: Vec<Option<notify::RecommendedWatcher>> = source_dirs.iter().map(|_| None).collect();
    let mut last_status = None;
    let mut next_presence = std::time::Instant::now();
    // Retain the sender even when every source is offline, so this owner can
    // observe returning sources without depending on a registered watch.
    let mut dirty: HashSet<PathBuf> = HashSet::new();
    let mut overflowed = false;
    loop {
        if !owns_generation(generation) {
            return Ok(());
        }
        if std::time::Instant::now() >= next_presence {
            let verified = crate::volume::verify_source_dirs(&data_root);
            let (missing, substituted, unknown) = match verified {
                Ok(status) => (status.missing, status.substituted, false),
                Err(error) => {
                    logging::warn("source availability verification failed", json!({"error": {"message": error}}));
                    (source_dirs.clone(), Vec::new(), true)
                }
            };
            if !owns_generation(generation) { return Ok(()); }
            let recovering = last_status.is_some();
            let status = (missing.clone(), substituted.clone(), unknown);
            if last_status.as_ref() != Some(&status) {
                reconcile_source_conditions(&data_root, &source_dirs, &missing, &substituted)?;
                crate::failure_runtime::emit_or_record(&app, "source://availability",
                    json!({"missing": missing, "substituted": substituted, "presenceUnknown": unknown}));
                last_status = Some(status);
            }
            let registrations = reconcile_watches(&source_dirs, &mut watchers, &missing, &substituted, unknown, &mut |index, root| {
                let sender = tx.clone();
                let overflow = recovery_flags[index].1.clone();
                watch_root(Path::new(root), move |event| forward_or_flag_overflow(&sender, &overflow, event))
                    .map_err(|error| error.to_string())
            });
            if !owns_generation(generation) { return Ok(()); }
            if registrations.iter().any(|(_, result)| result.is_ok()) {
                crate::failure_runtime::clear("watcher-failed", None);
            }
            for (index, result) in registrations {
                record_root_condition(&app, &source_dirs[index], result.as_ref().err().map(String::as_str))?;
                recovery_flags[index].1.store(recovering && result.is_ok(), Ordering::SeqCst);
            }
            for (index, watcher) in watchers.iter().enumerate() {
                if watcher.is_none() { recovery_flags[index].1.store(false, Ordering::SeqCst); }
            }
            next_presence = std::time::Instant::now() + Duration::from_secs(10);
        }
        // Wake periodically so a settings-driven watcher replacement can
        // retire this generation even on a completely quiet filesystem.
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => collect(event, &mut dirty, &mut overflowed, &data_root),
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("watcher event channel disconnected".to_string())
            }
        }
        if dirty.is_empty() && !overflowed && recovery_flags.iter().all(|(_, flag)| !flag.load(Ordering::SeqCst)) {
            continue;
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

        let mut recovery_roots: Vec<String> = recovery_flags.iter()
            .filter(|(_, flag)| flag.swap(false, Ordering::SeqCst))
            .map(|(root, _)| root.clone()).collect();
        if overflowed {
            overflowed = false;
            recovery_roots = source_dirs.iter().enumerate().filter(|(index, _)| watchers[*index].is_some()).map(|(_, root)| root.clone()).collect();
        }
        if !recovery_roots.is_empty() {
            recover_roots(&app, &recovery_roots, generation);
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
                    Some(failure) => {
                        logging::warn("watcher pass needs recovery", json!({ "error": { "message": failure } }));
                        let roots = affected_roots(&source_dirs, &pass.failed.iter().map(|(dir, _)| dir.clone()).collect::<Vec<_>>());
                        recover_roots(&app, &roots, generation);
                    },
                    None => crate::failure_runtime::clear("watcher-failed", None),
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
                    json!({ "changed": changed, "sections": pass.sections }),
                );
            }
            Err(err) => {
                logging::warn(
                    "watcher pass failed",
                    json!({ "error": { "message": &err } }),
                );
                recover_roots(&app, &affected_roots(&source_dirs, &dirs), generation);
            }
        }
    }
}

/// Registration belongs to the availability transition, including when no
/// watch has ever existed. Failed registration stays owed for the next probe.
fn reconcile_watches<W>(
    roots: &[String], watches: &mut [Option<W>], missing: &[String], substituted: &[String], unknown: bool,
    register: &mut dyn FnMut(usize, &str) -> Result<W, String>,
) -> Vec<(usize, Result<(), String>)> {
    let mut results = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        if unknown || missing.contains(root) || substituted.contains(root) {
            watches[index] = None;
        } else if watches[index].is_none() {
            let result = register(index, root).map(|watch| { watches[index] = Some(watch); });
            results.push((index, result));
        }
    }
    results
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

fn affected_roots(configured: &[String], dirs: &[PathBuf]) -> Vec<String> {
    let matched: Vec<String> = configured.iter().filter(|root| dirs.iter().any(|dir|
        crate::winpath::for_fs(dir).starts_with(crate::winpath::for_fs(Path::new(root)).as_ref())
    )).cloned().collect();
    // A path alias which cannot be attributed safely must not drop recovery.
    if matched.is_empty() { configured.to_vec() } else { matched }
}

fn recover_roots(app: &tauri::AppHandle, roots: &[String], generation: u64) {
    if roots.is_empty() || !owns_generation(generation) { return; }
    crate::failure_runtime::emit_or_record(app, "watch://rescan-needed", json!({ "roots": roots }));
    let sections = crate::section_changes::SectionLog::default();
    for root in roots {
        if !crate::volume_io::is_dir(Path::new(root)).unwrap_or(false) { continue; }
        let result = crate::scan_runtime::with_watcher_claim(
            move || !owns_generation(generation),
            || {
                let data_root = crate::paths::data_root()?;
                let config = crate::storage::config(&data_root)?;
                let settings = scanner::settings_from_config(Some(&config), &data_root, chrono::Utc::now().timestamp_millis());
                let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
                let mut trace = crate::activity::WorkTrace::begin(crate::activity::ActivityOwner::Watcher, None, None);
                let report = trace.progress_reporter();
                sections.track(&conn);
                let result = scanner::run_source_check_scoped(&conn, &settings, Some(std::slice::from_ref(root)), &|progress| report(progress.done, progress.total));
                sections.finish(&conn);
                if result.as_ref().is_ok_and(|summary| summary.failures > 0) {
                    trace.finish(crate::activity::ActivityState::Failed, None);
                } else { trace.result(&result); }
                result
            },
        );
        if !owns_generation(generation) { return; }
        match result {
            Ok(summary) if summary.failures == 0 => {
                crate::failure_runtime::clear("watcher-recovery-failed", Some(root));
            }
            result => {
                let detail = match result {
                    Err(error) => error,
                    Ok(summary) => format!("{} source entries could not be checked", summary.failures),
                };
                if crate::failure_runtime::record_active(app, "watcher-recovery-failed", Some(root), &detail).is_err() {
                    return;
                }
            }
        }
    }
    crate::file_information_runtime::wake(app.clone());
    // A successful check of one root cannot hide an unresolved condition in
    // another root, or claim that a watcher which stopped is running again.
    let unresolved = crate::paths::data_root().and_then(|root| {
        let conn = crate::index_store::open(&root.join(crate::storage::INDEX_DB_FILE_NAME))?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM active_issues WHERE kind IN ('watcher-recovery-failed', 'watcher-failed', 'watcher-root-failed'))", [], |row| row.get::<_, bool>(0)).map_err(|error| error.to_string())
    });
    let rescan_needed = match unresolved {
        Ok(value) => value,
        Err(error) => { crate::scan_runtime::record_runtime_failure(app, "watcher-recovery-failed", &error); true }
    };
    crate::failure_runtime::emit_or_record(app, "watch://recovered", json!({ "roots": roots, "rescanNeeded": rescan_needed, "sections": sections.sections() }));
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

/// Source availability is a current condition, not a notification or scan failure.
pub fn reconcile_source_conditions(
    data_root: &Path, configured: &[String], missing: &[String], substituted: &[String],
) -> Result<(), String> {
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let unavailable = missing.iter().chain(substituted).collect::<HashSet<_>>();
    let mut statement = conn.prepare("SELECT path FROM active_issues WHERE kind = 'source-unavailable'")
        .map_err(|error| error.to_string())?;
    let prior = statement.query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    for root in prior {
        if !unavailable.contains(&root) {
            crate::index_store::clear_issues(&conn, &root, &["source-unavailable"])?;
        }
    }
    for root in configured {
        if unavailable.contains(root) {
            crate::index_store::clear_issues(&conn, root, &["watcher-root-failed", "watcher-recovery-failed", scanner::WALK_ERROR])?;
            let key = if substituted.contains(root) { "source.substitutedTitle" } else {
                crate::volume::unavailable_message_with(data_root, root, &crate::volume::volume_identity)
            };
            crate::index_store::upsert_issue_with_descriptor(&conn, Some(root), "source-unavailable", Some(key), None, "")?;
        }
    }
    Ok(())
}

fn record_root_condition(
    app: &tauri::AppHandle,
    root: &str,
    error: Option<&str>,
) -> Result<(), String> {
    if let Some(message) = error {
        crate::failure_runtime::record_active(app, "watcher-root-failed", Some(root), message)
    } else {
        crate::failure_runtime::clear("watcher-root-failed", Some(root));
        Ok(())
    }
}

/// The `notify` callback: forwards into the bounded channel, or flags
/// overflow instead of blocking the callback thread when it is full (W-L4).
fn forward_or_flag_overflow(
    tx: &mpsc::SyncSender<notify::Result<notify::Event>>,
    overflowed: &AtomicBool,
    event: notify::Result<notify::Event>,
) {
    if event.as_ref().map_or(true, |event| event.need_rescan()) || tx.try_send(event).is_err() {
        overflowed.store(true, Ordering::SeqCst);
    }
}

/// Registers one recursive watch on `root` in its own bounded call. A root
/// whose drive does not answer fails within the bound (and every later call on
/// that drive fails at once), so it never keeps another root unwatched.
pub fn watch_root(
    root: &Path,
    handler: impl notify::EventHandler,
) -> std::io::Result<notify::RecommendedWatcher> {
    let root_path = root.to_path_buf();
    crate::volume_io::call(root, crate::volume_io::Op::Watch, None, move || {
        use notify::Watcher;
        let mut watcher = notify::recommended_watcher(handler).map_err(std::io::Error::other)?; // volume_io worker
        watcher
            .watch(&root_path, notify::RecursiveMode::Recursive)
            .map_err(std::io::Error::other)?;
        Ok(watcher)
    })
}

/// Folds one watcher event into the dirty-directory set.
///
/// A file event maps to its PARENT directory, since the drain calls
/// `read_dir` on whatever lands here — inserting the file path instead makes
/// that call fail silently and new photos never appear.
fn collect(
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
                    || crate::file_identity::is_private_tmp_name(&path)
                {
                    continue;
                }
                // Bounded: a volume that does not answer reads as "not a
                // directory", and the parent's re-read then reports it.
                let dir = if crate::volume_io::is_dir(&path).unwrap_or(false) {
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
    sections: Option<HashSet<crate::queries::SectionLocation>>,
}

impl WatchPass {
    /// How many index rows the pass changed.
    pub(crate) fn changed(&self) -> u64 {
        self.changed
    }

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
    let mut pass = WatchPass { changed: 0, failed: Vec::new(), sections: None };
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
    let data_root = crate::paths::data_root()?;
    // The settings in memory only: `storage::load_app_data` drains the
    // pending quarantine list for Main to report, and the watcher has no
    // reporting surface of its own.
    let config = crate::storage::config(&data_root)?;
    let settings = scanner::settings_from_config(
        Some(&config),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let affected_dirs: Vec<String> = dirs
        .iter()
        .map(|dir| crate::winpath::for_fs(dir).to_string_lossy().to_string())
        .collect();
    crate::volume::enforce_no_substitution(&data_root, &affected_roots(&settings.source_dirs, dirs))?;
    let repair_roots = scanner::begin_scoped_index_repair(&conn, &affected_dirs)?;

    let timezone = crate::queries::display_timezone();
    let before = crate::queries::sections_under_directories(&conn, &affected_dirs, timezone)?;
    let WatchPass { changed, failed, .. } = restat_batch(&conn, dirs, &settings, &|| {
        if !owns_generation(generation) {
            return Err(scanner::CANCELLED.to_string());
        }
        // A pending foreground action or urgent preparation takes the index
        // between directories.
        crate::scan_runtime::yield_at_safe_point().map(|_| ())
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
    let mut sections = before;
    sections.extend(crate::queries::sections_under_directories(&conn, &affected_dirs, timezone)?);
    Ok(WatchPass { changed, failed, sections: (!sections.is_empty()).then_some(sections) })
}
