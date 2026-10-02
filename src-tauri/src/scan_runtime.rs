//! Shared admission for index-changing work.
//!
//! One projection claim serializes every index owner: source checking,
//! file-information completion, watcher ingestion, and foreground actions.
//! Automatic derived work holds a share instead: the preview lane and the
//! heavy lane each hold one and run beside each other, and an exclusive claim
//! waits until every share is released.
//!
//! This module is the one authority that ranks waiting work, from the top:
//!
//! 1. Foreground admission preempts every holder. It announces itself before
//!    it waits; from then on file-information completion cancels, source
//!    checking and watcher ingestion yield the claim in place at their next
//!    safe point, and automatic derived work observes it through
//!    `derived_runtime::cancelled`. A foreground caller never waits without a
//!    deadline or a working cancel.
//! 2. An urgent share, for required preparation of the selected, visible and
//!    nearby items, preempts background index owners the same way at their
//!    safe points, so that preparation interleaves with long index upkeep.
//! 3. A waiting background index owner preempts ordinary shares, the rest of
//!    automatic derived work, at their safe points.
//! 4. Ordinary shares take whatever the index leaves free.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::AppHandle;

static ACTIVE_OWNER: AtomicU8 = AtomicU8::new(0);
static FOREGROUND_WAITERS: AtomicUsize = AtomicUsize::new(0);
static BACKGROUND_WAITERS: AtomicUsize = AtomicUsize::new(0);
static URGENT_WAITERS: AtomicUsize = AtomicUsize::new(0);
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
    /// A background owner parked in place for a foreground action. It takes
    /// the claim back before any other background owner, so nothing else
    /// writes between its walked prefix and the rest of its turn.
    parked: Option<u64>,
    next_claim: u64,
    /// Automatic derived shares currently held.
    shares: usize,
}

impl IndexState {
    fn free_for_background(&self) -> bool {
        self.holder.is_none()
            && self.parked.is_none()
            && self.shares == 0
            && !foreground_pending()
            && !urgent_pending()
    }

    /// A parked owner takes its claim back before any other background
    /// owner, once foreground work and urgent preparation are done.
    fn free_for_parked(&self) -> bool {
        self.holder.is_none() && self.shares == 0 && !foreground_pending() && !urgent_pending()
    }

    fn free_for_share(&self, rank: ShareRank) -> bool {
        self.holder.is_none()
            && !foreground_pending()
            && match rank {
                ShareRank::Urgent => true,
                ShareRank::Ordinary => self.parked.is_none() && !background_pending(),
            }
    }
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

/// Counts one announced waiter for as long as it waits or holds what it
/// waited for.
struct Waiting(&'static AtomicUsize);

impl Waiting {
    fn announce(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
        drop(index_state());
        INDEX.1.notify_all();
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

/// Waits behind the current holder, every pending foreground admission and
/// urgent share, a parked owner, and every share. While it waits, ordinary
/// shares yield to it at their safe points. Returns `None` when the owner is
/// retired or the app closes first.
fn claim_background(cancelled: &dyn Fn() -> bool) -> Option<Claim> {
    let mut waiting = None;
    loop {
        if crate::app_lifecycle::shutting_down() || cancelled() {
            return None;
        }
        let mut state = index_state();
        if state.free_for_background() {
            let claim = take(&mut state);
            drop(state);
            drop(waiting);
            return Some(claim);
        }
        if waiting.is_none() {
            drop(state);
            waiting = Some(Waiting::announce(&BACKGROUND_WAITERS));
            // Interrupt a native engine at once instead of at its next poll.
            crate::derived_runtime::preempt_automatic();
            continue;
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
    crate::derived_work::wake_scheduler();
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
/// with neither a foreground admission nor an urgent share waiting, it
/// returns `Ok(false)` at once. Otherwise it releases the claim, waits until
/// that work is done, takes the claim back, and returns `Ok(true)`; a stop or
/// exit while parked, or a library reset for a source walk, ends the owner's
/// turn instead. A parked owner is idle, so it releases its keep-awake
/// assertion while it waits.
pub(crate) fn yield_at_safe_point() -> Result<bool, String> {
    if !foreground_pending() && !urgent_pending() {
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
    let idle = crate::sleep_prevention::idle_while_parked();
    {
        let mut state = index_state();
        if state.holder == Some(claim) {
            state.holder = None;
            state.parked = Some(claim);
            INDEX.1.notify_all();
        }
    }
    loop {
        if crate::app_lifecycle::shutting_down() || cancelled() {
            let mut state = index_state();
            if state.parked == Some(claim) {
                state.parked = None;
                INDEX.1.notify_all();
            }
            drop(state);
            on_yield(false);
            return Err(crate::scanner::CANCELLED.to_string());
        }
        let mut state = index_state();
        if state.free_for_parked() {
            state.holder = Some(claim);
            state.parked = None;
            break;
        }
        wait_slice(state, None);
    }
    drop(idle);
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

/// How an automatic derived share ranks against index owners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShareRank {
    /// Required preparation for the selected, visible and nearby items.
    Urgent,
    /// Every other automatic derived turn.
    Ordinary,
}

thread_local! {
    static SHARE: Cell<Option<ShareRank>> = const { Cell::new(None) };
}

struct Share {
    previous: Option<ShareRank>,
}

impl Drop for Share {
    fn drop(&mut self) {
        SHARE.with(|share| share.set(self.previous));
        let mut state = index_state();
        state.shares -= 1;
        INDEX.1.notify_all();
    }
}

fn take_share(state: &mut IndexState, rank: ShareRank) -> Share {
    state.shares += 1;
    Share {
        previous: SHARE.with(|share| share.replace(Some(rank))),
    }
}

/// Whether automatic derived work may be admitted at all: never while the app
/// exits or a foreground action waits. The share admission below and derived
/// work's own availability both read this one answer.
pub(crate) fn derived_work_admissible() -> bool {
    !crate::app_lifecycle::shutting_down() && !foreground_pending()
}

/// Whether an ordinary automatic derived turn would be admitted now — what
/// Background Work reports as "held back by index work".
pub(crate) fn ordinary_share_free() -> bool {
    derived_work_admissible() && index_state().free_for_share(ShareRank::Ordinary)
}

/// Runs one automatic derived turn under a share. An ordinary share is taken
/// only when the index is free and no background owner waits; otherwise it
/// declines at once with `None`. An urgent share announces itself, lets a
/// background index owner reach its safe point, and waits for it; it declines
/// only for foreground work or exit. Either observes later preemption through
/// `derived_runtime::cancelled`.
pub(crate) fn with_derived_share<T>(rank: ShareRank, work: impl FnOnce() -> T) -> Option<T> {
    let mut waiting = None;
    let _share = loop {
        if !derived_work_admissible() {
            return None;
        }
        let mut state = index_state();
        if state.free_for_share(rank) {
            break take_share(&mut state, rank);
        }
        if rank == ShareRank::Ordinary {
            return None;
        }
        if waiting.is_none() {
            waiting = Some(Waiting::announce(&URGENT_WAITERS));
        }
        // Source checking and watcher ingestion park in place at their safe
        // point; completion has no place to park, so it ends its turn there
        // and requeues the rest, as it does for foreground work.
        if ACTIVE_OWNER.load(Ordering::SeqCst) == Owner::FileInformation as u8 {
            crate::file_information_runtime::preempt();
        }
        wait_slice(state, None);
    };
    drop(waiting);
    Some(work())
}

/// Whether automatic derived work admitted on this thread holds an urgent
/// share.
pub(crate) fn urgent_share() -> bool {
    SHARE.with(Cell::get) == Some(ShareRank::Urgent)
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
        if state.holder.is_none() && state.shares == 0 {
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
    try_foreground(app, work)?.ok_or_else(|| BUSY.to_string())
}

/// Like [`run_foreground`], but a busy refusal is `Ok(None)` for a caller
/// that leaves the work owed rather than reporting it.
pub(crate) fn try_foreground<T>(
    app: &AppHandle,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    if crate::app_lifecycle::shutting_down() {
        return Err(CLOSING.to_string());
    }
    let _foreground = match admit_foreground(
        Some(app),
        Some(Instant::now() + FOREGROUND_DEADLINE),
        &|| false,
        &mut || {},
    ) {
        Ok(guard) => guard,
        Err(Refusal::Busy) => return Ok(None),
        Err(refusal) => return Err(refusal.message()),
    };
    if crate::app_lifecycle::shutting_down() {
        return Err(CLOSING.to_string());
    }
    work().map(Some)
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

/// A background index owner waits for the claim; ordinary shares yield to
/// it at their safe points.
pub(crate) fn background_pending() -> bool {
    BACKGROUND_WAITERS.load(Ordering::SeqCst) != 0
}

fn urgent_pending() -> bool {
    URGENT_WAITERS.load(Ordering::SeqCst) != 0
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
    let throttle = std::cell::RefCell::new(
        crate::progress_throttle::ProgressThrottle::<crate::scanner::ScanPhase>::default(),
    );
    move |progress: crate::scanner::ScanProgress| {
        if throttle.borrow_mut().admit(
            progress.phase,
            progress.done == progress.total,
            Instant::now(),
        ) {
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

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum LibrarySettingsOutcome {
    Applied { resolved: u64 },
    Owed,
}

/// Publishes Settings-owned index projections. Visibility uses saved facts;
/// only a changed date/pairing policy recomputes those projections. The
/// derived-work coordinator remains the sole owner of preparation and
/// enrichment. Busy: the saved settings stay owed in the index, and releasing
/// the admission wakes the file-information owner that applies them.
pub(crate) fn apply_library_settings(app: &AppHandle) -> Result<LibrarySettingsOutcome, String> {
    let applied = try_foreground(app, || {
        // A source walk parked behind this apply walks again with the saved
        // configuration instead of finishing with the old one.
        restart_source_walks();
        let data_root = crate::paths::data_root()?;
        let config = crate::storage::config(&data_root)?;
        let settings = crate::scanner::settings_from_config(
            Some(&config),
            &data_root,
            chrono::Utc::now().timestamp_millis(),
        );
        let visibility = crate::visibility::Policy::from_config(&config)?;
        let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
        let resolved = crate::library_settings::apply(&conn, &settings, &visibility, &|_| {})?;
        crate::derived_work::wake();
        Ok(resolved)
    })?;
    Ok(match applied {
        Some(resolved) => LibrarySettingsOutcome::Applied { resolved },
        None => LibrarySettingsOutcome::Owed,
    })
}

#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum RescanSectionOutcome {
    Completed { changed: u64 },
    Cancelled,
}

impl RescanSectionOutcome {
    pub(crate) fn log_fields(&self) -> serde_json::Value {
        match self {
            Self::Completed { changed } => {
                serde_json::json!({ "status": "completed", "changed": changed })
            }
            Self::Cancelled => serde_json::json!({ "status": "cancelled" }),
        }
    }
}

/// Scoped rescan: re-stats exactly the directories that contributed files to
/// one section (never the whole roots), then runs the pending pipeline tail.
/// The full per-root walk remains the Scan button's escape hatch.
pub(crate) fn recheck_section(
    app: &AppHandle,
    kind: crate::queries::SectionKind,
    month: &str,
    timezone: chrono_tz::Tz,
) -> Result<RescanSectionOutcome, String> {
    let result = run_section(app, || {
        let data_root = crate::paths::data_root()?;
        let config = crate::storage::config(&data_root)?;
        let settings = crate::scanner::settings_from_config(
            Some(&config),
            &data_root,
            chrono::Utc::now().timestamp_millis(),
        );
        let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
        let dirs = crate::queries::section_dirs(&conn, kind, month, timezone)?;
        let bounds = crate::queries::month_bounds(month, timezone)?;
        let reopened = crate::attempt_boundaries::recheck_section(&conn, kind, bounds)?;
        if reopened > 0 {
            // The index claim prevents automatic execution until this
            // admitted recheck releases it, even if later stat fails.
            crate::derived_work::wake();
        }
        crate::scanner::with_scoped_index_repair(&conn, &dirs, || {
            let mut changed = 0u64;
            for dir in &dirs {
                changed += crate::watcher::restat_dir(
                    &conn,
                    std::path::Path::new(dir),
                    &settings.lists,
                    &settings.source_dirs,
                    &data_root,
                )?;
            }
            // Finish any interrupted index checkpoints too. Derived media is
            // woken after the index tail instead of being smuggled into the
            // rescan.
            if changed > 0 || crate::scanner::pending_index_work_exists(&conn)? {
                let mut summary = crate::scanner::ScanSummary::default();
                crate::scanner::run_index_tail_for_dirs(
                    &conn,
                    &settings,
                    &dirs,
                    &|_| {},
                    &mut summary,
                )?;
                crate::derived_work::wake();
            }
            Ok(changed)
        })
    });
    match result {
        Ok(changed) => Ok(RescanSectionOutcome::Completed { changed }),
        Err(error) if error == crate::scanner::CANCELLED => Ok(RescanSectionOutcome::Cancelled),
        Err(error) => Err(error),
    }
}

pub(crate) fn record_runtime_failure(app: &AppHandle, kind: &str, message: &str) {
    let _ = crate::failure_runtime::report(app, kind, None, message);
}
