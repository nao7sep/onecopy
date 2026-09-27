use super::*;

// exFAT on macOS has no exclusive rename: the restore reserves the original
// name with an empty placeholder and replaces only that, so an interruption
// can leave an empty file there, never partial bytes, and an occupied name
// is still never replaced.
#[cfg(target_os = "macos")]
#[test]
fn restore_works_on_a_volume_without_exclusive_rename() {
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

    std::fs::write(&file, b"occupied").unwrap();
    let occupied = without_exclusive_rename(|| {
        restore_one(&root, &entry, &file, &crate::file_identity::volume_of)
    });
    assert!(matches!(occupied, Err(StepFailure::File(OCCUPIED, _))));
    assert_eq!(std::fs::read(&file).unwrap(), b"occupied");

    std::fs::remove_file(&file).unwrap();
    let restored = without_exclusive_rename(|| {
        restore_one(&root, &entry, &file, &crate::file_identity::volume_of)
    });
    assert!(restored.is_ok());
    assert_eq!(std::fs::read(&file).unwrap(), b"bytes");
    assert!(!Path::new(&record.stored_path).exists());
}

// 54, 56, 6: a read-only or full drive, or a root that went away, stops the
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
