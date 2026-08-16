// The libheif acceleration's contracts. The absent case runs everywhere; the
// present case is CONDITIONAL — it skips with a message on hosts without a
// system libheif, and where one exists it decodes the committed orientation
// fixtures and must agree with the pinned expectations the ffmpeg route is
// held to: dimensions AND where the colour lands, because dimensions alone
// cannot tell one quarter turn from three.

use onecopy_lib::heif_native;

#[test]
fn absence_is_a_plain_error_never_a_panic() {
    // On a host WITHOUT libheif this exercises the whole absent branch; on a
    // host WITH it, decode of garbage bytes must error cleanly instead.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-heif-")
        .tempdir()
        .unwrap();
    let bogus = dir.path().join("not-a-heic.heic");
    std::fs::write(&bogus, b"garbage").unwrap();
    let result = heif_native::decode(&bogus);
    assert!(result.is_err(), "garbage can never decode");
}

#[test]
fn present_host_agrees_with_the_orientation_fixtures() {
    if !heif_native::available() {
        eprintln!("SKIP: no system libheif on this host — install it (brew install libheif) to run the present-branch checks");
        return;
    }

    // upright.heic: 160×90, red filling the stored LEFT half — the exact
    // truths live_still_decode_through_ffmpeg pins for the ffmpeg route.
    let upright = heif_native::decode(std::path::Path::new("tests/fixtures/upright.heic"))
        .expect("the committed fixture must decode");
    assert_eq!((upright.width(), upright.height()), (160, 90));
    let rgb = upright.to_rgb8();
    let left = rgb.get_pixel(4, 45).0;
    let right = rgb.get_pixel(155, 45).0;
    assert!(left[0] > 150 && left[2] < 100, "red belongs left, found {left:?}");
    assert!(right[2] > 150 && right[0] < 100, "blue belongs right, found {right:?}");

    // rotated.heic: stored 160×90, displayed a quarter turn round. 90×160
    // means the rotation was applied EXACTLY once — skipping it leaves
    // 160×90, and applying the EXIF orientation on top of the one already
    // performed turns it back the long way.
    let rotated = heif_native::decode(std::path::Path::new("tests/fixtures/rotated.heic"))
        .expect("the committed fixture must decode");
    assert_eq!(
        (rotated.width(), rotated.height()),
        (90, 160),
        "rotation applied exactly once"
    );

    // Dimensions cannot tell one quarter turn from three: the stored-left red
    // must land along the TOP, exactly where the ffmpeg route's test pins it.
    let rgb = rotated.to_rgb8();
    let top = rgb.get_pixel(45, 4).0;
    let bottom = rgb.get_pixel(45, 155).0;
    assert!(top[0] > 150 && top[2] < 100, "red belongs on top, found {top:?}");
    assert!(
        bottom[2] > 150 && bottom[0] < 100,
        "blue belongs on the bottom, found {bottom:?}"
    );
}

// ---- The decode-path matrix (the developer's four paths) ----
//
// HEIC decodes travel one of two routes — the managed ffmpeg, or a system
// libheif — and both must produce the fixtures' pinned truths on macOS AND
// Windows: four paths. The env kill switch (`ONECOPY_NO_LIBHEIF`) is what
// lets ONE machine exercise both of its routes; the plan's gated task runs
// this same file on the Windows box. Every test touching the switch shares
// one serial key so the process-global env cannot race the others.

use onecopy_lib::preview::decode_image;

fn pinned_upright(rgb: &image::RgbImage) {
    assert_eq!(rgb.dimensions(), (160, 90));
    let left = rgb.get_pixel(4, 45).0;
    let right = rgb.get_pixel(155, 45).0;
    assert!(left[0] > 150 && left[2] < 100, "red belongs left, found {left:?}");
    assert!(right[2] > 150 && right[0] < 100, "blue belongs right, found {right:?}");
}

fn pinned_rotated(rgb: &image::RgbImage) {
    assert_eq!(rgb.dimensions(), (90, 160), "rotation applied exactly once");
    let top = rgb.get_pixel(45, 4).0;
    let bottom = rgb.get_pixel(45, 155).0;
    assert!(top[0] > 150 && top[2] < 100, "red belongs on top, found {top:?}");
    assert!(bottom[2] > 150 && bottom[0] < 100, "blue belongs on the bottom, found {bottom:?}");
}

#[test]
#[serial_test::serial(libheif_env)]
fn libheif_route_decodes_through_the_routing_seam_without_ffmpeg() {
    // CONDITIONAL (skips without a system libheif): the ROUTE ITSELF —
    // decode_image, not the binding — serves a HEIC with NO ffmpeg at hand,
    // and its output matches the pinned truths. Orientation returns 1: the
    // route hands back an already-upright image exactly as ffmpeg does.
    std::env::remove_var("ONECOPY_NO_LIBHEIF");
    if !heif_native::available() {
        eprintln!("SKIP: no system libheif on this host — install it (brew install libheif / the setup script) to run the libheif route");
        return;
    }
    let (upright, orientation) =
        decode_image(std::path::Path::new("tests/fixtures/upright.heic"), None).unwrap();
    assert_eq!(orientation, 1);
    pinned_upright(&upright.to_rgb8());
    let (rotated, _) =
        decode_image(std::path::Path::new("tests/fixtures/rotated.heic"), None).unwrap();
    pinned_rotated(&rotated.to_rgb8());
}

#[test]
#[serial_test::serial(libheif_env)]
fn without_libheif_the_route_demands_ffmpeg_honestly() {
    // Runs everywhere: with libheif ruled out, a HEIC and no ffmpeg is a
    // plain honest error — the gate the wizard's skippable offer relies on.
    std::env::set_var("ONECOPY_NO_LIBHEIF", "1");
    assert!(!heif_native::available(), "the kill switch must rule libheif out");
    let result = decode_image(std::path::Path::new("tests/fixtures/upright.heic"), None);
    std::env::remove_var("ONECOPY_NO_LIBHEIF");
    let err = result.expect_err("no route can exist without either dependency");
    assert!(err.contains("ffmpeg"), "the error names the remedy, got: {err}");
}

// Run with `cargo test both_decode_routes -- --ignored --nocapture`.
// This ONE command per machine proves that machine's half of the matrix:
// on the Mac it is paths 1+2, on the Windows box paths 3+4.
#[test]
#[ignore]
#[serial_test::serial(libheif_env)]
#[serial_test::serial(backup_store)]
fn both_decode_routes_agree_on_the_pinned_fixtures() {
    use onecopy_lib::binaries_manager;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-routes-live-")
        .tempdir()
        .unwrap();
    binaries_manager::install_or_update(dir.path(), |_, _| {}).expect("ffmpeg install");
    let ffmpeg = binaries_manager::ffmpeg_path(dir.path());

    // Route 1: ffmpeg, forced even where libheif exists.
    std::env::set_var("ONECOPY_NO_LIBHEIF", "1");
    let (ff_upright, _) =
        decode_image(std::path::Path::new("tests/fixtures/upright.heic"), Some(&ffmpeg)).unwrap();
    let (ff_rotated, _) =
        decode_image(std::path::Path::new("tests/fixtures/rotated.heic"), Some(&ffmpeg)).unwrap();
    std::env::remove_var("ONECOPY_NO_LIBHEIF");
    pinned_upright(&ff_upright.to_rgb8());
    pinned_rotated(&ff_rotated.to_rgb8());

    // Route 2: libheif, when this host has one.
    if !heif_native::available() {
        eprintln!("HALF-RUN: ffmpeg route proven; no system libheif on this host for the other half");
        return;
    }
    let (lh_upright, _) =
        decode_image(std::path::Path::new("tests/fixtures/upright.heic"), None).unwrap();
    let (lh_rotated, _) =
        decode_image(std::path::Path::new("tests/fixtures/rotated.heic"), None).unwrap();
    pinned_upright(&lh_upright.to_rgb8());
    pinned_rotated(&lh_rotated.to_rgb8());

    // Same shapes from both routes — the agreement the fallback depends on.
    assert_eq!(
        (ff_upright.width(), ff_upright.height()),
        (lh_upright.width(), lh_upright.height())
    );
    assert_eq!(
        (ff_rotated.width(), ff_rotated.height()),
        (lh_rotated.width(), lh_rotated.height())
    );
    eprintln!("both decode routes agree on this machine");
}
