//! Shared video/audio transcription — whisper.cpp linked into the app via
//! `whisper-rs`, with the large-v3-turbo model provisioned by the managed
//! dependency registry. The two media kinds keep separate product policy and
//! queues while this module owns their common extraction and inference engine.
//!
//! The transcript is DERIVED data keyed by content hash, cached in the
//! `transcripts/` subtree like every other derived entry — reconstructible,
//! swept, moved, and re-keyed on identity promotion. The engine loads per
//! job and is dropped with it, so memory (~2–2.5 GB for large-v3-turbo)
//! exists only while a transcription runs.

use std::io::Read;
use std::path::{Path, PathBuf};
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
const PCM_SILENCE_PEAK: f32 = 1.0 / i16::MAX as f32;

struct RemoveFile(PathBuf);

impl Drop for RemoveFile {
    fn drop(&mut self) {
        crate::fs_recovery::remove_file(&self.0, "transcription audio staging cleanup");
    }
}

/// Extracts a media file's audio track as 16 kHz mono f32 PCM through the managed
/// ffmpeg (bounded, cancellable — the same subprocess rules every ffmpeg call
/// obeys). ~3.8 MB per minute of audio in memory, transient. An empty result
/// is a successful no-audio finding, not a failure to retry forever.
pub fn extract_pcm(ffmpeg: &Path, media: &Path, temp_dir: &Path) -> Result<Vec<f32>, String> {
    crate::resource_limits::require_available(
        crate::resource_limits::PCM_REQUIRED_AVAILABLE,
        "Audio extraction",
    )?;
    std::fs::create_dir_all(temp_dir).map_err(|error| error.to_string())?;
    let staged = temp_dir.join(format!("pcm-{}.f32le", crate::nanoid::generate()?));
    let _cleanup = RemoveFile(staged.clone());
    let mut command = std::process::Command::new(ffmpeg);
    command.args([
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-nostats",
        "-stats_period",
        "30",
        "-progress",
        "pipe:1",
        "-i",
    ]);
    command.arg(media);
    command.args([
        "-map",
        "0:a:0",
        "-vn",
        "-ar",
        "16000",
        "-ac",
        "1",
        "-fs",
        &crate::resource_limits::MAX_PCM_OUTPUT.to_string(),
        "-f",
        "f32le",
        "-y",
    ]);
    command.arg(&staged);
    let run = crate::subprocess::run_bounded(command, &is_cancelled)?;
    if !run.status_ok {
        if no_audio_output(&run.stderr) {
            return Ok(Vec::new());
        }
        return Err(format!("audio extraction failed: {}", run.stderr_tail()));
    }
    let bytes = std::fs::metadata(&staged)
        .map_err(|error| format!("audio extraction produced no PCM: {error}"))?
        .len();
    if bytes >= crate::resource_limits::MAX_PCM_OUTPUT as u64 {
        return Err(format!(
            "audio extraction exceeded the {} MiB safety limit",
            crate::resource_limits::MAX_PCM_OUTPUT / 1024 / 1024
        ));
    }
    if bytes % 4 != 0 {
        return Err("audio extraction produced a partial PCM sample".to_string());
    }
    let mut reader = std::io::BufReader::new(
        std::fs::File::open(&staged).map_err(|error| error.to_string())?,
    );
    let mut pcm = Vec::with_capacity(bytes as usize / 4);
    let mut sample = [0u8; 4];
    while pcm.len() < bytes as usize / 4 {
        reader
            .read_exact(&mut sample)
            .map_err(|error| error.to_string())?;
        pcm.push(f32::from_le_bytes(sample));
    }
    Ok(pcm)
}

/// The narrow ffmpeg result that means a valid video simply has no audio.
/// Other extraction failures remain failures; this never guesses from an
/// empty output alone after a non-zero exit.
pub fn no_audio_output(stderr: &str) -> bool {
    stderr.contains("matches no streams") || stderr.contains("does not contain any stream")
}

/// Digital silence and sub-quantization noise contain no useful speech and
/// make Whisper prone to inventing a short closing phrase. Rejecting them at
/// the shared PCM boundary also avoids loading the multi-gigabyte model.
pub fn has_audible_signal(pcm: &[f32]) -> bool {
    pcm.iter()
        .any(|sample| sample.is_finite() && sample.abs() > PCM_SILENCE_PEAK)
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

    crate::resource_limits::require_available(
        crate::resource_limits::WHISPER_REQUIRED_AVAILABLE,
        "Transcription",
    )?;

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

/// The ordinary job short-circuits on an existing transcript. Generation has
/// honest errors for missing pieces and publishes one complete staged result;
/// cancellation or failure leaves no partial cache entry.
pub fn transcribe_to_cache(
    cache: &CachePaths,
    temp_dir: &Path,
    model: Option<&Path>,
    ffmpeg: Option<&Path>,
    media: &Path,
    hash: &str,
    on_progress: impl FnMut(i32) + 'static,
) -> Result<String, String> {
    let target = cache.transcript(hash);
    if let Ok(existing) = std::fs::read_to_string(&target) {
        return Ok(existing);
    }
    let claim = claim()?;
    transcribe_to_cache_claimed(
        &claim,
        cache,
        temp_dir,
        model,
        ffmpeg,
        media,
        hash,
        false,
        on_progress,
    )
}

pub(crate) fn transcribe_to_cache_claimed(
    _claim: &TranscriptionClaim,
    cache: &CachePaths,
    temp_dir: &Path,
    model: Option<&Path>,
    ffmpeg: Option<&Path>,
    media: &Path,
    hash: &str,
    replace_existing: bool,
    on_progress: impl FnMut(i32) + 'static,
) -> Result<String, String> {
    let target = cache.transcript(hash);
    if !replace_existing {
        if let Ok(existing) = std::fs::read_to_string(&target) {
            return Ok(existing);
        }
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
    let pcm = extract_pcm(ffmpeg, media, temp_dir)?;
    let text = if !has_audible_signal(&pcm) {
        String::new()
    } else {
        render(&run_whisper(model, &pcm, on_progress)?)
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    // not recorded: a transcript is a re-derivable cache artifact colocated
    // with binary preview media.
    let tmp = target.with_extension(format!("{}.tmp", crate::nanoid::generate()?));
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| {
        crate::fs_recovery::remove_file(&tmp, "transcript staging write cleanup");
        e.to_string()
    })?;
    crate::fs_publish::replace_existing(&tmp, &target).map_err(|e| {
        crate::fs_recovery::remove_file(&tmp, "transcript publication cleanup");
        e.to_string()
    })?;
    Ok(text)
}
