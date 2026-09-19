use super::*;

#[test]
fn read_back_stays_bound_to_the_writer_when_its_path_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let staged = dir.path().join("stage.tmp");
    let held = dir.path().join("held.tmp");
    std::fs::write(&source, b"copied bytes").unwrap();

    let (hash, bytes, identity) =
        hash_while_copying_with_after_sync(&source, &staged, |path| {
            std::fs::rename(path, &held).unwrap();
            std::fs::write(path, b"replacement").unwrap();
        })
        .unwrap();

    assert_eq!(hash, blake3::hash(b"copied bytes").to_hex().to_string());
    assert_eq!(bytes, 12);
    assert!(crate::file_identity::path_names(&held, identity));
    assert_eq!(std::fs::read(&staged).unwrap(), b"replacement");
}
