//! Process-lifetime admission authority and the normal-exit sequence.
//!
//! Worker modules retain ownership of their cancellation, handles, and joins.
//! This module owns the irreversible transition that tells every owner no new
//! work may be admitted once final shutdown begins, and the one sequence that
//! asks every owner to stop, waits for them, and exits the process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter};

struct Lifecycle {
    shutting_down: AtomicBool,
    publication: Mutex<()>,
}

impl Lifecycle {
    const fn new() -> Self {
        Self {
            shutting_down: AtomicBool::new(false),
            publication: Mutex::new(()),
        }
    }

    fn begin_shutdown(&self) -> bool {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !self.shutting_down.swap(true, Ordering::SeqCst)
    }

    fn shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    fn publish_if_running<T>(&self, publish: impl FnOnce() -> T) -> Option<T> {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down() {
            return None;
        }
        Some(publish())
    }
}

static APP: Lifecycle = Lifecycle::new();

/// Closes process-wide work admission. The first caller owns shutdown setup;
/// later Tauri exit events observe the same irreversible state.
pub(crate) fn begin_shutdown() -> bool {
    APP.begin_shutdown()
}

pub(crate) fn shutting_down() -> bool {
    APP.shutting_down()
}

/// Linearizes runtime-to-webview publication with final shutdown. A publish
/// that owns this permit finishes before shutdown begins; once shutdown owns
/// it, no later runtime event reaches a tearing-down webview. The deliberately
/// final `app://exit-quiescing` and shutdown media-release events are emitted
/// directly by the exit owner.
pub(crate) fn publish_if_running<T>(publish: impl FnOnce() -> T) -> Option<T> {
    APP.publish_if_running(publish)
}

/// Bounds only the non-mutation exit joins (derived work, requested media,
/// watchers, source check, binaries, startup, instance owner). Mutation
/// quiescence has its own, longer deadline (`MUTATION_QUIESCE_DEADLINE`).
const EXIT_JOIN_DEADLINE: Duration = Duration::from_secs(10);

/// How long normal exit waits for the current file operation to reach its own
/// safe point before giving up on it (`specs/file-operations.md`, "Normal exit
/// and abnormal termination"). Giving up kills outstanding subprocesses and
/// exits; the operation's unpublished private output is never at the file's
/// final name, and it is removed at the next launch's discovery pass.
const MUTATION_QUIESCE_DEADLINE: Duration = Duration::from_secs(30);

// The normal budget retains both existing waits and gives optional cleanup
// two seconds. The watchdog includes shutdown setup and the Tauri Exit tail;
// no diagnostic or cleanup can prevent it from exiting the process.
const EXIT_TOTAL_DEADLINE: Duration = Duration::from_secs(42);
pub(crate) const SESSION_EXIT_DEADLINE: Duration = Duration::from_secs(5);
const BACKUP_DRAIN_DEADLINE: Duration = Duration::from_millis(500);

struct ExitBudget {
    deadline: Mutex<Option<Instant>>,
    changed: Condvar,
}

impl ExitBudget {
    const fn new() -> Self {
        Self { deadline: Mutex::new(None), changed: Condvar::new() }
    }

    fn arm(&self, budget: Duration) -> bool {
        let mut deadline = self.deadline.lock().unwrap_or_else(|p| p.into_inner());
        let first = deadline.is_none();
        let proposed = Instant::now() + budget;
        *deadline = Some(deadline.map_or(proposed, |existing| existing.min(proposed)));
        self.changed.notify_all();
        first
    }

    fn wait(&self) {
        let mut deadline = self.deadline.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            let remaining = deadline.expect("armed exit budget").saturating_duration_since(Instant::now());
            if remaining.is_zero() { return; }
            (deadline, _) = self.changed.wait_timeout(deadline, remaining).unwrap_or_else(|p| p.into_inner());
        }
    }
}

static EXIT_BUDGET: ExitBudget = ExitBudget::new();

pub(crate) fn start_exit_deadline(budget: Duration) {
    if !EXIT_BUDGET.arm(budget) { return; }
    let started = spawn_thread("onecopy-exit-deadline", Box::new(|| {
        EXIT_BUDGET.wait();
        crate::subprocess::signal_all_running_for_exit();
        // Deliberately independent of the event loop, logs, and stalled I/O.
        std::process::exit(0);
    }));
    if started.is_err() {
        crate::subprocess::signal_all_running_for_exit();
        std::process::exit(0);
    }
}

/// Set once the exit sequence has reached the point where exiting is safe;
/// only then may an exit request close the process.
static EXIT_READY: AtomicBool = AtomicBool::new(false);

/// Whether an exit request may close the process now. Every earlier request,
/// including a repeated quit while the sequence waits, is prevented, so no
/// second request forces exit past mutation quiescence.
pub(crate) fn exit_ready() -> bool {
    EXIT_READY.load(Ordering::SeqCst)
}

/// Starts the normal-exit sequence once; later requests join the one already
/// running. The event loop is never blocked unless no thread can be started,
/// and then the sequence still finishes and quits on this thread.
pub(crate) fn quiesce(app: &AppHandle) {
    start_exit_deadline(EXIT_TOTAL_DEADLINE);
    if !begin_shutdown() {
        return;
    }
    if let Err(error) = app.emit("app://exit-quiescing", ()) {
        crate::logging::warn(
            "exit wait state delivery failed",
            json!({ "error": { "message": error.to_string() } }),
        );
    }
    request_worker_shutdown(app);
    let joins = app.clone();
    let media = app.clone();
    let reporter = app.clone();
    run_exit(
        ExitSequence {
            join_workers: Box::new(move || join_workers(&joins)),
            wait_for_mutation: Box::new(|| {
                crate::mutation_runtime::wait_for_idle(MUTATION_QUIESCE_DEADLINE)
            }),
            exit: Box::new(move |clean| {
                let released = crate::media_use::begin_shutdown(&media);
                if let Err(error) = &released {
                    let _ = crate::failure_runtime::report(
                        &media,
                        "shutdown-media-release-failed",
                        None,
                        error,
                    );
                }
                drop(released);
                crate::activity::record_shutdown();
                crate::logging::flush(std::time::Duration::from_secs(2));
                // Pending history writes get a short bound at ordinary quit;
                // the OS ending the session skips them (data-backup conventions).
                if !crate::quit::session_ending() {
                    crate::backup_store::drain(BACKUP_DRAIN_DEADLINE);
                }
                if clean {
                    if let Ok(cache_root) = crate::paths::cache_root() {
                        crate::preview::clear_session_renders(&crate::preview::CachePaths::new(cache_root));
                    }
                }
                EXIT_READY.store(true, Ordering::SeqCst);
                crate::quit::finish_session_end(&media);
                media.exit(0);
            }),
            report: Arc::new(move |error: String| {
                let _ = crate::failure_runtime::report(
                    &reporter,
                    "shutdown-worker-failed",
                    None,
                    &error,
                );
            }),
        },
        spawn_thread,
        EXIT_JOIN_DEADLINE,
    );
}

/// Asks every worker owner to stop admitting and to cancel its current work.
fn request_worker_shutdown(app: &AppHandle) {
    crate::source_check_runtime::shutdown();
    crate::sleep_prevention::shutdown();
    crate::file_information_runtime::shutdown();
    crate::watcher::shutdown();
    crate::scan_runtime::shutdown();
    crate::startup::shutdown();
    crate::instance_owner::shutdown(app);
    crate::binaries_manager::begin_shutdown();
    crate::derived_work::shutdown(app);
    if let Err(error) = crate::mutation_runtime::request_shutdown() {
        let _ = crate::failure_runtime::report(app, "shutdown-worker-failed", None, &error);
    }
}

/// Joins every non-mutation worker; the exit sequence bounds this wait.
fn join_workers(app: &AppHandle) -> bool {
    let joined = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::source_check_runtime::join();
        crate::sleep_prevention::join();
        crate::file_information_runtime::join();
        crate::watcher::join();
        crate::binaries_manager::wait_for_idle();
        crate::startup::join();
        crate::instance_owner::join(app);
        crate::derived_work::join();
    }));
    if let Err(payload) = joined {
        let error = crate::failure_runtime::panic_message(payload);
        let _ = crate::failure_runtime::report(app, "shutdown-worker-failed", None, &error);
        return false;
    }
    true
}

/// The steps after admission closes, as seams the sequence runs in order.
struct ExitSequence {
    join_workers: Box<dyn FnOnce() -> bool + Send>,
    wait_for_mutation: Box<dyn FnOnce() -> Result<crate::mutation_runtime::IdleWait, String> + Send>,
    exit: Box<dyn FnOnce(bool) + Send>,
    report: Arc<dyn Fn(String) + Send + Sync>,
}

type Spawn = fn(&'static str, Box<dyn FnOnce() + Send>) -> std::io::Result<()>;

fn spawn_thread(name: &'static str, work: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(work)
        .map(|_| ())
}

fn take_sequence(slot: &Mutex<Option<ExitSequence>>) -> Option<ExitSequence> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Runs the sequence on its own thread. When that thread cannot start, the
/// failure is reported and the sequence runs on the caller's thread instead,
/// so quitting still completes and never before mutation quiescence.
fn run_exit(sequence: ExitSequence, spawn: Spawn, deadline: Duration) {
    let slot = Arc::new(Mutex::new(Some(sequence)));
    let owned = slot.clone();
    let started = spawn(
        "onecopy-exit",
        Box::new(move || {
            if let Some(sequence) = take_sequence(&owned) {
                finish_exit(sequence, spawn, deadline);
            }
        }),
    );
    if let Err(error) = started {
        if let Some(sequence) = take_sequence(&slot) {
            (sequence.report)(format!("could not start the exit thread: {error}"));
            finish_exit(sequence, spawn, deadline);
        }
    }
}

/// Waits for the non-mutation joins up to `deadline`, then for mutation
/// quiescence up to its own `MUTATION_QUIESCE_DEADLINE`, then exits. A join or
/// a file operation that never reaches its own safe point is abandoned at its
/// deadline, once, with outstanding subprocesses killed either way.
fn finish_exit(sequence: ExitSequence, spawn: Spawn, deadline: Duration) {
    let ExitSequence {
        join_workers,
        wait_for_mutation,
        exit,
        report,
    } = sequence;
    let (done_tx, done_rx) = mpsc::channel::<bool>();
    let joins = spawn(
        "onecopy-exit-joins",
        Box::new(move || {
            let joined = join_workers();
            let _ = done_tx.send(joined);
        }),
    );
    let joined = match joins {
        Ok(()) => await_other_joins_with_deadline(done_rx, deadline),
        Err(error) => {
            report(format!("could not start the exit joins: {error}"));
            crate::subprocess::kill_all_running();
            false
        }
    };
    let idle = match wait_for_mutation() {
        Ok(crate::mutation_runtime::IdleWait::Idle) => true,
        Ok(crate::mutation_runtime::IdleWait::TimedOut) => {
            crate::logging::warn(
                "mutation quiescence exceeded its deadline; giving up on the current file operation",
                json!({ "deadline_secs": MUTATION_QUIESCE_DEADLINE.as_secs() }),
            );
            crate::subprocess::kill_all_running();
            false
        }
        Err(error) => { report(error); false },
    };
    exit(joined && idle);
}

/// Waits up to `deadline` for the non-mutation exit joins signalled on `rx`.
/// If the deadline passes first, kills every managed-tool subprocess still
/// running (W-L1) and stops waiting for those joins. Returns whether the
/// joins finished in time.
fn await_other_joins_with_deadline(rx: mpsc::Receiver<bool>, deadline: Duration) -> bool {
    if let Ok(joined) = rx.recv_timeout(deadline) {
        return joined;
    }
    crate::logging::warn(
        "exit joins exceeded their deadline; killing outstanding subprocesses",
        json!({ "deadline_secs": deadline.as_secs() }),
    );
    crate::subprocess::kill_all_running();
    false
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private exit sequence
// and its spawn seam; promoting them would widen the crate's API only for
// these tests.
#[path = "../tests/unit/lib/exit_quiescence_tests.rs"]
mod exit_quiescence_tests;

// This process singleton is intentionally private application plumbing. A
// separate Lifecycle proves its transition without poisoning the shared test
// process or widening the shipped crate's public API solely for a test.
#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the `Lifecycle` state
// machine of a module that is private to the crate.
#[path = "../tests/unit/app_lifecycle.rs"]
mod tests;
