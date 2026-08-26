use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

pub mod backup_store;
pub mod background_work;
pub mod derived_work;
pub mod derived_state;
pub mod binaries;
mod binaries_acquisition;
pub mod binaries_manager;
pub mod extensions;
pub mod file_identity;
pub mod fs_publish;
pub mod hashing;
pub mod face;
pub mod index_store;
mod instance_owner;
pub mod live_photo;
pub mod logging;
pub mod media_protocol;
pub mod metadata;
mod nanoid;
pub mod operations;
pub mod paths;
pub mod path_identity;
pub mod preview;
pub mod queries;
pub mod resolution;
pub mod scanner;
pub mod similarity;
pub mod similar_exclusions;
pub mod storage;
pub mod subprocess;
pub mod timestamps;
pub mod transcription;
pub mod trash;
pub mod video;
pub mod volume;
pub mod watcher;
pub mod winpath;

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
// THREADING: Tauri dispatches a plain `#[tauri::command]` on the MAIN thread
// (`ExecutionContext::Blocking` in tauri-macros). On macOS the main thread
// also commits the window's layer updates, so a command doing file, network,
// subprocess or decode work freezes the visible UI for its whole duration —
// which is how a 40 ms check came to feel like half a second of dead button.
// Every command below that touches the disk, the network, a subprocess or the
// index carries `(async)` so it runs on the async runtime instead. The ones
// left plain are pure or atomic (validate_timezone, logging_debug_enabled,
// transcribe_cancel), or must keep strict call order (log_event). The get_*
// reads are `(async)` as well — their responses may now arrive OUT OF ORDER,
// which the stores absorb with request-sequence guards (`staleGuard` in
// src/state/request-seq.ts): a 30k-item month query on the main thread was
// exactly the block a slow machine felt as a frozen window.
#[tauri::command(async)]
fn load_app_data(app: AppHandle) -> Result<storage::LoadedAppData, String> {
    logging::boundary(
        "load_app_data",
        json!({}),
        || {
            let mut data = storage::load_app_data(&app)?;
            data.debug_enabled = logging::debug_enabled();
            Ok(data)
        },
        |d| {
            json!({
                "hasConfig": d.config.is_some(),
                "hasState": d.state.is_some(),
                "quarantines": d.quarantines.len(),
            })
        },
    )
}

/// A store can also be quarantined mid-session — a patch reads the file it is
/// about to merge into — where there is no load result to ride home on. The
/// patch hands its own outcome here, and it is pushed to the same reporting
/// surface, so the rule ("every quarantine reaches the user") has no hole.
fn report_quarantine(app: &AppHandle, record: Option<storage::QuarantineRecord>) {
    if let Some(record) = record {
        let _ = app.emit("storage://quarantined", json!({ "quarantines": [record] }));
    }
}

// Config and state saves are PATCHES merged core-side: the core holds the
// file, so it is the one owner of the read-modify-write, and no frontend
// store's stale cached copy can blind-overwrite another's save. Returns the
// merged document so the caller can publish it without a second read.
#[tauri::command(async)]
fn patch_config(app: AppHandle, mut patch: Value) -> Result<Value, String> {
    logging::boundary(
        "patch_config",
        json!({}),
        || {
            if let Some(value) = patch.get_mut("defaultTimezone") {
                let name = value
                    .as_str()
                    .ok_or("Default timezone must be an IANA timezone name")?;
                *value = Value::String(resolution::parse_timezone_name(name)?.to_string());
            }
            let outcome = storage::patch_config(&app, &patch)?;
            report_quarantine(&app, outcome.quarantined);
            Ok(outcome.merged)
        },
        |_| json!({}),
    )
}

#[tauri::command(async)]
fn patch_state(app: AppHandle, patch: Value) -> Result<Value, String> {
    logging::boundary(
        "patch_state",
        json!({}),
        || {
            let outcome = storage::patch_state(&app, &patch)?;
            report_quarantine(&app, outcome.quarantined);
            Ok(outcome.merged)
        },
        |_| json!({}),
    )
}

// One scan pipeline at a time; a second start is a no-op reported as `false`.
static SCAN_RUNNING: AtomicBool = AtomicBool::new(false);

// The live scan worker, joined at exit so a quit interrupts the scan through
// the cooperative cancel flag instead of killing it mid-write.
static SCAN_THREAD: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> =
    std::sync::Mutex::new(None);

// The storage root, for the mediafile protocol's hash→path lookups.
pub(crate) static DATA_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

fn cache_root() -> Option<std::path::PathBuf> {
    DATA_ROOT
        .get()
        .map(|root| root.join(storage::CACHE_DIR_NAME))
}

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
    let key = request.uri().path().trim_start_matches('/');
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return not_found(); // malformed request, not a filesystem failure
    }

    let conn = match index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME)) {
        Ok(conn) => conn,
        Err(err) => return warn_404("index open failed", err),
    };
    // Two key forms, both DB lookups so the webview never holds a path:
    // a content hash, or `path-<id>` for files that legitimately have no
    // hash — an audio memo or document with a unique size is never
    // content-read, so hash-only serving could not reach it at all.
    let path: Option<String> = match key.strip_prefix("path-") {
        Some(id) => id.parse::<i64>().ok().and_then(|id| {
            conn.query_row(
                "SELECT abs_path FROM paths WHERE id = ?1 AND missing = 0",
                [id],
                |r| r.get(0),
            )
            .ok()
        }),
        None => conn
            .query_row(
                "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
                [key],
                |r| r.get(0),
            )
            .ok(),
    };
    let Some(path) = path else {
        return warn_404("no live path for key", key.to_string());
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
    let Some(root) = cache_root() else {
        return not_found();
    };
    let cache = preview::CachePaths::new(root);
    let path = request.uri().path().trim_start_matches('/');
    let file = if let Some(hash) = path.strip_prefix("thumb-") {
        cache.thumb(hash)
    } else if let Some(hash) = path.strip_prefix("preview-") {
        cache.preview(hash)
    } else if let Some(hash) = path.strip_prefix("fullres-") {
        cache.fullres(hash)
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

// Launches the index pipeline (walk → hash → extract → resolve → pair) on a
// worker thread. Progress arrives as `scan://progress` events,
// completion as `scan://done` (with the summary) or `scan://error`. Returns
// false when a scan is already running.
#[tauri::command(async)]
fn start_scan(app: AppHandle) -> Result<bool, String> {
    spawn_scan(app, true)
}

// The one scan spawner: `include_walk` distinguishes the full scan (walk +
// tail) from the startup resume, which runs only the pipeline tail over the
// checkpointed pending rows. A cancelled run (app exit) reports as
// `scan://done { cancelled: true }`, never as an error — the pending rows are
// the resume point, and the next launch picks them up.
/// `(resume_wanted, needs_walk)`. A root whose walk was interrupted must be
/// RE-WALKED, not just tailed: `pending_index_work_exists` probes rows, and a
/// cancelled walk leaves whole directories with no rows at all, so once the
/// tail drains what the partial walk created it reports clean forever while
/// months stay silently empty.
fn resume_plan(data_root: &std::path::Path) -> (bool, bool) {
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
    match scanner::pending_index_work_exists(&conn) {
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
        let config = storage::read_config_for_setup(&data_root)?;
        let settings = scanner::settings_from_config(
            config.as_ref(),
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
                    scanner::run_index_tail(&conn, &settings, &emit_progress, &mut summary)
                        .map(|()| summary)
                }
            });
            match outcome {
                Ok(summary) => {
                    logging::info("scan complete", json!({ "summary": summary }));
                    let _ = handle.emit("scan://done", json!({ "summary": summary }));
                    derived_work::wake(true);
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
    let data_root = paths::data_root(app)?;
    let config = storage::read_config_for_setup(&data_root)?;
    let settings = scanner::settings_from_config(config.as_ref(), &data_root, 0);
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
        match volume::check_identity(&data_root, dir, &current)? {
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
    volume::prune_identities(&data_root, &settings.source_dirs)?;

    Ok(status)
}

// Deletes one logical item — every copy plus companions — to trash, or
// permanently when `permanent` is true. The item is addressed the way the grid
// knows it: by hash, or by path id for unhashed other-files.
#[tauri::command(async)]
fn delete_item(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
    permanent: bool,
) -> Result<operations::DeleteOutcome, String> {
    // Derived work must never compete with a user operation for the disk.
    let _heavy = derived_work::heavy_op();
    logging::boundary(
        "delete_item",
        json!({ "hash": hash, "pathId": path_id, "permanent": permanent }),
        || {
            ensure_sources_present(&app)?;
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let cache = preview::CachePaths::new(data_root.join(storage::CACHE_DIR_NAME));
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
#[tauri::command(async)]
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
#[tauri::command(async)]
fn move_item_out(
    app: AppHandle,
    hash: Option<String>,
    path_id: Option<i64>,
    dest_dir: String,
    mode: String,
) -> Result<operations::MoveOutOutcome, String> {
    // Derived work must never compete with a user operation for the disk.
    let _heavy = derived_work::heavy_op();
    logging::boundary(
        "move_item_out",
        json!({ "hash": hash, "pathId": path_id, "destDir": dest_dir, "mode": mode }),
        || {
            ensure_sources_present(&app)?;
            let data_root = paths::data_root(&app)?;
            let config = storage::read_config_for_setup(&data_root)?;
            let settings = scanner::settings_from_config(config.as_ref(), &data_root, 0);
            let dest = std::path::Path::new(&dest_dir);
            for source in &settings.source_dirs {
                if path_identity::directory_is_within(dest, std::path::Path::new(source))? {
                    return Err(format!(
                        "destination {dest_dir} lies inside the scanned directory {source}; \
                         move-out targets must be outside every source directory"
                    ));
                }
            }

            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let cache = preview::CachePaths::new(data_root.join(storage::CACHE_DIR_NAME));
            let item = match (&hash, path_id) {
                (Some(hash), _) => operations::ItemRef::Hash(hash),
                (None, Some(id)) => operations::ItemRef::PathId(id),
                (None, None) => return Err("move_item_out needs a hash or a pathId".to_string()),
            };
            // Same binary under DIFFERENT names (Phase 33): moving or copying
            // delivers exactly one name, and which one lands must never be a
            // surprise — for move-the-rest modes the other names would die
            // with the deleted copies. Blocked core-side (the UI badges and
            // pre-checks, but this is the authority); Reveal-per-copy is the
            // manual resolution path. Case-insensitive: IMG.JPG and img.jpg
            // are one name on the fleet's volumes.
            if let Some(hash) = &hash {
                let distinct: i64 = conn
                    .query_row(
                        "SELECT COUNT(DISTINCT lower(file_name)) FROM paths \
                         WHERE content_hash = ?1 AND missing = 0 AND companion_of IS NULL",
                        [hash],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if distinct > 1 {
                    return Err(format!(
                        "this item's copies carry {distinct} different names — \
                         reveal the copies and resolve the names first"
                    ));
                }
            }
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

#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
fn delete_empty_dir(path: String) -> Result<(), String> {
    logging::boundary(
        "delete_empty_dir",
        json!({ "path": path }),
        || std::fs::remove_dir(&path).map_err(|e| e.to_string()),
        |_| json!({}),
    )
}

// Is this directory empty? Drives the tree's distinct empty-folder rendering.
#[tauri::command(async)]
fn dir_is_empty(path: String) -> Result<bool, String> {
    let mut read = std::fs::read_dir(&path).map_err(|e| e.to_string())?;
    Ok(read.next().is_none())
}

// Opens a subdirectory of the app's data root in the OS file manager (the
// "Reveal logs folder" menu item). The path is BUILT HERE from paths.rs and a
// vetted subdir name — the frontend names a folder, never a path (paths.rs:
// "the frontend never reconstructs ~/.onecopy itself"). Routing through the
// opener plugin's RUST api rather than its JS command also sidesteps the
// plugin's permission scope, which applies to webview calls only — the JS
// route silently rejected every path because the scope allow-list was empty,
// and a `void openPath(...)` threw the rejection away.
#[tauri::command(async)]
fn reveal_data_subdir(app: AppHandle, name: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let root = paths::data_root(&app)?;
    // A vetted set, not a join of caller input: this command must never become
    // "open any path the webview asks for".
    let target = match name.as_str() {
        "logs" => root.join(paths::LOGS_DIR_NAME),
        other => return Err(format!("not a revealable folder: {other}")),
    };
    if !target.is_dir() {
        return Err(format!("{} does not exist yet", target.display()));
    }
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

// Opens an indexed item in its OS default app (the preview's "Open in player"
// codec-fallback). The path comes from the INDEX, never from the webview — a
// hash is resolved to a live copy here, so this can only ever open a file the
// scan actually indexed. Same reason as reveal_data_subdir for the Rust-side
// opener: the JS route was scope-rejected into a silent no-op, which left the
// fallback button for unplayable codecs doing nothing at all.
#[tauri::command(async)]
fn open_item_externally(app: AppHandle, hash: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let data_root = paths::data_root(&app)?;
    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
    let path: String = conn
        .query_row(
            "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
            [&hash],
            |r| r.get(0),
        )
        .map_err(|_| "no live copy of this item".to_string())?;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| e.to_string())
}

// Re-resolves every indexed item from stored evidence. Similarity is marked
// stale for its sole owner to rebuild; this command never performs derived
// work itself.
#[tauri::command(async)]
fn re_resolve_all(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "re_resolve_all",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let config = storage::read_config_for_setup(&data_root)?;
            let settings = scanner::settings_from_config(
                config.as_ref(),
                &data_root,
                chrono::Utc::now().timestamp_millis(),
            );
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let stats = scanner::resolve_from_evidence(
                &conn,
                &settings.resolution,
                scanner::ResolveScope::All,
            )?;
            scanner::pair_companions(&conn, settings.pairing_enabled)?;
            derived_work::wake(true);
            Ok(stats.resolved)
        },
        |resolved| json!({ "resolved": resolved }),
    )
}

// Scoped rescan: re-stats exactly the directories that contributed files to
// one section (never the whole roots), then runs the pending pipeline tail.
// The full per-root walk remains the Scan button's escape hatch.
#[tauri::command(async)]
fn rescan_section(app: AppHandle, kind: String, month: String) -> Result<u64, String> {
    logging::boundary(
        "rescan_section",
        json!({ "kind": kind, "month": month }),
        || {
            let data_root = paths::data_root(&app)?;
            let config = storage::read_config_for_setup(&data_root)?;
            let settings = scanner::settings_from_config(
                config.as_ref(),
                &data_root,
                chrono::Utc::now().timestamp_millis(),
            );
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let dirs = queries::section_dirs(&conn, &kind, &month, display_timezone())?;
            let mut changed = 0u64;
            for dir in &dirs {
                changed += watcher::restat_dir(&conn, std::path::Path::new(dir), &settings.lists)?;
            }
            // Finish any interrupted index checkpoints too. Derived media is
            // woken after the index tail instead of being smuggled into the
            // rescan command.
            if changed > 0 || scanner::pending_index_work_exists(&conn)? {
                let mut summary = scanner::ScanSummary::default();
                scanner::run_index_tail(&conn, &settings, &|_, _| {}, &mut summary)?;
                derived_work::wake(true);
            }
            Ok(changed)
        },
        |changed| json!({ "changed": changed }),
    )
}

// The first-class issues surface: unreadable files, decode failures,
// copies-disagree anomalies, delete/copy errors — a silent skip never happens.
#[tauri::command(async)]
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

// On-demand derive for ONE clicked photo the scan's bulk pass has not reached
// (walk-order; on a slow machine its tail is hours away). The preview surface
// calls this when its cache entry 404s, then reloads the entry. Idempotent
// and cheap when the entry already exists.
#[tauri::command(async)]
fn ensure_preview(app: AppHandle, hash: String) -> Result<String, String> {
    logging::boundary(
        "ensure_preview",
        json!({ "hash": hash }),
        || {
            let data_root = paths::data_root(&app)?;
            let config = storage::read_config_for_setup(&data_root)?;
            let canonical =
                derived_work::ensure_preview(&app, &data_root, config.as_ref(), &hash)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            derived_work::notify_item_update(&app, &conn, "previews", &hash, &canonical);
            let _ = app.emit("derived://issues", json!({}));
            Ok(canonical)
        },
        |canonical| json!({ "canonicalHash": canonical }),
    )
}

// The 100% view's on-demand conversion for formats the webview cannot paint
// (HEIC/AVIF — WebView2 paints neither; routing every platform through the
// same path keeps behaviour identical and testable on macOS). Runs on the
// command pool, never in the synchronous protocol handler; the view calls
// this and then loads `mediacache://fullres-<hash>`.
#[tauri::command(async)]
fn ensure_fullres(app: AppHandle, hash: String) -> Result<(), String> {
    logging::boundary(
        "ensure_fullres",
        json!({ "hash": hash }),
        || {
            let _work = background_work::begin_manual(&app, "previews")?;
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let cache_root = cache_root().ok_or("data root unset")?;
            let cache = preview::CachePaths::new(cache_root);
            // Presence decides availability, same rule as the scan settings.
            let ffmpeg = binaries_manager::ffmpeg_path(&data_root);
            let ffmpeg = ffmpeg.exists().then_some(ffmpeg);
            preview::ensure_fullres(&conn, &cache, ffmpeg.as_deref(), &hash)
        },
        |_| json!({}),
    )
}

// On-demand transcription (Design: Video handling): runs on its own thread —
// minutes-long and memory-heavy (~2–2.5 GB while running, released after) —
// with progress/done/error events; the transcript lands in the cache and
// `transcript_get` serves it thereafter. The process-wide claim prevents a
// manual run and coordinated background work from loading two models; cancel is immediate.
#[tauri::command(async)]
fn transcribe(app: AppHandle, hash: String) -> Result<(), String> {
    let data_root = paths::data_root(&app)?;
    let cache_root = cache_root().ok_or("data root unset")?;
    let work = background_work::begin_manual(&app, "transcripts")?;
    let claim = transcription::claim()?;
    let handle = app.clone();
    std::thread::spawn(move || {
        let _work = work;
        let result = (|| -> Result<String, String> {
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let video: String = conn
                .query_row(
                    "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
                    [&hash],
                    |r| r.get(0),
                )
                .map_err(|_| "no live copy of this video".to_string())?;
            let cache = preview::CachePaths::new(cache_root);
            let model_spec = binaries_manager::spec_of("whisper-large-v3-turbo")
                .expect("whisper model is registered");
            let model_state = binaries_manager::state_of(&data_root, model_spec);
            let model = (model_state.status != binaries::BinaryStatus::NotInstalled)
                .then(|| binaries_manager::installed_path(&data_root, model_spec));
            let ffmpeg = binaries_manager::ffmpeg_path(&data_root);
            let ffmpeg = ffmpeg.exists().then_some(ffmpeg);
            let tools_available = model.is_some() && ffmpeg.is_some();
            let progress_handle = handle.clone();
            let progress_hash = hash.clone();
            let text = transcription::transcribe_to_cache_claimed(
                &claim,
                &cache,
                model.as_deref(),
                ffmpeg.as_deref(),
                std::path::Path::new(&video),
                &hash,
                move |percent| {
                    background_work::report_manual_progress(
                        &progress_handle,
                        "transcripts",
                        percent.clamp(0, 100) as u64,
                        100,
                    );
                    let _ = progress_handle.emit(
                        "transcribe://progress",
                        json!({ "hash": progress_hash, "percent": percent }),
                    );
                },
            );
            match text {
                Ok(text) => {
                    derived_state::record_transcript_success(
                        &conn,
                        &hash,
                        &video,
                        !text.trim().is_empty(),
                    )?;
                    Ok(text)
                }
                Err(error)
                    if error == scanner::CANCELLED || transcription::is_cancelled() =>
                {
                    Err(error)
                }
                Err(error) if !tools_available => Err(error),
                Err(error) => {
                    derived_state::record_transcript_failure(&conn, &hash, &video, &error)?;
                    Err(error)
                }
            }
        })();
        match result {
            Ok(text) => {
                let _ = handle.emit("transcribe://done", json!({ "hash": hash, "text": text }));
            }
            Err(err) => {
                logging::warn(
                    "transcription failed",
                    json!({ "hash": hash, "error": { "message": err.clone() } }),
                );
                let _ = handle.emit("transcribe://error", json!({ "hash": hash, "message": err }));
            }
        }
    });
    Ok(())
}

// The transcript's explicit output state. A missing cache entry behind a
// ready receipt is repaired back to pending here rather than displayed as a
// false success.
#[tauri::command(async)]
fn transcript_get(app: AppHandle, hash: String) -> Result<derived_state::TranscriptResult, String> {
    let data_root = paths::data_root(&app)?;
    let cache_root = cache_root().ok_or("data root unset")?;
    let cache = preview::CachePaths::new(cache_root);
    let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
    derived_state::transcript_result(&conn, &cache, &hash)
}

// Comparison-mode fullscreen for the spread windows (Phase 33). macOS only:
// simple fullscreen (the pre-Lion kind) hides the menu bar and dock WITHOUT
// the Spaces animation that made real fullscreen unusable at keystroke pace.
// Elsewhere a borderless window at exact monitor bounds already covers the
// taskbar, so the command is a no-op — never tauri's fullscreen fallback,
// which would change proven Windows behaviour.
#[tauri::command(async)]
fn set_window_simple_fullscreen(app: AppHandle, label: String, enable: bool) -> Result<(), String> {
    let window = app
        .get_webview_window(&label)
        .ok_or_else(|| format!("no window labeled {label}"))?;
    #[cfg(target_os = "macos")]
    {
        window.set_simple_fullscreen(enable).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, enable);
        Ok(())
    }
}

    // The frontend's throttled input ping — the coordinator's whole view
// of the user. Atomic store; keeping it plain (main-thread) is deliberate,
// it must never queue behind async work.
#[tauri::command]
fn note_user_activity() {
    derived_work::note_activity();
}

#[tauri::command(async)]
fn background_work_snapshot(
    app: AppHandle,
) -> Result<background_work::BackgroundWorkSnapshot, String> {
    let data_root = paths::data_root(&app)?;
    background_work::snapshot(
        &data_root,
        derived_work::similarity_dirty(),
    )
}

#[tauri::command]
fn background_work_set_paused(
    app: AppHandle,
    class_id: Option<String>,
    paused: bool,
) -> Result<(), String> {
    background_work::set_paused(&app, class_id.as_deref(), paused)
}

/// Ephemeral viewport hints for the fixed derived-work coordinator. Output
/// facts remain the only queue; closing the app loses nothing that must be
/// recovered.
#[tauri::command]
fn prioritize_derived_work(
    selected_hash: Option<String>,
    visible_hashes: Vec<String>,
    section_kind: Option<String>,
    section_month: Option<String>,
) -> Result<(), String> {
    let section = match (section_kind, section_month) {
        (Some(kind), Some(month)) if matches!(kind.as_str(), "image" | "video") => {
            let bounds = queries::month_bounds(&month, display_timezone())?;
            Some(derived_work::SectionPriority {
                kind,
                start_ms: bounds.map(|value| value.0),
                end_ms: bounds.map(|value| value.1),
            })
        }
        _ => None,
    };
    derived_work::set_priority(selected_hash, visible_hashes, section);
    Ok(())
}

#[tauri::command]
fn transcribe_cancel() -> bool {
    transcription::request_cancel()
}

// The Trash surface: standing sizes per trash root// The Trash surface: standing sizes per trash root, and the one deliberately
// destructive convenience — emptying a root. The trash is otherwise
// write-only; these are the only two readers the design allows.
#[tauri::command(async)]
fn trash_overview(app: AppHandle) -> Result<Vec<trash::TrashRootInfo>, String> {
    logging::boundary(
        "trash_overview",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let dirs = storage::load_config_source_dirs(&data_root)?;
            Ok(trash::overview(&dirs, &data_root))
        },
        |roots| json!({ "roots": roots.len() }),
    )
}

// Emptying is PERMANENT (the trash is the safety net; emptying it removes
// the net for everything inside). The frontend confirms with the totals
// before calling; the root path must be one `trash_overview` reported —
// verified here so the command can never delete an arbitrary tree.
#[tauri::command(async)]
fn trash_empty(app: AppHandle, root: String) -> Result<(), String> {
    // Derived work must never compete with a user operation for the disk.
    let _heavy = derived_work::heavy_op();
    logging::boundary(
        "trash_empty",
        json!({ "root": root }),
        || {
            let data_root = paths::data_root(&app)?;
            let dirs = storage::load_config_source_dirs(&data_root)?;
            let known = trash::overview(&dirs, &data_root);
            if !known.iter().any(|r| r.root == root) {
                return Err("not a known trash root".to_string());
            }
            trash::empty_root(std::path::Path::new(&root))
        },
        |_| json!({}),
    )
}

// Dismissal is the user's half of the issues lifecycle: scan-derived rows
// clear themselves when a scan finds the condition resolved, these two clear
// everything else. Deleting is honest — the log file keeps the history, and a
// dismissed-but-persisting scan condition returns on the next scan.
#[tauri::command(async)]
fn dismiss_issue(app: AppHandle, id: i64) -> Result<(), String> {
    logging::boundary(
        "dismiss_issue",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            conn.execute("DELETE FROM issues WHERE id = ?1", [id])
                .map_err(|e| e.to_string())?;
            Ok(())
        },
        |_| json!({}),
    )
}

#[tauri::command(async)]
fn dismiss_all_issues(app: AppHandle) -> Result<(), String> {
    logging::boundary(
        "dismiss_all_issues",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            conn.execute("DELETE FROM issues", [])
                .map_err(|e| e.to_string())?;
            Ok(())
        },
        |_| json!({}),
    )
}

#[tauri::command(async)]
fn retry_issue(app: AppHandle, id: i64) -> Result<bool, String> {
    logging::boundary(
        "retry_issue",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let retried = derived_state::retry_issue(&conn, id)?;
            if retried {
                derived_work::wake(false);
            }
            Ok(retried)
        },
        |retried| json!({ "retried": retried }),
    )
}

#[tauri::command(async)]
fn retry_all_issues(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "retry_all_issues",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let retried = derived_state::retry_all(&conn)?;
            if retried > 0 {
                derived_work::wake(false);
            }
            Ok(retried)
        },
        |retried| json!({ "retried": retried }),
    )
}

// Every managed dependency's presence + facts + derived status, in display
// order — the Managed tools window renders one row per entry, and the ffmpeg
// chip reads its entry out of the same list.
#[tauri::command(async)]
fn binaries_state(app: AppHandle) -> Result<Vec<binaries_manager::DependencyState>, String> {
    let data_root = paths::data_root(&app)?;
    Ok(binaries_manager::states(&data_root))
}

// Installs or updates ONE registry entry on a worker thread; progress arrives
// as `binaries://progress` (id in the payload), completion as
// `binaries://done` / `binaries://cancelled` / `binaries://error`.
#[tauri::command(async)]
fn binaries_install(app: AppHandle, id: String) -> Result<(), String> {
    let data_root = paths::data_root(&app)?;
    let started = binaries_manager::begin_install(&id)?;
    let handle = app.clone();
    std::thread::spawn(move || {
        let progress_id = id.clone();
        let progress_handle = handle.clone();
        let emit = move |phase: &str, detail: String| {
            let _ = progress_handle.emit(
                "binaries://progress",
                json!({ "id": progress_id, "phase": phase, "detail": detail }),
            );
        };
        let is_ffmpeg = id == "ffmpeg";
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            binaries_manager::install_entry_started(&data_root, started, emit)
        }));
        match outcome {
            Ok(Ok(facts)) => {
                let _ = handle.emit("binaries://done", json!({ "id": id, "facts": facts }));
                if is_ffmpeg {
                    // Tool installation changes derived-work eligibility, not
                    // index debt. The coordinator re-reads the tool state on
                    // this wake; no scan restart or captured config is involved.
                    derived_work::wake(false);
                }
            }
            Ok(Err(err)) => {
                if binaries_manager::is_cancelled_error(&err) {
                    logging::info("dependency install cancelled", json!({ "id": id }));
                    let _ = handle.emit("binaries://cancelled", json!({ "id": id }));
                    return;
                }
                logging::warn(
                    "dependency install failed",
                    json!({ "id": id, "error": { "message": err.clone() } }),
                );
                let _ = handle.emit("binaries://error", json!({ "id": id, "message": err }));
            }
            Err(_) => {
                let message = "dependency install stopped unexpectedly";
                logging::error(
                    "dependency install panicked",
                    json!({ "id": id, "error": { "message": message } }),
                );
                let _ = handle.emit(
                    "binaries://error",
                    json!({ "id": id, "message": message }),
                );
            }
        }
    });
    Ok(())
}

#[tauri::command(async)]
fn binaries_cancel(id: String) -> bool {
    binaries_manager::cancel_entry(&id)
}

// Version check for one entry — never installs; a failure writes nothing.
#[tauri::command(async)]
fn binaries_check(
    app: AppHandle,
    id: String,
) -> Result<Vec<binaries_manager::DependencyState>, String> {
    logging::boundary(
        "binaries_check",
        json!({ "id": id }),
        || {
            let data_root = paths::data_root(&app)?;
            binaries_manager::check_entry(&data_root, &id)?;
            Ok(binaries_manager::states(&data_root))
        },
        |states| json!({ "entries": states.len() }),
    )
}

// Wizard support: is this a real IANA timezone name?
#[tauri::command]
fn validate_timezone(name: String) -> bool {
    resolution::parse_timezone_name(&name).is_ok()
}

// The session gate's check: configured source directories that are not
// currently present (an unmounted volume manifests as a missing directory).
#[tauri::command(async)]
fn check_source_dirs(app: AppHandle) -> Result<SourceDirsStatus, String> {
    logging::boundary(
        "check_source_dirs",
        json!({}),
        || verify_source_dirs(&app),
        |status| json!({ "missing": status.missing.len(), "substituted": status.substituted.len() }),
    )
}

// The comparison view's group members for one item, best-first.
#[tauri::command(async)]
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

// The comparison view's unlink: this image is not the same subject as its
// similar-family. Persistent — an excluded pair never regroups on any later
// rebuild — and non-destructive: no file is touched.
#[tauri::command(async)]
fn similar_unlink(app: AppHandle, hash: String) -> Result<u64, String> {
    logging::boundary(
        "similar_unlink",
        json!({ "hash": hash }),
        || {
            let data_root = paths::data_root(&app)?;
            let conn = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            similarity::unlink_from_group(&conn, &data_root, &hash)
        },
        |written| json!({ "exclusions": written }),
    )
}

// The Settings surface for the unlink store: how many verdicts exist, and the
// one way to take them all back. Without a visible count the exclusions would
// be an invisible permanent store — the kind of silence the app avoids.
#[tauri::command(async)]
fn similar_exclusions_count(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "similar_exclusions_count",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            similar_exclusions::count(&data_root)
        },
        |n| json!({ "count": n }),
    )
}

#[tauri::command(async)]
fn similar_exclusions_clear(app: AppHandle) -> Result<u64, String> {
    logging::boundary(
        "similar_exclusions_clear",
        json!({}),
        || {
            let data_root = paths::data_root(&app)?;
            similar_exclusions::clear(&data_root)
        },
        |n| json!({ "cleared": n }),
    )
}

// The metadata pane's detail for one logical item.
#[tauri::command(async)]
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
#[tauri::command(async)]
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
        // Process ownership is the FIRST plugin setup. Its OS file lock is the
        // atomic authority; a secondary routes activation to the owner and exits
        // before logs, stores, the index, watchers, or destructive commands start.
        .plugin(instance_owner::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("mediacache", |_ctx, request| serve_mediacache(&request))
        .register_uri_scheme_protocol("mediafile", |_ctx, request| serve_mediafile(&request))
        .setup(move |app| {
            // Everything in this closure runs BEFORE the window appears, so it
            // is the launch latency. Each phase logs its cost so a slow start
            // is attributable from the session log alone.
            let setup_started = std::time::Instant::now();
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
            let index = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME))?;
            let imported_exclusions = similar_exclusions::migrate_legacy(&data_root, &index)?;
            if imported_exclusions > 0 {
                logging::info(
                    "legacy similar exclusions imported",
                    json!({ "pairs": imported_exclusions }),
                );
            }

            // Download staging is crash debris by definition: wipe at launch.
            binaries_manager::reset_temp_dir(&data_root);

            // Models the registry no longer knows: the SigLIP 2 tower was
            // dropped with the embeddings (Phase 33) after two machines had
            // downloaded it — 1.2 GB each of dead weight the registry can no
            // longer even list for uninstall. One named sweep, not a general
            // unknown-file purge: the models dir is OURS, but a rule that
            // deletes anything unrecognized would eat a future version's
            // files the moment the user runs an older build once.
            for orphan in [
                "siglip2-large-vision.onnx", // embeddings dropped 2026-08-18
                "clip-vit-b32-vision.onnx",  // superseded by SigLIP 2026-08-17
                "ultraface-rfb320.onnx",     // superseded by RFB-640
                "emotion-ferplus-8.onnx",    // superseded by HSEmotion
            ] {
                let path = data_root.join(paths::MODELS_DIR_NAME).join(orphan);
                if path.exists() {
                    match std::fs::remove_file(&path) {
                        Ok(()) => logging::info(
                            "orphaned model removed",
                            json!({ "file": orphan }),
                        ),
                        Err(err) => logging::warn(
                            "orphaned model removal failed",
                            json!({ "file": orphan, "error": { "message": err.to_string() } }),
                        ),
                    }
                }
            }

            // The cache always lives under the managed data root. Existing
            // external cache trees from older builds are deliberately left
            // untouched; they are reconstructible and no longer referenced.
            let setup_config = storage::read_config_for_setup(&data_root)?;
            let cache_root = data_root.join(storage::CACHE_DIR_NAME);
            let _ = DATA_ROOT.set(data_root.clone());
            // One coordinator owns reconstructible media work; its optional
            // heavy classes run only while the user is away.
            derived_work::start(app.handle().clone());
            // The sweep walks the ENTIRE cache tree, which grows with the
            // library — it was the launch's biggest fixed cost, paid before
            // the window could appear. It maintains a reconstructible cache
            // and nothing needs its result before first paint, so it runs on
            // a background thread (its own connection; WAL carries the
            // concurrency).
            {
                let db_path = data_root.join(storage::INDEX_DB_FILE_NAME);
                let cache = preview::CachePaths::new(cache_root);
                std::thread::spawn(move || {
                    let started = std::time::Instant::now();
                    if let Ok(conn) = index_store::open(&db_path) {
                        match preview::startup_sweep(&conn, &cache) {
                            Ok(0) => {}
                            Ok(removed) => {
                                logging::info(
                                    "cache sweep",
                                    json!({ "removed": removed, "ms": started.elapsed().as_millis() as u64 }),
                                );
                            }
                            Err(err) => {
                                logging::warn("cache sweep failed", json!({ "error": { "message": err } }));
                            }
                        }
                    }
                });
            }

            // The watcher: ON by default, best-effort, over the configured
            // source roots (the Camera Roll inflow case). Restart picks up
            // source-dir changes; correctness never depends on it.
            let watch_settings = scanner::settings_from_config(setup_config.as_ref(), &data_root, 0);
            watcher::start(app.handle().clone(), watch_settings.source_dirs);

            // The one update switch (managed-runtime-dependencies): when ON,
            // an INSTALLED tool is checked at launch, throttled to ~daily so
            // launches never hammer the endpoints. Default off; a failed
            // check writes nothing (`check_entry`'s own contract).
            let check_at_launch = setup_config
                .as_ref()
                .and_then(|c| c.get("checkUpdatesAtLaunch"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if check_at_launch {
                // The conventions' one toggle covers every installed entry
                // that HAS an upstream to ask — binaries. A model's version
                // is compiled into the app, so it is never checked (and
                // never stamped): `state_of` derives its latest from the pin.
                let root = data_root.clone();
                let handle = app.handle().clone();
                let stale_ids: Vec<String> = binaries_manager::states(&data_root)
                    .into_iter()
                    .filter(|entry| {
                        entry.checkable
                            && entry.status != binaries::BinaryStatus::NotInstalled
                            && entry
                                .facts
                                .last_checked_at_utc
                                .as_deref()
                                .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                                .map(|t| {
                                    chrono::Utc::now().signed_duration_since(t)
                                        > chrono::Duration::hours(24)
                                })
                                .unwrap_or(true)
                    })
                    .map(|entry| entry.id)
                    .collect();
                if !stale_ids.is_empty() {
                    std::thread::spawn(move || {
                        for id in stale_ids {
                            match binaries_manager::check_entry(&root, &id) {
                                Ok(facts) => logging::info(
                                    "launch update check",
                                    json!({ "id": id, "latestKnown": facts.latest_known_version }),
                                ),
                                Err(err) => logging::warn(
                                    "launch update check failed",
                                    json!({ "id": id, "error": { "message": err } }),
                                ),
                            }
                        }
                        let _ = handle.emit("binaries://changed", json!({}));
                    });
                }
            }

            // Auto-resume: an interrupted scan leaves checkpointed pending
            // rows (unhashed media, underived images/videos); pick the work
            // back up without waiting for the user to press Scan. Includes the
            // WALK when a root was never walked to completion — the tail alone
            // cannot recover directories that have no rows at all, and would
            // otherwise report clean forever over a half-indexed library.
            // ALWAYS walk at startup when roots are configured. The watcher
            // only runs while the app runs, so anything added while it was
            // closed has no row at all — and every row-level probe is blind to
            // a file it has never seen. Without this the app opens on a
            // silently incomplete library and nothing on screen says so, which
            // is precisely the daily case in the Goal: an inflow directory
            // fills up while the app is not running.
            //
            // The cost is bounded by what already exists: the walk is
            // stat-only (unchanged size+mtime skips all content work), runs on
            // a worker thread, reports progress in the footer, and is
            // cancellable — quitting stops it and the next launch resumes.
            let configured = storage::load_config_source_dirs(&data_root).unwrap_or_default();
            if !configured.is_empty() {
                logging::info("scan started at launch", json!({ "roots": configured.len() }));
                let _ = spawn_scan(app.handle().clone(), true);
            } else {
                // No roots yet (first run): only finish work already begun.
                let (resume, needs_walk) = resume_plan(&data_root);
                if resume {
                    logging::info("scan resumed at startup", json!({ "walk": needs_walk }));
                    let _ = spawn_scan(app.handle().clone(), needs_walk);
                }
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
                    // The pre-window cost. Anything slow after this line is a
                    // background thread, not launch latency.
                    "setupMs": setup_started.elapsed().as_millis() as u64,
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
            similar_unlink,
            similar_exclusions_count,
            similar_exclusions_clear,
            delete_item,
            move_item_out,
            list_subdirs,
            create_subdir,
            delete_empty_dir,
            dir_is_empty,
            reveal_data_subdir,
            open_item_externally,
            note_user_activity,
            background_work_snapshot,
            background_work_set_paused,
            prioritize_derived_work,
            set_window_simple_fullscreen,
            ensure_preview,
            re_resolve_all,
            rescan_section,
            get_issues,
            ensure_fullres,
            transcribe,
            transcript_get,
            transcribe_cancel,
            trash_overview,
            trash_empty,
            dismiss_issue,
            dismiss_all_issues,
            retry_issue,
            retry_all_issues,
            binaries_state,
            binaries_install,
            binaries_cancel,
            binaries_check,
            validate_timezone,
            check_source_dirs,
            log_event,
            logging_debug_enabled
        ])
        .build(tauri::generate_context!());

    // A setup failure lands here — most consequentially a store that is unreadable for a
    // reason other than absence, or one that could not be set aside. OneCopy must not
    // reset over bytes it failed to preserve, so it halts; a panic into stderr is not a
    // halt for a double-clicked app, so the halt is reported natively before exiting
    // (storage-path conventions: a halt names the store and reaches the user).
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            let message = error.to_string();
            logging::error("startup failed", json!({ "error": { "message": message } }));
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("OneCopy could not start")
                .set_description(format!(
                    "A settings file could not be read, and OneCopy could not set it aside either — so it \
                     has been left exactly where it is rather than risk overwriting it.\n\n{message}\n\n\
                     Your photos are not affected. Repair or move the file under the OneCopy data folder, \
                     then start OneCopy again."
                ))
                .show();
            std::process::exit(1);
        }
    };

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
