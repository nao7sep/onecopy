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

    let record = trash_file(&file, &f.source, Some("hash123")).unwrap();
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

    let record = trash_file(&file, &f.source, None).unwrap();
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
        let record = trash_file(&file, &f.source, None).unwrap();
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
    assert!(trash_file(Path::new("relative.jpg"), &f.source, None).is_err());
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

    assert!(trash_file(&file, &f.source, None).is_err());
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

    let bob_record = trash_file(&bob_file, &bob, None).unwrap();
    let ann_record = trash_file(&ann_file, &ann, None).unwrap();

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
    trash_file(&a, &source, Some("h1")).unwrap();
    trash_file(&b, &source, Some("h2")).unwrap();

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
    let root = dir.path().join("trash");
    std::fs::create_dir_all(root.join("20260827-utc")).unwrap();
    std::fs::write(root.join("20260827-utc/one.jpg"), vec![1u8; 10]).unwrap();
    std::fs::write(root.join("20260827-utc/two.jpg"), vec![2u8; 20]).unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    let snapshots = std::cell::RefCell::new(Vec::new());

    let outcome = empty_root_with_progress(
        &root,
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
    let root = dir.path().join("trash");
    let day = root.join("20260827-utc");
    let file = day.join("one.jpg");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::write(&file, b"keep").unwrap();
    std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o500)).unwrap();
    let cancelled = std::sync::atomic::AtomicBool::new(false);

    let result = empty_root_with_progress(&root, &cancelled, &|_| {}, &|path, _| {
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
    let error = empty_root_with_progress(&root, &cancelled, &|_| {}, &|_, _| Ok(())).unwrap_err();

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
