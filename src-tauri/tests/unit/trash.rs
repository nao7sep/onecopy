use super::*;

#[test]
fn exact_boundary_winner_survives_and_source_remains_authoritative() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("photo.jpg");
    std::fs::write(&source, b"source").unwrap();

    let result = trash_file_with_before_move(&source, &source_dir, None, |target| {
        std::fs::write(target, b"winner").unwrap()
    });

    assert!(result.is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"source");
    let day = std::fs::read_dir(source_dir.join(TRASH_DIR_NAME))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(std::fs::read(day.join("photo.jpg")).unwrap(), b"winner");
}

#[test]
fn replacement_before_the_move_is_the_file_that_gets_trashed() {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let source = source_dir.join("photo.jpg");
    let held = source_dir.join("held.jpg");
    std::fs::write(&source, b"original").unwrap();

    let result = trash_file_with_before_move(&source, &source_dir, None, |_| {
        std::fs::rename(&source, &held).unwrap();
        std::fs::write(&source, b"replacement").unwrap();
    })
    .unwrap();

    assert!(!source.exists());
    assert_eq!(std::fs::read(&held).unwrap(), b"original");
    assert_eq!(std::fs::read(result.stored_path).unwrap(), b"replacement");
}
