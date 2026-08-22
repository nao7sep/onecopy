// Contract for the data directory's on-disk filenames.
//
// These constants are the single source of truth for what OneCopy writes into
// its storage root. This suite pins the volatile UI/session state to
// `state.json` and guards it against silently merging with, or drifting into,
// the durable `config.json`, the index, or the backup store.

use onecopy_lib::backup_store::BACKUPS_DB_FILE_NAME;
use onecopy_lib::paths::{
    BIN_DIR_NAME, DEPENDENCIES_FILE_NAME, LOGS_DIR_NAME, MODELS_DIR_NAME, TEMP_DIR_NAME,
    SOURCE_VOLUMES_FILE_NAME, TRASH_DIR_NAME,
};
use onecopy_lib::similar_exclusions::FILE_NAME as SIMILAR_EXCLUSIONS_FILE_NAME;
use onecopy_lib::storage::{CACHE_DIR_NAME, CONFIG_FILE_NAME, INDEX_DB_FILE_NAME, STATE_FILE_NAME};

#[test]
fn durable_config_stays_config_json() {
    assert_eq!(CONFIG_FILE_NAME, "config.json");
}

#[test]
fn volatile_state_resolves_to_state_json() {
    assert_eq!(STATE_FILE_NAME, "state.json");
}

#[test]
fn index_store_stays_index_sqlite3() {
    assert_eq!(INDEX_DB_FILE_NAME, "index.sqlite3");
}

#[test]
fn backup_store_stays_backups_sqlite3() {
    assert_eq!(BACKUPS_DB_FILE_NAME, "backups.sqlite3");
}

#[test]
fn cache_dir_stays_cache() {
    assert_eq!(CACHE_DIR_NAME, "cache");
}

#[test]
fn standard_subpaths_stay_pinned() {
    assert_eq!(LOGS_DIR_NAME, "logs");
    assert_eq!(BIN_DIR_NAME, "bin");
    assert_eq!(TEMP_DIR_NAME, "temp");
    assert_eq!(TRASH_DIR_NAME, "trash");
    // The per-volume trash directory — the name that actually reaches disk on
    // every non-home volume, and the one the scanner and watcher skip guards
    // match against. Unpinned before, so a rename would have silently stopped
    // the skip and made the app index its own trash.
    assert_eq!(onecopy_lib::trash::TRASH_DIR_NAME, ".onecopy-trash");
    assert_ne!(
        onecopy_lib::trash::TRASH_DIR_NAME,
        TRASH_DIR_NAME,
        "the per-volume and home-volume trash names are distinct"
    );
    assert_eq!(DEPENDENCIES_FILE_NAME, "dependencies.json");
    assert_eq!(SOURCE_VOLUMES_FILE_NAME, "source-volumes.json");
    assert_eq!(SIMILAR_EXCLUSIONS_FILE_NAME, "similar-exclusions.json");
    assert_eq!(MODELS_DIR_NAME, "models");
}

#[test]
fn every_store_has_its_own_file() {
    // Each kind of persisted thing gets its own file/directory; a collision
    // would let one silently overwrite another (persisted-store-separation
    // conventions).
    let names = [
        CONFIG_FILE_NAME,
        STATE_FILE_NAME,
        INDEX_DB_FILE_NAME,
        BACKUPS_DB_FILE_NAME,
        DEPENDENCIES_FILE_NAME,
        SOURCE_VOLUMES_FILE_NAME,
        SIMILAR_EXCLUSIONS_FILE_NAME,
        CACHE_DIR_NAME,
        LOGS_DIR_NAME,
        BIN_DIR_NAME,
        MODELS_DIR_NAME,
        TEMP_DIR_NAME,
        TRASH_DIR_NAME,
    ];
    for (i, a) in names.iter().enumerate() {
        for b in names.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }
}
