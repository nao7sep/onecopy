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
