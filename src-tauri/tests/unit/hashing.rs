use super::*;

#[test]
fn read_back_stays_bound_to_the_writer_when_its_path_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let staged = dir.path().join("stage.tmp");
    let held = dir.path().join("held.tmp");
    std::fs::write(&source, b"copied bytes").unwrap();

    let (hash, bytes, mut private) =
        hash_while_copying_with_after_sync(&source, &staged, |path| {
            std::fs::rename(path, &held).unwrap();
            std::fs::write(path, b"replacement").unwrap();
        })
        .unwrap();

    assert_eq!(hash, blake3::hash(b"copied bytes").to_hex().to_string());
    assert_eq!(bytes, 12);
    assert!(private.is_named_by(&held));
    assert_eq!(std::fs::read(&staged).unwrap(), b"replacement");
}

#[test]
fn a_cancel_during_read_back_verification_abandons_the_private_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let staged = dir.path().join("stage.tmp");
    std::fs::write(&source, vec![7u8; 3 * BUF_SIZE]).unwrap();
    let synced = std::cell::Cell::new(false);

    let copied = hash_while_copying_detailed(
        &source,
        &staged,
        &|| synced.get(),
        &mut |_, _| {},
        |_| synced.set(true),
    );

    assert!(matches!(copied, Err(CopyFailure::Cancelled)));
    assert!(!staged.exists());
}
