// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::scanner::ScanLists;
use onecopy_lib::watcher::*;
use onecopy_lib::extensions;
use onecopy_lib::index_store;
use std::collections::HashSet;
use std::path::PathBuf;

fn lists() -> ScanLists {
    let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    ScanLists {
        images: owned(extensions::IMAGE_EXTENSIONS),
        videos: owned(extensions::VIDEO_EXTENSIONS),
        companions: owned(extensions::COMPANION_EXTENSIONS),
    }
}

#[test]
fn restat_upserts_new_files_and_marks_vanished_missing() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-watch-")
        .tempdir()
        .unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let root = dir.path().join("watched");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("new.jpg"), b"fresh").unwrap();

    let changed = restat_dir(&conn, &root, &lists()).unwrap();
    assert_eq!(changed, 1);
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM paths WHERE missing = 0", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);

    // Unchanged re-stat: nothing to do.
    assert_eq!(restat_dir(&conn, &root, &lists()).unwrap(), 0);

    // Vanished file: marked missing, row kept.
    std::fs::remove_file(root.join("new.jpg")).unwrap();
    assert_eq!(restat_dir(&conn, &root, &lists()).unwrap(), 1);
    let missing: i64 = conn
        .query_row("SELECT COUNT(*) FROM paths WHERE missing = 1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(missing, 1);
}


fn fold(paths: Vec<PathBuf>) -> (HashSet<PathBuf>, bool) {
    let mut dirty = HashSet::new();
    let mut overflowed = false;
    let event = notify::Event {
        kind: notify::EventKind::Modify(notify::event::ModifyKind::Any),
        paths,
        attrs: Default::default(),
    };
    collect(Ok(event), &mut dirty, &mut overflowed);
    (dirty, overflowed)
}

#[test]
fn a_file_event_marks_its_parent_directory_dirty() {
    // The drain calls read_dir on whatever lands in the set. A file path there
    // fails silently, so new photos would simply never appear.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("IMG_0001.jpg");
    std::fs::write(&file, b"x").unwrap();

    let (dirty, overflowed) = fold(vec![file]);
    assert!(!overflowed);
    assert_eq!(
        dirty.into_iter().collect::<Vec<_>>(),
        vec![dir.path().to_path_buf()],
        "a file event marks the directory, never the file"
    );
}

#[test]
fn a_directory_event_marks_the_directory_itself() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let (dirty, _) = fold(vec![sub.clone()]);
    assert_eq!(dirty.into_iter().collect::<Vec<_>>(), vec![sub]);
}

#[test]
fn the_apps_own_trash_is_never_marked_dirty() {
    // Trashing is a same-volume rename INSIDE a watched root, so every delete
    // fires events here; re-indexing them would resurrect what was just culled.
    let dir = tempfile::tempdir().unwrap();
    let trashed = dir
        .path()
        .join(onecopy_lib::trash::TRASH_DIR_NAME)
        .join("20260101-utc")
        .join("IMG_0001.jpg");
    std::fs::create_dir_all(trashed.parent().unwrap()).unwrap();
    std::fs::write(&trashed, b"x").unwrap();

    let (dirty, _) = fold(vec![trashed]);
    assert!(dirty.is_empty(), "the app's own trash is not source material");
}

#[test]
fn a_lost_event_batch_flags_an_overflow() {
    // notify drops events under load; the flag is what turns that into a
    // visible "Rescan needed" instead of a silently incomplete index.
    let mut dirty = HashSet::new();
    let mut overflowed = false;
    collect(
        Err(notify::Error::generic("watch queue overflowed")),
        &mut dirty,
        &mut overflowed,
    );
    assert!(overflowed, "a watcher error must raise the rescan flag");
    assert!(dirty.is_empty());
}
