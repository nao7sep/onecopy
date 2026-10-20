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

#[test]
fn first_sight_records_the_identity_as_managed_text() {
    let root = root("first");
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::FirstSight
    );
    let file = root.path().join(SOURCE_VOLUMES_FILE_NAME);
    let text = std::fs::read_to_string(&file).expect("the record is a text file in the data root");
    serde_json::from_str::<serde_json::Value>(&text).expect("the record is JSON");
    assert!(text.contains("UUID-A"), "{text}");
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-B").unwrap(),
        IdentityCheck::Substituted {
            recorded: "UUID-A".to_string()
        },
        "a later check reads the recorded identity back"
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
fn identities_for_removed_sources_are_pruned() {
    let root = root("prune");
    check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap();
    check_identity(root.path(), "/Volumes/Old", "UUID-OLD").unwrap();

    assert_eq!(
        prune_identities(root.path(), &["/Volumes/Photos".to_string()]).unwrap(),
        1
    );
    assert_eq!(
        check_identity(root.path(), "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::Unchanged,
        "the configured source keeps its identity"
    );
    assert_eq!(
        check_identity(root.path(), "/Volumes/Old", "UUID-NEW").unwrap(),
        IdentityCheck::FirstSight
    );
}

#[test]
fn a_corrupt_trust_store_is_never_replaced_with_a_new_baseline() {
    let root = root("corrupt");
    let file = root.path().join(SOURCE_VOLUMES_FILE_NAME);
    std::fs::write(&file, b"{ not json").unwrap();

    assert!(check_identity(root.path(), "/Volumes/Photos", "UUID-B").is_err());
    assert_eq!(std::fs::read(file).unwrap(), b"{ not json");
}
