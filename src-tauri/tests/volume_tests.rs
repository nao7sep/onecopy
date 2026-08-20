// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

// One test per platform that can answer, so the import follows them both.
#[cfg(any(target_os = "macos", windows))]
use onecopy_lib::volume::volume_identity;

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
// it is PERSISTED in source_volumes, so its stored SHAPE is load-bearing too.
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
