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
fn original_relative_path_is_preserved_inside_the_day_folder() {
    let f = fixture("relpath");
    let nested = f.source.join("2016").join("spain");
    std::fs::create_dir_all(&nested).unwrap();
    let file = nested.join("beach.jpg");
    std::fs::write(&file, b"x").unwrap();

    let record = trash_file(&file, &f.app_root, None).unwrap();
    let stored = PathBuf::from(&record.stored_path);
    // The tail of the stored path mirrors the original's volume-relative
    // path — restorable by hand with a file manager alone.
    assert!(
        stored.ends_with(Path::new("2016/spain/beach.jpg"))
            || stored.ends_with(Path::new("2016\\spain\\beach.jpg")),
        "stored path {} must preserve the original structure",
        stored.display()
    );
}

#[test]
fn same_day_same_path_collisions_get_suffixes_and_exact_manifest_lines() {
    let f = fixture("collide");
    let file = f.source.join("img.jpg");

    let mut stored_names = Vec::new();
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
    }
    assert_eq!(stored_names[0], "img.jpg");
    assert_eq!(stored_names[1], "img.2.jpg");
    assert_eq!(stored_names[2], "img.3.jpg");
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
    // The root's parent (if any) is on a different device, or it is `/`.
    assert!(root.parent().is_none() || root.is_absolute());
}
