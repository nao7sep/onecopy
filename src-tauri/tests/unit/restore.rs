use super::*;

// A volume without exclusive rename refuses restore while keeping the
// recoverable file and any competing destination intact.
#[cfg(target_os = "macos")]
#[test]
fn restore_preserves_recoverable_files_without_exclusive_rename() {
    use crate::fs_publish::seam::without_exclusive_rename;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("a.jpg");
    std::fs::write(&file, b"bytes").unwrap();
    let record = trash::trash_file(
        &file,
        &root,
        None,
        &trash::TrashContext::new(trash::TrashKind::Delete, "op"),
    )
    .unwrap();
    let listing = trash::list_root(&root, dir.path()).unwrap();
    let entry = listing.entries[0].clone();
    let stored = std::fs::File::open(&record.stored_path).unwrap();

    std::fs::write(&file, b"occupied").unwrap();
    let occupied = without_exclusive_rename(|| {
        restore_one(&root, &entry, &file, &crate::file_identity::volume_of)
    });
    assert!(matches!(occupied, Err(StepFailure::File(NOTICE_FAILED, _))));
    assert_eq!(std::fs::read(&file).unwrap(), b"occupied");
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"bytes");

    std::fs::remove_file(&file).unwrap();
    let refused = without_exclusive_rename(|| {
        restore_one(&root, &entry, &file, &crate::file_identity::volume_of)
    });
    assert!(matches!(refused, Err(StepFailure::File(NOTICE_FAILED, _))));
    assert!(
        !file.exists(),
        "refusal must not expose an empty placeholder"
    );
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"bytes");
    assert!(crate::file_identity::path_names_file(
        Path::new(&record.stored_path),
        &stored
    ));
    assert_eq!(
        trash::list_root(&root, dir.path()).unwrap().entries.len(),
        1
    );
}

// A read-only or full drive, or a root that went away, stops the
// unstarted remainder; an ordinary refusal fails only its own file.
#[test]
fn full_read_only_or_vanished_storage_stops_the_remainder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for kind in [io::ErrorKind::ReadOnlyFilesystem, io::ErrorKind::StorageFull] {
        assert!(stops_remainder(&io::Error::from(kind), root), "{kind:?}");
    }
    assert!(!stops_remainder(&io::Error::from(io::ErrorKind::PermissionDenied), root));
    let unplugged = dir.path().join("unplugged");
    assert!(stops_remainder(&io::Error::from(io::ErrorKind::PermissionDenied), &unplugged));
}
