// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// The Windows long-path grammar, asserted on EVERY host. The rules are fiddly
// and the consequence of getting them wrong is silent — a photo beyond the
// classic limit simply never enters the app — so leaving them provable only on
// the machine we visit least would be the worst possible arrangement.

use onecopy_lib::winpath::{extended_form, for_display};

#[test]
fn a_drive_absolute_path_gets_the_verbatim_prefix() {
    assert_eq!(
        extended_form(r"C:\photos\2016\IMG_0001.jpg").as_deref(),
        Some(r"\\?\C:\photos\2016\IMG_0001.jpg")
    );
    // Any drive letter, either case.
    assert_eq!(extended_form(r"d:\x").as_deref(), Some(r"\\?\d:\x"));
}

#[test]
fn forward_slashes_become_backslashes_first() {
    // A verbatim path takes only backslashes; a forward slash would stay a
    // literal character inside the file name instead of separating it.
    assert_eq!(
        extended_form("C:/photos/a.jpg").as_deref(),
        Some(r"\\?\C:\photos\a.jpg")
    );
}

#[test]
fn a_network_share_takes_the_unc_form() {
    assert_eq!(
        extended_form(r"\\nas\photos\a.jpg").as_deref(),
        Some(r"\\?\UNC\nas\photos\a.jpg")
    );
    // A server with no share is not a usable root.
    assert_eq!(extended_form(r"\\nas"), None);
}

#[test]
fn already_verbatim_paths_are_left_alone() {
    // Prefixing twice yields a path that resolves to nothing.
    assert_eq!(extended_form(r"\\?\C:\photos\a.jpg"), None);
    assert_eq!(extended_form(r"\\?\UNC\nas\photos\a.jpg"), None);
}

#[test]
fn device_paths_are_left_alone() {
    assert_eq!(extended_form(r"\\.\PhysicalDrive0"), None);
}

#[test]
fn relative_paths_are_left_alone() {
    assert_eq!(extended_form(r"photos\a.jpg"), None);
    assert_eq!(extended_form(r"\photos\a.jpg"), None);
    // Drive-RELATIVE despite the drive letter: C:folder means "folder, on C's
    // current directory", which a verbatim prefix would silently change.
    assert_eq!(extended_form(r"C:photos"), None);
}

#[test]
fn a_parent_component_is_refused_rather_than_guessed() {
    // Verbatim paths are handed to the filesystem WITHOUT normalization, so
    // `..` would stop meaning "parent" and the path would address something
    // else entirely. Refusing keeps the classic path and the classic limit,
    // which is wrong-but-visible rather than wrong-and-silent.
    assert_eq!(extended_form(r"C:\photos\..\other\a.jpg"), None);
    assert_eq!(extended_form("C:/photos/../a.jpg"), None);
    // A file merely CONTAINING dots is fine.
    assert_eq!(
        extended_form(r"C:\photos\my..album\a.jpg").as_deref(),
        Some(r"\\?\C:\photos\my..album\a.jpg")
    );
}

#[test]
fn display_strips_the_prefix_back_off() {
    // The user reads these in the metadata pane's copy list and the issues
    // list; \\?\C:\photos\a.jpg is not what anyone recognises as a location.
    assert_eq!(for_display(r"\\?\C:\photos\a.jpg"), r"C:\photos\a.jpg");
    assert_eq!(for_display(r"\\?\UNC\nas\photos\a.jpg"), r"\\nas\photos\a.jpg");
    // Untouched when there is nothing to strip.
    assert_eq!(for_display(r"C:\photos\a.jpg"), r"C:\photos\a.jpg");
    assert_eq!(for_display("/Users/x/photos/a.jpg"), "/Users/x/photos/a.jpg");
}

#[test]
fn the_transform_round_trips_through_display() {
    for original in [
        r"C:\photos\2016\spain\beach.jpg",
        r"\\nas\media\clip.mov",
    ] {
        let extended = extended_form(original).expect("absolute paths convert");
        assert_eq!(for_display(&extended), original, "{original}");
    }
}

#[test]
fn a_path_past_the_classic_limit_is_exactly_what_this_is_for() {
    // 260 is the classic cap. A real backup tree reaches it with ordinary
    // folder names, and today such a file is simply invisible to the app.
    let deep = format!(r"C:\{}\IMG_0001.jpg", vec!["a-folder-name"; 20].join("\\"));
    assert!(deep.len() > 260, "the fixture must actually exceed the limit");
    let extended = extended_form(&deep).expect("it converts");
    assert!(extended.starts_with(r"\\?\"));
    assert_eq!(for_display(&extended), deep);
}

// The grammar above is half the story; this proves the whole library path on
// a real Windows volume: a photo nested past the classic 260-character limit
// is found, read, deleted recoverably and restored where it was.
#[cfg(windows)]
#[test]
fn a_file_beyond_the_classic_limit_is_scanned_hashed_deleted_and_restored() {
    use onecopy_lib::winpath::for_fs;
    use onecopy_lib::{file_identity, file_names, index_store, operations, restore, scanner, trash};

    let dir = tempfile::Builder::new().prefix("onecopy-long-path-").tempdir().unwrap();
    let root = dir.path().join("photos");
    let data = dir.path().join("apphome");
    let folder = (0..8).fold(root.clone(), |path, depth| {
        path.join(format!("{depth}-{}", "nested-camera-folder-".repeat(2)))
    });
    let file = folder.join("IMG_20160305_123456.jpg");
    assert!(file.as_os_str().len() > 300, "{} is not long enough", file.display());
    // Same size, different bytes: the size collision makes hashing read both
    // files completely instead of naming the unique one provisionally.
    let long_bytes = b"bytes far below the configured root";
    let short_bytes = b"bytes right beside the source root!";
    assert_eq!(long_bytes.len(), short_bytes.len());
    std::fs::create_dir_all(for_fs(&folder)).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(for_fs(&file), long_bytes).unwrap();
    std::fs::write(root.join("short.jpg"), short_bytes).unwrap();
    let config = serde_json::json!({
        "formatVersion": 1,
        "sourceDirs": [root.to_string_lossy()],
    });
    std::fs::write(data.join("config.json"), serde_json::to_vec(&config).unwrap()).unwrap();
    let settings = scanner::settings_from_config(Some(&config), &data, 0);
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let cache = onecopy_lib::preview::CachePaths::new(dir.path().join("cache"));
    let stored = for_fs(&file).to_string_lossy().into_owned();
    let live = |conn: &rusqlite::Connection| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM paths WHERE abs_path = ?1 AND missing = 0",
            [&stored],
            |row| row.get(0),
        )
        .unwrap()
    };

    // Scanned and read in full.
    let walked = scanner::walk_root(&conn, &root, &settings.lists).unwrap();
    assert_eq!((walked.added, walked.errors), (2, 0));
    assert_eq!(live(&conn), 1);
    scanner::hash_pending(&conn, &cache).unwrap();
    let hash: String = conn
        .query_row("SELECT content_hash FROM paths WHERE abs_path = ?1", [&stored], |row| row.get(0))
        .unwrap();
    assert_eq!(hash, blake3::hash(long_bytes).to_hex().to_string());

    // Deleted recoverably.
    let deleted = operations::delete_item(
        &conn,
        &data,
        &cache,
        operations::ItemRef::Hash(&hash),
        operations::DeleteMode::Trash,
    )
    .unwrap();
    assert_eq!((deleted.deleted_files, deleted.failed_files), (1, 0));
    assert!(!for_fs(&file).exists());
    assert_eq!(live(&conn), 0);
    assert_eq!(std::fs::read(root.join("short.jpg")).unwrap(), short_bytes);

    // Restored to the same long path and indexed there again.
    let listing = trash::list_root(&root, &data).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.entries[0].status, trash::EntryStatus::Restorable);
    let ids = vec![listing.entries[0].id.clone()];
    let candidates =
        restore::candidates(&root, &listing, &ids, &file_identity::volume_of, &|| false).unwrap();
    let plan = restore::plan_restore(
        &candidates,
        file_names::FolderNames::for_directory(&root),
        file_names::RenameStyle::SpaceNumber,
        &mut |path| restore::name_available(path),
    );
    let (restored, _) = restore::execute(
        &conn,
        &root,
        &plan,
        &settings,
        &file_identity::volume_of,
        &|| false,
        &mut |_| {},
    )
    .unwrap();
    assert_eq!((restored.restored.len(), restored.failed), (1, 0));
    assert_eq!(std::fs::read(for_fs(&file)).unwrap(), long_bytes);
    assert_eq!(live(&conn), 1);
}
