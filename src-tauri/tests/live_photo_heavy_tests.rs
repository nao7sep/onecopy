// A Live Photo movie's content identifier, written by the managed ffmpeg as a
// QuickTime metadata key, reads back through OneCopy's parser.

use onecopy_lib::binaries_manager;
use onecopy_lib::live_photo::{quicktime_content_identifier, CONTENT_IDENTIFIER_KEY};

use super::heavy_support::app_home;

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn an_identifier_written_by_ffmpeg_reads_back() {
    let home = app_home("live-photo");
    let clip = home.path().join("live.mov");
    let status = std::process::Command::new(binaries_manager::ffmpeg_path(home.path()))
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc=duration=2:size=320x240:rate=12")
        .args([
            "-metadata",
            &format!("{CONTENT_IDENTIFIER_KEY}=TEST-UUID-0001"),
            "-movflags",
            "use_metadata_tags",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg writes the clip");
    assert_eq!(
        quicktime_content_identifier(&clip).as_deref(),
        Some("TEST-UUID-0001")
    );
}
