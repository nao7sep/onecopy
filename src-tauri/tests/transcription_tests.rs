// Transcription's model-free contracts. Real models belong to the separately
// prepared live-integration and benchmark surfaces, so the production model
// never enters an ordinary test run.

use std::path::Path;

use onecopy_lib::preview::CachePaths;
use onecopy_lib::transcription::*;
use serial_test::serial;


fn cache(label: &str) -> (tempfile::TempDir, CachePaths) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-transcribe-{label}-"))
        .tempdir()
        .unwrap();
    let paths = CachePaths::new(dir.path().join("cache"));
    (dir, paths)
}

#[test]
#[serial(transcription)]
fn an_absent_model_names_the_remedy() {
    let (dir, cache) = cache("no-model");
    let err = transcribe_to_cache(
        &cache,
        &dir.path().join("temp"),
        None,
        Some(Path::new("/nonexistent/ffmpeg")),
        Path::new("/nonexistent/video.mov"),
        "h1",
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("Managed tools"), "{err}");
}

#[test]
#[serial(transcription)]
fn an_absent_ffmpeg_names_the_remedy_too() {
    let (dir, cache) = cache("no-ffmpeg");
    let err = transcribe_to_cache(
        &cache,
        &dir.path().join("temp"),
        Some(Path::new("/nonexistent/model.bin")),
        None,
        Path::new("/nonexistent/video.mov"),
        "h1",
        |_| {},
    )
    .unwrap_err();
    assert!(err.contains("Managed tools"), "{err}");
}

#[test]
fn an_existing_transcript_short_circuits_without_touching_the_engine() {
    // The cache is the contract: once transcribed, opening the media surface
    // must never load the model again — proven by passing paths that would
    // explode if anything tried to use them.
    let (dir, cache) = cache("cached");
    let target = cache.transcript("h2");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "[0:01] already transcribed\n").unwrap();

    let text = transcribe_to_cache(
        &cache,
        &dir.path().join("temp"),
        None, // no model — and none needed
        None, // no ffmpeg either
        Path::new("/nonexistent/video.mov"),
        "h2",
        |_| {},
    )
    .unwrap();
    assert_eq!(text, "[0:01] already transcribed\n");
}

#[test]
#[serial(transcription)]
fn a_failed_run_leaves_no_partial_cache_entry() {
    // Write-once-at-the-end is the design; a bogus ffmpeg fails extraction
    // long before any write, and the cache tree must stay empty.
    let (dir, cache) = cache("failed");
    let err = transcribe_to_cache(
        &cache,
        &dir.path().join("temp"),
        Some(Path::new("/nonexistent/model.bin")),
        Some(Path::new("/nonexistent/ffmpeg")),
        Path::new("/nonexistent/video.mov"),
        "h3",
        |_| {},
    );
    assert!(err.is_err());
    assert!(!cache.transcript("h3").exists());
}

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

    let (dir, cache) = cache("single-flight");
    let error = transcribe_to_cache(
        &cache,
        &dir.path().join("temp"),
        Some(Path::new("/nonexistent/model.bin")),
        Some(Path::new("/nonexistent/ffmpeg")),
        Path::new("/nonexistent/video.mov"),
        "busy",
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error, TRANSCRIPTION_BUSY);
    assert!(is_cancelled());

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
fn extracted_digital_silence_publishes_empty_without_loading_a_model() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, cache) = cache("digital-silence");
    let ffmpeg = dir.path().join("ffmpeg");
    std::fs::write(
        &ffmpeg,
        "#!/bin/sh\nfor last do :; done\nprintf '\\000\\000\\000\\000\\000\\000\\000\\000' > \"$last\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&ffmpeg, std::fs::Permissions::from_mode(0o700)).unwrap();

    let text = transcribe_to_cache(
        &cache,
        &dir.path().join("temp"),
        Some(Path::new("/model-that-must-not-be-loaded")),
        Some(&ffmpeg),
        Path::new("/media-that-the-fake-ffmpeg-ignores"),
        "silence",
        |_| {},
    )
    .unwrap();

    assert!(text.is_empty());
    assert_eq!(std::fs::read_to_string(cache.transcript("silence")).unwrap(), "");
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

fn segments_have_phrase_loop(segments: &[Segment]) -> bool {
    let normalized = segments
        .iter()
        .map(|segment| {
            segment
                .text
                .split_whitespace()
                .map(|token| token.to_lowercase())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    if normalized
        .windows(2)
        .any(|pair| !pair[0].is_empty() && pair[0] == pair[1])
    {
        return true;
    }
    let tokens = normalized.into_iter().flatten().collect::<Vec<_>>();
    for width in 3..=16.min(tokens.len() / 3) {
        for start in 0..=tokens.len().saturating_sub(width * 3) {
            if tokens[start..start + width] == tokens[start + width..start + width * 2]
                && tokens[start..start + width] == tokens[start + width * 2..start + width * 3]
            {
                return true;
            }
        }
    }
    false
}

#[test]
fn phrase_loop_probe_distinguishes_decoder_loops_from_ordinary_repetition() {
    let segment = |text: &str| Segment {
        start_ms: 0,
        text: text.to_string(),
    };
    assert!(segments_have_phrase_loop(&[
        segment("please remove the coordinates"),
        segment("please remove the coordinates"),
    ]));
    assert!(segments_have_phrase_loop(&[segment(
        "please remove the coordinates please remove the coordinates please remove the coordinates"
    )]));
    assert!(!segments_have_phrase_loop(&[segment(
        "thank you, thank you for removing the location before sharing"
    )]));
    assert!(!segments_have_phrase_loop(&[
        segment("please remove the coordinates"),
        segment("then share the photograph"),
    ]));
}
