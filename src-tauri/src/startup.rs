//! Application bootstrap and the gate between required data and runtime work.
//! Tauri's setup callback always succeeds; a data failure leaves the authored
//! shell running in a blocked state and admits no app-lifetime worker.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};

const BLOCKED_TITLE: &str = "OneCopy could not start safely";
const BLOCKED_MESSAGE: &str = "OneCopy could not safely open its application data. Your photos were not changed. Review the newest session log in the OneCopy data folder, then quit and try again.";
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static WORKERS: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartupFailure {
    pub(crate) title: &'static str,
    pub(crate) message: &'static str,
}

pub(crate) struct StartupGate {
    failure: Option<StartupFailure>,
}

impl StartupGate {
    fn ready() -> Self {
        Self { failure: None }
    }

    fn blocked() -> Self {
        Self {
            failure: Some(StartupFailure {
                title: BLOCKED_TITLE,
                message: BLOCKED_MESSAGE,
            }),
        }
    }

    pub(crate) fn failure(&self) -> Option<StartupFailure> {
        self.failure.clone()
    }
}

pub(crate) struct StartupState {
    pub(crate) data_root: PathBuf,
    pub(crate) cache_root: PathBuf,
    pub(crate) setup_config: Option<Value>,
    pub(crate) log_path: PathBuf,
    pub(crate) started: Instant,
}

struct PreparedData {
    cache_root: PathBuf,
    setup_config: Option<Value>,
}

fn shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

fn spawn_launch_worker(
    app: tauri::AppHandle,
    thread_name: &'static str,
    issue_kind: &'static str,
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) {
    let mut workers = match WORKERS.lock() {
        Ok(workers) => workers,
        Err(_) => {
            let _ = crate::failure_runtime::report(
                &app,
                issue_kind,
                None,
                "launch worker state is unavailable",
            );
            return;
        }
    };
    if shutting_down() {
        return;
    }
    join_finished(&mut workers);
    if let Ok(worker) =
        crate::failure_runtime::spawn_reported(app, thread_name, issue_kind, work)
    {
        workers.push(worker);
    }
}

pub fn shutdown() {
    let _workers = match WORKERS.lock() {
        Ok(workers) => workers,
        Err(_) => {
            SHUTTING_DOWN.store(true, Ordering::SeqCst);
            crate::logging::error("launch worker state is unavailable", json!({}));
            return;
        }
    };
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
}

pub fn join() {
    let workers = match WORKERS.lock() {
        Ok(mut workers) => workers.drain(..).collect::<Vec<_>>(),
        Err(_) => {
            crate::logging::error("launch worker state is unavailable", json!({}));
            return;
        }
    };
    for worker in workers {
        if worker.join().is_err() {
            crate::logging::error("launch worker join failed", json!({}));
        }
    }
}

fn join_finished(workers: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            if worker.join().is_err() {
                crate::logging::error("launch worker join failed", json!({}));
            }
        } else {
            index += 1;
        }
    }
}

fn prepare_data(data_root: &Path) -> Result<PreparedData, String> {
    crate::storage::materialize_config_if_missing(data_root)?;
    drop(crate::index_store::open(
        &data_root.join(crate::storage::INDEX_DB_FILE_NAME),
    )?);

    // Download staging is crash debris by definition: wipe at launch.
    crate::binaries_manager::reset_temp_dir(data_root);

    Ok(PreparedData {
        cache_root: data_root.join(crate::storage::CACHE_DIR_NAME),
        setup_config: crate::storage::read_config_for_setup(data_root)?,
    })
}

fn prepare(app: &tauri::App, debug_enabled: bool) -> Result<StartupState, String> {
    let started = Instant::now();
    let data_root = crate::paths::data_root(app.handle())?;
    let log_path = data_root
        .join(crate::paths::LOGS_DIR_NAME)
        .join(crate::logging::session_filename());
    crate::logging::init(&log_path, debug_enabled);
    crate::install_panic_hook();

    // The backup store is best-effort by contract and records its own failure.
    crate::backup_store::init(data_root.join(crate::backup_store::BACKUPS_DB_FILE_NAME));

    let PreparedData {
        cache_root,
        setup_config,
    } = prepare_data(&data_root)?;
    crate::DATA_ROOT
        .set(data_root.clone())
        .map_err(|_| "data root was initialized more than once".to_string())?;

    Ok(StartupState {
        data_root,
        cache_root,
        setup_config,
        log_path,
        started,
    })
}

fn start_runtime(app: &tauri::App, state: StartupState, debug_enabled: bool) {
    let StartupState {
        data_root,
        cache_root,
        setup_config,
        log_path,
        started,
    } = state;

    // Runtime services are admitted only after every required data check
    // succeeded. Each long-lived worker owns its later failure boundary.
    if let Err(error) = crate::derived_work::start(app.handle().clone()) {
        let _ = crate::failure_runtime::report(
            app.handle(),
            crate::issue_recovery::DERIVED_WORKER_FAILED,
            None,
            &error,
        );
    }

    {
        let db_path = data_root.join(crate::storage::INDEX_DB_FILE_NAME);
        let cache = crate::preview::CachePaths::new(cache_root);
        let handle = app.handle().clone();
        let clear_handle = handle.clone();
        spawn_launch_worker(
            handle,
            "onecopy-cache-sweep",
            "cache-sweep-failed",
            move || {
                let started = Instant::now();
                let conn = crate::index_store::open(&db_path)?;
                let removed =
                    crate::preview::startup_sweep(&conn, &cache, &shutting_down)?;
                if shutting_down() {
                    return Ok(());
                }
                if removed > 0 {
                    crate::logging::info(
                        "cache sweep",
                        json!({ "removed": removed, "ms": started.elapsed().as_millis() as u64 }),
                    );
                }
                crate::failure_runtime::clear(&clear_handle, "cache-sweep-failed", None)?;
                Ok(())
            },
        );
    }

    let watch_settings = crate::scanner::settings_from_config(setup_config.as_ref(), &data_root, 0);
    crate::watcher::start(app.handle().clone(), watch_settings.source_dirs);

    let check_at_launch = setup_config
        .as_ref()
        .and_then(|config| config.get("checkUpdatesAtLaunch"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if check_at_launch {
        let root = data_root.clone();
        let handle = app.handle().clone();
        let stale_ids: Vec<String> = crate::binaries_manager::states(&data_root)
            .into_iter()
            .filter(|entry| {
                entry.checkable
                    && entry.status != crate::binaries::BinaryStatus::NotInstalled
                    && entry
                        .facts
                        .last_checked_at_utc
                        .as_deref()
                        .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                        .map(|checked| {
                            chrono::Utc::now().signed_duration_since(checked)
                                > chrono::Duration::hours(24)
                        })
                        .unwrap_or(true)
            })
            .map(|entry| entry.id)
            .collect();
        if !stale_ids.is_empty() {
            let report_handle = handle.clone();
            spawn_launch_worker(
                handle,
                "onecopy-update-check",
                "update-check-worker-failed",
                move || {
                    for id in stale_ids {
                        if shutting_down() {
                            return Ok(());
                        }
                        match crate::binaries_manager::check_entry(&root, &id) {
                            Ok(facts) => {
                                crate::failure_runtime::clear(
                                    &report_handle,
                                    "update-check-failed",
                                    Some(&id),
                                )?;
                                crate::logging::info(
                                    "launch update check",
                                    json!({ "id": id, "latestKnown": facts.latest_known_version }),
                                );
                            }
                            Err(_) if shutting_down() => return Ok(()),
                            Err(error) => {
                                crate::failure_runtime::report(
                                    &report_handle,
                                    "update-check-failed",
                                    Some(&id),
                                    &error,
                                )?
                            }
                        }
                    }
                    if shutting_down() {
                        return Ok(());
                    }
                    crate::failure_runtime::emit_checked(
                        &report_handle,
                        "binaries://changed",
                        json!({}),
                    )?;
                    crate::failure_runtime::clear(
                        &report_handle,
                        "update-check-worker-failed",
                        None,
                    )?;
                    Ok(())
                },
            );
        }
    }

    crate::logging::info(
        "app startup",
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "build": if cfg!(debug_assertions) { "debug" } else { "release" },
            "debugLogging": debug_enabled,
            "logPath": log_path.to_string_lossy(),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "setupMs": started.elapsed().as_millis() as u64,
        }),
    );
}

/// Own the entire fallible launch boundary while keeping Tauri's setup hook
/// infallible. A required-data failure becomes shell state, not process death.
pub(crate) fn initialize(app: &tauri::App, debug_enabled: bool) -> StartupGate {
    let prepared =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prepare(app, debug_enabled)));

    let state = match prepared {
        Ok(Ok(state)) => state,
        Ok(Err(error)) => {
            crate::logging::error("startup blocked", json!({ "error": { "message": error } }));
            return StartupGate::blocked();
        }
        Err(payload) => {
            let error = crate::failure_runtime::panic_message(payload);
            crate::logging::error(
                "startup blocked",
                json!({ "error": { "message": format!("startup panicked: {error}") } }),
            );
            return StartupGate::blocked();
        }
    };

    // Required state is complete at this point. A defect while admitting a
    // best-effort runtime service must not cross Tauri's native callback or
    // retroactively turn valid application data into a fatal bootstrap.
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start_runtime(app, state, debug_enabled);
    })) {
        let error = crate::failure_runtime::panic_message(payload);
        crate::logging::error(
            "runtime startup panicked",
            json!({ "error": { "message": error } }),
        );
    }

    StartupGate::ready()
}

pub(crate) fn halt_before_runtime(diagnostic: &str) -> ! {
    crate::logging::error(
        "startup failed",
        json!({ "error": { "message": diagnostic } }),
    );
    if std::panic::catch_unwind(|| {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("OneCopy could not launch")
            .set_description(
                "OneCopy could not create its application window. Restart the computer, then try again.",
            )
            .set_buttons(rfd::MessageButtons::OkCustom("Quit".to_string()))
            .show();
    })
    .is_err()
    {
        eprintln!("[onecopy:startup] native startup halt dialog failed");
    }
    std::process::exit(1);
}

// `prepare_data` is a genuine private startup seam: testing it directly proves
// required data failures cannot yield the state that admits app-lifetime workers.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_index_path_never_yields_worker_admission_state() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(crate::storage::INDEX_DB_FILE_NAME)).unwrap();

        let error = match prepare_data(temp.path()) {
            Ok(_) => panic!("invalid index path unexpectedly prepared startup data"),
            Err(error) => error,
        };

        assert!(!error.is_empty());
        assert!(!temp.path().join(crate::storage::CACHE_DIR_NAME).exists());
    }

    #[test]
    fn blocked_gate_exposes_only_stable_private_safe_copy() {
        let failure = StartupGate::blocked().failure().unwrap();
        assert_eq!(failure.title, "OneCopy could not start safely");
        assert!(failure.message.contains("Your photos were not changed"));
        assert!(!failure.message.contains('/'));
        assert!(!failure.message.contains("sqlite"));
    }
}
