// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

// The only test here is macOS-only, so the import follows it: on Windows
// this file compiles to nothing at all.
#[cfg(target_os = "macos")]
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
