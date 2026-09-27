// The bounded filesystem owner (volume_io): every primitive's timeout and
// abandon path against a fake stalling volume, fail-fast per volume, unknown
// outcomes, and other volumes working while one stalls.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use onecopy_lib::volume_io::{self, FakeStallingVolume, Op, WaitFailure};

const BOUND: Duration = Duration::from_millis(200);
/// Scheduling slack on a loaded test machine; still far below any real bound.
const SLACK: Duration = Duration::from_millis(800);

fn failure(error: &std::io::Error) -> Option<WaitFailure> {
    volume_io::wait_failure(error).map(|wait| wait.failure)
}

#[test]
fn a_stalled_call_is_given_up_on_within_its_bound_and_the_volume_recovers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"x").unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    volume.stall(&[], None);

    let started = Instant::now();
    let error = volume_io::metadata(&dir.path().join("a")).unwrap_err();
    assert!(started.elapsed() < BOUND + SLACK, "waited {:?}", started.elapsed());
    assert_eq!(failure(&error), Some(WaitFailure::NotResponding));
    assert!(volume_io::is_not_responding(&error));
    assert!(!volume_io::outcome_unknown(&error), "a stat has no effect");
    assert!(volume.is_stalled());

    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
    assert!(volume_io::metadata(&dir.path().join("a")).is_ok());
}

#[test]
fn a_stalled_volume_fails_fast_without_starting_another_call() {
    let dir = tempfile::tempdir().unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    volume.stall(&[], None);
    assert!(volume_io::exists(&dir.path().join("a")).is_err());
    assert_eq!(volume.held(), 1);

    for _ in 0..20 {
        let started = Instant::now();
        let error = volume_io::symlink_metadata(&dir.path().join("b")).unwrap_err();
        assert_eq!(failure(&error), Some(WaitFailure::Stalled));
        assert!(started.elapsed() < Duration::from_millis(100));
    }
    // Only the first call ever reached a thread.
    assert_eq!(volume.held(), 1);
    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
}

#[test]
fn cancel_returns_promptly_while_a_call_is_stalled() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), vec![7u8; 1024]).unwrap();
    let mut file = volume_io::open_read(&dir.path().join("a")).unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), Duration::from_secs(60));
    volume.stall(&[Op::Read], None);
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        flag.store(true, Ordering::SeqCst);
    });

    let started = Instant::now();
    let mut buf = [0u8; 16];
    let error = file
        .read_cancellable(&mut buf, Some(&|| cancelled.load(Ordering::SeqCst)))
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_millis(50) + SLACK);
    assert_eq!(failure(&error), Some(WaitFailure::Cancelled));
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    // The handle went with the abandoned call.
    assert!(file.read(&mut buf).is_err());
    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
}

#[test]
fn other_volumes_keep_working_while_one_stalls() {
    let stalled_dir = tempfile::tempdir().unwrap();
    let healthy_dir = tempfile::tempdir().unwrap();
    std::fs::write(healthy_dir.path().join("h"), b"healthy").unwrap();
    let stalled = FakeStallingVolume::mount(stalled_dir.path(), Duration::from_secs(60));
    let _healthy = FakeStallingVolume::mount(healthy_dir.path(), BOUND);
    stalled.stall(&[], None);

    let blocked_path = stalled_dir.path().join("x");
    let blocked = std::thread::spawn(move || volume_io::exists(&blocked_path).is_ok());
    assert!(stalled.wait_until_held(1, Duration::from_secs(5)));

    let started = Instant::now();
    assert_eq!(volume_io::read(&healthy_dir.path().join("h")).unwrap(), b"healthy");
    let listed = volume_io::read_dir(healthy_dir.path(), true).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(started.elapsed() < Duration::from_secs(2));

    stalled.release();
    assert!(blocked.join().unwrap(), "released in time, the call answers");
}

#[test]
fn an_abandoned_rename_has_an_unknown_outcome_that_settles_later() {
    let dir = tempfile::tempdir().unwrap();
    let from = dir.path().join("from");
    let to = dir.path().join("to");
    std::fs::write(&from, b"bytes").unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    volume.stall(&[Op::Rename], None);

    let (source, target) = (from.clone(), to.clone());
    let error = volume_io::call(&to, Op::Rename, None, move || std::fs::rename(&source, &target))
        .unwrap_err();
    assert!(volume_io::outcome_unknown(&error));
    assert!(from.exists() && !to.exists(), "nothing has happened yet");

    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
    assert!(!from.exists() && to.exists(), "the given-up rename landed late");
}

#[test]
fn an_abandoned_open_file_settles_on_its_worker_when_the_call_returns() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"abc").unwrap();
    let mut file = volume_io::open_read(&dir.path().join("a")).unwrap();
    let settled = Arc::new(AtomicBool::new(false));
    let flag = settled.clone();
    file.set_settle(Arc::new(move |_file| flag.store(true, Ordering::SeqCst)));
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    volume.stall(&[Op::Read], None);

    let mut buf = [0u8; 3];
    assert!(volume_io::is_not_responding(&file.read(&mut buf).unwrap_err()));
    assert!(!settled.load(Ordering::SeqCst));
    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
    assert!(settled.load(Ordering::SeqCst));
}

#[test]
fn a_walk_that_goes_quiet_is_abandoned_after_the_entries_it_produced() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("fine")).unwrap();
    std::fs::write(dir.path().join("fine").join("f"), b"1").unwrap();
    std::fs::create_dir(dir.path().join("zz-stalls")).unwrap();
    std::fs::write(dir.path().join("zz-stalls").join("g"), b"2").unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    volume.stall(&[Op::List], Some(&dir.path().join("zz-stalls")));

    let mut walk = volume_io::walk(dir.path(), |_, _| true).unwrap();
    let mut seen = Vec::new();
    let mut ended = None;
    while let Some(next) = walk.next(None) {
        match next {
            Ok(Ok(entry)) => seen.push(entry.path),
            Ok(Err(error)) => panic!("unexpected walk error {}", error.message),
            Err(error) => {
                ended = Some(error);
                break;
            }
        }
    }
    let error = ended.expect("the walk must be given up on");
    assert_eq!(failure(&error), Some(WaitFailure::NotResponding));
    assert!(seen.iter().all(|path| !path.starts_with(dir.path().join("zz-stalls"))));
    assert!(volume.is_stalled());
    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
}

#[test]
fn a_walk_reads_file_metadata_and_honours_its_filter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("skip")).unwrap();
    std::fs::write(dir.path().join("skip").join("hidden"), b"1").unwrap();
    std::fs::write(dir.path().join("kept"), b"12345").unwrap();
    let mut walk = volume_io::walk(dir.path(), |path, _| {
        path.file_name().is_none_or(|name| name != "skip")
    })
    .unwrap();
    let mut files = Vec::new();
    while let Some(next) = walk.next(None) {
        let entry = next.unwrap().unwrap();
        if entry.file_type.is_file() {
            files.push((entry.path.clone(), entry.metadata.unwrap().unwrap().len()));
        }
    }
    assert_eq!(files, vec![(dir.path().join("kept"), 5)]);
}

/// A real kernel wait, no fake: opening a FIFO for reading blocks until a
/// writer appears, exactly like a call into a dead share.
#[cfg(unix)]
#[test]
fn a_real_blocking_open_is_abandoned_within_its_bound() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    // Mounted only for its short bound; nothing is stalled by the fake.
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    let started = Instant::now();
    let error = volume_io::open_read(&fifo).unwrap_err();
    assert!(started.elapsed() < BOUND + SLACK);
    assert_eq!(failure(&error), Some(WaitFailure::NotResponding));
    assert!(volume.is_stalled());
    // Give the blocked open its writer so the abandoned thread returns.
    let _writer = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
}

#[test]
fn volumes_are_told_apart_by_spelling_alone() {
    // Two paths on the boot volume share its fail-fast; the key never touches
    // the filesystem, so a path that does not exist still has a volume.
    assert!(!volume_io::is_stalled(Path::new("/definitely/not/here")));
}
