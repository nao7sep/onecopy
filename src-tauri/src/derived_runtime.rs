//! Ephemeral ownership and lifecycle for fixed derived-work classes. Durable
//! output state stays in `derived_state`; dispatch policy and cursors stay in
//! `derived_work`. This runtime has no dependency on either dispatcher or
//! projection.

use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use serde_json::json;
use tauri::AppHandle;

use crate::derived_state::WorkClass;

#[derive(Clone, Copy)]
pub struct ActiveWorkSnapshot {
    pub(crate) class: WorkClass,
    manual: bool,
    pub(crate) done: Option<u64>,
    pub(crate) total: Option<u64>,
}

#[derive(Default)]
struct RuntimeState {
    master_paused: bool,
    paused_classes: u8,
    active: Option<ActiveWorkSnapshot>,
    active_hash: Option<String>,
    preempt_requested: bool,
    exclusive: bool,
    next_manual_ticket: u64,
    serving_manual_ticket: u64,
}

impl RuntimeState {
    fn paused(&self, class: WorkClass) -> bool {
        self.master_paused || self.paused_classes & class.bit() != 0
    }
}

#[derive(Clone, Copy)]
pub struct RuntimeConditions {
    pub busy: bool,
}

#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub(crate) master_paused: bool,
    pub(crate) paused_classes: u8,
    pub(crate) active: Option<ActiveWorkSnapshot>,
    pub(crate) active_hash: Option<String>,
    pub(crate) preempt_requested: bool,
    pub(crate) busy: bool,
}

static RUNTIME: LazyLock<(Mutex<RuntimeState>, Condvar)> =
    LazyLock::new(|| (Mutex::new(RuntimeState::default()), Condvar::new()));
static POISON_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static POISON_ISSUE_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn request_active_cancel(class: WorkClass) {
    if class.is_transcription() {
        crate::transcription::request_cancel();
    } else if class == WorkClass::Faces {
        crate::face::request_cancel();
    }
}

pub fn cancel_active_transcription() -> bool {
    match RUNTIME.0.lock() {
        Ok(mut runtime) => {
            let Some(active) = runtime.active.filter(|active| active.class.is_transcription()) else {
                return false;
            };
            runtime.preempt_requested = true;
            // Keep the process-global signal tied to the runtime owner selected
            // above; a queued transcription cannot take over between the two.
            request_active_cancel(active.class);
            true
        }
        Err(_) => {
            report_poison_once(None);
            false
        }
    }
}

fn report_poison_once(app: Option<&AppHandle>) {
    if !POISON_LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        crate::logging::error(
            "background-work state is unavailable",
            serde_json::json!({}),
        );
    }
    if let Some(app) = app {
        if !POISON_ISSUE_REPORTED.load(std::sync::atomic::Ordering::SeqCst)
            && crate::failure_runtime::report(
                app,
                "background-work-state-failed",
                None,
                "Background-work state is unavailable. Restart OneCopy to repair it.",
            )
            .is_ok()
        {
            POISON_ISSUE_REPORTED.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

struct ActiveGuard {
    app: AppHandle,
    class: WorkClass,
    manual_ticket: Option<u64>,
}

impl ActiveGuard {
    fn begin(app: &AppHandle, class: WorkClass) -> Result<Option<Self>, String> {
        let mut runtime = RUNTIME.0.lock().map_err(|_| {
            report_poison_once(Some(app));
            "background-work state is unavailable".to_string()
        })?;
        if runtime.exclusive || runtime.paused(class) || runtime.active.is_some() {
            return Ok(None);
        }
        runtime.active = Some(ActiveWorkSnapshot {
            class,
            manual: false,
            done: None,
            total: None,
        });
        runtime.active_hash = None;
        drop(runtime);
        emit_state_changed(app);
        Ok(Some(Self {
            app: app.clone(),
            class,
            manual_ticket: None,
        }))
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut runtime) = RUNTIME.0.lock() {
            if runtime.active.map(|active| active.class) == Some(self.class) {
                runtime.active = None;
                runtime.active_hash = None;
                runtime.preempt_requested = false;
                if self.manual_ticket == Some(runtime.serving_manual_ticket) {
                    runtime.serving_manual_ticket = runtime.serving_manual_ticket.wrapping_add(1);
                }
                RUNTIME.1.notify_all();
            }
        } else {
            report_poison_once(Some(&self.app));
        }
        emit_state_changed(&self.app);
    }
}

pub(crate) fn with_active<T>(
    app: &AppHandle,
    class: WorkClass,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    let Some(_active) = ActiveGuard::begin(app, class)? else {
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
        .map_err(|_| {
            report_poison_once(Some(app));
            "background-work state is unavailable".to_string()
        })?;
    if runtime.exclusive {
        return Err("A file operation is using the media boundary.".to_string());
    }
    if runtime.paused(class) {
        return Err(paused_message(class));
    }
    if runtime.active.map(|active| active.manual).unwrap_or(false) {
        return Err("Another requested media task is already running.".to_string());
    }
    if let Some(active) = runtime.active {
        runtime.preempt_requested = true;
        drop(runtime);
        request_active_cancel(active.class);
        emit_state_changed(app);
        runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| {
                report_poison_once(Some(app));
                "background-work state is unavailable".to_string()
            })?;
        let (next, waited) = RUNTIME
            .1
            .wait_timeout_while(runtime, Duration::from_secs(10), |state| {
                state.active.is_some()
            })
            .map_err(|_| {
                report_poison_once(Some(app));
                "background-work state is unavailable".to_string()
            })?;
        runtime = next;
        if waited.timed_out() && runtime.active.is_some() {
            return Err("Background work is still stopping. Try again shortly.".to_string());
        }
        if runtime.paused(class) {
            return Err(paused_message(class));
        }
        if runtime.exclusive {
            return Err("A file operation is using the media boundary.".to_string());
        }
    }
    runtime.active = Some(ActiveWorkSnapshot {
        class,
        manual: true,
        done: None,
        total: None,
    });
    runtime.active_hash = None;
    runtime.preempt_requested = false;
    drop(runtime);
    emit_state_changed(app);
    Ok(ManualWorkGuard {
        _guard: ActiveGuard {
            app: app.clone(),
            class,
            manual_ticket: None,
        },
    })
}

/// Waits in FIFO order for the shared media boundary. Manual transcription
/// commands return to the UI before entering this wait, so a second request is
/// a real queued job rather than a rejected or blocking command.
pub fn begin_manual_queued(app: &AppHandle, class: &str) -> Result<ManualWorkGuard, String> {
    let class =
        WorkClass::parse(class).ok_or_else(|| format!("unknown background-work class: {class}"))?;
    let mut runtime = RUNTIME
        .0
        .lock()
        .map_err(|_| {
            report_poison_once(Some(app));
            "background-work state is unavailable".to_string()
        })?;
    let ticket = runtime.next_manual_ticket;
    runtime.next_manual_ticket = runtime.next_manual_ticket.wrapping_add(1);

    if runtime.active.is_some_and(|active| !active.manual) {
        let cancel_class = runtime.active.map(|active| active.class);
        runtime.preempt_requested = true;
        drop(runtime);
        if let Some(class) = cancel_class {
            request_active_cancel(class);
        }
        emit_state_changed(app);
        runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| {
                report_poison_once(Some(app));
                "background-work state is unavailable".to_string()
            })?;
    }

    runtime = RUNTIME
        .1
        .wait_while(runtime, |state| {
            state.exclusive || state.active.is_some() || state.serving_manual_ticket != ticket
        })
        .map_err(|_| {
            report_poison_once(Some(app));
            "background-work state is unavailable".to_string()
        })?;

    if runtime.paused(class) {
        runtime.serving_manual_ticket = runtime.serving_manual_ticket.wrapping_add(1);
        RUNTIME.1.notify_all();
        return Err(paused_message(class));
    }
    runtime.active = Some(ActiveWorkSnapshot {
        class,
        manual: true,
        done: None,
        total: None,
    });
    runtime.active_hash = None;
    runtime.preempt_requested = false;
    drop(runtime);
    emit_state_changed(app);
    Ok(ManualWorkGuard {
        _guard: ActiveGuard {
            app: app.clone(),
            class,
            manual_ticket: Some(ticket),
        },
    })
}

/// Temporarily excludes every automatic and requested derived-media owner.
/// Mutations use this before asking webviews to release playback handles, so
/// neither a cache derive nor a model job can reopen an item during the
/// release-to-mutate interval.
pub struct ExclusiveGuard {
    app: AppHandle,
}

impl Drop for ExclusiveGuard {
    fn drop(&mut self) {
        if let Ok(mut runtime) = RUNTIME.0.lock() {
            runtime.exclusive = false;
            runtime.preempt_requested = false;
            RUNTIME.1.notify_all();
        } else {
            report_poison_once(Some(&self.app));
        }
        emit_state_changed(&self.app);
    }
}

pub fn begin_exclusive(app: &AppHandle) -> Result<ExclusiveGuard, String> {
    let mut runtime = RUNTIME
        .0
        .lock()
        .map_err(|_| {
            report_poison_once(Some(app));
            "background-work state is unavailable".to_string()
        })?;
    if runtime.exclusive {
        return Err("Another file operation is already running.".to_string());
    }
    runtime.exclusive = true;
    if let Some(active) = runtime.active {
        runtime.preempt_requested = true;
        drop(runtime);
        request_active_cancel(active.class);
        emit_state_changed(app);
        runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| {
                report_poison_once(Some(app));
                "background-work state is unavailable".to_string()
            })?;
        let (next, waited) = RUNTIME
            .1
            .wait_timeout_while(runtime, Duration::from_secs(10), |state| {
                state.active.is_some()
            })
            .map_err(|_| {
                report_poison_once(Some(app));
                "background-work state is unavailable".to_string()
            })?;
        runtime = next;
        if waited.timed_out() && runtime.active.is_some() {
            runtime.exclusive = false;
            runtime.preempt_requested = false;
            RUNTIME.1.notify_all();
            drop(runtime);
            emit_state_changed(app);
            return Err("Media work is still stopping; no files were changed.".to_string());
        }
    }
    drop(runtime);
    emit_state_changed(app);
    Ok(ExclusiveGuard { app: app.clone() })
}

pub(crate) fn exclusive() -> bool {
    RUNTIME
        .0
        .lock()
        .map(|runtime| runtime.exclusive)
        .unwrap_or_else(|_| {
            report_poison_once(None);
            true
        })
}

pub(crate) fn automatic_optional_active() -> bool {
    RUNTIME
        .0
        .lock()
        .map(|runtime| {
            runtime
                .active
                .is_some_and(|active| !active.manual && active.class != WorkClass::Previews)
        })
        .unwrap_or_else(|_| {
            report_poison_once(None);
            false
        })
}

pub(crate) fn preempt_automatic_optional_for_required() {
    let active_class = RUNTIME.0.lock().ok().and_then(|mut runtime| {
        let active = runtime
            .active
            .filter(|active| !active.manual && active.class != WorkClass::Previews)?;
        runtime.preempt_requested = true;
        Some(active.class)
    });
    if let Some(class) = active_class {
        request_active_cancel(class);
    }
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
        .unwrap_or_else(|_| {
            report_poison_once(None);
            true
        })
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
        .map(|runtime| {
            runtime.preempt_requested
                || runtime
                    .active
                    .map(|active| runtime.paused(active.class))
                    .unwrap_or(false)
        })
        .unwrap_or_else(|_| {
            report_poison_once(None);
            true
        })
}

pub fn set_paused(app: &AppHandle, class: Option<&str>, paused: bool) -> Result<(), String> {
    let active = {
        let mut runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| {
                report_poison_once(Some(app));
                "background-work state is unavailable".to_string()
            })?;
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
        RUNTIME.1.notify_all();
        runtime.active
    };

    if paused {
        if let Some(active) = active.filter(|active| {
            class.is_none() || class == Some(active.class.id())
        }) {
            request_active_cancel(active.class);
        }
    }
    emit_state_changed(app);
    Ok(())
}

pub(crate) fn pause_for_safety(app: &AppHandle, class: WorkClass) -> Result<(), String> {
    set_paused(app, Some(class.id()), true)
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
    } else {
        report_poison_once(Some(app));
    }
    crate::failure_runtime::emit_or_record(
        app,
        "derived://progress",
        json!({ "class": class.id(), "done": done, "total": total }),
    );
    emit_state_changed(app);
}

pub(crate) fn active_item(app: &AppHandle, class: WorkClass, hash: &str) {
    if let Ok(mut runtime) = RUNTIME.0.lock() {
        if runtime.active.map(|active| active.class) == Some(class) {
            runtime.active_hash = Some(hash.to_string());
        }
    } else {
        report_poison_once(Some(app));
    }
    emit_state_changed(app);
}

pub fn report_manual_progress(app: &AppHandle, class: &str, done: u64, total: u64) {
    if let Some(class) = WorkClass::parse(class) {
        progress(app, class, Some((done, total)));
    }
}

pub(crate) fn emit_state_changed(app: &AppHandle) {
    let payload = match RUNTIME.0.lock() {
        Ok(runtime) => {
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
                    "hash": runtime.active_hash.as_deref(),
                    "done": active.done,
                    "total": active.total,
                    "stopping": runtime.preempt_requested || runtime.paused(active.class),
                })),
            })
        }
        Err(_) => {
            report_poison_once(Some(app));
            return;
        }
    };
    crate::failure_runtime::emit_or_record(app, "derived://state-changed", payload);
}

pub fn snapshot(conditions: RuntimeConditions) -> Result<RuntimeSnapshot, String> {
    let (master_paused, paused_classes, active, active_hash, preempt_requested) = {
        let runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| {
                report_poison_once(None);
                "background-work state is unavailable".to_string()
            })?;
        (
            runtime.master_paused,
            runtime.paused_classes,
            runtime.active,
            runtime.active_hash.clone(),
            runtime.preempt_requested,
        )
    };
    Ok(RuntimeSnapshot {
        master_paused,
        paused_classes,
        active,
        active_hash,
        preempt_requested,
        busy: conditions.busy,
    })
}
