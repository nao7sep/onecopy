// Transcription's model-free contracts (the always-on majority per the Phase
// 28 test doctrine) plus the ignored LIVE test that proves the linked engine
// with the TINY model and the canonical public-domain speech sample — the
// same pair whisper.cpp itself tests with — so the 1.6 GB default never has
// to enter a test run.

use std::path::Path;

use onecopy_lib::preview::CachePaths;
use onecopy_lib::transcription::*;

fn cache(label: &str) -> (tempfile::TempDir, CachePaths) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-transcribe-{label}-"))
        .tempdir()
        .unwrap();
    let paths = CachePaths::new(dir.path().join("cache"));
    (dir, paths)
}

#[test]
fn an_absent_model_names_the_remedy() {
    let (_dir, cache) = cache("no-model");
    let err = transcribe_to_cache(
        &cache,
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
fn an_absent_ffmpeg_names_the_remedy_too() {
    let (_dir, cache) = cache("no-ffmpeg");
    let err = transcribe_to_cache(
        &cache,
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
    // The cache is the contract: once transcribed, opening the scenes modal
    // must never load the model again — proven by passing paths that would
    // explode if anything tried to use them.
    let (_dir, cache) = cache("cached");
    let target = cache.transcript("h2");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "[0:01] already transcribed\n").unwrap();

    let text = transcribe_to_cache(
        &cache,
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
fn a_failed_run_leaves_no_partial_cache_entry() {
    // Write-once-at-the-end is the design; a bogus ffmpeg fails extraction
    // long before any write, and the cache tree must stay empty.
    let (_dir, cache) = cache("failed");
    let err = transcribe_to_cache(
        &cache,
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
fn rendering_formats_timestamps_and_drops_empty_segments() {
    let segments = vec![
        Segment { start_ms: 1_000, text: "hello".into() },
        Segment { start_ms: 0, text: String::new() }, // engine noise — dropped
        Segment { start_ms: 75_000, text: "world".into() },
    ];
    assert_eq!(render(&segments), "[0:01] hello\n[1:15] world\n");
}

// LIVE: downloads the tiny model (~75 MB; sha256 from the upstream's LFS
// metadata) and the canonical jfk.wav sample, parses the WAV directly (16 kHz
// mono s16 — no ffmpeg needed here; production's ffmpeg extraction is covered
// by its own subprocess contracts), and asserts the engine hears the known
// phrase. Run: cargo test --test transcription_tests -- --ignored --nocapture
#[test]
#[ignore]
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
    let fetch = |url: &str, dest: &Path| {
        let mut response = ureq::get(url).call().expect("download");
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
    let sample = dir.path().join("jfk.wav");
    fetch(SAMPLE_URL, &sample);

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
