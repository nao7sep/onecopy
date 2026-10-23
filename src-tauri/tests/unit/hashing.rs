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
            // Private: no group or other access while the bytes stream in.
            assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o077, 0);
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

#[test]
fn read_back_rejects_corrupted_output_and_preserves_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.bin");
    let stage = dir.path().join("stage.tmp");
    let original = vec![7u8; 2 * BUF_SIZE];
    std::fs::write(&source, &original).unwrap();
    let copied = hash_while_copying_detailed(
        &source, &stage, &|| false, &mut |_, _| {}, false,
        |path| std::fs::write(path, vec![8u8; original.len()]).unwrap(),
    );
    match copied {
        Err(CopyFailure::Destination(error)) => assert_eq!(
            error.to_string(), "staged destination read-back did not match the copied bytes",
        ),
        _ => panic!("corrupted output must fail read-back verification"),
    }
    assert!(!stage.exists(), "rejected output must remain private and be removed");
    assert_eq!(std::fs::read(&source).unwrap(), original);
}

#[test]
fn only_whole_file_prehashes_carry_complete_version_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    for size in [0, 1, PREHASH_WINDOW, 2 * PREHASH_WINDOW, 2 * PREHASH_WINDOW + 1] {
        std::fs::write(&path, vec![7; size as usize]).unwrap();
        let evidence = prehash_evidence(&path).unwrap();
        assert_eq!(evidence.complete_version.is_some(), size <= 2 * PREHASH_WINDOW);
        if let Some(version) = evidence.complete_version {
            assert!(version.matches_path(&path).unwrap());
            assert_eq!(evidence.hash, full_hash(&path).unwrap());
        }
    }
}

#[test]
fn a_changed_file_cannot_supply_complete_prehash_evidence() {
    use crate::volume_io::{FakeStallingVolume, Op};
    use std::time::Duration;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.bin");
    std::fs::write(&path, b"old").unwrap();
    let volume = FakeStallingVolume::mount(directory.path(), Duration::from_secs(30));
    volume.stall(&[Op::Read], None);
    let worker_path = path.clone();
    let worker = std::thread::spawn(move || prehash_evidence(&worker_path));
    let held = volume.wait_until_held(1, Duration::from_secs(5));
    std::fs::write(&path, b"replacement").unwrap();
    volume.release();
    let result = worker.join().unwrap();
    assert!(held);
    assert!(result.err().unwrap().to_string().contains("source changed"));
}

#[test]
fn an_early_end_cannot_satisfy_a_complete_prehash_range() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("short.bin");
    std::fs::write(&path, b"short").unwrap();
    let mut file = volume_io::open_read(&path).unwrap();
    let error = copy_exact_into(&mut file, &mut blake3::Hasher::new(), 6).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
}
