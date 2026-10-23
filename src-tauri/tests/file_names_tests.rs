// A name the filesystem itself refuses is told apart from a failing drive by
// the platform's own answer, so these probe a real volume rather than build
// the error by hand.

use onecopy_lib::file_names::is_name_error;

#[test]
fn a_missing_folder_is_not_a_name_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = std::fs::write(dir.path().join("absent").join("a.jpg"), b"x").unwrap_err();
    assert!(!is_name_error(&error), "{error:?}");
}

// APFS and HFS+ store names as UTF-8 and refuse any other bytes.
#[cfg(target_os = "macos")]
#[test]
fn a_name_that_is_not_utf8_is_refused_as_a_name() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let name = std::ffi::OsStr::from_bytes(b"\xff.jpg");
    let error = std::fs::write(dir.path().join(name), b"x").unwrap_err();
    assert!(is_name_error(&error), "{error:?}");
}

windows_test! {
    "Windows only: NTFS refuses reserved characters in names",
    fn a_name_with_a_reserved_character_is_refused_as_a_name() {
        let dir = tempfile::tempdir().unwrap();
        let error = std::fs::write(dir.path().join("a<b.jpg"), b"x").unwrap_err();
        assert!(is_name_error(&error), "{error:?}");
    }
}

#[cfg(unix)]
#[test]
fn native_names_compare_and_rename_without_replacement_aliases() {
    use onecopy_lib::file_names::{renamed, FolderNames, RenameStyle};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    let first = Path::new(std::ffi::OsStr::from_bytes(b"image\xff.jpg"));
    let other = Path::new(std::ffi::OsStr::from_bytes(b"image\xfe.jpg"));
    for rules in [FolderNames::new(false, false), FolderNames::new(true, true)] {
        assert!(!rules.same(first, other));
        assert!(!rules.same(first, Path::new("image\u{FFFD}.jpg")));
    }
    assert_eq!(
        renamed(first, 2, RenameStyle::SpaceNumber).unwrap(),
        Path::new(std::ffi::OsStr::from_bytes(b"image\xff 2.jpg"))
    );
}
