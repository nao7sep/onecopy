use super::*;

fn measured_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let day = source.join(TRASH_DIR_NAME).join("20261001-utc");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("a.jpg"), vec![1u8; 100]).unwrap();
    std::fs::write(day.join("b.jpg"), vec![2u8; 900]).unwrap();
    let mut options = std::fs::File::options();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES,
        };
        options
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    options
        .open(crate::winpath::for_fs(&day))
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
        .unwrap();
    (dir, source, day)
}

#[cfg(unix)]
#[test]
fn measurement_uses_the_walks_filesystem_spelling_and_keeps_its_ui_owner() {
    let (dir, source, _) = measured_fixture();
    let alias = dir.path().join("configured-alias");
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let ui_root = alias.join(TRASH_DIR_NAME);
    let filesystem_root = std::fs::canonicalize(source.join(TRASH_DIR_NAME)).unwrap();
    // The actual walker yields a different spelling of the same tree. This
    // exercises the same boundary as ordinary UI paths vs verbatim Windows
    // entries without substituting a fake traversal or prefix algorithm.
    let measured = measure_root_at(&ui_root, &filesystem_root, None).unwrap();
    assert_eq!((measured.files, measured.bytes), (2, 1000));
    let row = overview(std::slice::from_ref(&alias)).remove(0);
    assert_eq!(row.root, ui_root.to_string_lossy());
    assert_eq!(row.plan_token, measured.token);
    assert!(row.measurement_error.is_none());
    let emptied = empty_root_with_progress(
        &ui_root,
        &row.plan_token,
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(!emptied.plan_changed);
    assert_eq!(std::fs::read_dir(&ui_root).unwrap().count(), 0);
}

windows_test! {
    "Windows only: the verbatim long-path spelling exists only on Windows",
    fn ordinary_windows_root_can_be_measured_and_emptied() {
        let (_dir, source, _) = measured_fixture();
        let ordinary_source =
            PathBuf::from(crate::winpath::for_display(source.to_str().unwrap()).as_ref());
        let row = overview(std::slice::from_ref(&ordinary_source)).remove(0);
        let ordinary_root = ordinary_source.join(TRASH_DIR_NAME);
        assert_eq!(row.root, ordinary_root.to_string_lossy());
        assert_eq!((row.files, row.bytes), (2, 1000));
        assert!(row.measurement_error.is_none());
        assert!(!row.plan_token.is_empty());
        let emptied = empty_root_with_progress(
            &ordinary_root,
            &row.plan_token,
            &AtomicBool::new(false),
            &|_| {},
            &|_, _| Ok(()),
        )
        .unwrap();
        assert!(!emptied.plan_changed);
        assert_eq!(std::fs::read_dir(&ordinary_root).unwrap().count(), 0);
    }
}

#[test]
fn incomplete_measurement_cannot_authorize_empty_after_recovery() {
    use crate::volume_io::{FakeStallingVolume, Op};
    use std::time::Duration;
    let (_dir, source, day) = measured_fixture();
    let fake = FakeStallingVolume::mount(&source, Duration::from_secs(10));
    // The day root and one file are measured; the next file fails on the
    // actual volume worker. Its directory mtime stays old and unchanged.
    fake.stall_after(&[Op::List], Some(&day), 2);
    let reviewed = std::thread::scope(|scope| {
        let measuring = scope.spawn(|| overview(std::slice::from_ref(&source)).remove(0));
        let held = fake.wait_until_held(1, Duration::from_secs(5));
        fake.fail();
        let row = measuring.join().unwrap();
        assert!(held, "the real size walk reached the held file");
        row
    });
    fake.release();
    let outcome = empty_root_with_progress(
        Path::new(&reviewed.root),
        &reviewed.plan_token,
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(
        outcome.plan_changed,
        "incomplete review must remove nothing after recovery"
    );
    assert!(day.join("a.jpg").exists());
    assert!(day.join("b.jpg").exists());
    assert!(
        reviewed.plan_token.is_empty(),
        "failed totals grant no review authority"
    );
    assert!(reviewed.measurement_error.is_some());
    let current = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!((current.files, current.bytes), (2, 1000));
    assert!(!current.plan_token.is_empty());
    let retry = empty_root_with_progress(
        Path::new(&current.root),
        &current.plan_token,
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(!retry.plan_changed);
    assert_eq!(std::fs::read_dir(&current.root).unwrap().count(), 0);
}

#[test]
fn editing_an_existing_stored_file_refreshes_totals_and_refuses_old_review() {
    let (_dir, source, day) = measured_fixture();
    let reviewed = overview(std::slice::from_ref(&source)).remove(0);
    let before = std::fs::metadata(&day).unwrap().modified().unwrap();
    std::fs::write(day.join("a.jpg"), vec![3u8; 500]).unwrap();
    assert_eq!(std::fs::metadata(&day).unwrap().modified().unwrap(), before);
    let current = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!((current.files, current.bytes), (2, 1400));
    assert_ne!(current.plan_token, reviewed.plan_token);
    let outcome = empty_root_with_progress(
        Path::new(&reviewed.root),
        &reviewed.plan_token,
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(outcome.plan_changed);
    assert!(day.join("a.jpg").exists());
    assert!(day.join("b.jpg").exists());
    let retry = empty_root_with_progress(
        Path::new(&current.root),
        &current.plan_token,
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(!retry.plan_changed);
    assert!(
        !day.exists(),
        "the newly reviewed complete totals permit Empty"
    );
}

#[test]
fn a_held_size_walk_never_blocks_measurement_of_another_root() {
    use crate::volume_io::{FakeStallingVolume, Op};
    use std::time::Duration;
    let (_first, source, day) = measured_fixture();
    let (_second, other, _) = measured_fixture();
    let fake = FakeStallingVolume::mount(&source, Duration::from_secs(10));
    fake.stall(&[Op::List], Some(&day));
    let (held, independent) = std::thread::scope(|scope| {
        let first = scope.spawn(|| overview(std::slice::from_ref(&source)));
        let held = fake.wait_until_held(1, Duration::from_secs(5));
        let (done, completed) = std::sync::mpsc::channel();
        let second = scope.spawn(move || {
            done.send(overview(std::slice::from_ref(&other))).unwrap();
        });
        let independent = completed.recv_timeout(Duration::from_millis(500)).is_ok();
        fake.release();
        first.join().unwrap();
        second.join().unwrap();
        (held, independent)
    });
    assert!(held);
    assert!(
        independent,
        "another source's totals cannot wait on this volume's walk"
    );
}

#[test]
fn nested_changes_are_measured_even_when_the_day_folder_is_unchanged() {
    let (_dir, source, day) = measured_fixture();
    let nested = day.join("recovered-by-hand");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("extra.jpg"), b"extra").unwrap();
    let reviewed = overview(std::slice::from_ref(&source)).remove(0);
    let modified = std::fs::metadata(&day).unwrap().modified().unwrap();
    std::fs::write(nested.join("later.jpg"), b"later").unwrap();
    assert_eq!(
        std::fs::metadata(&day).unwrap().modified().unwrap(),
        modified
    );
    let current = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!((current.files, current.bytes), (4, 1010));
    let outcome = empty_root_with_progress(
        Path::new(&reviewed.root),
        &reviewed.plan_token,
        &AtomicBool::new(false),
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(outcome.plan_changed);
    assert!(nested.join("later.jpg").exists());
}

#[test]
fn unavailable_measurement_is_distinct_from_a_missing_empty_location() {
    use crate::volume_io::{FakeStallingVolume, Op};
    use std::time::Duration;
    let (_dir, source, _) = measured_fixture();
    let location = source.join(TRASH_DIR_NAME);
    let fake = FakeStallingVolume::mount(&source, Duration::from_secs(10));
    fake.stall(&[Op::Stat], Some(&location));
    let unavailable = std::thread::scope(|scope| {
        let reading = scope.spawn(|| overview(std::slice::from_ref(&source)).remove(0));
        let held = fake.wait_until_held(1, Duration::from_secs(5));
        fake.fail();
        let row = reading.join().unwrap();
        assert!(held);
        row
    });
    assert!(unavailable.measurement_error.is_some());
    assert!(unavailable.plan_token.is_empty());
    fake.release();
    let missing_source = source.with_file_name("empty-source");
    std::fs::create_dir(&missing_source).unwrap();
    let missing = overview(std::slice::from_ref(&missing_source)).remove(0);
    assert!(missing.available);
    assert!(missing.measurement_error.is_none());
    assert!(!missing.plan_token.is_empty());
    assert_eq!((missing.files, missing.bytes), (0, 0));
}

#[test]
fn cancelling_empty_during_its_held_measurement_returns_without_removing_files() {
    use crate::volume_io::{FakeStallingVolume, Op};
    use std::time::Duration;
    let (_dir, source, day) = measured_fixture();
    let reviewed = overview(std::slice::from_ref(&source)).remove(0);
    let fake = FakeStallingVolume::mount(&source, Duration::from_secs(10));
    // Hold the first yielded entry: there are no queued entries whose
    // per-entry cancellation check could mask an uncancellable wait.
    fake.stall(&[Op::List], Some(Path::new(&reviewed.root)));
    let cancelled = AtomicBool::new(false);
    let progress = std::sync::atomic::AtomicU64::new(0);
    let (held, returned, outcome) = std::thread::scope(|scope| {
        let (done, completed) = std::sync::mpsc::channel();
        let (reviewed, cancelled, progress) = (&reviewed, &cancelled, &progress);
        let emptying = scope.spawn(move || {
            let outcome = empty_root_with_progress(
                Path::new(&reviewed.root),
                &reviewed.plan_token,
                &cancelled,
                &|_| {
                    progress.fetch_add(1, Ordering::Relaxed);
                },
                &|_, _| Ok(()),
            );
            done.send(outcome).unwrap();
        });
        let held = fake.wait_until_held(1, Duration::from_secs(5));
        cancelled.store(true, Ordering::SeqCst);
        let early = completed.recv_timeout(Duration::from_secs(1));
        let returned = early.is_ok();
        fake.release();
        emptying.join().unwrap();
        let outcome = early.unwrap_or_else(|_| completed.recv().unwrap()).unwrap();
        (held, returned, outcome)
    });
    assert!(held);
    assert!(
        returned,
        "Cancel ends planning while the volume call is still held"
    );
    assert!(outcome.cancelled);
    assert_eq!(progress.load(Ordering::Relaxed), 0, "Empty never started");
    assert!(day.join("a.jpg").exists());
    assert!(day.join("b.jpg").exists());
    assert!(fake.wait_until_settled(Duration::from_secs(2)));
}

#[test]
fn exact_boundary_winner_survives_and_source_remains_authoritative() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("photo.jpg");
    std::fs::write(&source, b"source").unwrap();

    let result = trash_file_with_before_move(&source, &source_dir, None, &ctx(), |target| {
        std::fs::write(target, b"winner").unwrap()
    });

    assert!(result.is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"source");
    let day = std::fs::read_dir(source_dir.join(TRASH_DIR_NAME))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(day.join("photo.jpg")).unwrap(), b"winner");
}

#[test]
fn newer_records_added_after_trash_preparation_leave_the_source_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    std::fs::create_dir(&root).unwrap();
    let source = root.join("photo.jpg");
    std::fs::write(&source, b"keep").unwrap();
    let mut manifest = PathBuf::new();
    let mut bytes = Vec::new();

    let error = trash_file_with_before_move(&source, &root, None, &ctx(), |target| {
        manifest = target.parent().unwrap().join(MANIFEST_FILE_NAME);
        bytes = std::fs::read(&manifest).unwrap();
        bytes.extend_from_slice(b"{\"formatVersion\":2}\n");
        std::fs::write(&manifest, &bytes).unwrap();
    }).unwrap_err();

    assert!(!error.outcome_unknown);
    assert_eq!(std::fs::read(&source).unwrap(), b"keep");
    assert_eq!(std::fs::read(&manifest).unwrap(), bytes);
    assert!(!manifest.parent().unwrap().join("photo.jpg").exists());
}

#[test]
fn replacement_before_the_move_is_the_file_that_gets_trashed() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("photo.jpg");
    let held = source_dir.join("held.jpg");
    std::fs::write(&source, b"original").unwrap();

    let result = trash_file_with_before_move(&source, &source_dir, None, &ctx(), |_| {
        std::fs::rename(&source, &held).unwrap();
        std::fs::write(&source, b"replacement").unwrap();
    })
    .unwrap();

    assert!(!source.exists());
    assert_eq!(std::fs::read(&held).unwrap(), b"original");
    assert_eq!(std::fs::read(result.stored_path).unwrap(), b"replacement");
}

#[test]
fn unchanged_complete_measurements_keep_the_same_review_token() {
    let (_dir, source, _) = measured_fixture();
    let root = source.join(TRASH_DIR_NAME);
    let first = measure_root(&root).unwrap();
    let second = measure_root(&root).unwrap();
    assert_eq!((first.bytes, first.files), (1000, 2));
    assert_eq!((second.bytes, second.files), (first.bytes, first.files));
    assert_eq!(second.token, first.token);
}

#[test]
fn overview_notices_a_file_added_directly_into_an_existing_day_folder() {
    // The recovery contract lets a user remove (or, here, add to) trash
    // contents outside OneCopy. Every complete read must account for that
    // external change before giving the user new Empty authority.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let a = source.join("one.jpg");
    std::fs::write(&a, vec![1u8; 100]).unwrap();
    trash_file(&a, &source, Some("h1"), &ctx()).unwrap();

    let root = source.join(TRASH_DIR_NAME);
    assert_eq!(size_of(&root), (100, 1));

    let day_dir = std::fs::read_dir(&root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(day_dir.join("added-by-hand.bin"), vec![2u8; 50]).unwrap();

    assert_eq!(
        size_of(&root),
        (150, 2),
        "a file added outside OneCopy must be reflected on the very next read"
    );
}

fn size_of(root: &Path) -> (u64, u64) {
    let measure = measure_root(root).unwrap();
    (measure.bytes, measure.files)
}

#[cfg(target_os = "macos")]
#[test]
fn recoverable_deletion_works_on_a_volume_without_exclusive_rename() {
    use crate::fs_publish::seam::without_exclusive_rename;

    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("photo.jpg");
    std::fs::write(&source, b"source").unwrap();

    let record = without_exclusive_rename(|| trash_file(&source, &source_dir, None, &ctx())).unwrap();
    assert!(!source.exists());
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"source");

    // An exact-boundary winner at the stored name still survives.
    std::fs::write(&source, b"second").unwrap();
    let result = without_exclusive_rename(|| {
        trash_file_with_before_move(&source, &source_dir, None, &ctx(), |target| {
            std::fs::write(target, b"winner").unwrap()
        })
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"second");
}

fn ctx() -> TrashContext {
    TrashContext::new(TrashKind::Delete, "test-operation")
}

#[test]
fn the_latest_record_names_a_stored_file_reused_after_a_failed_move() {
    // A line is written before the move; when the move fails
    // the stored name stays free, and a later deletion the same day is given
    // it. Two lines then name one stored file, and only the later is true.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    std::fs::create_dir_all(root.join("first")).unwrap();
    std::fs::create_dir_all(root.join("second")).unwrap();
    let failed = root.join("first").join("photo.jpg");
    std::fs::write(&failed, b"first version").unwrap();
    let result = trash_file_with_before_move(&failed, &root, None, &ctx(), |_| {
        std::fs::remove_file(&failed).unwrap();
    });
    assert!(result.is_err(), "the move of a vanished source fails");

    let reused = root.join("second").join("photo.jpg");
    std::fs::write(&reused, b"second").unwrap();
    let record = trash_file(&reused, &root, None, &ctx()).unwrap();
    assert_eq!(record.stored_name, "photo.jpg", "the free name was handed out again");

    let day = Path::new(&record.stored_path).parent().unwrap();
    let raw = std::fs::read_to_string(day.join(MANIFEST_FILE_NAME)).unwrap();
    assert_eq!(raw.lines().count(), 2, "both lines stay: manifests are append-only");
    let listing = read_day(day).unwrap();
    assert_eq!(listing.records.len(), 1);
    let (latest, stored) = &listing.records[0];
    assert_eq!(latest.original_relative, "second/photo.jpg");
    assert_eq!(
        *stored,
        StoredState::Regular {
            size: 6,
            mtime_ms: latest.mtime_ms
        }
    );
    assert_eq!(listing.unrecorded_files, 0);
    assert_eq!(listing.malformed_lines, 0);
}

// A process that stops after the manifest line is written but before the move
// leaves a line naming no stored file: the day lists nothing, and the source
// file is untouched, still indexed where it was.
#[test]
fn a_stop_between_the_manifest_line_and_the_move_lists_nothing_and_keeps_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    std::fs::create_dir(&root).unwrap();
    let source = root.join("photo.jpg");
    std::fs::write(&source, b"keep").unwrap();
    let mut day = PathBuf::new();

    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = trash_file_with_before_move(&source, &root, None, &ctx(), |target| {
            day = target.parent().unwrap().to_path_buf();
            panic!("the process stops here");
        });
    }));

    assert!(stopped.is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"keep");
    assert!(day.join(MANIFEST_FILE_NAME).exists(), "the line was written first");
    let listing = read_day(&day).unwrap();
    assert!(listing.records.is_empty());
    assert_eq!(listing.restored, 0);
}

#[test]
fn separators_and_drives_are_unrepresentable_only_under_windows_name_rules() {
    for relative in [r"trips\a.jpg", "trips/c:a.jpg", "C:/a.jpg", "trips/a.jpg:stream"] {
        assert_eq!(components_under(relative, true), Err(UnfitPath::Unrepresentable), "{relative}");
        assert!(components_under(relative, false).is_ok(), "{relative}");
    }
    assert_eq!(components_under("trips/a.jpg", true).unwrap(), ["trips", "a.jpg"]);
}
