// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use std::path::{Path, PathBuf};
use onecopy_lib::trash::*;

// These tests run entirely under the temp dir, which lives on the home
// volume — so trash_root_for routes into the app-root trash and the
// whole flow stays same-volume, exactly the production shape.

struct Fixture {
    _dir: tempfile::TempDir,
    app_root: PathBuf,
    source: PathBuf,
}

fn fixture(label: &str) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-trash-{label}-"))
        .tempdir()
        .unwrap();
    let app_root = dir.path().join("apphome");
    let source = dir.path().join("photos");
    std::fs::create_dir_all(&app_root).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    Fixture {
        _dir: dir,
        app_root,
        source,
    }
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

    let record = trash_file(&file, &f.app_root, Some("hash123")).unwrap();
    assert!(!file.exists(), "the original must be gone");
    let stored = PathBuf::from(&record.stored_path);
    assert!(stored.exists(), "the stored file must exist");
    assert_eq!(std::fs::read(&stored).unwrap(), b"bytes");

    // Same-volume trash for home-volume files lives under the app root.
    assert!(stored.starts_with(f.app_root.join("trash")));

    // The day folder is self-contained: manifest sits inside it.
    let day_dir = stored
        .ancestors()
        .find(|a| a.parent().is_some_and(|p| p.ends_with("trash")))
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

    let record = trash_file(&file, &f.app_root, None).unwrap();
    let stored = PathBuf::from(&record.stored_path);

    assert_eq!(
        stored.file_name().unwrap(),
        std::ffi::OsStr::new("beach.jpg"),
        "the stored name is the original file name"
    );
    let day_dir = stored.parent().expect("stored inside a day folder");
    assert!(
        day_dir.file_name().unwrap().to_string_lossy().ends_with("-utc"),
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
        let record = trash_file(&file, &f.app_root, None).unwrap();
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
    // The day folder is <app_root>/trash/<yyyymmdd-utc>/; the stored path's own
    // parent is the deepest PRESERVED source directory, not the day folder,
    // because the original relative path is kept underneath it.
    let day_dir = std::fs::read_dir(f.app_root.join("trash"))
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
    assert!(trash_file(Path::new("relative.jpg"), &f.app_root, None).is_err());
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
                root_dev, parent_dev,
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
fn external_volume_files_trash_into_a_dot_onecopy_trash_at_their_volume_root() {
    // The whole point of the per-volume trash: the move must stay a rename on
    // the SAME volume. Routing an external drive's files to the app root would
    // make every delete a cross-device copy — slow, space-consuming, and
    // EXDEV-failing on some filesystems. Every other test in this file runs on
    // the home volume, so this branch never executed.
    let dir = tempfile::tempdir().unwrap();
    let app_root = dir.path().join("apphome");
    let external = dir.path().join("Volumes").join("SD_CARD");
    std::fs::create_dir_all(&app_root).unwrap();
    std::fs::create_dir_all(&external).unwrap();

    let root = trash_root_for(&external, &app_root).unwrap();

    assert_eq!(
        root,
        external.join(onecopy_lib::trash::TRASH_DIR_NAME),
        "an external volume trashes at its OWN root"
    );
    assert!(
        root.starts_with(&external),
        "the trash must stay on the same volume as the file"
    );
    assert!(
        !root.starts_with(&app_root),
        "an external volume must never route through the app root"
    );
}

#[test]
fn home_volume_files_trash_into_the_app_root() {
    // The complement, and the reason the branch exists: macOS forbids creating
    // /.onecopy-trash, so the home volume's files go under the app root.
    let dir = tempfile::tempdir().unwrap();
    let app_root = dir.path().join("apphome");
    std::fs::create_dir_all(&app_root).unwrap();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .expect("the platform home variable is set");
    let home_volume = volume_root_of(&std::path::PathBuf::from(home)).unwrap();

    let root = trash_root_for(&home_volume, &app_root).unwrap();

    assert_eq!(root, app_root.join("trash"));
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
    let app_root = dir.path().join("apphome");
    std::fs::create_dir_all(&app_root).unwrap();

    // Two files trashed through the real path so the day-folder layout is
    // the one the surface will meet.
    let source = dir.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    let a = source.join("one.jpg");
    let b = source.join("two.jpg");
    std::fs::write(&a, vec![1u8; 1000]).unwrap();
    std::fs::write(&b, vec![2u8; 500]).unwrap();
    trash_file(&a, &app_root, Some("h1")).unwrap();
    trash_file(&b, &app_root, Some("h2")).unwrap();

    let rows = overview(&[source.to_string_lossy().to_string()], &app_root);
    // The temp dir lives on the home volume, so the source's volume trash IS
    // the app-home trash — one deduplicated row.
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

    empty_root(Path::new(&row.root)).unwrap();
    let after = overview(&[source.to_string_lossy().to_string()], &app_root);
    let same = after.iter().find(|r| r.root == row.root).unwrap();
    assert_eq!(same.files, 0, "emptied means empty");
    assert_eq!(same.bytes, 0);
    assert!(
        Path::new(&row.root).exists(),
        "the root itself survives for the next trash move"
    );
}

#[test]
fn mounted_volume_discovery_finds_only_real_trash_dirs() {
    // The pure seam behind "every attached drive appears in the overview":
    // a volume WITH a trash is found, one without is not, and a symlinked
    // volume entry (macOS keeps one for the boot volume) is skipped so the
    // home volume can never appear twice.
    let f = fixture("discover");
    let volumes = f.app_root.join("volumes");
    std::fs::create_dir_all(volumes.join("DriveA/.onecopy-trash/20260101-utc")).unwrap();
    std::fs::write(
        volumes.join("DriveA/.onecopy-trash/20260101-utc/img.jpg"),
        b"bytes",
    )
    .unwrap();
    std::fs::create_dir_all(volumes.join("DriveB")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(volumes.join("DriveA"), volumes.join("BootAlias")).unwrap();

    let found = discover_in_volumes_dir(&volumes);
    assert_eq!(found, vec![volumes.join("DriveA/.onecopy-trash")]);

    // A discovered root is emptiable exactly like a configured one: day
    // folders go, the root itself survives for the next trash move.
    empty_root(&found[0]).unwrap();
    assert!(found[0].exists(), "the root survives emptying");
    assert_eq!(
        std::fs::read_dir(&found[0]).unwrap().count(),
        0,
        "its day folders are gone"
    );
}
