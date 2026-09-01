// Transcription's model-free contracts (the always-on majority per the Phase
// 28 test doctrine) plus explicit ignored LIVE tests for the linked engine
// and production model behavior, so the 1.6 GB default never enters an
// ordinary test run.

use std::path::{Path, PathBuf};

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

// LIVE: downloads the tiny model (~75 MB; sha256 from the upstream's LFS
// metadata) and the canonical jfk.wav sample, parses the WAV directly (16 kHz
// mono s16 — no ffmpeg needed here; production's ffmpeg extraction is covered
// by its own subprocess contracts), and asserts the engine hears the known
// phrase. Run: cargo test --test transcription_tests -- --ignored --nocapture
#[test]
#[ignore]
#[serial(transcription)]
fn live_tiny_model_transcribes_the_canonical_sample() {
    const TINY_URL: &str =
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/98aa99a0a9db05ae2342309f5096248665f7cba3/ggml-tiny.bin";
    const TINY_SHA256: &str = "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21";
    const SAMPLE_URL: &str =
        "https://raw.githubusercontent.com/ggerganov/whisper.cpp/45f1593fd326b3435c04392e3151dff65967e523/samples/jfk.wav";

    let dir = tempfile::Builder::new()
        .prefix("onecopy-transcribe-live-")
        .tempdir()
        .unwrap();
    let agent = ureq::config::Config::builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent();
    let fetch = |url: &str, dest: &Path| {
        let mut response = agent.get(url).call().expect("download");
        let mut file = std::fs::File::create(dest).unwrap();
        std::io::copy(&mut response.body_mut().as_reader(), &mut file).unwrap();
    };
    let model = dir.path().join("ggml-tiny.bin");
    fetch(TINY_URL, &model);
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(std::fs::read(&model).unwrap());
        assert_eq!(hex::encode(hasher.finalize()), TINY_SHA256, "model integrity");
    }
    eprintln!(
        "model: {TINY_URL} ({} bytes, sha256 {TINY_SHA256})",
        std::fs::metadata(&model).unwrap().len()
    );
    let sample = dir.path().join("jfk.wav");
    fetch(SAMPLE_URL, &sample);
    eprintln!(
        "sample: {SAMPLE_URL} ({} bytes)",
        std::fs::metadata(&sample).unwrap().len()
    );

    // 16 kHz mono s16 WAV → f32 PCM: skip the 44-byte header, scale.
    let bytes = std::fs::read(&sample).unwrap();
    let pcm: Vec<f32> = bytes[44..]
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
        .collect();

    let segments = run_whisper(&model, &pcm, |p| eprintln!("progress {p}%")).unwrap();
    let text = render(&segments).to_lowercase();
    eprintln!("transcript: {text}");
    assert!(text.contains("country"), "the known phrase must be heard: {text}");
}

// LIVE: uses ONECOPY_TEST_WHISPER_MODEL when supplied, otherwise downloads
// the exact production model (~1.6 GB), transcribes the
// shared synthetic noisy dialogue with a long silent tail, and rejects the
// phrase-loop shape reported during dogfooding. This is deliberately separate
// from the tiny linked-engine smoke above: model behavior is the fact under
// test. Run explicitly by name with --ignored and one test thread.
#[test]
#[ignore]
#[serial(transcription)]
fn live_production_model_does_not_loop_a_phrase_into_trailing_silence() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-transcribe-production-live-")
        .tempdir()
        .unwrap();
    let model = production_model(dir.path());
    let ffmpeg = live_ffmpeg();
    let fixture = production_transcription_fixture();
    let extraction_started = std::time::Instant::now();
    let mut pcm = extract_pcm(&ffmpeg, &fixture, &dir.path().join("pcm")).unwrap();
    eprintln!("production PCM extraction: {:?}", extraction_started.elapsed());
    pcm.resize(pcm.len() + 45 * SAMPLE_RATE as usize, 0.0);

    let inference_started = std::time::Instant::now();
    let segments = run_whisper(&model, &pcm, |_| {}).unwrap();
    eprintln!("production inference: {:?}", inference_started.elapsed());
    for segment in &segments {
        eprintln!("{} ms: {}", segment.start_ms, segment.text);
    }
    let transcript = render(&segments);
    assert!(
        transcript.to_lowercase().contains("coordinates"),
        "the known fixture dialogue must be heard: {transcript}"
    );
    assert!(
        !segments_have_phrase_loop(&segments),
        "one decoder attempt emitted a repeated phrase loop: {}",
        transcript
    );
}

fn production_model(temp_dir: &Path) -> PathBuf {
    use sha2::Digest;

    let spec = onecopy_lib::binaries_manager::spec_of("whisper-large-v3-turbo")
        .expect("production transcription model is registered");
    let pin = spec.pinned.as_ref().expect("production model is pinned");
    let model = std::env::var_os("ONECOPY_TEST_WHISPER_MODEL")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let model = temp_dir.join(spec.file_name);
            let agent = ureq::config::Config::builder()
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .provider(ureq::tls::TlsProvider::NativeTls)
                        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                        .build(),
                )
                .build()
                .new_agent();
            let mut response = agent.get(pin.url).call().expect("model download");
            let mut output = std::fs::File::create(&model).unwrap();
            let downloaded =
                std::io::copy(&mut response.body_mut().as_reader(), &mut output).unwrap();
            assert_eq!(downloaded, pin.bytes, "model byte count");
            model
        });
    assert_eq!(
        std::fs::metadata(&model).unwrap().len(),
        pin.bytes,
        "model byte count"
    );
    let mut input = std::io::BufReader::new(std::fs::File::open(&model).unwrap());
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut input, &mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    assert_eq!(hex::encode(hasher.finalize()), pin.sha256, "model integrity");
    model
}

fn live_ffmpeg() -> PathBuf {
    let ffmpeg = std::env::var_os("ONECOPY_TEST_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/homebrew/bin/ffmpeg"));
    assert!(ffmpeg.is_file(), "live transcription needs ffmpeg");
    ffmpeg
}

fn production_transcription_fixture() -> PathBuf {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../company/assets/test-fixtures/video/dialogue/dialogue-english-with-noise.mp4");
    assert!(fixture.is_file(), "shared synthetic fixture checkout");
    fixture
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
