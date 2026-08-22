//! Durable user verdicts for pairs that must not be grouped as similar.
//!
//! The scan index is disposable and rebuilt from source media. These verdicts
//! are authored data, so they live in their own atomic text store and are
//! loaded into each wholesale group rebuild. A one-time, idempotent import
//! preserves verdicts written by older builds into `index.sqlite3`.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{logging, storage};

pub const FILE_NAME: &str = "similar-exclusions.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Exclusion {
    hash_a: String,
    hash_b: String,
    created_at_utc: String,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExclusionStore {
    exclusions: Vec<Exclusion>,
}

fn lock() -> MutexGuard<'static, ()> {
    static STORE_LOCK: Mutex<()> = Mutex::new(());
    STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn load_unlocked(root: &Path) -> Result<ExclusionStore, String> {
    let path = root.join(FILE_NAME);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExclusionStore::default());
        }
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("could not read {}: {error}", path.display()))
}

fn normalize(store: ExclusionStore) -> BTreeMap<(String, String), Exclusion> {
    let mut normalized = BTreeMap::new();
    for exclusion in store.exclusions {
        let (hash_a, hash_b) = canonical_pair(&exclusion.hash_a, &exclusion.hash_b);
        if hash_a == hash_b {
            continue;
        }
        normalized
            .entry((hash_a.clone(), hash_b.clone()))
            .or_insert(Exclusion {
                hash_a,
                hash_b,
                created_at_utc: exclusion.created_at_utc,
            });
    }
    normalized
}

fn save_unlocked(
    root: &Path,
    exclusions: BTreeMap<(String, String), Exclusion>,
) -> Result<(), String> {
    let store = ExclusionStore {
        exclusions: exclusions.into_values().collect(),
    };
    let mut text = serde_json::to_string_pretty(&store).map_err(|error| error.to_string())?;
    text.push('\n');
    storage::write_atomic(&root.join(FILE_NAME), text.as_bytes())
}

pub fn pairs(root: &Path) -> Result<HashSet<(String, String)>, String> {
    let _guard = lock();
    Ok(normalize(load_unlocked(root)?).into_keys().collect())
}

pub fn count(root: &Path) -> Result<u64, String> {
    let _guard = lock();
    Ok(normalize(load_unlocked(root)?).len() as u64)
}

/// Atomically adds one verdict for every current peer. The full new set lands
/// in one rename, so a failure leaves either all new verdicts or none of them.
pub fn add_for_peers(root: &Path, hash: &str, peers: &[String]) -> Result<u64, String> {
    let _guard = lock();
    let mut exclusions = normalize(load_unlocked(root)?);
    let now = logging::now_iso_millis();
    let mut added = 0u64;
    for peer in peers {
        let (hash_a, hash_b) = canonical_pair(hash, peer);
        if hash_a == hash_b {
            continue;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            exclusions.entry((hash_a.clone(), hash_b.clone()))
        {
            entry.insert(Exclusion {
                hash_a,
                hash_b,
                created_at_utc: now.clone(),
            });
            added += 1;
        }
    }
    if added > 0 {
        save_unlocked(root, exclusions)?;
    }
    Ok(added)
}

pub fn clear(root: &Path) -> Result<u64, String> {
    let _guard = lock();
    let previous = normalize(load_unlocked(root)?);
    if !previous.is_empty() || root.join(FILE_NAME).exists() {
        save_unlocked(root, BTreeMap::new())?;
    }
    Ok(previous.len() as u64)
}

/// Imports and then removes the legacy cache-table copy. Saving happens first;
/// if the SQLite cleanup fails, the next launch repeats the merge harmlessly.
pub fn migrate_legacy(root: &Path, conn: &Connection) -> Result<u64, String> {
    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'similar_exclusions'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .is_some();
    if !table_exists {
        return Ok(0);
    }

    let legacy: Vec<Exclusion> = {
        let mut statement = conn
            .prepare("SELECT hash_a, hash_b, created_at_utc FROM similar_exclusions")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok(Exclusion {
                    hash_a: row.get(0)?,
                    hash_b: row.get(1)?,
                    created_at_utc: row.get(2)?,
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };

    let imported = legacy.len() as u64;
    let _guard = lock();
    if !legacy.is_empty() {
        let mut exclusions = normalize(load_unlocked(root)?);
        for exclusion in legacy {
            let (hash_a, hash_b) = canonical_pair(&exclusion.hash_a, &exclusion.hash_b);
            exclusions
                .entry((hash_a.clone(), hash_b.clone()))
                .or_insert(Exclusion {
                    hash_a,
                    hash_b,
                    created_at_utc: exclusion.created_at_utc,
                });
        }
        save_unlocked(root, exclusions)?;
    }
    conn.execute("DROP TABLE similar_exclusions", [])
        .map_err(|error| error.to_string())?;
    Ok(imported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_updates_land_as_one_canonical_deduplicated_store() {
        let root = tempfile::Builder::new()
            .prefix("onecopy-exclusions-")
            .tempdir()
            .unwrap();
        assert_eq!(
            add_for_peers(
                root.path(),
                "z",
                &["a".to_string(), "b".to_string(), "a".to_string()],
            )
            .unwrap(),
            2
        );
        assert_eq!(count(root.path()).unwrap(), 2);
        assert!(
            pairs(root.path())
                .unwrap()
                .contains(&("a".into(), "z".into()))
        );

        let text = std::fs::read_to_string(root.path().join(FILE_NAME)).unwrap();
        assert!(text.ends_with('\n'));
        assert!(!text.contains("\"hashA\": \"z\""));
    }

    #[test]
    fn corrupt_authored_store_is_never_replaced_with_empty_data() {
        let root = tempfile::Builder::new()
            .prefix("onecopy-exclusions-corrupt-")
            .tempdir()
            .unwrap();
        let path = root.path().join(FILE_NAME);
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(add_for_peers(root.path(), "a", &["b".to_string()]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json");
    }

    #[test]
    fn legacy_index_rows_are_imported_before_the_cache_table_is_dropped() {
        let root = tempfile::Builder::new()
            .prefix("onecopy-exclusions-migrate-")
            .tempdir()
            .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE similar_exclusions (
               hash_a TEXT NOT NULL,
               hash_b TEXT NOT NULL,
               created_at_utc TEXT NOT NULL,
               PRIMARY KEY (hash_a, hash_b)
             );
             INSERT INTO similar_exclusions VALUES ('a', 'b', '2026-08-01T00:00:00.000Z');",
        )
        .unwrap();

        assert_eq!(migrate_legacy(root.path(), &conn).unwrap(), 1);
        assert!(
            pairs(root.path())
                .unwrap()
                .contains(&("a".into(), "b".into()))
        );
        let still_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'similar_exclusions')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!still_exists);
    }
}
