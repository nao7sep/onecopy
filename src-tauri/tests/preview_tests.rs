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

    let facts = generate_for_image(&src, "abcd1234", &cache, 320, 1600, None).unwrap();
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
            "SELECT COUNT(*) FROM issues WHERE kind = 'decode-error'",
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
        .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
        .unwrap();
    assert_eq!(issues, 0);
    let marker: String = conn
        .query_row(
            "SELECT derived_at_utc FROM contents WHERE hash = 'heic01'",
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
            "SELECT derived_at_utc FROM contents WHERE hash = 'large01'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
        NEEDS_FFMPEG,
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0,
    );
}

// Live end-to-end for the ffmpeg still route — THE still route since Phase 33
// dropped the libheif accelerator: installs (or reuses) ffmpeg, derives the
// two committed HEIC fixtures, and proves the orientation rule.
// Run with `cargo test live_still_decode -- --ignored --nocapture`.
#[test]
#[ignore]
#[serial_test::serial(backup_store)]
fn live_still_decode_through_ffmpeg() {
    use onecopy_lib::binaries_manager;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-still-live-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    binaries_manager::install_entry(root, "ffmpeg", |_| {}).expect("ffmpeg install");
    let ffmpeg = binaries_manager::ffmpeg_path(root);

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let conn = index_store::open(&root.join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(root.join("cache"));

    // A HEIC stored upright, and one whose display orientation is a quarter
    // turn away. These small fixtures pin orientation; the canonical
    // acceptance corpus separately exercises Apple's tiled HEIC structure.
    for (hash, name) in [("up01", "upright.heic"), ("rot01", "rotated.heic")] {
        let src = fixtures.join(name);
        assert!(src.is_file(), "fixture {name} is committed");
        conn.execute_batch(&format!(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('{hash}', 1, 'image');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
               VALUES ('{}', '{}', '{name}', 'image', '{hash}');",
            src.display(),
            fixtures.display(),
        ))
        .unwrap();
    }

    // Without ffmpeg they are blocked, not failed — the wizard's skippable
    // offer in one step.
    let skipped = derive_images_pending(&conn, &cache, 320, 1600, None, None).unwrap();
    assert_eq!((skipped.derived, skipped.blocked_no_ffmpeg), (0, 2));

    // Installing it is the whole remedy: the blocked rows come straight back.
    let stats = derive_images_pending(&conn, &cache, 320, 1600, Some(&ffmpeg), None).unwrap();
    assert_eq!(
        (stats.derived, stats.failed, stats.blocked_no_ffmpeg),
        (2, 0, 0),
        "both HEICs derive once ffmpeg is present"
    );

    let dims = |hash: &str| -> (i64, i64) {
        conn.query_row(
            "SELECT width, height FROM contents WHERE hash = ?1",
            [hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    };
    assert_eq!(dims("up01"), (160, 90));
    // Stored 160×90, displayed a quarter turn round. 90×160 means the
    // rotation was applied EXACTLY once: skipping it leaves 160×90, and
    // applying the file's EXIF orientation on top of the one ffmpeg already
    // performed turns it back to 160×90 the long way.
    assert_eq!(dims("rot01"), (90, 160), "rotation applied exactly once");

    // Dimensions alone cannot tell a quarter turn from three, so check where
    // the colour landed: the fixture is red on the stored left half, which a
    // correct clockwise quarter turn puts along the TOP.
    let preview = image::open(cache.preview("rot01")).unwrap().to_rgb8();
    let (w, h) = preview.dimensions();
    let top = preview.get_pixel(w / 2, 4).0;
    let bottom = preview.get_pixel(w / 2, h - 5).0;
    assert!(top[0] > 150 && top[2] < 100, "red belongs on top, found {top:?}");
    assert!(
        bottom[2] > 150 && bottom[0] < 100,
        "blue belongs on the bottom, found {bottom:?}"
    );

    // The preview must be a format the webview can actually paint: never a
    // byte-copy of the HEIC original.
    let bytes = std::fs::read(cache.preview("rot01")).unwrap();
    assert!(bytes.starts_with(b"RIFF"), "preview is re-encoded WebP");
}

// The same route carries AVIF, which the image crate also cannot open. The
// fixture is generated rather than committed: ffmpeg can write AVIF itself.
#[test]
#[ignore]
#[serial_test::serial(backup_store)]
fn live_avif_decode_through_ffmpeg() {
    use onecopy_lib::binaries_manager;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-avif-live-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    binaries_manager::install_entry(root, "ffmpeg", |_| {}).expect("ffmpeg install");
    let ffmpeg = binaries_manager::ffmpeg_path(root);

    let src = gradient_jpeg(root, "seed.jpg", 240, 160);
    let avif = root.join("still.avif");
    let status = std::process::Command::new(&ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&src)
        .args(["-c:v", "libaom-av1", "-still-picture", "1", "-y"])
        .arg(&avif)
        .status()
        .unwrap();
    assert!(status.success(), "avif fixture written");

    let conn = index_store::open(&root.join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(root.join("cache"));
    conn.execute_batch(&format!(
        "INSERT INTO contents (hash, byte_size, kind) VALUES ('avif01', 1, 'image');
         INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
           VALUES ('{}', '{}', 'still.avif', 'image', 'avif01');",
        avif.display(),
        root.display(),
    ))
    .unwrap();

    let stats = derive_images_pending(&conn, &cache, 320, 1600, Some(&ffmpeg), None).unwrap();
    assert_eq!((stats.derived, stats.failed), (1, 0));
    assert!(cache.thumb("avif01").exists());
    let (w, h): (i64, i64) = conn
        .query_row("SELECT width, height FROM contents WHERE hash = 'avif01'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((w, h), (240, 160));
}

#[test]
fn a_stale_derive_version_makes_a_row_pending_again() {
    // Both derive passes checkpoint on derived_at_utc alone, and only a
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
        "UPDATE contents SET derived_at_utc = 'failed', derived_version = 0 WHERE hash = 'good01'",
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

// SCALE: measures the longest uncancellable portion of a native still-image
// job. The supplied image must remain below the decoder's allocation ceiling,
// otherwise the production route deliberately hands it to cancellable ffmpeg.
// Run with ONECOPY_TEST_LARGE_STILL=/path/to/image and --ignored --nocapture.
#[test]
#[ignore]
fn scale_native_still_safe_boundary_cost() {
    let source = std::env::var_os("ONECOPY_TEST_LARGE_STILL")
        .map(PathBuf::from)
        .expect("set ONECOPY_TEST_LARGE_STILL to a disposable large image");
    assert!(source.is_file(), "large still fixture exists");
    assert!(!needs_ffmpeg_decode(&source), "fixture uses the native decode route");

    let dir = tempfile::Builder::new()
        .prefix("onecopy-native-still-live-")
        .tempdir()
        .unwrap();
    let cache = CachePaths::new(dir.path().join("cache"));
    let started = std::time::Instant::now();
    let facts = generate_for_image(&source, "large-native", &cache, 320, 1600, None).unwrap();
    let elapsed = started.elapsed();
    eprintln!(
        "native still {}x{} safe boundary: {elapsed:?}",
        facts.width, facts.height
    );
    assert!(cache.thumb("large-native").is_file());
    assert!(cache.preview("large-native").is_file());
}

// SCALE Windows counterpart to the native-boundary measurement: the supplied
// still must exceed the target's native ceiling, then complete through the
// verified managed ffmpeg route. Run with ONECOPY_TEST_LARGE_STILL and
// --release --ignored --nocapture.
#[cfg(windows)]
#[test]
#[ignore]
#[serial_test::serial(backup_store)]
fn scale_windows_oversized_still_uses_managed_ffmpeg() {
    use onecopy_lib::binaries_manager;

    let source = std::env::var_os("ONECOPY_TEST_LARGE_STILL")
        .map(PathBuf::from)
        .expect("set ONECOPY_TEST_LARGE_STILL to a disposable oversized image");
    let native_error = onecopy_lib::resource_limits::decode_file(&source)
        .expect_err("fixture exceeds the Windows native decode boundary");
    assert!(onecopy_lib::resource_limits::is_decode_limit(&native_error));

    let dir = tempfile::Builder::new()
        .prefix("onecopy-windows-still-fallback-live-")
        .tempdir()
        .unwrap();
    let root = dir.path();
    binaries_manager::install_entry(root, "ffmpeg", |progress| eprintln!("{progress:?}"))
        .expect("managed ffmpeg install");
    let ffmpeg = binaries_manager::ffmpeg_path(root);
    let cache = CachePaths::new(root.join("cache"));
    let started = std::time::Instant::now();
    let facts = generate_for_image(
        &source,
        "large-windows-fallback",
        &cache,
        320,
        1600,
        Some(&ffmpeg),
    )
    .unwrap();
    eprintln!(
        "managed ffmpeg still fallback {}x{}: {:?}",
        facts.width,
        facts.height,
        started.elapsed(),
    );
    assert!(cache.thumb("large-windows-fallback").is_file());
    assert!(cache.preview("large-windows-fallback").is_file());
}
