use super::*;

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
fn overview_reuses_a_day_folders_size_while_its_mtime_is_unchanged() {
    // C-L3: `trash_overview` used to walk every trashed file on every Trash
    // modal open. The fix caches each day folder's size keyed by that
    // folder's own mtime; this proves the cache is actually consulted (not
    // merely present) by poisoning a cached entry and observing it win over
    // the real on-disk size, as long as the folder's mtime it was cached
    // under still matches.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let a = source.join("one.jpg");
    std::fs::write(&a, vec![1u8; 100]).unwrap();
    trash_file(&a, &source, Some("h1"), &ctx()).unwrap();

    let root = source.join(TRASH_DIR_NAME);
    let first = measure_root(&root);
    let (first_bytes, first_files) = (first.bytes, first.files);
    assert_eq!((first_bytes, first_files), (100, 1));

    let day_dir = std::fs::read_dir(&root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let modified = std::fs::metadata(&day_dir).unwrap().modified().unwrap();

    {
        let mut cache = DAY_SIZE_CACHE.lock().unwrap();
        cache.insert(
            day_dir.clone(),
            CachedDaySize {
                modified,
                measured: modified + std::time::Duration::from_secs(60),
                bytes: 999,
                files: 7,
            },
        );
    }

    let cached = measure_root(&root);
    let (cached_bytes, cached_files) = (cached.bytes, cached.files);
    assert_eq!(
        (cached_bytes, cached_files),
        (999, 7),
        "an unchanged day-folder mtime must reuse the cached size rather than re-walking"
    );
}

#[test]
fn overview_notices_a_file_added_directly_into_an_existing_day_folder() {
    // The recovery contract lets a user remove (or, here, add to) trash
    // contents outside OneCopy. Because the day folder's own mtime changes
    // whenever an entry is added or removed directly inside it, the cache
    // must never report a stale total after such an external change.
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
    let measure = measure_root(root);
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
    // Blueprint 1.4: a line is written before the move; when the move fails
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
    let listing = read_day(day, &root_spellings(&root)).unwrap();
    assert_eq!(listing.records.len(), 1);
    let (latest, stored) = &listing.records[0];
    assert_eq!(latest.original_relative.as_deref(), Some("second/photo.jpg"));
    assert_eq!(
        *stored,
        StoredState::Regular {
            size: 6,
            mtime_ms: latest.mtime_ms.unwrap()
        }
    );
    assert_eq!(listing.unrecorded_files, 0);
    assert_eq!(listing.malformed_lines, 0);
}
