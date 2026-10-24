use super::*;
use std::thread;

/// Each test owns its workers; fault injection applies only to its caller.
struct TestPool(Arc<Pool>);

impl TestPool {
    fn new() -> Self {
        Self(Arc::new(Pool {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                idle: 0,
                live: 0,
                closed: false,
            }),
            ready: Condvar::new(),
        }))
    }

    fn hold_worker(&self) -> mpsc::Sender<()> {
        let (started, running) = mpsc::channel();
        let (release, held) = mpsc::channel();
        self.0
            .submit(Box::new(move || {
                started.send(()).unwrap();
                let _ = held.recv();
            }))
            .unwrap();
        running.recv_timeout(Duration::from_secs(2)).unwrap();
        release
    }
}

impl Drop for TestPool {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.closed = true;
        self.0.ready.notify_all();
        while state.live != 0 {
            state = self.0.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        assert!(state.queue.is_empty());
    }
}

struct PoolScope {
    previous: Option<Arc<Pool>>,
    previous_failure: bool,
    previous_attempt: Option<mpsc::Sender<()>>,
}

impl PoolScope {
    fn failing(pool: Arc<Pool>, attempt: Option<mpsc::Sender<()>>) -> Self {
        Self {
            previous: TEST_POOL.with(|current| current.replace(Some(pool))),
            previous_failure: TEST_SPAWN_FAILURE.with(|failure| failure.replace(true)),
            previous_attempt: TEST_SPAWN_ATTEMPT.with(|current| current.replace(attempt)),
        }
    }
}

impl Drop for PoolScope {
    fn drop(&mut self) {
        TEST_POOL.with(|current| current.replace(self.previous.take()));
        TEST_SPAWN_FAILURE.with(|failure| failure.set(self.previous_failure));
        TEST_SPAWN_ATTEMPT.with(|current| current.replace(self.previous_attempt.take()));
    }
}

#[test]
fn failed_worker_expansion_keeps_a_rename_pending_until_it_settles() {
    let pool = TestPool::new();
    let release = pool.hold_worker();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let target = root.path().join("target");
    std::fs::write(&source, b"owned mutation").unwrap();
    let (done, result) = mpsc::channel();
    let (attempt, attempted) = mpsc::channel();
    let caller_pool = pool.0.clone();
    let from = source.clone();
    let to = target.clone();
    let caller = thread::spawn(move || {
        let _scope = PoolScope::failing(caller_pool, Some(attempt));
        let path = from.clone();
        let renamed = call_bounded(&path, Op::Rename, Duration::from_secs(2), None, move || {
            std::fs::rename(from, to)
        });
        done.send(renamed).unwrap();
    });
    attempted.recv_timeout(Duration::from_secs(2)).unwrap();
    let pending = matches!(
        result.recv_timeout(Duration::from_millis(100)),
        Err(RecvTimeoutError::Timeout)
    );
    release.send(()).unwrap();
    let renamed = if pending {
        Some(result.recv_timeout(Duration::from_secs(2)).unwrap())
    } else {
        None
    };
    caller.join().unwrap();
    assert!(
        pending,
        "worker expansion reported failure while its rename was still queued"
    );
    renamed.unwrap().unwrap();
    assert!(!source.exists());
    assert_eq!(std::fs::read(target).unwrap(), b"owned mutation");
}

#[test]
fn failed_worker_expansion_retains_unknown_rename_and_close_accounting() {
    let pool = TestPool::new();
    let release = pool.hold_worker();
    let _scope = PoolScope::failing(pool.0.clone(), None);
    let root = tempfile::tempdir().unwrap();
    let fake = FakeStallingVolume::mount(root.path(), Duration::ZERO);
    fake.stall(&[Op::Read], None);
    let source = root.path().join("source");
    let target = root.path().join("target");
    std::fs::write(&source, b"late mutation").unwrap();
    let from = source.clone();
    let to = target.clone();
    let error = call(&source, Op::Rename, None, move || std::fs::rename(from, to)).unwrap_err();
    let unknown = outcome_unknown(&error);
    abandon_now(root.path(), Op::Read, (), |_| {});
    let lane = lane_name(root.path());
    let counted = registry().abandoned.get(&lane).copied().unwrap_or(0);
    release.send(()).unwrap();
    assert!(fake.wait_until_held(1, Duration::from_secs(2)));
    let outstanding_close = registry().abandoned.get(&lane).copied().unwrap_or(0);
    fake.release();
    assert!(fake.wait_until_settled(Duration::from_secs(2)));
    assert!(
        unknown,
        "the admitted rename must report an unknown timeout outcome: {error}"
    );
    assert_eq!(counted, 2, "rename and close each own settlement");
    assert_eq!(
        outstanding_close, 1,
        "late rename cannot settle the queued close"
    );
    assert!(!source.exists());
    assert_eq!(std::fs::read(target).unwrap(), b"late mutation");
}

#[test]
fn failed_first_worker_rejects_a_rename_without_enqueuing_or_effects() {
    let pool = TestPool::new();
    let _scope = PoolScope::failing(pool.0.clone(), None);
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let target = root.path().join("target");
    std::fs::write(&source, b"preserved").unwrap();
    let from = source.clone();
    let to = target.clone();
    let error = call(&source, Op::Rename, None, move || std::fs::rename(from, to)).unwrap_err();
    assert!(!outcome_unknown(&error));
    let state = pool.0.state();
    assert!(state.queue.is_empty());
    assert_eq!(state.live, 0);
    drop(state);
    assert_eq!(std::fs::read(source).unwrap(), b"preserved");
    assert!(!target.exists());
}

#[test]
fn a_directory_probe_stops_before_reading_later_entries_and_metadata() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("child")).unwrap();
    let entries = directory_entries(root.path(), true).unwrap()
        .chain(std::iter::once_with(|| panic!("an answered probe read another directory entry")));
    assert_eq!(probe_directory_entries(entries, &AtomicBool::new(false), |entry| {
        assert!(entry.metadata.as_ref().unwrap().as_ref().unwrap().is_dir());
        Ok(entry.file_type.unwrap().is_dir())
    }).unwrap(), (true, false));
}

#[test]
fn a_timed_out_directory_probe_stops_after_its_current_entry_settles() {
    let root = tempfile::tempdir().unwrap();
    for name in ["first", "second", "third"] {
        std::fs::write(root.path().join(name), b"entry").unwrap();
    }
    let volume = FakeStallingVolume::mount(root.path(), Duration::from_millis(200));
    let (entered, observed) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let visits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = visits.clone();
    let path = root.path().to_path_buf();
    let caller = std::thread::spawn(move || probe_dir(&path, move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        entered.send(()).unwrap();
        held.recv().unwrap();
        Ok(false)
    }));
    observed.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(caller.join().unwrap().is_err());
    assert!(is_stalled(root.path()));
    release.send(()).unwrap();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
    assert_eq!(visits.load(Ordering::SeqCst), 1);
    assert!(!is_stalled(root.path()));
}
