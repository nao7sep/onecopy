use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

pub mod backup_store;
pub mod extensions;
pub mod hashing;
pub mod index_store;
pub mod logging;
pub mod metadata;
mod nanoid;
pub mod paths;
pub mod preview;
pub mod resolution;
pub mod scanner;
pub mod storage;
pub mod timestamps;

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
