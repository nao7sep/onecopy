//! One release boundary for in-app media readers and derived workers.
//!
//! A destructive operation first takes the derived runtime's exclusive claim,
//! then asks every live webview to pause and clear its registered audio/video
//! elements. The mutation starts only after every still-live webview
//! acknowledges. Nothing durable is recorded: a timed-out release changes no
//! file, and dropping the guard resumes the webviews and background owner.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const WAIT_SLICE: Duration = Duration::from_millis(50);

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
struct Release {
    pending: HashSet<String>,
    keys: Vec<String>,
}

static RELEASES: LazyLock<(Mutex<HashMap<u64, Release>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(HashMap::new()), Condvar::new()));

pub struct Guard {
    app: AppHandle,
    token: u64,
    exclusive: Option<crate::derived_runtime::ExclusiveGuard>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut releases) = RELEASES.0.lock() {
            releases.remove(&self.token);
            RELEASES.1.notify_all();
        } else {
            let _ = crate::failure_runtime::report(
                &self.app,
                "media-use-state-failed",
                None,
                "Media ownership state is unavailable. Restart OneCopy before changing files.",
            );
        }
        crate::failure_runtime::emit_or_record(
            &self.app,
            "media-use://resume",
            json!({ "token": self.token }),
        );
        self.exclusive.take();
        crate::derived_work::wake(false);
    }
}

/// Prevents new derived readers, stops the active one, and releases playback
/// handles in every webview. An empty `keys` list means every displayed item
/// (used for shutdown and Trash-root operations).
pub fn begin(app: &AppHandle, keys: &[String]) -> Result<Guard, String> {
    let exclusive = crate::derived_runtime::begin_exclusive(app)?;
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    let windows: HashSet<String> = app.webview_windows().into_keys().collect();
    if windows.is_empty() {
        return Ok(Guard {
            app: app.clone(),
            token,
            exclusive: Some(exclusive),
        });
    }

    RELEASES
        .0
        .lock()
        .map_err(|_| "media-use state is unavailable".to_string())?
        .insert(
            token,
            Release {
                pending: windows,
                keys: keys.to_vec(),
            },
        );
    if let Err(error) = app.emit(
        "media-use://release",
        json!({ "token": token, "keys": keys }),
    ) {
        match RELEASES.0.lock() {
            Ok(mut releases) => {
                releases.remove(&token);
            }
            Err(_) => {
                let _ = crate::failure_runtime::report(
                    app,
                    "media-use-state-failed",
                    None,
                    "Media ownership state is unavailable. Restart OneCopy before changing files.",
                );
            }
        }
        return Err(format!("could not request media release: {error}"));
    }

    let deadline = Instant::now() + RELEASE_TIMEOUT;
    let mut releases = RELEASES
        .0
        .lock()
        .map_err(|_| "media-use state is unavailable".to_string())?;
    loop {
        let live: HashSet<String> = app.webview_windows().into_keys().collect();
        let Some(release) = releases.get_mut(&token) else {
            return Err("media release was interrupted".to_string());
        };
        release.pending.retain(|label| live.contains(label));
        if release.pending.is_empty() {
            return Ok(Guard {
                app: app.clone(),
                token,
                exclusive: Some(exclusive),
            });
        }
        let now = Instant::now();
        if now >= deadline {
            let pending = release
                .pending
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            releases.remove(&token);
            drop(releases);
            crate::failure_runtime::emit_or_record(
                app,
                "media-use://resume",
                json!({ "token": token }),
            );
            return Err(format!(
                "Media is still in use in {pending}; no files were changed."
            ));
        }
        let wait = RELEASES
            .1
            .wait_timeout(releases, WAIT_SLICE.min(deadline - now))
            .map_err(|_| "media-use state is unavailable".to_string())?;
        releases = wait.0;
    }
}

/// Registers a bootstrapping webview in an already-active release before that
/// webview is allowed to render. Keeping the release record for the guard's
/// full lifetime closes the create-window-after-broadcast race.
pub fn current(window_label: &str) -> Result<Option<serde_json::Value>, String> {
    let mut releases = RELEASES
        .0
        .lock()
        .map_err(|_| "media-use state is unavailable".to_string())?;
    let Some((&token, release)) = releases.iter_mut().next() else {
        return Ok(None);
    };
    release.pending.insert(window_label.to_string());
    Ok(Some(json!({ "token": token, "keys": release.keys })))
}

/// Returns whether this release is still active. A webview uses the answer to
/// recover if the matching resume event raced ahead of its acknowledgement.
pub fn acknowledge(token: u64, window_label: &str) -> Result<bool, String> {
    let mut releases = RELEASES
        .0
        .lock()
        .map_err(|_| "media-use state is unavailable".to_string())?;
    if let Some(release) = releases.get_mut(&token) {
        release.pending.remove(window_label);
        RELEASES.1.notify_all();
        return Ok(true);
    }
    Ok(false)
}
