// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use std::path::Path;
use onecopy_lib::scanner::ScanLists;
use onecopy_lib::watcher::*;
use onecopy_lib::extensions;
use onecopy_lib::index_store;

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
