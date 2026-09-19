use onecopy_lib::file_identity::*;

#[test]
fn private_cleanup_preserves_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stage.tmp");
    let held = dir.path().join("held.tmp");
    std::fs::write(&path, b"ours").unwrap();
    let identity = FileIdentity::from_path(&path).unwrap();
    std::fs::rename(&path, &held).unwrap();
    std::fs::write(&path, b"winner").unwrap();

    remove_private_if_owned(&path, identity);

    assert_eq!(std::fs::read(&path).unwrap(), b"winner");
    assert_eq!(std::fs::read(&held).unwrap(), b"ours");
}

#[test]
fn physical_claim_rejects_and_restores_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stage.tmp");
    let ours = dir.path().join("ours.tmp");
    std::fs::write(&path, b"ours").unwrap();
    let identity = FileIdentity::from_path(&path).unwrap();
    std::fs::rename(&path, &ours).unwrap();
    std::fs::write(&path, b"winner").unwrap();

    assert!(claim_private(&path, identity).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"winner");
    assert_eq!(std::fs::read(&ours).unwrap(), b"ours");
}

#[cfg(unix)]
#[test]
fn nofollow_regular_open_rejects_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.bin");
    let link = dir.path().join("link.bin");
    std::fs::write(&real, b"bytes").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert!(open_regular_nofollow(&link).is_err());
    assert_eq!(std::fs::read(&real).unwrap(), b"bytes");
}
