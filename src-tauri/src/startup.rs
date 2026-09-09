//! Application bootstrap and the gate between required data and runtime work.
//! Tauri's setup callback always succeeds; a data failure leaves the authored
//! shell running in a blocked state and admits no app-lifetime worker.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};

const BLOCKED_TITLE: &str = "OneCopy could not start safely";
const BLOCKED_MESSAGE: &str = "OneCopy could not safely open its application data. Your photos were not changed. Review the newest session log in the OneCopy data folder, then quit and try again.";
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

fn spawn_launch_worker(
    app: tauri::AppHandle,
    thread_name: &'static str,
    issue_kind: &'static str,
    work: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let mut workers = WORKERS
        .lock()
        .map_err(|_| "launch worker state is unavailable".to_string())?;
    if crate::app_lifecycle::shutting_down() {
        return Ok(());
    }
    join_finished(&mut workers);
    let worker = crate::failure_runtime::spawn_reported(app, thread_name, issue_kind, work)?;
    workers.push(worker);
    Ok(())
}

pub fn shutdown() {
    if WORKERS.lock().is_err() {
        crate::logging::error("launch worker state is unavailable", json!({}));
    }
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
    let conn = crate::index_store::open(
        &data_root.join(crate::storage::INDEX_DB_FILE_NAME),
    )?;
    // Once per process, before any executor or window-driven request exists.
    // Opening another database connection or section must never reset failure.
    crate::information_attempts::reset_library(&conn)?;
    crate::derived_state::reset_failed_outputs(
        &conn,
        crate::derived_state::FailedOutputScope::Library,
    )?;
    drop(conn);

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
    crate::activity::init(data_root.join(crate::activity::ACTIVITY_DB_FILE_NAME));
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

struct RuntimeService<'a> {
    name: &'static str,
    issue_kind: &'static str,
    start: Box<dyn FnOnce() -> Result<(), String> + 'a>,
}

fn run_runtime_services(
    services: Vec<RuntimeService<'_>>,
    mut report_failure: impl FnMut(&'static str, &'static str, String),
) {
    for service in services {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(service.start));
        let failure = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(payload) => Some(crate::failure_runtime::panic_message(payload)),
        };
        if let Some(error) = failure {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                report_failure(service.name, service.issue_kind, error)
            }))
            .is_err()
            {
                eprintln!("[onecopy:startup] runtime service failure reporter panicked");
            }
        }
    }
}

fn start_runtime(app: &tauri::App, state: StartupState, debug_enabled: bool) {
    let StartupState {
        data_root,
        cache_root,
        setup_config,
        log_path,
        started,
    } = state;

    let derived_handle = app.handle().clone();
    let sleep_handle = app.handle().clone();
    let keep_awake = crate::sleep_prevention::configured(setup_config.as_ref());
    let cache_service = {
        let db_path = data_root.join(crate::storage::INDEX_DB_FILE_NAME);
        let cache = crate::preview::CachePaths::new(cache_root);
        let handle = app.handle().clone();
        let clear_handle = handle.clone();
        RuntimeService {
            name: "cache sweep",
            issue_kind: "cache-sweep-failed",
            start: Box::new(move || {
                spawn_launch_worker(
                    handle,
                    "onecopy-cache-sweep",
                    "cache-sweep-failed",
                    move || {
                        let started = Instant::now();
                        let conn = crate::index_store::open(&db_path)?;
                        let removed = crate::preview::startup_sweep(
                            &conn,
                            &cache,
                            &crate::app_lifecycle::shutting_down,
                        )?;
                        if crate::app_lifecycle::shutting_down() {
                            return Ok(());
                        }
                        if removed > 0 {
                            crate::logging::info(
                                "cache sweep",
                                json!({
                                    "removed": removed,
                                    "ms": started.elapsed().as_millis() as u64,
                                }),
                            );
                        }
                        crate::failure_runtime::clear(&clear_handle, "cache-sweep-failed", None)?;
                        Ok(())
                    },
                )
            }),
        }
    };

    let watcher_service = {
        let handle = app.handle().clone();
        let root = data_root.clone();
        let config = setup_config.clone();
        RuntimeService {
            name: "source watcher",
            issue_kind: "watcher-failed",
            start: Box::new(move || {
                let settings = crate::scanner::settings_from_config(config.as_ref(), &root, 0);
                crate::watcher::start(handle, settings.source_dirs).map(|_| ())
            }),
        }
    };

    let update_service = {
        let root = data_root.clone();
        let handle = app.handle().clone();
        let config = setup_config;
        RuntimeService {
            name: "managed-tool update check",
            issue_kind: "update-check-worker-failed",
            start: Box::new(move || {
                let check_at_launch = config
                    .as_ref()
                    .and_then(|value| value.get("checkUpdatesAtLaunch"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !check_at_launch {
                    return Ok(());
                }
                let stale_ids: Vec<String> = crate::binaries_manager::states(&root)
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
                if stale_ids.is_empty() {
                    return Ok(());
                }
                let report_handle = handle.clone();
                spawn_launch_worker(
                    handle,
                    "onecopy-update-check",
                    "update-check-worker-failed",
                    move || {
                        for id in stale_ids {
                            if crate::app_lifecycle::shutting_down() {
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
                                        json!({
                                            "id": id,
                                            "latestKnown": facts.latest_known_version,
                                        }),
                                    );
                                }
                                Err(_) if crate::app_lifecycle::shutting_down() => return Ok(()),
                                Err(error) => crate::failure_runtime::report(
                                    &report_handle,
                                    "update-check-failed",
                                    Some(&id),
                                    &error,
                                )?,
                            }
                        }
                        if crate::app_lifecycle::shutting_down() {
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
                )
            }),
        }
    };

    // Required data is complete before these independent best-effort services
    // are considered. Each admission is contained separately so one panic or
    // start failure cannot skip the services that follow it.
    run_runtime_services(
        vec![
            RuntimeService {
                name: "sleep prevention",
                issue_kind: "sleep-prevention-failed",
                start: Box::new(move || crate::sleep_prevention::start(sleep_handle, keep_awake)),
            },
            RuntimeService {
                name: "derived work",
                issue_kind: crate::issue_recovery::DERIVED_WORKER_FAILED,
                start: Box::new(move || crate::derived_work::start(derived_handle).map(|_| ())),
            },
            cache_service,
            watcher_service,
            update_service,
        ],
        |name, issue_kind, error| {
            let diagnostic = format!("{name}: {error}");
            let _ = crate::failure_runtime::report(app.handle(), issue_kind, None, &diagnostic);
        },
    );

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
    crate::activity::record_app_admitted();
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

    // The service-level boundaries above keep independent admissions moving.
    // This outer framework boundary has a different job: no unforeseen panic
    // may unwind through Tauri's setup callback and abort inside native code.
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        start_runtime(app, state, debug_enabled)
    })) {
        let error = crate::failure_runtime::panic_message(payload);
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::logging::error(
                "runtime startup failed outside a service boundary",
                json!({ "error": { "message": error } }),
            )
        }))
        .is_err()
        {
            eprintln!("[onecopy:startup] runtime startup and its logger both panicked");
        }
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
    use std::cell::{Cell, RefCell};

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

    #[test]
    fn runtime_service_failures_do_not_skip_later_admission() {
        let later_started = Cell::new(false);
        let failures = RefCell::new(Vec::new());
        let services = vec![
            RuntimeService {
                name: "returned failure",
                issue_kind: "returned-failure",
                start: Box::new(|| Err("could not start".to_string())),
            },
            RuntimeService {
                name: "panic failure",
                issue_kind: "panic-failure",
                start: Box::new(|| panic!("service panic")),
            },
            RuntimeService {
                name: "later service",
                issue_kind: "later-service-failed",
                start: Box::new(|| {
                    later_started.set(true);
                    Ok(())
                }),
            },
        ];

        run_runtime_services(services, |name, kind, error| {
            failures.borrow_mut().push((name, kind, error));
        });

        assert!(later_started.get());
        let failures = failures.into_inner();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].0, "returned failure");
        assert_eq!(failures[0].1, "returned-failure");
        assert_eq!(failures[0].2, "could not start");
        assert_eq!(failures[1].0, "panic failure");
        assert_eq!(failures[1].1, "panic-failure");
        assert_eq!(failures[1].2, "service panic");
    }

    #[test]
    fn a_failure_reporter_panic_does_not_skip_later_admission() {
        let later_started = Cell::new(false);
        run_runtime_services(
            vec![
                RuntimeService {
                    name: "failed service",
                    issue_kind: "failed-service",
                    start: Box::new(|| Err("could not start".to_string())),
                },
                RuntimeService {
                    name: "later service",
                    issue_kind: "later-service",
                    start: Box::new(|| {
                        later_started.set(true);
                        Ok(())
                    }),
                },
            ],
            |_, _, _| panic!("reporter panic"),
        );

        assert!(later_started.get());
    }
}
