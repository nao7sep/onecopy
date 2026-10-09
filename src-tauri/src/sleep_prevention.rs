//! Actual execution owns idle-sleep prevention, never durable queue depth.
//! Windows SetThreadExecutionState is thread-affine: the assertion must be
//! created AND dropped on this dedicated owner thread, not on worker threads.

use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;

const ISSUE: &str = "sleep-prevention-failed";
static SERVICE: OnceLock<Arc<Control>> = OnceLock::new();
static WORKER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

thread_local! {
    /// Work scopes this thread holds, so a parked owner can release exactly
    /// its own part of the assertion.
    static HELD: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Default)]
struct State {
    active: usize,
    enabled: bool,
    stopped: bool,
    revision: u64,
    preference_revision: u64,
}

impl State {
    fn wanted(self) -> bool {
        self.enabled && self.active > 0 && !self.stopped
    }

    fn should_acquire(self, held: bool, failed_preference: Option<u64>) -> bool {
        self.wanted() && !held && failed_preference != Some(self.preference_revision)
    }
}

#[derive(Default)]
struct Control {
    state: Mutex<State>,
    changed: Condvar,
}

impl Control {
    fn update(&self, update: impl FnOnce(&mut State)) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        update(&mut state);
        state.revision += 1;
        self.changed.notify_one();
    }

    fn configure(&self, enabled: bool) {
        self.update(|state| {
            if state.enabled != enabled {
                state.enabled = enabled;
                state.preference_revision += 1;
            }
        });
    }

    fn begin(self: &Arc<Self>) -> WorkGuard {
        self.update(|state| state.active += 1);
        HELD.with(|held| held.set(held.get() + 1));
        WorkGuard(Some(Arc::clone(self)))
    }

    fn park(self: &Arc<Self>) -> ParkedGuard {
        let held = HELD.with(Cell::get);
        if held > 0 {
            self.update(|state| state.active -= held);
        }
        ParkedGuard(Some((Arc::clone(self), held)))
    }
}

/// Kept through the actual operation, including its cancellation cleanup.
/// Returning an error or unwinding a worker drops it just like success. It
/// is created and dropped on the thread that does the work.
pub(crate) struct WorkGuard(Option<Arc<Control>>);

impl Drop for WorkGuard {
    fn drop(&mut self) {
        if let Some(control) = &self.0 {
            HELD.with(|held| held.set(held.get() - 1));
            control.update(|state| state.active -= 1);
        }
    }
}

pub(crate) fn begin_work() -> WorkGuard {
    SERVICE.get().map_or(WorkGuard(None), Control::begin)
}

/// Releases the calling thread's work scopes while its owner is parked
/// waiting for other work, which is idle rather than actual execution, and
/// takes them back when dropped.
pub(crate) struct ParkedGuard(Option<(Arc<Control>, usize)>);

impl Drop for ParkedGuard {
    fn drop(&mut self) {
        if let Some((control, held)) = self.0.take() {
            if held > 0 {
                control.update(|state| state.active += held);
            }
        }
    }
}

pub(crate) fn idle_while_parked() -> ParkedGuard {
    SERVICE.get().map_or(ParkedGuard(None), Control::park)
}

pub(crate) fn configured(config: Option<&serde_json::Value>) -> bool {
    config
        .and_then(|value| value.get("keepAwakeDuringIndexing"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| crate::storage::DefaultConfig::default().keep_awake_during_indexing)
}

/// Called inside the durable config writer's serialization boundary, after
/// its atomic save. Racing callers cannot publish older merged preferences.
pub(crate) fn configure(config: &serde_json::Value) {
    if let Some(control) = SERVICE.get() {
        control.configure(configured(Some(config)));
    }
}

fn run<L>(
    control: &Control,
    mut acquire: impl FnMut() -> Result<L, String>,
    mut report: impl FnMut(&str) -> Result<(), String>,
    mut recovered: impl FnMut(),
) -> Result<(), String> {
    let mut lease = None;
    let mut failed_preference = None;
    // The previous app run may have retained this condition. First successful
    // acquisition establishes recovery even without a failure in this run.
    let mut has_failure = true;
    loop {
        let snapshot = *control.state.lock().unwrap_or_else(|p| p.into_inner());
        if !snapshot.wanted() {
            drop(lease.take());
        } else if snapshot.should_acquire(lease.is_some(), failed_preference) {
            match acquire() {
                Ok(assertion) => {
                    lease = Some(assertion);
                    if has_failure {
                        recovered();
                        has_failure = false;
                    }
                }
                Err(error) => {
                    failed_preference = Some(snapshot.preference_revision);
                    has_failure = true;
                    report(&error)?;
                }
            }
        }
        if snapshot.stopped {
            return Ok(());
        }
        // Recheck after every external call. A disable/finish/shutdown during
        // acquisition must release immediately, not sleep on a lost wakeup.
        let state = control.state.lock().unwrap_or_else(|p| p.into_inner());
        drop(
            control
                .changed
                .wait_while(state, |current| current.revision == snapshot.revision)
                .unwrap_or_else(|p| p.into_inner()),
        );
    }
}

/// While enabled work runs, the computer is kept from idle sleep only: the
/// display may still dim and sleep, and an explicit sleep request (the lid,
/// the menu) is never overridden.
pub(crate) fn start(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let mut worker = WORKER
        .lock()
        .map_err(|_| "sleep-prevention worker state unavailable")?;
    if SERVICE.get().is_some() {
        return Err("sleep prevention was initialized more than once".to_string());
    }
    let control = Arc::new(Control::default());
    control.configure(enabled);
    SERVICE
        .set(Arc::clone(&control))
        .map_err(|_| "sleep prevention already initialized")?;
    let handle = app.clone();
    *worker = Some(crate::failure_runtime::spawn_reported(
        app,
        "onecopy-sleep-prevention",
        ISSUE,
        move || {
            run(
                &control,
                || {
                    keepawake::Builder::default()
                        .idle(true)
                        .display(false)
                        .sleep(false)
                        .app_name("OneCopy")
                        .reason("Preparing the OneCopy library")
                        .create()
                        .map_err(|error| format!("{error:#}"))
                },
                |error| crate::failure_runtime::report(&handle, ISSUE, None, error),
                || crate::failure_runtime::clear(ISSUE, None),
            )
        },
    )?);
    Ok(())
}

pub(crate) fn shutdown() {
    if let Some(control) = SERVICE.get() {
        control.update(|state| state.stopped = true);
    }
}

pub(crate) fn join() {
    let worker = WORKER.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(worker) = worker {
        if worker.join().is_err() {
            crate::logging::error("sleep-prevention worker join failed", serde_json::json!({}));
        }
    }
}

#[cfg(test)]
// The assertion factory and control are private process-lifetime ownership,
// not an app API. Test their thread-affinity without acquiring real OS locks.
#[path = "../tests/unit/sleep_prevention.rs"]
mod tests;
