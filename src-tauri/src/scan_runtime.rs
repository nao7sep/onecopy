//! Shared admission for index-changing work.
//!
//! One projection claim serializes every index writer: source checking,
//! file-information completion, watcher ingestion, foreground actions, and
//! automatic derived turns. Foreground admission is the single authority that
//! preempts every holder. It announces itself before it waits; from then on
//! file-information completion cancels, source checking and watcher
//! ingestion yield the claim in place at their next safe point, and automatic
//! derived work observes the pending admission through
//! `derived_runtime::cancelled`. A foreground caller never waits without a
//! deadline or a working cancel, and background owners wait behind it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::AppHandle;

static ACTIVE_OWNER: AtomicU8 = AtomicU8::new(0);
static FOREGROUND_WAITERS: AtomicUsize = AtomicUsize::new(0);
static WALK_EPOCH: AtomicU64 = AtomicU64::new(0);
const CLOSING: &str = "OneCopy is closing; no new library work can start.";
const WAIT_SLICE: Duration = Duration::from_millis(50);
/// How long a foreground action that answers busy waits for background work
/// to reach its safe point. File operations wait visibly without it.
pub(crate) const FOREGROUND_DEADLINE: Duration = Duration::from_secs(10);
pub(crate) const BUSY: &str =
    "OneCopy is still finishing background work. Try again in a moment.";
/// A yielded source walk whose index or configuration was replaced by the
/// foreground action it yielded to starts again from its first root. It
/// begins with `scanner::CANCELLED`, so the abandoned attempt is recorded as
/// cancelled rather than failed.
pub(crate) const RESTART: &str = "scan cancelled: the source walk restarts after a library reset";

#[derive(Default)]
struct IndexState {
    holder: Option<u64>,
    next_claim: u64,
}

static INDEX: LazyLock<(Mutex<IndexState>, Condvar)> =
    LazyLock::new(|| (Mutex::new(IndexState::default()), Condvar::new()));

fn index_state() -> MutexGuard<'static, IndexState> {
    match INDEX.0.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            crate::logging::error(
                "index admission state recovered after a panic",
                serde_json::json!({}),
            );
            INDEX.0.clear_poison();
            poisoned.into_inner()
        }
    }
}

fn take(state: &mut IndexState) -> Claim {
    state.next_claim = state.next_claim.wrapping_add(1);
    state.holder = Some(state.next_claim);
    Claim {
        id: state.next_claim,
    }
}

fn release(id: u64) {
    let mut state = index_state();
    if state.holder == Some(id) {
        state.holder = None;
        INDEX.1.notify_all();
    }
}

fn wait_slice(state: MutexGuard<'static, IndexState>, limit: Option<Instant>) {
    let slice = limit.map_or(WAIT_SLICE, |limit| {
        WAIT_SLICE.min(limit.saturating_duration_since(Instant::now()))
    });
    let _ = INDEX.1.wait_timeout(state, slice);
}

/// The projection claim. A yielding owner may have released it already; the
/// drop then leaves the current holder alone.
struct Claim {
    id: u64,
}

impl Drop for Claim {
    fn drop(&mut self) {
        release(self.id);
    }
}

struct ForegroundWait {
    app: Option<AppHandle>,
}

impl Drop for ForegroundWait {
    fn drop(&mut self) {
        FOREGROUND_WAITERS.fetch_sub(1, Ordering::SeqCst);
        drop(index_state());
        INDEX.1.notify_all();
        if let Some(app) = self.app.take() {
            crate::file_information_runtime::wake(app);
            crate::derived_work::wake_scheduler();
        }
    }
}

pub(crate) struct ForegroundGuard {
    active: Option<ActiveOwner>,
    claim: Option<Claim>,
    waiting: Option<ForegroundWait>,
}

impl Drop for ForegroundGuard {
    fn drop(&mut self) {
        self.active.take();
        self.claim.take();
        self.waiting.take();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    SourceCheck = 1,
    FileInformation = 2,
    Foreground = 3,
    Watcher = 4,
}

struct ActiveOwner {
    owner: Owner,
}

impl Drop for ActiveOwner {
    fn drop(&mut self) {
        // Retire the addressable owner before clearing its cancellation bit.
        // A concurrent stop then either targets this turn and is cleared here,
        // or observes no owner; it cannot strand cancellation for the next.
        // A yielded owner that never resumed no longer owns the bit.
        if ACTIVE_OWNER
            .compare_exchange(self.owner as u8, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
        }
    }
}

fn publish(owner: Owner, cancelled: &dyn Fn() -> bool) {
    // Sample on both sides of publication. Before ACTIVE_OWNER is visible the
    // owner-specific stop flag is authoritative; afterwards request_cancel
    // can target this turn directly. No stop can fall between the two owners.
    crate::scanner::SCAN_CANCEL.store(cancelled(), Ordering::SeqCst);
    ACTIVE_OWNER.store(owner as u8, Ordering::SeqCst);
    if cancelled() {
        crate::scanner::SCAN_CANCEL.store(true, Ordering::SeqCst);
    }
}

fn enter(owner: Owner, cancelled: &dyn Fn() -> bool) -> ActiveOwner {
    publish(owner, cancelled);
    ActiveOwner { owner }
}

pub(crate) fn request_cancel(owner: Owner) {
    if ACTIVE_OWNER.load(Ordering::SeqCst) == owner as u8 {
        crate::scanner::SCAN_CANCEL.store(true, Ordering::SeqCst);
    }
}

/// Cancels any admitted non-mutation foreground scan after the app-lifecycle
/// owner has closed new admission. Mutation work has its own bounded-step
/// cancellation and uses this claim only to serialize database publication.
pub(crate) fn shutdown() {
    request_cancel(Owner::Foreground);
}

/// How a background owner answers a pending foreground admission.
struct Yielder {
    owner: Owner,
    claim: u64,
    epoch: Option<u64>,
    cancelled: Rc<dyn Fn() -> bool>,
    on_yield: Rc<dyn Fn(bool)>,
}

thread_local! {
    static YIELDER: RefCell<Option<Yielder>> = const { RefCell::new(None) };
}

struct YielderScope {
    previous: Option<Yielder>,
}

impl Drop for YielderScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        YIELDER.with(|current| *current.borrow_mut() = previous);
    }
}

/// Waits behind the current holder and every pending foreground admission.
/// Returns `None` when the owner is retired or the app closes first.
fn claim_background(cancelled: &dyn Fn() -> bool) -> Option<Claim> {
    loop {
        if crate::app_lifecycle::shutting_down() || cancelled() {
            return None;
        }
        let mut state = index_state();
        if state.holder.is_none() && !foreground_pending() {
            return Some(take(&mut state));
        }
        wait_slice(state, None);
    }
}

fn run_owner<T>(
    owner: Owner,
    cancelled: Rc<dyn Fn() -> bool>,
    on_yield: Option<Rc<dyn Fn(bool)>>,
    restart_on_reset: bool,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let Some(claim) = claim_background(&*cancelled) else {
        return Err(crate::scanner::CANCELLED.to_string());
    };
    if crate::app_lifecycle::shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    let active = enter(owner, &*cancelled);
    if crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst) {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    let scope = on_yield.map(|on_yield| {
        let yielder = Yielder {
            owner,
            claim: claim.id,
            epoch: restart_on_reset.then(|| WALK_EPOCH.load(Ordering::SeqCst)),
            cancelled: cancelled.clone(),
            on_yield,
        };
        YielderScope {
            previous: YIELDER.with(|current| current.borrow_mut().replace(yielder)),
        }
    });
    let result = work();
    // Retire the owner before the claim so the next holder never publishes
    // beside a still-visible previous owner.
    drop(scope);
    drop(active);
    drop(claim);
    result
}

/// File-information completion: a foreground admission cancels its turn and
/// the owner requeues the remaining durable work.
pub(crate) fn with_owner<T>(
    owner: Owner,
    cancelled: impl Fn() -> bool + 'static,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    run_owner(owner, Rc::new(cancelled), None, false, work)
}

/// Source checking: a foreground admission parks the walk at its next safe
/// point and it continues from there afterwards. `on_yield` reports whether
/// the walk is currently waiting.
pub(crate) fn with_source_check_claim<T>(
    cancelled: impl Fn() -> bool + 'static,
    on_yield: impl Fn(bool) + 'static,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    run_owner(
        Owner::SourceCheck,
        Rc::new(cancelled),
        Some(Rc::new(on_yield)),
        true,
        work,
    )
}

pub(crate) fn with_watcher_claim<T>(
    retired: impl Fn() -> bool + 'static,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    run_owner(
        Owner::Watcher,
        Rc::new(retired),
        Some(Rc::new(|_| {})),
        false,
        work,
    )
}

/// A safe point for a yielding background owner. Outside such an owner, or
/// with no foreground admission pending, it returns `Ok(false)` at once.
/// Otherwise it releases the claim, waits until foreground work is done, takes
/// the claim back, and returns `Ok(true)`; a stop or exit while parked, or a
/// library reset for a source walk, ends the owner's turn instead.
pub(crate) fn yield_to_foreground() -> Result<bool, String> {
    if !foreground_pending() {
        return Ok(false);
    }
    let parked = YIELDER.with(|current| {
        current.borrow().as_ref().map(|yielder| {
            (
                yielder.owner,
                yielder.claim,
                yielder.epoch,
                yielder.cancelled.clone(),
                yielder.on_yield.clone(),
            )
        })
    });
    let Some((owner, claim, epoch, cancelled, on_yield)) = parked else {
        return Ok(false);
    };
    if ACTIVE_OWNER
        .compare_exchange(owner as u8, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        crate::scanner::SCAN_CANCEL.store(false, Ordering::SeqCst);
    }
    on_yield(true);
    release(claim);
    loop {
        if crate::app_lifecycle::shutting_down() || cancelled() {
            on_yield(false);
            return Err(crate::scanner::CANCELLED.to_string());
        }
        let mut state = index_state();
        if state.holder.is_none() && !foreground_pending() {
            state.holder = Some(claim);
            break;
        }
        wait_slice(state, None);
    }
    publish(owner, &*cancelled);
    on_yield(false);
    if epoch.is_some_and(|epoch| epoch != WALK_EPOCH.load(Ordering::SeqCst)) {
        return Err(RESTART.to_string());
    }
    if crate::scanner::SCAN_CANCEL.load(Ordering::SeqCst) {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    Ok(true)
}

/// Called inside a foreground action that replaces what a parked source walk
/// already recorded (index rebuild) or the configuration it walks with
/// (Settings apply). The parked walk starts again when it resumes.
pub(crate) fn restart_source_walks() {
    WALK_EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// Automatic derived work owns one bounded turn only when no index lifecycle
/// already owns or is waiting on the projection boundary. It observes a later
/// foreground admission through `derived_runtime::cancelled` and returns.
pub(crate) fn try_with_derived_claim<T>(work: impl FnOnce() -> T) -> Option<T> {
    if crate::app_lifecycle::shutting_down() {
        return None;
    }
    let _claim = {
        let mut state = index_state();
        if state.holder.is_some() || foreground_pending() {
            return None;
        }
        take(&mut state)
    };
    if crate::app_lifecycle::shutting_down()
        || crate::source_check_runtime::running()
        || crate::file_information_runtime::running()
        || ACTIVE_OWNER.load(Ordering::SeqCst) != 0
    {
        return None;
    }
    Some(work())
}

#[derive(Debug)]
pub(crate) enum Refusal {
    Busy,
    Cancelled,
    Closing,
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Self::Busy => BUSY.to_string(),
            Self::Cancelled => crate::scanner::CANCELLED.to_string(),
            Self::Closing => CLOSING.to_string(),
        }
    }
}

/// Announces a foreground request, preempts every holder, and waits for the
/// claim until `deadline` or `cancelled`. `on_wait` runs once, only when the
/// claim is not immediately free, so the caller can show that it is waiting.
pub(crate) fn admit_foreground(
    app: Option<&AppHandle>,
    deadline: Option<Instant>,
    cancelled: &dyn Fn() -> bool,
    on_wait: &mut dyn FnMut(),
) -> Result<ForegroundGuard, Refusal> {
    FOREGROUND_WAITERS.fetch_add(1, Ordering::SeqCst);
    let waiting = ForegroundWait { app: app.cloned() };
    crate::file_information_runtime::preempt();
    crate::derived_runtime::preempt_automatic();
    let mut announced = false;
    loop {
        if crate::app_lifecycle::shutting_down() {
            return Err(Refusal::Closing);
        }
        if cancelled() {
            return Err(Refusal::Cancelled);
        }
        let mut state = index_state();
        if state.holder.is_none() {
            let claim = take(&mut state);
            drop(state);
            let active = enter(Owner::Foreground, &|| false);
            return Ok(ForegroundGuard {
                active: Some(active),
                claim: Some(claim),
                waiting: Some(waiting),
            });
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(Refusal::Busy);
        }
        if !announced {
            announced = true;
            drop(state);
            on_wait();
            continue;
        }
        wait_slice(state, deadline);
    }
}

/// Section recheck is refused outright while the whole library is being
/// checked or a file operation owns the index; neither is queued invisibly.
pub(crate) fn section_admission() -> Result<(), String> {
    if crate::source_check_runtime::running() {
        return Err(
            "Recheck this section is unavailable while OneCopy is checking all source folders."
                .to_string(),
        );
    }
    if crate::mutation_runtime::active() {
        return Err(
            "Recheck this section is unavailable while a file operation is running.".to_string(),
        );
    }
    Ok(())
}

pub(crate) fn run_section<T>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    section_admission()?;
    run_foreground(app, work)
}

/// Runs a foreground action that answers busy when background work does not
/// reach its safe point within [`FOREGROUND_DEADLINE`].
pub(crate) fn run_foreground<T>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if crate::app_lifecycle::shutting_down() {
        return Err(CLOSING.to_string());
    }
    let _foreground = admit_foreground(
        Some(app),
        Some(Instant::now() + FOREGROUND_DEADLINE),
        &|| false,
        &mut || {},
    )
    .map_err(|refusal| refusal.message())?;
    if crate::app_lifecycle::shutting_down() {
        return Err(CLOSING.to_string());
    }
    work()
}

/// Enters the projection boundary for a mutation that already owns the
/// mutation-runtime claim. It waits as long as background work needs to reach
/// its safe point, calling `on_wait` once so the operation can show that, and
/// returns `Ok(None)` when the operation is cancelled first. Final shutdown
/// may cancel that operation between files, but it must retain this boundary
/// long enough to publish the current bounded filesystem step honestly.
pub(crate) fn begin_admitted_mutation(
    app: &AppHandle,
    cancelled: &dyn Fn() -> bool,
    on_wait: &mut dyn FnMut(),
) -> Result<Option<ForegroundGuard>, String> {
    match admit_foreground(Some(app), None, cancelled, on_wait) {
        Ok(guard) => Ok(Some(guard)),
        Err(Refusal::Cancelled) => Ok(None),
        Err(refusal) => Err(refusal.message()),
    }
}

pub(crate) fn foreground_pending() -> bool {
    FOREGROUND_WAITERS.load(Ordering::SeqCst) != 0
}

pub fn running() -> bool {
    crate::source_check_runtime::running()
        || crate::file_information_runtime::running()
        || ACTIVE_OWNER.load(Ordering::SeqCst) != 0
}

/// Serializes unit tests that drive the process-wide admission, derived-work
/// and mutation state.
#[cfg(test)]
pub(crate) static RUNTIME_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) fn serial_test() -> MutexGuard<'static, ()> {
    RUNTIME_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// EXCEPTION to tests-folder conventions: exercises the private scan owner
// (`ACTIVE_OWNER`, `with_owner`, `admit_foreground`); promoting it would
// widen the crate's API only for this test.
#[cfg(test)]
#[path = "../tests/unit/scan_runtime.rs"]
mod admission_tests;

pub(crate) fn progress_emitter(
    handle: AppHandle,
    event: &'static str,
    next_sequence: fn() -> u64,
) -> impl Fn(crate::scanner::ScanProgress) {
    let last_phase = Cell::new(None::<crate::scanner::ScanPhase>);
    let last_emit = Cell::new(Instant::now() - Duration::from_secs(1));
    move |progress: crate::scanner::ScanProgress| {
        let now = Instant::now();
        let phase_changed = last_phase.get() != Some(progress.phase);
        let completed = progress.done == progress.total;
        if phase_changed
            || completed
            || now.duration_since(last_emit.get()) >= Duration::from_millis(125)
        {
            last_phase.set(Some(progress.phase));
            last_emit.set(now);
            crate::failure_runtime::emit_or_record(
                &handle,
                event,
                serde_json::json!({
                    "eventSequence": next_sequence(),
                    "progress": progress,
                }),
            );
        }
    }
}

pub(crate) fn record_runtime_failure(app: &AppHandle, kind: &str, message: &str) {
    let _ = crate::failure_runtime::report(app, kind, None, message);
}
