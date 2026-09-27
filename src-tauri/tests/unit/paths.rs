use super::*;

#[test]
fn default_root_is_home_dot_onecopy() {
    let home = PathBuf::from("/home/tester");
    // Unset / empty / whitespace all fall back to the default root.
    assert_eq!(resolve_root(&home, None).unwrap(), home.join(".onecopy"));
    assert_eq!(
        resolve_root(&home, Some(String::new())).unwrap(),
        home.join(".onecopy")
    );
    assert_eq!(
        resolve_root(&home, Some("   ".to_string())).unwrap(),
        home.join(".onecopy")
    );
}

#[test]
fn env_var_relocates_root_to_absolute_path() {
    let home = PathBuf::from("/home/tester");
    assert_eq!(
        resolve_root(&home, Some("/tmp/oc-test".to_string())).unwrap(),
        PathBuf::from("/tmp/oc-test")
    );
}

#[test]
fn env_var_expands_leading_tilde() {
    let home = PathBuf::from("/home/tester");
    assert_eq!(resolve_root(&home, Some("~".to_string())).unwrap(), home);
    assert_eq!(
        resolve_root(&home, Some("~/profiles/work".to_string())).unwrap(),
        home.join("profiles/work")
    );
}

#[test]
fn relative_env_var_resolves_against_home_not_cwd() {
    let home = PathBuf::from("/home/tester");
    assert_eq!(
        resolve_root(&home, Some("alt-root".to_string())).unwrap(),
        home.join("alt-root")
    );
}

#[test]
fn expands_environment_references_in_the_override() {
    let home = PathBuf::from("/home/tester");
    std::env::set_var("ONECOPY_TEST_BASE", "/mnt/disk2");
    assert_eq!(
        resolve_root(&home, Some("$ONECOPY_TEST_BASE/oc".to_string())).unwrap(),
        PathBuf::from("/mnt/disk2/oc")
    );
    assert_eq!(
        resolve_root(&home, Some("${ONECOPY_TEST_BASE}/oc".to_string())).unwrap(),
        PathBuf::from("/mnt/disk2/oc")
    );
    std::env::remove_var("ONECOPY_TEST_BASE");
}

#[test]
fn override_that_expands_to_empty_is_rejected() {
    let home = PathBuf::from("/home/tester");
    std::env::remove_var("ONECOPY_UNSET_FOR_TEST");
    assert!(resolve_root(&home, Some("$ONECOPY_UNSET_FOR_TEST".to_string())).is_err());
}

#[test]
fn is_within_data_root_matches_the_root_and_its_descendants_only() {
    // R6-02: a source containing the data root must be excluded from walking
    // and watching wherever it lies.
    let data_root = PathBuf::from("/Users/tester/.onecopy");
    assert!(is_within_data_root(&data_root, &data_root), "the root itself");
    assert!(is_within_data_root(
        &data_root.join("cache/previews/ab/abc123.webp"),
        &data_root
    ));
    assert!(
        is_within_data_root(&PathBuf::from("/Users/tester/.ONECOPY/index.sqlite3"), &data_root),
        "component comparison is case-insensitive, like the trash-name check"
    );
    assert!(!is_within_data_root(
        &PathBuf::from("/Users/tester/Pictures/vacation.jpg"),
        &data_root
    ));
    assert!(
        !is_within_data_root(&PathBuf::from("/Users/tester/.onecopy-extra/x"), &data_root),
        "a sibling name sharing the prefix is not a descendant"
    );
}

// R6-03: the data root must be private to the owner, and an existing root an
// earlier build left too open must be tightened, not just a freshly created
// one. Unix-only: Windows has no equivalent permission bits to assert on
// (the profile ACL already applies there).
// R6-03/Phase 9: a freshly created root must be born owner-only, not merely
// tightened afterward. Uses a throwaway home so a broad umask (e.g. 022)
// cannot leave the directory briefly world-readable before any tightening.
#[cfg(unix)]
#[test]
fn resolve_data_root_creates_a_fresh_root_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".onecopy");

    create_data_root(&root).unwrap();

    let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "the data root must be created owner-only, not just tightened after the fact"
    );
}

#[cfg(unix)]
#[test]
fn ensure_private_restricts_a_world_readable_root_to_the_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".onecopy");
    std::fs::create_dir_all(&root).unwrap();
    // Simulate a root an earlier build created under the default umask.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

    ensure_private(&root);

    let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "the data root must be readable only by its owner");
}

#[test]
fn revealable_folder_is_created_lazily() {
    let root = tempfile::tempdir().unwrap();
    let target = revealable_data_subdir(root.path(), "logs").unwrap();
    assert!(target.is_dir());
    assert_eq!(target, root.path().join("logs"));
}

#[test]
fn arbitrary_subdirectories_remain_rejected() {
    let root = tempfile::tempdir().unwrap();
    assert!(revealable_data_subdir(root.path(), "../private").is_err());
    assert!(!root.path().join("private").exists());
}
