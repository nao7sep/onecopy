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
use std::sync::mpsc;
use std::time::Duration;

use notify::Watcher;
use serde_json::json;

use crate::logging;
use crate::scanner::{self, ScanLists};

/// Re-stats ONE directory (non-recursive): upserts its current files and marks
/// rows for vanished files missing — the walk logic scoped to a single dir.
pub fn restat_dir(
    conn: &rusqlite::Connection,
    dir: &Path,
    lists: &ScanLists,
) -> Result<u64, String> {
    let mut changed = 0u64;
    let mut present: HashSet<String> = HashSet::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue };
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            let abs = path.to_string_lossy().to_string();
            if abs.contains(crate::trash::TRASH_DIR_NAME) {
                continue;
            }
            present.insert(abs);
            if scanner::upsert_file(conn, &path, lists)? != scanner::Upsert::Unchanged {
                changed += 1;
            }
        }
    }

    // Rows directly in this dir whose files are gone → missing.
    let dir_str = dir.to_string_lossy().to_string();
    let mut stmt = conn
        .prepare("SELECT abs_path FROM paths WHERE dir_path = ?1 AND missing = 0")
        .map_err(|e| e.to_string())?;
    let known: Vec<String> = stmt
        .query_map([&dir_str], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    for path in known {
        if !present.contains(&path) {
            conn.execute("UPDATE paths SET missing = 1 WHERE abs_path = ?1", [&path])
                .map_err(|e| e.to_string())?;
            changed += 1;
        }
    }

    Ok(changed)
}

/// Starts the watcher thread over the configured source roots. Best-effort:
/// a watcher that cannot start logs one warn and the app continues (rescan
/// remains the manual path).
pub fn start(app: tauri::AppHandle, source_dirs: Vec<String>) {
    if source_dirs.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(err) => {
                logging::warn(
                    "watcher could not start; manual rescan remains",
                    json!({ "error": { "message": err.to_string() } }),
                );
                return;
            }
        };
        for root in &source_dirs {
            if let Err(err) = watcher.watch(Path::new(root), notify::RecursiveMode::Recursive) {
                logging::warn(
                    "watcher could not watch a root",
                    json!({ "root": root, "error": { "message": err.to_string() } }),
                );
            }
        }
        logging::info("watcher started", json!({ "roots": source_dirs.len() }));

        let mut dirty: HashSet<PathBuf> = HashSet::new();
        let mut overflowed = false;
        loop {
            // Block for the first event, then drain for a debounce window.
            match rx.recv() {
                Ok(event) => collect(event, &mut dirty, &mut overflowed),
                Err(_) => break, // watcher dropped; thread ends
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while let Ok(event) = rx.recv_timeout(
                deadline.saturating_duration_since(std::time::Instant::now()),
            ) {
                collect(event, &mut dirty, &mut overflowed);
            }

            if overflowed {
                overflowed = false;
                dirty.clear();
                let _ = tauri::Emitter::emit(
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
                Ok(0) => {}
                Ok(changed) => {
                    let _ = tauri::Emitter::emit(
                        &app,
                        "watch://updated",
                        json!({ "changed": changed }),
                    );
                }
                Err(err) => {
                    logging::warn(
                        "watcher pass failed",
                        json!({ "error": { "message": err } }),
                    );
                }
            }
        }
    });
}

fn collect(
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

/// Re-stats the dirty directories and runs the pending pipeline tail. Waits
/// out an in-flight full scan (its walk covers the changes anyway).
fn process_dirty(app: &tauri::AppHandle, dirs: &[PathBuf]) -> Result<u64, String> {
    if crate::scan_running() {
        return Ok(0);
    }
    let data_root = crate::paths::data_root(app)?;
    let loaded = crate::storage::load_app_data(app)?;
    let settings = scanner::settings_from_config(
        loaded.config.as_ref(),
        &data_root,
        chrono::Utc::now().timestamp_millis(),
    );
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;

    let mut changed = 0u64;
    for dir in dirs {
        changed += restat_dir(&conn, dir, &settings.lists)?;
    }
    if changed > 0 {
        // The shared pipeline tail (hash → … → group), so the watcher can
        // never drift from the scan and rescan paths on what a pass covers.
        let mut summary = scanner::ScanSummary::default();
        scanner::run_pipeline_tail(&conn, &settings, &|_, _| {}, &mut summary)?;
        logging::info("watcher pass", json!({ "dirs": dirs.len(), "changed": changed }));
    }
    Ok(changed)
}
