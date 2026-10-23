use super::*;

#[test]
fn foreign_private_days_are_excluded_without_hiding_legacy_days() {
    let own = account_prefix().unwrap();
    assert!(visible_day(
        std::ffi::OsStr::new(&format!("{own}20261024-utc")),
        &own
    ));
    assert!(visible_day(std::ffi::OsStr::new("20261024-utc"), &own));
    assert!(!visible_day(
        std::ffi::OsStr::new("private-other--20261024-utc"),
        &own
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn creates_owner_only_directory_and_refuses_existing_shared_directory() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    let guard = Directory::open(&private, true).unwrap();
    let mode = std::fs::metadata(&private).unwrap().mode();
    assert_eq!(mode & 0o777, 0o700);
    guard
        .append(&private.join("manifest.jsonl"), "private provenance".into())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(private.join("manifest.jsonl")).unwrap(),
        "private provenance\n"
    );
    drop(guard);
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Directory::open(&private, true).is_err());
    assert_eq!(std::fs::metadata(&private).unwrap().mode() & 0o777, 0o755);
}

#[cfg(target_os = "macos")]
#[test]
fn held_directory_prevents_path_substitution_from_exposing_the_manifest() {
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("private");
    let guard = Directory::open(&original, true).unwrap();
    let displaced = root.path().join("still-private");
    std::fs::rename(&original, &displaced).unwrap();
    std::fs::create_dir(&original).unwrap();
    guard
        .append(
            &original.join("manifest.jsonl"),
            "private provenance".into(),
        )
        .unwrap();
    assert!(!original.join("manifest.jsonl").exists());
    assert_eq!(
        std::fs::read_to_string(displaced.join("manifest.jsonl")).unwrap(),
        "private provenance\n"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn existing_extended_grants_are_refused_and_new_inherited_grants_are_removed() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let inherited = root.path().join("inherited");
    std::fs::create_dir(&inherited).unwrap();
    let applied = std::process::Command::new("/bin/chmod")
        .args([
            "+a",
            "everyone allow read,search,file_inherit,directory_inherit",
        ])
        .arg(&inherited)
        .status()
        .unwrap();
    assert!(applied.success());
    let private = inherited.join("private");
    let _guard = Directory::open(&private, true).unwrap();
    let existing = root.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read,search"])
        .arg(&existing)
        .status()
        .unwrap()
        .success());
    assert!(Directory::open(&existing, false).is_err());
}
