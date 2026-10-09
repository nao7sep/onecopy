use super::*;


#[test]
fn orientation_transforms_swap_dimensions_where_they_should() {
    let img = DynamicImage::new_rgb8(40, 20);
    assert_eq!(apply_orientation(img.clone(), 1).dimensions_tuple(), (40, 20));
    assert_eq!(apply_orientation(img.clone(), 3).dimensions_tuple(), (40, 20));
    assert_eq!(apply_orientation(img.clone(), 6).dimensions_tuple(), (20, 40));
    assert_eq!(apply_orientation(img, 8).dimensions_tuple(), (20, 40));
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

// Small helper so the orientation test reads naturally.
trait DimTuple {
    fn dimensions_tuple(&self) -> (u32, u32);
}
impl DimTuple for DynamicImage {
    fn dimensions_tuple(&self) -> (u32, u32) {
        (self.width(), self.height())
    }
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
            }).unwrap();
        assert_eq!((stats.failed, stats.derived), (1, 1));
        let issues: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE kind = 'decode-error'", [], |row| row.get(0)).unwrap();
        assert_eq!(issues, 1);
        assert!(crate::derived_state::image_candidates(&conn, false, None, None).unwrap().is_empty(), "ordinary browsing must not retry the panic");
    }
}

#[test]
fn startup_sweep_cannot_run_between_an_identity_promotions_commit_and_its_rename() {
    // R2-07 follow-up (Phase 10): `scanner::promote_identity` commits the DB
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
    let sweep = std::thread::spawn(move || {
        let sweep_conn = crate::index_store::open(&sweep_db_path).unwrap();
        startup_sweep(&sweep_conn, &sweep_cache, launched, &|| false)
    });

    // Give the sweep thread every chance to run if it were not blocked.
    std::thread::sleep(std::time::Duration::from_millis(200));
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
