//! Atomic process ownership and second-launch activation.
//!
//! The generic Tauri single-instance plug-in's macOS connect-then-bind sequence
//! permits two simultaneous cold starts to both pass its initial probe; the bind
//! loser logs and continues. OneCopy cannot tolerate that over its shared index
//! and destructive filesystem commands. This small app-owned plug-in instead
//! takes an OS file lock before any stateful setup. The owner publishes a
//! loopback activation endpoint in the locked file; a loser can therefore focus
//! the established owner and exit before any shared store opens. The kernel
//! releases both lock and endpoint on a crash.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use tauri::{Manager, Runtime};

const LOCK_FILE_NAME: &str = "instance.lock";
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_DELAY: Duration = Duration::from_millis(20);

struct Owner {
    // not recorded: this is an OS-lock carrier and activation endpoint fact,
    // not managed text. The file may remain after exit; only its live lock is
    // authoritative.
    _lock: File,
}

enum Claim {
    Primary { lock: File, listener: TcpListener },
    Secondary { lock_file: File },
}

fn claim(root: &Path) -> Result<Claim, String> {
    let path = root.join(LOCK_FILE_NAME);
    let mut lock_file = OpenOptions::new()
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
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .map_err(|e| format!("could not open process activation endpoint: {e}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|e| format!("could not configure process activation endpoint: {e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("could not inspect process activation endpoint: {e}"))?
                .port();
            lock_file.set_len(0).map_err(|e| e.to_string())?;
            lock_file
                .seek(SeekFrom::Start(0))
                .map_err(|e| e.to_string())?;
            write!(lock_file, "{port}\n").map_err(|e| e.to_string())?;
            lock_file.sync_all().map_err(|e| e.to_string())?;
            Ok(Claim::Primary {
                lock: lock_file,
                listener,
            })
        }
        Err(TryLockError::WouldBlock) => Ok(Claim::Secondary { lock_file }),
        Err(TryLockError::Error(err)) => Err(format!(
            "could not acquire process ownership file {}: {err}",
            path.display()
        )),
    }
}

fn notify_primary(mut lock_file: File) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let mut text = String::new();
        lock_file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        lock_file
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        if let Ok(port) = text.trim().parse::<u16>() {
            if let Ok(mut stream) = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)) {
                stream.write_all(b"activate").map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
        if started.elapsed() >= NOTIFY_TIMEOUT {
            return Err("the primary instance did not expose its activation endpoint".to_string());
        }
        std::thread::sleep(RETRY_DELAY);
    }
}

fn listen<R: Runtime>(listener: TcpListener, app: tauri::AppHandle<R>) {
    std::thread::spawn(move || loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut request = [0u8; 8];
                if stream.read(&mut request).is_ok() && request.starts_with(b"activate") {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(RETRY_DELAY);
            }
            Err(_) => break,
        }
    });
}

pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::<tauri::Wry>::new("instance-owner")
        .setup(|app, _api| {
            let root = crate::paths::data_root(app.app_handle())?;
            match claim(&root)? {
                Claim::Primary { lock, listener } => {
                    listen(listener, app.app_handle().clone());
                    app.manage(Owner { _lock: lock });
                    Ok(())
                }
                Claim::Secondary { lock_file } => {
                    let _ = notify_primary(lock_file);
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
        let Claim::Secondary { lock_file } = claim(dir.path()).unwrap() else {
            panic!("second claim must be secondary");
        };

        let receiver = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            stream.read_to_string(&mut request).unwrap();
            request
        });
        notify_primary(lock_file).unwrap();
        assert_eq!(receiver.join().unwrap(), "activate");
    }
}
