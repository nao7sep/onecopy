use super::*;

#[test]
fn cancellation_is_visible_to_chunked_work_and_released_with_the_claim() {
    let id = "whisper-large-v3-turbo";
    let operation_id = "first-attempt";
    let started = begin_install(id, operation_id).unwrap();
    assert!(!cancel_entry(id, "older-attempt"));
    assert!(cancel_entry(id, operation_id));
    assert!(is_cancelled_error(
        &acquisition::check_cancelled(&started.0.cancelled).unwrap_err()
    ));
    drop(started);
    assert!(!cancel_entry(id, operation_id));
}

#[test]
fn installed_model_status_needs_no_persisted_facts() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-model-state-")
        .tempdir()
        .unwrap();
    let spec = spec_of("ultraface-rfb640").unwrap();
    let pinned = spec.pinned.as_ref().unwrap();
    let target = installed_path(dir.path(), spec);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::File::create(&target)
        .unwrap()
        .set_len(pinned.bytes)
        .unwrap();
    write_model_identity(dir.path(), spec, pinned).unwrap();
    let expected_version = pin_version(pinned);

    let model = state_of(dir.path(), spec);

    assert_eq!(model.status, BinaryStatus::UpToDate);
    assert_eq!(
        model.installed_version.as_deref(),
        Some(expected_version.as_str())
    );
    assert_eq!(model.facts.last_checked_at_utc, None);
    assert!(!dir.path().join(DEPENDENCIES_FILE_NAME).exists());
    assert!(model_identity_path(dir.path(), spec).is_file());
}

#[test]
fn a_same_size_older_model_remains_update_available() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-old-model-")
        .tempdir()
        .unwrap();
    let spec = spec_of("ultraface-rfb640").unwrap();
    let pinned = spec.pinned.as_ref().unwrap();
    let target = installed_path(dir.path(), spec);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::File::create(&target)
        .unwrap()
        .set_len(pinned.bytes)
        .unwrap();
    let older = ModelIdentity {
        sha256: "a".repeat(64),
        bytes: pinned.bytes,
    };
    std::fs::write(
        model_identity_path(dir.path(), spec),
        serde_json::to_vec(&older).unwrap(),
    )
    .unwrap();

    let model = state_of(dir.path(), spec);

    assert_eq!(model.status, BinaryStatus::UpdateAvailable);
    assert_eq!(model.installed_version.as_deref(), Some("aaaaaaaaaaaa"));
    assert_eq!(
        model.facts.latest_known_version.as_deref(),
        Some(pin_version(pinned).as_str())
    );
}

#[test]
fn facts_store_self_heals_on_missing_and_corrupt() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-")
        .tempdir()
        .unwrap();
    // Missing → defaults.
    assert_eq!(load_facts_for(dir.path(), "ffmpeg"), BinaryFacts::default());
    // Corrupt → defaults, no quarantine (re-derivable facts).
    std::fs::write(dir.path().join(DEPENDENCIES_FILE_NAME), b"{ not json").unwrap();
    assert_eq!(load_facts_for(dir.path(), "ffmpeg"), BinaryFacts::default());
}

#[test]
#[serial_test::serial(backup_store)]
fn facts_round_trip_through_the_store() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-rt-")
        .tempdir()
        .unwrap();
    let facts = BinaryFacts {
        latest_known_version: Some("9.1".into()),
        last_checked_at_utc: Some("2026-08-08T12:00:00.000Z".into()),
    };
    save_facts_for(dir.path(), "ffmpeg", &facts).unwrap();
    assert_eq!(load_facts_for(dir.path(), "ffmpeg"), facts);
}

#[test]
fn reset_temp_dir_wipes_and_recreates() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-temp-")
        .tempdir()
        .unwrap();
    let temp = dir.path().join(TEMP_DIR_NAME);
    std::fs::create_dir_all(&temp).unwrap();
    std::fs::write(temp.join("debris.partial"), b"x").unwrap();
    reset_temp_dir(dir.path());
    assert!(temp.is_dir());
    assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
}
