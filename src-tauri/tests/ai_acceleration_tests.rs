// Tests exercising the crate's public AI acceleration contract from the
// dedicated Rust test tree.

use onecopy_lib::ai_acceleration::{
    available, default_for, selection_from_config, validate_patch, Mode, FACE_SCORING,
    TRANSCRIPTION,
};

#[test]
fn cpu_is_always_available_and_each_default_is_selectable() {
    for feature in [TRANSCRIPTION, FACE_SCORING] {
        let modes = available(feature).unwrap();
        assert!(modes.contains(&Mode::None), "{feature} offers CPU");
        assert!(modes.contains(&default_for(feature).unwrap()), "{feature}'s default is offered");
    }
}

#[test]
fn missing_values_use_the_accepted_platform_default() {
    let selected = selection_from_config(None).unwrap();
    assert_eq!(selected.transcription, default_for(TRANSCRIPTION).unwrap());
    assert_eq!(selected.face_scoring, default_for(FACE_SCORING).unwrap());
}

#[test]
fn unsupported_and_unknown_values_never_fall_back() {
    let unknown = serde_json::json!({ "aiAcceleration": { "transcription": "cuda" } });
    assert!(selection_from_config(Some(&unknown))
        .unwrap_err()
        .contains("unknown"));
    let metal = serde_json::json!({ "aiAcceleration": { "face-scoring": "metal" } });
    assert!(selection_from_config(Some(&metal))
        .unwrap_err()
        .contains("not available"));
    let unknown_feature = serde_json::json!({ "aiAcceleration": { "future": "none" } });
    assert!(validate_patch(&unknown_feature)
        .unwrap_err()
        .contains("unknown"));
}
