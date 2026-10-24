//! One release boundary for in-app media readers and derived workers.
//!
//! A destructive operation first takes the derived runtime's exclusive claim
//! on its files, then asks every live webview to pause and clear its registered audio/video
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

#[derive(Default)]
struct ReleaseRegistry {
    releases: Mutex<HashMap<u64, Release>>,
    changed: Condvar,
}

static RELEASES: LazyLock<ReleaseRegistry> = LazyLock::new(ReleaseRegistry::default);

struct ReleaseLease<'a> {
    registry: &'a ReleaseRegistry,
    token: Option<u64>,
}

impl ReleaseLease<'_> {
    fn retire(&mut self) -> Result<(), String> {
        let Some(token) = self.token.take() else {
            return Ok(());
        };
        self.registry
            .releases
            .lock()
            .map_err(|_| "media-use state is unavailable".to_string())?
            .remove(&token);
        self.registry.changed.notify_all();
        Ok(())
    }
}

impl Drop for ReleaseLease<'_> {
    fn drop(&mut self) {
        // Ordinary owners retire explicitly so they can surface a state failure.
        // This also removes admission when preparation unwinds unexpectedly.
        if let Err(error) = self.retire() {
            crate::logging::error("media release retirement failed", json!({ "error": error }));
        }
    }
}

impl ReleaseRegistry {
    fn acquire(
        &self,
        token: u64,
        release: Release,
        deadline: Instant,
        publish: impl FnOnce() -> Result<(), String>,
        live_windows: impl Fn() -> HashSet<String>,
        resume: impl FnOnce(),
    ) -> Result<ReleaseLease<'_>, String> {
        self.releases
            .lock()
            .map_err(|_| "media-use state is unavailable".to_string())?
            .insert(token, release);
        let mut lease = ReleaseLease {
            registry: self,
            token: Some(token),
        };
        if let Err(error) = publish().and_then(|()| self.wait(token, deadline, live_windows)) {
            // Retire before resuming: a webview whose acknowledgement races
            // recovery must observe that the operation no longer owns readers.
            let retired = lease.retire();
            resume();
            retired?;
            return Err(error);
        }
        Ok(lease)
    }

    fn wait(
        &self,
        token: u64,
        deadline: Instant,
        live_windows: impl Fn() -> HashSet<String>,
    ) -> Result<(), String> {
        let mut releases = self
            .releases
            .lock()
            .map_err(|_| "media-use state is unavailable".to_string())?;
        loop {
            let live = live_windows();
            let release = releases
                .get_mut(&token)
                .ok_or_else(|| "media release was interrupted".to_string())?;
            release.pending.retain(|label| live.contains(label));
            if release.pending.is_empty() {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                let mut pending: Vec<_> = release.pending.iter().cloned().collect();
                pending.sort();
                return Err(format!(
                    "Media is still in use in {}; no files were changed.",
                    pending.join(", ")
                ));
            }
            releases = self
                .changed
                .wait_timeout(releases, WAIT_SLICE.min(deadline - now))
                .map_err(|_| "media-use state is unavailable".to_string())?
                .0;
        }
    }

    fn current(&self, window_label: &str) -> Result<Option<serde_json::Value>, String> {
        let mut releases = self
            .releases
            .lock()
            .map_err(|_| "media-use state is unavailable".to_string())?;
        let Some((&token, release)) = releases.iter_mut().next() else {
            return Ok(None);
        };
        release.pending.insert(window_label.to_string());
        Ok(Some(
            json!({ "token": token, "keys": release.keys, "restorePlayback": release.restore_playback }),
        ))
    }

    fn acknowledge(&self, token: u64, window_label: &str) -> Result<bool, String> {
        let mut releases = self
            .releases
            .lock()
            .map_err(|_| "media-use state is unavailable".to_string())?;
        if let Some(release) = releases.get_mut(&token) {
            release.pending.remove(window_label);
            self.changed.notify_all();
            return Ok(true);
        }
        Ok(false)
    }
}

pub struct Guard {
    app: AppHandle,
    lease: ReleaseLease<'static>,
    token: u64,
    item_count: Option<u64>,
    exclusive: Option<crate::derived_runtime::ExclusiveGuard>,
    restore_playback: bool,
    resume_on_drop: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        let state_available = match self.lease.retire() {
            Ok(()) => true,
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

/// Prevents new derived readers of `keys`, stops every derived job on them,
/// and releases playback handles in every webview. An empty `keys` list means
/// every displayed item (used for shutdown and index rebuild). Waiting for a
/// job to stop ends only when `cancelled` answers true.
pub fn begin(
    app: &AppHandle,
    keys: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Result<Guard, String> {
    let exclusive = crate::derived_runtime::begin_exclusive(
        app,
        keys,
        crate::derived_runtime::ClaimPolicy::Mutation,
        cancelled,
    )?;
    begin_release(app, keys, true, Some(exclusive), false)
}

/// External applications own an independent session. Readers are released
/// as for a mutation, but only automatic work on the same file is stopped,
/// and an in-app player that had been running is restored paused when the
/// launch call returns.
pub fn begin_external(app: &AppHandle, keys: &[String]) -> Result<Guard, String> {
    let exclusive = crate::derived_runtime::begin_exclusive(
        app,
        keys,
        crate::derived_runtime::ClaimPolicy::ExternalOpen,
        &|| false,
    )?;
    begin_release(app, keys, false, Some(exclusive), false)
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
    record_activity(
        crate::activity::ActivityKind::Admitted,
        token,
        None,
        crate::activity::ActivityState::Waiting,
        None,
        item_count,
    );
    // Even a release with no current windows remains registered until its
    // guard retires, so a newly created webview cannot render during mutation.
    let lease = RELEASES.acquire(
        token,
        Release { pending: app.webview_windows().into_keys().collect(), keys: keys.to_vec(), restore_playback },
        Instant::now() + RELEASE_TIMEOUT,
        || {
            let emit = || app.emit("media-use://release", json!({ "token": token, "keys": keys, "restorePlayback": restore_playback }));
            let publication = if shutting_down { Some(emit()) } else { crate::app_lifecycle::publish_if_running(emit) };
            match publication {
                Some(Ok(())) => Ok(()),
                Some(Err(error)) => Err(format!("could not request media release: {error}")),
                None => Err(crate::scanner::CANCELLED.to_string()),
            }
        },
        || app.webview_windows().into_keys().collect(),
        || crate::failure_runtime::emit_or_record(app, "media-use://resume", json!({ "token": token, "restorePlayback": restore_playback })),
    ).map_err(|error| {
        record_activity(
            crate::activity::ActivityKind::Failed, token,
            Some(crate::activity::ActivityState::Waiting), crate::activity::ActivityState::Failed,
            Some(crate::activity::ActivityReason::Error), item_count,
        );
        error
    })?;
    record_activity(
        crate::activity::ActivityKind::Started,
        token,
        Some(crate::activity::ActivityState::Waiting),
        crate::activity::ActivityState::Running,
        None,
        item_count,
    );
    Ok(Guard {
        app: app.clone(),
        lease,
        token,
        item_count,
        exclusive,
        restore_playback,
        resume_on_drop: !shutting_down,
    })
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
        target_hash: None,
    });
}

/// Registers a bootstrapping webview in an already-active release before that
/// webview is allowed to render. Keeping the release record for the guard's
/// full lifetime closes the create-window-after-broadcast race.
pub fn current(window_label: &str) -> Result<Option<serde_json::Value>, String> {
    RELEASES.current(window_label)
}

/// Returns whether this release is still active. A webview uses the answer to
/// recover if the matching resume event raced ahead of its acknowledgement.
pub fn acknowledge(token: u64, window_label: &str) -> Result<bool, String> {
    RELEASES.acknowledge(token, window_label)
}

#[cfg(test)]
#[path = "../tests/unit/media_use.rs"]
mod tests;
