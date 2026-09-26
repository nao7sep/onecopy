use super::*;

fn capabilities() -> WorkCapabilities {
    WorkCapabilities {
        ffmpeg: true,
        video_snapshots_enabled: true,
        similarity_enabled: true,
        face_enabled: true,
        face_models: true,
        transcription_model: true,
        transcription_acceleration: true,
        face_acceleration: true,
        video_transcription_enabled: true,
        audio_transcription_enabled: true,
    }
}

fn facts<'a>(kind: &'a str, derived_at: Option<&'a str>, duration_ms: Option<i64>) -> ItemWorkFacts<'a> {
    ItemWorkFacts {
        kind,
        derived_at,
        derived_version: DERIVE_VERSION,
        strip_frames: None,
        duration_ms,
        similar_group_id: None,
        face_state: None,
        face_score: None,
        transcript_state: None,
    }
}

#[test]
fn each_prerequisite_selects_in_sql_exactly_the_items_its_fact_form_admits() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    for prerequisite in [
        Prerequisite::None,
        Prerequisite::Preview,
        Prerequisite::PosterAndDuration,
        Prerequisite::DurationOrPoster,
    ] {
        for (derived_at, version, ready) in [
            (None, DERIVE_VERSION, false),
            (Some(FAILED), DERIVE_VERSION, false),
            (Some(NEEDS_FFMPEG), DERIVE_VERSION, false),
            (Some("2026-01-01T00:00:00Z"), DERIVE_VERSION - 1, false),
            (Some("2026-01-01T00:00:00Z"), DERIVE_VERSION, true),
        ] {
            for duration in [None, Some(1_000_i64)] {
                let selected: bool = conn
                    .query_row(
                        &format!(
                            "SELECT {} FROM (SELECT ?1 AS derived_at_utc, ?2 AS derived_version, ?3 AS duration_ms) c",
                            prerequisite.sql()
                        ),
                        rusqlite::params![derived_at, version, duration],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    selected,
                    prerequisite.holds(ready, duration.is_some()),
                    "{prerequisite:?} derived_at={derived_at:?} version={version} duration={duration:?}"
                );
            }
        }
    }
}

#[test]
fn a_video_whose_duration_is_known_is_owed_a_transcript_even_after_its_poster_failed() {
    // The candidate SQL admits it (duration known); the item state must say
    // pending too, not "blocked" as it once did.
    let states = item_work_states(facts("video", Some(FAILED), Some(5_000)), capabilities(), false);
    assert_eq!(states.transcripts.unwrap().state, "pending");

    let states = item_work_states(facts("video", Some(FAILED), None), capabilities(), false);
    let transcript = states.transcripts.unwrap();
    assert_eq!(transcript.state, "blocked");
    assert_eq!(transcript.reason, Some("Video poster generation failed"));
    assert_eq!(states.snapshots.unwrap().state, "blocked");
}

#[test]
fn the_item_state_and_the_class_debt_name_the_same_gate_reason() {
    let mut off = capabilities();
    off.video_snapshots_enabled = false;
    off.face_models = false;
    off.transcription_model = false;
    let video = item_work_states(facts("video", None, None), off, false);
    assert_eq!(
        video.snapshots.unwrap().reason,
        match WorkClass::Snapshots.gate(off) {
            ClassGate::Disabled(reason) => Some(reason),
            gate => panic!("{gate:?}"),
        }
    );
    let transcript = video.transcripts.unwrap();
    assert_eq!(transcript.state, "unavailable");
    assert_eq!(transcript.reason, Some("waiting-for-transcription-model"));
    let image = item_work_states(facts("image", None, None), off, false);
    let faces = image.faces.unwrap();
    assert_eq!(faces.state, "unavailable");
    assert_eq!(
        WorkClass::Faces.gate(off),
        ClassGate::Unavailable(faces.reason.unwrap())
    );
}

#[test]
fn a_gated_class_is_never_a_priority_candidate() {
    let mut off = capabilities();
    off.face_enabled = false;
    assert_eq!(priority_predicate(WorkClass::Faces, off), "0");
    assert_eq!(
        priority_predicate(WorkClass::Faces, capabilities()),
        WorkClass::Faces.owed_sql().unwrap()
    );
}
