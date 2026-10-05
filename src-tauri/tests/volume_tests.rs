// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

// One test per platform that can answer, so the import follows them both.
#[cfg(any(target_os = "macos", windows))]
use onecopy_lib::volume::{check_identity, enforce_no_substitution, volume_identity};

#[cfg(target_os = "macos")]
#[test]
fn same_volume_paths_share_one_nonempty_identity() {
    // Two temp paths live on the same (home/boot) volume: their identities
    // must agree and be non-empty. diskutil ships with every macOS.
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let ia = volume_identity(a.path()).expect("identity for temp dir");
    let ib = volume_identity(b.path()).expect("identity for temp dir");
    assert!(!ia.is_empty());
    assert_eq!(ia, ib);
}

// The Windows counterpart, and the only automated cover the
// GetVolumeInformationW FFI has. Agreement is half the contract: the identity
// is compared at the session gate and before every destructive operation, and
// it is persisted in source-volumes.json, so its stored shape is load-bearing too.
// A serial that started rendering in another form would read as a substituted
// drive and block work on a volume nothing is wrong with.
#[cfg(windows)]
#[test]
fn same_volume_paths_share_one_serial_in_the_stored_form() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let ia = volume_identity(a.path()).expect("identity for temp dir");
    let ib = volume_identity(b.path()).expect("identity for temp dir");
    assert_eq!(ia, ib, "one volume must answer with one identity");
    assert_eq!(ia.len(), 8, "the serial is stored as eight hex digits: {ia}");
    assert!(
        ia.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()),
        "the serial is stored as uppercase hex: {ia}"
    );
}

// (R1-14, R3-07) The volume-substitution gate is enforced in the backend,
// not only through the frontend's own recheck, and a failed check keeps it
// closed rather than reading as "nothing recorded".
#[cfg(any(target_os = "macos", windows))]
#[test]
fn enforce_no_substitution_passes_first_sight_and_an_unchanged_volume() {
    let app_data = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dir_str = dir.path().to_string_lossy().to_string();
    let identity = volume_identity(dir.path()).expect("identity for temp dir");

    // First sight: nothing recorded yet, so nothing to compare against.
    assert!(enforce_no_substitution(app_data.path(), &[dir_str.clone()]).is_ok());

    check_identity(app_data.path(), &dir_str, &identity).unwrap();
    assert!(
        enforce_no_substitution(app_data.path(), &[dir_str]).is_ok(),
        "the recorded identity still matches"
    );
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn enforce_no_substitution_refuses_a_different_volume_at_the_same_path() {
    let app_data = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dir_str = dir.path().to_string_lossy().to_string();
    // A recorded identity nothing on this machine can produce.
    check_identity(app_data.path(), &dir_str, "not-the-real-volume-identity").unwrap();

    let result = enforce_no_substitution(app_data.path(), &[dir_str]);
    assert!(result.is_err(), "a substituted volume must refuse admission");
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn enforce_no_substitution_fails_closed_when_the_record_cannot_be_read() {
    let app_data = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dir_str = dir.path().to_string_lossy().to_string();
    let identity = volume_identity(dir.path()).expect("identity for temp dir");
    check_identity(app_data.path(), &dir_str, &identity).unwrap();

    // Corrupt the store the check reads: a failed check must keep the gate
    // CLOSED, never read as "nothing recorded" and let work through.
    std::fs::write(app_data.path().join("source-volumes.json"), b"{ not json").unwrap();

    let result = enforce_no_substitution(app_data.path(), &[dir_str]);
    assert!(result.is_err(), "an unreadable record must refuse admission, not pass it open");
}

// A directory with a recorded identity that can no longer be read (a
// different volume without one at the same path, or a failed probe) keeps the
// gate closed. `/dev` is a directory on a filesystem `diskutil` cannot
// identify.
#[cfg(target_os = "macos")]
#[test]
fn enforce_no_substitution_fails_closed_when_a_recorded_volume_cannot_be_identified() {
    let app_data = tempfile::tempdir().unwrap();
    assert_eq!(volume_identity(std::path::Path::new("/dev")), None);
    check_identity(app_data.path(), "/dev", "recorded-volume-identity").unwrap();

    let result = enforce_no_substitution(app_data.path(), &["/dev".to_string()]);
    assert!(result.is_err(), "an unreadable current identity must refuse admission");
}

// A recorded source whose drive stops answering is never verified-safe: the
// gate refuses within the check's bound instead of waiting on the drive, and
// holds no lock while it probes, so another gate check still answers.
#[cfg(any(target_os = "macos", windows))]
#[test]
fn enforce_no_substitution_refuses_promptly_when_a_recorded_volume_does_not_answer() {
    use onecopy_lib::volume_io::FakeStallingVolume;
    let app_data = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dir_str = dir.path().to_string_lossy().to_string();
    check_identity(app_data.path(), &dir_str, "recorded-identity").unwrap();
    let volume = FakeStallingVolume::mount(dir.path(), std::time::Duration::from_millis(300));
    volume.stall(&[], None);

    let started = std::time::Instant::now();
    let refused = enforce_no_substitution(app_data.path(), std::slice::from_ref(&dir_str));
    assert!(refused.is_err_and(|error| error.contains("could not verify")));
    // The volume now fails fast; the gate answers again at once.
    assert!(enforce_no_substitution(app_data.path(), &[dir_str]).is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    volume.release();
    assert!(volume.wait_until_settled(std::time::Duration::from_secs(5)));
}

// A batch spanning several configured roots is not all-or-nothing: a
// substituted root's failure names only that root, and a caller that scopes
// the check to a different, healthy root sees no refusal at all (R3-07,
// R1-14 — work touching an unaffected drive keeps working). The volume probe
// is stood in for: this is the gate's scoping, and the real probe has its own
// tests above.
#[test]
fn enforce_no_substitution_refuses_only_the_affected_root_in_a_multi_root_batch() {
    let app_data = tempfile::tempdir().unwrap();
    let healthy = tempfile::tempdir().unwrap();
    let healthy_str = healthy.path().to_string_lossy().to_string();
    onecopy_lib::volume::check_identity(app_data.path(), &healthy_str, "this-volume").unwrap();

    let substituted = tempfile::tempdir().unwrap();
    let substituted_str = substituted.path().to_string_lossy().to_string();
    onecopy_lib::volume::check_identity(app_data.path(), &substituted_str, "not-the-real-volume-identity")
        .unwrap();
    let probe = |_: &std::path::Path| Some("this-volume".to_string());

    // Scoped to only the healthy root: no refusal, even though the store
    // also records a substituted one elsewhere.
    assert!(onecopy_lib::volume::enforce_no_substitution_with(
        app_data.path(),
        &[healthy_str.clone()],
        &probe
    )
    .is_ok());

    // A batch touching both roots names the substituted one and still
    // reports the healthy one as fine by not mentioning it.
    let result = onecopy_lib::volume::enforce_no_substitution_with(
        app_data.path(),
        &[healthy_str.clone(), substituted_str.clone()],
        &probe,
    );
    let error = result.expect_err("a substituted root in the batch must refuse it");
    assert!(error.contains(&substituted_str), "names the affected root: {error}");
    assert!(!error.contains(&healthy_str), "does not implicate the healthy root: {error}");
}

#[test]
fn the_source_volume_record_carries_its_format_version() {
    let app_data = tempfile::tempdir().unwrap();
    onecopy_lib::volume::check_identity(app_data.path(), "/photos", "volume-a").unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(app_data.path().join("source-volumes.json")).unwrap()).unwrap();
    assert_eq!(stored["formatVersion"], onecopy_lib::formats::SOURCE_VOLUMES);
    assert_eq!(stored["sources"][0]["identity"], "volume-a");
}

#[test]
fn a_source_volume_record_written_by_a_newer_onecopy_closes_the_gate_and_is_left_as_it_is() {
    let app_data = tempfile::tempdir().unwrap();
    let path = app_data.path().join("source-volumes.json");
    let bytes = br#"{"formatVersion":2,"sources":[]}"#;
    std::fs::write(&path, bytes).unwrap();
    let error = onecopy_lib::volume::check_identity(app_data.path(), "/photos", "volume-a")
        .expect_err("a newer record is not read");
    assert!(error.contains("newer"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}
