use super::*;

#[test]
fn exact_boundary_winner_survives_and_source_remains_authoritative() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("photo.jpg");
    std::fs::write(&source, b"source").unwrap();

    let result = trash_file_with_before_move(&source, &source_dir, None, |target| {
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

    let result = trash_file_with_before_move(&source, &source_dir, None, |_| {
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
    trash_file(&a, &source, Some("h1")).unwrap();

    let root = source.join(TRASH_DIR_NAME);
    let (first_bytes, first_files) = tree_size(&root);
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
                bytes: 999,
                files: 7,
            },
        );
    }

    let (cached_bytes, cached_files) = tree_size(&root);
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
    trash_file(&a, &source, Some("h1")).unwrap();

    let root = source.join(TRASH_DIR_NAME);
    assert_eq!(tree_size(&root), (100, 1));

    let day_dir = std::fs::read_dir(&root)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(day_dir.join("added-by-hand.bin"), vec![2u8; 50]).unwrap();

    assert_eq!(
        tree_size(&root),
        (150, 2),
        "a file added outside OneCopy must be reflected on the very next read"
    );
}
