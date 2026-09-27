//! The one bounded owner of filesystem calls on user volumes.
//!
//! OneCopy's job is operating on volumes that can stall: a sleeping NAS, a
//! share whose server went away, a failing USB drive. A filesystem call has no
//! native timeout, so every call on a configured source or destination (and
//! everything beneath them) runs here, on a pool thread this module can
//! abandon, and the caller waits at most the call's bound (PLAYBOOK "Bound
//! every external wait"):
//!
//! - quick checks (stat, metadata, exists, canonicalize, identity): 15 s;
//! - reading, writing, listing, renaming, removing, creating: 30 s each call;
//! - flushing a file: 30 s, or longer for a large file at 5 MB/s, derived
//!   from its size so a legitimate flush of gigabytes on a slow drive is not
//!   mistaken for a stall.
//!
//! A call given up on is *abandoned*: its thread stays blocked in the kernel
//! and the caller moves on. What the call did is then unknown until it
//! settles (`outcome_unknown`): an abandoned read is simply discarded, while
//! the caller of an abandoned rename, remove or write must treat its effect as
//! possibly done. The undelivered result is dropped on the worker when the call
//! finally returns, so a type whose `Drop` cleans up (a private staged file)
//! settles itself there.
//!
//! **Fail fast per volume.** While a volume has an abandoned call outstanding,
//! every new call on that volume answers `Stalled` at once without taking a
//! thread, so retries and other work never pile threads up behind a stall,
//! and calls on every other volume keep working. The volume recovers by
//! itself when the abandoned call returns.
//!
//! A call made from inside a pool worker (a primitive composed of several
//! filesystem steps) runs inline: the outer call already bounds it.
//!
//! OneCopy's own data folder is assumed to be on a local disk and is not
//! routed through this owner (plan decision); a data folder on a network
//! share would reintroduce unbounded waits in SQLite and the cache.
//!
//! Code outside this module does not name `std::fs`, `walkdir`, file opens or
//! path probes on user volumes; `tests/volume_egress_tests.rs` enforces that.

use std::collections::{HashMap, VecDeque};
use std::fs::{File, FileType, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Bound for a quick check: covers an HDD spin-up and an SMB reconnect.
pub const QUICK_BOUND: Duration = Duration::from_secs(15);
/// Bound for one read, write, listing, rename, remove or directory creation.
pub const IO_BOUND: Duration = Duration::from_secs(30);
/// The slowest flush rate still treated as a healthy drive (USB 2 class).
const SYNC_BYTES_PER_SECOND: u64 = 5 * 1000 * 1000;
/// How long a cancelled call may still finish on its own before it is
/// abandoned. A healthy drive answers well inside it, so an ordinary Cancel
/// never marks its volume stalled.
const CANCEL_GRACE: Duration = Duration::from_millis(250);
/// How often a waiting caller checks its Cancel and deadline.
const POLL: Duration = Duration::from_millis(20);
/// An idle pool worker exits after this long.
const WORKER_IDLE: Duration = Duration::from_secs(30);
/// Items a walk producer may run ahead of its consumer.
const STREAM_CAPACITY: usize = 256;

/// The primitive family of one call; it fixes the call's bound and whether an
/// abandoned call leaves an effect of unknown outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// metadata, symlink_metadata, canonicalize, exists, identity.
    Stat,
    /// Opening a file.
    Open,
    /// Reading bytes, or a whole parse over an open file.
    Read,
    /// Listing a directory or walking a tree (bounded per entry).
    List,
    /// Writing bytes to a private file.
    Write,
    /// Flushing a file or directory to the device.
    Sync,
    /// Renaming or publishing: the effect matters.
    Rename,
    /// Removing a file or directory: the effect matters.
    Remove,
    /// Creating a file or directory.
    Create,
    /// Registering a filesystem watch.
    Watch,
}

impl Op {
    pub fn bound(self) -> Duration {
        match self {
            Op::Stat | Op::Watch => QUICK_BOUND,
            _ => IO_BOUND,
        }
    }

    /// Whether giving up on this call leaves its effect unknown.
    pub fn has_effect(self) -> bool {
        matches!(
            self,
            Op::Write | Op::Sync | Op::Rename | Op::Remove | Op::Create
        )
    }

    fn name(self) -> &'static str {
        match self {
            Op::Stat => "stat",
            Op::Open => "open",
            Op::Read => "read",
            Op::List => "list",
            Op::Write => "write",
            Op::Sync => "sync",
            Op::Rename => "rename",
            Op::Remove => "remove",
            Op::Create => "create",
            Op::Watch => "watch",
        }
    }
}

/// The flush bound for `bytes` of freshly written data.
pub fn sync_bound(bytes: u64) -> Duration {
    IO_BOUND.max(Duration::from_secs(bytes / SYNC_BYTES_PER_SECOND))
}

/// Why a bounded call did not return its own result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitFailure {
    /// The call exceeded its bound and was abandoned.
    NotResponding,
    /// The caller cancelled while the call ran; it was abandoned.
    Cancelled,
    /// The call never started: an earlier call on this volume has not
    /// returned yet.
    Stalled,
}

/// The payload of an `io::Error` from this owner that is not the call's own
/// error. `volume` names the volume (a drive, share or mount) for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeWait {
    pub failure: WaitFailure,
    pub volume: String,
    pub op: Op,
}

impl std::fmt::Display for VolumeWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.failure {
            WaitFailure::NotResponding => write!(
                f,
                "the volume at {} is not responding (a {} call was given up on)",
                self.volume,
                self.op.name()
            ),
            WaitFailure::Cancelled => write!(
                f,
                "cancelled while a {} call on {} was still running",
                self.op.name(),
                self.volume
            ),
            WaitFailure::Stalled => write!(
                f,
                "the volume at {} is not responding (an earlier call has not returned)",
                self.volume
            ),
        }
    }
}

impl std::error::Error for VolumeWait {}

impl VolumeWait {
    fn into_io(self) -> io::Error {
        let kind = match self.failure {
            WaitFailure::Cancelled => io::ErrorKind::Interrupted,
            WaitFailure::NotResponding | WaitFailure::Stalled => io::ErrorKind::TimedOut,
        };
        io::Error::new(kind, self)
    }
}

/// This owner's reason, when `error` is not the call's own error.
pub fn wait_failure(error: &io::Error) -> Option<&VolumeWait> {
    error.get_ref()?.downcast_ref::<VolumeWait>()
}

/// Whether `error` means a call with an effect was given up on while it ran,
/// so the effect may or may not have happened.
pub fn outcome_unknown(error: &io::Error) -> bool {
    wait_failure(error).is_some_and(|wait| {
        wait.op.has_effect()
            && matches!(
                wait.failure,
                WaitFailure::NotResponding | WaitFailure::Cancelled
            )
    })
}

/// Whether `error` means the volume did not answer (now or earlier), as
/// opposed to the call failing or being cancelled.
pub fn is_not_responding(error: &io::Error) -> bool {
    wait_failure(error).is_some_and(|wait| {
        matches!(
            wait.failure,
            WaitFailure::NotResponding | WaitFailure::Stalled
        )
    })
}

// ---------------------------------------------------------------------------
// Volumes ("lanes")

/// The drive a call fails fast with, chosen without touching the filesystem.
///
/// A path's spelling alone cannot tell a network share from the startup disk:
/// SMB or NFS mounted under a home folder, an automount, or a folder reached
/// through a link all spell like the boot volume, and one stalled share would
/// then make every call on the startup disk fail fast. So each configured
/// source and destination root is its own drive (`register_roots`), in its
/// configured spelling and in the resolved spelling `canonicalize` finds for
/// it. Within a root the spelled volume (a drive letter or `\\server\share` on
/// Windows, `/Volumes/<name>` on macOS, otherwise the boot volume) still
/// splits it, so a root above several volumes never merges them. A path
/// outside every root falls back to its spelled volume. Two roots on one drive
/// are two lanes: a stall on one costs the other one bound before it fails
/// fast too, which is cheaper than any identification that could merge two
/// drives.
#[derive(Clone)]
struct Lane {
    key: String,
    /// The drive as the user knows it, for messages.
    display: String,
    fake: Option<Arc<FakeState>>,
}

fn lane_of(path: &Path) -> Lane {
    let registry = registry();
    if FAKES_MOUNTED.load(Ordering::Acquire) {
        if let Some((root, fake)) = registry
            .fakes
            .iter()
            .filter(|(root, _)| path.starts_with(root))
            .max_by_key(|(root, _)| root.components().count())
        {
            return Lane {
                key: format!("fake:{}", root.display()),
                display: root.display().to_string(),
                fake: Some(fake.clone()),
            };
        }
    }
    let volume = volume_key(path);
    match registry
        .roots
        .iter()
        .filter(|(spelling, _)| path.starts_with(spelling))
        .max_by_key(|(spelling, _)| spelling.components().count())
    {
        Some((spelling, root)) => Lane {
            key: format!("root:{root}|{volume}"),
            display: if spelling.starts_with(&volume) {
                root.clone()
            } else {
                volume
            },
            fake: None,
        },
        None => Lane {
            key: volume.clone(),
            display: volume,
            fake: None,
        },
    }
}

/// Makes each configured root its own drive for fail-fast. Registration only
/// adds: a root removed from the settings keeps its lane, which groups nothing
/// wrongly. The settings owner calls it whenever it reads or writes them.
pub fn register_roots(roots: &[PathBuf]) {
    let mut registry = registry();
    for root in roots {
        if !registry.roots.iter().any(|(spelling, _)| spelling == root) {
            registry
                .roots
                .push((root.clone(), root.display().to_string()));
        }
    }
}

/// Records `resolved` as another spelling of `path` when `path` is a
/// registered root, so entries a walk reports in the resolved spelling stay on
/// that root's lane.
fn note_resolved(path: &Path, resolved: &Path) {
    let mut registry = registry();
    let Some(root) = registry
        .roots
        .iter()
        .find(|(spelling, _)| spelling == path)
        .map(|(_, root)| root.clone())
    else {
        return;
    };
    if !registry.roots.iter().any(|(spelling, _)| spelling == resolved) {
        registry.roots.push((resolved.to_path_buf(), root));
    }
}

/// The lane a call on `path` fails fast with, for diagnostics and tests.
pub fn lane_name(path: &Path) -> String {
    lane_of(path).key
}

fn volume_key(path: &Path) -> String {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                format!("{}:", (letter as char).to_ascii_uppercase())
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                r"\\{}\{}",
                server.to_string_lossy().to_lowercase(),
                share.to_string_lossy().to_lowercase()
            ),
            _ => prefix.as_os_str().to_string_lossy().into_owned(),
        },
        Some(Component::RootDir) => {
            if cfg!(target_os = "macos") {
                if let (Some(Component::Normal(first)), Some(Component::Normal(name))) =
                    (components.next(), components.next())
                {
                    if first == "Volumes" {
                        return format!("/Volumes/{}", name.to_string_lossy());
                    }
                }
            }
            "/".to_string()
        }
        _ => "/".to_string(),
    }
}

struct Registry {
    /// Abandoned calls still outstanding, per lane.
    abandoned: HashMap<String, usize>,
    fakes: Vec<(PathBuf, Arc<FakeState>)>,
    /// Spellings of configured roots, each with the root it belongs to.
    roots: Vec<(PathBuf, String)>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| {
    Mutex::new(Registry {
        abandoned: HashMap::new(),
        fakes: Vec::new(),
        roots: Vec::new(),
    })
});
static FAKES_MOUNTED: AtomicBool = AtomicBool::new(false);

fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lane_stalled(lane: &Lane) -> bool {
    registry().abandoned.get(&lane.key).copied().unwrap_or(0) > 0
}

fn lane_settled(key: &str) {
    let mut registry = registry();
    if let Some(count) = registry.abandoned.get_mut(key) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            registry.abandoned.remove(key);
            drop(registry);
            crate::logging::info(
                "volume answered again",
                serde_json::json!({ "lane": key }),
            );
        }
    }
}

/// Whether a call on `path`'s volume would fail fast right now.
pub fn is_stalled(path: &Path) -> bool {
    lane_stalled(&lane_of(path))
}

// ---------------------------------------------------------------------------
// Pool

type Job = Box<dyn FnOnce() + Send + 'static>;

struct Pool {
    state: Mutex<PoolState>,
    ready: Condvar,
}

struct PoolState {
    queue: VecDeque<Job>,
    idle: usize,
    live: usize,
}

static POOL: LazyLock<Pool> = LazyLock::new(|| Pool {
    state: Mutex::new(PoolState {
        queue: VecDeque::new(),
        idle: 0,
        live: 0,
    }),
    ready: Condvar::new(),
});

thread_local! {
    static ON_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn on_worker() -> bool {
    ON_WORKER.with(std::cell::Cell::get)
}

fn pool_state() -> MutexGuard<'static, PoolState> {
    POOL.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn submit(job: Job) -> io::Result<()> {
    let mut state = pool_state();
    state.queue.push_back(job);
    if state.idle >= state.queue.len() {
        POOL.ready.notify_one();
        return Ok(());
    }
    state.live += 1;
    let number = state.live;
    drop(state);
    let spawned = std::thread::Builder::new()
        .name(format!("onecopy-volume-io-{number}"))
        .spawn(worker_loop);
    if let Err(error) = spawned {
        let mut state = pool_state();
        state.live -= 1;
        // The job stays queued for a live worker if there is one; with none,
        // take it back so the caller fails instead of waiting for nothing.
        if state.live == 0 {
            state.queue.pop_back();
        }
        return Err(error);
    }
    Ok(())
}

fn worker_loop() {
    ON_WORKER.with(|flag| flag.set(true));
    let mut state = pool_state();
    loop {
        if let Some(job) = state.queue.pop_front() {
            drop(state);
            // A panicking call must not take the worker's bookkeeping down.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            state = pool_state();
            continue;
        }
        state.idle += 1;
        let (next, timeout) = POOL
            .ready
            .wait_timeout(state, WORKER_IDLE)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
        state.idle -= 1;
        if timeout.timed_out() && state.queue.is_empty() {
            state.live -= 1;
            return;
        }
    }
}

/// Pool threads alive now, abandoned ones included.
pub fn live_workers() -> usize {
    pool_state().live
}

// ---------------------------------------------------------------------------
// Calls

#[derive(Clone, Copy, PartialEq, Eq)]
enum CallState {
    Running,
    Done,
    Abandoned,
}

/// Runs `f` on a pool thread and waits at most `op`'s bound for it. `cancel`
/// is polled while waiting; a cancelled call gets a short grace to finish on
/// its own before it is abandoned. Pass `None` for a short step that must be
/// allowed to finish within its bound once started (publication, a Move's
/// source cleanup).
pub fn call<T: Send + 'static>(
    path: &Path,
    op: Op,
    cancel: Option<&dyn Fn() -> bool>,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    call_bounded(path, op, op.bound(), cancel, f)
}

/// `call` with an explicit bound derived by the caller (a flush sized to the
/// bytes it must write).
pub fn call_bounded<T: Send + 'static>(
    path: &Path,
    op: Op,
    bound: Duration,
    cancel: Option<&dyn Fn() -> bool>,
    f: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    match call_with(path, op, bound, cancel, (), move |()| f()) {
        Ok(result) => result,
        Err((error, ())) => Err(error),
    }
}

/// `call_bounded` over `data`, which comes back untouched (`Err`) when the
/// call never started because its volume is stalled, so a resource it holds
/// is neither lost nor released on the caller's thread.
fn call_with<D: Send + 'static, T: Send + 'static>(
    path: &Path,
    op: Op,
    bound: Duration,
    cancel: Option<&dyn Fn() -> bool>,
    data: D,
    f: impl FnOnce(D) -> io::Result<T> + Send + 'static,
) -> Result<io::Result<T>, (io::Error, D)> {
    if on_worker() {
        return Ok(f(data));
    }
    let lane = lane_of(path);
    let wait = |failure| VolumeWait {
        failure,
        volume: lane.display.clone(),
        op,
    };
    if lane_stalled(&lane) {
        return Err((wait(WaitFailure::Stalled).into_io(), data));
    }
    let bound = lane.fake.as_ref().map_or(bound, |fake| fake.bound);
    let (sender, receiver) = mpsc::sync_channel::<io::Result<T>>(1);
    let state = Arc::new(Mutex::new(CallState::Running));
    let job_state = state.clone();
    let job_key = lane.key.clone();
    let fake = lane.fake.clone();
    let job_path = path.to_path_buf();
    if let Err(error) = submit(Box::new(move || {
        let result = match fake.and_then(|fake| fake.hold(op, &job_path)) {
            Some(failed) => Err(failed),
            None => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(data))) {
                Ok(result) => result,
                Err(_) => Err(io::Error::other("a filesystem call panicked")),
            },
        };
        let mut state = job_state.lock().unwrap_or_else(|p| p.into_inner());
        if *state == CallState::Abandoned {
            drop(state);
            // Settle: the undelivered result is dropped here, on this thread,
            // so whatever it owns cleans itself up without holding anyone.
            drop(result);
            lane_settled(&job_key);
        } else {
            *state = CallState::Done;
            drop(state);
            let _ = sender.send(result);
        }
    })) {
        return Ok(Err(error));
    }

    let started = Instant::now();
    let deadline = started + bound;
    let mut cancel_at: Option<Instant> = None;
    loop {
        match receiver.recv_timeout(POLL) {
            Ok(result) => return Ok(result),
            Err(RecvTimeoutError::Disconnected) => {
                return Ok(Err(io::Error::other(
                    "a filesystem call ended without a result",
                )))
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if cancel_at.is_none() && cancel.is_some_and(|cancelled| cancelled()) {
            cancel_at = Some(now + CANCEL_GRACE);
        }
        let failure = if cancel_at.is_some_and(|at| now >= at) {
            WaitFailure::Cancelled
        } else if now >= deadline {
            WaitFailure::NotResponding
        } else {
            continue;
        };
        let mut call_state = state.lock().unwrap_or_else(|p| p.into_inner());
        if *call_state == CallState::Done {
            drop(call_state);
            return Ok(receiver.recv().unwrap_or_else(|_| {
                Err(io::Error::other("a filesystem call ended without a result"))
            }));
        }
        *call_state = CallState::Abandoned;
        // Counted while the call's state is still held, so its settle can
        // never run before this increment.
        *registry().abandoned.entry(lane.key.clone()).or_insert(0) += 1;
        drop(call_state);
        let wait = wait(failure);
        crate::logging::warn(
            "filesystem call given up on",
            serde_json::json!({
                "volume": wait.volume,
                "op": op.name(),
                "path": path,
                "reason": match failure {
                    WaitFailure::Cancelled => "cancelled",
                    _ => "notResponding",
                },
                "waitedMs": started.elapsed().as_millis() as u64,
            }),
        );
        return Ok(Err(wait.into_io()));
    }
}

// ---------------------------------------------------------------------------
// Named primitives

fn fs_path(path: &Path) -> PathBuf {
    crate::winpath::for_fs(path).into_owned()
}

pub fn metadata(path: &Path) -> io::Result<Metadata> {
    let target = fs_path(path);
    call(path, Op::Stat, None, move || std::fs::metadata(target))
}

pub fn symlink_metadata(path: &Path) -> io::Result<Metadata> {
    let target = fs_path(path);
    call(path, Op::Stat, None, move || std::fs::symlink_metadata(target))
}

pub fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    let target = fs_path(path);
    let resolved = call(path, Op::Stat, None, move || std::fs::canonicalize(target))?;
    note_resolved(path, &resolved);
    Ok(resolved)
}

/// Whether `path` exists (following a final symlink). Only `NotFound` is
/// "no"; any other failure, a stall included, is an error the caller must not
/// read as absence.
pub fn exists(path: &Path) -> io::Result<bool> {
    match metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether `path` is a directory (following a final symlink); `NotFound` is
/// "no", any other failure an error.
pub fn is_dir(path: &Path) -> io::Result<bool> {
    match metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn create_dir(path: &Path) -> io::Result<()> {
    let target = fs_path(path);
    call(path, Op::Create, None, move || std::fs::create_dir(target))
}

pub fn create_dir_all(path: &Path) -> io::Result<()> {
    let target = fs_path(path);
    call(path, Op::Create, None, move || std::fs::create_dir_all(target))
}

pub fn remove_file(path: &Path) -> io::Result<()> {
    let target = fs_path(path);
    call(path, Op::Remove, None, move || std::fs::remove_file(target))
}

pub fn remove_dir(path: &Path) -> io::Result<()> {
    let target = fs_path(path);
    call(path, Op::Remove, None, move || std::fs::remove_dir(target))
}

/// Flushes a directory's entries to the device. Windows has no portable
/// directory handle; its no-replace move is journaled by the filesystem.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let target = fs_path(path);
        call(path, Op::Sync, None, move || File::open(target)?.sync_all())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// One directory entry as a listing saw it.
#[derive(Debug)]
pub struct DirEntryInfo {
    pub path: PathBuf,
    pub file_name: std::ffi::OsString,
    /// The entry's own type (a symlink is a symlink); `None` when unreadable.
    pub file_type: Option<FileType>,
    /// Metadata following a final symlink, when the listing asked for it.
    pub metadata: Option<io::Result<Metadata>>,
}

/// Lists a directory in one bounded call. `with_metadata` also reads each
/// entry's metadata inside the same call.
pub fn read_dir(path: &Path, with_metadata: bool) -> io::Result<Vec<DirEntryInfo>> {
    let target = fs_path(path);
    call(path, Op::List, None, move || {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&target)? {
            let entry = entry?;
            entries.push(DirEntryInfo {
                path: entry.path(),
                file_name: entry.file_name(),
                file_type: entry.file_type().ok(),
                metadata: with_metadata.then(|| entry.metadata()),
            });
        }
        Ok(entries)
    })
}

/// Reads a whole small file in one bounded call.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let target = fs_path(path);
    call(path, Op::Read, None, move || std::fs::read(target))
}

/// Opens `path` for reading.
pub fn open_read(path: &Path) -> io::Result<VolumeFile> {
    let target = fs_path(path);
    let file = call(path, Op::Open, None, move || File::open(target))?;
    Ok(VolumeFile::new(file, path.to_path_buf()))
}

/// Creates `path` exclusively for reading and writing; an existing entry
/// answers `AlreadyExists`.
pub fn create_new(path: &Path) -> io::Result<VolumeFile> {
    let target = fs_path(path);
    let file = call(path, Op::Create, None, move || {
        File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(target)
    })?;
    Ok(VolumeFile::new(file, path.to_path_buf()))
}

/// Renames `from` to `to`, replacing an existing `to` where the platform
/// does.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let (source, target) = (fs_path(from), fs_path(to));
    call(to, Op::Rename, None, move || std::fs::rename(source, target))
}

/// Appends `bytes` to `path` (creating it) and flushes, in one bounded call.
pub fn append_synced(path: &Path, bytes: Vec<u8>) -> io::Result<()> {
    let target = fs_path(path);
    call(path, Op::Write, None, move || {
        let mut file = File::options().create(true).append(true).open(target)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })
}

// ---------------------------------------------------------------------------
// Open files

/// An open file on a user volume whose every read, write, seek, flush and
/// metadata call is a bounded call. It implements `Read + Seek + Write`, so a
/// parser or hasher takes it unchanged. A call given up on takes the file
/// handle with it (it closes on the abandoned thread once the call returns),
/// and every later call answers an error.
pub struct VolumeFile {
    file: Option<File>,
    path: PathBuf,
    buffer: Vec<u8>,
    settle: Option<Arc<dyn Fn(File) + Send + Sync>>,
}

impl std::fmt::Debug for VolumeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VolumeFile")
            .field("path", &self.path)
            .field("open", &self.file.is_some())
            .finish()
    }
}

/// Carries an open file through one call. If the call was abandoned, the
/// carrier is dropped on the worker when the call returns and hands the file
/// to its settle action (a private file removes itself there).
struct Carrier {
    file: Option<File>,
    settle: Option<Arc<dyn Fn(File) + Send + Sync>>,
}

impl Drop for Carrier {
    fn drop(&mut self) {
        if let (Some(file), Some(settle)) = (self.file.take(), self.settle.take()) {
            settle(file);
        }
    }
}

impl VolumeFile {
    /// Wraps a file opened inside a bounded call.
    pub fn new(file: File, path: PathBuf) -> Self {
        Self {
            file: Some(file),
            path,
            buffer: Vec::new(),
            settle: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this handle still holds its file (no call on it was given up
    /// on).
    pub fn is_open(&self) -> bool {
        self.file.is_some()
    }

    /// What to do with the file if a call on it is abandoned and later
    /// returns: runs on the worker, where calls run inline.
    pub fn set_settle(&mut self, settle: Arc<dyn Fn(File) + Send + Sync>) {
        self.settle = Some(settle);
    }

    fn lost(&self) -> io::Error {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "{} was given up on with a call that did not return",
                self.path.display()
            ),
        )
    }

    /// Runs `f` with the open file on a pool thread under `op`'s bound (or
    /// `bound` when given).
    pub fn with<R: Send + 'static>(
        &mut self,
        op: Op,
        bound: Option<Duration>,
        cancel: Option<&dyn Fn() -> bool>,
        f: impl FnOnce(&mut File) -> io::Result<R> + Send + 'static,
    ) -> io::Result<R> {
        let carrier = Carrier {
            file: Some(self.file.take().ok_or_else(|| self.lost())?),
            settle: self.settle.clone(),
        };
        let result = call_with(
            &self.path,
            op,
            bound.unwrap_or_else(|| op.bound()),
            cancel,
            carrier,
            move |mut carrier| {
                let result = f(carrier.file.as_mut().expect("carrier holds the file"));
                Ok((carrier, result))
            },
        );
        match result {
            Ok(Ok((mut carrier, result))) => {
                self.file = carrier.file.take();
                result
            }
            Ok(Err(error)) => Err(error),
            // Never started: the file is still ours.
            Err((error, mut carrier)) => {
                self.file = carrier.file.take();
                Err(error)
            }
        }
    }

    pub fn metadata(&mut self) -> io::Result<Metadata> {
        self.with(Op::Stat, None, None, |file| file.metadata())
    }

    /// Flushes `bytes` of written data; the bound grows with the size.
    /// `cancel` ends the wait (a flush of a large file can legitimately take
    /// many minutes, far longer than a user waits on Cancel).
    pub fn sync_all(&mut self, bytes: u64, cancel: Option<&dyn Fn() -> bool>) -> io::Result<()> {
        self.with(Op::Sync, Some(sync_bound(bytes)), cancel, |file| file.sync_all())
    }

    /// One bounded read of up to `buf.len()` bytes.
    pub fn read_cancellable(
        &mut self,
        buf: &mut [u8],
        cancel: Option<&dyn Fn() -> bool>,
    ) -> io::Result<usize> {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.resize(buf.len(), 0);
        let (buffer, read) = self.with(Op::Read, None, cancel, move |file| {
            let mut buffer = buffer;
            let read = file.read(&mut buffer)?;
            Ok((buffer, read))
        })?;
        buf[..read].copy_from_slice(&buffer[..read]);
        self.buffer = buffer;
        Ok(read)
    }

    /// One bounded write of all of `data`.
    pub fn write_all_cancellable(
        &mut self,
        data: &[u8],
        cancel: Option<&dyn Fn() -> bool>,
    ) -> io::Result<()> {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.clear();
        buffer.extend_from_slice(data);
        let buffer = self.with(Op::Write, None, cancel, move |file| {
            file.write_all(&buffer)?;
            Ok(buffer)
        })?;
        self.buffer = buffer;
        Ok(())
    }
}

impl Read for VolumeFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.read_cancellable(buf, None)
    }
}

impl Seek for VolumeFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.with(Op::Read, None, None, move |file| file.seek(position))
    }
}

impl Write for VolumeFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_all_cancellable(buf, None)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for VolumeFile {
    fn drop(&mut self) {
        // Closing can wait on the device too; close on a pool thread. On a
        // stalled volume the close is left to a pool thread without waiting.
        if let Some(file) = self.file.take() {
            if let Err((_, file)) =
                call_with(&self.path, Op::Read, IO_BOUND, None, file, |file| {
                    drop(file);
                    Ok(())
                })
            {
                let _ = submit(Box::new(move || drop(file)));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Walks

/// One entry of a walk.
#[derive(Debug)]
pub struct WalkEntry {
    pub path: PathBuf,
    pub file_type: FileType,
    pub depth: usize,
    /// For a regular file, its metadata (following nothing: a regular file is
    /// what it is), read on the walk's thread.
    pub metadata: Option<io::Result<Metadata>>,
}

/// A walk error: the path it concerned, when known, and the message.
#[derive(Debug)]
pub struct WalkError {
    pub path: Option<PathBuf>,
    pub message: String,
}

enum WalkMessage {
    Entry(Result<WalkEntry, WalkError>),
    /// The walk ended; `Err` when its thread failed or panicked.
    End(Result<(), String>),
    /// The walk stopped early because its consumer cancelled it.
    Stopped,
}

/// A walk of a directory tree running on a pool thread, consumed on the
/// caller's thread. Each `next` waits at most `IO_BOUND` for the next entry;
/// a walk that goes quiet longer is abandoned and its volume marked stalled
/// until the walk's thread returns. Dropping the walk early stops it at its
/// next entry.
pub struct Walk {
    receiver: mpsc::Receiver<WalkMessage>,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<CallState>>,
    lane: Lane,
    bound: Duration,
    finished: bool,
    /// The walk's thread returned while a wait was giving up on it; the rest
    /// of its messages are queued and are delivered without waiting.
    producer_done: bool,
}

/// Starts walking `root` without following symlinks. `filter` decides, on
/// the walk's thread, whether to yield an entry and descend into it; calls it
/// makes run inline there.
pub fn walk(
    root: &Path,
    filter: impl Fn(&Path, &FileType) -> bool + Send + 'static,
) -> io::Result<Walk> {
    let lane = lane_of(root);
    if lane_stalled(&lane) {
        return Err(VolumeWait {
            failure: WaitFailure::Stalled,
            volume: lane.display.clone(),
            op: Op::List,
        }
        .into_io());
    }
    let bound = lane.fake.as_ref().map_or(IO_BOUND, |fake| fake.bound);
    let (sender, receiver) = mpsc::sync_channel::<WalkMessage>(STREAM_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(CallState::Running));
    let producer_stop = stop.clone();
    let producer_state = state.clone();
    let key = lane.key.clone();
    let fake = lane.fake.clone();
    let walk_root = fs_path(root);
    submit(Box::new(move || {
        let produced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for entry in walkdir::WalkDir::new(&walk_root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|entry| filter(entry.path(), &entry.file_type()))
            {
                if producer_stop.load(Ordering::Acquire) {
                    return Ok(false);
                }
                let message = match entry {
                    Ok(entry) => {
                        if let Some(error) =
                            fake.as_ref().and_then(|fake| fake.hold(Op::List, entry.path()))
                        {
                            Err(WalkError {
                                path: Some(entry.path().to_path_buf()),
                                message: error.to_string(),
                            })
                        } else {
                            let metadata = entry
                                .file_type()
                                .is_file()
                                .then(|| entry.metadata().map_err(io::Error::from));
                            Ok(WalkEntry {
                                path: entry.path().to_path_buf(),
                                file_type: entry.file_type(),
                                depth: entry.depth(),
                                metadata,
                            })
                        }
                    }
                    Err(error) => Err(WalkError {
                        path: error.path().map(Path::to_path_buf),
                        message: error.to_string(),
                    }),
                };
                if sender.send(WalkMessage::Entry(message)).is_err() {
                    return Ok(false);
                }
            }
            Ok(true)
        }));
        // A walk cut short is never reported as a complete one: a consumer
        // must not read absence into entries it never received.
        let end = match produced {
            Ok(Ok(true)) => WalkMessage::End(Ok(())),
            Ok(Ok(false)) => WalkMessage::Stopped,
            Ok(Err(error)) => WalkMessage::End(Err(error)),
            Err(_) => WalkMessage::End(Err("the walk panicked".to_string())),
        };
        let mut state = producer_state.lock().unwrap_or_else(|p| p.into_inner());
        if *state == CallState::Abandoned {
            drop(state);
            lane_settled(&key);
        } else {
            *state = CallState::Done;
            drop(state);
            let _ = sender.send(end);
        }
    }))?;
    Ok(Walk {
        receiver,
        stop,
        state,
        lane,
        bound,
        finished: false,
        producer_done: false,
    })
}

impl Walk {
    /// The next entry, `None` at the end. An `Err` whose `io::Error` carries
    /// a `VolumeWait` ends the walk: it was abandoned or cancelled.
    pub fn next(
        &mut self,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Option<io::Result<Result<WalkEntry, WalkError>>> {
        if self.finished {
            return None;
        }
        if self.producer_done {
            // The walk's thread has returned: everything it produced, its end
            // included, is already queued.
            let message = self.receiver.recv().ok();
            return self.deliver(message);
        }
        let started = Instant::now();
        let deadline = started + self.bound;
        let mut cancel_at: Option<Instant> = None;
        loop {
            match self.receiver.recv_timeout(POLL) {
                Ok(message) => return self.deliver(Some(message)),
                Err(RecvTimeoutError::Disconnected) => return self.deliver(None),
                Err(RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if cancel_at.is_none() && cancel.is_some_and(|cancelled| cancelled()) {
                cancel_at = Some(now + CANCEL_GRACE);
                self.stop.store(true, Ordering::Release);
            }
            let failure = if cancel_at.is_some_and(|at| now >= at) {
                WaitFailure::Cancelled
            } else if now >= deadline {
                WaitFailure::NotResponding
            } else {
                continue;
            };
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if *state == CallState::Done {
                // The walk finished between the last poll and this check: its
                // remaining entries and its end are queued, never dropped, so
                // a walk cut short is still never read as a complete one.
                drop(state);
                self.producer_done = true;
                let message = self.receiver.recv().ok();
                return self.deliver(message);
            }
            self.finished = true;
            self.stop.store(true, Ordering::Release);
            *state = CallState::Abandoned;
            *registry()
                .abandoned
                .entry(self.lane.key.clone())
                .or_insert(0) += 1;
            drop(state);
            let wait = VolumeWait {
                failure,
                volume: self.lane.display.clone(),
                op: Op::List,
            };
            crate::logging::warn(
                "directory walk given up on",
                serde_json::json!({
                    "volume": wait.volume,
                    "reason": match failure {
                        WaitFailure::Cancelled => "cancelled",
                        _ => "notResponding",
                    },
                    "waitedMs": started.elapsed().as_millis() as u64,
                }),
            );
            return Some(Err(wait.into_io()));
        }
    }

    /// Hands one message from the walk's thread to the consumer.
    fn deliver(
        &mut self,
        message: Option<WalkMessage>,
    ) -> Option<io::Result<Result<WalkEntry, WalkError>>> {
        match message {
            Some(WalkMessage::Entry(entry)) => Some(Ok(entry)),
            Some(WalkMessage::End(Ok(()))) => {
                self.finished = true;
                None
            }
            Some(WalkMessage::End(Err(message))) => {
                self.finished = true;
                Some(Err(io::Error::other(message)))
            }
            Some(WalkMessage::Stopped) => {
                self.finished = true;
                Some(Err(VolumeWait {
                    failure: WaitFailure::Cancelled,
                    volume: self.lane.display.clone(),
                    op: Op::List,
                }
                .into_io()))
            }
            None => {
                self.finished = true;
                None
            }
        }
    }
}

impl Drop for Walk {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// A fake stalling volume: the test seam

struct FakeState {
    bound: Duration,
    gate: Mutex<FakeGate>,
    changed: Condvar,
    held: AtomicUsize,
}

#[derive(Default)]
struct FakeGate {
    /// Ops that stall (empty: every op), an optional path prefix, and how
    /// many matching calls still pass before the stall begins.
    stalling: Option<(Vec<Op>, Option<PathBuf>, usize)>,
    /// Bumped by each release; a held call leaves when it changes.
    generation: u64,
    /// How the last release let held calls go: run them, or fail them.
    fail: bool,
}

impl FakeState {
    /// Blocks a matching call until released. `Some`: the release failed it.
    fn hold(&self, op: Op, path: &Path) -> Option<io::Error> {
        let mut gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        let Some((ops, prefix, passing)) = gate.stalling.as_mut() else {
            return None;
        };
        if !(ops.is_empty() || ops.contains(&op))
            || !prefix.as_ref().is_none_or(|prefix| path.starts_with(prefix))
        {
            return None;
        }
        if *passing > 0 {
            *passing -= 1;
            return None;
        }
        let generation = gate.generation;
        self.held.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_all();
        while gate.generation == generation {
            gate = self
                .changed
                .wait(gate)
                .unwrap_or_else(|p| p.into_inner());
        }
        self.held.fetch_sub(1, Ordering::SeqCst);
        gate.fail
            .then(|| io::Error::other("the fake volume failed the call"))
    }
}

/// A volume that stalls on demand, for tests: every path beneath `root` is its
/// own volume with its own `bound`, and `stall` makes matching calls block in
/// their pool thread (exactly as a kernel call on a dead share would) until
/// `release` runs them or `fail` fails them.
pub struct FakeStallingVolume {
    root: PathBuf,
    state: Arc<FakeState>,
}

impl FakeStallingVolume {
    pub fn mount(root: &Path, bound: Duration) -> Self {
        let state = Arc::new(FakeState {
            bound,
            gate: Mutex::new(FakeGate::default()),
            changed: Condvar::new(),
            held: AtomicUsize::new(0),
        });
        let mut registry = registry();
        registry.fakes.push((root.to_path_buf(), state.clone()));
        FAKES_MOUNTED.store(true, Ordering::Release);
        Self {
            root: root.to_path_buf(),
            state,
        }
    }

    /// From now on, calls of `ops` (every op when empty) beneath `under`
    /// (anywhere on the volume when `None`) stall until released.
    pub fn stall(&self, ops: &[Op], under: Option<&Path>) {
        self.stall_after(ops, under, 0);
    }

    /// Like `stall`, but the first `passing` matching calls still run.
    pub fn stall_after(&self, ops: &[Op], under: Option<&Path>, passing: usize) {
        let mut gate = self.state.gate.lock().unwrap_or_else(|p| p.into_inner());
        gate.stalling = Some((ops.to_vec(), under.map(Path::to_path_buf), passing));
    }

    /// Stops stalling and runs every held call.
    pub fn release(&self) {
        self.open(false);
    }

    /// Stops stalling and fails every held call without running it.
    pub fn fail(&self) {
        self.open(true);
    }

    fn open(&self, fail: bool) {
        let mut gate = self.state.gate.lock().unwrap_or_else(|p| p.into_inner());
        gate.stalling = None;
        gate.fail = fail;
        gate.generation += 1;
        self.state.changed.notify_all();
    }

    /// Calls blocked right now.
    pub fn held(&self) -> usize {
        self.state.held.load(Ordering::SeqCst)
    }

    /// Waits until at least `count` calls are held.
    pub fn wait_until_held(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut gate = self.state.gate.lock().unwrap_or_else(|p| p.into_inner());
        while self.held() < count {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            gate = self
                .state
                .changed
                .wait_timeout(gate, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        true
    }

    /// Whether this volume has an abandoned call outstanding.
    pub fn is_stalled(&self) -> bool {
        is_stalled(&self.root)
    }

    /// Waits until this volume's abandoned calls have all returned.
    pub fn wait_until_settled(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.is_stalled() {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }
}

impl Drop for FakeStallingVolume {
    fn drop(&mut self) {
        self.release();
        let mut registry = registry();
        registry
            .fakes
            .retain(|(_, state)| !Arc::ptr_eq(state, &self.state));
        if registry.fakes.is_empty() {
            FAKES_MOUNTED.store(false, Ordering::Release);
        }
    }
}
