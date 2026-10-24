use super::seam::without_exclusive_rename;
use super::*;

#[test]
fn a_volume_without_exclusive_rename_preserves_the_source_without_publishing() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("output-random.tmp");
    let target = dir.path().join("output.jpg");
    std::fs::write(&staged, b"ours").unwrap();
    let file = std::fs::File::open(&staged).unwrap();

    let error = without_exclusive_rename(|| rename_no_replace(&staged, &target)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(!target.exists());
    assert_eq!(std::fs::read(&staged).unwrap(), b"ours");
    assert!(crate::file_identity::path_names_file(&staged, &file));
}

#[test]
fn a_volume_without_exclusive_rename_never_replaces_an_occupied_target() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("output-random.tmp");
    let target = dir.path().join("output.jpg");
    std::fs::write(&staged, b"ours").unwrap();
    std::fs::write(&target, b"winner").unwrap();
    let winner = std::fs::File::open(&target).unwrap();

    let error = without_exclusive_rename(|| rename_no_replace(&staged, &target)).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert_eq!(std::fs::read(&target).unwrap(), b"winner");
    assert!(crate::file_identity::path_names_file(&target, &winner));
    assert_eq!(std::fs::read(&staged).unwrap(), b"ours");
}

#[test]
fn an_unsupported_publication_creates_no_placeholder_for_invalid_sources() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("vanished.tmp");
    let target = dir.path().join("output.jpg");

    let error = without_exclusive_rename(|| rename_no_replace(&missing, &target)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(!target.exists());

    let directory = dir.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    let error = without_exclusive_rename(|| rename_no_replace(&directory, &target)).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    assert!(!target.exists());
    assert!(directory.is_dir());
}

#[test]
fn a_proven_move_without_exclusive_rename_preserves_a_queued_source_replacement() {
    use crate::file_identity::{open_regular_nofollow, FileVersion};
    use crate::volume_io::{FakeStallingVolume, Op};
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.jpg");
    let target = dir.path().join("stored.jpg");
    std::fs::write(&source, b"covered-by-output").unwrap();
    let (mut file, _) = open_regular_nofollow(&source).unwrap();
    let version = FileVersion::of(&mut file).unwrap();
    let bound = Duration::from_secs(10);
    let volume = FakeStallingVolume::mount(dir.path(), bound);
    volume.stall(&[Op::Rename], None);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            assert!(volume.wait_until_held(1, bound));
            std::fs::rename(&source, dir.path().join("old-source")).unwrap();
            std::fs::write(&source, b"new-uncovered-source-bytes").unwrap();
            volume.release();
        });
        without_exclusive_rename(|| rename_into_directory(&source, &target, std::sync::Arc::new(std::fs::File::open(dir.path()).unwrap()), Some(version), None))
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Unsupported);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        b"new-uncovered-source-bytes"
    );
    assert_eq!(
        std::fs::read(dir.path().join("old-source")).unwrap(),
        b"covered-by-output"
    );
    assert!(!target.exists());
}
