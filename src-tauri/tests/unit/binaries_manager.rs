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
    let mut document = serde_json::to_value(&older).unwrap();
    document["formatVersion"] = serde_json::json!(1);
    std::fs::write(
        model_identity_path(dir.path(), spec),
        serde_json::to_vec(&document).unwrap(),
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

// R6-09: the checksum-mismatch, missing-sums-entry and extracted-entry-
// mismatch refusals are pure local-file verification (`verify_checksum`,
// `verify_pinned_artifact`), so they are exercised directly here against
// local fixtures — no network, no fake GitHub-shaped server.

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn verify_checksum_accepts_a_matching_digest_and_refuses_a_mismatch() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-verify-checksum-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("artifact.bin");
    let bytes = b"pinned artifact bytes";
    std::fs::write(&path, bytes).unwrap();
    let digest = sha256_hex(bytes);
    let cancelled = AtomicBool::new(false);
    let deadline = acquisition::OperationDeadline::for_check();

    assert!(verify_checksum(
        &path,
        &digest,
        &cancelled,
        &deadline,
        |_, _| {},
        |actual| format!("unexpected mismatch: {actual}"),
    )
    .is_ok());

    let wrong = "0".repeat(64);
    let error = verify_checksum(
        &path,
        &wrong,
        &cancelled,
        &deadline,
        |_, _| {},
        |actual| format!("checksum mismatch: expected {wrong}, got {actual}"),
    )
    .unwrap_err();
    assert!(error.contains("checksum mismatch"));
    assert!(error.contains(&digest));
}

fn pinned_artifact(sha256: &'static str, bytes: u64) -> PinnedArtifact {
    PinnedArtifact {
        url: "https://example.test/artifact",
        sha256,
        bytes,
        extracted: None,
        released: "2026-01-01",
    }
}

#[test]
fn verify_pinned_artifact_refuses_a_downloaded_file_that_does_not_match_its_pin() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-verify-pinned-flat-")
        .tempdir()
        .unwrap();
    let downloaded = dir.path().join("downloaded.bin");
    let staged = dir.path().join("staged.bin");
    std::fs::write(&downloaded, b"actual bytes on disk").unwrap();
    // Deliberately wrong digest: the pin expects bytes this file never had.
    let pinned = pinned_artifact(
        "0".repeat(64).leak(),
        21,
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = acquisition::OperationDeadline::for_check();

    let error = verify_pinned_artifact(
        "test-dependency",
        &downloaded,
        &staged,
        &pinned,
        &cancelled,
        &deadline,
        |_, _| {},
    )
    .unwrap_err();

    assert!(error.contains("checksum mismatch for test-dependency"));
    assert!(!staged.exists(), "a mismatch must publish nothing");
}

#[test]
fn verify_pinned_artifact_accepts_a_matching_flat_download() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-verify-pinned-ok-")
        .tempdir()
        .unwrap();
    let downloaded = dir.path().join("downloaded.bin");
    let staged = dir.path().join("staged.bin");
    let bytes = b"exactly the pinned bytes";
    std::fs::write(&downloaded, bytes).unwrap();
    let digest: &'static str = Box::leak(sha256_hex(bytes).into_boxed_str());
    let pinned = pinned_artifact(digest, bytes.len() as u64);
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = acquisition::OperationDeadline::for_check();

    let published_from = verify_pinned_artifact(
        "test-dependency",
        &downloaded,
        &staged,
        &pinned,
        &cancelled,
        &deadline,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(published_from, downloaded);
}

#[test]
fn verify_pinned_artifact_refuses_an_extracted_entry_that_does_not_match_its_pin() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-verify-pinned-zip-")
        .tempdir()
        .unwrap();
    let downloaded = dir.path().join("archive.zip");
    let staged = dir.path().join("staged.bin");
    let entry_bytes = b"the entry's real bytes";
    {
        let file = std::fs::File::create(&downloaded).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file::<_, ()>("payload.bin", Default::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, entry_bytes).unwrap();
        zip.finish().unwrap();
    }
    let archive_digest: &'static str =
        Box::leak(sha256_hex(&std::fs::read(&downloaded).unwrap()).into_boxed_str());
    let mut pinned = pinned_artifact(archive_digest, std::fs::metadata(&downloaded).unwrap().len());
    pinned.extracted = Some(ExtractedArtifact {
        archive_entry: "payload.bin",
        // Deliberately wrong digest: the entry's real bytes never match this.
        sha256: "1".repeat(64).leak(),
        bytes: entry_bytes.len() as u64,
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = acquisition::OperationDeadline::for_check();

    let error = verify_pinned_artifact(
        "test-dependency",
        &downloaded,
        &staged,
        &pinned,
        &cancelled,
        &deadline,
        |_, _| {},
    )
    .unwrap_err();

    assert!(error.contains("checksum mismatch for extracted test-dependency"));
    // The function returns no path to publish from on failure; the caller
    // (`install_entry_started`) never reaches `acquisition::publish_staged`
    // without one, so a mismatched entry can never be installed even though
    // the rejected bytes remain on disk in the caller's own temp staging file.
}

#[test]
fn verify_pinned_artifact_accepts_a_matching_extracted_entry() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-verify-pinned-zip-ok-")
        .tempdir()
        .unwrap();
    let downloaded = dir.path().join("archive.zip");
    let staged = dir.path().join("staged.bin");
    let entry_bytes = b"the entry's real bytes";
    {
        let file = std::fs::File::create(&downloaded).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file::<_, ()>("payload.bin", Default::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, entry_bytes).unwrap();
        zip.finish().unwrap();
    }
    let archive_digest: &'static str =
        Box::leak(sha256_hex(&std::fs::read(&downloaded).unwrap()).into_boxed_str());
    let entry_digest: &'static str = Box::leak(sha256_hex(entry_bytes).into_boxed_str());
    let mut pinned = pinned_artifact(archive_digest, std::fs::metadata(&downloaded).unwrap().len());
    pinned.extracted = Some(ExtractedArtifact {
        archive_entry: "payload.bin",
        sha256: entry_digest,
        bytes: entry_bytes.len() as u64,
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    let deadline = acquisition::OperationDeadline::for_check();

    let published_from = verify_pinned_artifact(
        "test-dependency",
        &downloaded,
        &staged,
        &pinned,
        &cancelled,
        &deadline,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(published_from, staged);
    assert_eq!(std::fs::read(&staged).unwrap(), entry_bytes);
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

#[test]
fn the_facts_store_and_sidecars_carry_their_format_versions() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-formats-")
        .tempdir()
        .unwrap();
    save_check_attempt(dir.path(), GITHUB_RELEASE_ATTEMPT_KEY, "2026-10-05T00:00:00.000Z").unwrap();
    std::fs::create_dir_all(dir.path().join(BIN_DIR_NAME)).unwrap();
    write_version_sidecar(dir.path(), "9.0").unwrap();
    let spec = spec_of("ultraface-rfb640").unwrap();
    std::fs::create_dir_all(installed_path(dir.path(), spec).parent().unwrap()).unwrap();
    write_model_identity(dir.path(), spec, spec.pinned.as_ref().unwrap()).unwrap();
    let marker = |path: PathBuf| -> serde_json::Value {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()["formatVersion"].clone()
    };
    assert_eq!(marker(dir.path().join(DEPENDENCIES_FILE_NAME)), formats::DEPENDENCIES);
    assert_eq!(marker(version_sidecar_path(dir.path())), formats::FFMPEG_VERSION_SIDECAR);
    assert_eq!(marker(model_identity_path(dir.path(), spec)), formats::MODEL_IDENTITY);
    assert_eq!(
        load_check_attempt(dir.path(), GITHUB_RELEASE_ATTEMPT_KEY).as_deref(),
        Some("2026-10-05T00:00:00.000Z")
    );
    assert_eq!(load_check_attempt(dir.path(), formats::JSON_KEY), None, "the marker is not a fact");
    assert_eq!(read_version_sidecar(dir.path()).as_deref(), Some("9.0"));
}

#[test]
fn files_a_newer_onecopy_wrote_read_as_absent_and_are_left_as_they_are() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-binmgr-newer-")
        .tempdir()
        .unwrap();
    let facts = dir.path().join(DEPENDENCIES_FILE_NAME);
    let facts_bytes = br#"{"formatVersion":2,"githubReleaseLastAttemptAtUtc":"2026-10-05T00:00:00.000Z"}"#;
    std::fs::write(&facts, facts_bytes).unwrap();
    assert_eq!(load_check_attempt(dir.path(), GITHUB_RELEASE_ATTEMPT_KEY), None);
    assert!(save_check_attempt(dir.path(), GITHUB_RELEASE_ATTEMPT_KEY, "later").is_err());
    assert_eq!(std::fs::read(&facts).unwrap(), facts_bytes);

    std::fs::create_dir_all(dir.path().join(BIN_DIR_NAME)).unwrap();
    let sidecar_bytes = br#"{"formatVersion":2,"version":"9.0"}"#;
    std::fs::write(version_sidecar_path(dir.path()), sidecar_bytes).unwrap();
    assert_eq!(read_version_sidecar(dir.path()), None);
    assert!(write_version_sidecar(dir.path(), "8.0").is_err());
    assert_eq!(std::fs::read(version_sidecar_path(dir.path())).unwrap(), sidecar_bytes);

    let spec = spec_of("ultraface-rfb640").unwrap();
    let identity = model_identity_path(dir.path(), spec);
    std::fs::create_dir_all(identity.parent().unwrap()).unwrap();
    let identity_bytes = br#"{"formatVersion":2,"sha256":"x","bytes":1}"#;
    std::fs::write(&identity, identity_bytes).unwrap();
    assert!(invalidate_model_identity(dir.path(), spec).is_err());
    assert_eq!(std::fs::read(&identity).unwrap(), identity_bytes);
}
