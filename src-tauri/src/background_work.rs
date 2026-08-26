//! Runtime control and read projection for the fixed background-work classes.
//! This is not a job store: absent/stale outputs remain the reconstructible
//! queue in the index, while active and paused state exists only for this app
//! process.

use std::path::Path;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::derived_state::{WorkCapabilities, WorkClass};

#[derive(Clone, Copy)]
struct ActiveWork {
    class: WorkClass,
    manual: bool,
    done: Option<u64>,
    total: Option<u64>,
}

#[derive(Default)]
struct RuntimeState {
    master_paused: bool,
    paused_classes: u8,
    active: Option<ActiveWork>,
    preempt_requested: bool,
}

impl RuntimeState {
    fn paused(&self, class: WorkClass) -> bool {
        self.master_paused || self.paused_classes & class.bit() != 0
    }
}

static RUNTIME: LazyLock<(Mutex<RuntimeState>, Condvar)> =
    LazyLock::new(|| (Mutex::new(RuntimeState::default()), Condvar::new()));

pub(crate) struct ActiveGuard {
    app: AppHandle,
    class: WorkClass,
}

impl ActiveGuard {
    fn begin(app: &AppHandle, class: WorkClass) -> Option<Self> {
        let mut runtime = RUNTIME.0.lock().ok()?;
        if runtime.paused(class) || runtime.active.is_some() {
            return None;
        }
        runtime.active = Some(ActiveWork {
            class,
            manual: false,
            done: None,
            total: None,
        });
        drop(runtime);
        emit_state_changed(app);
        Some(Self {
            app: app.clone(),
            class,
        })
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut runtime) = RUNTIME.0.lock() {
            if runtime.active.map(|active| active.class) == Some(self.class) {
                runtime.active = None;
                runtime.preempt_requested = false;
                RUNTIME.1.notify_all();
            }
        }
        emit_state_changed(&self.app);
    }
}

pub(crate) fn with_active<T>(
    app: &AppHandle,
    class: WorkClass,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    let Some(_active) = ActiveGuard::begin(app, class) else {
        return Ok(None);
    };
    work().map(Some)
}

pub struct ManualWorkGuard {
    _guard: ActiveGuard,
}

pub fn begin_manual(app: &AppHandle, class: &str) -> Result<ManualWorkGuard, String> {
    let class =
        WorkClass::parse(class).ok_or_else(|| format!("unknown background-work class: {class}"))?;
    let mut runtime = RUNTIME
        .0
        .lock()
        .map_err(|_| "background-work state is unavailable".to_string())?;
    if runtime.paused(class) {
        return Err(paused_message(class));
    }
    if runtime.active.map(|active| active.manual).unwrap_or(false) {
        return Err("Another requested media task is already running.".to_string());
    }
    if let Some(active) = runtime.active {
        runtime.preempt_requested = true;
        drop(runtime);
        if active.class == WorkClass::Transcripts {
            crate::transcription::request_cancel();
        }
        emit_state_changed(app);
        runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| "background-work state is unavailable".to_string())?;
        let (next, waited) = RUNTIME
            .1
            .wait_timeout_while(runtime, Duration::from_secs(10), |state| {
                state.active.is_some()
            })
            .map_err(|_| "background-work state is unavailable".to_string())?;
        runtime = next;
        if waited.timed_out() && runtime.active.is_some() {
            return Err("Background work is still stopping. Try again shortly.".to_string());
        }
        if runtime.paused(class) {
            return Err(paused_message(class));
        }
    }
    runtime.active = Some(ActiveWork {
        class,
        manual: true,
        done: None,
        total: None,
    });
    runtime.preempt_requested = false;
    drop(runtime);
    emit_state_changed(app);
    Ok(ManualWorkGuard {
        _guard: ActiveGuard {
            app: app.clone(),
            class,
        },
    })
}

fn paused_message(class: WorkClass) -> String {
    format!(
        "{} work is paused. Resume it from Background work.",
        class.id()
    )
}

pub(crate) fn is_paused(class: WorkClass) -> bool {
    RUNTIME
        .0
        .lock()
        .map(|runtime| runtime.paused(class))
        .unwrap_or(true)
}

/// Shared by every owned ffmpeg process. A pause kills its child within the
/// subprocess poll interval; in-process work stops at the next safe item edge.
pub fn cancelled() -> bool {
    if crate::scanner::cancelled() {
        return true;
    }
    RUNTIME
        .0
        .lock()
        .ok()
        .map(|runtime| {
            runtime.preempt_requested
                || runtime
                    .active
                    .map(|active| runtime.paused(active.class))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

pub fn set_paused(app: &AppHandle, class: Option<&str>, paused: bool) -> Result<(), String> {
    let active = {
        let mut runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| "background-work state is unavailable".to_string())?;
        match class {
            None => runtime.master_paused = paused,
            Some(id) => {
                let class = WorkClass::parse(id)
                    .ok_or_else(|| format!("unknown background-work class: {id}"))?;
                if paused {
                    runtime.paused_classes |= class.bit();
                } else {
                    runtime.paused_classes &= !class.bit();
                }
            }
        }
        runtime.active
    };

    if paused
        && active
            .map(|active| class.is_none() || class == Some(active.class.id()))
            .unwrap_or(false)
        && active.map(|active| active.class) == Some(WorkClass::Transcripts)
    {
        crate::transcription::request_cancel();
    }
    emit_state_changed(app);
    crate::derived_work::wake(false);
    Ok(())
}

pub(crate) fn progress(app: &AppHandle, class: WorkClass, counts: Option<(u64, u64)>) {
    let (done, total) = counts.map_or((None, None), |(done, total)| (Some(done), Some(total)));
    if let Ok(mut runtime) = RUNTIME.0.lock() {
        if let Some(active) = runtime
            .active
            .as_mut()
            .filter(|active| active.class == class)
        {
            active.done = done;
            active.total = total;
        }
    }
    let _ = app.emit(
        "derived://progress",
        json!({ "class": class.id(), "done": done, "total": total }),
    );
    emit_state_changed(app);
}

pub fn report_manual_progress(app: &AppHandle, class: &str, done: u64, total: u64) {
    if let Some(class) = WorkClass::parse(class) {
        progress(app, class, Some((done, total)));
    }
}

pub(crate) fn emit_state_changed(app: &AppHandle) {
    let payload = RUNTIME
        .0
        .lock()
        .map(|runtime| {
            let paused_classes = WorkClass::ALL
                .into_iter()
                .filter(|class| runtime.paused_classes & class.bit() != 0)
                .map(WorkClass::id)
                .collect::<Vec<_>>();
            json!({
                "masterPaused": runtime.master_paused,
                "pausedClasses": paused_classes,
                "active": runtime.active.map(|active| json!({
                    "id": active.class.id(),
                    "done": active.done,
                    "total": active.total,
                    "stopping": runtime.preempt_requested || runtime.paused(active.class),
                })),
            })
        })
        .unwrap_or_else(|_| json!({}));
    let _ = app.emit("derived://state-changed", payload);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundWorkSnapshot {
    master_paused: bool,
    classes: Vec<BackgroundClassSnapshot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundClassSnapshot {
    id: &'static str,
    state: &'static str,
    queued: u64,
    failed: u64,
    done: Option<u64>,
    total: Option<u64>,
    reason: Option<&'static str>,
}

fn whisper_available(data_root: &Path) -> bool {
    crate::binaries_manager::spec_of("whisper-large-v3-turbo")
        .map(|spec| {
            crate::binaries_manager::state_of(data_root, spec).status
                != crate::binaries::BinaryStatus::NotInstalled
        })
        .unwrap_or(false)
}

pub fn snapshot(
    data_root: &Path,
    similarity_dirty: bool,
) -> Result<BackgroundWorkSnapshot, String> {
    let config = crate::storage::read_config_for_setup(data_root)?;
    let settings = crate::derived_work::settings_from_config(config.as_ref(), data_root);
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let (master_paused, paused_classes, active, preempt_requested) = RUNTIME
        .0
        .lock()
        .map(|runtime| {
            (
                runtime.master_paused,
                runtime.paused_classes,
                runtime.active,
                runtime.preempt_requested,
            )
        })
        .map_err(|_| "background-work state is unavailable".to_string())?;
    let busy = !crate::derived_work::available();
    let idle = crate::derived_work::is_idle();
    let capabilities = WorkCapabilities {
        ffmpeg: settings.ffmpeg.is_some(),
        face_enabled: settings.face_enabled,
        face_models: settings.face_models.is_some(),
        transcripts: settings.ffmpeg.is_some() && whisper_available(&settings.data_root),
    };
    let mut classes = Vec::with_capacity(WorkClass::ALL.len());
    for class in WorkClass::ALL {
        let debt = crate::derived_state::work_debt(
            &conn,
            class,
            capabilities,
            similarity_dirty,
        )?;
        let paused = master_paused || paused_classes & class.bit() != 0;
        let is_active = active.map(|value| value.class) == Some(class);
        let active_progress = active.filter(|value| value.class == class);
        let queued = debt.runnable + debt.blocked;
        let (state, reason) = if is_active && (paused || preempt_requested) {
            ("stopping", None)
        } else if is_active {
            ("running", None)
        } else if paused {
            ("paused", None)
        } else if debt.disabled {
            ("disabled", debt.reason)
        } else if debt.runnable == 0 && debt.blocked > 0 {
            ("unavailable", debt.reason)
        } else if debt.runnable == 0 && debt.failed > 0 {
            ("failed", Some("Open Issues to retry failed work"))
        } else if debt.runnable > 0 && busy {
            ("waiting", Some("Waiting for indexing or a file operation"))
        } else if debt.runnable > 0 && class.idle_only() && !idle {
            ("waiting", Some("Waiting until the app is idle"))
        } else if debt.runnable > 0 {
            ("queued", debt.reason)
        } else {
            ("up-to-date", debt.reason)
        };
        classes.push(BackgroundClassSnapshot {
            id: class.id(),
            state,
            queued,
            failed: debt.failed,
            done: active_progress.and_then(|value| value.done),
            total: active_progress.and_then(|value| value.total),
            reason,
        });
    }
    Ok(BackgroundWorkSnapshot {
        master_paused,
        classes,
    })
}
