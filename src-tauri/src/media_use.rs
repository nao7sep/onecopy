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
    restore_playback: bool,
}

static RELEASES: LazyLock<(Mutex<HashMap<u64, Release>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(HashMap::new()), Condvar::new()));

pub struct Guard {
    app: AppHandle,
    token: u64,
    item_count: Option<u64>,
    exclusive: Option<crate::derived_runtime::ExclusiveGuard>,
    restore_playback: bool,
    resume_on_drop: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let state_available = match RELEASES.0.lock() {
            Ok(mut releases) => {
                releases.remove(&self.token);
                RELEASES.1.notify_all();
                true
            }
            Err(_) => {
                let _ = crate::failure_runtime::report(
                    &self.app,
                    "media-use-state-failed",
                    None,
                    "Media ownership state is unavailable. Restart OneCopy before changing files.",
                );
                false
            }
        };
        if self.resume_on_drop {
            crate::failure_runtime::emit_or_record(
                &self.app,
                "media-use://resume",
                json!({ "token": self.token, "restorePlayback": self.restore_playback }),
            );
        }
        self.exclusive.take();
        if self.resume_on_drop {
            crate::derived_work::wake();
        }
        record_activity(
            if state_available {
                crate::activity::ActivityKind::Completed
            } else {
                crate::activity::ActivityKind::Failed
            },
            self.token,
            Some(crate::activity::ActivityState::Running),
            if state_available {
                crate::activity::ActivityState::Succeeded
            } else {
                crate::activity::ActivityState::Failed
            },
            Some(if state_available {
                crate::activity::ActivityReason::Completion
            } else {
                crate::activity::ActivityReason::Error
            }),
            self.item_count,
        );
    }
}

/// Prevents new derived readers, stops the active one, and releases playback
/// handles in every webview. An empty `keys` list means every displayed item
/// (used for shutdown and Trash-root operations).
pub fn begin(app: &AppHandle, keys: &[String]) -> Result<Guard, String> {
    begin_with_resume_policy(app, keys, true)
}

/// External applications own an independent session. Readers are released
/// exactly as for a mutation, but an in-app player that had been running is
/// restored paused when the launch call returns.
pub fn begin_external(app: &AppHandle, keys: &[String]) -> Result<Guard, String> {
    begin_with_resume_policy(app, keys, false)
}

fn begin_with_resume_policy(
    app: &AppHandle,
    keys: &[String],
    restore_playback: bool,
) -> Result<Guard, String> {
    let exclusive = crate::derived_runtime::begin_exclusive(app)?;
    begin_release(app, keys, restore_playback, Some(exclusive), false)
}

/// Releases every webview only after all derived-media workers have joined.
/// This is one of the exit owner's two intentional final publications; it
/// neither reopens derived admission nor emits a resume while windows close.
pub fn begin_shutdown(app: &AppHandle) -> Result<Guard, String> {
    begin_release(app, &[], false, None, true)
}

fn begin_release(
    app: &AppHandle,
    keys: &[String],
    restore_playback: bool,
    exclusive: Option<crate::derived_runtime::ExclusiveGuard>,
    shutting_down: bool,
) -> Result<Guard, String> {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    // An empty key list means "all displayed media", not zero items.
    let item_count = (!keys.is_empty()).then_some(keys.len() as u64);
    let windows: HashSet<String> = app.webview_windows().into_keys().collect();
    if windows.is_empty() {
        record_activity(
            crate::activity::ActivityKind::Started,
            token,
            None,
            crate::activity::ActivityState::Running,
            None,
            item_count,
        );
        return Ok(Guard {
            app: app.clone(),
            token,
            item_count,
            exclusive,
            restore_playback,
            resume_on_drop: !shutting_down,
        });
    }

    record_activity(
        crate::activity::ActivityKind::Admitted,
        token,
        None,
        crate::activity::ActivityState::Waiting,
        None,
        item_count,
    );
    let mut releases = RELEASES.0.lock().map_err(|_| {
        record_activity(
            crate::activity::ActivityKind::Failed,
            token,
            Some(crate::activity::ActivityState::Waiting),
            crate::activity::ActivityState::Failed,
            Some(crate::activity::ActivityReason::Error),
            item_count,
        );
        "media-use state is unavailable".to_string()
    })?;
    releases.insert(
        token,
        Release {
            pending: windows,
            keys: keys.to_vec(),
            restore_playback,
        },
    );
    drop(releases);
    let emit = || app.emit("media-use://release", json!({ "token": token, "keys": keys }));
    let publication = if shutting_down {
        Some(emit())
    } else {
        crate::app_lifecycle::publish_if_running(emit)
    };
    let publication_error = match publication {
        Some(Ok(())) => None,
        Some(Err(error)) => Some(format!("could not request media release: {error}")),
        None => Some(crate::scanner::CANCELLED.to_string()),
    };
    if let Some(error) = publication_error {
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
        record_activity(
            crate::activity::ActivityKind::Failed,
            token,
            Some(crate::activity::ActivityState::Waiting),
            crate::activity::ActivityState::Failed,
            Some(crate::activity::ActivityReason::Error),
            item_count,
        );
        return Err(error);
    }

    let deadline = Instant::now() + RELEASE_TIMEOUT;
    let mut releases = RELEASES.0.lock().map_err(|_| {
        record_activity(
            crate::activity::ActivityKind::Failed,
            token,
            Some(crate::activity::ActivityState::Waiting),
            crate::activity::ActivityState::Failed,
            Some(crate::activity::ActivityReason::Error),
            item_count,
        );
        "media-use state is unavailable".to_string()
    })?;
    loop {
        let live: HashSet<String> = app.webview_windows().into_keys().collect();
        let Some(release) = releases.get_mut(&token) else {
            record_activity(
                crate::activity::ActivityKind::Failed,
                token,
                Some(crate::activity::ActivityState::Waiting),
                crate::activity::ActivityState::Failed,
                Some(crate::activity::ActivityReason::Error),
                item_count,
            );
            return Err("media release was interrupted".to_string());
        };
        release.pending.retain(|label| live.contains(label));
        if release.pending.is_empty() {
            record_activity(
                crate::activity::ActivityKind::Started,
                token,
                Some(crate::activity::ActivityState::Waiting),
                crate::activity::ActivityState::Running,
                None,
                item_count,
            );
            return Ok(Guard {
                app: app.clone(),
                token,
                item_count,
                exclusive,
                restore_playback,
                resume_on_drop: !shutting_down,
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
            record_activity(
                crate::activity::ActivityKind::Failed,
                token,
                Some(crate::activity::ActivityState::Waiting),
                crate::activity::ActivityState::Failed,
                Some(crate::activity::ActivityReason::Error),
                item_count,
            );
            return Err(format!(
                "Media is still in use in {pending}; no files were changed."
            ));
        }
        let wait = RELEASES
            .1
            .wait_timeout(releases, WAIT_SLICE.min(deadline - now))
            .map_err(|_| {
                record_activity(
                    crate::activity::ActivityKind::Failed,
                    token,
                    Some(crate::activity::ActivityState::Waiting),
                    crate::activity::ActivityState::Failed,
                    Some(crate::activity::ActivityReason::Error),
                    item_count,
                );
                "media-use state is unavailable".to_string()
            })?;
        releases = wait.0;
    }
}

fn record_activity(
    kind: crate::activity::ActivityKind,
    token: u64,
    previous: Option<crate::activity::ActivityState>,
    current: crate::activity::ActivityState,
    reason: Option<crate::activity::ActivityReason>,
    item_count: Option<u64>,
) {
    let _ = crate::activity::record(crate::activity::ActivityDraft {
        kind,
        owner: crate::activity::ActivityOwner::Media,
        subject: None,
        operation_id: Some(format!("media:{token}")),
        cause_id: None,
        generation: Some(token),
        previous,
        current: Some(current),
        reason,
        lane: None,
        item_count,
        queued: None,
        done: None,
        total: None,
    });
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
    Ok(Some(json!({
        "token": token,
        "keys": release.keys,
        "restorePlayback": release.restore_playback,
    })))
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
