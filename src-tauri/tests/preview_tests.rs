// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use std::path::{Path, PathBuf};
use image::DynamicImage;
use onecopy_lib::preview::*;
use onecopy_lib::index_store;

fn gradient_jpeg(dir: &Path, name: &str, w: u32, h: u32) -> PathBuf {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    });
    let path = dir.join(name);
    img.save(&path).unwrap();
    path
}

#[test]
fn generates_thumb_and_preview_within_limits_preserving_aspect() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-preview-")
        .tempdir()
        .unwrap();
    let src = gradient_jpeg(dir.path(), "big.jpg", 2000, 1000);
    let cache = CachePaths::new(dir.path().join("cache"));

    let facts = generate_for_image(&src, "abcd1234", &cache, 320, 1600).unwrap();
    assert_eq!((facts.width, facts.height), (2000, 1000));

    let preview = image::open(cache.preview("abcd1234")).unwrap();
    assert_eq!((preview.width(), preview.height()), (1600, 800));

    let thumb = image::open(cache.thumb("abcd1234")).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (320, 160));

    // Sharded layout: thumbs/ab/abcd1234.webp.
    assert!(cache
        .thumb("abcd1234")
        .to_string_lossy()
        .contains(&format!("thumbs{}ab{}", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR)));
}

#[test]
fn small_images_are_never_upscaled_and_skip_the_reencode() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-preview-small-")
        .tempdir()
        .unwrap();
    let src = gradient_jpeg(dir.path(), "small.jpg", 200, 100);
    let cache = CachePaths::new(dir.path().join("cache"));

    generate_for_image(&src, "ffff0000", &cache, 320, 1600).unwrap();

    // Fits the preview edge + displayable format + no orientation: the
    // preview entry is a byte-copy of the original, not a WebP re-encode
    // (the .webp cache name is load-bearing; the protocol sniffs bytes).
    assert_eq!(
        std::fs::read(cache.preview("ffff0000")).unwrap(),
        std::fs::read(&src).unwrap()
    );
    let preview = image::ImageReader::open(cache.preview("ffff0000"))
        .unwrap()
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    assert_eq!((preview.width(), preview.height()), (200, 100));
}



#[test]
fn dhash_known_answers_pin_the_bit_layout() {
    // Strictly increasing brightness left-to-right: every neighbor
    // comparison is "left < right", so every bit is set. Decreasing:
    // none. These are hand-derivable reference vectors, not
    // implementation echoes.
    let rising = DynamicImage::ImageRgb8(image::RgbImage::from_fn(90, 80, |x, _| {
        let v = (x * 2) as u8;
        image::Rgb([v, v, v])
    }));
    assert_eq!(dhash(&rising), u64::MAX);

    let falling = DynamicImage::ImageRgb8(image::RgbImage::from_fn(90, 80, |x, _| {
        let v = 200 - (x * 2) as u8;
        image::Rgb([v, v, v])
    }));
    assert_eq!(dhash(&falling), 0);

    // A flat image is degenerate by design: zero hash, distance 0 to
    // every other flat image — which is why the group-size cap exists.
    let flat = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([128; 3])));
    assert_eq!(dhash(&flat), 0);
}

#[test]
fn dhash_survives_scaling_but_not_rotation() {
    let scene = |w: u32, h: u32| {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
            // An asymmetric gradient-plus-blob scene, scale-independent.
            let fx = f64::from(x) / f64::from(w);
            let fy = f64::from(y) / f64::from(h);
            let blob = if (fx - 0.3).powi(2) + (fy - 0.6).powi(2) < 0.04 { 100.0 } else { 0.0 };
            let v = (fx * 155.0 + blob).min(255.0) as u8;
            image::Rgb([v, v, v])
        }))
    };
    let big = scene(800, 600);
    let small = scene(200, 150);
    let dist_scaled = (dhash(&big) ^ dhash(&small)).count_ones();
    assert!(dist_scaled <= 4, "same scene at two scales must match: {dist_scaled}");

    let rotated = big.rotate90();
    let dist_rotated = (dhash(&big) ^ dhash(&rotated)).count_ones();
    assert!(
        dist_rotated > 4,
        "a 90-degree rotation must not silently match: {dist_rotated}"
    );
}


#[test]
fn sweep_removes_orphans_and_temps_but_keeps_live_entries() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-sweep-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('live01', 1, 'image')",
        [],
    )
    .unwrap();

    for hash in ["live01", "orphan"] {
        for path in [cache.thumb(hash), cache.preview(hash)] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"webp-bytes").unwrap();
        }
    }
    let stray_tmp = cache.thumb("live01").with_file_name("live01-xyz.tmp");
    std::fs::write(&stray_tmp, b"partial").unwrap();

    let removed = startup_sweep(&conn, &cache).unwrap();
    assert_eq!(removed, 3); // orphan thumb + orphan preview + stray tmp
    assert!(cache.thumb("live01").exists());
    assert!(cache.preview("live01").exists());
    assert!(!cache.thumb("orphan").exists());
    assert!(!stray_tmp.exists());

    // remove_entries drops a live pair on demand (the synchronous half).
    remove_entries(&cache, "live01");
    assert!(!cache.thumb("live01").exists());
    assert!(!cache.preview("live01").exists());
}

#[test]
fn cache_move_copies_subtrees_verifies_sizes_and_spares_bystanders() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-cachemove-")
        .tempdir()
        .unwrap();
    let old_root = dir.path().join("old");
    let new_root = dir.path().join("new");
    let cache = CachePaths::new(old_root.clone());
    for (path, bytes) in [
        (cache.thumb("aa11"), b"thumb-bytes".as_slice()),
        (cache.preview("aa11"), b"preview-bytes-longer"),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    // A stranded temp is not carried; a bystander file outside the cache
    // subtrees is neither copied nor deleted.
    std::fs::write(cache.thumb("aa11").with_file_name("aa11-x.tmp"), b"partial").unwrap();
    std::fs::write(old_root.join("bystander.txt"), b"not ours").unwrap();

    let reports: std::cell::RefCell<Vec<(u64, u64)>> = std::cell::RefCell::new(Vec::new());
    let moved =
        move_cache_tree(&old_root, &new_root, &|c, t| reports.borrow_mut().push((c, t)))
            .unwrap();
    assert_eq!(moved, 11 + 20);
    let reports = reports.into_inner();
    assert_eq!(reports.first(), Some(&(0, 31)));
    assert_eq!(reports.last(), Some(&(31, 31)));

    let new_cache = CachePaths::new(new_root.clone());
    assert_eq!(std::fs::read(new_cache.thumb("aa11")).unwrap(), b"thumb-bytes");
    assert!(!new_cache.thumb("aa11").with_file_name("aa11-x.tmp").exists());

    remove_cache_subtrees(&old_root);
    assert!(!old_root.join("thumbs").exists());
    assert!(
        old_root.join("bystander.txt").exists(),
        "a user-picked folder's own content must survive"
    );
}

#[test]
fn derive_tees_the_real_hash_out_of_a_provisional_image() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derive-tee-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let src = gradient_jpeg(dir.path(), "solo.jpg", 400, 300);

    // A provisionally-identified image (unique size, never read).
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('p1', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'solo.jpg', 'image', 'p1');",
        src.display(),
        dir.path().display(),
    ))
    .unwrap();

    let stats = derive_images_pending(&conn, &cache, 320, 1600, None).unwrap();
    assert_eq!((stats.derived, stats.failed), (1, 0));

    // The decode's read teed the REAL hash: identity promoted, cache
    // written under the real key, provisional gone everywhere.
    let real = blake3::hash(&std::fs::read(&src).unwrap()).to_hex().to_string();
    let stored: String = conn
        .query_row("SELECT content_hash FROM paths LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored, real);
    let provisional_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM contents WHERE hash GLOB 'p*'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(provisional_left, 0);
    assert!(cache.thumb(&real).exists());
    assert!(!cache.thumb("p1").exists());
}

#[test]
fn derive_pending_processes_images_once_and_flags_decode_failures() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derive-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));

    let good = gradient_jpeg(dir.path(), "good.jpg", 800, 600);
    let bad = dir.path().join("bad.jpg");
    std::fs::write(&bad, b"not a jpeg at all").unwrap();

    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('good01', 1, 'image');
         INSERT INTO contents (hash, byte_size, kind) VALUES ('bad001', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'good.jpg', 'image', 'good01');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'bad.jpg', 'image', 'bad001');",
        good.display(),
        dir.path().display(),
        bad.display(),
        dir.path().display(),
    ))
    .unwrap();

    let stats = derive_images_pending(&conn, &cache, 320, 1600, None).unwrap();
    assert_eq!((stats.derived, stats.failed), (1, 1));
    assert!(cache.preview("good01").exists());

    let issue_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM issues WHERE kind = 'decode-error'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issue_count, 1);

    // A second pass has nothing left to do — failures are not retried.
    let again = derive_images_pending(&conn, &cache, 320, 1600, None).unwrap();
    assert_eq!((again.derived, again.failed), (0, 0));

    // The good row carries dimensions + sharpness.
    let (w, h): (i64, i64) = conn
        .query_row(
            "SELECT width, height FROM contents WHERE hash = 'good01'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((w, h), (800, 600));
}
