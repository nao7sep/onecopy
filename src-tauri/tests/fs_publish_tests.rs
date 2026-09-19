use onecopy_lib::file_identity::{path_names, FileIdentity};
use onecopy_lib::fs_publish::*;

#[test]
fn publication_is_atomic_no_clobber_and_moves_the_exact_file() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("output-random.tmp");
    let target = dir.path().join("output.jpg");
    std::fs::write(&staged, b"ours").unwrap();
    let expected = FileIdentity::from_path(&staged).unwrap();

    rename_no_replace(&staged, &target).unwrap();
    assert!(!staged.exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"ours");
    assert!(path_names(&target, expected));
}

#[test]
fn occupied_target_survives_and_completed_stage_remains_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("output-random.tmp");
    let target = dir.path().join("output.jpg");
    std::fs::write(&staged, b"ours").unwrap();
    std::fs::write(&target, b"winner").unwrap();

    assert!(rename_no_replace(&staged, &target).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"winner");
    assert_eq!(std::fs::read(&staged).unwrap(), b"ours");
}

#[test]
fn private_cache_replacement_keeps_one_complete_version() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("transcript-random.tmp");
    let target = dir.path().join("transcript.txt");
    std::fs::write(&staged, b"replacement").unwrap();
    std::fs::write(&target, b"previous").unwrap();

    replace_existing(&staged, &target).unwrap();

    assert!(!staged.exists());
    assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
}
