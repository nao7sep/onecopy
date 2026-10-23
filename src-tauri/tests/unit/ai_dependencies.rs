use super::*;

fn artifact(path: &Path, byte: u8, digest: &str) {
    std::fs::write(path, [byte; 16]).unwrap();
    let (mut file, _) = crate::file_identity::open_regular_nofollow(path).unwrap();
    let version = crate::file_identity::FileVersion::of(&mut file).unwrap();
    std::fs::write(path.with_extension("json"), serde_json::to_vec(&serde_json::json!({
        "formatVersion": crate::formats::MODEL_IDENTITY,
        "sha256": digest, "bytes": 16, "fileVersion": version,
    })).unwrap()).unwrap();
}

#[test]
fn captured_model_identity_survives_updates_but_a_replacement_during_load_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.bin");
    let old = "a".repeat(64);
    artifact(&path, 1, &old);
    let captured = ModelObservation::before(&path).after_load();
    assert_eq!(captured.as_deref(), Some(old.as_str()));
    let loading = ModelObservation::before(&path);
    std::fs::rename(&path, dir.path().join("old.bin")).unwrap();
    artifact(&path, 2, &"b".repeat(64));
    assert_eq!(loading.after_load(), None);
    assert_eq!(transcription_model_with_version(captured).version, old);
    assert_eq!(ModelObservation::before(&path).after_load(), Some("b".repeat(64)));
}

#[test]
fn absent_evidence_is_unknown_and_face_components_keep_independent_versions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.bin");
    std::fs::write(&path, b"unchecked").unwrap();
    assert_eq!(ModelObservation::before(&path).after_load(), None);
    assert_eq!(transcription_model().version, "unknown");
    assert_eq!(face_model_with_versions(Some("a".repeat(64)), None).version,
        format!("{}+unknown", "a".repeat(64)));
}

#[test]
fn runtime_model_presence_requires_exact_artifact_size_but_not_status_or_provenance() {
    let root = tempfile::tempdir().unwrap();
    let spec = binaries_manager::spec_of(ULTRAFACE).unwrap();
    let path = binaries_manager::installed_path(root.path(), spec);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let expected = spec.pinned.as_ref().unwrap().bytes;
    assert!(production_path(root.path(), ULTRAFACE).is_none());
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(expected - 1).unwrap();
    assert!(production_path(root.path(), ULTRAFACE).is_none());
    file.set_len(expected).unwrap();
    // A full installed model remains usable without check facts or a digest.
    assert_eq!(production_path(root.path(), ULTRAFACE), Some(path.clone()));
    assert_eq!(ModelObservation::before(&path).after_load(), None);
    assert_eq!(binaries_manager::state_of(root.path(), spec).status, crate::binaries::BinaryStatus::InstalledUnchecked);
    file.set_len(expected + 1).unwrap();
    assert!(production_path(root.path(), ULTRAFACE).is_none());
    drop(file);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(production_path(root.path(), ULTRAFACE).is_none());
}
