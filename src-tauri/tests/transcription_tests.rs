// Transcription's model-free contracts. Real models belong to the separately
// prepared live-integration and benchmark surfaces, so the production model
// never enters an ordinary test run.

use std::path::Path;

use onecopy_lib::transcription::*;
use serial_test::serial;

#[test]
#[serial(transcription)]
fn one_claim_blocks_every_contender_without_cross_cancelling() {
    let active = claim().unwrap();
    assert!(request_cancel(), "an active run accepts cancellation");
    assert!(is_cancelled());

    assert_eq!(claim().unwrap_err(), TRANSCRIPTION_BUSY);
    assert!(
        is_cancelled(),
        "a rejected contender never resets the active run's cancellation"
    );

    drop(active);
    assert!(!is_cancelled(), "the owner clears cancellation on release");
    assert!(!request_cancel(), "there is no later run to cross-cancel");
    drop(claim().unwrap());
}

#[test]
fn rendering_formats_timestamps_and_drops_empty_segments() {
    let segments = vec![
        Segment { start_ms: 1_000, text: "hello".into() },
        Segment { start_ms: 0, text: String::new() }, // engine noise — dropped
        Segment { start_ms: 75_000, text: "world".into() },
    ];
    assert_eq!(render(&segments), "[0:01] hello\n[1:15] world\n");
    assert_eq!(render(&[]), "", "no speech is a successful empty transcript");
}

#[test]
fn only_ffmpegs_explicit_no_audio_results_become_empty_success() {
    assert!(no_audio_output(
        "Stream map '0:a:0' matches no streams. To ignore this, add a trailing '?'"
    ));
    assert!(no_audio_output("Output file #0 does not contain any stream"));
    assert!(!no_audio_output("Invalid data found when processing input"));
}

#[test]
fn digital_silence_never_reaches_whisper() {
    assert!(!has_audible_signal(&[]));
    assert!(!has_audible_signal(&[0.0; 128]));
    assert!(!has_audible_signal(&[
        1.0 / i16::MAX as f32,
        -1.0 / i16::MAX as f32,
    ]));
    assert!(has_audible_signal(&[2.0 / i16::MAX as f32]));
}

#[cfg(unix)]
#[test]
#[serial(transcription)]
fn a_video_without_audio_extracts_as_a_successful_empty_stream() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-no-audio-")
        .tempdir()
        .unwrap();
    let ffmpeg = dir.path().join("ffmpeg");
    std::fs::write(
        &ffmpeg,
        "#!/bin/sh\necho \"Stream map '0:a:0' matches no streams\" >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();

    let pcm = extract_pcm(
        &ffmpeg,
        Path::new("/nonexistent/silent.mov"),
        dir.path(),
    )
    .unwrap();
    assert!(pcm.is_empty());
}

#[cfg(unix)]
#[test]
#[serial(transcription)]
fn pcm_is_staged_then_streamed_once_into_the_float_buffer() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let ffmpeg = dir.path().join("ffmpeg");
    std::fs::write(
        &ffmpeg,
        "#!/bin/sh\nfor last do :; done\nprintf '\\000\\000\\000\\077\\000\\000\\000\\277' > \"$last\"\necho progress=continue\n",
    )
    .unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();

    let temp = dir.path().join("temp");
    let pcm = extract_pcm(&ffmpeg, Path::new("ignored.mov"), &temp).unwrap();
    assert_eq!(pcm, vec![0.5, -0.5]);
    assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
}
