//! Ephemeral ownership and lifecycle for fixed derived-work classes. Durable
//! output state stays in `derived_state`; dispatch policy and cursors stay in
//! `derived_work`. This runtime has no dependency on either dispatcher or
//! projection.
//!
//! Work runs in two lanes. Preview preparation (thumbnails, screen previews,
//! full-resolution images), automatic or requested, has its own
//! capacity-gated lane; every other class (snapshots, similarity, faces,
//! transcription) shares one heavy lane. So a transcription never stops
//! preview work and preview work never waits behind one.
//!
//! Each running job carries its own stop request. Automatic jobs run inside
//! a `scan_runtime` share and also stop when that share is preempted: every
//! share by a pending foreground admission, and an ordinary share by a
//! waiting background index owner. Requested jobs hold no share, so neither
//! ever stops them. `cancelled` answers for the job owned by the calling thread; a
//! helper thread that polls for a job carries it with
//! [`on_behalf_of_current_job`].

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::AppHandle;

use crate::derived_state::WorkClass;

/// How long a requested preview or an external open waits for its lane or
/// for automatic work on the same file to stop. Reused at exit (W-L2/exit
/// joins) as the bound on how long quitting waits for derived work, mutation
/// and requested-media joins before killing any subprocess still running and
/// exiting anyway — the same "how long is reasonable to make the user wait"
/// judgment already made for foreground preemption.
pub(crate) const REQUESTED_WAIT: Duration = Duration::from_secs(10);
const WAIT_SLICE: Duration = Duration::from_millis(50);
const STATE_UNAVAILABLE: &str = "background-work state is unavailable";
const FILE_IN_USE: &str = "A file operation is using this file.";

#[derive(Clone)]
struct Job {
    id: u64,
    class: WorkClass,
    manual: bool,
    /// Admitted under an urgent share: a waiting background index owner does
    /// not stop it.
    urgent: bool,
    hash: Option<String>,
    stop: bool,
    done: Option<u64>,
    total: Option<u64>,
}

fn lane_is_previews(class: WorkClass) -> bool {
    class == WorkClass::Previews
}

/// Which jobs an exclusive claim stops, and which new admissions it blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimPolicy {
    /// A file operation or rebuild: every job on its files, requested or not.
    Mutation,
    /// Open in Default App: only automatic work on the same file.
    ExternalOpen,
}

impl ClaimPolicy {
    fn stops(self, job: &Job) -> bool {
        self == Self::Mutation || !job.manual
    }
}

struct Claim {
    /// Empty means every item.
    keys: Vec<String>,
}

impl Claim {
    fn covers(&self, hash: Option<&str>) -> bool {
        self.keys.is_empty()
            || hash.is_some_and(|hash| self.keys.iter().any(|key| key == hash))
    }
}

#[derive(Default)]
struct RuntimeState {
    paused_classes: u8,
    heavy: Option<Job>,
    previews: Vec<Job>,
    claim: Option<Claim>,
    next_job: u64,
    next_manual_ticket: u64,
    serving_manual_ticket: u64,
    preview_queue: VecDeque<u64>,
    next_preview_ticket: u64,
}

impl RuntimeState {
    fn paused(&self, class: WorkClass) -> bool {
        self.paused_classes & class.bit() != 0
    }

    fn jobs(&self) -> impl Iterator<Item = &Job> {
        self.heavy.iter().chain(self.previews.iter())
    }

    fn jobs_mut(&mut self) -> impl Iterator<Item = &mut Job> {
        self.heavy.iter_mut().chain(self.previews.iter_mut())
    }

    fn job(&self, id: u64) -> Option<&Job> {
        self.jobs().find(|job| job.id == id)
    }

    fn manual_heavy_queued(&self) -> bool {
        self.next_manual_ticket != self.serving_manual_ticket
    }

    fn claim_covers(&self, hash: Option<&str>) -> bool {
        self.claim.as_ref().is_some_and(|claim| claim.covers(hash))
    }

    fn insert(&mut self, class: WorkClass, manual: bool, urgent: bool, hash: Option<String>) -> u64 {
        self.next_job = self.next_job.wrapping_add(1);
        let job = Job {
            id: self.next_job,
            class,
            manual,
            urgent,
            hash,
            stop: false,
            done: None,
            total: None,
        };
        if lane_is_previews(class) {
            self.previews.push(job);
        } else {
            self.heavy = Some(job);
        }
        self.next_job
    }

    fn remove(&mut self, id: u64) -> bool {
        if self.heavy.as_ref().is_some_and(|job| job.id == id) {
            self.heavy = None;
            return true;
        }
        let before = self.previews.len();
        self.previews.retain(|job| job.id != id);
        before != self.previews.len()
    }

    /// The job the Background Work surface shows: the heavy lane's, else the
    /// first preview job.
    fn display_job(&self) -> Option<&Job> {
        self.heavy.as_ref().or(self.previews.first())
    }

    /// The job owned by the calling thread.
    fn current_job(&self) -> Option<&Job> {
        CURRENT_JOB.with(Cell::get).and_then(|id| self.job(id))
    }

    fn current_job_mut(&mut self, class: WorkClass) -> Option<&mut Job> {
        let current = CURRENT_JOB.with(Cell::get);
        let id = current
            .and_then(|id| self.job(id))
            .filter(|job| job.class == class)
            .or_else(|| self.jobs().find(|job| job.class == class))
            .map(|job| job.id)?;
        self.jobs_mut().find(|job| job.id == id)
    }
}

/// What the index admission currently asks automatic work to yield to.
#[derive(Clone, Copy, Default)]
struct Preemption {
    foreground: bool,
    background: bool,
}

impl Preemption {
    fn current() -> Self {
        Self {
            foreground: crate::scan_runtime::foreground_pending(),
            background: crate::scan_runtime::background_pending(),
        }
    }

    fn stops(self, manual: bool, urgent: bool) -> bool {
        !manual && (self.foreground || (self.background && !urgent))
    }
}

fn job_cancelled(
    runtime: &RuntimeState,
    job: &Job,
    shutting_down: bool,
    preemption: Preemption,
) -> bool {
    shutting_down
        || job.stop
        || runtime.paused(job.class)
        || preemption.stops(job.manual, job.urgent)
}

/// Whether a queued requested heavy job must keep waiting for its turn.
fn manual_waits(runtime: &RuntimeState, ticket: u64, hash: Option<&str>, shutting_down: bool) -> bool {
    !shutting_down
        && (runtime.claim_covers(hash)
            || runtime.heavy.is_some()
            || runtime.serving_manual_ticket != ticket)
}

/// Whether a queued requested preview may take a preview-lane slot now.
/// Requested and automatic preview jobs never overlap, and requested ones
/// share the lane up to its capacity but never on the same item, so two
/// owners never derive or promote the same item at once. A later request for
/// an item finds the earlier one's output already in place.
fn preview_admits(runtime: &RuntimeState, ticket: u64, hash: &str, capacity: usize) -> bool {
    runtime.preview_queue.front() == Some(&ticket)
        && runtime.previews.len() < capacity
        && runtime
            .previews
            .iter()
            .all(|job| job.manual && job.hash.as_deref() != Some(hash))
}

#[derive(Clone, Copy)]
pub struct RuntimeConditions {
    pub busy: bool,
    pub worker_running: bool,
}

#[derive(Clone)]
pub struct ActiveWorkSnapshot {
    pub(crate) class: WorkClass,
    pub(crate) hash: Option<String>,
    pub(crate) done: Option<u64>,
    pub(crate) total: Option<u64>,
    pub(crate) stopping: bool,
}

#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub(crate) worker_running: bool,
    pub(crate) paused_classes: u8,
    pub(crate) active: Option<ActiveWorkSnapshot>,
    pub(crate) busy: bool,
}

static RUNTIME: LazyLock<(Mutex<RuntimeState>, Condvar)> =
    LazyLock::new(|| (Mutex::new(RuntimeState::default()), Condvar::new()));
static POISON_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static POISON_ISSUE_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

thread_local! {
    static CURRENT_JOB: Cell<Option<u64>> = const { Cell::new(None) };
}

fn lock(app: Option<&AppHandle>) -> Result<MutexGuard<'static, RuntimeState>, String> {
    RUNTIME.0.lock().map_err(|_| {
        report_poison_once(app);
        STATE_UNAVAILABLE.to_string()
    })
}

fn request_active_cancel(class: WorkClass) {
    if class.is_transcription() {
        crate::transcription::request_cancel();
    } else if class == WorkClass::Faces {
        crate::face::request_cancel();
    }
}

fn emit(app: Option<&AppHandle>) {
    if let Some(app) = app {
        emit_state_changed(app);
    }
}

pub fn cancel_active_transcription() -> bool {
    match RUNTIME.0.lock() {
        Ok(mut runtime) => {
            let Some(job) = runtime
                .heavy
                .as_mut()
                .filter(|job| job.class.is_transcription())
            else {
                return false;
            };
            job.stop = true;
            // Keep the process-global signal tied to the runtime owner selected
            // above; a queued transcription cannot take over between the two.
            request_active_cancel(job.class);
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

/// One admitted job. It binds the job to the admitting thread for
/// `cancelled`, and releases its lane slot and queue turn when dropped.
struct ActiveGuard {
    app: Option<AppHandle>,
    id: u64,
    manual_ticket: Option<u64>,
    previous: Option<u64>,
}

impl ActiveGuard {
    fn bind(app: Option<&AppHandle>, id: u64, manual_ticket: Option<u64>) -> Self {
        let previous = CURRENT_JOB.with(|current| current.replace(Some(id)));
        emit(app);
        Self {
            app: app.cloned(),
            id,
            manual_ticket,
            previous,
        }
    }

    /// Automatic admission never waits: it declines while the lane is full,
    /// requested work runs or is queued in it, the class is paused, the
    /// calling thread's share is preempted, or a file operation holds the
    /// media boundary.
    fn begin(app: Option<&AppHandle>, class: WorkClass) -> Result<Option<Self>, String> {
        let capacity = preview_capacity(class);
        let urgent = crate::scan_runtime::urgent_share();
        let mut runtime = lock(app)?;
        let lane_full = if lane_is_previews(class) {
            runtime.previews.len() >= capacity
                || runtime.previews.iter().any(|job| job.manual)
                || !runtime.preview_queue.is_empty()
        } else {
            runtime.heavy.is_some() || runtime.manual_heavy_queued()
        };
        if shutting_down()
            || Preemption::current().stops(false, urgent)
            || runtime.claim.is_some()
            || runtime.paused(class)
            || lane_full
        {
            return Ok(None);
        }
        let id = runtime.insert(class, false, urgent, None);
        drop(runtime);
        Ok(Some(Self::bind(app, id, None)))
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        CURRENT_JOB.with(|current| current.set(self.previous));
        if let Ok(mut runtime) = RUNTIME.0.lock() {
            runtime.remove(self.id);
            if self.manual_ticket == Some(runtime.serving_manual_ticket) {
                runtime.serving_manual_ticket = runtime.serving_manual_ticket.wrapping_add(1);
            }
            RUNTIME.1.notify_all();
        } else {
            report_poison_once(self.app.as_ref());
        }
        emit(self.app.as_ref());
    }
}

fn preview_capacity(class: WorkClass) -> usize {
    if lane_is_previews(class) {
        crate::resource_limits::image_worker_capacity(false).max(1)
    } else {
        1
    }
}

/// Whether the preview lane could actually admit automatic work right now —
/// the same refusal conditions `ActiveGuard::begin` checks for the Previews
/// class, minus the per-thread `urgent`/`Preemption` state that only exists
/// once a share is already held. `derived_work::run_preview_pass` checks this
/// before contending for the index admission's urgent share: without it, a
/// preview pass whose visible/nearby tier has required work, but whose lane
/// is entirely occupied by a requested preview (or its FIFO queue), still
/// took the urgent share — preempting a background index owner's turn (e.g.
/// file-information completion, at whatever it had done so far) only to find
/// no slot free and do nothing (Phase 10 fresh review of Phase 3).
pub(crate) fn preview_lane_open_for_automatic() -> bool {
    let capacity = preview_capacity(WorkClass::Previews);
    let Ok(runtime) = RUNTIME.0.lock() else {
        return false;
    };
    !shutting_down()
        && runtime.claim.is_none()
        && !runtime.paused(WorkClass::Previews)
        && runtime.previews.len() < capacity
        && !runtime.previews.iter().any(|job| job.manual)
        && runtime.preview_queue.is_empty()
}

pub(crate) fn with_active<T>(
    app: &AppHandle,
    class: WorkClass,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    let Some(_active) = ActiveGuard::begin(Some(app), class)? else {
        return Ok(None);
    };
    work().map(Some)
}

pub struct ManualWorkGuard {
    _guard: ActiveGuard,
}

/// Admits a preview or full-resolution image the user is waiting for into
/// the preview lane, beside any heavy work. It waits in FIFO order for a slot,
/// stopping automatic preview work in the lane, and answers busy after a
/// bounded wait instead of failing at once.
pub fn begin_requested_preview(app: &AppHandle, hash: &str) -> Result<ManualWorkGuard, String> {
    requested_preview(Some(app), hash)
}

fn requested_preview(app: Option<&AppHandle>, hash: &str) -> Result<ManualWorkGuard, String> {
    let capacity = preview_capacity(WorkClass::Previews);
    let mut runtime = lock(app)?;
    if shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    if runtime.paused(WorkClass::Previews) {
        return Err(paused_message(WorkClass::Previews));
    }
    if runtime.claim_covers(Some(hash)) {
        return Err(FILE_IN_USE.to_string());
    }
    let ticket = runtime.next_preview_ticket;
    runtime.next_preview_ticket = runtime.next_preview_ticket.wrapping_add(1);
    runtime.preview_queue.push_back(ticket);
    let deadline = Instant::now() + REQUESTED_WAIT;
    let refusal = loop {
        if shutting_down() {
            break crate::scanner::CANCELLED.to_string();
        }
        if runtime.paused(WorkClass::Previews) {
            break paused_message(WorkClass::Previews);
        }
        if runtime.claim_covers(Some(hash)) {
            break FILE_IN_USE.to_string();
        }
        if preview_admits(&runtime, ticket, hash, capacity) {
            runtime.preview_queue.pop_front();
            let id = runtime.insert(WorkClass::Previews, true, false, Some(hash.to_string()));
            RUNTIME.1.notify_all();
            drop(runtime);
            return Ok(ManualWorkGuard {
                _guard: ActiveGuard::bind(app, id, None),
            });
        }
        for job in runtime.previews.iter_mut().filter(|job| !job.manual) {
            job.stop = true;
        }
        let now = Instant::now();
        if now >= deadline {
            break "Preview work is busy. Try again in a moment.".to_string();
        }
        runtime = match RUNTIME
            .1
            .wait_timeout(runtime, WAIT_SLICE.min(deadline - now))
        {
            Ok((next, _)) => next,
            Err(_) => {
                report_poison_once(app);
                return Err(STATE_UNAVAILABLE.to_string());
            }
        };
    };
    runtime.preview_queue.retain(|queued| *queued != ticket);
    RUNTIME.1.notify_all();
    Err(refusal)
}

/// Waits in FIFO order for the heavy lane. Manual transcription commands
/// return to the UI before entering this wait, so a second request is a real
/// queued job rather than a rejected or blocking command. A running
/// automatic heavy job is stopped for it and restarts later.
pub(crate) fn begin_requested(app: &AppHandle, class: WorkClass, hash: &str) -> Result<ManualWorkGuard, String> {
    requested_heavy(Some(app), class, hash)
}

fn requested_heavy(
    app: Option<&AppHandle>,
    class: WorkClass,
    hash: &str,
) -> Result<ManualWorkGuard, String> {
    let mut runtime = lock(app)?;
    if shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    let ticket = runtime.next_manual_ticket;
    runtime.next_manual_ticket = runtime.next_manual_ticket.wrapping_add(1);

    if let Some(job) = runtime.heavy.as_mut().filter(|job| !job.manual) {
        job.stop = true;
        let class = job.class;
        drop(runtime);
        request_active_cancel(class);
        emit(app);
        runtime = lock(app)?;
    }

    runtime = RUNTIME
        .1
        .wait_while(runtime, |state| {
            manual_waits(state, ticket, Some(hash), shutting_down())
        })
        .map_err(|_| {
            report_poison_once(app);
            STATE_UNAVAILABLE.to_string()
        })?;

    if shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    if runtime.paused(class) {
        runtime.serving_manual_ticket = runtime.serving_manual_ticket.wrapping_add(1);
        RUNTIME.1.notify_all();
        return Err(paused_message(class));
    }
    let id = runtime.insert(class, true, false, Some(hash.to_string()));
    drop(runtime);
    Ok(ManualWorkGuard {
        _guard: ActiveGuard::bind(app, id, Some(ticket)),
    })
}

/// Temporarily excludes derived-media owners from a set of files. File
/// operations take it before asking webviews to release playback handles, so
/// neither a cache derive nor a model job can reopen an item during the
/// release-to-mutate interval.
pub struct ExclusiveGuard {
    app: Option<AppHandle>,
}

impl Drop for ExclusiveGuard {
    fn drop(&mut self) {
        if let Ok(mut runtime) = RUNTIME.0.lock() {
            runtime.claim = None;
            RUNTIME.1.notify_all();
        } else {
            report_poison_once(self.app.as_ref());
        }
        emit(self.app.as_ref());
    }
}

/// Claims `keys` (empty: every item) and stops the jobs `policy` allows:
/// a file operation stops every job on its files and waits, however long it
/// takes, until `cancelled`; an external open stops only automatic work on
/// the same file, waits at most [`REQUESTED_WAIT`], and otherwise proceeds at
/// once.
pub(crate) fn begin_exclusive(
    app: &AppHandle,
    keys: &[String],
    policy: ClaimPolicy,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExclusiveGuard, String> {
    exclusive_claim(Some(app), keys, policy, cancelled)
}

fn exclusive_claim(
    app: Option<&AppHandle>,
    keys: &[String],
    policy: ClaimPolicy,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExclusiveGuard, String> {
    let mut runtime = lock(app)?;
    if shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    if runtime.claim.is_some() {
        return Err("Another file operation is already running.".to_string());
    }
    let claim = Claim {
        keys: keys.to_vec(),
    };
    let mut targets = Vec::new();
    let mut kick = Vec::new();
    for job in runtime.jobs_mut() {
        if claim.covers(job.hash.as_deref()) && policy.stops(job) {
            job.stop = true;
            targets.push(job.id);
            kick.push(job.class);
        }
    }
    runtime.claim = Some(claim);
    drop(runtime);
    let guard = ExclusiveGuard { app: app.cloned() };
    for class in kick {
        request_active_cancel(class);
    }
    emit(app);
    let deadline = (policy == ClaimPolicy::ExternalOpen).then(|| Instant::now() + REQUESTED_WAIT);
    loop {
        if shutting_down() || cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        let runtime = lock(app)?;
        if !targets.iter().any(|id| runtime.job(*id).is_some()) {
            return Ok(guard);
        }
        let now = Instant::now();
        let slice = match deadline {
            Some(deadline) if now >= deadline => {
                return Err(
                    "Background work on this file is still stopping. Try again in a moment."
                        .to_string(),
                )
            }
            Some(deadline) => WAIT_SLICE.min(deadline - now),
            None => WAIT_SLICE,
        };
        let _ = RUNTIME.1.wait_timeout(runtime, slice);
    }
}

pub(crate) fn exclusive() -> bool {
    RUNTIME
        .0
        .lock()
        .map(|runtime| runtime.claim.is_some())
        .unwrap_or_else(|_| {
            report_poison_once(None);
            true
        })
}

/// A foreground admission or background index owner is waiting: automatic
/// jobs already observe it through `cancelled`; this also interrupts a native
/// engine at once instead of at its next poll. The heavy lane never holds an
/// urgent share, so both preempt its automatic job.
pub(crate) fn preempt_automatic() {
    let class = RUNTIME.0.lock().ok().and_then(|runtime| {
        runtime
            .heavy
            .as_ref()
            .filter(|job| !job.manual)
            .map(|job| job.class)
    });
    if let Some(class) = class {
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

/// Whether the calling thread's job should stop at its next safe point.
/// Shared by every owned ffmpeg process: a stop kills its child within the
/// subprocess poll interval; in-process work stops at the next item edge.
pub fn cancelled() -> bool {
    let shutting_down = shutting_down();
    let preemption = Preemption::current();
    RUNTIME
        .0
        .lock()
        .map(|runtime| {
            shutting_down
                || runtime
                    .current_job()
                    .is_some_and(|job| job_cancelled(&runtime, job, shutting_down, preemption))
        })
        .unwrap_or_else(|_| {
            report_poison_once(None);
            true
        })
}

/// Carries the calling thread's job to a helper thread that polls a stop
/// condition on its behalf, such as a native engine's cancel watcher.
pub(crate) fn on_behalf_of_current_job(
    check: Box<dyn Fn() -> bool + Send + 'static>,
) -> impl Fn() -> bool + Send + 'static {
    let job = CURRENT_JOB.with(Cell::get);
    move || {
        let previous = CURRENT_JOB.with(|current| current.replace(job));
        let stop = check();
        CURRENT_JOB.with(|current| current.set(previous));
        stop
    }
}

/// Closes derived-media admission before waking queued owners. Active native
/// work receives its existing class-specific cancellation signal; queued
/// manual requests wake and settle as cancellations.
pub fn begin_shutdown(app: &AppHandle) {
    let heavy = match RUNTIME.0.lock() {
        Ok(mut runtime) => {
            for job in runtime.jobs_mut() {
                job.stop = true;
            }
            RUNTIME.1.notify_all();
            runtime.heavy.as_ref().map(|job| job.class)
        }
        Err(_) => {
            report_poison_once(Some(app));
            None
        }
    };
    if let Some(class) = heavy {
        request_active_cancel(class);
    }
}

pub(crate) fn shutting_down() -> bool {
    crate::app_lifecycle::shutting_down()
}

pub fn set_paused(app: &AppHandle, class: Option<&str>, paused: bool) -> Result<(), String> {
    if shutting_down() {
        return Err(crate::scanner::CANCELLED.to_string());
    }
    let heavy = {
        let mut runtime = lock(Some(app))?;
        runtime.paused_classes = changed_pause_classes(runtime.paused_classes, class, paused)?;
        RUNTIME.1.notify_all();
        runtime.heavy.as_ref().map(|job| job.class)
    };

    if paused {
        if let Some(active) = heavy.filter(|active| class.is_none() || class == Some(active.id())) {
            request_active_cancel(active);
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
        if let Some(job) = runtime.current_job_mut(class) {
            job.done = done;
            job.total = total;
        }
    } else {
        report_poison_once(Some(app));
    }
    emit_state_changed(app);
}

/// Names the item the calling thread's job is working on, which is what a
/// file operation's claim matches: call it before the job opens the file,
/// and again when the item's identity is promoted.
pub(crate) fn active_item(app: &AppHandle, class: WorkClass, hash: &str) {
    if !set_active_item(class, hash) {
        report_poison_once(Some(app));
    }
    emit_state_changed(app);
}

fn set_active_item(class: WorkClass, hash: &str) -> bool {
    let Ok(mut runtime) = RUNTIME.0.lock() else {
        return false;
    };
    if let Some(job) = runtime.current_job_mut(class) {
        job.hash = Some(hash.to_string());
    }
    true
}

fn active_snapshot(runtime: &RuntimeState, preemption: Preemption) -> Option<ActiveWorkSnapshot> {
    runtime.display_job().map(|job| ActiveWorkSnapshot {
        class: job.class,
        hash: job.hash.clone(),
        done: job.done,
        total: job.total,
        stopping: job_cancelled(runtime, job, false, preemption),
    })
}

pub(crate) fn emit_state_changed(app: &AppHandle) {
    if shutting_down() {
        return;
    }
    match state_payload() {
        Ok(payload) => crate::failure_runtime::emit_or_record(app, "derived://state-changed", payload),
        Err(_) => report_poison_once(Some(app)),
    }
}

pub fn state_payload() -> Result<serde_json::Value, String> {
    let preemption = Preemption::current();
    let payload = match RUNTIME.0.lock() {
        Ok(runtime) => {
            let paused_classes = WorkClass::ALL
                .into_iter()
                .filter(|class| runtime.paused(*class))
                .map(WorkClass::id)
                .collect::<Vec<_>>();
            json!({
                "workerRunning": crate::derived_work::started(),
                "pausedClasses": paused_classes,
                "active": active_snapshot(&runtime, preemption).map(|active| json!({
                    "id": active.class.id(),
                    "hash": active.hash,
                    "done": active.done,
                    "total": active.total,
                    "stopping": active.stopping,
                })),
            })
        }
        Err(_) => {
            report_poison_once(None);
            return Err(STATE_UNAVAILABLE.to_string());
        }
    };
    Ok(payload)
}

pub fn snapshot(conditions: RuntimeConditions) -> Result<RuntimeSnapshot, String> {
    let preemption = Preemption::current();
    let runtime = lock(None)?;
    Ok(RuntimeSnapshot {
        worker_running: conditions.worker_running,
        paused_classes: runtime.paused_classes,
        active: active_snapshot(&runtime, preemption),
        busy: conditions.busy,
    })
}

/// Bulk controls change the same per-class bits as individual controls.
pub fn changed_pause_classes(current: u8, class: Option<&str>, paused: bool) -> Result<u8, String> {
    let class = class.map(|id| WorkClass::parse(id)
        .ok_or_else(|| format!("unknown background-work class: {id}"))).transpose()?;
    let mask = class.map_or_else(|| WorkClass::ALL.into_iter().fold(0, |bits, class| bits | class.bit()), WorkClass::bit);
    Ok(if paused { current | mask } else { current & !mask })
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// `RuntimeState`, its lanes, claims and wait and cancellation internals;
// promoting them would widen the crate's API only for this test.
#[path = "../tests/unit/derived_runtime.rs"]
mod tests;
