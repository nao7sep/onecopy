// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// Source-volume identity is a durable destructive-operation trust baseline,
// independent of the disposable scan index.

use onecopy_lib::index_store;
use onecopy_lib::paths::SOURCE_VOLUMES_FILE_NAME;
use onecopy_lib::volume::{check_identity, prune_identities, IdentityCheck};

fn root(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("onecopy-presence-{label}-"))
        .tempdir()
        .unwrap()
}

fn recorded(root: &std::path::Path, dir: &str) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(SOURCE_VOLUMES_FILE_NAME)).ok()?).ok()?;
    value["sources"]
        .as_array()?
        .iter()
        .find(|source| source["dir"] == dir)
        .and_then(|source| source["identity"].as_str())
        .map(str::to_string)
}

fn recorded_at(root: &std::path::Path, dir: &str) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(SOURCE_VOLUMES_FILE_NAME)).ok()?).ok()?;
    value["sources"]
        .as_array()?
        .iter()
        .find(|source| source["dir"] == dir)
        .and_then(|source| source["recordedAtUtc"].as_str())
        .map(str::to_string)
}

fn create_legacy_store(root: &std::path::Path, rows: &[(&str, &str, &str)]) {
    let conn = rusqlite::Connection::open(root.join("index.sqlite3")).unwrap();
    conn.execute_batch(
        "CREATE TABLE source_volumes (
            dir TEXT PRIMARY KEY,
            identity TEXT NOT NULL,
            recorded_at_utc TEXT NOT NULL
        );",
    )
    .unwrap();
    for (dir, identity, recorded_at_utc) in rows {
        conn.execute(
            "INSERT INTO source_volumes(dir, identity, recorded_at_utc) VALUES (?1, ?2, ?3)",
            rusqlite::params![dir, identity, recorded_at_utc],
        )
        .unwrap();
    }
}

#[test]
fn first_sight_records_the_identity_as_managed_text() {
    let root = root("first");
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::FirstSight
    );
    assert_eq!(
        recorded(root.path(), "/Volumes/Photos").as_deref(),
        Some("UUID-A")
    );
}

#[test]
fn the_same_volume_verifies_quietly() {
    let root = root("same");
    check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::Unchanged
    );
}

#[test]
fn a_different_volume_is_reported_without_overwriting_the_record() {
    let root = root("substituted");
    check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();

    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-B").unwrap(),
        IdentityCheck::Substituted {
            recorded: "UUID-A".to_string()
        }
    );
    assert_eq!(
        recorded(root.path(), "/Volumes/Photos").as_deref(),
        Some("UUID-A")
    );
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-B").unwrap(),
        IdentityCheck::Substituted {
            recorded: "UUID-A".to_string()
        },
        "the substitution stays blocked until the recorded drive returns"
    );
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::Unchanged
    );
}

#[test]
fn each_directory_keeps_its_own_identity() {
    let root = root("per-directory");
    check_identity(root.path(), "/Volumes/A", "UUID-A").unwrap();
    check_identity(root.path(), "/Volumes/B", "UUID-B").unwrap();

    assert_eq!(
        check_identity(root.path(), "/Volumes/A", "UUID-A").unwrap(),
        IdentityCheck::Unchanged
    );
    assert_eq!(
        check_identity(root.path(), "/Volumes/B", "UUID-B").unwrap(),
        IdentityCheck::Unchanged
    );
}

#[test]
fn deleting_the_scan_index_cannot_reset_the_trust_baseline() {
    let root = root("index-independent");
    check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();

    let index = root.path().join("index.sqlite3");
    drop(index_store::open(&index).unwrap());
    std::fs::remove_file(&index).unwrap();
    drop(index_store::open(&index).unwrap());

    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-B").unwrap(),
        IdentityCheck::Substituted {
            recorded: "UUID-A".to_string()
        }
    );
}

#[test]
fn a_legacy_baseline_is_preserved_before_a_substitution_is_compared() {
    let root = root("legacy-substituted");
    create_legacy_store(
        root.path(),
        &[("/Volumes/Photos", "UUID-A", "2026-08-21T00:00:00.000Z")],
    );

    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-B").unwrap(),
        IdentityCheck::Substituted {
            recorded: "UUID-A".to_string()
        }
    );
    assert_eq!(
        recorded(root.path(), "/Volumes/Photos").as_deref(),
        Some("UUID-A")
    );
    assert_eq!(
        recorded_at(root.path(), "/Volumes/Photos").as_deref(),
        Some("2026-08-21T00:00:00.000Z")
    );

    std::fs::remove_file(root.path().join("index.sqlite3")).unwrap();
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::Unchanged,
        "the imported managed store remains authoritative after the legacy index is gone"
    );
}

#[test]
fn an_absent_legacy_table_or_an_empty_one_is_genuinely_first_sight() {
    let no_table = root("legacy-no-table");
    drop(rusqlite::Connection::open(no_table.path().join("index.sqlite3")).unwrap());
    assert_eq!(
        check_identity(no_table.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::FirstSight
    );

    let empty = root("legacy-empty");
    create_legacy_store(empty.path(), &[]);
    assert_eq!(
        check_identity(empty.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::FirstSight
    );
}

#[test]
fn identities_for_removed_sources_are_pruned() {
    let root = root("prune");
    check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();
    check_identity(root.path(), "/Volumes/Old", "UUID-OLD").unwrap();

    assert_eq!(
        prune_identities(root.path(), &["/Volumes/Photos".to_string()]).unwrap(),
        1
    );
    assert_eq!(
        recorded(root.path(), "/Volumes/Photos").as_deref(),
        Some("UUID-A")
    );
    assert_eq!(recorded(root.path(), "/Volumes/Old"), None);
    assert_eq!(
        check_identity(root.path(), "/Volumes/Old", "UUID-NEW").unwrap(),
        IdentityCheck::FirstSight
    );
}

#[test]
fn a_corrupt_trust_store_is_never_replaced_with_a_new_baseline() {
    let root = root("corrupt");
    create_legacy_store(
        root.path(),
        &[("/Volumes/Photos", "UUID-A", "2026-08-21T00:00:00.000Z")],
    );
    let file = root.path().join(SOURCE_VOLUMES_FILE_NAME);
    std::fs::write(&file, b"{ not json").unwrap();

    assert!(check_identity(root.path(), "/Volumes/Photos", "UUID-B").is_err());
    assert_eq!(std::fs::read(file).unwrap(), b"{ not json");
}
