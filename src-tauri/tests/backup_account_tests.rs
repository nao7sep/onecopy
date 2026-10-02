// The developer-approved managed-text account: durable safety/authored text
// records; re-derivable dependency facts and volatile state do not.

use onecopy_lib::backup_store;
use onecopy_lib::binaries::BinaryFacts;
use onecopy_lib::{binaries_manager, paths, storage, volume};

#[test]
fn durable_text_records_and_dependency_facts_and_volatile_state_do_not() {
    let root = tempfile::Builder::new()
        .prefix("onecopy-backup-account-")
        .tempdir()
        .unwrap();
    let backup_file = root.path().join(backup_store::BACKUPS_DB_FILE_NAME);
    backup_store::init(backup_file.clone());

    volume::check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();
    binaries_manager::save_facts_for(
        root.path(),
        "ffmpeg",
        &BinaryFacts {
            latest_known_version: Some("9.1".to_string()),
            last_checked_at_utc: Some("2026-08-22T00:00:00.000Z".to_string()),
        },
    )
    .unwrap();

    // Volatile state is written atomically but never recorded; config is.
    binaries_manager::save_check_attempt(
        root.path(),
        binaries_manager::GITHUB_RELEASE_ATTEMPT_KEY,
        "2026-08-22T00:00:00.000Z",
    )
    .unwrap();
    assert_eq!(
        binaries_manager::load_check_attempt(
            root.path(),
            binaries_manager::GITHUB_RELEASE_ATTEMPT_KEY
        )
        .as_deref(),
        Some("2026-08-22T00:00:00.000Z")
    );
    storage::patch_json_store(
        &root.path().join(storage::STATE_FILE_NAME),
        &serde_json::json!({ "zoomLevel": 1.2 }),
    )
    .unwrap();
    storage::save_window_state(root.path(), &serde_json::json!({ "x": 10 })).unwrap();
    storage::save_preview_window_state(root.path(), &serde_json::json!({ "x": 20 })).unwrap();
    storage::save_config(root.path(), &serde_json::json!({ "theme": "dark" })).unwrap();

    let conn = rusqlite::Connection::open(backup_file).unwrap();
    let mut statement = conn
        .prepare("SELECT path FROM backups ORDER BY path")
        .unwrap();
    let paths_recorded: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(paths_recorded
        .iter()
        .any(|path| path.ends_with(paths::SOURCE_VOLUMES_FILE_NAME)));
    assert!(!paths_recorded
        .iter()
        .any(|path| path.ends_with(paths::DEPENDENCIES_FILE_NAME)));
    assert!(paths_recorded
        .iter()
        .any(|path| path.ends_with(storage::CONFIG_FILE_NAME)));
    for state in [
        storage::STATE_FILE_NAME,
        storage::WINDOW_FILE_NAME,
        storage::PREVIEW_WINDOW_FILE_NAME,
    ] {
        assert!(root.path().join(state).is_file(), "{state} was written");
        assert!(
            !paths_recorded.iter().any(|path| path.ends_with(state)),
            "{state} is never recorded"
        );
    }
}
