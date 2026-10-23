//! Bounded subprocess execution for the managed tools.
//!
//! Every ffmpeg invocation used to be a plain blocking `output()`/`status()`,
//! which meant two things the app could not recover from: a hung ffmpeg hung
//! the scan AND the quit forever, and the derive thread could not see the
//! cancel flag while a child ran, so quitting during a large video waited for
//! that video to finish.
//!
//! Both are fixed by watching the child rather than blocking on it. Following
//! tapebox's `spawn.ts`, the bound is an IDLE timeout rather than a wall-clock
//! one: a legitimately slow job on a 4 GB video keeps producing output and is
//! left alone, while a genuinely stuck one is killed. A wall-clock limit
//! cannot tell those apart and would kill real work on big files.
//!
//! A caller may stop waiting after cancellation while its owner retains the
//! child, bounded pipe readers, staged output and capacity until settlement.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// No output at all for this long means stuck, not slow.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub const STDOUT_BYTES: usize = 1024 * 1024;
pub const STDERR_BYTES: usize = 1024 * 1024;

/// How often the watcher wakes to check liveness, idleness and cancellation.
const POLL: Duration = Duration::from_millis(100);
const CANCEL_SETTLEMENT: Duration = Duration::from_secs(2);
static PHYSICAL_RUNS: AtomicUsize = AtomicUsize::new(0);

struct Capacity(&'static AtomicUsize);

impl Capacity {
    fn reserve() -> Result<Self, String> {
        let ceiling = crate::resource_limits::cpu_thread_budget().max(1) as usize;
        Self::reserve_from(&PHYSICAL_RUNS, ceiling)
    }

    fn reserve_from(counter: &'static AtomicUsize, ceiling: usize) -> Result<Self, String> {
        counter.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < ceiling).then_some(current + 1)
        }).map_err(|_| format!(
            "{}managed tool capacity is occupied by work that has not settled",
            crate::resource_limits::SAFETY_ERROR_PREFIX,
        ))?;
        Ok(Self(counter))
    }
}

impl Drop for Capacity {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); }
}

/// A private output stays owned until the last caller or writer settles.
pub(crate) struct StagedOutput {
    path: PathBuf,
    file: Mutex<Option<std::fs::File>>,
}

impl StagedOutput {
    pub(crate) fn new(path: PathBuf) -> std::io::Result<Self> {
        let mut options = std::fs::OpenOptions::new(); // data root
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Self { path, file: Mutex::new(Some(file)) })
    }

    pub(crate) fn path(&self) -> &Path { &self.path }

    fn take_file(&self) -> Result<std::fs::File, String> {
        self.file.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
            .take().ok_or_else(|| "staged subprocess output already has a writer".to_string())
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        // Windows cannot remove an open file. Close any unused writer first.
        let file = self.file.get_mut().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        drop(file);
        if let Err(error) = std::fs::remove_file(&self.path) { // data root
            if error.kind() != std::io::ErrorKind::NotFound {
                crate::logging::warn("staged subprocess output cleanup failed",
                    serde_json::json!({ "error": { "message": error.to_string() } }));
            }
        }
    }
}

enum StdoutTarget {
    Capture(usize),
    File(Arc<StagedOutput>, u64),
}

/// Every subprocess currently running, for the exit watchdog: if quitting's
/// bounded wait for derived work, mutations or requested media expires
/// before every job reaches its own cancellation point, `kill_all_running`
/// force-kills whatever managed-tool subprocess is still alive instead of
/// leaving it to write into `temp/` or the cache after the app has exited.
static RUNNING: Mutex<Vec<Arc<Mutex<OwnedProcess>>>> = Mutex::new(Vec::new());

/// A spawned managed tool and, on Windows, the job object that holds it and
/// every process it starts, so ending it ends the whole tree as the unix
/// process group does.
pub(crate) struct OwnedProcess {
    child: std::process::Child,
    #[cfg(unix)]
    guardian: Option<crate::subprocess_supervisor::Guard>,
    #[cfg(windows)]
    tree: Option<process_tree::Job>,
}

impl OwnedProcess {
    fn new(child: std::process::Child) -> Self {
        #[cfg(windows)]
        let tree = match process_tree::Job::holding(&child) {
            Ok(job) => Some(job),
            Err(error) => {
                // Without the job only the direct child can be ended.
                crate::logging::warn(
                    "subprocess job object failed",
                    serde_json::json!({ "error": { "message": error.to_string() } }),
                );
                None
            }
        };
        Self {
            child,
            #[cfg(unix)]
            guardian: None,
            #[cfg(windows)]
            tree,
        }
    }
}

impl std::ops::Deref for OwnedProcess {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for OwnedProcess {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

/// Registers a spawned child for the exit watchdog; dropping the guard
/// removes it only when the owner and all pipe readers have settled.
struct Registration(Arc<Mutex<OwnedProcess>>);

impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut running) = RUNNING.lock() {
            running.retain(|child| !Arc::ptr_eq(child, &self.0));
        }
    }
}

fn register_running(child: Arc<Mutex<OwnedProcess>>) -> Registration {
    if let Ok(mut running) = RUNNING.lock() {
        running.push(Arc::clone(&child));
    }
    Registration(child)
}

fn lock_child(
    child: &Arc<Mutex<OwnedProcess>>,
) -> std::sync::MutexGuard<'_, OwnedProcess> {
    child.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Kills every subprocess still running right now. Best-effort and
/// idempotent: a child that already exited is left to its own owning thread,
/// which reaps it on its next poll. Called once quitting's bounded wait for
/// the non-mutation exit joins expires, and again if quitting gives up on the
/// active mutation at its own deadline.
pub fn kill_all_running() {
    kill_running(&RUNNING);
}

fn kill_running(registry: &Mutex<Vec<Arc<Mutex<OwnedProcess>>>>) {
    let running = match registry.lock() {
        Ok(running) => running.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    for child in running {
        kill_owned(&mut lock_child(&child));
    }
}

/// Last best-effort signals before a forced process exit. Never wait for a
/// registry lock, child lock, child reap, or diagnostic: the ordinary owners
/// still reap; a held owner must not extend the aggregate quit deadline.
pub(crate) fn signal_all_running_for_exit() {
    signal_for_exit(&RUNNING);
}

/// `signal_all_running_for_exit` over a given registry, so tests can signal a
/// registry of their own: signalling the process-wide one would also kill
/// subprocesses that tests running in parallel own.
fn signal_for_exit(registry: &Mutex<Vec<Arc<Mutex<OwnedProcess>>>>) {
    let running = match registry.try_lock() {
        Ok(running) => running,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return,
    };
    for child in running.iter() {
        let mut child = match child.try_lock() {
            Ok(child) => child,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => continue,
        };
        kill_owned(&mut child);
    }
}

pub struct Run {
    pub status_ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Run {
    /// The whole of stderr, trimmed, for an error message. It is never cut:
    /// errors reach Issues and Records, and only their display may shorten
    /// them (text-cleanup-conventions, truncation is display-only).
    /// `STDERR_BYTES` already bounds what a run can capture.
    pub fn stderr_text(&self) -> String {
        self.stderr.trim().to_string()
    }
}

/// Runs `command` to completion, killing it if it goes idle or if `cancelled`
/// starts returning true.
///
/// `cancelled` is polled rather than captured once so a quit mid-derive kills
/// the child instead of waiting for it — the difference between an app that
/// closes and one that appears to hang.
pub fn run_bounded(command: Command, cancelled: &dyn Fn() -> bool) -> Result<Run, String> {
    run_bounded_idle_output(command, cancelled, IDLE_TIMEOUT, STDOUT_BYTES)
}

/// `run_bounded` with a hard stdout ceiling. Binary-producing commands use
/// this so a malformed or unexpectedly large stream cannot grow a `Vec`
/// until the process is out of memory.
pub fn run_bounded_output(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    max_stdout: usize,
) -> Result<Run, String> {
    run_bounded_idle_output(command, cancelled, IDLE_TIMEOUT, max_stdout)
}

/// `run_bounded` with a caller-chosen idle bound, for work whose healthy runtime
/// is far below the media default — a `-version` probe answers in milliseconds,
/// and letting a wedged one hold a status read for the full 120s would look like
/// a hang.
pub fn run_bounded_idle(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    idle_timeout: Duration,
) -> Result<Run, String> {
    run_bounded_idle_output(command, cancelled, idle_timeout, STDOUT_BYTES)
}

fn run_bounded_idle_output(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    idle_timeout: Duration,
    max_stdout: usize,
) -> Result<Run, String> {
    start_owned(command, cancelled, idle_timeout, StdoutTarget::Capture(max_stdout))
}

/// Streams bounded stdout into an exclusively created private file. The
/// reader owns the file and an output reference even if its caller cancels.
pub(crate) fn run_bounded_to_file(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    output: Arc<StagedOutput>,
    max_bytes: u64,
) -> Result<Run, String> {
    start_owned(command, cancelled, IDLE_TIMEOUT, StdoutTarget::File(output, max_bytes))
}

enum OwnerEvent {
    Stopping(String),
    Settled(Result<Run, String>),
}

fn start_owned(
    command: Command,
    cancelled: &dyn Fn() -> bool,
    idle_timeout: Duration,
    target: StdoutTarget,
) -> Result<Run, String> {
    if cancelled() { return Err(crate::scanner::CANCELLED.to_string()); }
    // A brief busy period queues at the caller, without creating another
    // physical owner. Unsettled children keep their slots and eventually
    // refuse admission rather than spawning an unbounded replacement fleet.
    let admission_started = Instant::now();
    let capacity = loop {
        match Capacity::reserve() {
            Ok(capacity) => break capacity,
            Err(error) if admission_started.elapsed() >= CANCEL_SETTLEMENT => return Err(error),
            Err(_) => {
                if cancelled() { return Err(crate::scanner::CANCELLED.to_string()); }
                std::thread::sleep(POLL);
            }
        }
    };
    let cancellation = Arc::new(AtomicBool::new(false));
    let owner_cancellation = cancellation.clone();
    let (settled, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("onecopy-subprocess-owner".to_string())
        .spawn(move || {
            let _capacity = capacity;
            let result = crate::failure_runtime::contain_item(|| {
                run_owned(command, &owner_cancellation, idle_timeout, target, &settled)
            });
            let _ = settled.send(OwnerEvent::Settled(result));
        }).map_err(|error| format!("could not start subprocess owner: {error}"))?;
    wait_for_owner(receiver, cancellation, cancelled, CANCEL_SETTLEMENT)
}

fn wait_for_owner(
    receiver: std::sync::mpsc::Receiver<OwnerEvent>,
    cancellation: Arc<AtomicBool>,
    cancelled: &dyn Fn() -> bool,
    settlement_bound: Duration,
) -> Result<Run, String> {
    let mut stopping: Option<(String, Instant)> = None;
    loop {
        if cancelled() && stopping.is_none() {
            cancellation.store(true, Ordering::Release);
            stopping = Some((crate::scanner::CANCELLED.to_string(), Instant::now()));
        }
        if let Some((reason, started)) = &stopping {
            if started.elapsed() >= settlement_bound {
                crate::logging::warn("subprocess settlement continues after caller return",
                    serde_json::json!({"reason": reason}));
                return Err(if reason == crate::scanner::CANCELLED { reason.clone() }
                    else { format!("{reason}: subprocess settlement pending") });
            }
        }
        let wait = stopping.as_ref().map_or(POLL, |(_, started)| {
            settlement_bound.saturating_sub(started.elapsed()).min(POLL)
        });
        match receiver.recv_timeout(wait) {
            Ok(OwnerEvent::Stopping(reason)) => {
                let started = stopping.as_ref().map_or_else(Instant::now, |(_, started)| *started);
                stopping = Some((reason, started));
            }
            Ok(OwnerEvent::Settled(result)) => return match result {
                Ok(_) if cancellation.load(Ordering::Acquire) => Err(crate::scanner::CANCELLED.to_string()),
                result => result,
            },
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {},
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(stopping.map_or_else(
                    || "subprocess owner ended without a result".to_string(),
                    |(reason, _)| format!("{reason}: subprocess owner ended without a result"),
                ));
            }
        }
    }
}

fn run_owned(
    mut command: Command,
    cancelled: &AtomicBool,
    idle_timeout: Duration,
    target: StdoutTarget,
    events: &std::sync::mpsc::Sender<OwnerEvent>,
) -> Result<Run, String> {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    if cancelled.load(Ordering::Acquire) { return Err(crate::scanner::CANCELLED.to_string()); }
    #[cfg(unix)]
    let guardian = crate::subprocess_supervisor::prepare(&mut command).map_err(|error| error.to_string())?;
    let mut spawned = command.stdin(Stdio::null()).stdout(Stdio::piped())
        .stderr(Stdio::piped()).spawn().map_err(|error| error.to_string())?;
    let stdout_pipe = spawned.stdout.take();
    let stderr_pipe = spawned.stderr.take();
    let owned = OwnedProcess::new(spawned);
    #[cfg(unix)]
    let owned = { let mut owned = owned; owned.guardian = Some(guardian); owned };
    let child = Arc::new(Mutex::new(owned));
    let _registration = register_running(child.clone());
    let last_output = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let failure = Arc::new(Mutex::new(None::<String>));
    let mut readers = Vec::new();
    let mut terminal = None;
    if let Some(out) = stdout_pipe {
        match start_reader("stdout", out, target, last_output.clone(), started, failure.clone()) {
            Ok(reader) => readers.push(reader),
            Err(error) => terminal = Some(error),
        }
    }
    if let Some(err) = stderr_pipe {
        match start_reader("stderr", err, StdoutTarget::Capture(STDERR_BYTES), last_output.clone(), started, failure.clone()) {
            Ok(reader) => readers.push(reader),
            Err(error) => { if terminal.is_none() { terminal = Some(error); } },
        }
    }
    let mut status = None;
    let mut signalled = false;
    loop {
        if terminal.is_none() {
            terminal = failure.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        }
        if terminal.is_none() && cancelled.load(Ordering::Acquire) {
            terminal = Some(crate::scanner::CANCELLED.to_string());
        }
        if terminal.is_none() {
            let idle_ms = (started.elapsed().as_millis() as u64)
                .saturating_sub(last_output.load(Ordering::Relaxed));
            if idle_ms > idle_timeout.as_millis() as u64 {
                terminal = Some(format!("no output for {}s — killed as stuck", idle_timeout.as_secs()));
            }
        }
        if terminal.is_some() && !signalled {
            let _ = events.send(OwnerEvent::Stopping(terminal.as_ref().unwrap().clone()));
            kill_owned(&mut lock_child(&child));
            signalled = true;
        }
        if status.is_none() {
            match lock_child(&child).try_wait() {
                Ok(Some(exited)) => status = Some(exited.success()),
                Ok(None) => {},
                Err(error) => { if terminal.is_none() { terminal = Some(error.to_string()); } },
            }
        }
        #[cfg(not(unix))]
        let guardian_settled = true;
        #[cfg(unix)]
        let guardian_settled = if status.is_some() {
            let mut owned = lock_child(&child);
            if let Some(guardian) = owned.guardian.as_mut() {
                guardian.signal();
                guardian.try_settle().unwrap_or(false)
            } else { true }
        } else { false };
        if status.is_some() && guardian_settled && readers.iter().all(|reader| reader.is_finished()) { break; }
        std::thread::sleep(POLL);
    }
    let mut outputs = Vec::new();
    for reader in readers {
        match reader.join().map_err(crate::failure_runtime::panic_message).and_then(|result| result) {
            Ok(output) => outputs.push(output),
            Err(error) => { if terminal.is_none() { terminal = Some(error); } },
        }
    }
    let mut stdout = Vec::new();
    let mut stderr = String::new();
    for (name, output) in outputs {
        if name == "stdout" { stdout = output; }
        else { stderr = String::from_utf8_lossy(&output).into_owned(); }
    }
    match terminal {
        None => Ok(Run { status_ok: status.unwrap_or(false), stdout, stderr }),
        Some(reason) => {
            let detail = stderr.trim();
            Err(if detail.is_empty() { reason } else { format!("{reason}: {detail}") })
        }
    }
}

type Reader = std::thread::JoinHandle<Result<(&'static str, Vec<u8>), String>>;

fn start_reader(
    name: &'static str,
    mut pipe: impl Read + Send + 'static,
    target: StdoutTarget,
    seen: Arc<AtomicU64>,
    started: Instant,
    failure: Arc<Mutex<Option<String>>>,
) -> Result<Reader, String> {
    std::thread::Builder::new().name(format!("onecopy-subprocess-{name}"))
        .spawn(move || {
            let result = crate::failure_runtime::contain_item(|| {
                let mut bytes = Vec::new();
                // File closes before the target's last Arc can remove it.
                let mut file = match &target {
                    StdoutTarget::File(output, _) => Some(output.take_file()?),
                    StdoutTarget::Capture(_) => None,
                };
                let limit = match &target {
                    StdoutTarget::Capture(limit) => *limit as u64,
                    StdoutTarget::File(_, limit) => *limit,
                };
                let mut total = 0u64;
                let mut chunk = [0u8; 64 * 1024];
                loop {
                    let count = pipe.read(&mut chunk).map_err(|error| format!("subprocess {name} read failed: {error}"))?;
                    if count == 0 { break; }
                    seen.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                    total = total.saturating_add(count as u64);
                    if total > limit {
                        let stream = if name == "stdout" { "output" } else { name };
                        return Err(format!("subprocess {stream} exceeded the {limit} byte safety limit"));
                    }
                    if let Some(file) = &mut file {
                        file.write_all(&chunk[..count]).map_err(|error| crate::resource_limits::cache_write_error("subprocess output write failed", error))?;
                    } else { bytes.extend_from_slice(&chunk[..count]); }
                }
                if let Some(file) = &mut file {
                    file.flush().map_err(|error| crate::resource_limits::cache_write_error("subprocess output flush failed", error))?;
                }
                Ok((name, bytes))
            });
            if let Err(error) = &result {
                let mut recorded = failure.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if recorded.is_none() { *recorded = Some(error.clone()); }
            }
            result
        }).map_err(|error| format!("could not start subprocess {name} reader: {error}"))
}

#[cfg(unix)]
fn kill_owned(child: &mut OwnedProcess) {
    if let Some(guardian) = child.guardian.as_mut() {
        guardian.signal();
    } else if matches!(child.try_wait(), Ok(None)) {
        // Direct isolated test owners have their own group. A live/unreaped
        // child pins its PID, so this cannot signal a reused process group.
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL); }
    }
}

#[cfg(not(unix))]
fn kill_owned(child: &mut OwnedProcess) { signal_tree(child); }

/// Ends the child and everything it started: the whole job when one holds
/// it, otherwise only the direct child.
#[cfg(not(unix))]
fn signal_tree(owned: &mut OwnedProcess) {
    #[cfg(windows)]
    if let Some(tree) = &owned.tree {
        match tree.terminate() {
            Ok(()) => return,
            Err(error) => crate::logging::warn(
                "subprocess tree termination failed",
                serde_json::json!({ "error": { "message": error.to_string() } }),
            ),
        }
    }
    if let Err(error) = owned.child.kill() {
        if error.kind() != std::io::ErrorKind::InvalidInput {
            crate::logging::warn(
                "subprocess termination failed",
                serde_json::json!({ "error": { "message": error.to_string() } }),
            );
        }
    }
}

/// A job object holding one managed tool and every process it starts.
/// Closing its last handle ends whatever is still in it, so nothing a tool
/// started outlives its owner even when the owner never signals it. A process
/// the tool starts in the instant between spawn and assignment escapes the
/// job; the managed tools start none.
#[cfg(windows)]
mod process_tree {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    pub(super) struct Job(HANDLE);

    // SAFETY: a job object handle is a kernel handle any thread may use.
    unsafe impl Send for Job {}

    impl Job {
        pub(super) fn holding(child: &std::process::Child) -> std::io::Result<Job> {
            // SAFETY: null attributes and name create an unnamed job owned by
            // this process; the handle is closed by Drop.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Job(handle);
            // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is valid.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: the pointer and size describe `limits`, alive for the call.
            let set = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: the child's process handle stays open while `child` lives.
            let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
            if assigned == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }

        pub(super) fn terminate(&self) -> std::io::Result<()> {
            // SAFETY: the handle is this job's, open until Drop.
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is owned by this value and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(all(test, unix))]
use std::os::unix::process::CommandExt;

#[cfg(test)]
// EXCEPTION: the deadline-only signal and registry are crate-private; these
// fixtures verify live child ownership and contended cleanup without exposing
// shutdown internals as a public API.
#[path = "../tests/unit/subprocess.rs"]
mod tests;
