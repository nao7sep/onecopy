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
fn closing_a_file_on_an_already_stalled_volume_counts_as_outstanding_without_its_own_timeout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a"), b"abc").unwrap();
    let other = dir.path().join("b");
    std::fs::write(&other, b"x").unwrap();
    let file = volume_io::open_read(&dir.path().join("a")).unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), BOUND);
    volume.stall(&[], None);

    // One call abandons first, so the lane is already known stalled before
    // the file is closed.
    assert!(volume_io::metadata(&other).is_err());
    assert!(volume.is_stalled());
    assert_eq!(volume.held(), 1);

    let started = Instant::now();
    drop(file);
    assert!(started.elapsed() < Duration::from_millis(100), "closing must not wait out a fresh bound");

    // The close is a second outstanding call on the same lane, gated by the
    // same fake, not a fire-and-forget one the registry never counted.
    assert!(volume.wait_until_held(2, Duration::from_secs(5)));

    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
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
fn a_lane_never_touches_the_filesystem() {
    // A path that does not exist still has a lane.
    assert!(!volume_io::is_stalled(Path::new("/definitely/not/here")));
}

fn lane_test_base(name: &str) -> std::path::PathBuf {
    if cfg!(windows) {
        std::path::PathBuf::from(format!(r"C:\onecopy-lane-test-{name}"))
    } else {
        std::path::PathBuf::from(format!("/onecopy-lane-test-{name}"))
    }
}

/// A share mounted under a home folder (or reached any other way that spells
/// like the startup disk) is its own drive once configured, so its stall
/// never makes a configured folder on the startup disk fail fast.
#[test]
fn each_configured_root_is_its_own_drive_wherever_it_is_mounted() {
    let home = lane_test_base("roots").join("home");
    let share = home.join("mnt").join("share");
    let pictures = home.join("Pictures");
    volume_io::register_roots(&[share.clone(), pictures.clone()]);

    let on_share = volume_io::lane_name(&share.join("a.jpg"));
    assert_eq!(on_share, volume_io::lane_name(&share.join("deep").join("b.jpg")));
    assert_ne!(on_share, volume_io::lane_name(&pictures.join("c.jpg")));
    assert_ne!(on_share, volume_io::lane_name(&home.join("elsewhere.jpg")));
}

/// Walk entries carry a root's resolved spelling; they stay on the root's
/// lane instead of falling back to the startup disk.
#[cfg(unix)]
#[test]
fn a_root_reached_through_a_link_keeps_its_lane_in_its_resolved_spelling() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    volume_io::register_roots(std::slice::from_ref(&link));
    let resolved = volume_io::canonicalize(&link).unwrap();

    let through_link = volume_io::lane_name(&link.join("a.jpg"));
    assert_eq!(volume_io::lane_name(&resolved.join("a.jpg")), through_link);
    assert_ne!(
        through_link,
        volume_io::lane_name(&lane_test_base("link").join("b.jpg"))
    );
}

/// A root above several volumes still keeps them apart.
#[cfg(target_os = "macos")]
#[test]
fn a_root_above_several_volumes_never_merges_them() {
    let volumes = Path::new("/Volumes");
    volume_io::register_roots(&[volumes.to_path_buf()]);
    assert_ne!(
        volume_io::lane_name(&volumes.join("A").join("x.jpg")),
        volume_io::lane_name(&volumes.join("B").join("x.jpg"))
    );
}

/// Reading the settings registers their roots before any call on them.
#[test]
fn reading_the_settings_registers_each_configured_root() {
    let data_root = tempfile::tempdir().unwrap();
    let base = lane_test_base("settings");
    let (source, destination) = (base.join("home").join("nas"), base.join("home").join("backup"));
    std::fs::write(
        data_root.path().join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "sourceDirs": [source.to_string_lossy()],
            "destinationRoots": [destination.to_string_lossy()],
        }))
        .unwrap(),
    )
    .unwrap();
    onecopy_lib::storage::configured_roots(data_root.path()).unwrap();

    let elsewhere = volume_io::lane_name(&base.join("home").join("x.jpg"));
    assert_ne!(volume_io::lane_name(&source.join("a.jpg")), elsewhere);
    assert_ne!(volume_io::lane_name(&destination.join("a.jpg")), elsewhere);
}

/// Cancel while the walk waits: the walk stops at its next entry and says it
/// was cancelled, so no caller mistakes the entries it got for all of them.
#[test]
fn a_cancelled_walk_ends_as_cancelled_never_as_complete() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a").join("f"), b"x").unwrap();
    std::fs::write(dir.path().join("a").join("g"), b"y").unwrap();
    let volume = Arc::new(FakeStallingVolume::mount(dir.path(), Duration::from_secs(60)));
    volume.stall(&[Op::List], Some(&dir.path().join("a")));
    let releaser = volume.clone();
    // Released well inside the cancel grace, so the walk stops by itself
    // rather than being abandoned.
    std::thread::spawn(move || {
        releaser.wait_until_held(1, Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(50));
        releaser.release();
    });
    let mut walk = volume_io::walk(dir.path(), |_, _| true).unwrap();
    let ended = loop {
        match walk.next(Some(&|| true)) {
            Some(Ok(_)) => {}
            Some(Err(error)) => break Some(error),
            None => break None,
        }
    };
    let error = ended.expect("a cancelled walk must not read as a complete one");
    assert_eq!(failure(&error), Some(WaitFailure::Cancelled));
    assert!(!volume.is_stalled(), "a walk that stopped by itself was not abandoned");
}

/// The walk finishes in the instant its consumer's wait gives up: the entries
/// it queued and its end are still delivered, so the consumer never reads a
/// walk it did not see finish as a complete one. The consumer's cancel check
/// runs exactly there, between the last poll and the give-up, so it is the
/// seam that lets the walk finish at that instant.
#[test]
fn a_walk_that_finishes_as_its_wait_gives_up_still_delivers_every_entry() {
    const QUIET: Duration = Duration::from_millis(100);
    let caught_at_the_give_up = std::cell::Cell::new(0);
    for _ in 0..20 {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("f"), b"x").unwrap();
        let volume = FakeStallingVolume::mount(dir.path(), QUIET);
        volume.stall(&[Op::List], None);
        let mut walk = volume_io::walk(dir.path(), |_, _| true).unwrap();
        assert!(volume.wait_until_held(1, Duration::from_secs(5)));
        let started = Instant::now();
        let finish_walk = || {
            // Late in the wait, let the walk run to its end before the
            // consumer decides.
            if started.elapsed() + Duration::from_millis(5) >= QUIET && volume.held() > 0 {
                volume.release();
                std::thread::sleep(Duration::from_millis(50));
                caught_at_the_give_up.set(caught_at_the_give_up.get() + 1);
            }
            false
        };
        let mut seen = 0;
        let ended = loop {
            match walk.next(Some(&finish_walk)) {
                Some(Ok(Ok(_))) => seen += 1,
                Some(Ok(Err(error))) => panic!("unexpected walk error {}", error.message),
                Some(Err(error)) => break Some(error),
                None => break None,
            }
        };
        if let Some(error) = ended {
            // Given up on before the release: not this case.
            assert_eq!(failure(&error), Some(WaitFailure::NotResponding));
            volume.release();
            assert!(volume.wait_until_settled(Duration::from_secs(5)));
            continue;
        }
        assert_eq!(seen, 2, "a walk read as complete must have delivered the root and its file");
    }
    assert!(caught_at_the_give_up.get() > 0, "the walk never finished during a wait");
}

/// Cancel during the final flush of a private output returns without waiting
/// for the flush's size-derived bound, which for a large file on a slow drive
/// is many minutes; the private output settles when the flush returns.
#[test]
fn cancel_ends_a_stalled_flush_of_a_private_output() {
    let source_dir = tempfile::tempdir().unwrap();
    let dest_dir = tempfile::tempdir().unwrap();
    let source = source_dir.path().join("photo.jpg");
    std::fs::write(&source, vec![3u8; 64 * 1024]).unwrap();
    let dest = dest_dir.path().join("photo.jpg.private");
    let volume = Arc::new(FakeStallingVolume::mount(dest_dir.path(), Duration::from_secs(10)));
    volume.stall(&[Op::Sync], None);
    let cancelled = Arc::new(AtomicBool::new(false));
    let (flag, held) = (cancelled.clone(), volume.clone());
    std::thread::spawn(move || {
        held.wait_until_held(1, Duration::from_secs(5));
        flag.store(true, Ordering::SeqCst);
    });

    let started = Instant::now();
    let error = onecopy_lib::hashing::hash_while_copying_cancellable(
        &source,
        &dest,
        &|| cancelled.load(Ordering::SeqCst),
        &mut |_, _| {},
    )
    .unwrap_err();
    assert!(started.elapsed() < SLACK, "waited {:?}", started.elapsed());
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);

    volume.release();
    assert!(volume.wait_until_settled(Duration::from_secs(5)));
    assert!(!dest.exists(), "the unpublished output removes itself once the flush returns");
}
