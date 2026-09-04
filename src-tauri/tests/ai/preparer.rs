//! Test-only managed-artifact preparer. It deliberately calls the production
//! registry and acquisition path; URLs, pins, platform rules, and publication
//! logic are never copied into JavaScript test infrastructure.

use std::path::Path;
use std::time::Instant;

use onecopy_lib::ai_dependencies::{self, Requirement};
use onecopy_lib::binaries::BinaryStatus;
use onecopy_lib::binaries_manager;
use serde_json::json;
use sha2::{Digest, Sha256};

fn usage() -> ! {
    eprintln!("usage: onecopy-ai-preparer <prepare|verify|live-face|live-transcription> ...");
    std::process::exit(2);
}

fn live_face(root: &Path, fixtures: Vec<String>) -> Result<(), String> {
    if fixtures.is_empty() {
        return Err("live-face requires at least one fixture".to_string());
    }
    ai_dependencies::require_prepared(root, &[Requirement::FaceScoring])?;
    let dependencies = ai_dependencies::production_face_scoring(root)
        .ok_or("prepared face-scoring dependencies are unavailable")?;
    let started = Instant::now();
    let mut scorer = onecopy_lib::face::FaceScorer::load(
        dependencies.runtime.as_deref(),
        &dependencies.detector,
        &dependencies.emotion,
    )?;
    let model_load_ms = started.elapsed().as_millis() as u64;
    let mut results = Vec::new();
    for fixture in fixtures {
        let path = Path::new(&fixture);
        let started = Instant::now();
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let image = onecopy_lib::resource_limits::decode_bytes(&bytes)?;
        let score = scorer.score(&image)?;
        results.push(json!({
            "basename": path.file_name().and_then(|name| name.to_str()).ok_or("fixture has no basename")?,
            "score": score,
            "wallMs": started.elapsed().as_millis() as u64,
        }));
    }
    println!(
        "{}",
        json!({
            "event": "live-result",
            "feature": "face",
            "requestedAcceleration": "none",
            "effectiveAcceleration": "none",
            "modelLoadMs": model_load_ms,
            "items": results,
        })
    );
    Ok(())
}

fn phrase_loop(segments: &[onecopy_lib::transcription::Segment]) -> bool {
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

fn live_transcription(
    root: &Path,
    scratch: &Path,
    acceleration: &str,
    fixture: &str,
    semantic_terms: Vec<String>,
) -> Result<(), String> {
    let mode = match acceleration {
        "none" => onecopy_lib::ai_acceleration::Mode::None,
        "metal" => onecopy_lib::ai_acceleration::Mode::Metal,
        _ => {
            return Err(format!(
                "unknown transcription acceleration: {acceleration}"
            ))
        }
    };
    onecopy_lib::ai_acceleration::require_supported(
        onecopy_lib::ai_acceleration::TRANSCRIPTION,
        mode,
    )?;
    ai_dependencies::require_prepared(root, &[Requirement::Transcription])?;
    let dependencies = ai_dependencies::production_transcription(root);
    let ffmpeg = dependencies
        .ffmpeg
        .ok_or("prepared ffmpeg dependency is unavailable")?;
    let model = dependencies
        .model
        .ok_or("prepared transcription model is unavailable")?;
    let extraction_started = Instant::now();
    let pcm = onecopy_lib::transcription::extract_pcm(&ffmpeg, Path::new(fixture), scratch)?;
    let extraction_ms = extraction_started.elapsed().as_millis() as u64;
    let inference_started = Instant::now();
    let segments = onecopy_lib::transcription::run_whisper(&model, &pcm, mode, |_| {})?;
    let inference_ms = inference_started.elapsed().as_millis() as u64;
    let rendered = onecopy_lib::transcription::render(&segments);
    let normalized = rendered.to_lowercase();
    let matched_terms = semantic_terms
        .iter()
        .filter(|term| normalized.contains(&term.to_lowercase()))
        .count();
    let digest = hex::encode(Sha256::digest(normalized.as_bytes()));
    println!(
        "{}",
        json!({
            "event": "live-result",
            "feature": "transcription",
            "requestedAcceleration": mode.id(),
            "effectiveAcceleration": mode.id(),
            "extractionMs": extraction_ms,
            "inferenceMs": inference_ms,
            "segmentCount": segments.len(),
            "matchedTerms": matched_terms,
            "phraseLoop": phrase_loop(&segments),
            "normalizedOutputSha256": digest,
        })
    );
    Ok(())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let action = args.next().unwrap_or_else(|| usage());
    let root = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| usage());
    if action == "live-face" {
        require_offline_mode()?;
        return live_face(&root, args.collect());
    }
    if action == "live-transcription" {
        require_offline_mode()?;
        let scratch = args
            .next()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| usage());
        let acceleration = args.next().unwrap_or_else(|| usage());
        let fixture = args.next().unwrap_or_else(|| usage());
        return live_transcription(&root, &scratch, &acceleration, &fixture, args.collect());
    }
    if action != "prepare" && action != "verify" {
        usage();
    }
    let requirements = args
        .map(|value| value.parse::<Requirement>())
        .collect::<Result<Vec<_>, _>>()?;
    if requirements.is_empty() {
        return Err("at least one AI dependency requirement is required".to_string());
    }
    if action == "prepare" {
        std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        binaries_manager::reset_temp_dir(&root);
        for id in ai_dependencies::dependency_ids(&requirements) {
            let spec = binaries_manager::spec_of(id)
                .ok_or_else(|| format!("dependency is not available on this platform: {id}"))?;
            if binaries_manager::state_of(&root, spec).status != BinaryStatus::UpToDate {
                binaries_manager::install_entry(&root, id, |progress| {
                    println!(
                        "{}",
                        json!({ "event": "dependency-progress", "id": id, "progress": progress })
                    );
                })?;
            }
        }
    } else {
        require_offline_mode()?;
    }
    let context = ai_dependencies::require_prepared(&root, &requirements)?;
    println!(
        "{}",
        json!({ "event": "prepared-context", "context": context })
    );
    Ok(())
}

fn require_offline_mode() -> Result<(), String> {
    if std::env::var("ONECOPY_AI_OFFLINE").as_deref() == Ok("1") {
        Ok(())
    } else {
        Err("AI verification and execution require ONECOPY_AI_OFFLINE=1".to_string())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
