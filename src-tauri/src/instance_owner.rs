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
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
    stop: tokio::sync::watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Owner {
    fn request_shutdown(&self) {
        self.stop.send_replace(true);
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
                crate::native_path::label(&path)
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
            crate::native_path::label(&path)
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
    stop: tokio::sync::watch::Receiver<bool>,
) -> Result<JoinHandle<()>, String> {
    // One readiness-driven loop owns the socket through shutdown. It has no
    // blocking tasks, so dropping its future closes every accepted connection.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io().enable_time().build()
        .map_err(|error| format!("could not prepare instance listener: {error}"))?;
    let handle = app.clone();
    std::thread::Builder::new()
        .name("onecopy-instance-listener".to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(run_listener(listener, stop, || activate(&handle)))
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

async fn run_listener(
    listener: TcpListener,
    mut stop: tokio::sync::watch::Receiver<bool>,
    mut activate: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::from_std(listener)
        .map_err(|error| format!("could not register instance listener: {error}"))?;
    let service = async {
        loop {
            let (mut stream, _) = listener.accept().await
                .map_err(|error| format!("instance listener stopped: {error}"))?;
            if read_activation(&mut stream).await {
                activate()?;
            }
        }
    };
    tokio::select! {
        biased;
        // A retained watch value also handles shutdown before the loop starts.
        _ = stop.wait_for(|requested| *requested) => Ok(()),
        result = service => result,
    }
}

async fn read_activation(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> bool {
    use tokio::io::AsyncReadExt;
    // TCP may split even this small frame. One total connection deadline
    // covers all fragments; an idle peer cannot retain the listener.
    let mut request = [0u8; 8];
    matches!(tokio::time::timeout(Duration::from_millis(500), stream.read_exact(&mut request)).await, Ok(Ok(_)))
        && &request == b"activate"
}

fn activate(app: &tauri::AppHandle) -> Result<(), String> {
    if crate::app_lifecycle::shutting_down() { return Ok(()); }
    if let Some(window) = app.get_webview_window("main") {
        match window.show().and_then(|()| window.set_focus()) {
            Ok(()) => crate::failure_runtime::clear("instance-activation-failed", Some("main")),
            Err(error) => crate::failure_runtime::report(
                app, "instance-activation-failed", Some("main"), &error.to_string(),
            )?,
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
            let root = crate::paths::resolve_data_root(app.app_handle())?;
            match claim(&root)? {
                Claim::Primary { lock, listener } => {
                    let (stop, stopped) = tokio::sync::watch::channel(false);
                    let worker = listen(listener, app.app_handle().clone(), stopped)?;
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
// EXCEPTION to tests-folder conventions: process-lock acquisition is a
// private startup primitive; widening it only for an integration test would
// make the ownership boundary less clear.
#[path = "../tests/unit/instance_owner.rs"]
mod tests;
