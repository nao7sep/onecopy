use super::*;

#[test]
fn a_path_below_a_synced_folder_is_synced_in_any_case() {
    let synced = vec![PathBuf::from("/Users/me/Library/CloudStorage")];
    assert!(is_synced(Path::new("/Users/me/Library/CloudStorage/OneDrive-Personal/Photos/a.jpg"), &synced));
    assert!(is_synced(Path::new("/users/ME/library/cloudstorage/Dropbox/b.jpg"), &synced));
    assert!(!is_synced(Path::new("/Users/me/Pictures/a.jpg"), &synced));
    // A sibling whose name only starts the same is not inside.
    assert!(!is_synced(Path::new("/Users/me/Library/CloudStorageBackup/a.jpg"), &synced));
}

#[test]
fn an_ordinary_local_file_is_not_online_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.jpg");
    std::fs::write(&file, b"here").unwrap();
    assert!(!is_online_only(&std::fs::metadata(&file).unwrap()));
}

#[test]
fn an_item_with_a_copy_in_a_synced_folder_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    conn.execute("INSERT INTO contents (hash, kind, byte_size) VALUES ('h1', 'image', 1)", []).unwrap();
    for (path, folder) in [("/Local/a.jpg", "/Local"), ("/Cloud/OneDrive/a.jpg", "/Cloud/OneDrive")] {
        conn.execute(
            "INSERT INTO paths (abs_path, dir_path, file_name, stem, ext, kind, size, content_hash) \
             VALUES (?1, ?2, 'a.jpg', 'a', 'jpg', 'image', 1, 'h1')",
            [path, folder],
        )
        .unwrap();
    }
    let item = crate::operations::ItemIdentity { hash: Some("h1".to_string()), path_id: None };
    let synced = vec![PathBuf::from("/Cloud")];
    assert!(items_in_synced_folders(&conn, std::slice::from_ref(&item), &synced).unwrap());
    assert!(!items_in_synced_folders(&conn, &[item], &[PathBuf::from("/Elsewhere")]).unwrap());
}
