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

#[cfg(unix)]
#[test]
fn streamed_bytes_stay_private_until_source_metadata_is_applied() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let stage = dir.path().join("stage");
    std::fs::write(&source, vec![7; BUF_SIZE * 2]).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (_, _, private) = hash_while_copying_detailed(&source, &stage, &|| false, &mut |copied, _| {
        if copied > 0 {
            assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }, |_| {}).unwrap();
    assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o644);
    drop(private);
}

#[cfg(target_os = "macos")]
#[test]
fn cancelling_after_retaining_a_writeattr_denial_cleans_the_owned_stage() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let stage = dir.path().join("stage");
    std::fs::write(&source, b"ACL bytes").unwrap();
    assert!(std::process::Command::new("/bin/chmod").args(["+a", "everyone deny writeattr"]).arg(&source).status().unwrap().success());
    let synced = std::cell::Cell::new(false);
    let result = hash_while_copying_detailed(&source, &stage, &|| synced.get(), &mut |_, _| {}, |_| synced.set(true));
    assert!(matches!(result, Err(CopyFailure::Cancelled)));
    assert!(!stage.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn inherited_read_grants_are_removed_before_streaming_and_source_acl_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("shared");
    std::fs::create_dir(&destination).unwrap();
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755)).unwrap();
    let chmod = |path: &Path, entry: &str| {
        assert!(std::process::Command::new("/bin/chmod").args(["+a", entry]).arg(path).status().unwrap().success());
    };
    let acl = |path: &Path| {
        let output = std::process::Command::new("/bin/ls").arg("-le").arg(path).output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().lines().skip(1).map(str::to_owned).collect::<Vec<_>>()
    };
    chmod(&destination, "everyone allow read,file_inherit");
    // Native control: mode600 by itself still has an inherited read grant.
    let control = destination.join("control");
    let mut options = std::fs::OpenOptions::new();
    use std::os::unix::fs::OpenOptionsExt;
    options.write(true).create_new(true).mode(0o600);
    drop(options.open(&control).unwrap());
    assert_eq!(std::fs::metadata(&control).unwrap().permissions().mode() & 0o777, 0o600);
    assert!(acl(&control).iter().any(|entry| entry.contains("inherited allow read")));
    let source = dir.path().join("source");
    let stage = destination.join("stage");
    std::fs::write(&source, vec![7; BUF_SIZE * 2]).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
    chmod(&source, "everyone deny execute");
    let (_, _, private) = hash_while_copying_detailed(&source, &stage, &|| false, &mut |copied, _| {
        if copied > 0 {
            assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o600);
            assert!(acl(&stage).is_empty(), "no inherited grant can bypass private mode while bytes stream");
        }
    }, |_| {}).unwrap();
    assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o640);
    assert_eq!(acl(&stage), acl(&source));
    drop(private);
    assert!(!stage.exists());
    let cancelled = std::cell::Cell::new(false);
    let result = hash_while_copying_detailed(&source, &stage, &|| cancelled.get(), &mut |copied, _| {
        if copied > 0 {
            assert!(acl(&stage).is_empty());
            cancelled.set(true);
        }
    }, |_| {});
    assert!(matches!(result, Err(CopyFailure::Cancelled)));
    assert!(!stage.exists());
    assert!(control.exists(), "cleanup only removes the owned stage");
}
