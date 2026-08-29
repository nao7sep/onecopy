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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use notify::Watcher;
use serde_json::json;

use crate::logging;
use crate::scanner::{self, ScanLists};

static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn restart_from_config(app: tauri::AppHandle) -> Result<(), String> {
    let data_root = crate::paths::data_root(&app)?;
    let source_dirs = crate::storage::load_config_source_dirs(&data_root)?;
    start(app, source_dirs);
    Ok(())
}

/// Re-stats ONE directory (non-recursive): upserts its current files and marks
/// rows for vanished files missing — the walk logic scoped to a single dir.
pub fn restat_dir(
    conn: &rusqlite::Connection,
    dir: &Path,
    lists: &ScanLists,
) -> Result<u64, String> {
    // notify reports ordinary Windows paths even when the full scan stores
    // their long-path spelling. Normalize at the boundary so a watcher pass
    // cannot turn one physical file into a second database row.
    let dir = crate::winpath::for_fs(dir);
    let dir = dir.as_ref();
    let mut changed = 0u64;
    let mut present: HashSet<String> = HashSet::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
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
        if abs.contains(crate::trash::TRASH_DIR_NAME) {
            continue;
        }
        present.insert(abs.clone());
        match scanner::upsert_file(conn, &path, lists) {
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
pub fn start(app: tauri::AppHandle, source_dirs: Vec<String>) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if source_dirs.is_empty() {
        return;
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
                Ok(Err(error)) => report_failure(&handle, &error),
                Err(payload) => {
                    let error = payload
                        .downcast_ref::<&str>()
                        .map(|value| (*value).to_string())
                        .or_else(|| payload.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "watcher stopped unexpectedly".to_string());
                    report_failure(&handle, &error);
                }
            }
        });
    if let Err(error) = started {
        report_failure(&app, &format!("could not start watcher thread: {error}"));
    }
}

fn run(app: tauri::AppHandle, source_dirs: Vec<String>, generation: u64) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(tx).map_err(|error| error.to_string())?;
    let mut watched = 0usize;
    for root in &source_dirs {
        if let Err(err) = watcher.watch(Path::new(root), notify::RecursiveMode::Recursive) {
            logging::warn(
                "watcher could not watch a root",
                json!({ "root": root, "error": { "message": err.to_string() } }),
            );
            record_root_condition(&app, root, Some(&err.to_string()))?;
        } else {
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
        if GENERATION.load(Ordering::SeqCst) != generation {
            return Ok(());
        }
        // Wake periodically so a settings-driven watcher replacement can
        // retire this generation even on a completely quiet filesystem.
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => collect(event, &mut dirty, &mut overflowed),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("watcher event channel disconnected".to_string())
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while let Ok(event) =
            rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            collect(event, &mut dirty, &mut overflowed);
        }

        if overflowed {
            overflowed = false;
            dirty.clear();
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
        match process_dirty(&app, &dirs) {
            Ok(0) => crate::failure_runtime::clear(&app, "watcher-failed", None)?,
            Ok(changed) => {
                crate::failure_runtime::clear(&app, "watcher-failed", None)?;
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

fn report_failure(app: &tauri::AppHandle, error: &str) {
    logging::error("watcher failed", json!({ "error": { "message": error } }));
    crate::scan_runtime::record_runtime_failure(app, "watcher-failed", error);
    for event in ["watch://failed", "watch://rescan-needed"] {
        crate::failure_runtime::emit_or_record(app, event, json!({ "reason": error }));
    }
}

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

/// Folds one watcher event into the dirty-directory set.
///
/// `pub` for the tests: a file event must map to its PARENT directory, since
/// the drain calls `read_dir` on whatever lands here — inserting the file path
/// instead makes that call fail silently and new photos never appear.
pub fn collect(
    event: notify::Result<notify::Event>,
    dirty: &mut HashSet<PathBuf>,
    overflowed: &mut bool,
) {
    match event {
        Ok(event) => {
            if event.need_rescan() {
                *overflowed = true;
                return;
            }
            for path in event.paths {
                let dir = if path.is_dir() {
                    path
                } else {
                    match path.parent() {
                        Some(parent) => parent.to_path_buf(),
                        None => continue,
                    }
                };
                if dir.to_string_lossy().contains(crate::trash::TRASH_DIR_NAME) {
                    continue;
                }
                dirty.insert(dir);
            }
        }
        Err(_) => *overflowed = true,
    }
}

/// Re-stats the dirty directories and leaves durable index debt for the
/// independent file-information owner. The shared index claim retains this
/// event batch until any active projection reaches a safe boundary.
fn process_dirty(app: &tauri::AppHandle, dirs: &[PathBuf]) -> Result<u64, String> {
    crate::scan_runtime::with_watcher_claim(|| process_dirty_claimed(app, dirs))
}

fn process_dirty_claimed(app: &tauri::AppHandle, dirs: &[PathBuf]) -> Result<u64, String> {
    let data_root = crate::paths::data_root(app)?;
    let loaded = crate::storage::load_app_data(app)?;
    let settings = scanner::settings_from_config(
        loaded.config.as_ref(),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let affected_dirs: Vec<String> = dirs
        .iter()
        .map(|dir| crate::winpath::for_fs(dir).to_string_lossy().to_string())
        .collect();
    let repair_roots = scanner::begin_scoped_index_repair(&conn, &affected_dirs)?;

    let mut changed = 0u64;
    for dir in dirs {
        changed += restat_dir(&conn, dir, &settings.lists)?;
    }
    if changed > 0 {
        logging::info(
            "watcher pass",
            json!({ "dirs": dirs.len(), "changed": changed }),
        );
        crate::file_information_runtime::wake(app.clone());
    } else {
        scanner::complete_scoped_index_repair(&conn, &repair_roots)?;
    }
    Ok(changed)
}
