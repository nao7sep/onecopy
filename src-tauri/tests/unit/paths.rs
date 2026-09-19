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
