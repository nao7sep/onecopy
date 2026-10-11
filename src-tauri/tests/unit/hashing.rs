use super::*;

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
        false,
        |_| synced.set(true),
    );

    assert!(matches!(copied, Err(CopyFailure::Cancelled)));
    assert!(!staged.exists());
}

#[cfg(unix)]
#[test]
fn streamed_bytes_stay_private_until_source_metadata_is_applied() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let stage = dir.path().join("stage");
    std::fs::write(&source, vec![7; BUF_SIZE * 2]).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (_, _, private, _) = hash_while_copying_detailed(&source, &stage, &|| false, &mut |copied, _| {
        if copied > 0 {
            assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }, false, |_| {}).unwrap();
    assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o644);
    drop(private);
}

#[test]
fn move_proof_rejects_an_inplace_change_during_the_source_read() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let stage = dir.path().join("stage.tmp");
    std::fs::write(&source, vec![7u8; 2 * BUF_SIZE]).unwrap();
    let mut edited = false;
    let copied = hash_while_copying_detailed(
        &source, &stage, &|| false, &mut |done, _| {
            if done > 0 && !edited {
                std::fs::write(&source, b"new-authored-bytes").unwrap();
                edited = true;
            }
        }, true, |_| {},
    );
    assert!(edited);
    assert!(matches!(copied, Err(CopyFailure::SourceChanged)));
    assert!(!stage.exists(), "the changed read cannot become a cleanup proof");
    assert_eq!(std::fs::read(&source).unwrap(), b"new-authored-bytes");
}
