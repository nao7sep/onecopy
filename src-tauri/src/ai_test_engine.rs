//! Deterministic AI engines for fast integration. This module exists only
//! under `ai-test-engine`; it exercises production persistence and receipt
//! boundaries without loading or downloading a model.

use std::time::Duration;

#[derive(Clone, Debug)]
pub enum Outcome {
    Success(String),
    Empty,
    Failure(String),
}

#[derive(Clone, Debug)]
pub struct Scenario {
    pub outcome: Outcome,
    pub progress: Vec<u32>,
    pub delay_ms: u64,
    pub cancel_at: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    pub progress: Vec<u32>,
    pub published: bool,
}

fn advance(scenario: &Scenario) -> Result<Vec<u32>, String> {
    let mut observed = Vec::new();
    for (index, value) in scenario.progress.iter().copied().enumerate() {
        if scenario.cancel_at == Some(index) {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        if scenario.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(scenario.delay_ms));
        }
        observed.push(value.min(100));
    }
    Ok(observed)
}

pub fn transcribe(
    conn: &rusqlite::Connection,
    cache: &crate::preview::CachePaths,
    hash: &str,
    source_path: &str,
    replace_existing: bool,
    scenario: &Scenario,
) -> Result<RunResult, String> {
    let progress = advance(scenario)?;
    let text = match &scenario.outcome {
        Outcome::Success(text) => text.clone(),
        Outcome::Empty => String::new(),
        Outcome::Failure(error) => {
            if replace_existing {
                crate::derived_state::record_transcript_replacement_failure(
                    conn,
                    source_path,
                    error,
                )?;
            } else {
                crate::derived_state::record_transcript_failure(conn, hash, source_path, error)?;
            }
            return Err(error.clone());
        }
    };
    crate::transcription::publish_transcript(&cache.transcript(hash), &text)?;
    crate::derived_state::record_transcript_success(
        conn,
        hash,
        source_path,
        !text.trim().is_empty(),
    )?;
    Ok(RunResult {
        progress,
        published: true,
    })
}

pub fn score_face(
    conn: &rusqlite::Connection,
    hash: &str,
    source_path: &str,
    scenario: &Scenario,
) -> Result<RunResult, String> {
    let progress = advance(scenario)?;
    match &scenario.outcome {
        Outcome::Success(score) => {
            let score = score
                .parse::<f64>()
                .map_err(|_| "fake face score must be numeric".to_string())?;
            if !score.is_finite() || !(0.0..=1.0).contains(&score) {
                return Err("fake face score must be finite and bounded".to_string());
            }
            crate::derived_state::record_face_success(conn, hash, source_path, score)?;
            Ok(RunResult {
                progress,
                published: true,
            })
        }
        Outcome::Empty => {
            crate::derived_state::record_face_success(conn, hash, source_path, 0.0)?;
            Ok(RunResult {
                progress,
                published: true,
            })
        }
        Outcome::Failure(error) => {
            crate::derived_state::record_face_failure(conn, hash, source_path, error)?;
            Err(error.clone())
        }
    }
}
