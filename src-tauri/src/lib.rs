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

// Returns the absolute storage root (`~/.onecopy`, or `ONECOPY_HOME`), creating
// it if missing. The Rust core is the only path resolver: the webview calls
// this once at startup and derives every subpath from the returned absolute
// root, never reconstructing the root itself.
#[tauri::command]
fn app_data_root(app: AppHandle) -> Result<String, String> {
    logging::boundary(
        "app_data_root",
        json!({}),
        || paths::data_root(&app).map(|root| root.to_string_lossy().to_string()),
        |root| json!({ "root": root }),
    )
}

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

#[tauri::command]
fn save_config(app: AppHandle, config: Value) -> Result<(), String> {
    logging::boundary(
        "save_config",
        json!({}),
        || storage::save_config(&app, &config),
        |_| json!({}),
    )
}

#[tauri::command]
fn save_state(app: AppHandle, state: Value) -> Result<(), String> {
    logging::boundary(
        "save_state",
        json!({}),
        || storage::save_state(&app, &state),
        |_| json!({}),
    )
}

// One scan pipeline at a time; a second start is a no-op reported as `false`.
static SCAN_RUNNING: AtomicBool = AtomicBool::new(false);

// The live scan worker, joined at exit so a quit interrupts the scan through
// the cooperative cancel flag instead of killing it mid-write.
static SCAN_THREAD: std::sync::Mutex<Option<std::thread::JoinHandle<()>>> =
    std::sync::Mutex::new(None);

// The cache root, resolved once at setup (config `cacheDir` or `<root>/cache`)
// for the mediacache protocol handler. A cacheDir change takes effect on the
// next launch — the protocol reads this, never the config, per request.
static CACHE_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

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
    let Some(data_root) = DATA_ROOT.get() else {
        return not_found();
    };
    let hash = request.uri().path().trim_start_matches('/');
    if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return not_found();
    }

    let Ok(conn) = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME)) else {
        return not_found();
    };
    let path: Option<String> = conn
        .query_row(
            "SELECT abs_path FROM paths WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
            [hash],
            |r| r.get(0),
        )
        .ok();
    let Some(path) = path else { return not_found() };

    let Ok(mut file) = std::fs::File::open(&path) else {
        return not_found();
    };
    let Ok(total) = file.metadata().map(|m| m.len()) else {
        return not_found();
    };

    let content_type = content_type_for(&path);
    let range = request
        .headers()
        .get("Range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_byte_range(v, total));

    // Large file, no Range: serve a head chunk as 206 so the player switches
    // to ranged loading rather than the handler materializing gigabytes.
    const HEAD_CHUNK: u64 = 1024 * 1024;
    const WHOLE_FILE_LIMIT: u64 = 32 * 1024 * 1024;
    let (start, end, status) = match range {
        Some((start, end)) => (start, end, 206),
        None if total > WHOLE_FILE_LIMIT => (0, HEAD_CHUNK.min(total) - 1, 206),
        None => (0, total.saturating_sub(1), 200),
    };

    let length = end - start + 1;
    let mut bytes = vec![0u8; length as usize];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut bytes).is_err() {
        return not_found();
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

// `bytes=start-end` / `bytes=start-` / `bytes=-suffix`, single range only.
fn parse_byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let spec = header.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (start_text, end_text) = spec.split_once('-')?;
    if start_text.is_empty() {
        // Suffix form: the last N bytes.
        let suffix: u64 = end_text.parse().ok()?;
        if suffix == 0 || total == 0 {
            return None;
        }
        return Some((total.saturating_sub(suffix), total - 1));
    }
    let start: u64 = start_text.parse().ok()?;
    if start >= total {
        return None;
    }
    let end = if end_text.is_empty() {
        total - 1
    } else {
        end_text.parse::<u64>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

fn content_type_for(path: &str) -> &'static str {
    match extensions::lowercase_ext(path).as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" | "heif" | "hif" => "image/heif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mpg" | "mpeg" => "video/mpeg",
        "wmv" => "video/x-ms-wmv",
        "3gp" => "video/3gpp",
        "mts" | "m2ts" => "video/mp2t",
        _ => "application/octet-stream",
    }
}

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
    let Some(root) = CACHE_ROOT.get() else {
        return not_found();
    };
    let cache = preview::CachePaths::new(root.clone());
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

/// Magic-byte sniff over the formats the cache can hold; WebP (and anything
/// unrecognized) reports as WebP, the tree's native encode format.
fn sniff_image_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "image/webp"
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

// The volume-loss guard (the session gate's runtime counterpart): destructive
// operations refuse to run while any configured source directory is absent —
// a vanished volume must block deletes, not let them half-apply.
fn ensure_sources_present(app: &AppHandle) -> Result<(), String> {
    let loaded = storage::load_app_data(app)?;
    let data_root = paths::data_root(app)?;
    let settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
    let missing: Vec<&String> = settings
        .source_dirs
        .iter()
        .filter(|dir| !std::path::Path::new(dir.as_str()).is_dir())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "destructive operations are blocked: {} configured source directorie(s) are missing ({})",
            missing.len(),
            missing
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
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
            let cache_root = CACHE_ROOT
                .get()
                .cloned()
                .unwrap_or_else(|| data_root.join(storage::CACHE_DIR_NAME));
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
            let cache_root = CACHE_ROOT
                .get()
                .cloned()
                .unwrap_or_else(|| data_root.join(storage::CACHE_DIR_NAME));
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
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.contains(['/', '\\']) {
        return Err("folder names must be non-empty and slash-free".to_string());
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
}

// Deletes a tree folder ONLY when empty — remove_dir refuses otherwise, which
// is the entire safety model (empty folders render distinctly in the tree).
#[tauri::command]
fn delete_empty_dir(path: String) -> Result<(), String> {
    std::fs::remove_dir(&path).map_err(|e| e.to_string())
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
fn check_source_dirs(app: AppHandle) -> Result<Vec<String>, String> {
    logging::boundary(
        "check_source_dirs",
        json!({}),
        || {
            let loaded = storage::load_app_data(&app)?;
            let data_root = paths::data_root(&app)?;
            let settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
            Ok(settings
                .source_dirs
                .iter()
                .filter(|dir| !std::path::Path::new(dir).is_dir())
                .cloned()
                .collect())
        },
        |missing: &Vec<String>| json!({ "missing": missing.len() }),
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
            let log_path = data_root.join("logs").join(logging::session_filename());
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
            let _ = CACHE_ROOT.set(cache_root.clone());
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

            // Auto-resume: an interrupted scan leaves checkpointed pending
            // rows (unhashed media, underived images/videos); pick the work
            // back up without waiting for the user to press Scan. Runs only
            // the pipeline tail — no walk — and no-ops on a clean index.
            if let Ok(conn) = index_store::open(&data_root.join(storage::INDEX_DB_FILE_NAME)) {
                match scanner::pending_work_exists(&conn, ffmpeg_present) {
                    Ok(true) => {
                        logging::info("scan resumed at startup", json!({}));
                        let _ = spawn_scan(app.handle().clone(), false);
                    }
                    Ok(false) => {}
                    Err(err) => {
                        logging::warn(
                            "pending-work probe failed",
                            json!({ "error": { "message": err } }),
                        );
                    }
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
                }),
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_data_root,
            load_app_data,
            save_config,
            save_state,
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
