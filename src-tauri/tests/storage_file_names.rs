// Contract for the data directory's on-disk filenames.
//
// These constants are the single source of truth for what OneCopy writes into
// its storage root. This suite pins the volatile UI/session state to
// `state.json` and guards it against silently merging with, or drifting into,
// the durable `config.json`, the index, or the backup store.

use onecopy_lib::backup_store::BACKUPS_DB_FILE_NAME;
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
fn every_store_has_its_own_file() {
    // Config, state, index, and the backup store are four distinct kinds of
    // persisted thing; a collision would let one silently overwrite another
    // (persisted-store-separation conventions).
    let names = [
        CONFIG_FILE_NAME,
        STATE_FILE_NAME,
        INDEX_DB_FILE_NAME,
        BACKUPS_DB_FILE_NAME,
    ];
    for (i, a) in names.iter().enumerate() {
        for b in names.iter().skip(i + 1) {
            assert_ne!(a, b);
        }
    }
}
