//! Real-artifact integration scenarios over OneCopy's production AI
//! operations. Scenario assertions live here so live correctness checks and
//! measured runs cannot drift.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use onecopy_lib::ai_acceleration::Mode;
use onecopy_lib::ai_dependencies::{self, Requirement};
use onecopy_lib::ai_measurement::Observer;
use onecopy_lib::derived_work::{
    complete_transcription_attempt, TranscriptionAttempt, TranscriptionAttemptOutcome,
};
use onecopy_lib::face::{complete_face_scoring_attempt, FaceScorer, FaceScoringAttemptOutcome};
use onecopy_lib::{derived_state, index_store, preview};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const SEMANTIC_TERMS: &[&str] = &[
    "upload",
    "photograph",
    "noon",
    "file",
    "location",
    "coordinates",
    "sharing",
];
const MINIMUM_TERM_MATCHES: usize = 4;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ScenarioId {
    Face,
    AudioTranscription,
    VideoTranscription,
}

impl ScenarioId {
    fn id(self) -> &'static str {
        match self {
            Self::Face => "face",
            Self::AudioTranscription => "audio-transcription",
            Self::VideoTranscription => "video-transcription",
        }
    }

    fn requirement(self) -> Requirement {
        match self {
            Self::Face => Requirement::FaceScoring,
            Self::AudioTranscription | Self::VideoTranscription => Requirement::Transcription,
        }
    }

    fn content_kind(self) -> &'static str {
        match self {
            Self::Face => "image",
            Self::AudioTranscription => "audio",
            Self::VideoTranscription => "video",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    path: PathBuf,
    basename: String,
    sha256: String,
    bytes: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    schema_version: u32,
    scenario_id: ScenarioId,
    managed_root: PathBuf,
    scratch_root: PathBuf,
    configured_acceleration: Mode,
    observe: bool,
    fixtures: Vec<Fixture>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhaseObservation {
    phase: &'static str,
    wall_ms: f64,
}

#[derive(Default)]
struct Collector {
    enabled: bool,
    phases: Mutex<Vec<PhaseObservation>>,
}

impl Collector {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phases: Mutex::new(Vec::new()),
        }
    }

    fn observations(&self) -> Option<Value> {
        self.enabled.then(|| {
            let phases = self
                .phases
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            json!({ "phases": phases })
        })
    }
}

impl Observer for Collector {
    fn enabled(&self) -> bool {
        self.enabled
    }

    fn phase(&self, name: &'static str, elapsed: Duration) {
        self.phases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(PhaseObservation {
                phase: name,
                wall_ms: elapsed.as_secs_f64() * 1_000.0,
            });
    }
}

fn insert_fixture(
    conn: &rusqlite::Connection,
    fixture: &Fixture,
    kind: &str,
) -> Result<(), String> {
    let bytes = i64::try_from(fixture.bytes)
        .map_err(|_| "scenario fixture byte size exceeds the database range")?;
    let path = fixture
        .path
        .to_str()
        .ok_or("scenario fixture path is not valid UTF-8")?;
    let directory = fixture
        .path
        .parent()
        .and_then(Path::to_str)
        .ok_or("scenario fixture directory is not valid UTF-8")?;
    conn.execute(
        "INSERT INTO contents (hash, byte_size, kind, derived_at_utc, derived_version)
         VALUES (?1, ?2, ?3, 'ready', 3)",
        params![fixture.sha256, bytes, kind],
    )
    .map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash, missing)
         VALUES (?1, ?2, ?3, ?4, ?5, 0)",
        params![path, directory, fixture.basename, kind, fixture.sha256],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn result(
    request: &Request,
    outcome: &str,
    correctness: Option<Value>,
    failure: Option<Value>,
    collector: &Collector,
) -> Value {
    json!({
        "schemaVersion": 1,
        "scenarioId": request.scenario_id.id(),
        "outcome": outcome,
        "configuredAcceleration": request.configured_acceleration,
        // Neither native engine currently provides an independent effective-
        // backend signal. Never turn the configured request into fake proof.
        "observedAcceleration": Value::Null,
        "correctness": correctness,
        "failure": failure,
        "observations": collector.observations(),
    })
}

fn failure(request: &Request, category: &str, message: &str, collector: &Collector) -> Value {
    result(
        request,
        "failed",
        None,
        Some(json!({ "category": category, "message": message })),
        collector,
    )
}

fn run_face(
    request: &Request,
    conn: &rusqlite::Connection,
    cache: &preview::CachePaths,
    collector: &Collector,
) -> Result<Value, String> {
    if request.fixtures.is_empty() {
        return Ok(failure(
            request,
            "scenario-input",
            "face scoring requires at least one fixture",
            collector,
        ));
    }
    onecopy_lib::ai_acceleration::require_supported(
        onecopy_lib::ai_acceleration::FACE_SCORING,
        request.configured_acceleration,
    )?;
    let dependencies = ai_dependencies::production_face_scoring(&request.managed_root)
        .ok_or("prepared face-scoring dependencies are unavailable")?;
    let mut scorer = FaceScorer::load(
        dependencies.runtime.as_deref(),
        &dependencies.detector,
        &dependencies.emotion,
        collector,
    )?;

    let mut scores = Vec::new();
    for fixture in &request.fixtures {
        insert_fixture(conn, fixture, request.scenario_id.content_kind())?;
        let preview = cache.preview(&fixture.sha256);
        std::fs::create_dir_all(
            preview
                .parent()
                .ok_or("scenario preview path has no parent")?,
        )
        .map_err(|error| error.to_string())?;
        std::fs::copy(&fixture.path, &preview).map_err(|error| error.to_string())?;
        let source_path = fixture
            .path
            .to_str()
            .ok_or("scenario fixture path is not valid UTF-8")?;
        let outcome = complete_face_scoring_attempt(
            conn,
            cache,
            &fixture.sha256,
            source_path,
            &|| false,
            collector,
            |_| {},
            |image| scorer.score(image),
        )?;
        let FaceScoringAttemptOutcome::Completed { score } = outcome else {
            return Ok(failure(
                request,
                "operation",
                "face-scoring operation did not complete",
                collector,
            ));
        };
        let persisted: f64 = {
            let _measurement =
                onecopy_lib::ai_measurement::Span::begin(collector, "result-readback");
            conn.query_row(
                "SELECT face_score FROM contents WHERE hash = ?1",
                [&fixture.sha256],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?
        };
        if !score.is_finite()
            || !(0.01..=1.0).contains(&score)
            || (persisted - f64::from(score)).abs() > f64::EPSILON
        {
            return Ok(failure(
                request,
                "correctness",
                "face score was absent, non-finite, out of bounds, or not durably published",
                collector,
            ));
        }
        scores.push(score);
    }
    Ok(result(
        request,
        "passed",
        Some(json!({ "ready": scores.len(), "total": request.fixtures.len() })),
        None,
        collector,
    ))
}

fn phrase_loop(text: &str) -> bool {
    let segments = text
        .lines()
        .map(|line| {
            let content = line
                .split_once(']')
                .map(|(_, remainder)| remainder)
                .unwrap_or(line);
            content
                .split_whitespace()
                .map(|token| token.to_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|tokens| !tokens.is_empty())
        .collect::<Vec<_>>();
    if segments.windows(2).any(|pair| pair.first() == pair.get(1)) {
        return true;
    }
    let tokens = segments.into_iter().flatten().collect::<Vec<_>>();
    for width in 3..=16.min(tokens.len() / 3) {
        for start in 0..=tokens.len() - width * 3 {
            if tokens[start..start + width] == tokens[start + width..start + width * 2]
                && tokens[start..start + width] == tokens[start + width * 2..start + width * 3]
            {
                return true;
            }
        }
    }
    false
}

fn run_transcription(
    request: &Request,
    conn: &rusqlite::Connection,
    cache: &preview::CachePaths,
    collector: &Collector,
) -> Result<Value, String> {
    if request.fixtures.len() != 1 {
        return Ok(failure(
            request,
            "scenario-input",
            "transcription requires exactly one fixture",
            collector,
        ));
    }
    onecopy_lib::ai_acceleration::require_supported(
        onecopy_lib::ai_acceleration::TRANSCRIPTION,
        request.configured_acceleration,
    )?;
    let fixture = &request.fixtures[0];
    insert_fixture(conn, fixture, request.scenario_id.content_kind())?;
    let source_path = fixture
        .path
        .to_str()
        .ok_or("scenario fixture path is not valid UTF-8")?;
    let outcome = complete_transcription_attempt(
        TranscriptionAttempt {
            conn,
            cache,
            data_root: &request.managed_root,
            temp_dir: request.scratch_root.join("transcription-temp"),
            source_hash: &fixture.sha256,
            source_path,
            replace_existing: false,
            acceleration: request.configured_acceleration,
            observer: collector,
            cancel_when: None,
        },
        |_| {},
        |_| {},
        |_, _| {},
    )?;
    let TranscriptionAttemptOutcome::Completed { hash, text } = outcome else {
        return Ok(failure(
            request,
            "operation",
            "transcription operation did not complete",
            collector,
        ));
    };
    let persisted = {
        let _measurement = onecopy_lib::ai_measurement::Span::begin(collector, "result-readback");
        derived_state::transcript_result(conn, cache, &hash)?
    };
    if persisted.status != derived_state::READY || persisted.text.as_deref() != Some(text.as_str())
    {
        return Ok(failure(
            request,
            "correctness",
            "transcription result was not durably published",
            collector,
        ));
    }
    let normalized = text.to_lowercase();
    let matched_terms = SEMANTIC_TERMS
        .iter()
        .filter(|term| normalized.contains(**term))
        .count();
    let has_phrase_loop = phrase_loop(&text);
    if matched_terms < MINIMUM_TERM_MATCHES || has_phrase_loop {
        return Ok(failure(
            request,
            "correctness",
            "transcript missed required semantic coverage or contained a phrase loop",
            collector,
        ));
    }
    Ok(result(
        request,
        "passed",
        Some(json!({
            "matchedTerms": matched_terms,
            "segmentCount": text.lines().filter(|line| !line.trim().is_empty()).count(),
            "phraseLoop": false,
            "normalizedOutputSha256": hex::encode(Sha256::digest(normalized.as_bytes())),
        })),
        None,
        collector,
    ))
}

fn run(request: &Request) -> Result<Value, String> {
    if request.schema_version != 1 {
        return Err("unsupported scenario request schema".to_string());
    }
    ai_dependencies::require_prepared(&request.managed_root, &[request.scenario_id.requirement()])?;
    std::fs::create_dir_all(&request.scratch_root).map_err(|error| error.to_string())?;
    let conn = index_store::open(&request.scratch_root.join("index.sqlite3"))?;
    let cache = preview::CachePaths::new(request.scratch_root.join("cache"));
    let collector = Collector::new(request.observe);
    match request.scenario_id {
        ScenarioId::Face => run_face(request, &conn, &cache, &collector),
        ScenarioId::AudioTranscription | ScenarioId::VideoTranscription => {
            run_transcription(request, &conn, &cache, &collector)
        }
    }
}

fn main() {
    let Some(request_path) = std::env::args_os().nth(1) else {
        eprintln!("usage: onecopy-ai-scenario REQUEST.json");
        std::process::exit(2);
    };
    let outcome = std::fs::read(request_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<Request>(&bytes).map_err(|error| error.to_string())
        })
        .and_then(|request| run(&request));
    match outcome {
        Ok(result) => println!(
            "{}",
            json!({ "event": "scenario-result", "result": result })
        ),
        Err(error) => {
            eprintln!("scenario infrastructure failed: {error}");
            std::process::exit(1);
        }
    }
}
