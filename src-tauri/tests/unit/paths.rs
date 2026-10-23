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
    // A source containing the data root must be excluded from walking
    // and watching wherever it lies.
    let data_root = PathBuf::from("/Users/tester/.onecopy");
    assert!(is_within_data_root(&data_root, &data_root), "the root itself");
    assert!(is_within_data_root(
        &data_root.join("cache/previews/ab/abc123.webp"),
        &data_root
    ));
    assert_eq!(
        is_within_data_root(&PathBuf::from("/Users/tester/.ONECOPY/index.sqlite3"), &data_root),
        cfg!(windows),
        "Unix case-only siblings remain distinct; Windows retains its folded spelling comparison"
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

// The data root must be private to the owner, and an existing root an
// earlier build left too open must be tightened, not just a freshly created
// one. Unix mode-bit assertions live here; Windows ACL coverage lives with
// the Windows data-root privacy owner.
// This observes final Unix permissions. Restrictive creation itself is a
// source-level guarantee of DirBuilderExt::mode; a final stat cannot establish
// whether permissions were temporarily broader during creation.
#[cfg(unix)]
#[test]
fn a_fresh_data_root_has_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".onecopy");

    create_data_root(&root).unwrap();

    let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o700,
        "the finished data root must have owner-only permissions"
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
fn privacy_recovery_visits_only_known_app_owned_root_entries() {
    let kind = |name: &str| owned_data_entry(std::ffi::OsStr::new(name));
    for name in ["cache", "logs", "models", "bin", "temp"] { assert_eq!(kind(name), Some(true)); }
    for name in ["config.json", "index.sqlite3-wal", "records.sqlite3-shm", "installation-id",
        "config-20261024-010203-utc.invalid", "records-20261024-010203-utc.invalid-wal",
        "state-abcdefghijklmnopqrstu.tmp", "installation-id-abcdefghijklmnopqrstu.tmp", "CONFIG.JSON", PRIVATE_ROOT_CERTIFICATE] { assert_eq!(kind(name), Some(false), "{name}"); }
    for name in ["photos", "notes.txt", "config-my-notes.invalid", "config-🙂.tmp", "logs-old",
        "config-20261024-010203-utx.invalid", "config-abcdefgh-010203-utc.invalid", "config.json-wal"] {
        assert_eq!(kind(name), None, "{name}");
    }
}

#[cfg(windows)]
#[test]
fn data_root_containment_preserves_windows_drive_and_unc_components() {
    for (root, child, sibling) in [
        (r"C:\Users\tester\onecopy", r"c:\users\TESTER\ONECOPY\cache", r"C:\Users\tester\onecopy-more"),
        (r"C:/Users/tester/onecopy", r"\\?\C:\Users\tester\onecopy\cache", r"D:\Users\tester\onecopy"),
        (r"\\server\share\onecopy", r"\\SERVER\SHARE\OneCopy\logs", r"\\server\other-share\onecopy"),
    ] {
        assert!(is_within_data_root(Path::new(child), Path::new(root)));
        assert!(!is_within_data_root(Path::new(sibling), Path::new(root)));
    }
}
