use super::ensure_revealable_data_subdir;

#[test]
fn revealable_folder_is_created_lazily() {
    let root = tempfile::tempdir().unwrap();
    let target = ensure_revealable_data_subdir(root.path(), "logs").unwrap();
    assert!(target.is_dir());
    assert_eq!(target, root.path().join("logs"));
}

#[test]
fn arbitrary_subdirectories_remain_rejected() {
    let root = tempfile::tempdir().unwrap();
    assert!(ensure_revealable_data_subdir(root.path(), "../private").is_err());
    assert!(!root.path().join("private").exists());
}
