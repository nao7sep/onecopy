use onecopy_lib::ai_dependencies::{
    dependency_ids, inspect_prepared, ArtifactReadiness, Requirement,
};

#[test]
fn prepared_context_is_typed_deduplicated_and_read_only_when_artifacts_are_absent() {
    let parent = tempfile::tempdir().unwrap();
    let absent = parent.path().join("absent-managed-root");
    let requirements = [
        Requirement::FaceScoring,
        Requirement::Transcription,
        Requirement::Transcription,
    ];
    let context = inspect_prepared(&absent, &requirements).unwrap();

    assert!(
        !absent.exists(),
        "verification must not create its managed root"
    );
    assert_eq!(
        context.requirements,
        [Requirement::FaceScoring, Requirement::Transcription]
    );
    assert_eq!(
        dependency_ids(&context.requirements),
        [
            #[cfg(windows)]
            "onnxruntime-win-x64",
            "ultraface-rfb640",
            "hsemotion-enet-b2",
            "ffmpeg",
            "whisper-large-v3-turbo",
        ]
    );
    assert!(context.artifacts.iter().all(|artifact| {
        artifact.readiness == ArtifactReadiness::NotInstalled && artifact.identity.is_none()
    }));
    let error = context.require_current().unwrap_err();
    assert!(error.starts_with("preparation required:"), "{error}");
    assert!(error.contains("whisper-large-v3-turbo"), "{error}");
}
