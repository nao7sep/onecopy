use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

pub mod backup_store;
pub mod extensions;
pub mod hashing;
pub mod index_store;
pub mod logging;
pub mod metadata;
mod nanoid;
pub mod operations;
pub mod paths;
pub mod preview;
pub mod queries;
pub mod resolution;
pub mod scanner;
pub mod storage;
pub mod timestamps;
pub mod trash;

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

// The cache root, resolved once at setup (config `cacheDir` or `<root>/cache`)
// for the mediacache protocol handler. A cacheDir change takes effect on the
// next launch — the protocol reads this, never the config, per request.
static CACHE_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

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
    } else {
        return not_found();
    };
    match std::fs::read(&file) {
        Ok(bytes) => tauri::http::Response::builder()
            .status(200)
            .header("Content-Type", "image/webp")
            .header("Cache-Control", "public, max-age=31536000, immutable")
            .body(bytes)
            .unwrap_or_else(|_| not_found()),
        Err(_) => not_found(),
    }
}

// Launches the full scan pipeline (walk → hash → extract → resolve → pair →
// derive) on a worker thread. Progress arrives as `scan://progress` events,
// completion as `scan://done` (with the summary) or `scan://error`. Returns
// false when a scan is already running.
#[tauri::command]
fn start_scan(app: AppHandle) -> Result<bool, String> {
    if SCAN_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(false);
    }
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

        std::thread::spawn(move || {
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
                scanner::run_full_scan(&conn, &settings, &emit_progress)
            });
            match outcome {
                Ok(summary) => {
                    logging::info("scan complete", json!({ "summary": summary }));
                    let _ = handle.emit("scan://done", json!({ "summary": summary }));
                }
                Err(err) => {
                    logging::error("scan failed", json!({ "error": { "message": err.clone() } }));
                    let _ = handle.emit("scan://error", json!({ "message": err }));
                }
            }
            SCAN_RUNNING.store(false, Ordering::SeqCst);
        });
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

#[tauri::command]
fn quick_count(app: AppHandle, root: String) -> Result<QuickCount, String> {
    logging::boundary(
        "quick_count",
        json!({ "root": root }),
        || {
            let loaded = storage::load_app_data(&app)?;
            let data_root = paths::data_root(&app)?;
            let settings = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0);
            let mut count = QuickCount::default();
            for entry in walkdir::WalkDir::new(&root).follow_links(false) {
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

            // Resolve the cache root once for the mediacache protocol, then
            // sweep crash leftovers (hash-orphaned entries, stranded temps).
            let loaded = storage::load_app_data(app.handle())?;
            let cache_root = scanner::settings_from_config(loaded.config.as_ref(), &data_root, 0)
                .cache_root;
            let _ = CACHE_ROOT.set(cache_root.clone());
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
            delete_item,
            quick_count,
            validate_timezone,
            check_source_dirs,
            log_event,
            logging_debug_enabled
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            logging::info("app shutdown", json!({ "reason": "exit" }));
        }
    });
}
