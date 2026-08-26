//! On-demand transcription (Design: Video handling) — whisper.cpp linked into
//! the app via `whisper-rs`, the large-v3-turbo model provisioned by the
//! managed-dependency registry. Work starts either from the shared media surface's
//! Transcribe control or from the derived-work coordinator.
//!
//! The transcript is DERIVED data keyed by content hash, cached in the
//! `transcripts/` subtree like every other derived entry — reconstructible,
//! swept, moved, and re-keyed on identity promotion. The engine loads per
//! job and is dropped with it, so memory (~2–2.5 GB for large-v3-turbo)
//! exists only while a transcription runs.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use crate::preview::CachePaths;

pub const TRANSCRIPTION_BUSY: &str = "a transcription is already running";

#[derive(Default)]
struct TranscriptionState {
    running: bool,
    cancelled: bool,
}

fn state() -> MutexGuard<'static, TranscriptionState> {
    static STATE: Mutex<TranscriptionState> = Mutex::new(TranscriptionState {
        running: false,
        cancelled: false,
    });
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Ownership of the one process-wide Whisper slot. Dropping the claim resets
/// cancellation for the next run; rejected contenders never touch either.
#[derive(Debug)]
pub struct TranscriptionClaim {
    _private: (),
}

impl Drop for TranscriptionClaim {
    fn drop(&mut self) {
        let mut state = state();
        state.running = false;
        state.cancelled = false;
    }
}

pub fn claim() -> Result<TranscriptionClaim, String> {
    let mut state = state();
    if state.running {
        return Err(TRANSCRIPTION_BUSY.to_string());
    }
    state.running = true;
    state.cancelled = false;
    Ok(TranscriptionClaim { _private: () })
}

pub fn request_cancel() -> bool {
    let mut state = state();
    if !state.running {
        return false;
    }
    state.cancelled = true;
    true
}

pub fn is_cancelled() -> bool {
    state().cancelled
}

/// 16 kHz mono f32 — the one input whisper accepts.
pub const SAMPLE_RATE: u32 = 16_000;

/// Extracts a video's audio track as 16 kHz mono f32 PCM through the managed
/// ffmpeg (bounded, cancellable — the same subprocess rules every ffmpeg call
/// obeys). ~3.8 MB per minute of audio in memory, transient. An empty result
/// is a successful no-audio finding, not a failure to retry forever.
pub fn extract_pcm(ffmpeg: &Path, video: &Path) -> Result<Vec<f32>, String> {
    let mut command = std::process::Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-i",
    ]);
    command.arg(video);
    command.args([
        "-map",
        "0:a:0",
        "-vn",
        "-ar",
        "16000",
        "-ac",
        "1",
        "-f",
        "f32le",
        "-",
    ]);
    let run = crate::subprocess::run_bounded(command, &is_cancelled)?;
    if !run.status_ok {
        if no_audio_output(&run.stderr) {
            return Ok(Vec::new());
        }
        return Err(format!("audio extraction failed: {}", run.stderr_tail()));
    }
    let mut pcm = Vec::with_capacity(run.stdout.len() / 4);
    for chunk in run.stdout.chunks_exact(4) {
        pcm.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(pcm)
}

/// The narrow ffmpeg result that means a valid video simply has no audio.
/// Other extraction failures remain failures; this never guesses from an
/// empty output alone after a non-zero exit.
pub fn no_audio_output(stderr: &str) -> bool {
    stderr.contains("matches no streams") || stderr.contains("does not contain any stream")
}

/// One transcribed segment, ready for display.
pub struct Segment {
    pub start_ms: i64,
    pub text: String,
}

/// Runs the whisper engine over PCM. Language auto-detected (the developer's
/// library mixes Japanese and English); progress is 0–100; the abort callback
/// polls the cancel flag so a quit or Cancel stops the engine mid-run.
pub fn run_whisper(
    model: &Path,
    pcm: &[f32],
    mut on_progress: impl FnMut(i32) + 'static,
) -> Result<Vec<Segment>, String> {
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    // whisper.cpp otherwise writes its internal decoder trace directly to
    // stderr. The app owns useful progress and errors; the repeated token
    // dumps are neither and made run-dev unreadable.
    whisper_rs::install_logging_hooks();
    let context = WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .map_err(|e| format!("model load failed: {e}"))?;
    let mut state = context
        .create_state()
        .map_err(|e| format!("whisper state failed: {e}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("auto"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_progress_callback_safe(move |progress: i32| on_progress(progress));
    params.set_abort_callback_safe(is_cancelled);

    state
        .full(params, pcm)
        .map_err(|e| format!("transcription failed: {e}"))?;

    let count = state.full_n_segments();
    let mut segments = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count {
        let Some(segment) = state.get_segment(i) else {
            continue;
        };
        let text = segment.to_str_lossy().map_err(|e| e.to_string())?.into_owned();
        // whisper timestamps are in centiseconds.
        segments.push(Segment {
            start_ms: segment.start_timestamp() * 10,
            text: text.trim().to_string(),
        });
    }
    Ok(segments)
}

/// Renders segments as the cached transcript: one `[m:ss] text` line each —
/// the timestamps are what let a reader jump the scene strip to the moment.
pub fn render(segments: &[Segment]) -> String {
    let mut out = String::new();
    for segment in segments {
        if segment.text.is_empty() {
            continue;
        }
        let total_seconds = segment.start_ms / 1000;
        out.push_str(&format!(
            "[{}:{:02}] {}\n",
            total_seconds / 60,
            total_seconds % 60,
            segment.text
        ));
    }
    out
}

/// The whole job: short-circuit on an existing transcript, honest errors for
/// the missing pieces, extract → transcribe → write ONCE atomically (a
/// cancelled or failed run leaves no partial cache entry by construction).
pub fn transcribe_to_cache(
    cache: &CachePaths,
    model: Option<&Path>,
    ffmpeg: Option<&Path>,
    video: &Path,
    hash: &str,
    on_progress: impl FnMut(i32) + 'static,
) -> Result<String, String> {
    let target = cache.transcript(hash);
    if let Ok(existing) = std::fs::read_to_string(&target) {
        return Ok(existing);
    }
    let claim = claim()?;
    transcribe_to_cache_claimed(&claim, cache, model, ffmpeg, video, hash, on_progress)
}

pub(crate) fn transcribe_to_cache_claimed(
    _claim: &TranscriptionClaim,
    cache: &CachePaths,
    model: Option<&Path>,
    ffmpeg: Option<&Path>,
    video: &Path,
    hash: &str,
    on_progress: impl FnMut(i32) + 'static,
) -> Result<String, String> {
    let target = cache.transcript(hash);
    if let Ok(existing) = std::fs::read_to_string(&target) {
        return Ok(existing);
    }
    let Some(model) = model else {
        return Err(
            "the transcription model is not installed — install it from Managed tools"
                .to_string(),
        );
    };
    let Some(ffmpeg) = ffmpeg else {
        return Err("ffmpeg is not installed — install it from Managed tools".to_string());
    };
    let pcm = extract_pcm(ffmpeg, video)?;
    let text = if pcm.is_empty() {
        String::new()
    } else {
        render(&run_whisper(model, &pcm, on_progress)?)
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // not recorded: a transcript is a re-derivable cache artifact colocated
    // with binary preview media.
    let tmp = target.with_extension(format!("{}.tmp", crate::nanoid::generate()));
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(text)
}
