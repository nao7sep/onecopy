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
        file_version: Some(crate::file_identity::FileVersion::of(
            &mut crate::file_identity::open_regular_nofollow(&target).unwrap().0,
        ).unwrap()),
    };
    let mut document = serde_json::to_value(&older).unwrap();
    document[formats::JSON_KEY] = serde_json::json!(formats::MODEL_IDENTITY);
    std::fs::write(
        model_identity_path(dir.path(), spec),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();

    let model = state_of(dir.path(), spec);

    assert_eq!(model.status, BinaryStatus::UpdateAvailable);
    let installed = model.installed_version.unwrap();
    assert!(older.sha256.starts_with(&installed), "{installed} names the installed file");
    assert_ne!(installed, pin_version(pinned));
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

// The checksum-mismatch, missing-sums-entry and extracted-entry-
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
        upstream_version: None,
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

    assert!(error.contains("test-dependency"), "{error}");
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

    assert!(error.contains("test-dependency"), "{error}");
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
    save_check_attempt(dir.path(), MANAGED_TOOL_UPDATE_ATTEMPT_KEY, "2026-10-05T00:00:00.000Z").unwrap();
    std::fs::create_dir_all(dir.path().join(BIN_DIR_NAME)).unwrap();
    std::fs::write(ffmpeg_path(dir.path()), b"verified executable fixture").unwrap();
    write_version_sidecar(dir.path(), "9.0", binary_file_version(&ffmpeg_path(dir.path())).unwrap()).unwrap();
    let spec = spec_of("ultraface-rfb640").unwrap();
    std::fs::create_dir_all(installed_path(dir.path(), spec).parent().unwrap()).unwrap();
    std::fs::File::create(installed_path(dir.path(), spec)).unwrap()
        .set_len(installed_bytes(spec.pinned.as_ref().unwrap())).unwrap();
    write_model_identity(dir.path(), spec, spec.pinned.as_ref().unwrap()).unwrap();
    let marker = |path: PathBuf| -> serde_json::Value {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()["formatVersion"].clone()
    };
    assert_eq!(marker(dir.path().join(DEPENDENCIES_FILE_NAME)), formats::DEPENDENCIES);
    assert_eq!(marker(version_sidecar_path(dir.path())), formats::FFMPEG_VERSION_SIDECAR);
    assert_eq!(marker(model_identity_path(dir.path(), spec)), formats::MODEL_IDENTITY);
    assert_eq!(
        load_check_attempt(dir.path(), MANAGED_TOOL_UPDATE_ATTEMPT_KEY).as_deref(),
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
    let newer = |format: i64, mut document: serde_json::Value| {
        document[formats::JSON_KEY] = serde_json::json!(format + 1);
        serde_json::to_vec(&document).unwrap()
    };
    let facts_bytes = newer(
        formats::DEPENDENCIES,
        serde_json::json!({ "managedToolUpdateLastAttemptAtUtc": "2026-10-05T00:00:00.000Z" }),
    );
    std::fs::write(&facts, &facts_bytes).unwrap();
    assert_eq!(load_check_attempt(dir.path(), MANAGED_TOOL_UPDATE_ATTEMPT_KEY), None);
    assert!(save_check_attempt(dir.path(), MANAGED_TOOL_UPDATE_ATTEMPT_KEY, "later").is_err());
    assert_eq!(std::fs::read(&facts).unwrap(), facts_bytes);

    std::fs::create_dir_all(dir.path().join(BIN_DIR_NAME)).unwrap();
    let sidecar_bytes = newer(formats::FFMPEG_VERSION_SIDECAR, serde_json::json!({ "version": "9.0" }));
    std::fs::write(version_sidecar_path(dir.path()), &sidecar_bytes).unwrap();
    assert_eq!(read_version_sidecar(dir.path()), None);
    std::fs::write(ffmpeg_path(dir.path()), b"verified executable fixture").unwrap();
    assert!(write_version_sidecar(dir.path(), "8.0", binary_file_version(&ffmpeg_path(dir.path())).unwrap()).is_err());
    assert_eq!(std::fs::read(version_sidecar_path(dir.path())).unwrap(), sidecar_bytes);

    let spec = spec_of("ultraface-rfb640").unwrap();
    let identity = model_identity_path(dir.path(), spec);
    std::fs::create_dir_all(identity.parent().unwrap()).unwrap();
    let identity_bytes = newer(formats::MODEL_IDENTITY, serde_json::json!({ "sha256": "x", "bytes": 1 }));
    std::fs::write(&identity, &identity_bytes).unwrap();
    assert!(invalidate_model_identity(dir.path(), spec).is_err());
    assert_eq!(std::fs::read(&identity).unwrap(), identity_bytes);
}


fn model_identity_fixture() -> (tempfile::TempDir, &'static DependencySpec, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let spec = spec_of("ultraface-rfb640").unwrap();
    let target = installed_path(dir.path(), spec);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::File::create(&target).unwrap()
        .set_len(installed_bytes(spec.pinned.as_ref().unwrap())).unwrap();
    (dir, spec, target)
}

#[test]
fn verified_model_digest_uses_published_file_evidence_and_returns_the_full_digest() {
    let (dir, spec, target) = model_identity_fixture();
    let pinned = spec.pinned.as_ref().unwrap();
    write_model_identity(dir.path(), spec, pinned).unwrap();
    let sidecar = model_identity_path(dir.path(), spec);
    let mut document: serde_json::Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    assert!(document.get("fileVersion").is_some_and(|value| !value.is_null()));
    document["sha256"] = serde_json::json!(pinned.sha256.to_ascii_uppercase());
    std::fs::write(sidecar, serde_json::to_vec(&document).unwrap()).unwrap();
    assert_eq!(verified_model_digest(&target).as_deref(), Some(pinned.sha256.to_ascii_lowercase().as_str()));
    assert_eq!(read_model_identity(dir.path(), spec).as_deref(), Some(pin_version(pinned).as_str()));
}

#[test]
fn missing_and_legacy_model_evidence_keep_installed_presence_without_a_version() {
    let (dir, spec, target) = model_identity_fixture();
    assert_eq!(verified_model_digest(&target), None);
    let pinned = spec.pinned.as_ref().unwrap();
    let document = serde_json::json!({
        "formatVersion": formats::MODEL_IDENTITY,
        "sha256": pinned.sha256,
        "bytes": installed_bytes(pinned),
    });
    let sidecar = model_identity_path(dir.path(), spec);
    let before = serde_json::to_vec(&document).unwrap();
    std::fs::write(&sidecar, &before).unwrap();
    let state = state_of(dir.path(), spec);
    assert_eq!(state.status, BinaryStatus::InstalledUnchecked);
    assert_eq!(state.installed_version, None);
    assert_eq!(verified_model_digest(&target), None);
    assert_eq!(std::fs::read(sidecar).unwrap(), before);
}

#[test]
fn replacing_a_model_with_same_size_bytes_invalidates_its_digest_evidence() {
    let (dir, spec, target) = model_identity_fixture();
    write_model_identity(dir.path(), spec, spec.pinned.as_ref().unwrap()).unwrap();
    let before = std::fs::read(model_identity_path(dir.path(), spec)).unwrap();
    let previous = target.with_extension("previous");
    std::fs::rename(&target, &previous).unwrap();
    std::fs::File::create(&target).unwrap()
        .set_len(installed_bytes(spec.pinned.as_ref().unwrap())).unwrap();
    assert_eq!(verified_model_digest(&target), None);
    let state = state_of(dir.path(), spec);
    assert_eq!(state.status, BinaryStatus::InstalledUnchecked);
    assert_eq!(state.installed_version, None);
    assert_eq!(std::fs::read(model_identity_path(dir.path(), spec)).unwrap(), before);
}

#[test]
fn invalid_and_newer_model_sidecars_never_supply_verified_digests_or_get_rewritten() {
    let (dir, spec, target) = model_identity_fixture();
    write_model_identity(dir.path(), spec, spec.pinned.as_ref().unwrap()).unwrap();
    let sidecar = model_identity_path(dir.path(), spec);
    let valid: serde_json::Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    let cases = [
        ("sha256", serde_json::json!("a".repeat(63))),
        ("sha256", serde_json::json!("g".repeat(64))),
        ("bytes", serde_json::json!(installed_bytes(spec.pinned.as_ref().unwrap()) + 1)),
        ("fileVersion", serde_json::Value::Null),
        ("formatVersion", serde_json::json!(formats::MODEL_IDENTITY + 1)),
    ];
    for (key, value) in cases {
        let mut document = valid.clone();
        document[key] = value;
        let before = serde_json::to_vec(&document).unwrap();
        std::fs::write(&sidecar, &before).unwrap();
        assert_eq!(verified_model_digest(&target), None, "{key}");
        assert_eq!(state_of(dir.path(), spec).status, BinaryStatus::InstalledUnchecked);
        assert_eq!(std::fs::read(&sidecar).unwrap(), before);
    }
}

#[test]
fn failed_ffmpeg_probe_retries_and_success_belongs_to_the_observed_artifact() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(BIN_DIR_NAME)).unwrap();
    let path = ffmpeg_path(root.path());
    std::fs::write(&path, b"first binary").unwrap();
    let cache = std::sync::Mutex::new(None);
    assert_eq!(installed_ffmpeg_version_with(root.path(), &cache, |_| None), None);
    assert_eq!(installed_ffmpeg_version_with(root.path(), &cache, |_| Some("9.0".into())).as_deref(), Some("9.0"));
    assert_eq!(installed_ffmpeg_version_with(root.path(), &cache, |_| panic!("successful observation should be reused")).as_deref(), Some("9.0"));
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, b"second binary").unwrap();
    assert_eq!(installed_ffmpeg_version_with(root.path(), &cache, |_| Some("9.1".into())).as_deref(), Some("9.1"));
}

#[test]
fn ffmpeg_replacement_during_probe_cannot_cache_the_old_answer() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(BIN_DIR_NAME)).unwrap();
    let path = ffmpeg_path(root.path());
    std::fs::write(&path, b"first binary").unwrap();
    let cache = std::sync::Mutex::new(None);
    let result = installed_ffmpeg_version_with(root.path(), &cache, |_| {
        std::fs::rename(&path, path.with_extension("old")).unwrap();
        std::fs::write(&path, b"second binary").unwrap();
        Some("9.0".into())
    });
    assert!(result.is_none());
    assert!(cache.lock().unwrap().is_none());
    assert_eq!(installed_ffmpeg_version_with(root.path(), &cache, |_| Some("9.1".into())).as_deref(), Some("9.1"));
}

#[test]
fn ffmpeg_sidecar_cannot_label_a_replacement_or_unidentified_legacy_binary() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(BIN_DIR_NAME)).unwrap();
    let path = ffmpeg_path(root.path());
    std::fs::write(&path, b"first binary").unwrap();
    let first = binary_file_version(&path).unwrap();
    write_version_sidecar(root.path(), "autobuild-first", first).unwrap();
    assert_eq!(read_version_sidecar(root.path()).as_deref(), Some("autobuild-first"));
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    std::fs::write(&path, b"second binary").unwrap();
    let old_record = std::fs::read(version_sidecar_path(root.path())).unwrap();
    assert_eq!(read_version_sidecar(root.path()), None, "interruption before the sidecar write leaves identity unknown");
    assert!(write_version_sidecar(root.path(), "autobuild-first", first).is_err());
    assert_eq!(std::fs::read(version_sidecar_path(root.path())).unwrap(), old_record);
    write_version_sidecar(root.path(), "autobuild-second", binary_file_version(&path).unwrap()).unwrap();
    assert_eq!(read_version_sidecar(root.path()).as_deref(), Some("autobuild-second"));
    storage::write_json_file(&version_sidecar_path(root.path()), &serde_json::json!({"version": "autobuild-legacy"}), formats::FFMPEG_VERSION_SIDECAR, false).unwrap();
    let legacy = std::fs::read(version_sidecar_path(root.path())).unwrap();
    assert_eq!(read_version_sidecar(root.path()), None);
    assert_eq!(std::fs::read(version_sidecar_path(root.path())).unwrap(), legacy, "inspection preserves unbound legacy evidence");
}
