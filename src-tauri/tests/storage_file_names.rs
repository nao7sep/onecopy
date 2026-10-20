// Contract for the data directory's on-disk filenames.
//
// The names a written convention fixes are pinned here: the storage-path
// conventions name `config.json`, `state.json`, `logs/` and
// `backups.sqlite3`, the data-lifecycle conventions name `records.sqlite3`,
// and the managed-runtime-dependencies conventions name the dependency
// `temp/` staging directory. Every other store's name is the app's own
// choice, so the suite only guards that no two stores share one.

use onecopy_lib::records::RECORDS_DB_FILE_NAME;
use onecopy_lib::backup_store::BACKUPS_DB_FILE_NAME;
use onecopy_lib::file_identity::INSTALLATION_ID_FILE_NAME;
use onecopy_lib::paths::{
    BIN_DIR_NAME, DEPENDENCIES_FILE_NAME, LOGS_DIR_NAME, MODELS_DIR_NAME, SOURCE_VOLUMES_FILE_NAME,
    TEMP_DIR_NAME,
};
use onecopy_lib::storage::{
    CACHE_DIR_NAME, CONFIG_FILE_NAME, INDEX_DB_FILE_NAME, PREVIEW_WINDOW_FILE_NAME,
    RECORDS_WINDOW_FILE_NAME, STATE_FILE_NAME, WINDOW_FILE_NAME,
};

#[test]
fn durable_config_stays_config_json() {
    assert_eq!(CONFIG_FILE_NAME, "config.json");
}

#[test]
fn volatile_state_resolves_to_state_json() {
    assert_eq!(STATE_FILE_NAME, "state.json");
}

#[test]
fn backup_store_stays_backups_sqlite3() {
    assert_eq!(BACKUPS_DB_FILE_NAME, "backups.sqlite3");
}

#[test]
fn records_store_stays_records_sqlite3() {
    assert_eq!(RECORDS_DB_FILE_NAME, "records.sqlite3");
}

#[test]
fn convention_named_subpaths_stay_pinned() {
    assert_eq!(LOGS_DIR_NAME, "logs");
    assert_eq!(TEMP_DIR_NAME, "temp");
}

#[test]
fn every_store_has_its_own_file() {
    // Each kind of persisted thing gets its own file/directory; a collision
    // would let one silently overwrite another (persisted-store-separation
    // conventions).
    let names = [
        CONFIG_FILE_NAME,
        STATE_FILE_NAME,
        WINDOW_FILE_NAME,
        PREVIEW_WINDOW_FILE_NAME,
        RECORDS_WINDOW_FILE_NAME,
        INDEX_DB_FILE_NAME,
        BACKUPS_DB_FILE_NAME,
        RECORDS_DB_FILE_NAME,
        DEPENDENCIES_FILE_NAME,
        SOURCE_VOLUMES_FILE_NAME,
        CACHE_DIR_NAME,
        LOGS_DIR_NAME,
        BIN_DIR_NAME,
        MODELS_DIR_NAME,
        TEMP_DIR_NAME,
        INSTALLATION_ID_FILE_NAME,
    ];
    for (i, a) in names.iter().enumerate() {
        for b in names.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }
}
