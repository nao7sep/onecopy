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

fn ctx() -> TrashContext {
    TrashContext::new(TrashKind::Delete, "test-operation")
}

// ---------------------------------------------------------------------------
// Manifest version 2 and its reader

fn day_of(record: &TrashedRecord) -> PathBuf {
    PathBuf::from(&record.stored_path).parent().unwrap().to_path_buf()
}

#[test]
fn a_version_2_record_round_trips_through_the_reader() {
    let f = fixture("v2-round-trip");
    std::fs::create_dir_all(f.source.join("2016").join("spain")).unwrap();
    let file = f.source.join("2016").join("spain").join("beach.xmp");
    std::fs::write(&file, b"sidecar").unwrap();
    let context = TrashContext::new(TrashKind::MoveCleanup, "op-1")
        .item(Some("item-key".to_string()))
        .role(TrashRole::Companion)
        .moved_to(Some("/dest/beach.xmp".to_string()));

    let record = trash_file(&file, &f.source, Some("h"), &context).unwrap();
    assert_eq!(record.v, 2);
    assert_eq!(record.original_relative, "2016/spain/beach.xmp");
    assert_eq!(record.stored_name, "beach.xmp");
    assert_eq!(record.size, 7);

    let listing = read_day(&day_of(&record), &root_spellings(&f.source)).unwrap();
    assert_eq!(listing.records.len(), 1);
    let (read, stored) = &listing.records[0];
    assert_eq!(read.version, 2);
    assert_eq!(read.original_relative.as_deref(), Some("2016/spain/beach.xmp"));
    assert_eq!(read.kind, Some(TrashKind::MoveCleanup));
    assert_eq!(read.operation.as_deref(), Some("op-1"));
    assert_eq!(read.item.as_deref(), Some("item-key"));
    assert_eq!(read.role, Some(TrashRole::Companion));
    assert_eq!(read.moved_to.as_deref(), Some("/dest/beach.xmp"));
    assert_eq!(read.content_hash.as_deref(), Some("h"));
    assert_eq!(read.size, Some(7));
    assert_eq!(read.mtime_ms, Some(record.mtime_ms));
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
fn an_original_four_field_line_still_reads() {
    let f = fixture("legacy");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    let line = legacy_line(&f.source.join("trips").join("a.jpg"), &day.join("a.jpg"));
    let day = hand_day(&f.source, "20260901-utc", &[line], &[("a.jpg", b"aaa")]);

    let listing = read_day(&day, &root_spellings(&f.source)).unwrap();
    assert_eq!(listing.records.len(), 1);
    let (record, _) = &listing.records[0];
    assert_eq!(record.version, 1);
    assert_eq!(record.stored_name, "a.jpg");
    assert_eq!(record.original_relative.as_deref(), Some("trips/a.jpg"));
    assert_eq!(record.size, None, "an original line cannot be verified");
    assert_eq!(record.kind, None);
}

#[test]
fn original_lines_outside_the_root_or_from_older_layouts_do_not_resolve_here() {
    let f = fixture("legacy-outside");
    let day = f.source.join(TRASH_DIR_NAME).join("20260901-utc");
    let lines = vec![
        // The root was renamed, or the drive came from Windows: the absolute
        // path no longer starts with any spelling of the current root.
        legacy_line(Path::new(r"C:\Photos\old.jpg"), &day.join("old.jpg")),
        // The nested layout of August 2026: the stored path is not a flat
        // entry of this day folder, so it is never resolved to one.
        legacy_line(
            &f.source.join("nested.jpg"),
            &day.join("Users").join("me").join("nested.jpg"),
        ),
        // A stored path into another day folder, copied by hand.
        legacy_line(
            &f.source.join("elsewhere.jpg"),
            &f.source.join(TRASH_DIR_NAME).join("20260902-utc").join("elsewhere.jpg"),
        ),
    ];
    let day = hand_day(
        &f.source,
        "20260901-utc",
        &lines,
        &[("old.jpg", b"o"), ("nested.jpg", b"n"), ("elsewhere.jpg", b"e")],
    );

    let listing = read_day(&day, &root_spellings(&f.source)).unwrap();
    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.records[0].0.stored_name, "old.jpg");
    assert_eq!(listing.records[0].0.original_relative, None, "outside this root");
    assert_eq!(listing.malformed_lines, 2, "the other two describe nothing here");
    assert_eq!(listing.unrecorded_files, 2);
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

    let listing = read_day(&day, &root_spellings(&f.source)).unwrap();
    let names: Vec<_> = listing.records.iter().map(|(r, _)| r.stored_name.as_str()).collect();
    assert_eq!(names, ["one.jpg", "two.jpg"]);
    assert_eq!(listing.malformed_lines, 3);
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), torn, "never rewritten");
}

#[test]
fn files_without_a_record_are_counted_not_listed() {
    let f = fixture("unrecorded");
    // No manifest at all: every file is unrecorded.
    let bare = hand_day(&f.source, "20260901-utc", &[], &[("a.jpg", b"a"), ("b.jpg", b"b")]);
    let listing = read_day(&bare, &root_spellings(&f.source)).unwrap();
    assert!(listing.records.is_empty());
    assert_eq!(listing.unrecorded_files, 2);

    // A file moved by hand into another day folder: unrecorded there, and its
    // old record has no file, so it is not listed either.
    let file = f.source.join("moved.jpg");
    std::fs::write(&file, b"m").unwrap();
    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    std::fs::rename(&record.stored_path, bare.join("moved.jpg")).unwrap();
    let old = read_day(&day_of(&record), &root_spellings(&f.source)).unwrap();
    assert!(old.records.is_empty());
    assert_eq!(old.unrecorded_files, 0);
    let new = read_day(&bare, &root_spellings(&f.source)).unwrap();
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
    let listing = read_day(&day, &root_spellings(&f.source)).unwrap();
    assert!(listing.records.is_empty(), "a restored record is not listed");
    assert_eq!(listing.restored, 1);
    assert_eq!(listing.malformed_lines, 0, "the event line is understood");

    let after = overview(std::slice::from_ref(&f.source));
    assert_eq!((before[0].files, before[0].bytes), (1, 40));
    assert_eq!((after[0].files, after[0].bytes), (0, 0), "the manifest never counts");

    // Deleting the same name again after the restore: the new record wins
    // over both the old record and the restored line.
    trash_file(&file, &f.source, None, &ctx()).unwrap();
    let listing = read_day(&day, &root_spellings(&f.source)).unwrap();
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

fn v2_line(stored: &str, relative: &str, size: u64, mtime_ms: i64) -> String {
    serde_json::json!({
        "originalPath": format!("/old/{relative}"),
        "storedPath": format!("/old/.onecopy-trash/20260901-utc/{stored}"),
        "contentHash": null,
        "deletedAtUtc": "2026-09-01T10:00:00.000Z",
        "v": 2,
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
        ("legacy.jpg", b"legacy"),
        ("outside.jpg", b"o"),
    ] {
        std::fs::write(day.join(name), bytes).unwrap();
    }
    std::fs::create_dir(day.join("now-a-folder.jpg")).unwrap();
    let mtime = |name: &str| mtime_of(&day.join(name));
    let lines = vec![
        v2_line("ok.jpg", "a/ok.jpg", 2, mtime("ok.jpg")),
        v2_line("edited.jpg", "a/edited.jpg", 3, mtime("edited.jpg")),
        v2_line("now-a-folder.jpg", "a/now-a-folder.jpg", 1, 0),
        v2_line("lossy.jpg", "a/\u{FFFD}.jpg", 1, mtime("lossy.jpg")),
        v2_line("into-trash.jpg", ".onecopy-trash/x/into-trash.jpg", 1, mtime("into-trash.jpg")),
        legacy_line(&f.source.join("b").join("legacy.jpg"), &day.join("legacy.jpg")),
        legacy_line(Path::new("/somewhere/else/outside.jpg"), &day.join("outside.jpg")),
    ];
    std::fs::write(day.join("manifest.jsonl"), lines.join("\n") + "\n").unwrap();

    let listing = list_root(&f.source, &data_root).unwrap();
    assert_eq!(
        statuses(&listing),
        [
            ("edited.jpg".to_string(), EntryStatus::Changed),
            ("into-trash.jpg".to_string(), EntryStatus::Excluded),
            ("legacy.jpg".to_string(), EntryStatus::Unverified),
            ("lossy.jpg".to_string(), EntryStatus::Unrepresentable),
            ("now-a-folder.jpg".to_string(), EntryStatus::Changed),
            ("ok.jpg".to_string(), EntryStatus::Restorable),
            ("outside.jpg".to_string(), EntryStatus::OutsideRoot),
        ]
    );
    let ok = listing.entries.iter().find(|entry| entry.stored_name == "ok.jpg").unwrap();
    assert_eq!(ok.id, "20260901-utc/ok.jpg");
    assert_eq!(ok.group, "item:op:item", "one operation's item is one deleted item");
    let legacy = listing.entries.iter().find(|entry| entry.stored_name == "legacy.jpg").unwrap();
    assert_eq!(legacy.group, "day:20260901-utc:b/legacy", "older records group by folder and stem");
    assert_eq!(legacy.role, Some(TrashRole::Main));
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
    let line = v2_line("index.sqlite3", "apphome/index.sqlite3", 1, mtime_of(&day.join("index.sqlite3")));
    std::fs::write(day.join("manifest.jsonl"), line + "\n").unwrap();

    let listing = list_root(&f.source, &data_root).unwrap();
    assert_eq!(listing.entries[0].status, EntryStatus::Excluded);
}

#[test]
fn version_2_records_still_restore_after_the_root_moves_and_older_ones_do_not() {
    let f = fixture("root-renamed");
    let data_root = f._dir.path().join("apphome");
    let file = f.source.join("trip").join("a.jpg");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"a").unwrap();
    let record = trash_file(&file, &f.source, None, &ctx()).unwrap();
    let day_name = day_of(&record).file_name().unwrap().to_string_lossy().into_owned();
    let legacy_day = f.source.join(TRASH_DIR_NAME).join(&day_name);
    std::fs::write(legacy_day.join("old.jpg"), b"o").unwrap();
    let mut manifest = std::fs::read_to_string(legacy_day.join("manifest.jsonl")).unwrap();
    manifest.push_str(&legacy_line(&f.source.join("old.jpg"), &legacy_day.join("old.jpg")));
    manifest.push('\n');
    std::fs::write(legacy_day.join("manifest.jsonl"), manifest).unwrap();

    let moved = f._dir.path().join("renamed-photos");
    std::fs::rename(&f.source, &moved).unwrap();
    let listing = list_root(&moved, &data_root).unwrap();
    assert_eq!(
        statuses(&listing),
        [
            ("a.jpg".to_string(), EntryStatus::Restorable),
            ("old.jpg".to_string(), EntryStatus::OutsideRoot),
        ]
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
