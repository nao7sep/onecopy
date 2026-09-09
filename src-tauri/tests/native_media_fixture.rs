//! Opt-in native acceptance setup. Media is supplied beside a fresh isolated
//! profile; the production settings serializer owns its complete config.
use onecopy_lib::storage::{self, DefaultConfig};

#[test]
#[ignore = "requires ONECOPY_HOME pointing to a fresh disposable profile and a sibling media directory"]
fn prepare_native_media_profile() {
    let profile = std::path::PathBuf::from(std::env::var_os("ONECOPY_HOME").expect("explicit isolated ONECOPY_HOME required"));
    assert!(profile.is_absolute());
    let source = profile.parent().unwrap().join("media");
    assert!(source.is_dir(), "supply disposable media first");
    assert!(!profile.exists(), "never overwrite a pre-existing profile");
    std::fs::create_dir(&profile).unwrap();
    let config = DefaultConfig {
        source_dirs: vec![source.to_string_lossy().into_owned()],
        video_transcription_enabled: false,
        audio_transcription_enabled: false,
        score_faces: false,
        similar_photo_analysis_enabled: false,
        video_snapshots_enabled: false,
        check_updates_at_launch: false,
        ..DefaultConfig::default()
    };
    storage::materialize_config_if_missing(&profile).unwrap();
    storage::patch_json_store(&profile.join(storage::CONFIG_FILE_NAME), &serde_json::to_value(config).unwrap()).unwrap();
    println!("Native media profile: {}", profile.display());
}
