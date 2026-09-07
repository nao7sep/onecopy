//! Atomic process ownership and second-launch activation.
//!
//! The generic Tauri single-instance plug-in's macOS connect-then-bind sequence
//! permits two simultaneous cold starts to both pass its initial probe; the bind
//! loser logs and continues. OneCopy cannot tolerate that over its shared index
//! and destructive filesystem commands. This small app-owned plug-in instead
//! takes an OS file lock before any stateful setup. While holding that lock, the
//! owner publishes a loopback activation endpoint in a retry-safe sidecar; a
//! loser can therefore focus the established owner and exit before any shared
//! store opens. The kernel releases the authoritative lock on a crash, and the
//! next owner clears any stale endpoint before publishing its own.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::Manager;

const LOCK_FILE_NAME: &str = "instance.lock";
const ENDPOINT_FILE_NAME: &str = "instance.endpoint";
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_millis(20);

struct Owner {
    // not recorded: this is an OS-lock carrier and activation endpoint fact,
    // not managed text. The file may remain after exit; only its live lock is
    // authoritative.
    _lock: File,
    stop: Arc<AtomicBool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Owner {
    fn request_shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    fn join(&self) {
        let worker = match self.worker.lock() {
            Ok(mut worker) => worker.take(),
            Err(_) => {
                crate::logging::error(
                    "instance-listener worker state is unavailable",
                    serde_json::json!({}),
                );
                return;
            }
        };
        if worker.is_some_and(|worker| worker.join().is_err()) {
            crate::logging::error(
                "instance-listener worker join failed",
                serde_json::json!({}),
            );
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.request_shutdown();
        self.join();
    }
}

enum Claim {
    Primary { lock: File, listener: TcpListener },
    Secondary { endpoint_path: PathBuf },
}

fn claim(root: &Path) -> Result<Claim, String> {
    let path = root.join(LOCK_FILE_NAME);
    let endpoint_path = root.join(ENDPOINT_FILE_NAME);
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| {
            format!(
                "could not open process ownership file {}: {e}",
                path.display()
            )
        })?;

    match lock_file.try_lock() {
        Ok(()) => {
            // Truncate stale endpoint bytes immediately after winning the lock,
            // before another launch can observe ownership and consume a port
            // left by the previous process generation.
            let mut endpoint_file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&endpoint_path)
                .map_err(|e| e.to_string())?;
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .map_err(|e| format!("could not open process activation endpoint: {e}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|e| format!("could not configure process activation endpoint: {e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("could not inspect process activation endpoint: {e}"))?
                .port();
            // Windows byte-range locks are mandatory, so a secondary cannot
            // read endpoint bytes from the locked handle as it can on macOS.
            // Publish a retry-safe sidecar while this process owns the lock;
            // the lock remains the only process-ownership authority.
            write!(endpoint_file, "{port}\n").map_err(|e| e.to_string())?;
            endpoint_file.sync_all().map_err(|e| e.to_string())?;
            Ok(Claim::Primary {
                lock: lock_file,
                listener,
            })
        }
        Err(TryLockError::WouldBlock) => Ok(Claim::Secondary { endpoint_path }),
        Err(TryLockError::Error(err)) => Err(format!(
            "could not acquire process ownership file {}: {err}",
            path.display()
        )),
    }
}

fn notify_primary(endpoint_path: &Path) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(endpoint_path) {
            if let Ok(port) = text.trim().parse::<u16>() {
                if let Ok(mut stream) = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)) {
                    stream.write_all(b"activate").map_err(|e| e.to_string())?;
                    return Ok(());
                }
            }
        }
        if started.elapsed() >= NOTIFY_TIMEOUT {
            return Err("the primary instance did not expose its activation endpoint".to_string());
        }
        std::thread::sleep(RETRY_DELAY);
    }
}

fn listen(
    listener: TcpListener,
    app: tauri::AppHandle,
    stop: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, String> {
    let handle = app.clone();
    std::thread::Builder::new()
        .name("onecopy-instance-listener".to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_listener(listener, &handle, &stop)
            }));
            let failure = match outcome {
                Ok(Ok(())) => return,
                Ok(Err(error)) => error,
                Err(payload) => crate::failure_runtime::panic_message(payload),
            };
            if crate::app_lifecycle::shutting_down() {
                crate::logging::error(
                    "instance listener failed during shutdown",
                    serde_json::json!({ "error": { "message": failure } }),
                );
                return;
            }
            let failure = format!(
                "{failure} Restart OneCopy to restore second-launch activation."
            );
            let _ = crate::failure_runtime::report(
                &handle,
                "instance-listener-failed",
                None,
                &failure,
            );
        })
        .map_err(|error| format!("could not start instance listener: {error}"))
}

fn run_listener(
    listener: TcpListener,
    app: &tauri::AppHandle,
    stop: &AtomicBool,
) -> Result<(), String> {
    while !stop.load(Ordering::SeqCst) && !crate::app_lifecycle::shutting_down() {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if stop.load(Ordering::SeqCst) || crate::app_lifecycle::shutting_down() {
                    return Ok(());
                }
                let mut request = [0u8; 8];
                if stream.read(&mut request).is_ok() && request.starts_with(b"activate") {
                    if crate::app_lifecycle::shutting_down() {
                        return Ok(());
                    }
                    if let Some(window) = app.get_webview_window("main") {
                        let activation = window
                            .show()
                            .and_then(|()| window.set_focus())
                            .map_err(|error| error.to_string());
                        match activation {
                            Ok(()) => {
                                if let Err(error) = crate::failure_runtime::clear(
                                    app,
                                    "instance-activation-failed",
                                    Some("main"),
                                ) {
                                    let _ = crate::failure_runtime::report(
                                        app,
                                        "issue-recovery-failed",
                                        Some("main"),
                                        &error,
                                    );
                                }
                            }
                            Err(error) => crate::failure_runtime::report(
                                app,
                                "instance-activation-failed",
                                Some("main"),
                                &error,
                            )?,
                        }
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(RETRY_DELAY);
            }
            Err(error) => return Err(format!("instance listener stopped: {error}")),
        }
    }
    Ok(())
}

pub fn shutdown(app: &tauri::AppHandle) {
    if let Some(owner) = app.try_state::<Owner>() {
        owner.request_shutdown();
    }
}

pub fn join(app: &tauri::AppHandle) {
    if let Some(owner) = app.try_state::<Owner>() {
        owner.join();
    }
}

pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::<tauri::Wry>::new("instance-owner")
        .setup(|app, _api| {
            let root = crate::paths::data_root(app.app_handle())?;
            match claim(&root)? {
                Claim::Primary { lock, listener } => {
                    let stop = Arc::new(AtomicBool::new(false));
                    let worker =
                        listen(listener, app.app_handle().clone(), stop.clone())?;
                    app.manage(Owner {
                        _lock: lock,
                        stop,
                        worker: Mutex::new(Some(worker)),
                    });
                    Ok(())
                }
                Claim::Secondary { endpoint_path } => {
                    notify_primary(&endpoint_path)?;
                    std::process::exit(0);
                }
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    // EXCEPTION to tests-folder conventions: process-lock acquisition is a
    // private startup primitive; widening it only for an integration test would
    // make the ownership boundary less clear.
    use super::*;

    #[test]
    fn owner_shutdown_joins_its_listener_thread() {
        let dir = tempfile::tempdir().unwrap();
        let lock = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(dir.path().join("owner"))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_observed = observed.clone();
        let worker = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            thread_observed.store(true, Ordering::SeqCst);
        });
        let owner = Owner {
            _lock: lock,
            stop,
            worker: Mutex::new(Some(worker)),
        };

        owner.request_shutdown();
        owner.join();

        assert!(observed.load(Ordering::SeqCst));
        assert!(owner.worker.lock().unwrap().is_none());
    }

    #[test]
    fn simultaneous_claims_have_exactly_one_owner() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::sync::Arc::new(dir.path().to_path_buf());
        let start = std::sync::Arc::new(std::sync::Barrier::new(3));
        let hold = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let start = start.clone();
            let hold = hold.clone();
            workers.push(std::thread::spawn(move || {
                start.wait();
                let claim = claim(&root).unwrap();
                let primary = matches!(claim, Claim::Primary { .. });
                hold.wait();
                primary
            }));
        }
        start.wait();
        hold.wait();

        assert_eq!(
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|primary| *primary)
                .count(),
            1
        );
    }

    #[test]
    fn exactly_one_claim_owns_a_root_and_drop_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = claim(dir.path()).unwrap();
        assert!(matches!(first, Claim::Primary { .. }));

        let second = claim(dir.path()).unwrap();
        assert!(matches!(second, Claim::Secondary { .. }));

        drop(second);
        drop(first);
        assert!(matches!(claim(dir.path()).unwrap(), Claim::Primary { .. }));
    }

    #[test]
    fn secondary_activation_uses_the_endpoint_published_under_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let Claim::Primary {
            lock: _lock,
            listener,
        } = claim(dir.path()).unwrap()
        else {
            panic!("first claim must own the root");
        };
        listener.set_nonblocking(false).unwrap();
        let Claim::Secondary { endpoint_path } = claim(dir.path()).unwrap() else {
            panic!("second claim must be secondary");
        };

        let receiver = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            request
        });
        notify_primary(&endpoint_path).unwrap();
        assert_eq!(receiver.join().unwrap(), "activate");
    }
}
