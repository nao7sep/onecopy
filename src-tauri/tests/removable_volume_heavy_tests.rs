// Copy, Move and recoverable deletion on the removable-drive filesystems
// people actually use: exFAT (no exclusive rename on macOS) and FAT32 (file
// numbers that follow the data, so an empty file's number changes when it is
// written or renamed). Each test builds a throwaway disk image, mounts it
// privately, and detaches it afterwards.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

use onecopy_lib::operations::*;
use onecopy_lib::preview::CachePaths;
use onecopy_lib::{extensions, index_store, scanner};

struct MountedImage {
    _dir: tempfile::TempDir,
    volume: PathBuf,
}

impl MountedImage {
    fn create(filesystem: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-volume-")
            .tempdir()
            .unwrap();
        let image = dir.path().join("volume.dmg");
        let mounts = dir.path().join("mnt");
        std::fs::create_dir_all(&mounts).unwrap();
        let created = Command::new("hdiutil")
            .args(["create", "-quiet", "-size", "64m", "-fs", filesystem, "-volname", "OCVOL", "-o"])
            .arg(&image)
            .status()
            .unwrap();
        assert!(created.success(), "hdiutil create {filesystem}");
        let attached = Command::new("hdiutil")
            .args(["attach", "-quiet", "-nobrowse", "-mountroot"])
            .arg(&mounts)
            .arg(&image)
            .status()
            .unwrap();
        assert!(attached.success(), "hdiutil attach {filesystem}");
        Self {
            volume: mounts.join("OCVOL"),
            _dir: dir,
        }
    }
}

impl Drop for MountedImage {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet", "-force"])
            .arg(&self.volume)
            .status();
    }
}

fn lists() -> scanner::ScanLists {
    let owned = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    scanner::ScanLists {
        images: owned(extensions::IMAGE_EXTENSIONS),
        videos: owned(extensions::VIDEO_EXTENSIONS),
        audio: owned(extensions::AUDIO_EXTENSIONS),
        companions: owned(extensions::COMPANION_EXTENSIONS),
    }
}

fn items(conn: &rusqlite::Connection, dir: &Path) -> Vec<ItemIdentity> {
    let mut statement = conn
        .prepare(
            "SELECT content_hash, id FROM paths WHERE dir_path = ?1 AND companion_of IS NULL \
             ORDER BY file_name",
        )
        .unwrap();
    statement
        .query_map([dir.to_string_lossy()], |row| {
            Ok(match row.get::<_, Option<String>>(0)? {
                Some(hash) => ItemIdentity { hash: Some(hash), path_id: None },
                None => ItemIdentity { hash: None, path_id: Some(row.get(1)?) },
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn xattr_call(path: &Path, name: &str, value: Option<&[u8]>) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    let name = std::ffi::CString::new(name).unwrap();
    if let Some(value) = value {
        let status = unsafe {
            libc::setxattr(path.as_ptr(), name.as_ptr(), value.as_ptr().cast(), value.len(), 0, 0)
        };
        assert_eq!(status, 0, "{}", std::io::Error::last_os_error());
        return None;
    }
    let mut read = vec![0u8; 4096];
    let size = unsafe {
        libc::getxattr(path.as_ptr(), name.as_ptr(), read.as_mut_ptr().cast(), read.len(), 0, 0)
    };
    (size >= 0).then(|| {
        read.truncate(size as usize);
        read
    })
}

fn private_leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".onecopy-") && name != ".onecopy-trash")
        .collect()
}

fn copy_move_and_delete_on(filesystem: &str) {
    let image = MountedImage::create(filesystem);
    let home = tempfile::tempdir().unwrap();
    let local = home.path().join("local");
    let on_volume = image.volume.join("source");
    let dest = image.volume.join("dest");
    for dir in [&local, &on_volume, &dest] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(local.join("photo.jpg"), vec![7u8; 200_000]).unwrap();
    // An even second, which FAT can hold exactly.
    let dated = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    std::fs::File::options()
        .write(true)
        .open(local.join("photo.jpg"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(dated))
        .unwrap();
    xattr_call(&local.join("photo.jpg"), "com.example.onecopy-test", Some(b"local only"));
    // Dated 1969, which no FAT-family volume holds; FAT32 on macOS would
    // store it as 2105.
    std::fs::write(local.join("old.jpg"), vec![9u8; 1000]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(local.join("old.jpg"))
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(
            std::time::UNIX_EPOCH - std::time::Duration::from_secs(365 * 86_400),
        ))
        .unwrap();
    std::fs::write(local.join("empty.bin"), b"").unwrap();
    std::fs::write(on_volume.join("moved.jpg"), vec![3u8; 150_000]).unwrap();
    std::fs::write(on_volume.join("deleted.jpg"), vec![5u8; 120_000]).unwrap();
    // Files written here carry extended attributes, which macOS keeps in
    // AppleDouble `._` companions on these filesystems and moves along with
    // each file. They are not this test's subject.
    for entry in std::fs::read_dir(&on_volume).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().unwrap().to_string_lossy().starts_with("._") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let app_root = home.path().join("app");
    std::fs::create_dir_all(&app_root).unwrap();
    std::fs::write(
        app_root.join("config.json"),
        serde_json::to_vec(&serde_json::json!({
            "formatVersion": 1,
            "sourceDirs": [local.to_string_lossy(), on_volume.to_string_lossy()],
            "destinationRoots": [image.volume.to_string_lossy()],
        }))
        .unwrap(),
    )
    .unwrap();
    let conn = index_store::open(&home.path().join("index.sqlite3")).unwrap();
    let cache = CachePaths::new(home.path().join("cache"));
    for root in [&local, &on_volume] {
        let settled = std::fs::canonicalize(root).unwrap();
        scanner::walk_root(&conn, &settled, &lists()).unwrap();
    }
    scanner::hash_pending(&conn, &cache).unwrap();
    let local = std::fs::canonicalize(&local).unwrap();
    let on_volume = std::fs::canonicalize(&on_volume).unwrap();

    // Copy onto the volume: a written file and an empty one.
    let copied = move_batch(
        &conn, &app_root, &cache, &items(&conn, &local), &dest,
        MoveOutMode::CopyKeepAll, &|| false, |_| {},
    )
    .unwrap();
    assert_eq!(copied.error, None, "{filesystem}");
    assert_eq!(copied.exported, 3, "{filesystem}: {copied:?}");
    assert_eq!(std::fs::read(dest.join("photo.jpg")).unwrap(), vec![7u8; 200_000]);
    // The volume keeps the modified time; the attribute it cannot hold itself
    // is dropped rather than written to a `._` file.
    let output = dest.join("photo.jpg");
    assert_eq!(std::fs::metadata(&output).unwrap().modified().unwrap(), dated, "{filesystem}");
    // A time the volume cannot hold becomes the earliest it can, not the
    // unrelated one FAT32 would wrap it to.
    assert_eq!(
        std::fs::metadata(dest.join("old.jpg")).unwrap().modified().unwrap(),
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(315_619_200),
        "{filesystem}"
    );
    assert_eq!(xattr_call(&output, "com.example.onecopy-test", None), None, "{filesystem}");
    assert_eq!(std::fs::read(dest.join("empty.bin")).unwrap(), b"");
    assert!(private_leftovers(&dest).is_empty(), "{filesystem}: {:?}", private_leftovers(&dest));

    // Move within the volume: publish there and keep the source recoverable.
    let moved_hash: String = conn
        .query_row(
            "SELECT content_hash FROM paths WHERE file_name = 'moved.jpg'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let moved = vec![ItemIdentity { hash: Some(moved_hash), path_id: None }];
    let outcome = move_batch(
        &conn, &app_root, &cache, &moved, &dest,
        MoveOutMode::MoveTrashRest, &|| false, |_| {},
    )
    .unwrap();
    assert_eq!(outcome.error, None, "{filesystem}");
    assert_eq!(outcome.exported, 1, "{filesystem}: {outcome:?}");
    assert_eq!(outcome.post_action.deleted_files, 1, "{filesystem}: {outcome:?}");
    assert!(!on_volume.join("moved.jpg").exists());
    assert!(dest.join("moved.jpg").exists());

    // Recoverable deletion from a source on the volume.
    let deleted = delete_batch(
        &conn, &app_root, &cache, &items(&conn, &on_volume), DeleteMode::Trash,
        &|| false, |_| {},
    )
    .unwrap();
    assert_eq!(deleted.error, None, "{filesystem}");
    assert_eq!((deleted.deleted_files, deleted.failed_files), (1, 0), "{filesystem}");
    let trash = on_volume.join(onecopy_lib::trash::TRASH_DIR_NAME);
    let stored = walkdir::WalkDir::new(&trash)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() == "deleted.jpg" || entry.file_name() == "moved.jpg")
        .count();
    assert_eq!(stored, 2, "{filesystem}: both sources are in Deleted files");
}

#[test]
#[ignore = "heavy: creates and mounts disk images with hdiutil; run by npm run test:full"]
fn copy_move_and_recoverable_deletion_work_on_exfat() {
    copy_move_and_delete_on("ExFAT");
}

#[test]
#[ignore = "heavy: creates and mounts disk images with hdiutil; run by npm run test:full"]
fn copy_move_and_recoverable_deletion_work_on_fat32() {
    copy_move_and_delete_on("MS-DOS FAT32");
}

// A case-sensitive volume can hold two folders whose names differ only by
// capitalisation; they are two source roots, each scanned on its own.
#[test]
#[ignore = "heavy: creates and mounts disk images with hdiutil; run by npm run test:full"]
fn folders_differing_only_by_case_on_a_case_sensitive_volume_are_two_roots() {
    let image = MountedImage::create("Case-sensitive APFS");
    let upper = image.volume.join("Photos");
    let lower = image.volume.join("photos");
    std::fs::create_dir_all(&upper).unwrap();
    std::fs::create_dir_all(&lower).unwrap();
    std::fs::write(upper.join("a.jpg"), b"upper bytes").unwrap();
    std::fs::write(lower.join("b.jpg"), b"lower bytes").unwrap();
    let index = tempfile::tempdir().unwrap();
    let conn = index_store::open(&index.path().join("index.sqlite3")).unwrap();

    let first = scanner::settled_root(&conn, &upper).unwrap();
    scanner::walk_root(&conn, &first, &lists()).unwrap();
    let second = scanner::settled_root(&conn, &lower).unwrap();
    scanner::walk_root(&conn, &second, &lists()).unwrap();

    assert_ne!(first, second);
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM paths", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 2, "each folder's file is indexed under its own root");
}
