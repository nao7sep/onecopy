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
    // Small edges keep the decode and both resizes cheap; the limits apply
    // the same way at any scale.
    let src = gradient_jpeg(dir.path(), "big.jpg", 400, 200);
    let cache = CachePaths::new(dir.path().join("cache"));

    let facts = generate_for_image(&src, "abcd1234", &cache, 32, 160, None).unwrap();
    assert_eq!((facts.width, facts.height), (400, 200));

    let preview = image::open(cache.preview("abcd1234")).unwrap();
    assert_eq!((preview.width(), preview.height()), (160, 80));

    let thumb = image::open(cache.thumb("abcd1234")).unwrap();
    assert_eq!((thumb.width(), thumb.height()), (32, 16));

    // Sharded layout: thumbs/ab/abcd1234.webp.
    assert!(cache
        .thumb("abcd1234")
        .to_string_lossy()
        .contains(&format!("thumbs{}ab{}", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR)));
}

#[test]
fn built_in_image_extensions_have_one_proven_decode_route() {
    use std::collections::HashSet;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-image-format-matrix-")
        .tempdir()
        .unwrap();
    let pixels = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        3,
        2,
        image::Rgb([31, 127, 223]),
    ));
    let native = [
        ("jpg", image::ImageFormat::Jpeg),
        ("jpeg", image::ImageFormat::Jpeg),
        ("png", image::ImageFormat::Png),
        ("webp", image::ImageFormat::WebP),
        ("gif", image::ImageFormat::Gif),
        ("bmp", image::ImageFormat::Bmp),
        ("tif", image::ImageFormat::Tiff),
        ("tiff", image::ImageFormat::Tiff),
    ];
    for (extension, format) in native {
        let path = dir.path().join(format!("sample.{extension}"));
        pixels.save_with_format(&path, format).unwrap();
        let (decoded, orientation) = decode_image(&path, None).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2), "{extension}");
        assert_eq!(orientation, 1, "{extension}");
    }

    let ffmpeg: HashSet<&str> = ["heic", "heif", "hif", "avif"].into_iter().collect();
    let routed: HashSet<&str> = native
        .iter()
        .map(|(extension, _)| *extension)
        .chain(ffmpeg.iter().copied())
        .collect();
    assert_eq!(
        routed,
        onecopy_lib::extensions::IMAGE_EXTENSIONS
            .iter()
            .copied()
            .collect(),
        "every built-in image extension needs exactly one tested route"
    );
    for extension in ffmpeg {
        assert!(needs_ffmpeg_decode(Path::new(&format!("sample.{extension}"))));
    }
}

#[cfg(unix)]
#[test]
fn oversized_native_preview_retries_once_through_scaled_ffmpeg() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("oversized.bmp");
    let mut header = vec![0u8; 54];
    header[0..2].copy_from_slice(b"BM");
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&30_000i32.to_le_bytes());
    header[22..26].copy_from_slice(&30_000i32.to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&24u16.to_le_bytes());
    std::fs::write(&source, header).unwrap();

    let fallback = dir.path().join("fallback.bmp");
    image::RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30]))
        .save_with_format(&fallback, image::ImageFormat::Bmp)
        .unwrap();
    let ffmpeg = dir.path().join("fake-ffmpeg");
    std::fs::write(&ffmpeg, format!("#!/bin/sh\ncat '{}'\n", fallback.display())).unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o755)).unwrap();

    let cache = CachePaths::new(dir.path().join("cache"));
    let facts = generate_for_image(&source, "large", &cache, 320, 1600, Some(&ffmpeg)).unwrap();
    assert_eq!((facts.width, facts.height), (2, 2));
    assert!(cache.thumb("large").is_file());
    assert!(cache.preview("large").is_file());
}

#[test]
fn small_images_are_never_upscaled_and_skip_the_reencode() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-preview-small-")
        .tempdir()
        .unwrap();
    let src = gradient_jpeg(dir.path(), "small.jpg", 200, 100);
    let cache = CachePaths::new(dir.path().join("cache"));

    generate_for_image(&src, "ffff0000", &cache, 320, 1600, None).unwrap();

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

    // The THUMBNAIL is the half the grid actually renders, and nothing here
    // opened it: fit_long_edge's no-upscale early return was untested, so a
    // 200x100 source could have been blown up to the 320 thumb edge without
    // this failing.
    let thumb_bytes = std::fs::read(cache.thumb("ffff0000")).expect("a thumb was written");
    let thumb = image::load_from_memory(&thumb_bytes).expect("the thumb decodes");
    assert_eq!(
        (thumb.width(), thumb.height()),
        (200, 100),
        "a source smaller than the thumb edge is never upscaled"
    );
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
    // every other flat image — bounded-diameter clustering is what keeps
    // that corner of hash space from chaining into one mega-family.
    let flat = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(64, 64, image::Rgb([128; 3])));
    assert_eq!(dhash(&flat), 0);
}

#[test]
fn dhash_sees_what_the_user_sees_never_the_pixels_under_transparency() {
    // Two icons IDENTICAL on screen — an opaque bright square on a
    // transparent field — differing only in the RGB hidden under the
    // alpha-zero pixels. `to_luma8` alone reads that hidden RGB, so these
    // two hashed APART while genuinely different icons collided; the
    // analysis luminance composites over mid-gray instead.
    let icon = |hidden: [u8; 3]| {
        DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, y| {
            let inside = (16..48).contains(&x) && (16..48).contains(&y);
            if inside {
                image::Rgba([230, 230, 230, 255])
            } else {
                image::Rgba([hidden[0], hidden[1], hidden[2], 0])
            }
        }))
    };
    assert_eq!(
        dhash(&icon([0, 0, 0])),
        dhash(&icon([255, 20, 147])),
        "invisible pixels must not change the hash"
    );

    // And the backdrop is MID-gray so both polarities stay visible: a white
    // shape and a black shape on transparency must not collapse together.
    let shape = |v: u8| {
        DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 64, |x, y| {
            let inside = (16..48).contains(&x) && (16..48).contains(&y);
            if inside {
                image::Rgba([v, v, v, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        }))
    };
    assert_ne!(
        dhash(&shape(245)),
        dhash(&shape(10)),
        "white-on-transparent and black-on-transparent are different icons"
    );
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


/// A launch after every file the test wrote.
fn later() -> std::time::SystemTime {
    std::time::SystemTime::now() + std::time::Duration::from_secs(60)
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
    // A `paths` row is what makes `live01` live rather than leaked: since
    // `startup_sweep` now reconciles any `contents` row with no surviving
    // `paths` row before it walks the cache tree (D-L2's reconciler), a bare
    // `contents` row with nothing pointing at it is itself an orphan.
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
         VALUES ('/live01.jpg', '/', 'live01.jpg', 'image', 'live01')",
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

    let removed = startup_sweep(&conn, &cache, later(), &|| false).unwrap();
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
fn strip_frames_leave_with_their_video_and_survive_while_it_stays() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-strips-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('live01', 1, 'video')",
        [],
    )
    .unwrap();
    // See sweep_removes_orphans_and_temps_but_keeps_live_entries: a `paths`
    // row is what keeps `live01` from being reconciled away as leaked.
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) \
         VALUES ('/live01.mp4', '/', 'live01.mp4', 'video', 'live01')",
        [],
    )
    .unwrap();

    for hash in ["live01", "orphan"] {
        for index in 0..3u32 {
            let path = onecopy_lib::video::strip_path(&cache, hash, index);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"webp-bytes").unwrap();
        }
    }

    // A frame's name carries its number, so reading it as a whole hash would find no content
    // and condemn a frame whose video is still here.
    let removed = startup_sweep(&conn, &cache, later(), &|| false).unwrap();
    assert_eq!(removed, 3); // the orphan's three frames, and only those
    for index in 0..3u32 {
        assert!(onecopy_lib::video::strip_path(&cache, "live01", index).exists());
        assert!(!onecopy_lib::video::strip_path(&cache, "orphan", index).exists());
    }

    // Leaving the library takes the frames now, rather than at the next launch.
    remove_entries(&cache, "live01");
    for index in 0..3u32 {
        assert!(!onecopy_lib::video::strip_path(&cache, "live01", index).exists());
    }
}

#[test]
fn sweep_preserves_unvisited_cache_entries_after_shutdown_cancellation() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-sweep-cancel-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let orphan = cache.thumb("orphan");
    std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    std::fs::write(&orphan, b"webp-bytes").unwrap();

    let removed = startup_sweep(&conn, &cache, later(), &|| true).unwrap();

    assert_eq!(removed, 0);
    assert!(orphan.exists());
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
    let conn = index_store::open(&db_path).unwrap();
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
        let sweep_conn = index_store::open(&sweep_db_path).unwrap();
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

#[test]
fn on_demand_derive_returns_the_promoted_hash_for_a_provisional_image() {
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

    let canonical = derive_one(&conn, &cache, 320, 1600, None, "p1").unwrap();

    // The decode's read teed the REAL hash: identity promoted, cache
    // written under the real key, provisional gone everywhere.
    let real = blake3::hash(&std::fs::read(&src).unwrap()).to_hex().to_string();
    assert_eq!(canonical, real);
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

    let stats = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((stats.derived, stats.failed), (1, 1));
    assert_eq!(
        stats.changes,
        [
            ("bad001".to_string(), "bad001".to_string()),
            ("good01".to_string(), "good01".to_string()),
        ],
        "success and failure both publish their durable item transition"
    );
    assert!(cache.preview("good01").exists());

    let issue_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM active_issues WHERE kind = 'decode-error'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(issue_count, 1);

    // A second pass has nothing left to do — failures are not retried.
    let again = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((again.derived, again.failed), (0, 0));

    // The good row carries dimensions AND the two measurements the comparison
    // surface depends on. Nothing anywhere read phash or sharpness back after
    // a real derive: dhash is tested directly and the similarity tests
    // hand-insert phash values, so a wrong binding here would collapse every
    // image into one cluster and silently kill the whole feature.
    let (w, h, phash, sharpness): (i64, i64, Option<i64>, Option<f64>) = conn
        .query_row(
            "SELECT width, height, phash, sharpness FROM contents WHERE hash = 'good01'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!((w, h), (800, 600));
    assert!(phash.is_some(), "a derived image must carry a phash");
    let sharpness = sharpness.expect("a derived image must carry a sharpness");
    assert!(
        sharpness > 0.0,
        "sharpness orders a group best-first; zero would flatten it"
    );
    // The stored phash must be the dhash OF THE DERIVED PREVIEW — the link
    // between the two, not merely that some number landed in the column.
    // load_from_memory, not open(): the cache entry is named .webp but may
    // hold JPEG/PNG bytes (a displayable original is byte-copied rather than
    // re-encoded), so the extension is not the format. The protocol handler
    // sniffs for the same reason.
    let bytes = std::fs::read(cache.preview("good01")).expect("the preview exists");
    let preview = image::load_from_memory(&bytes).expect("the preview decodes");
    assert_eq!(
        phash.unwrap() as u64,
        dhash(&preview),
        "the stored phash is the derived preview's dhash"
    );
}

#[test]
fn the_ffmpeg_route_claims_exactly_the_formats_the_image_crate_cannot_open() {
    for name in ["a.heic", "a.HEIF", "a.hif", "a.avif", "a.HEIC"] {
        assert!(needs_ffmpeg_decode(Path::new(name)), "{name} needs ffmpeg");
    }
    for name in ["a.jpg", "a.jpeg", "a.png", "a.webp", "a.gif", "a.tif", "a.bmp", "a"] {
        assert!(!needs_ffmpeg_decode(Path::new(name)), "{name} decodes natively");
    }

    // The claim in the name is about the IMAGE CRATE, and restating our own
    // match arms against a hardcoded list cannot detect that crate drifting.
    // Actually invoking it makes a feature-flag change fail here instead of
    // shipping blank tiles. Encoded in memory so no fixture can rot.
    let img = DynamicImage::new_rgb8(4, 4);
    for format in [image::ImageFormat::Png, image::ImageFormat::Bmp, image::ImageFormat::Tiff] {
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, format)
            .unwrap_or_else(|e| panic!("the image crate must ENCODE {format:?}: {e}"));
        image::load_from_memory(bytes.get_ref())
            .unwrap_or_else(|e| panic!("the image crate must DECODE {format:?}: {e}"));
    }
    // And the other half of "exactly": a real HEIC the crate cannot open, so
    // the ffmpeg route is genuinely required rather than merely declared.
    let heic = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upright.heic"),
    )
    .expect("the committed HEIC fixture");
    assert!(
        image::load_from_memory(&heic).is_err(),
        "if the image crate learns HEIC, needs_ffmpeg_decode should shrink"
    );
}

#[test]
fn stills_needing_ffmpeg_wait_for_it_instead_of_failing() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derive-noffmpeg-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));

    // Never opened: without ffmpeg the route is decided by extension alone,
    // so the bytes are irrelevant to what this asserts.
    let heic = dir.path().join("photo.heic");
    std::fs::write(&heic, b"not read without ffmpeg").unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('heic01', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'photo.heic', 'image', 'heic01');",
        heic.display(),
        dir.path().display(),
    ))
    .unwrap();

    let stats = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((stats.derived, stats.failed, stats.blocked_no_ffmpeg), (0, 0, 1));
    assert_eq!(stats.changes, [("heic01".to_string(), "heic01".to_string())]);

    // Waiting on a tool is not a bad file: no issue row, and the marker is
    // distinct from `failed` so installing ffmpeg is enough to derive it.
    let issues: i64 = conn
        .query_row("SELECT COUNT(*) FROM active_issues", [], |r| r.get(0))
        .unwrap();
    assert_eq!(issues, 0);
    let marker: String = conn
        .query_row(
            "SELECT derive_outcome FROM contents WHERE hash = 'heic01'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marker, NEEDS_FFMPEG);

    // A second ffmpeg-less pass leaves it alone rather than re-marking it.
    let again = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((again.derived, again.blocked_no_ffmpeg), (0, 0));
}

#[cfg(windows)]
#[test]
fn windows_native_decode_limit_waits_for_ffmpeg_instead_of_failing_the_file() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-windows-decode-limit-")
        .tempdir()
        .unwrap();
    let source = gradient_jpeg(dir.path(), "over-native-boundary.jpg", 1664, 1664);
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('large01', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'over-native-boundary.jpg', 'image', 'large01');",
        source.display(),
        dir.path().display(),
    ))
    .unwrap();

    let stats = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((stats.derived, stats.failed, stats.blocked_no_ffmpeg), (0, 0, 1));
    assert_eq!(
        conn.query_row(
            "SELECT derive_outcome FROM contents WHERE hash = 'large01'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        NEEDS_FFMPEG,
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM active_issues", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0,
    );
}

#[test]
fn a_stale_derive_version_makes_a_row_pending_again() {
    // Both derive passes once checkpointed on derived_at_utc alone, and only a
    // changed source file ever cleared it — so a derive that completed with
    // wrong output stayed wrong for the life of the index and no rescan could
    // fix it. DERIVE_VERSION is the escape hatch: bumping it re-derives
    // everything without touching a user file.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-derive-version-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let src = gradient_jpeg(dir.path(), "a.jpg", 80, 60);
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('good01', 10, 'image')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing) \
         VALUES (?1, ?2, 'a.jpg', 'image', 'good01', 0)",
        rusqlite::params![src.to_string_lossy(), dir.path().to_string_lossy()],
    )
    .unwrap();

    let first = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!(first.derived, 1);
    // Current version: nothing left to do.
    let again = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!(again.derived, 0, "a current row is not re-derived");

    // Stamp it as produced by an older pipeline.
    conn.execute(
        "UPDATE contents SET derived_version = derived_version - 1 WHERE hash = 'good01'",
        [],
    )
    .unwrap();
    let after_bump = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!(after_bump.derived, 1, "a stale row derives again");

    // A permanent decode failure is NOT retried by a version bump: the file is
    // broken, not the pipeline, and retrying it every scan is the churn the
    // failed sentinel exists to prevent.
    conn.execute(
        "UPDATE contents SET derive_outcome = 'failed', derived_version = 0 WHERE hash = 'good01'",
        [],
    )
    .unwrap();
    let failed = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!(failed.derived, 0, "a failed row stays failed");
}

#[test]
fn ensure_fullres_short_circuits_and_reports_missing_ffmpeg_honestly() {
    // The two contracts that need no live ffmpeg: an existing entry returns
    // at once (the idempotence the 100% view leans on per keystroke), and a
    // missing ffmpeg is a plain actionable error, never a panic or a blank.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-fullres-")
        .tempdir()
        .unwrap();
    let db = dir.path().join("index.sqlite3");
    let conn = onecopy_lib::index_store::open(&db).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));

    // Pre-existing entry: no ffmpeg needed, no DB row needed.
    let target = cache.fullres("abc123");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"png-bytes").unwrap();
    assert!(ensure_fullres(&conn, &cache, None, "abc123").is_ok());

    // No entry and no ffmpeg: the error names the remedy.
    let err = ensure_fullres(&conn, &cache, None, "def456").unwrap_err();
    assert!(err.contains("Managed tools"), "{err}");
}

#[test]
fn full_resolution_renders_last_one_session_and_previews_stay() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-session-renders-")
        .tempdir()
        .unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    for path in [cache.fullres("abc123"), cache.preview("abc123")] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"bytes").unwrap();
    }
    clear_session_renders(&cache);
    assert!(!dir.path().join("cache").join("fullres").exists());
    assert!(cache.preview("abc123").exists());
    // Nothing left over is not an error.
    clear_session_renders(&cache);
}

#[test]
fn webp_entries_keep_transparency() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-webp-alpha-")
        .tempdir()
        .unwrap();
    let target = dir.path().join("cache").join("alpha.webp");
    let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(8, 8, |x, _| {
        image::Rgba([200, 40, 40, if x < 4 { 0 } else { 255 }])
    }));
    write_webp(&img, &target, 80.0).unwrap();
    let decoded = image::open(&target).unwrap().to_rgba8();
    assert_eq!(decoded.get_pixel(0, 0)[3], 0);
    assert_eq!(decoded.get_pixel(7, 0)[3], 255);
}

// Unix-only, gated at the ITEM so Windows is honestly MISSING this coverage
// rather than running it vacuously green: the failure is staged with a chmod
// 0o000 that Windows has no equivalent for.
//
// (R6-05) A full disk or unwritable cache is a lifecycle condition, not a
// bad file: the pass must stop and report one reason, not record a permanent
// per-item failure for every remaining item.
#[cfg(unix)]
#[test]
fn an_unwritable_cache_pauses_the_pass_instead_of_failing_every_item() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-derive-storage-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache_root = dir.path().join("cache");
    std::fs::create_dir_all(&cache_root).unwrap();
    let cache = CachePaths::new(cache_root.clone());

    let good = gradient_jpeg(dir.path(), "good.jpg", 800, 600);
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('good01', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'good.jpg', 'image', 'good01');",
        good.display(),
        dir.path().display(),
    ))
    .unwrap();

    std::fs::set_permissions(&cache_root, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = derive_images_pending(&conn, &cache, 320, 1600, None, None);
    std::fs::set_permissions(&cache_root, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = result.expect_err("an unwritable cache must fail the pass, not silently succeed");
    assert!(
        err.contains("cache storage unavailable"),
        "the pass must report the lifecycle condition, not a plain I/O message: {err}"
    );

    let failed_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM contents WHERE derive_outcome = 'failed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failed_rows, 0, "the item must not be recorded as permanently broken");
    let issues: i64 = conn.query_row("SELECT COUNT(*) FROM active_issues", [], |r| r.get(0)).unwrap();
    assert_eq!(issues, 0, "the derive pass itself records no per-item Issue for this condition");

    // Space returns: the same pending row derives normally on the next pass.
    let recovered = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((recovered.derived, recovered.failed), (1, 0));
}

/// Builds one fresh cache with an exact-hash entry and a provisional-key
/// entry in every purged tree (thumbs, previews, fullres, strips), returning
/// the two path lists for assertions.
fn seeded_rebuild_cache(dir: &Path) -> (CachePaths, Vec<PathBuf>, Vec<PathBuf>) {
    let cache = CachePaths::new(dir.join("cache"));
    let exact = "3f2a9c0d";
    let provisional = "p17";
    let mut exact_paths = vec![
        cache.thumb(exact),
        cache.preview(exact),
        cache.fullres(exact),
    ];
    exact_paths.push(onecopy_lib::video::strip_path(&cache, exact, 0));
    let mut provisional_paths = vec![
        cache.thumb(provisional),
        cache.preview(provisional),
        cache.fullres(provisional),
    ];
    provisional_paths.push(onecopy_lib::video::strip_path(&cache, provisional, 2));
    for path in exact_paths.iter().chain(&provisional_paths) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"bytes").unwrap();
    }
    (cache, exact_paths, provisional_paths)
}

#[test]
fn a_default_rebuild_discards_only_provisional_entries_and_keeps_content_addressed_previews() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-rebuild-cache-default-")
        .tempdir()
        .unwrap();
    let (cache, exact_paths, provisional_paths) = seeded_rebuild_cache(dir.path());

    purge_for_rebuild(&cache, false).unwrap();

    for path in &exact_paths {
        assert!(path.exists(), "{} was discarded by default", path.display());
    }
    for path in &provisional_paths {
        assert!(!path.exists(), "{} survived the rebuild", path.display());
    }
    // An empty or never-created cache is not an error.
    purge_for_rebuild(&cache, false).unwrap();
    purge_for_rebuild(&CachePaths::new(dir.path().join("absent")), false).unwrap();
}

#[test]
fn discarding_previews_also_removes_the_content_addressed_ones() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-rebuild-cache-previews-")
        .tempdir()
        .unwrap();
    let (cache, exact_paths, provisional_paths) = seeded_rebuild_cache(dir.path());

    purge_for_rebuild(&cache, true).unwrap();

    for path in exact_paths.iter().chain(&provisional_paths) {
        assert!(!path.exists(), "{} survived discarding previews", path.display());
    }
}

#[test]
fn the_sweep_never_collects_what_this_launch_is_writing() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-sweep-live-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let launched = std::time::SystemTime::now();
    let earlier = launched - std::time::Duration::from_secs(3600);
    // Left by an earlier run.
    let stale = cache.thumb("gone01");
    let stale_tmp = cache.thumb("gone01").with_file_name("gone01-old.tmp");
    // Written by this launch's derivation: staging, and an entry under a
    // real hash whose `contents` row the promotion has not created yet.
    let staging = cache.preview("fresh1").with_file_name("fresh1-new.tmp");
    let promoting = cache.preview("fresh1");
    for (path, modified) in [
        (&stale, Some(earlier)),
        (&stale_tmp, Some(earlier)),
        (&staging, None),
        (&promoting, None),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"bytes").unwrap();
        if let Some(modified) = modified {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
    }

    let removed = startup_sweep(&conn, &cache, launched, &|| false).unwrap();

    assert_eq!(removed, 2);
    assert!(!stale.exists() && !stale_tmp.exists());
    assert!(staging.exists(), "a staging file being written was removed");
    assert!(promoting.exists(), "an entry awaiting its promotion was removed");
}
