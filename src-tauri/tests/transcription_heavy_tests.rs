// Transcription through the managed ffmpeg and the production Whisper model,
// judged against the spoken-source transcripts in the shared corpus.

use std::collections::HashMap;

use onecopy_lib::ai_acceleration::{self, Mode, TRANSCRIPTION};
use onecopy_lib::derived_state;
use onecopy_lib::derived_work::{
    complete_transcription_attempt, TranscriptionAttempt, TranscriptionAttemptOutcome,
};

use super::heavy_support::{corpus_file, corpus_json, library, Library};

/// Share of the spoken words a transcript must recover. Synthetic speech under
/// light noise leaves Whisper large-v3-turbo little room for honest misses.
const MINIMUM_RECALL: f64 = 0.9;

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .replace('\u{2019}', "'")
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The words of a transcript whose lines carry `[m:ss]` timestamps.
fn spoken_words(transcript: &str) -> Vec<String> {
    transcript
        .lines()
        .flat_map(|line| {
            let content = match line.strip_prefix('[') {
                Some(rest) => rest.split_once(']').map_or("", |(_, text)| text),
                None => line,
            };
            words(content)
        })
        .collect()
}

fn source_words(transcript_json: &str) -> Vec<String> {
    corpus_json(transcript_json)["segments"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|segment| words(segment["text"].as_str().unwrap()))
        .collect()
}

/// The share of `expected` words the transcript contains, each counted once.
fn recall(expected: &[String], transcript: &[String]) -> f64 {
    let mut available: HashMap<&str, usize> = HashMap::new();
    for word in transcript {
        *available.entry(word).or_default() += 1;
    }
    let found = expected
        .iter()
        .filter(|word| match available.get_mut(word.as_str()) {
            Some(count) if *count > 0 => {
                *count -= 1;
                true
            }
            _ => false,
        })
        .count();
    found as f64 / expected.len() as f64
}

/// A decoder loop: one segment repeated, or a phrase of three to sixteen words
/// said three times in a row.
fn phrase_loop(transcript: &str) -> bool {
    let segments: Vec<Vec<String>> = transcript
        .lines()
        .map(spoken_words)
        .filter(|tokens| !tokens.is_empty())
        .collect();
    if segments.windows(2).any(|pair| pair[0] == pair[1]) {
        return true;
    }
    let tokens: Vec<String> = segments.into_iter().flatten().collect();
    (3..=16.min(tokens.len() / 3)).any(|width| {
        (0..=tokens.len() - width * 3).any(|start| {
            tokens[start..start + width] == tokens[start + width..start + width * 2]
                && tokens[start..start + width] == tokens[start + width * 2..start + width * 3]
        })
    })
}

fn transcribe(library: &Library, file_name: &str, mode: Mode, replace_existing: bool) -> String {
    let hash = library.hash_of(file_name);
    let source = library.path_of(file_name);
    let outcome = complete_transcription_attempt(
        TranscriptionAttempt {
            conn: &library.conn,
            cache: &library.cache,
            data_root: library.home.path(),
            temp_dir: library.settings.temp_dir.clone(),
            source_hash: &hash,
            source_path: source.to_str().unwrap(),
            replace_existing,
            acceleration: mode,
            cancel_when: None,
        },
        |_| {},
        |_| {},
        |_, _| {},
    )
    .unwrap();
    let (hash, text) = match outcome {
        TranscriptionAttemptOutcome::Completed { hash, text } => (hash, text),
        other => panic!("{file_name} with {mode} acceleration did not complete: {other:?}"),
    };
    let published = derived_state::transcript_result(&library.conn, &library.cache, &hash).unwrap();
    assert_eq!(
        published.text.as_deref(),
        Some(text.as_str()),
        "{file_name}: the transcript is durably published"
    );
    text
}

fn judge(file_name: &str, mode: Mode, transcript: &str, expected: &[String]) {
    let recalled = recall(expected, &spoken_words(transcript));
    assert!(
        recalled >= MINIMUM_RECALL,
        "{file_name} with {mode} acceleration recovered {recalled:.2} of the spoken words:\n{transcript}"
    );
    assert!(
        !phrase_loop(transcript),
        "{file_name} with {mode} acceleration looped:\n{transcript}"
    );
}

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn every_selectable_acceleration_transcribes_english_dialogue() {
    let name = "dialogue-english-with-noise.flac";
    let library = library(
        "transcribe-audio",
        &[corpus_file(&format!("audio/dialogue/{name}"))],
        serde_json::json!({ "audioTranscriptionEnabled": true }),
    );
    let expected = source_words("audio/dialogue/dialogue-english-with-noise.transcript.json");
    for (index, mode) in ai_acceleration::available(TRANSCRIPTION).unwrap().into_iter().enumerate() {
        let transcript = transcribe(&library, name, mode, index > 0);
        judge(name, mode, &transcript, &expected);
    }
}

#[test]
#[ignore = "heavy: real managed tools, models and the shared corpus; run by npm run test:full"]
#[serial_test::serial(heavy)]
fn video_dialogue_transcribes_through_its_audio_track() {
    let name = "dialogue-english-with-noise.mp4";
    let library = library(
        "transcribe-video",
        &[corpus_file(&format!("video/dialogue/{name}"))],
        serde_json::json!({ "videoTranscriptionEnabled": true }),
    );
    let expected = source_words("video/dialogue/dialogue-english-with-noise.transcript.json");
    let mode = ai_acceleration::default_for(TRANSCRIPTION).unwrap();
    let transcript = transcribe(&library, name, mode, false);
    judge(name, mode, &transcript, &expected);
}

#[test]
fn the_loop_probe_tells_decoder_loops_from_ordinary_repetition() {
    assert!(phrase_loop(
        "[0:00] please remove the coordinates\n[0:02] please remove the coordinates\n"
    ));
    assert!(phrase_loop(
        "[0:00] please remove the coordinates please remove the coordinates please remove the coordinates\n"
    ));
    assert!(!phrase_loop(
        "[0:00] thank you, thank you for removing the location before sharing\n"
    ));
    assert!(!phrase_loop(
        "[0:00] please remove the coordinates\n[0:02] then share the photograph\n"
    ));
}

#[test]
fn recall_counts_each_spoken_word_once() {
    let expected = words("Thank you. Thank you, please.");
    assert_eq!(recall(&expected, &spoken_words("[0:00] thank you please")), 0.6);
    assert_eq!(recall(&expected, &spoken_words("[0:00] Thank you. Thank you, please!")), 1.0);
}
