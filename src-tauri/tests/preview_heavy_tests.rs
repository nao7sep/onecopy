// Still derivation through the managed ffmpeg: every image format OneCopy
// accepts decodes at its recorded size, and the ffmpeg route applies a HEIC's
// display orientation exactly once.

use std::path::Path;

use onecopy_lib::extensions::{lowercase_ext, IMAGE_EXTENSIONS};
use onecopy_lib::preview;

use super::heavy_support::{corpus_file, library, manifest, Library};

fn dimensions(library: &Library, file_name: &str) -> (i64, i64) {
    library
        .conn
        .query_row(
            "SELECT width, height FROM contents WHERE hash = ?1",
            [library.hash_of(file_name)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn derive(library: &Library, ffmpeg: Option<&Path>) -> preview::DeriveStats {
    preview::derive_images_pending(
        &library.conn,
        &library.cache,
        library.settings.thumb_edge,
        library.settings.preview_long_edge,
        ffmpeg,
        None,
    )
    .unwrap()
}

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn every_accepted_still_format_derives_at_its_recorded_size() {
    let stills: Vec<serde_json::Value> = manifest()
        .into_iter()
        .filter(|entry| {
            let path = entry["path"].as_str().unwrap();
            (path.starts_with("formats/image/") || path.starts_with("formats/animation/"))
                && IMAGE_EXTENSIONS.contains(&lowercase_ext(path).as_str())
        })
        .collect();
    let files: Vec<_> = stills
        .iter()
        .map(|entry| corpus_file(entry["path"].as_str().unwrap()))
        .collect();
    let library = library("stills", &files, serde_json::json!({}));

    let stats = derive(&library, Some(library.ffmpeg()));
    assert_eq!((stats.failed, stats.blocked_no_ffmpeg), (0, 0));

    for entry in &stills {
        let path = entry["path"].as_str().unwrap();
        let file_name = Path::new(path).file_name().unwrap().to_str().unwrap();
        let hash = library.hash_of(file_name);
        assert!(library.cache.thumb(&hash).is_file(), "{path} has a thumbnail");
        assert!(library.cache.preview(&hash).is_file(), "{path} has a preview");
        let probe = &entry["probe"];
        if let (Some(width), Some(height)) = (probe["width"].as_i64(), probe["height"].as_i64()) {
            assert_eq!(dimensions(&library, file_name), (width, height), "{path}");
        }
    }
}

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn a_heic_display_orientation_is_applied_exactly_once() {
    // A HEIC stored upright, and one whose display orientation is a quarter
    // turn away. Both need ffmpeg: without it they wait instead of failing.
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let library = library(
        "heic-orientation",
        &[fixtures.join("upright.heic"), fixtures.join("rotated.heic")],
        serde_json::json!({}),
    );
    let waiting = derive(&library, None);
    assert_eq!((waiting.derived, waiting.blocked_no_ffmpeg), (0, 2));

    let stats = derive(&library, Some(library.ffmpeg()));
    assert_eq!((stats.derived, stats.failed, stats.blocked_no_ffmpeg), (2, 0, 0));

    assert_eq!(dimensions(&library, "upright.heic"), (160, 90));
    // Stored 160×90, displayed a quarter turn round. 90×160 means the rotation
    // was applied exactly once: skipping it leaves 160×90, and applying the
    // file's orientation on top of the one ffmpeg already performed turns it
    // back to 160×90 the long way.
    assert_eq!(dimensions(&library, "rotated.heic"), (90, 160));

    // Dimensions alone cannot tell a quarter turn from three, so check where
    // the colour landed: the fixture is red on the stored left half, which a
    // correct clockwise quarter turn puts along the top.
    let rotated = library.cache.preview(&library.hash_of("rotated.heic"));
    let pixels = image::open(&rotated).unwrap().to_rgb8();
    let (width, height) = pixels.dimensions();
    let top = pixels.get_pixel(width / 2, 4).0;
    let bottom = pixels.get_pixel(width / 2, height - 5).0;
    assert!(top[0] > 150 && top[2] < 100, "red belongs on top, found {top:?}");
    assert!(bottom[2] > 150 && bottom[0] < 100, "blue belongs on the bottom, found {bottom:?}");

    // The preview is re-encoded for the webview, never a copy of the HEIC.
    assert!(std::fs::read(&rotated).unwrap().starts_with(b"RIFF"));
}
