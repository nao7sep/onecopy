// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::trash::*;
use std::path::{Path, PathBuf};

#[test]
fn reserved_trash_components_do_not_hide_unrelated_names() {
    for path in [
        PathBuf::from(TRASH_DIR_NAME),
        Path::new("root").join(".ONECOPY-TRASH").join("photo.jpg"),
        Path::new("root").join(TRASH_DIR_NAME).join("day").join("photo.jpg"),
    ] {
        assert!(is_trash_path(&path));
    }
    for path in [
        "root/.onecopy-trash-notes/photo.jpg",
        "root/my.onecopy-trash/photo.jpg",
        "root/.onecopy-trash.jpg",
        "root/.photo.jpg",
    ] {
        assert!(!is_trash_path(Path::new(path)), "{path}");
    }
}

// These tests run entirely under a configured temp root, so recoverable
// deletions stay inside the same permission and filesystem boundary.

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
}

fn fixture(label: &str) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-trash-{label}-"))
        .tempdir()
        .unwrap();
    let source = dir.path().join("photos");
    std::fs::create_dir_all(&source).unwrap();
    Fixture { _dir: dir, source }
}

fn read_manifest(day_dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(day_dir.join("manifest.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn trashing_moves_the_file_and_writes_a_manifest_line() {
    let f = fixture("basic");
    let file = f.source.join("img.jpg");
    std::fs::write(&file, b"bytes").unwrap();

    let record = trash_file(&file, &f.source, Some("hash123"), &ctx()).unwrap();
    assert!(!file.exists(), "the original must be gone");
    let stored = PathBuf::from(&record.stored_path);
    assert!(stored.exists(), "the stored file must exist");
    assert_eq!(std::fs::read(&stored).unwrap(), b"bytes");

    assert!(stored.starts_with(f.source.join(TRASH_DIR_NAME)));

    // The day folder is self-contained: manifest sits inside it.
    let day_dir = stored
        .ancestors()
        .find(|a| a.parent().is_some_and(|p| p.ends_with(TRASH_DIR_NAME)))
        .unwrap();
    let manifest = read_manifest(day_dir);
    assert_eq!(manifest.len(), 1);
    assert_eq!(manifest[0]["contentHash"], "hash123");
    assert_eq!(
        manifest[0]["originalPath"],
        file.to_string_lossy().to_string()
    );
}

#[test]
fn files_are_stored_flat_with_provenance_in_the_manifest() {
    // The day folder is a plain "everything deleted this day" view, like an OS
    // trash: names only, no mirrored directory structure. Provenance lives in
    // the manifest instead, which is also what keeps a trashed path from ever
    // growing longer than <trash>/<day>/<name> — the amplification that made
    // the platform path-length limit a deletion problem specifically.
    let f = fixture("flat");
    let nested = f.source.join("2016").join("spain");
    std::fs::create_dir_all(&nested).unwrap();
    let file = nested.join("beach.jpg");
    std::fs::write(&file, b"x").unwrap();

    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    let stored = PathBuf::from(&record.stored_path);

    assert_eq!(
        stored.file_name().unwrap(),
        std::ffi::OsStr::new("beach.jpg"),
        "the stored name is the original file name"
    );
    let day_dir = stored.parent().expect("stored inside a day folder");
    assert!(
        day_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-utc"),
        "the file sits DIRECTLY in the day folder, not under a rebuilt tree"
    );
    // Nothing from the source structure was recreated.
    for part in ["2016", "spain"] {
        assert!(
            !day_dir.join(part).exists(),
            "no source directory may be reproduced in the trash"
        );
    }
    // The full original path survives where it belongs.
    let manifest = read_manifest(day_dir);
    assert_eq!(manifest.len(), 1);
    assert_eq!(
        manifest[0]["originalPath"],
        file.to_string_lossy().to_string(),
        "the manifest is the provenance record"
    );
}

#[test]
fn same_day_same_path_collisions_get_suffixes_and_exact_manifest_lines() {
    let f = fixture("collide");
    let file = f.source.join("img.jpg");

    let mut stored_names = Vec::new();
    let mut last_record = None;
    for content in [b"first" as &[u8], b"second", b"third"] {
        std::fs::write(&file, content).unwrap();
        let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
        stored_names.push(
            PathBuf::from(&record.stored_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );
        last_record = Some(record);
    }
    let last_record = last_record.expect("three files were trashed");
    assert_eq!(stored_names[0], "img.jpg");
    // Hyphen and a number: a period would read as a second extension, and a
    // repeated separator would grow the name without bound.
    assert_eq!(stored_names[1], "img-2.jpg");
    assert_eq!(stored_names[2], "img-3.jpg");

    // "exact manifest lines" is the name's promise, and until now nothing read
    // the manifest at all — every assertion above reads the RETURN value. The
    // manifest is the only record mapping a stored name back to its original,
    // so a suffix loop that drifted from what it writes would be undetectable.
    let day_dir = std::fs::read_dir(f.source.join(TRASH_DIR_NAME))
        .expect("the trash root exists")
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("one day folder");
    let _ = &last_record;
    let manifest = read_manifest(&day_dir);
    assert_eq!(manifest.len(), 3, "one line per trashed file");
    let logged: Vec<String> = manifest
        .iter()
        .map(|line| {
            PathBuf::from(line["storedPath"].as_str().expect("storedPath"))
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert_eq!(logged, stored_names, "the manifest records what was stored");
    for line in &manifest {
        assert_eq!(
            line["originalPath"].as_str().expect("originalPath"),
            file.to_string_lossy(),
            "all three came from the same original path"
        );
    }
    // Each stored file keeps its OWN bytes — a suffix collision must never
    // overwrite the file it was avoiding.
    for (line, content) in manifest
        .iter()
        .zip([b"first" as &[u8], b"second", b"third"])
    {
        let stored = line["storedPath"].as_str().expect("storedPath");
        assert_eq!(std::fs::read(stored).unwrap(), content);
    }
}

#[test]
fn relative_paths_are_rejected() {
    let f = fixture("relative");
    assert!(trash_file(Path::new("relative.jpg"), &f.source, None, &ctx()).is_err());
}

#[test]
fn manifest_failure_leaves_the_indexed_source_authoritative() {
    let f = fixture("manifest-failure");
    let file = f.source.join("kept.jpg");
    std::fs::write(&file, b"source-bytes").unwrap();
    let day = chrono::Utc::now().format("%Y%m%d-utc").to_string();
    let manifest_path = f
        .source
        .join(TRASH_DIR_NAME)
        .join(day)
        .join("manifest.jsonl");
    std::fs::create_dir_all(&manifest_path).unwrap();

    assert!(trash_file(&file, &f.source, None, &ctx()).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"source-bytes");
}

#[cfg(unix)]
#[test]
fn volume_root_of_temp_paths_resolves_to_a_real_ancestor() {
    let dir = tempfile::tempdir().unwrap();
    let root = volume_root_of(dir.path()).unwrap();
    assert!(dir.path().starts_with(&root));
    // The REAL invariant, and the one that matters: the root is a mount point,
    // so it sits on a different device from its parent (or it is `/`). The
    // previous assertion — parent-is-none OR is-absolute — could not fail,
    // since volume_root_of starts absolute and only walks upward. Getting this
    // wrong is what makes a trash move cross devices and fail with EXDEV on an
    // SD card, which is exactly what the same-volume rename exists to avoid.
    use std::os::unix::fs::MetadataExt;
    let root_dev = std::fs::metadata(&root).unwrap().dev();
    match root.parent() {
        Some(parent) => {
            let parent_dev = std::fs::metadata(parent).unwrap().dev();
            assert_ne!(
                root_dev,
                parent_dev,
                "{} is not a mount point — its parent is on the same device",
                root.display()
            );
        }
        None => assert_eq!(root, std::path::Path::new("/")),
    }
}

#[cfg(windows)]
#[test]
fn verbatim_paths_resolve_to_their_ordinary_volume_roots() {
    assert_eq!(
        volume_root_of(Path::new(r"\\?\C:\photos\deep\image.jpg")).unwrap(),
        PathBuf::from(r"C:\")
    );
    assert_eq!(
        volume_root_of(Path::new(r"\\?\UNC\server\share\deep\image.jpg")).unwrap(),
        PathBuf::from(r"\\server\share\")
    );
}

#[test]
fn each_configured_root_owns_its_deleted_files() {
    let dir = tempfile::tempdir().unwrap();
    let bob = dir.path().join("bob/photos");
    let ann = dir.path().join("ann/photos");
    std::fs::create_dir_all(&bob).unwrap();
    std::fs::create_dir_all(&ann).unwrap();
    let bob_file = bob.join("bob.jpg");
    let ann_file = ann.join("ann.jpg");
    std::fs::write(&bob_file, b"bob").unwrap();
    std::fs::write(&ann_file, b"ann").unwrap();

    let bob_record = trash_file(&bob_file, &bob, None, &ctx()).unwrap();
    let ann_record = trash_file(&ann_file, &ann, None, &ctx()).unwrap();

    assert!(Path::new(&bob_record.stored_path).starts_with(bob.join(TRASH_DIR_NAME)));
    assert!(Path::new(&ann_record.stored_path).starts_with(ann.join(TRASH_DIR_NAME)));
    assert!(!Path::new(&bob_record.stored_path).starts_with(&ann));
    assert!(!Path::new(&ann_record.stored_path).starts_with(&bob));
}

#[test]
fn most_specific_configured_root_owns_nested_files() {
    let dir = tempfile::tempdir().unwrap();
    let outer = dir.path().join("photos");
    let inner = outer.join("private");
    std::fs::create_dir_all(&inner).unwrap();
    let file = inner.join("photo.jpg");
    std::fs::write(&file, b"photo").unwrap();

    assert_eq!(
        root_for_file(&file, &[outer, inner.clone()]).unwrap(),
        inner
    );
}

#[test]
fn a_missing_file_keeps_its_configured_owner_but_an_outside_path_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let configured = dir.path().join("photos");
    std::fs::create_dir_all(&configured).unwrap();

    assert_eq!(
        root_for_file(
            &configured.join("missing.jpg"),
            std::slice::from_ref(&configured),
        )
        .unwrap(),
        configured
    );
    assert!(root_for_file(&dir.path().join("outside.jpg"), &[configured]).is_err());
}

#[test]
fn overview_reports_sizes_and_empty_leaves_the_root_standing() {
    // The Trash surface's whole contract: sizes tell the truth on open, and
    // emptying destroys the CONTENTS while the root survives for the next
    // trash move. The root path check lives in the command layer; this is
    // the engine half.
    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-surface-")
        .tempdir()
        .unwrap();
    // Two files trashed through the real path so the day-folder layout is
    // the one the surface will meet.
    let source = dir.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    let a = source.join("one.jpg");
    let b = source.join("two.jpg");
    std::fs::write(&a, vec![1u8; 1000]).unwrap();
    std::fs::write(&b, vec![2u8; 500]).unwrap();
    trash_file(&a, &source, Some("h1"), &ctx()).unwrap();
    trash_file(&b, &source, Some("h2"), &ctx()).unwrap();

    let rows = overview(std::slice::from_ref(&source));
    let row = rows
        .iter()
        .find(|r| r.files > 0)
        .expect("a row must carry the two trashed files");
    // EXACTLY the two files and EXACTLY their bytes. The counts answer "how
    // much of my library is in here", so our own manifest.jsonl must not
    // appear in either number — when it did, two trashed photos read as
    // "3 files" and the byte total drifted by the ledger's size.
    assert_eq!(row.files, 2, "only recoverable files may be counted");
    assert_eq!(row.bytes, 1500, "only recoverable bytes may be counted");

    let snapshots = std::cell::RefCell::new(Vec::new());
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let outcome = empty_root_with_progress(
        Path::new(&row.root),
        &row.plan_token,
        &cancelled,
        &|progress| snapshots.borrow_mut().push(progress),
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(!outcome.cancelled);
    assert_eq!(outcome.failures, 0);
    let snapshots = snapshots.into_inner();
    assert_eq!(snapshots.first().unwrap().done, 0);
    assert_eq!(snapshots.first().unwrap().total, 2);
    assert_eq!(snapshots.first().unwrap().bytes_total, 1500);
    assert_eq!(snapshots.last().unwrap().done, 2);
    assert_eq!(snapshots.last().unwrap().bytes_done, 1500);
    let after = overview(std::slice::from_ref(&source));
    let same = after.iter().find(|r| r.root == row.root).unwrap();
    assert_eq!(same.files, 0, "emptied means empty");
    assert_eq!(same.bytes, 0);
    assert!(
        Path::new(&row.root).exists(),
        "the root itself survives for the next trash move"
    );
}

#[test]
fn empty_cancellation_stops_between_files_without_hiding_remaining_contents() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-empty-cancel-")
        .tempdir()
        .unwrap();
    let root = dir.path().join(TRASH_DIR_NAME);
    std::fs::create_dir_all(root.join("20260827-utc")).unwrap();
    std::fs::write(root.join("20260827-utc/one.jpg"), vec![1u8; 10]).unwrap();
    std::fs::write(root.join("20260827-utc/two.jpg"), vec![2u8; 20]).unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    let snapshots = std::cell::RefCell::new(Vec::new());

    let outcome = empty_root_with_progress(
        &root,
        &reviewed_token(&root),
        &cancelled,
        &|progress| snapshots.borrow_mut().push(progress),
        &|_, _| Ok(()),
    )
    .unwrap();

    assert!(outcome.cancelled);
    assert!(snapshots.into_inner().is_empty());
    assert!(root.join("20260827-utc/one.jpg").exists());
    assert!(root.join("20260827-utc/two.jpg").exists());
}

#[cfg(unix)]
#[test]
fn empty_stops_when_a_file_failure_cannot_be_recorded() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-empty-record-failure-")
        .tempdir()
        .unwrap();
    let root = dir.path().join(TRASH_DIR_NAME);
    let day = root.join("20260827-utc");
    let file = day.join("one.jpg");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(&file, b"keep").unwrap();
    std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o500)).unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(false);

    let token = reviewed_token(&root);
    let result = empty_root_with_progress(&root, &token, &cancelled, &|_| {}, &|path, _| {
        assert_eq!(path, file);
        Err("Issues unavailable".to_string())
    });

    std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.unwrap_err(), "Issues unavailable");
    assert!(file.exists());
}

#[cfg(unix)]
#[test]
fn empty_never_follows_a_replaced_root_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-empty-root-link-")
        .tempdir()
        .unwrap();
    let outside = dir.path().join("outside");
    let root = dir.path().join("trash");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.jpg"), b"keep").unwrap();
    symlink(&outside, &root).unwrap();

    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let error = empty_root_with_progress(&root, "reviewed", &cancelled, &|_| {}, &|_, _| Ok(())).unwrap_err();

    assert_eq!(error, "trash root is not a directory");
    assert_eq!(std::fs::read(outside.join("keep.jpg")).unwrap(), b"keep");
}

#[test]
fn overview_never_discovers_unconfigured_roots() {
    let dir = tempfile::tempdir().unwrap();
    let configured = dir.path().join("configured");
    let unrelated = dir.path().join("mounted-drive");
    std::fs::create_dir_all(configured.join(TRASH_DIR_NAME)).unwrap();
    std::fs::create_dir_all(unrelated.join(TRASH_DIR_NAME)).unwrap();

    let rows = overview(std::slice::from_ref(&configured));
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].root,
        configured.join(TRASH_DIR_NAME).to_string_lossy()
    );
}

#[test]
fn revealing_an_empty_location_creates_only_the_selected_configured_root() {
    let dir = tempfile::tempdir().unwrap();
    let configured = dir.path().join("photos");
    let other = dir.path().join("other");
    std::fs::create_dir_all(&configured).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let requested = configured.join(TRASH_DIR_NAME);

    assert_eq!(
        ensure_root_for_reveal(std::slice::from_ref(&configured), &requested).unwrap(),
        requested
    );
    assert!(requested.is_dir());
    assert!(ensure_root_for_reveal(&[configured], &other.join(TRASH_DIR_NAME)).is_err());
    assert!(!other.join(TRASH_DIR_NAME).exists());
}

/// The token the Deleted files surface would have confirmed for `trash_root`
/// (a `<configured root>/.onecopy-trash` directory).
fn reviewed_token(trash_root: &Path) -> String {
    overview(&[trash_root.parent().unwrap().to_path_buf()])
        .into_iter()
        .find(|row| Path::new(&row.root) == trash_root)
        .unwrap()
        .plan_token
}

#[test]
fn empty_removes_nothing_when_the_location_changed_after_its_totals_were_confirmed() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-empty-changed-")
        .tempdir()
        .unwrap();
    let source = dir.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    let first = source.join("reviewed.jpg");
    std::fs::write(&first, vec![1u8; 100]).unwrap();
    trash_file(&first, &source, Some("h1"), &ctx()).unwrap();
    let reviewed = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!(reviewed.files, 1);

    // A Move finishes while the confirmation shows "1 file" and trashes more.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let later = source.join("later.jpg");
    std::fs::write(&later, vec![2u8; 900]).unwrap();
    trash_file(&later, &source, Some("h2"), &ctx()).unwrap();

    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let outcome = empty_root_with_progress(
        Path::new(&reviewed.root),
        &reviewed.plan_token,
        &cancelled,
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();

    assert!(outcome.plan_changed);
    let current = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!((current.files, current.bytes), (2, 1000), "nothing was removed");

    let outcome = empty_root_with_progress(
        Path::new(&current.root),
        &current.plan_token,
        &cancelled,
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();
    assert!(!outcome.plan_changed);
    assert_eq!(overview(std::slice::from_ref(&source)).remove(0).files, 0);
}

#[cfg(unix)]
#[test]
fn a_root_configured_through_a_symlink_keeps_recoverable_deletion() {
    let f = fixture("symlinked-root");
    let link = f._dir.path().join("photos-link");
    std::os::unix::fs::symlink(&f.source, &link).unwrap();
    // The index records files under the root's resolved spelling.
    let resolved = std::fs::canonicalize(&f.source).unwrap();
    let file = resolved.join("a.jpg");
    std::fs::write(&file, b"bytes").unwrap();

    assert_eq!(root_for_file(&file, std::slice::from_ref(&link)).unwrap(), link);
    assert_eq!(
        root_for_file(&resolved.join("gone.jpg"), std::slice::from_ref(&link)).unwrap(),
        link,
        "a missing file under the resolved spelling keeps its configured owner"
    );
    let record = trash_file(&file, &link, None, &ctx()).unwrap();

    assert!(!file.exists());
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"bytes");
    assert!(f.source.join(TRASH_DIR_NAME).is_dir());
}

#[cfg(unix)]
#[test]
fn a_deleted_files_folder_that_is_not_a_real_directory_is_never_used() {
    let f = fixture("symlinked-trash");
    let elsewhere = f._dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.source.join(TRASH_DIR_NAME)).unwrap();
    let file = f.source.join("a.jpg");
    std::fs::write(&file, b"bytes").unwrap();

    assert!(trash_file(&file, &f.source, None, &ctx()).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"bytes");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

    std::fs::remove_file(f.source.join(TRASH_DIR_NAME)).unwrap();
    std::fs::write(f.source.join(TRASH_DIR_NAME), b"a file").unwrap();
    assert!(trash_file(&file, &f.source, None, &ctx()).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"bytes");
}

#[test]
fn empty_notices_a_file_added_within_the_same_mtime_granule() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-empty-granule-")
        .tempdir()
        .unwrap();
    let source = dir.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    let first = source.join("reviewed.jpg");
    std::fs::write(&first, vec![1u8; 100]).unwrap();
    let record = trash_file(&first, &source, Some("h1"), &ctx()).unwrap();
    let day = Path::new(&record.stored_path).parent().unwrap().to_path_buf();
    let reviewed = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!(reviewed.files, 1);
    let day_modified = std::fs::metadata(&day).unwrap().modified().unwrap();

    // A Delete finishes while the confirmation shows "1 file" and trashes one
    // more file into the same day folder within one mtime granule (FAT keeps
    // two seconds, HFS+ one), so the folder's mtime reads as it did.
    let later = source.join("later.jpg");
    std::fs::write(&later, vec![2u8; 900]).unwrap();
    trash_file(&later, &source, Some("h2"), &ctx()).unwrap();
    std::fs::File::open(&day)
        .unwrap()
        .set_modified(day_modified)
        .unwrap();

    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let outcome = empty_root_with_progress(
        Path::new(&reviewed.root),
        &reviewed.plan_token,
        &cancelled,
        &|_| {},
        &|_, _| Ok(()),
    )
    .unwrap();

    assert!(outcome.plan_changed, "the unreviewed file must not be emptied");
    assert!(Path::new(&record.stored_path).exists());
    let current = overview(std::slice::from_ref(&source)).remove(0);
    assert_eq!((current.files, current.bytes), (2, 1000));
}

#[test]
fn each_trash_action_is_also_a_record() {
    let f = fixture("records");
    let records = f.source.parent().unwrap().join("records.sqlite3");
    onecopy_lib::records::init(&records, "trash-records-test");
    let file = f.source.join("recorded.jpg");
    std::fs::write(&file, b"bytes").unwrap();

    let record = trash_file(&file, &f.source, Some("hash-recorded"), &ctx()).unwrap();

    let conn = rusqlite::Connection::open(&records).unwrap();
    let (session, action, operation, hash, original, stored, detail): (String, String, String, String, String, String, String) = conn
        .query_row(
            "SELECT session_id, action, operation_id, content_hash, original_path, stored_path, detail_json
             FROM trash_actions WHERE original_path = ?1",
            [file.to_string_lossy()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
        )
        .unwrap();
    assert_eq!(session, "trash-records-test");
    assert_eq!((action.as_str(), operation.as_str(), hash.as_str()), ("trashed", "test-operation", "hash-recorded"));
    assert_eq!(original, file.to_string_lossy());
    assert_eq!(stored, record.stored_path);
    let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
    assert_eq!(detail["storedName"], "recorded.jpg");
}

fn ctx() -> TrashContext {
    TrashContext::new(TrashKind::Delete, "test-operation")
}

// ---------------------------------------------------------------------------
// Manifest version 2 and its reader

fn day_of(record: &TrashedRecord) -> PathBuf {
    PathBuf::from(&record.stored_path).parent().unwrap().to_path_buf()
}

#[test]
fn a_record_round_trips_through_the_reader() {
    let f = fixture("round-trip");
    std::fs::create_dir_all(f.source.join("2016").join("spain")).unwrap();
    let file = f.source.join("2016").join("spain").join("beach.xmp");
    std::fs::write(&file, b"sidecar").unwrap();
    let context = TrashContext::new(TrashKind::MoveCleanup, "op-1")
        .item(Some("item-key".to_string()))
        .role(TrashRole::Companion)
        .moved_to(Some("/dest/beach.xmp".to_string()));

    let record = trash_file(&file, &f.source, Some("h"), &context).unwrap();
    assert_eq!(record.format_version, onecopy_lib::formats::DELETED_FILES_MANIFEST);
    assert_eq!(record.original_relative, "2016/spain/beach.xmp");
    assert_eq!(record.stored_name, "beach.xmp");
    assert_eq!(record.size, 7);

    let listing = read_day(&day_of(&record)).unwrap();
    assert_eq!(listing.records.len(), 1);
    let (read, stored) = &listing.records[0];
    assert_eq!(read.original_relative, "2016/spain/beach.xmp");
    assert_eq!(read.kind, TrashKind::MoveCleanup);
    assert_eq!(read.operation, "op-1");
    assert_eq!(read.item.as_deref(), Some("item-key"));
    assert_eq!(read.role, TrashRole::Companion);
    assert_eq!(read.moved_to.as_deref(), Some("/dest/beach.xmp"));
    assert_eq!(read.content_hash.as_deref(), Some("h"));
    assert_eq!(read.size, 7);
    assert_eq!(read.mtime_ms, record.mtime_ms);
    assert_eq!(
        *stored,
        StoredState::Regular {
            size: 7,
            mtime_ms: record.mtime_ms
        }
    );
}

/// A day folder written by hand, as an older OneCopy or a user left it.
fn hand_day(root: &Path, day: &str, lines: &[String], files: &[(&str, &[u8])]) -> PathBuf {
    let day_dir = root.join(TRASH_DIR_NAME).join(day);
    std::fs::create_dir_all(&day_dir).unwrap();
    if !lines.is_empty() {
        std::fs::write(day_dir.join("manifest.jsonl"), lines.join("\n") + "\n").unwrap();
    }
    for (name, bytes) in files {
        std::fs::write(day_dir.join(name), bytes).unwrap();
    }
    day_dir
}

fn legacy_line(original: &Path, stored: &Path) -> String {
    serde_json::json!({
        "originalPath": original.to_string_lossy(),
        "storedPath": stored.to_string_lossy(),
        "contentHash": null,
        "deletedAtUtc": "2026-09-01T10:00:00.000Z",
    })
    .to_string()
}

#[test]
fn every_line_written_carries_its_format_version() {
    let f = fixture("line-format");
    let file = f.source.join("a.jpg");
    std::fs::write(&file, b"a").unwrap();
    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    let day = day_of(&record);
    std::fs::rename(&record.stored_path, &file).unwrap();
    append_restored(&day, &record.stored_name, "a.jpg");
    let text = std::fs::read_to_string(day.join("manifest.jsonl")).unwrap();
    let lines: Vec<serde_json::Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert_eq!(line["formatVersion"], 1, "{line}");
        assert!(line.get("v").is_none(), "{line}");
    }
}

#[test]
fn a_line_without_its_marker_is_unreadable() {
    let f = fixture("unmarked-line");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("a.jpg"), b"aa").unwrap();
    let mut line: serde_json::Value = serde_json::from_str(&record_line("a.jpg", "trips/a.jpg", 2, mtime_of(&day.join("a.jpg")))).unwrap();
    line.as_object_mut().unwrap().remove("formatVersion");
    let day = hand_day(&f.source, "20260901-utc", &[line.to_string()], &[]);

    let listing = read_day(&day).unwrap();
    assert!(listing.records.is_empty(), "nothing is inferred from the shape");
    assert_eq!(listing.malformed_lines, 1);
    assert_eq!(listing.unrecorded_files, 1);
}

#[test]
fn an_unversioned_four_field_line_is_not_a_record() {
    let f = fixture("legacy");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    let line = legacy_line(&f.source.join("trips").join("a.jpg"), &day.join("a.jpg"));
    let day = hand_day(&f.source, "20260901-utc", &[line], &[("a.jpg", b"aaa")]);

    let listing = read_day(&day).unwrap();
    assert!(listing.records.is_empty());
    assert_eq!(listing.malformed_lines, 1);
    assert_eq!(listing.unrecorded_files, 1);
}

#[test]
fn lines_a_newer_onecopy_wrote_are_counted_and_left_as_they_are() {
    let f = fixture("newer-lines");
    let file = f.source.join("one.jpg");
    std::fs::write(&file, b"one").unwrap();
    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    let unrelated = f.source.join("two.jpg");
    std::fs::write(&unrelated, b"two").unwrap();
    trash_file(&unrelated, &f.source, None, &ctx()).unwrap();
    let day = day_of(&record);
    let manifest = day.join("manifest.jsonl");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str(&record_line("gone.jpg", "gone.jpg", 3, 0));
    text.push('\n');
    text.push_str("{\"formatVersion\":1,\"event\":\"restored\",\"storedName\":\"gone.jpg\",\"restoredTo\":\"gone 2.jpg\"}\n");
    text.push_str("{\"formatVersion\":2,\"storedName\":\"later.jpg\",\"somethingNew\":true}\n");
    text.push_str("{\"formatVersion\":2,\"event\":\"restored\",\"storedName\":\"one.jpg\"}\n");
    std::fs::write(&manifest, &text).unwrap();

    let listing = read_day(&day).unwrap();
    assert_eq!(listing.newer_lines, 2);
    assert_eq!(listing.malformed_lines, 0);
    assert!(listing.records.is_empty(), "no earlier row in the affected day is authoritative");
    assert_eq!(listing.restored, 0);
    assert!(listing.restored_to.is_empty());
    assert_eq!(listing.unrecorded_files, 2);
    let other_day = hand_day(&f.source, "20000101-utc", &[], &[("other.jpg", b"other")]);
    hand_day(&f.source, "20000101-utc", &[record_line("other.jpg", "other.jpg", 5, mtime_of(&other_day.join("other.jpg")))], &[]);
    let root = list_root(&f.source, &f._dir.path().join("apphome")).unwrap();
    assert_eq!(root.newer_lines, 2);
    assert_eq!(root.entries.len(), 1);
    assert_eq!(root.entries[0].day, "20000101-utc");
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), text, "never rewritten");
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"one");
    assert_eq!(std::fs::read(day.join("two.jpg")).unwrap(), b"two");
}

#[test]
fn newer_day_refuses_recoverable_moves_and_restored_manifest_appends() {
    let f = fixture("newer-append");
    let first = f.source.join("first.jpg");
    std::fs::write(&first, b"first").unwrap();
    let record = trash_file(&first, &f.source, None, &ctx()).unwrap();
    let day = day_of(&record);
    let manifest = day.join(MANIFEST_FILE_NAME);
    let mut bytes = std::fs::read(&manifest).unwrap();
    bytes.extend_from_slice(b"{\"formatVersion\":2}\n");
    std::fs::write(&manifest, &bytes).unwrap();
    let source = f.source.join("keep.jpg");
    std::fs::write(&source, b"keep").unwrap();

    let error = trash_file(&source, &f.source, None, &ctx()).unwrap_err();
    assert!(error.message.contains(day.to_string_lossy().as_ref()), "{error}");
    assert_eq!(std::fs::read(&source).unwrap(), b"keep");
    assert!(!day.join("keep.jpg").exists());
    append_restored(&day, &record.stored_name, "first.jpg");
    assert_eq!(std::fs::read(&manifest).unwrap(), bytes);
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"first");
}

#[test]
fn explicit_empty_removes_a_protected_day_without_interpreting_its_records() {
    let f = fixture("newer-empty");
    let day = hand_day(&f.source, "20000101-utc", &["{\"formatVersion\":2}".to_string()], &[("photo.jpg", b"protected")]);
    let root = f.source.join(TRASH_DIR_NAME);
    let before = overview(std::slice::from_ref(&f.source));
    assert_eq!(before[0].files, 1);
    let outcome = empty_root_with_progress(
        &root, &reviewed_token(&root), &std::sync::atomic::AtomicBool::new(false),
        &|_| {}, &|_, _| Ok(()),
    ).unwrap();

    assert_eq!(outcome.failures, 0);
    assert!(!outcome.plan_changed);
    assert!(!day.exists());
    let after = overview(std::slice::from_ref(&f.source));
    assert_eq!((after[0].bytes, after[0].files), (0, 0));
}

#[test]
fn malformed_and_torn_lines_are_skipped_and_counted_without_rewriting() {
    let f = fixture("malformed");
    let first = f.source.join("one.jpg");
    std::fs::write(&first, b"one").unwrap();
    let record = trash_file(&first, &f.source, None, &ctx()).unwrap();
    let day = day_of(&record);
    let manifest = day.join("manifest.jsonl");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("not json at all\n");
    text.push_str("{\"v\":2,\"storedName\":\"x.jpg\"}\n");
    std::fs::write(&manifest, &text).unwrap();
    let second = f.source.join("two.jpg");
    std::fs::write(&second, b"two").unwrap();
    trash_file(&second, &f.source, None, &ctx()).unwrap();
    // A crash tore the last line.
    let mut torn = std::fs::read_to_string(&manifest).unwrap();
    torn.push_str("{\"originalPath\":\"/x/three.jpg\",\"stor");
    std::fs::write(&manifest, &torn).unwrap();

    let listing = read_day(&day).unwrap();
    let names: Vec<_> = listing.records.iter().map(|(r, _)| r.stored_name.as_str()).collect();
    assert_eq!(names, ["one.jpg", "two.jpg"]);
    assert_eq!(listing.malformed_lines, 3);
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), torn, "never rewritten");
}

#[test]
fn a_line_appended_after_a_torn_one_starts_a_line_of_its_own() {
    // 37: a crash (or a write given up on) tore the last line; the records
    // and restored lines written after it are not lost to it.
    let f = fixture("torn-then-append");
    let first = f.source.join("one.jpg");
    std::fs::write(&first, b"one").unwrap();
    let record = trash_file(&first, &f.source, None, &ctx()).unwrap();
    let day = day_of(&record);
    let manifest = day.join("manifest.jsonl");
    let mut torn = std::fs::read_to_string(&manifest).unwrap();
    torn.push_str("{\"originalPath\":\"/x/three.jpg\",\"stor");
    std::fs::write(&manifest, &torn).unwrap();

    let second = f.source.join("two.jpg");
    std::fs::write(&second, b"two").unwrap();
    trash_file(&second, &f.source, None, &ctx()).unwrap();
    std::fs::rename(&record.stored_path, &first).unwrap();
    std::fs::write(&manifest, std::fs::read_to_string(&manifest).unwrap() + "{\"v\":2,\"ev").unwrap();
    append_restored(&day, &record.stored_name, "one.jpg");

    let listing = read_day(&day).unwrap();
    let names: Vec<_> = listing.records.iter().map(|(r, _)| r.stored_name.as_str()).collect();
    assert_eq!(names, ["two.jpg"], "the record after the torn line is read");
    assert_eq!(listing.restored, 1, "the restored line after the torn one is read");
    assert_eq!(listing.malformed_lines, 2, "only the torn fragments are lost");
    assert!(std::fs::read_to_string(&manifest).unwrap().starts_with(&torn), "appended, never rewritten");
}

#[test]
fn files_without_a_record_are_counted_not_listed() {
    let f = fixture("unrecorded");
    // No manifest at all: every file is unrecorded.
    let bare = hand_day(&f.source, "20260901-utc", &[], &[("a.jpg", b"a"), ("b.jpg", b"b")]);
    let listing = read_day(&bare).unwrap();
    assert!(listing.records.is_empty());
    assert_eq!(listing.unrecorded_files, 2);

    // A file moved by hand into another day folder: unrecorded there, and its
    // old record has no file, so it is not listed either.
    let file = f.source.join("moved.jpg");
    std::fs::write(&file, b"m").unwrap();
    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    std::fs::rename(&record.stored_path, bare.join("moved.jpg")).unwrap();
    let old = read_day(&day_of(&record)).unwrap();
    assert!(old.records.is_empty());
    assert_eq!(old.unrecorded_files, 0);
    let new = read_day(&bare).unwrap();
    assert_eq!(new.unrecorded_files, 3);
}

#[test]
fn a_restored_line_hides_its_record_and_leaves_the_totals_alone() {
    let f = fixture("restored-line");
    let file = f.source.join("photo.jpg");
    std::fs::write(&file, vec![1u8; 40]).unwrap();
    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    let day = day_of(&record);
    let before = overview(std::slice::from_ref(&f.source));

    std::fs::rename(&record.stored_path, &file).unwrap();
    append_restored(&day, &record.stored_name, "photo.jpg");
    let listing = read_day(&day).unwrap();
    assert!(listing.records.is_empty(), "a restored record is not listed");
    assert_eq!(listing.restored, 1);
    assert_eq!(listing.malformed_lines, 0, "the event line is understood");

    let after = overview(std::slice::from_ref(&f.source));
    assert_eq!((before[0].files, before[0].bytes), (1, 40));
    assert_eq!((after[0].files, after[0].bytes), (0, 0), "the manifest never counts");

    // Deleting the same name again after the restore: the new record wins
    // over both the old record and the restored line.
    trash_file(&file, &f.source, None, &ctx()).unwrap();
    let listing = read_day(&day).unwrap();
    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.restored, 0);
}

// ---------------------------------------------------------------------------
// Browsing one root

fn statuses(listing: &TrashListing) -> Vec<(String, EntryStatus)> {
    let mut statuses: Vec<_> = listing
        .entries
        .iter()
        .map(|entry| (entry.stored_name.clone(), entry.status))
        .collect();
    statuses.sort_by(|left, right| left.0.cmp(&right.0));
    statuses
}

fn record_line(stored: &str, relative: &str, size: u64, mtime_ms: i64) -> String {
    serde_json::json!({
        "originalPath": format!("/old/{relative}"),
        "storedPath": format!("/old/.onecopy-trash/20260901-utc/{stored}"),
        "contentHash": null,
        "deletedAtUtc": "2026-09-01T10:00:00.000Z",
        "formatVersion": 1,
        "storedName": stored,
        "originalRelative": relative,
        "kind": "delete",
        "operation": "op",
        "item": "item",
        "role": "main",
        "size": size,
        "mtimeMs": mtime_ms,
    })
    .to_string()
}

fn mtime_of(path: &Path) -> i64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn each_entry_says_whether_it_can_be_restored() {
    let f = fixture("statuses");
    let data_root = f._dir.path().join("apphome");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    std::fs::create_dir_all(&day).unwrap();
    for (name, bytes) in [
        ("ok.jpg", &b"ok"[..]),
        ("edited.jpg", b"edited later"),
        ("lossy.jpg", b"l"),
        ("into-trash.jpg", b"t"),
    ] {
        std::fs::write(day.join(name), bytes).unwrap();
    }
    std::fs::create_dir(day.join("now-a-folder.jpg")).unwrap();
    let mtime = |name: &str| mtime_of(&day.join(name));
    let lines = vec![
        record_line("ok.jpg", "a/ok.jpg", 2, mtime("ok.jpg")),
        record_line("edited.jpg", "a/edited.jpg", 3, mtime("edited.jpg")),
        record_line("now-a-folder.jpg", "a/now-a-folder.jpg", 1, 0),
        record_line("lossy.jpg", "a/\u{FFFD}.jpg", 1, mtime("lossy.jpg")),
        record_line("into-trash.jpg", ".onecopy-trash/x/into-trash.jpg", 1, mtime("into-trash.jpg")),
    ];
    std::fs::write(day.join("manifest.jsonl"), lines.join("\n") + "\n").unwrap();

    let listing = list_root(&f.source, &data_root).unwrap();
    assert_eq!(
        statuses(&listing),
        [
            ("edited.jpg".to_string(), EntryStatus::Changed),
            ("into-trash.jpg".to_string(), EntryStatus::Excluded),
            ("lossy.jpg".to_string(), EntryStatus::Unrepresentable),
            ("now-a-folder.jpg".to_string(), EntryStatus::Changed),
            ("ok.jpg".to_string(), EntryStatus::Restorable),
        ]
    );
    let ok = listing.entries.iter().find(|entry| entry.stored_name == "ok.jpg").unwrap();
    assert_eq!(ok.id, "20260901-utc/ok.jpg");
    assert_eq!(ok.group, "item:op:item", "one operation's item is one deleted item");
    assert_eq!(ok.original_relative.as_deref(), Some("a/ok.jpg"));
    assert_eq!(ok.size, 2);
}

#[test]
fn a_target_inside_the_data_folder_is_excluded() {
    let f = fixture("data-root");
    // The data folder happens to live inside this configured root.
    let data_root = f.source.join("apphome");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(day.join("index.sqlite3"), b"x").unwrap();
    let line = record_line("index.sqlite3", "apphome/index.sqlite3", 1, mtime_of(&day.join("index.sqlite3")));
    std::fs::write(day.join("manifest.jsonl"), line + "\n").unwrap();

    let listing = list_root(&f.source, &data_root).unwrap();
    assert_eq!(listing.entries[0].status, EntryStatus::Excluded);
}

#[test]
fn records_still_restore_after_the_root_moves() {
    let f = fixture("root-renamed");
    let data_root = f._dir.path().join("apphome");
    let file = f.source.join("trip").join("a.jpg");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"a").unwrap();
    trash_file(&file, &f.source, None, &ctx()).unwrap();

    let moved = f._dir.path().join("renamed-photos");
    std::fs::rename(&f.source, &moved).unwrap();
    let listing = list_root(&moved, &data_root).unwrap();
    assert_eq!(
        statuses(&listing),
        [("a.jpg".to_string(), EntryStatus::Restorable)]
    );
}

#[cfg(unix)]
#[test]
fn a_day_folder_replaced_by_a_link_is_refused_not_followed() {
    let f = fixture("day-link");
    let elsewhere = f._dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(f.source.join(TRASH_DIR_NAME)).unwrap();
    std::os::unix::fs::symlink(&elsewhere, f.source.join(TRASH_DIR_NAME).join("20260901-utc"))
        .unwrap();
    assert!(list_root(&f.source, f._dir.path()).is_err());

    let g = fixture("location-link");
    std::os::unix::fs::symlink(&elsewhere, g.source.join(TRASH_DIR_NAME)).unwrap();
    assert!(list_root(&g.source, g._dir.path()).is_err());
}

#[test]
fn a_root_with_no_location_lists_nothing_and_an_absent_root_is_unavailable() {
    let f = fixture("no-location");
    assert_eq!(list_root(&f.source, f._dir.path()).unwrap(), TrashListing::default());
    let absent = f._dir.path().join("unplugged");
    let rows = overview(&[f.source.clone(), absent]);
    assert!(rows[0].available);
    assert!(!rows[1].available, "a root that is not there cannot be browsed");
}

// ---------------------------------------------------------------------------
// Windows: junctions, the hidden attribute and name components
//
// Every Windows-only step goes through a command (`mklink /J`, `attrib`,
// PowerShell) so these tests need nothing from `std::os::windows`. A
// junction needs no privilege, so the link-safety guarantees the Unix tests
// above prove with symlinks are proven here with junctions.

#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mklink /J {} {} failed: {}{}",
        link.display(),
        target.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The file attributes Windows reports for `path`.
#[cfg(windows)]
fn windows_attributes(path: &Path) -> u32 {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[int][System.IO.File]::GetAttributes($env:ONECOPY_TEST_PATH)",
        ])
        .env("ONECOPY_TEST_PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "attributes of {} could not be read: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().parse().unwrap()
}

#[cfg(windows)]
#[test]
fn empty_never_follows_a_root_replaced_by_a_junction() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-trash-empty-root-junction-")
        .tempdir()
        .unwrap();
    let outside = dir.path().join("outside");
    let root = dir.path().join("trash");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.jpg"), b"keep").unwrap();
    junction(&root, &outside);

    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let error = empty_root_with_progress(&root, "reviewed", &cancelled, &|_| {}, &|_, _| Ok(()))
        .unwrap_err();

    assert_eq!(error, "trash root is not a directory");
    assert_eq!(std::fs::read(outside.join("keep.jpg")).unwrap(), b"keep");
}

#[cfg(windows)]
#[test]
fn a_root_configured_through_a_junction_keeps_recoverable_deletion() {
    let f = fixture("junction-root");
    let link = f._dir.path().join("photos-link");
    junction(&link, &f.source);
    // The index records files under the root's resolved spelling.
    let resolved = std::fs::canonicalize(&f.source).unwrap();
    let file = resolved.join("a.jpg");
    std::fs::write(&file, b"bytes").unwrap();

    assert_eq!(root_for_file(&file, std::slice::from_ref(&link)).unwrap(), link);
    assert_eq!(
        root_for_file(&resolved.join("gone.jpg"), std::slice::from_ref(&link)).unwrap(),
        link,
        "a missing file under the resolved spelling keeps its configured owner"
    );
    let record = trash_file(&file, &link, None, &ctx()).unwrap();

    assert!(!file.exists());
    assert_eq!(std::fs::read(&record.stored_path).unwrap(), b"bytes");
    assert!(f.source.join(TRASH_DIR_NAME).is_dir());
}

#[cfg(windows)]
#[test]
fn a_deleted_files_folder_that_is_a_junction_is_never_used() {
    let f = fixture("junction-trash");
    let elsewhere = f._dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    junction(&f.source.join(TRASH_DIR_NAME), &elsewhere);
    let file = f.source.join("a.jpg");
    std::fs::write(&file, b"bytes").unwrap();

    assert!(trash_file(&file, &f.source, None, &ctx()).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"bytes");
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);

    // Removing a junction removes the link, never what it points at.
    std::fs::remove_dir(f.source.join(TRASH_DIR_NAME)).unwrap();
    std::fs::write(f.source.join(TRASH_DIR_NAME), b"a file").unwrap();
    assert!(trash_file(&file, &f.source, None, &ctx()).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"bytes");
}

#[cfg(windows)]
#[test]
fn a_day_folder_replaced_by_a_junction_is_refused_not_followed() {
    let f = fixture("day-junction");
    let elsewhere = f._dir.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(f.source.join(TRASH_DIR_NAME)).unwrap();
    junction(&f.source.join(TRASH_DIR_NAME).join("20260901-utc"), &elsewhere);
    assert!(list_root(&f.source, f._dir.path()).is_err());

    let g = fixture("location-junction");
    junction(&g.source.join(TRASH_DIR_NAME), &elsewhere);
    assert!(list_root(&g.source, g._dir.path()).is_err());
}

#[cfg(windows)]
#[test]
fn the_deleted_files_folder_is_hidden_once_when_it_is_created() {
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    let f = fixture("hidden-location");
    let first = f.source.join("first.jpg");
    std::fs::write(&first, b"first").unwrap();
    trash_file(&first, &f.source, None, &ctx()).unwrap();
    let location = f.source.join(TRASH_DIR_NAME);
    assert_ne!(
        windows_attributes(&location) & FILE_ATTRIBUTE_HIDDEN,
        0,
        "a new deleted-files folder is hidden"
    );

    // Hidden once, when created, never per trashed file: a folder the user
    // chose to show stays shown.
    let shown = std::process::Command::new("attrib")
        .arg("-H")
        .arg(&location)
        .output()
        .unwrap();
    assert!(shown.status.success(), "{}", String::from_utf8_lossy(&shown.stdout));
    assert_eq!(windows_attributes(&location) & FILE_ATTRIBUTE_HIDDEN, 0);
    let second = f.source.join("second.jpg");
    std::fs::write(&second, b"second").unwrap();
    trash_file(&second, &f.source, None, &ctx()).unwrap();
    assert_eq!(
        windows_attributes(&location) & FILE_ATTRIBUTE_HIDDEN,
        0,
        "a later deletion does not hide the folder again"
    );
}

#[cfg(windows)]
#[test]
fn a_component_windows_reads_as_a_separator_or_a_drive_is_unrepresentable() {
    for relative in [r"trips\a.jpg", "trips/c:a.jpg", "C:/a.jpg", "trips/a.jpg:stream"] {
        assert_eq!(relative_components(relative), Err(UnfitPath::Unrepresentable), "{relative}");
        assert_eq!(
            target_in_root(Path::new(r"C:\photos"), relative),
            Err(UnfitPath::Unrepresentable),
            "{relative}"
        );
    }
    assert_eq!(relative_components("trips/a.jpg").unwrap(), ["trips", "a.jpg"]);

    // A recorded path holding such a component is listed but never placed.
    let f = fixture("windows-components");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    std::fs::create_dir_all(&day).unwrap();
    for name in ["backslash.jpg", "colon.jpg", "ok.jpg"] {
        std::fs::write(day.join(name), b"x").unwrap();
    }
    let lines = [
        record_line("backslash.jpg", r"trips\backslash.jpg", 1, mtime_of(&day.join("backslash.jpg"))),
        record_line("colon.jpg", "trips/c:colon.jpg", 1, mtime_of(&day.join("colon.jpg"))),
        record_line("ok.jpg", "trips/ok.jpg", 1, mtime_of(&day.join("ok.jpg"))),
    ];
    std::fs::write(day.join("manifest.jsonl"), lines.join("\n") + "\n").unwrap();

    let listing = list_root(&f.source, &f._dir.path().join("apphome")).unwrap();
    assert_eq!(
        statuses(&listing),
        [
            ("backslash.jpg".to_string(), EntryStatus::Unrepresentable),
            ("colon.jpg".to_string(), EntryStatus::Unrepresentable),
            ("ok.jpg".to_string(), EntryStatus::Restorable),
        ]
    );
}
