use super::*;


#[test]
fn every_exif_orientation_places_asymmetric_pixels_correctly() {
    // Stored rows are A B C / D E F. Fixed expected rows distinguish every
    // corner and both axes without deriving the oracle from image transforms.
    let colors = [
        [255, 0, 0], [0, 255, 0], [0, 0, 255],
        [255, 255, 0], [255, 0, 255], [0, 255, 255],
    ];
    let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(3, 2, |x, y| {
        image::Rgb(colors[(y * 3 + x) as usize])
    }));
    let cases = [
        (1, (3, 2), [0, 1, 2, 3, 4, 5]), // A B C / D E F
        (2, (3, 2), [2, 1, 0, 5, 4, 3]), // C B A / F E D
        (3, (3, 2), [5, 4, 3, 2, 1, 0]), // F E D / C B A
        (4, (3, 2), [3, 4, 5, 0, 1, 2]), // D E F / A B C
        (5, (2, 3), [0, 3, 1, 4, 2, 5]), // A D / B E / C F
        (6, (2, 3), [3, 0, 4, 1, 5, 2]), // D A / E B / F C
        (7, (2, 3), [5, 2, 4, 1, 3, 0]), // F C / E B / D A
        (8, (2, 3), [2, 5, 1, 4, 0, 3]), // C F / B E / A D
    ];
    for (orientation, dimensions, order) in cases {
        let result = apply_orientation(img.clone(), orientation).to_rgb8();
        assert_eq!(result.dimensions(), dimensions, "orientation {orientation}");
        let pixels: Vec<_> = result.pixels().map(|pixel| pixel.0).collect();
        assert_eq!(pixels, order.map(|index| colors[index]), "orientation {orientation}");
    }
}

#[test]
fn sharp_images_score_higher_than_their_blurred_versions() {
    let sharp = image::RgbImage::from_fn(200, 200, |x, _| {
        if (x / 10) % 2 == 0 {
            image::Rgb([255, 255, 255])
        } else {
            image::Rgb([0, 0, 0])
        }
    });
    let sharp_dyn = DynamicImage::ImageRgb8(sharp);
    let blurred = sharp_dyn.blur(4.0);
    let s_sharp = laplacian_variance(&sharp_dyn.to_luma8());
    let s_blur = laplacian_variance(&blurred.to_luma8());
    assert!(
        s_sharp > s_blur * 2.0,
        "sharp {s_sharp} should clearly exceed blurred {s_blur}"
    );
}

#[test]
fn a_downscaled_copy_never_outranks_its_original_on_sharpness() {
    // Fine detail a camera recorded; an export halves it.
    let original = DynamicImage::ImageRgb8(image::RgbImage::from_fn(1600, 1200, |x, y| {
        let v = if ((x / 8) + (y / 8)) % 2 == 0 { 220 } else { 30 };
        image::Rgb([v, v, v])
    }));
    let copy = original.resize_exact(640, 480, image::imageops::FilterType::CatmullRom);
    // At their own sizes the copy looks sharper, which ranked it first.
    assert!(laplacian_variance(&copy.to_luma8()) > laplacian_variance(&original.to_luma8()));
    let (original, copy) = (sharpness(&original), sharpness(&copy));
    assert!(copy <= original * 1.1, "copy {copy} outranks original {original}");
}

#[test]
fn decoder_panics_record_one_failure_and_continue_in_serial_and_parallel() {
    for capacity in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));
        let mut rows = Vec::new();
        for hash in ["bad001", "good01"] {
            let path = dir.path().join(format!("{hash}.jpg")).to_string_lossy().into_owned();
            std::fs::write(&path, b"synthetic").unwrap();
            conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 1, 'image')", [hash]).unwrap();
            conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES (?1, ?2, ?3, 'image', ?3)",
                rusqlite::params![path, dir.path().to_string_lossy(), hash]).unwrap();
            rows.push((hash.to_string(), path));
        }
        let stats = derive_candidate_rows_with(&conn, &cache, None, None, None, rows, capacity, &|| false,
            &|hash, _, _| {
                if hash == "bad001" { panic!("synthetic corrupt decoder input"); }
                Ok((None, DerivedFacts { width: 10, height: 10, sharpness: 1.0, phash: 1 }))
            }, None).unwrap();
        assert_eq!((stats.failed, stats.derived), (1, 1));
        let issues: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE kind = ?1", [crate::derived_state::PREVIEW_ERROR], |row| row.get(0)).unwrap();
        assert_eq!(issues, 1);
        assert!(crate::derived_state::image_candidates(&conn, false, None, None).unwrap().is_empty(), "ordinary browsing must not retry the panic");
    }
}

#[test]
fn a_sweep_cohort_cannot_run_between_an_identity_promotions_commit_and_its_rename() {
    // `scanner::promote_identity` commits the DB
    // move from a provisional hash to the real one, then renames the cache
    // files to match — two steps that cannot be one filesystem+DB
    // transaction. If the startup sweep's orphan check ran in the gap
    // between them, it would see the provisional key already gone from
    // `contents` and delete the not-yet-renamed cache file out from under
    // the rename, losing a derived thumb/preview that then has to be
    // regenerated. `CACHE_IDENTITY_LOCK` (`lock_cache_identity`) closes that
    // gap: this test holds the lock across a manual commit-to-rename step,
    // exactly where a real promotion would, and proves a concurrently
    // running sweep cannot observe the intermediate state.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-sweep-vs-promote-")
        .tempdir()
        .unwrap();
    let db_path = dir.path().join("index.sqlite3");
    let cache_root = dir.path().join("cache");
    let conn = crate::index_store::open(&db_path).unwrap();
    let cache = CachePaths::new(cache_root.clone());

    // A provisional identity, with its cache files already on disk from an
    // earlier launch (their mtime predates `launched`, so the sweep's
    // "written this run" guard does not shield them).
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('p1', 1, 'image')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
         VALUES ('/solo.jpg', '/', 'solo.jpg', 'image', 'p1')",
        [],
    )
    .unwrap();
    for path in [cache.thumb("p1"), cache.preview("p1")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"webp-bytes").unwrap();
    }
    let launched = std::time::SystemTime::now() + std::time::Duration::from_secs(120);

    // Take the lock, then perform exactly the DB half of a promotion
    // (matching `promote_identity`'s not-already-known branch: the real row
    // exists, `paths` is repointed, and the provisional row is gone) —
    // "commit" — before doing the cache-file half — "rename".
    let identity_lock = lock_cache_identity();
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('real1', 1, 'image')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE paths SET content_hash = 'real1' WHERE content_hash = 'p1'",
        [],
    )
    .unwrap();
    conn.execute("DELETE FROM contents WHERE hash = 'p1'", [])
        .unwrap();

    // A concurrent sweep, on its own connection like the real startup
    // sequence, must now block instead of running ahead of the rename.
    let sweep_cache = CachePaths::new(cache_root.clone());
    let sweep_db_path = db_path.clone();
    let (blocked, observed) = std::sync::mpsc::channel();
    let sweep = std::thread::spawn(move || {
        IDENTITY_LOCK_BLOCKED.with(|hook| *hook.borrow_mut() = Some(blocked));
        let sweep_conn = crate::index_store::open(&sweep_db_path).unwrap();
        let cohort = vec![
            ("thumbs".to_string(), sweep_cache.thumb("p1")),
            ("previews".to_string(), sweep_cache.preview("p1")),
        ];
        sweep_cohort(&sweep_conn, &cohort, launched)
    });

    // Notification is emitted only after the real mutex refuses try_lock.
    observed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(
        cache.thumb("p1").exists() && cache.preview("p1").exists(),
        "the sweep must not touch the provisional cache files while the lock is held"
    );

    // Now do the rename half of the promotion, still under the lock.
    rename_entries(&cache, "p1", "real1", 0);
    drop(identity_lock);

    let removed = sweep.join().unwrap().unwrap();
    assert_eq!(removed, 0, "the renamed files are live under `real1`, not orphaned");
    assert!(cache.thumb("real1").exists());
    assert!(cache.preview("real1").exists());
    assert!(!cache.thumb("p1").exists(), "renamed away, not deleted by the sweep");
}

#[test]
fn an_identity_promotion_waiting_for_the_sweep_never_holds_sqlites_writer() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("index.sqlite3");
    let cache_root = dir.path().join("cache");
    let sweep_conn = crate::index_store::open(&db_path).unwrap();
    sweep_conn.busy_timeout(std::time::Duration::from_millis(100)).unwrap();
    sweep_conn.execute_batch(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('p1', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('/a.jpg', '/', 'a.jpg', 'image', 'p1');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('orphan', 1, 'image');",
    ).unwrap();
    let cache = CachePaths::new(cache_root.clone());
    let thumb = cache.thumb("p1");
    std::fs::create_dir_all(thumb.parent().unwrap()).unwrap();
    std::fs::write(&thumb, b"derived thumb").unwrap();

    // This is the sweep's reservation-to-orphan-delete boundary. A separate
    // promotion must queue for that reservation before claiming SQLite's
    // writer, or the two owners wait for each other in the reverse order.
    let identity_lock = lock_cache_identity();
    let (writer_entered, writer_observed) = std::sync::mpsc::channel();
    let (blocked, observed) = std::sync::mpsc::channel();
    let ready = std::sync::Arc::new(std::sync::Barrier::new(2));
    let promotion_ready = ready.clone();
    let promotion = std::thread::spawn(move || {
        IDENTITY_LOCK_BLOCKED.with(|hook| *hook.borrow_mut() = Some(blocked));
        let conn = crate::index_store::open(&db_path).unwrap();
        conn.create_collation("promotion_writer", move |left, right| {
            let _ = writer_entered.send(());
            left.cmp(right)
        }).unwrap();
        conn.execute_batch(
            "CREATE TEMP TRIGGER observe_promotion_writer BEFORE UPDATE OF content_hash ON main.paths
             WHEN OLD.content_hash = 'p1' BEGIN
               SELECT 'entered' = 'writer' COLLATE promotion_writer;
             END;",
        ).unwrap();
        promotion_ready.wait();
        crate::scanner::promote_identity(&conn, &CachePaths::new(cache_root), "p1", "real1")
    });
    ready.wait();
    observed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    let held_writer = writer_observed.try_recv().is_ok();
    let sweep_result = reconcile_orphan_batch(&sweep_conn, "").map(|(_, removed)| removed);
    drop(identity_lock);
    let promotion_result = promotion.join().unwrap();

    assert!(sweep_result.is_ok(), "sweep orphan deletion failed while a promotion queued (writer entered: {held_writer}): {sweep_result:?}");
    assert!(promotion_result.is_ok(), "{promotion_result:?}");
    assert_eq!(sweep_result.unwrap(), 1);
    assert_eq!(sweep_conn.query_row("SELECT content_hash FROM paths", [], |row| row.get::<_, String>(0)).unwrap(), "real1");
    assert!(cache.thumb("real1").exists(), "promotion keeps its derived cache after the sweep finishes");
    assert!(!thumb.exists());
}

#[test]
fn orientation_read_distinguishes_missing_metadata_from_failed_source_io() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plain.png");
    DynamicImage::new_rgb8(8, 6).save(&path).unwrap();
    assert_eq!(read_orientation(&path).unwrap(), 1);
    // The decode can have finished before a removable source disappears.
    let _decoded = crate::resource_limits::decode_file(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(read_orientation(&path).is_err());
}


#[test]
fn retirement_visits_a_shared_strip_shard_once_per_hash_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let hashes: Vec<_> = (0..129).map(|index| format!("ab{index:04}")).collect();
    for hash in &hashes {
        for frame in 0..3 {
            let path = crate::video::strip_path(&cache, hash, frame);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"frame").unwrap();
        }
    }
    STRIP_DIRECTORY_VISITS.with(|count| count.set(0));
    remove_entries_batch(&cache, &hashes);
    STRIP_DIRECTORY_VISITS.with(|count| assert_eq!(count.get(), 2));
    assert!(hashes.iter().all(|hash| !crate::video::strip_path(&cache, hash, 0).exists()));
}

#[test]
fn retirement_flushes_full_cohorts_and_drop_flushes_the_committed_tail() {
    let dir = tempfile::tempdir().unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let hashes: Vec<_> = (0..129).map(|index| format!("ab{index:04}")).collect();
    for hash in &hashes {
        let path = cache.thumb(hash);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"thumb").unwrap();
    }
    {
        let mut retirement = CacheRetirementBatch::new(&cache);
        for hash in &hashes[..128] { retirement.push(hash.clone()); }
        assert!(!cache.thumb(&hashes[0]).exists());
        retirement.push(hashes[128].clone());
        assert!(cache.thumb(&hashes[128]).exists());
    }
    assert!(!cache.thumb(&hashes[128]).exists());
}

#[test]
fn sweep_membership_is_shared_within_one_reservation_but_not_across_cohorts() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let launched = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    let paths: Vec<_> = (0..128).map(|frame| ("strips".to_string(), crate::video::strip_path(&cache, "abcdef", frame))).collect();
    for (_, path) in &paths {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"frame").unwrap();
    }
    SWEEP_MEMBERSHIP_QUERIES.with(|count| count.set(0));
    assert_eq!(sweep_cohort(&conn, &paths, launched).unwrap(), 128);
    SWEEP_MEMBERSHIP_QUERIES.with(|count| assert_eq!(count.get(), 1));
    conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES ('abcdef', 1, 'video')", []).unwrap();
    std::fs::write(&paths[0].1, b"new frame").unwrap();
    assert_eq!(sweep_cohort(&conn, &paths[..1], launched).unwrap(), 0);
    assert!(paths[0].1.exists());
    SWEEP_MEMBERSHIP_QUERIES.with(|count| assert_eq!(count.get(), 2));
}

#[test]
fn orphan_reconciliation_limits_each_writer_batch_to_one_keyset_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for index in 0..260 {
        conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES (?1, 1, 'image')", [format!("h{index:04}")]).unwrap();
    }
    let _identity_lock = lock_cache_identity();
    let (first, count) = reconcile_orphan_batch(&conn, "").unwrap();
    assert_eq!((first.len(), count), (128, 128));
    let (second, count) = reconcile_orphan_batch(&conn, first.last().unwrap()).unwrap();
    assert_eq!((second.len(), count), (128, 128));
    let (tail, count) = reconcile_orphan_batch(&conn, second.last().unwrap()).unwrap();
    assert_eq!((tail.len(), count), (4, 4));
}

#[test]
fn live_contents_also_advance_the_bounded_orphan_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for index in 0..260 {
        let hash = format!("h{index:04}");
        conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES (?1, 1, 'image')", [&hash]).unwrap();
        conn.execute("INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash) VALUES (?1, '/', ?1, 'image', ?1)", [&hash]).unwrap();
    }
    conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES ('z-orphan', 1, 'image')", []).unwrap();
    let _identity = lock_cache_identity();
    let (visited, removed) = reconcile_orphan_batch(&conn, "").unwrap();
    assert_eq!((visited.len(), removed), (128, 0));
    let (visited, removed) = reconcile_orphan_batch(&conn, visited.last().unwrap()).unwrap();
    assert_eq!((visited.len(), removed), (128, 0));
    let (visited, removed) = reconcile_orphan_batch(&conn, visited.last().unwrap()).unwrap();
    assert_eq!((visited.len(), removed), (5, 1));
    assert_eq!(conn.query_row("SELECT count(*) FROM contents", [], |row| row.get::<_, i64>(0)).unwrap(), 260);
}

#[test]
fn orphan_reconciliation_prepares_one_delete_per_table_and_keeps_offline_members() {
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute("INSERT INTO similar_groups(id, bucket, created_at_utc) VALUES (1, 'test', 'test')", []).unwrap();
    for index in 0..260 {
        let hash = format!("h{index:04}");
        conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES (?1, 1, 'image')", [&hash]).unwrap();
        conn.execute("INSERT INTO similar_group_members(group_id, content_hash) VALUES (1, ?1)", [&hash]).unwrap();
        if index % 2 == 0 {
            conn.execute("INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash, missing)
                          VALUES (?1, '/', ?1, 'image', ?1, 1)", [&hash]).unwrap();
        }
    }
    let deletes = Arc::new(AtomicUsize::new(0));
    let observed = deletes.clone();
    conn.authorizer(Some(move |context: AuthContext<'_>| {
        if matches!(context.action, AuthAction::Delete { table_name: "contents" | "similar_group_members" }) {
            observed.fetch_add(1, Ordering::Relaxed);
        }
        Authorization::Allow
    })).unwrap();
    let _identity = lock_cache_identity();
    let (visited, removed) = reconcile_orphan_batch(&conn, "").unwrap();
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>).unwrap();
    assert_eq!((visited.len(), removed), (128, 64));
    assert_eq!(deletes.load(Ordering::Relaxed), 2);
    let remaining: Vec<String> = conn.prepare("SELECT hash FROM contents ORDER BY hash").unwrap()
        .query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    let expected: Vec<String> = (0..260).filter(|index| *index >= 128 || index % 2 == 0)
        .map(|index| format!("h{index:04}")).collect();
    assert_eq!(remaining, expected);
    let members: Vec<String> = conn.prepare("SELECT content_hash FROM similar_group_members ORDER BY content_hash").unwrap()
        .query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(members, expected);
}

#[cfg(unix)]
#[test]
fn preview_reopens_the_exact_native_path_from_encoded_text() {
    use std::os::unix::ffi::OsStrExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(std::ffi::OsStr::from_bytes(b"photo-\xff.png"));
    // Probe the filesystem separately so a decoder failure cannot masquerade
    // as a filesystem that refuses invalid native names.
    if let Err(error) = std::fs::write(&source, b"") {
        if error.kind() == std::io::ErrorKind::InvalidInput
            || (cfg!(target_os = "macos") && error.raw_os_error() == Some(92)) {
            eprintln!("native filename fixture unsupported by this filesystem: {error}");
            return;
        }
        panic!("could not create native filename fixture: {error}");
    }
    DynamicImage::new_rgb8(8, 6).save_with_format(&source, image::ImageFormat::Png).unwrap();
    let cache = CachePaths::new(root.path().join("cache"));
    let encoded = crate::native_path::encode(&source);
    let (_, facts) = derive_candidate("exact-native", &encoded, &cache, 4, 8, None).unwrap();
    assert_eq!((facts.width, facts.height), (8, 6));
    assert!(cache.thumb("exact-native").exists());
    assert!(cache.preview("exact-native").exists());
    assert!(!root.path().join("photo-\u{fffd}.png").exists());
}

#[test]
fn requested_and_automatic_decode_limits_share_retryable_dependency_debt() {
    for requested in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));
        let source = dir.path().join("large.jpg").to_string_lossy().into_owned();
        std::fs::write(&source, b"decoder substituted").unwrap();
        conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES ('large01', 1, 'image')", []).unwrap();
        conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES (?1, ?2, 'large.jpg', 'image', 'large01')",
            rusqlite::params![source, dir.path().to_str().unwrap()]).unwrap();
        let error = format!("{}native allocation ceiling", crate::resource_limits::DECODE_LIMIT_PREFIX);
        if requested {
            assert_eq!(record_preview_failure_or_combine(&conn, "large01", &source, false, error), NEEDS_FFMPEG);
        } else {
            let stats = derive_candidate_rows_with(&conn, &cache, None, None, None,
                vec![("large01".into(), source.clone())], 1, &|| false, &|_, _, _| Err(error.clone()), None).unwrap();
            assert_eq!((stats.failed, stats.blocked_no_ffmpeg), (0, 1));
        }
        assert!(!crate::derived_state::preview_failed(&conn, "large01").unwrap());
        assert!(crate::derived_state::image_candidates(&conn, false, None, None).unwrap().is_empty());
        let rows = crate::derived_state::image_candidates(&conn, true, None, None).unwrap();
        assert_eq!(rows, [("large01".into(), source)]);
        let resumed = derive_candidate_rows_with(&conn, &cache, Some(Path::new("managed-ffmpeg")), None, None,
            rows, 1, &|| false, &|_, _, _| Ok((None, DerivedFacts { width: 10, height: 10, sharpness: 1.0, phash: 1 })), None).unwrap();
        assert_eq!(resumed.derived, 1);
        assert!(crate::derived_state::image_candidates(&conn, true, None, None).unwrap().is_empty());
    }
}

#[test]
fn automatic_preview_turn_records_one_ordinary_terminal_outcome() {
    const CHILD_ROOT: &str = "ONECOPY_PREVIEW_ACTIVITY_TEST_ROOT";
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "preview::tests::automatic_preview_turn_records_one_ordinary_terminal_outcome", "--nocapture"])
            .env(CHILD_ROOT, root.path()).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(root.path().join("activity-verified").is_file());
        return;
    };
    let root = std::path::PathBuf::from(root);
    let records_path = root.join(crate::records::RECORDS_DB_FILE_NAME);
    crate::logging::init(&records_path, &root.join("logs"), false);
    crate::activity::init(records_path.clone());
    assert!(!crate::logging::debug_enabled());
    let conn = crate::index_store::open(&root.join("index.sqlite3")).unwrap();
    let reader = crate::records::open(&records_path).unwrap();
    let cache = CachePaths::new(root.join("cache"));
    for (case, expected) in [
        ("success", "succeeded"), ("failure", "failed"), ("dependency", "waiting"),
        ("cancel", "cancelled"), ("storage", "waiting"), ("yield", "waiting"),
        ("empty", "none"),
    ] {
        let before: i64 = reader.query_row("SELECT coalesce(max(id), 0) FROM activity_events", [], |row| row.get(0)).unwrap();
        let mut rows = Vec::new();
        if case != "empty" {
            for index in 0..2 {
                let hash = format!("{case}-{index}");
                let path = root.join(format!("{hash}.jpg"));
                std::fs::write(&path, b"decode fixture").unwrap();
                conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 1, 'image')", [&hash]).unwrap();
                let path = crate::native_path::encode(&path).into_owned();
                conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES (?1, ?2, ?3, 'image', ?3)",
                    rusqlite::params![path, crate::native_path::encode(&root), hash]).unwrap();
                rows.push((hash, path));
            }
        }
        let result = derive_candidate_rows_with(&conn, &cache, None, None, None, rows, 1,
            &|| case == "yield", &|_, _, _| match case {
                "failure" => Err("bad image".into()),
                "dependency" => Err(NEEDS_FFMPEG.into()),
                "cancel" => Err(crate::scanner::CANCELLED.into()),
                "storage" => Err(format!("{}fixture", crate::resource_limits::STORAGE_UNAVAILABLE_PREFIX)),
                _ => Ok((None, DerivedFacts { width: 2, height: 2, sharpness: 1.0, phash: 1 })),
            }, None);
        assert_eq!(result.is_err(), matches!(case, "cancel" | "storage"));
        let mut query = reader.prepare("SELECT draft_json, user_visible FROM activity_events WHERE id > ?1 ORDER BY id").unwrap();
        let events = query.query_map([before], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        if case == "empty" { assert!(events.is_empty()); continue; }
        let drafts: Vec<serde_json::Value> = events.iter().map(|(json, visible)| {
            assert_eq!(*visible, 1);
            serde_json::from_str(json).unwrap()
        }).collect();
        assert_eq!(drafts.iter().filter(|draft| draft["kind"] == "started").count(), 1, "{case}");
        let operation = &drafts[0]["operationId"];
        for draft in &drafts {
            assert_eq!(&draft["operationId"], operation);
            assert_eq!(draft["subject"], "previews");
            assert!(draft["targetHash"].is_null(), "per-item detail must stay debug-only");
        }
        let terminal = drafts.last().unwrap();
        assert_eq!(terminal["current"], expected, "{case}");
        assert_eq!(terminal["total"], 2);
        assert_eq!(terminal["done"], if matches!(case, "success" | "failure" | "dependency") { 2 } else { 0 });
    }
    assert!(crate::logging::flush(std::time::Duration::from_secs(5)));
    std::fs::write(root.join("activity-verified"), b"complete").unwrap();
}

#[test]
fn healthy_parallel_previews_publish_before_a_held_peer_and_fallback_waits_for_it() {
    use std::sync::{Mutex, atomic::{AtomicBool, Ordering}, mpsc};
    let root = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&root.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(root.path().join("cache"));
    let mut rows = Vec::new();
    for hash in ["held", "fallback", "healthy"] {
        let path = root.path().join(format!("{hash}.jpg"));
        std::fs::write(&path, b"fixture").unwrap();
        let path = crate::native_path::encode(&path).into_owned();
        conn.execute("INSERT INTO contents(hash, byte_size, kind) VALUES (?1, 7, 'image')", [hash]).unwrap();
        conn.execute("INSERT INTO paths(abs_path, dir_path, file_name, kind, content_hash)
                      VALUES (?1, ?2, ?3, 'image', ?3)", rusqlite::params![path, crate::native_path::encode(root.path()), hash]).unwrap();
        rows.push((hash.to_string(), path));
    }
    let (release, held) = mpsc::channel();
    let held = Mutex::new(held);
    let settled = AtomicBool::new(false);
    let observed = std::cell::RefCell::new(Vec::new());
    let result = derive_candidate_rows_with(&conn, &cache, Some(Path::new("fixture-ffmpeg")), None, None,
        rows, 3, &|| false, &|hash, _, tool| {
            if hash == "held" {
                held.lock().unwrap().recv_timeout(std::time::Duration::from_secs(5)).unwrap();
                settled.store(true, Ordering::SeqCst);
            }
            if hash == "fallback" {
                if tool.is_none() {
                    return Err(format!("{}fixture", crate::resource_limits::DECODE_LIMIT_PREFIX));
                }
                assert!(settled.load(Ordering::SeqCst), "exclusive fallback overlapped the native worker");
            }
            Ok((None, DerivedFacts { width: 10, height: 10, sharpness: 1.0, phash: 1 }))
        }, Some(&|previous, current| {
            assert_eq!(previous, current);
            let ready: bool = conn.query_row("SELECT derived_at_utc IS NOT NULL FROM contents WHERE hash = ?1", [current], |row| row.get(0)).unwrap();
            assert!(ready, "notification precedes the durable receipt");
            if current == "healthy" {
                assert!(!settled.load(Ordering::SeqCst), "healthy publication waited for its peer");
                release.send(()).unwrap();
            }
            observed.borrow_mut().push(current.to_string());
        })).unwrap();
    assert_eq!(result.derived, 3);
    assert_eq!(*observed.borrow(), ["healthy", "held", "fallback"]);
}

#[test]
fn a_failed_parallel_publication_still_joins_its_physical_workers() {
    use std::sync::{Arc, Mutex, mpsc};
    let (entered, observed) = mpsc::channel();
    let (release, held) = mpsc::channel();
    let held = Arc::new(Mutex::new(held));
    let (failed, failure_seen) = mpsc::channel();
    let (returned, result) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let rows = vec![(0, "held".into(), "held.jpg".into()), (1, "healthy".into(), "healthy.jpg".into())];
        let result = derive_native_candidates_parallel(&rows, &|hash, _, _| {
            if hash == "held" {
                entered.send(()).unwrap();
                held.lock().unwrap().recv().unwrap();
            }
            Ok((None, DerivedFacts { width: 10, height: 10, sharpness: 1.0, phash: 1 }))
        }, |_, _| {
            failed.send(()).unwrap();
            Err("fixture publication failure".into())
        });
        returned.send(result).unwrap();
    });
    observed.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    failure_seen.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release.send(()).unwrap();
    assert_eq!(result.recv_timeout(std::time::Duration::from_secs(5)).unwrap().unwrap_err(), "fixture publication failure");
    worker.join().unwrap();
}
