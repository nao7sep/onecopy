use onecopy_lib::ai_dependencies::{production_face_scoring, production_transcription};
use onecopy_lib::binaries::BinaryStatus;
use onecopy_lib::binaries_manager::{self, DependencyKind};

fn materialize_present_artifact(root: &std::path::Path, id: &str) {
    let spec = binaries_manager::spec_of(id).unwrap();
    let target = binaries_manager::installed_path(root, spec);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let bytes = match spec.kind {
        DependencyKind::Binary => 1,
        DependencyKind::Runtime | DependencyKind::Model => spec
            .pinned
            .as_ref()
            .and_then(|pin| pin.extracted.as_ref().map(|artifact| artifact.bytes))
            .or_else(|| spec.pinned.as_ref().map(|pin| pin.bytes))
            .unwrap(),
    };
    std::fs::File::create(&target)
        .unwrap()
        .set_len(bytes)
        .unwrap();
}

#[test]
fn production_resolution_preserves_installed_without_requiring_current_identity() {
    let root = tempfile::tempdir().unwrap();
    for id in [
        "ultraface-rfb640",
        "hsemotion-enet-b2",
        "whisper-large-v3-turbo",
        "ffmpeg",
    ] {
        materialize_present_artifact(root.path(), id);
    }
    #[cfg(windows)]
    materialize_present_artifact(root.path(), "onnxruntime-win-x64");

    assert!(production_face_scoring(root.path()).is_some());
    let transcription = production_transcription(root.path());
    assert!(transcription.ffmpeg.is_some());
    assert!(transcription.model.is_some());
    assert_eq!(
        binaries_manager::state_of(
            root.path(),
            binaries_manager::spec_of("whisper-large-v3-turbo").unwrap(),
        )
        .status,
        BinaryStatus::InstalledUnchecked,
        "production may run an installed model without claiming exact-current preparation",
    );
}
