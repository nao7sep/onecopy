use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

pub mod backup_store;
pub mod binaries;
pub mod binaries_manager;
pub mod extensions;
pub mod hashing;
pub mod index_store;
pub mod live_photo;
pub mod logging;
pub mod media_protocol;
pub mod metadata;
mod nanoid;
pub mod operations;
pub mod paths;
pub mod preview;
pub mod queries;
pub mod resolution;
pub mod scanner;
pub mod similarity;
pub mod storage;
pub mod timestamps;
pub mod trash;
pub mod video;
pub mod volume;
pub mod watcher;

/// Whether the full scan pipeline is currently running (the watcher defers to
/// it — the scan's own walk covers whatever changed).
pub fn scan_running() -> bool {
    SCAN_RUNNING.load(Ordering::SeqCst)
}

// Records the panic payload, location, and (when RUST_BACKTRACE is set) the
// backtrace, flushes, then defers to the previous hook so the process still
// aborts and prints as usual.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let backtrace = std::backtrace::Backtrace::capture();
        logging::error(
            "panic",
            json!({
                "error": {
                    "message": payload,
                    "location": location,
                    "backtrace": format!("{backtrace}"),
                }
            }),
        );
        // The error line is already on disk (the logger is unbuffered); defer to
        // the previous hook so the process still aborts and prints as usual.
        default_hook(info);
    }));
}

// --- Commands ---
//
// Every fallible command runs inside logging::boundary(): one `debug` line at
// the start, one `info` (success) or `error` (failure) line with the elapsed
// duration. Expected outcomes are modeled as serde-tagged enums where they
// arise, not as errors.

// Config + state + data root in one startup round-trip.
#[tauri::command]
fn load_app_data(app: AppHandle) -> Result<storage::LoadedAppData, String> {
    logging::boundary(
        "load_app_data",
        json!({}),
        || {
            let mut data = storage::load_app_data(&app)?;
            data.debug_enabled = logging::debug_enabled();
            Ok(data)
        },
        |d| json!({ "hasConfig": d.config.is_some(), "hasState": d.state.is_some() }),
    )
}

// Config and state saves are PATCHES merged core-side: the core holds the
// file, so it is the one owner of the read-modify-write, and no frontend
// store's stale cached copy can blind-overwrite another's save. Returns the
// merged document so the caller can publish it without a second read.
#[tauri::command]
fn patch_config(app: AppHandle, patch: Value) -> Result<Value, String> {
    logging::boundary(
        "patch_config",
        json!({}),
        || storage::patch_config(&app, &patch),
        |_| json!({}),
    )
}

#[tauri::command]
fn patch_state(app: AppHandle, patch: Value) -> Result<Value, String> {
    logging::boundary(
        "patch_state",
        json!({}),
        || storage::patch_state(&app, &patch),
        |_| json!({}),
    )
}

// One scan pipeline at a time; a second start is a no-op reported as `false`.
static SCAN_RUNNING: AtomicBool = AtomicBool::new(false);

// The live scan worker, joined at exit so a quit interrupts the scan through
// the cooperative cancel flag instead of killing it mid-write.
static SCAN_THREAD: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> =
    std::sync::Mutex::new(None);

// The cache root, resolved at setup (config `cacheDir` or `<root>/cache`) and
// read by the mediacache protocol handler. RwLock, not OnceLock: a cache move
// swaps the root mid-session (the storage-path conventions' honest-relocation
// contract — no restart, no redirect-only trap).
static CACHE_ROOT: std::sync::RwLock<Option<std::path::PathBuf>> = std::sync::RwLock::new(None);

fn cache_root_or(data_root: &std::path::Path) -> std::path::PathBuf {
    CACHE_ROOT
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_else(|| data_root.join(storage::CACHE_DIR_NAME))
}

fn set_cache_root(path: std::path::PathBuf) {
    if let Ok(mut guard) = CACHE_ROOT.write() {
        *guard = Some(path);
    }
}

// The storage root, for the mediafile protocol's hash→path lookups.
static DATA_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

// Serves ORIGINAL files by content hash (`mediafile://localhost/<hash>`) with
// single-range support — what makes <video> seeking and the 100% zoom view
// work. Only hashes present in the index resolve; the path comes from the DB,
// so the webview never handles filesystem paths. Large files without a Range
// get a 206 head-chunk to push players into ranged loading instead of a
// whole-file read.
fn serve_mediafile(request: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let not_found = || {
        tauri::http::Response::builder()
            .status(404)
            .body(Vec::new())
            .expect("static response")
    };
    // None of these branches is expected control flow (unlike a cache miss in
    // serve_mediacache), so each failure leaves a log line — an unmounted
    // drive otherwise reads as blank views with a silent session log.
    let warn_404 = |reason: &str, detail: String| {
        logging::warn(
            "mediafile request failed",
            json!({ "reason": reason, "detail": detail }),
        );
        not_found()
    };
    let Some(data_root) = DATA_ROOT.get() else {
        return warn_404("data root unset", String::new());
    };
    let hash = request.uri().path().trim_start_matches('/');
    if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return not_found(); // malformed request, not a filesystem failure
    }

    let conn = match index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME)) {
        Ok(conn) => conn,
        Err(err) => return warn_404("index open failed", err),
    };
    let path: Option<String> = conn
        .query_row(
            "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
            [hash],
            |r| r.get(0),
        )
        .ok();
    let Some(path) = path else {
        return warn_404("no live path for hash", hash.to_string());
    };

    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(err) => return warn_404("original unreadable", format!("{path}: {err}")),
    };
    let total = match file.metadata().map(|m| m.len()) {
        Ok(total) => total,
        Err(err) => return warn_404("metadata failed", format!("{path}: {err}")),
    };

    let content_type = content_type_for(&path);
    let range_header = request
        .headers()
        .get("Range")
        .and_then(|v| v.to_str().ok());

    // The span decision is pure and unit-tested in media_protocol: it caps
    // every span (a webview's `bytes=0-` otherwise resolves to the whole file,
    // and this handler is synchronous, so wry runs it on the main thread), and
    // it applies the head-chunk shortcut only to streamable content — a
    // truncated image is a broken tile, not a partial one.
    let (start, end, status) =
        resolve_range(range_header, total, is_streamable(content_type));

    let length = end - start + 1;
    let mut bytes = vec![0u8; length as usize];
    if let Err(err) = file
        .seek(SeekFrom::Start(start))
        .and_then(|_| file.read_exact(&mut bytes))
    {
        return warn_404("read failed", format!("{path}: {err}"));
    }

    let mut builder = tauri::http::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", length.to_string());
    if status == 206 {
        builder = builder.header("Content-Range", format!("bytes {start}-{end}/{total}"));
    }
    builder.body(bytes).unwrap_or_else(|_| not_found())
}

// Range parsing, content types, and byte sniffing live in media_protocol.rs
// (pure and unit-tested there; lib.rs is the bootstrap and has no tests).
use media_protocol::{
    content_type_for, is_streamable, resolve_range, sniff_image_content_type,
};

// Serves `mediacache://localhost/thumb-<hash>` and `/preview-<hash>` straight
// from the hash-keyed cache (http://mediacache.localhost/… on Windows). Cache
// entries are content-addressed, so responses are immutable-cacheable; misses
// are plain 404s (the grid falls back to a placeholder).
fn serve_mediacache(request: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let not_found = || {
        tauri::http::Response::builder()
            .status(404)
            .body(Vec::new())
            .expect("static response")
    };
    let Some(root) = CACHE_ROOT.read().ok().and_then(|guard| guard.clone()) else {
        return not_found();
    };
    let cache = preview::CachePaths::new(root);
    let path = request.uri().path().trim_start_matches('/');
    let file = if let Some(hash) = path.strip_prefix("thumb-") {
        cache.thumb(hash)
    } else if let Some(hash) = path.strip_prefix("preview-") {
        cache.preview(hash)
    } else if let Some(rest) = path.strip_prefix("strip-") {
        // strip-<hash>-<index>
        match rest.rsplit_once('-') {
            Some((hash, index)) => match index.parse::<u32>() {
                Ok(index) => video::strip_path(&cache, hash, index),
                Err(_) => return not_found(),
            },
            None => return not_found(),
        }
    } else {
        return not_found();
    };
    match std::fs::read(&file) {
        Ok(bytes) => {
            // Preview entries can be byte-copies of originals (the derive
            // fast path when an image already fits the preview edge), so the
            // .webp cache name may hold JPEG/PNG/GIF bytes — sniff the magic
            // instead of trusting the extension.
            let content_type = sniff_image_content_type(&bytes);
            tauri::http::Response::builder()
                .status(200)
                .header("Content-Type", content_type)
                .header("Cache-Control", "public, max-age=31536000, immutable")
                .body(bytes)
                .unwrap_or_else(|_| not_found())
        }
        Err(_) => not_found(),
    }
}

// Launches the full scan pipeline (walk → hash → extract → resolve → pair →
// derive) on a worker thread. Progress arrives as `scan://progress` events,
// completion as `scan://done` (with the summary) or `scan://error`. Returns
// false when a scan is already running.
#[tauri::command]
fn start_scan(app: AppHandle) -> Result<bool, String> {
    spawn_scan(app, true)
}

// The one scan spawner: `include_walk` distinguishes the full scan (walk +
// tail) from the startup resume, which runs only the pipeline tail over the
// checkpointed pending rows. A cancelled run (app exit) reports as
// `scan://done { cancelled: true }`, never as an error — the pending rows are
// the resume point, and the next launch picks them up.
/// Whether checkpointed work is waiting and worth resuming — the one probe
/// behind both resume triggers, the startup one and the ffmpeg install that
/// unblocks formats it alone can decode. A failed probe is logged and read as
/// "no": a resume is a convenience, never a gate on anything.
fn scan_resume_wanted(data_root: &std::path::Path) -> bool {
    resume_plan(data_root).0
}

/// `(resume_wanted, needs_walk)`. A root whose walk was interrupted must be
/// RE-WALKED, not just tailed: `pending_work_exists` probes rows, and a
/// cancelled walk leaves whole directories with no rows at all, so once the
/// tail drains what the partial walk created it reports clean forever while
/// months stay silently empty.
fn resume_plan(data_root: &std::path::Path) -> (bool, bool) {
    let ffmpeg_present = binaries_manager::ffmpeg_path(data_root).is_file();
    let Ok(conn) = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME)) else {
        return (false, false);
    };
    let roots = match storage::load_config_source_dirs(data_root) {
        Ok(roots) => roots,
        Err(err) => {
            logging::warn(
                "source dirs unreadable for resume",
                json!({ "error": { "message": err } }),
            );
            Vec::new()
        }
    };
    let needs_walk = !roots.is_empty()
        && match scanner::walk_owed(&conn, &roots) {
            Ok(owed) => owed,
            Err(err) => {
                logging::warn(
                    "walk-owed probe failed",
                    json!({ "error": { "message": err } }),
                );
                false
            }
        };
    if needs_walk {
        return (true, true);
    }
    match scanner::pending_work_exists(&conn, ffmpeg_present) {
        Ok(pending) => (pending, false),
        Err(err) => {
            logging::warn(
                "pending-work probe failed",
                json!({ "error": { "message": err } }),
            );
            (false, false)
        }
    }
}

fn spawn_scan(app: AppHandle, include_walk: bool) -> Result<bool, String> {
    if SCAN_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
    scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    let prepared = (|| -> Result<(), String> {
        let data_root = paths::data_root(&app)?;
        let loaded = storage::load_app_data(&app)?;
        let settings = scanner::settings_from_config(
            loaded.config.as_ref(),
            &data_root,
            chrono::Utc::now().timestamp_millis(),
        );
        let db_file = data_root.join(storage::INDEX_DB_FILE_NAME);
        let handle = app.clone();

        let worker = std::thread::spawn(move || {
            // Sleep inhibition for the day-scale first index (config-gated).
            // Display sleep stays allowed; only system sleep is held off.
            let _awake = settings.keep_awake.then(|| {
                keepawake::Builder::default()
                    .idle(true)
                    .sleep(true)
                    .reason("Indexing media")
                    .app_name("OneCopy")
                    .create()
                    .ok()
            });

            let emit_progress = |phase: &str, detail: String| {
                let _ = handle.emit("scan://progress", json!({ "phase": phase, "detail": detail }));
            };

            let outcome = index_store::open(&db_file).and_then(|conn| {
                if include_walk {
                    scanner::run_full_scan(&conn, &settings, &emit_progress)
                } else {
                    let mut summary = scanner::ScanSummary::default();
                    scanner::run_pipeline_tail(&conn, &settings, &emit_progress, &mut summary)
                        .map(|()| summary)
                }
            });
            match outcome {
                Ok(summary) => {
                    logging::info("scan complete", json!({ "summary": summary }));
                    let _ = handle.emit("scan://done", json!({ "summary": summary }));
                }
                Err(err) if err == scanner::CANCELLED => {
                    logging::info("scan cancelled", json!({ "resumesAtNextLaunch": true }));
                    let _ = handle.emit("scan://done", json!({ "cancelled": true }));
                }
                Err(err) => {
                    logging::error("scan failed", json!({ "error": { "message": err.clone() } }));
                    let _ = handle.emit("scan://error", json!({ "message": err }));
                }
            }
            SCAN_RUNNING.store(false, Ordering::SeqCst);
        });
        if let Ok(mut slot) = SCAN_THREAD.lock() {
            *slot = Some(worker);
        }
        Ok(())
    })();

    match prepared {
        Ok(()) => Ok(true),
        Err(err) => {
            SCAN_RUNNING.store(false, Ordering::SeqCst);
            Err(err)
        }
    }
}

// Moves the cache tree to a new root (None = the default `<root>/cache`):
// copy → verify → swap the live root and config → delete the old subtrees.
// The developer's decided contract: a REAL move behind a blocking progress
// modal, no restart, never a redirect that orphans tens of GB. Refused while
// a scan runs (derive writes into the cache mid-move). On failure the copied
// partial is removed and the old location stays live.
#[tauri::command]
fn move_cache(app: AppHandle, new_dir: Option<String>) -> Result<Value, String> {
    logging::boundary(
        "move_cache",
        json!({ "newDir": new_dir }),
        || {
            if scan_running() {
                return Err("a scan is running — move the cache after it finishes".to_string());
            }
            let data_root = paths::data_root(&app)?;
            let old_root = cache_root_or(&data_root);
            let new_root = new_dir
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| data_root.join(storage::CACHE_DIR_NAME));
            if new_root == old_root {
                return Ok(json!({ "movedBytes": 0, "unchanged": true }));
            }
            if new_root.starts_with(&old_root) || old_root.starts_with(&new_root) {
                return Err("the new cache location cannot nest with the old one".to_string());
            }
            std::fs::create_dir_all(&new_root).map_err(|e| e.to_string())?;

            let handle = app.clone();
            let emit_progress = |copied: u64, total: u64| {
                let _ = handle.emit(
                    "cache-move://progress",
                    json!({ "copiedBytes": copied, "totalBytes": total }),
                );
            };
            match preview::move_cache_tree(&old_root, &new_root, &emit_progress) {
                Ok(moved) => {
                    storage::patch_config(&app, &json!({ "cacheDir": new_dir }))?;
                    set_cache_root(new_root);
                    // Only the cache's own subtrees — either root may be a
                    // user-picked folder holding unrelated content.
                    preview::remove_cache_subtrees(&old_root);
                    Ok(json!({ "movedBytes": moved }))
                }
                Err(err) => {
                    preview::remove_cache_subtrees(&new_root);
                    Err(err)
                }
            }
        },
        |result| result.clone(),
    )
}

// The volume-loss guard (the session gate's runtime counterpart): destructive
// operations refuse to run while any configured source directory is absent —
// a vanished volume must block deletes, not let them half-apply.
fn ensure_sources_present(app: &AppHandle) -> Result<(), String> {
    let status = verify_source_dirs(app)?;
    if !status.missing.is_empty() {
        return Err(format!(
            "destructive operations are blocked: {} configured source directorie(s) are missing ({})",
            status.missing.len(),
            status.missing.join(", ")
        ));
    }
    if !status.substituted.is_empty() {
        return Err(format!(
            "destructive operations are blocked: {} source directorie(s) sit on a DIFFERENT volume \
             than the one recorded — a substituted drive with the same folder layout ({})",
            status.substituted.len(),
            status.substituted.join(", ")
        ));
    }
    Ok(())
}

#[derive(serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct SourceDirsStatus {
    missing: Vec<String>,
    substituted: Vec<String>,
}

// Presence AND identity verification over the configured source dirs: a dir
// that is not there is missing; a dir whose volume identity differs from the
// recorded one is substituted (the developer's backup drives share identical
// trees, so presence alone proves nothing). First sight records the identity
// — the "when the directory was added" moment as the core observes it. Rows
// for since-removed dirs are pruned; a volume without a readable identity
// degrades to presence-only, logged at debug.
fn verify_source_dirs(app: &AppHandle) -> Result<SourceDirsStatus, String> {
    let loaded = storage::load_app_data(app)?;
    let data_root = paths::data_root(app)?;
    let settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;

    let mut status = SourceDirsStatus::default();
    for dir in &settings.source_dirs {
        let path = std::path::Path::new(dir);
        if !path.is_dir() {
            status.missing.push(dir.clone());
            continue;
        }
        let Some(current) = volume::volume_identity(path) else {
            logging::debug(
                "no volume identity readable; presence-only verification",
                json!({ "dir": dir }),
            );
            continue;
        };
        match volume::check_identity(&conn, dir, &current)? {
            volume::IdentityCheck::FirstSight => logging::info(
                "source volume identity recorded",
                json!({ "dir": dir, "identity": current }),
            ),
            volume::IdentityCheck::Substituted { recorded } => {
                logging::warn(
                    "source volume SUBSTITUTED",
                    json!({ "dir": dir, "recorded": recorded, "current": current }),
                );
                status.substituted.push(dir.clone());
            }
            volume::IdentityCheck::Unchanged => {}
        }
    }

    // Identities for directories no longer configured are stale — prune.
    volume::prune_identities(&conn, &settings.source_dirs)?;

    Ok(status)
}

// Deletes one logical item — every copy plus companions — to trash, or
// permanently when `permanent` is true. The item is addressed the way the grid
// knows it: by hash, or by path id for unhashed other-files.
#[tauri::command]
fn delete_item(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
    permanent: bool,
) -> Result<operations::DeleteOutcome, String> {
    logging::boundary(
        "delete_item",
        json!({ "hash": hash, "pathId": path_id, "permanent": permanent }),
        || {
            ensure_sources_present(&app)?;
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let cache_root = cache_root_or(&data_root);
            let cache = preview::CachePaths::new(cache_root);
            let item = match (&hash, path_id) {
                (Some(hash), _) => operations::ItemRef::Hash(hash),
                (None, Some(id)) => operations::ItemRef::PathId(id),
                (None, None) => return Err("delete_item needs a hash or a pathId".to_string()),
            };
            let mode = if permanent {
                operations::DeleteMode::Permanent
            } else {
                operations::DeleteMode::Trash
            };
            operations::delete_item(&conn, &data_root, &cache, item, mode)
        },
        |outcome| json!({ "deleted": outcome.deleted_files, "failed": outcome.failed_files }),
    )
}

// One (kind, month) section's grid items, same month keys and timezone as the
// counts so the two always agree.
#[tauri::command]
fn get_section_items(
    app: AppHandle,
    kind: String,
    month: String,
) -> Result<Vec<queries::SectionItem>, String> {
    logging::boundary(
        "get_section_items",
        json!({ "kind": kind, "month": month }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::section_items(&conn, &kind, &month, display_timezone())
        },
        |items| json!({ "items": items.len() }),
    )
}

fn display_timezone() -> chrono_tz::Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse().ok())
        .unwrap_or(chrono_tz::UTC)
}

// Moves or copies one logical item out to a destination directory. Modes:
// "move-trash-rest" (plain drag), "move-delete-rest" (Shift), "copy" (Cmd/Ctrl).
// Destinations under a configured source root are rejected — moving files into
// a scanned directory would only re-index them.
#[tauri::command]
fn move_item_out(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
    dest_dir: String,
    mode: String,
) -> Result<operations::MoveOutOutcome, String> {
    logging::boundary(
        "move_item_out",
        json!({ "hash": hash, "pathId": path_id, "destDir": dest_dir, "mode": mode }),
        || {
            ensure_sources_present(&app)?;
            let data_root = paths::data_root(&app)?;
            let loaded = storage::load_app_data(&app)?;
            let settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
            let dest = std::path::Path::new(&dest_dir);
            for source in &settings.source_dirs {
                if dest.starts_with(source) {
                    return Err(format!(
                        "destination {dest_dir} lies inside the scanned directory {source}; \
                         move-out targets must be outside every source directory"
                    ));
                }
            }

            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let cache_root = cache_root_or(&data_root);
            let cache = preview::CachePaths::new(cache_root);
            let item = match (&hash, path_id) {
                (Some(hash), _) => operations::ItemRef::Hash(hash),
                (None, Some(id)) => operations::ItemRef::PathId(id),
                (None, None) => return Err("move_item_out needs a hash or a pathId".to_string()),
            };
            let mode = match mode.as_str() {
                "move-trash-rest" => operations::MoveOutMode::MoveTrashRest,
                "move-delete-rest" => operations::MoveOutMode::MoveDeleteRest,
                "copy" => operations::MoveOutMode::CopyKeepAll,
                other => return Err(format!("unknown move-out mode: {other}")),
            };
            operations::move_out(&conn, &data_root, &cache, item, dest, mode)
        },
        |outcome| {
            json!({
                "exported": outcome.exported,
                "skipped": outcome.skipped_identical,
                "conflicts": outcome.conflicts.len(),
            })
        },
    )
}

// Destination-tree support: immediate subdirectories of one directory (the
// tree expands lazily; files are never listed — it is a destination panel, not
// a file manager).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DirEntry {
    name: String,
    path: String,
    has_children: bool,
}

#[tauri::command]
fn list_subdirs(path: String) -> Result<Vec<DirEntry>, String> {
    let mut entries: Vec<DirEntry> = Vec::new();
    let read = std::fs::read_dir(&path).map_err(|e| e.to_string())?;
    for entry in read.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // dotfolders (incl. .onecopy-trash) stay out of the tree
        }
        let child_path = entry.path();
        let has_children = std::fs::read_dir(&child_path)
            .map(|mut children| {
                children.any(|c| c.as_ref().is_ok_and(|e| e.file_type().is_ok_and(|t| t.is_dir())))
            })
            .unwrap_or(false);
        entries.push(DirEntry {
            name,
            path: child_path.to_string_lossy().to_string(),
            has_children,
        });
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(entries)
}

// Creates a subfolder under a tree node. The name must be case-insensitively
// unique within its directory (storage-path conventions' hard invariant).
#[tauri::command]
fn create_subdir(parent: String, name: String) -> Result<String, String> {
    logging::boundary(
        "create_subdir",
        json!({ "parent": parent, "name": name }),
        || {
            let trimmed = name.trim();
            // Control characters (a pasted newline is the real case) would
            // create a directory whose name cannot be typed or read sanely.
            if trimmed.is_empty()
                || trimmed.contains(['/', '\\'])
                || trimmed.chars().any(char::is_control)
            {
                return Err(
                    "folder names must be non-empty, slash-free, and single-line".to_string(),
                );
            }
            let parent_path = std::path::Path::new(&parent);
            let lower = trimmed.to_lowercase();
            if let Ok(read) = std::fs::read_dir(parent_path) {
                for entry in read.flatten() {
                    if entry.file_name().to_string_lossy().to_lowercase() == lower {
                        return Err(format!(
                            "\"{trimmed}\" already exists here (names are case-insensitively unique)"
                        ));
                    }
                }
            }
            let target = parent_path.join(trimmed);
            std::fs::create_dir(&target).map_err(|e| e.to_string())?;
            Ok(target.to_string_lossy().to_string())
        },
        |path| json!({ "created": path }),
    )
}

// Deletes a tree folder ONLY when empty — remove_dir refuses otherwise, which
// is the entire safety model (empty folders render distinctly in the tree).
#[tauri::command]
fn delete_empty_dir(path: String) -> Result<(), String> {
    logging::boundary(
        "delete_empty_dir",
        json!({ "path": path }),
        || std::fs::remove_dir(&path).map_err(|e| e.to_string()),
        |_| json!({}),
    )
}

// Is this directory empty? Drives the tree's distinct empty-folder rendering.
#[tauri::command]
fn dir_is_empty(path: String) -> Result<bool, String> {
    let mut read = std::fs::read_dir(&path).map_err(|e| e.to_string())?;
    Ok(read.next().is_none())
}

// Re-resolves every indexed item from stored evidence and rebuilds similar
// groups — the settings-change path (timezone, good range, thresholds); no
// file is read.
#[tauri::command]
fn re_resolve_all(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "re_resolve_all",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let loaded = storage::load_app_data(&app)?;
            let settings = scanner::settings_from_config(
                loaded.config.as_ref(),
                &data_root,
                chrono::Utc::now().timestamp_millis(),
            );
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let stats = scanner::resolve_from_evidence(
                &conn,
                &settings.resolution,
                scanner::ResolveScope::All,
            )?;
            similarity::rebuild_groups(&conn, &settings.similarity)?;
            Ok(stats.resolved)
        },
        |resolved| json!({ "resolved": resolved }),
    )
}

// Scoped rescan: re-stats exactly the directories that contributed files to
// one section (never the whole roots), then runs the pending pipeline tail.
// The full per-root walk remains the Scan button's escape hatch.
#[tauri::command]
fn rescan_section(app: AppHandle, kind: String, month: String) -> Result<u64, String> {
    logging::boundary(
        "rescan_section",
        json!({ "kind": kind, "month": month }),
        || {
            let data_root = paths::data_root(&app)?;
            let loaded = storage::load_app_data(&app)?;
            let settings = scanner::settings_from_config(
                loaded.config.as_ref(),
                &data_root,
                chrono::Utc::now().timestamp_millis(),
            );
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let dirs = queries::section_dirs(&conn, &kind, &month, display_timezone())?;
            let mut changed = 0u64;
            for dir in &dirs {
                changed += watcher::restat_dir(&conn, std::path::Path::new(dir), &settings.lists)?;
            }
            // The tail also runs when nothing changed on disk but checkpointed
            // work is still pending — a section rescan is the recovery the
            // user reaches for after an interrupted scan, and gating the whole
            // tail on `changed` made it a no-op exactly then.
            if changed > 0 || scanner::pending_work_exists(&conn, settings.ffmpeg.is_some())? {
                let mut summary = scanner::ScanSummary::default();
                scanner::run_pipeline_tail(&conn, &settings, &|_, _| {}, &mut summary)?;
            }
            Ok(changed)
        },
        |changed| json!({ "changed": changed }),
    )
}

// The first-class issues surface: unreadable files, decode failures,
// copies-disagree anomalies, delete/copy errors — a silent skip never happens.
#[tauri::command]
fn get_issues(app: AppHandle, limit: Option<u32>) -> Result<serde_json::Value, String> {
    logging::boundary(
        "get_issues",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let (total, rows) = queries::issues(&conn, limit.unwrap_or(500))?;
            Ok(json!({ "total": total, "rows": rows }))
        },
        |v| json!({ "total": v.get("total") }),
    )
}

// Managed ffmpeg: presence + facts + derived status.
#[tauri::command]
fn binaries_state(app: AppHandle) -> Result<binaries_manager::FfmpegState, String> {
    let data_root = paths::data_root(&app)?;
    Ok(binaries_manager::state(&data_root))
}

// Installs or updates ffmpeg on a worker thread; progress arrives as
// `binaries://progress`, completion as `binaries://done` / `binaries://error`.
#[tauri::command]
fn binaries_install(app: AppHandle) -> Result<(), String> {
    let data_root = paths::data_root(&app)?;
    let handle = app.clone();
    std::thread::spawn(move || {
        let emit = |phase: &str, detail: String| {
            let _ = handle.emit("binaries://progress", json!({ "phase": phase, "detail": detail }));
        };
        match binaries_manager::install_or_update(&data_root, emit) {
            Ok(facts) => {
                let _ = handle.emit("binaries://done", json!({ "facts": facts }));
                // Installing ffmpeg IS the remedy for everything it blocked —
                // HEIC/AVIF stills and every video — so pick that work up now
                // rather than leaving the library on placeholder tiles until
                // the next launch. The same tail-only resume the startup path
                // runs, and its single-run guard makes it a no-op mid-scan.
                if scan_resume_wanted(&data_root) {
                    // Report what actually happened. A scan already running
                    // makes spawn_scan a no-op AND cannot pick this work up
                    // itself — its ScanSettings captured `ffmpeg: None` at
                    // spawn — so the blocked rows wait for the next scan,
                    // rescan, or watcher pass. The wizard leads straight into
                    // this case by design: "Finish and scan" stays enabled
                    // while the install runs.
                    match spawn_scan(handle.clone(), false) {
                        Ok(true) => {
                            logging::info("scan resumed after ffmpeg install", json!({}))
                        }
                        Ok(false) => logging::info(
                            "ffmpeg installed mid-scan; blocked items wait for the next pass",
                            json!({}),
                        ),
                        Err(err) => logging::warn(
                            "resume after ffmpeg install failed",
                            json!({ "error": { "message": err } }),
                        ),
                    }
                }
            }
            Err(err) => {
                logging::warn("ffmpeg install failed", json!({ "error": { "message": err.clone() } }));
                let _ = handle.emit("binaries://error", json!({ "message": err }));
            }
        }
    });
    Ok(())
}

// Version check only — never installs; a failure writes nothing.
#[tauri::command]
fn binaries_check(app: AppHandle) -> Result<binaries_manager::FfmpegState, String> {
    logging::boundary(
        "binaries_check",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            binaries_manager::check_for_updates(&data_root)?;
            Ok(binaries_manager::state(&data_root))
        },
        |state| json!({ "status": format!("{:?}", state.status) }),
    )
}

// Wizard support: a fast extension-classified count of one directory tree —
// no stat, no hashing, just names — so an added directory shows its
// image/video/other numbers while the user is still in the wizard.
#[derive(serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct QuickCount {
    images: u64,
    videos: u64,
    others: u64,
}

// Roots whose in-flight quick count the wizard cancelled (directory removed
// mid-count): the walk checks membership and stops, so removing a huge
// directory stops its disk churn, not just discards the result.
static QUICK_COUNT_CANCELLED: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

#[tauri::command]
fn cancel_quick_count(root: String) {
    if let Ok(mut cancelled) = QUICK_COUNT_CANCELLED.lock() {
        cancelled.insert(root);
    }
}

#[tauri::command]
fn quick_count(app: AppHandle, root: String) -> Result<QuickCount, String> {
    logging::boundary(
        "quick_count",
        json!({ "root": root }),
        || {
            if let Ok(mut cancelled) = QUICK_COUNT_CANCELLED.lock() {
                cancelled.remove(&root);
            }
            let loaded = storage::load_app_data(&app)?;
            let data_root = paths::data_root(&app)?;
            let settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
            let mut count = QuickCount::default();
            let mut checked = 0u32;
            for entry in walkdir::WalkDir::new(&root).follow_links(false) {
                checked += 1;
                if checked % 256 == 0 {
                    let is_cancelled = QUICK_COUNT_CANCELLED
                        .lock()
                        .map(|mut c| c.remove(&root))
                        .unwrap_or(false);
                    if is_cancelled {
                        return Err("cancelled".to_string());
                    }
                }
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy();
                if entry.path().to_string_lossy().contains(trash::TRASH_DIR_NAME) {
                    continue;
                }
                let ext = extensions::lowercase_ext(&name);
                match extensions::classify(
                    &ext,
                    &settings.lists.images,
                    &settings.lists.videos,
                    &settings.lists.companions,
                ) {
                    extensions::Kind::Image => count.images += 1,
                    extensions::Kind::Video => count.videos += 1,
                    // Companions ride with primaries; the wizard counts them
                    // as other-files, matching how unattached ones display.
                    extensions::Kind::Companion | extensions::Kind::Other => count.others += 1,
                }
            }
            Ok(count)
        },
        |c| json!({ "images": c.images, "videos": c.videos, "others": c.others }),
    )
}

// Wizard support: is this a real IANA timezone name?
#[tauri::command]
fn validate_timezone(name: String) -> bool {
    name.parse::<chrono_tz::Tz>().is_ok()
}

// The session gate's check: configured source directories that are not
// currently present (an unmounted volume manifests as a missing directory).
#[tauri::command]
fn check_source_dirs(app: AppHandle) -> Result<SourceDirsStatus, String> {
    logging::boundary(
        "check_source_dirs",
        json!({}),
        || verify_source_dirs(&app),
        |status| json!({ "missing": status.missing.len(), "substituted": status.substituted.len() }),
    )
}

// The comparison view's group members for one item, best-first.
#[tauri::command]
fn get_similar_group(app: AppHandle, hash: String) -> Result<Vec<queries::GroupMember>, String> {
    logging::boundary(
        "get_similar_group",
        json!({ "hash": hash }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::similar_group_of(&conn, &hash)
        },
        |members| json!({ "members": members.len() }),
    )
}

// The metadata pane's detail for one logical item.
#[tauri::command]
fn get_item_detail(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
) -> Result<queries::ItemDetail, String> {
    logging::boundary(
        "get_item_detail",
        json!({ "hash": hash, "pathId": path_id }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::item_detail(&conn, hash.as_deref(), path_id)
        },
        |detail| json!({ "copies": detail.copy_paths.len() }),
    )
}

// Left-pane section counts (logical items per kind per month), bucketed in the
// OS display timezone.
#[tauri::command]
fn get_section_counts(app: AppHandle) -> Result<queries::SectionCounts, String> {
    logging::boundary(
        "get_section_counts",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            queries::section_counts(&conn, display_timezone())
        },
        |counts| {
            json!({
                "imageMonths": counts.images.len(),
                "videoMonths": counts.videos.len(),
                "otherMonths": counts.others.len(),
            })
        },
    )
}

// Receives a structured log object from the webview frontend and writes it to
// the session file (the frontend has no filesystem access of its own).
#[tauri::command]
fn log_event(entry: Value) {
    logging::emit_forwarded(entry);
}

// Reports whether developer-only `debug` logging is on, so the frontend can
// gate its own debug events identically (a dev build, or ONECOPY_DEBUG=1).
#[tauri::command]
fn logging_debug_enabled() -> bool {
    logging::debug_enabled()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Developer-only `debug` logging: on for a dev build, or when explicitly
    // requested via ONECOPY_DEBUG=1. Off (and compiled-quiet) in release.
    let debug_enabled = cfg!(debug_assertions)
        || std::env::var("ONECOPY_DEBUG")
            .map(|v| v == "1")
            .unwrap_or(false);

    let app = tauri::Builder::default()
        // Single instance first: destructive operations over a shared index DB
        // from two processes is asking for trouble; a second launch focuses the
        // first window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("mediacache", |_ctx, request| serve_mediacache(&request))
        .register_uri_scheme_protocol("mediafile", |_ctx, request| serve_mediafile(&request))
        .setup(move |app| {
            // Open the per-session log file under the app's own data dir. The Rust
            // core has filesystem access even though the webview is sandboxed, and
            // it routes through the single storage-root resolver (paths::data_root)
            // so the log directory and the data directory share one source of
            // truth and both honor ONECOPY_HOME.
            let data_root = paths::data_root(app.handle())?;
            let log_path = data_root
                .join(paths::LOGS_DIR_NAME)
                .join(logging::session_filename());
            logging::init(&log_path, debug_enabled);
            install_panic_hook();

            // Open the write-through data-backup store once, best-effort, under the
            // same ONECOPY_HOME-aware root. If it cannot open, one warn is logged
            // and recording is disabled for the session — it never blocks startup.
            backup_store::init(data_root.join(backup_store::BACKUPS_DB_FILE_NAME));

            // Materialize config.json from the canonical defaults when absent —
            // the populated-but-not-yet-used point, before any consumer reads it.
            storage::materialize_config_if_missing(&data_root)?;

            // Create/verify the index schema so a schema problem surfaces at
            // startup, not mid-scan. Phase 2 owns a long-lived connection; this
            // one closes on drop.
            index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;

            // Download staging is crash debris by definition: wipe at launch.
            binaries_manager::reset_temp_dir(&data_root);

            // Resolve the cache root once for the mediacache protocol, then
            // sweep crash leftovers (hash-orphaned entries, stranded temps).
            let loaded = storage::load_app_data(app.handle())?;
            let cache_root = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0)
                .cache_root;
            set_cache_root(cache_root.clone());
            let _ = DATA_ROOT.set(data_root.clone());
            if let Ok(conn) = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME)) {
                let cache = preview::CachePaths::new(cache_root);
                match preview::startup_sweep(&conn, &cache) {
                    Ok(0) => {}
                    Ok(removed) => {
                        logging::info("cache sweep", json!({ "removed": removed }));
                    }
                    Err(err) => {
                        logging::warn("cache sweep failed", json!({ "error": { "message": err } }));
                    }
                }
            }

            // The watcher: ON by default, best-effort, over the configured
            // source roots (the Camera Roll inflow case). Restart picks up
            // source-dir changes; correctness never depends on it.
            let watch_settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
            let ffmpeg_present = watch_settings.ffmpeg.is_some();
            watcher::start(app.handle().clone(), watch_settings.source_dirs);

            // The one update switch (managed-runtime-dependencies): when ON,
            // an INSTALLED tool is checked at launch, throttled to ~daily so
            // launches never hammer the endpoints. Default off; a failed
            // check writes nothing (check_for_updates' own contract).
            let check_at_launch = loaded
                .config
                .as_ref()
                .and_then(|c| c.get("checkUpdatesAtLaunch"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if check_at_launch && ffmpeg_present {
                let facts = binaries_manager::load_facts(&data_root);
                let stale = facts
                    .last_checked_at_utc
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(24))
                    .unwrap_or(true);
                if stale {
                    let root = data_root.clone();
                    let handle = app.handle().clone();
                    std::thread::spawn(move || {
                        match binaries_manager::check_for_updates(&root) {
                            Ok(facts) => {
                                logging::info(
                                    "launch update check",
                                    json!({ "latestKnown": facts.latest_known_version }),
                                );
                                let _ = handle.emit("binaries://changed", json!({}));
                            }
                            Err(err) => {
                                logging::warn(
                                    "launch update check failed",
                                    json!({ "error": { "message": err } }),
                                );
                            }
                        }
                    });
                }
            }

            // Auto-resume: an interrupted scan leaves checkpointed pending
            // rows (unhashed media, underived images/videos); pick the work
            // back up without waiting for the user to press Scan. Includes the
            // WALK when a root was never walked to completion — the tail alone
            // cannot recover directories that have no rows at all, and would
            // otherwise report clean forever over a half-indexed library.
            let (resume, needs_walk) = resume_plan(&data_root);
            if resume {
                logging::info("scan resumed at startup", json!({ "walk": needs_walk }));
                let _ = spawn_scan(app.handle().clone(), needs_walk);
            }

            logging::info(
                "app startup",
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "build": if cfg!(debug_assertions) { "debug" } else { "release" },
                    "debugLogging": debug_enabled,
                    "logPath": log_path.to_string_lossy(),
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                }),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_app_data,
            patch_config,
            patch_state,
            start_scan,
            get_section_counts,
            get_section_items,
            get_item_detail,
            get_similar_group,
            delete_item,
            move_item_out,
            list_subdirs,
            create_subdir,
            delete_empty_dir,
            dir_is_empty,
            re_resolve_all,
            rescan_section,
            move_cache,
            get_issues,
            binaries_state,
            binaries_install,
            binaries_check,
            quick_count,
            cancel_quick_count,
            validate_timezone,
            check_source_dirs,
            log_event,
            logging_debug_enabled
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| match event {
        // Cooperative scan interruption: flag as soon as exit is requested so
        // the worker starts winding down, then join it at Exit — bounded by
        // the per-item cancel checks — so no SQLite write is killed halfway.
        tauri::RunEvent::ExitRequested { .. } => {
            scanner::SCAN_CANCEL.store(true, Ordering::Relaxed);
        }
        tauri::RunEvent::Exit => {
            scanner::SCAN_CANCEL.store(true, Ordering::Relaxed);
            if let Some(worker) = SCAN_THREAD.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = worker.join();
            }
            logging::info("app shutdown", json!({ "reason": "exit" }));
        }
        _ => {}
    });
}
