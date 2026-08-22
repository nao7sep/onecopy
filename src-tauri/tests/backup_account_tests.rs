// The developer-approved managed-text account: durable safety/authored text
// records; re-derivable dependency facts do not.

use onecopy_lib::backup_store;
use onecopy_lib::binaries::BinaryFacts;
use onecopy_lib::{binaries_manager, paths, similar_exclusions, volume};

#[test]
fn durable_text_records_and_dependency_facts_do_not() {
    let root = tempfile::Builder::new()
        .prefix("onecopy-backup-account-")
        .tempdir()
        .unwrap();
    let backup_file = root.path().join(backup_store::BACKUPS_DB_FILE_NAME);
    backup_store::init(backup_file.clone());

    volume::check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();
    similar_exclusions::add_for_peers(root.path(), "a", &["b".to_string()]).unwrap();
    binaries_manager::save_facts_for(
        root.path(),
        "ffmpeg",
        &BinaryFacts {
            latest_known_version: Some("9.1".to_string()),
            last_checked_at_utc: Some("2026-08-22T00:00:00.000Z".to_string()),
        },
    )
    .unwrap();

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
    assert!(paths_recorded
        .iter()
        .any(|path| path.ends_with(similar_exclusions::FILE_NAME)));
    assert!(!paths_recorded
        .iter()
        .any(|path| path.ends_with(paths::DEPENDENCIES_FILE_NAME)));
}
