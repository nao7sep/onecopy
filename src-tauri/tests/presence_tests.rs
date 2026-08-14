// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).
//
// The volume-identity gate. It runs at the session gate AND before every
// destructive operation, and it exists for the case presence alone cannot
// catch: a DIFFERENT drive mounted at a configured path. Backup drives share
// directory structures, so "the folder is there" proves nothing. It had no
// tests.

use onecopy_lib::index_store;
use onecopy_lib::volume::{check_identity, prune_identities, IdentityCheck};
use rusqlite::Connection;

fn db(label: &str) -> Connection {
    let dir = std::env::temp_dir().join(format!("onecopy-presence-{label}"));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("index.sqlite3");
    let _ = std::fs::remove_file(&file);
    index_store::open(&file).unwrap()
}

fn recorded(conn: &Connection, dir: &str) -> Option<String> {
    conn.query_row(
        "SELECT identity FROM source_volumes WHERE dir = ?1",
        [dir],
        |r| r.get(0),
    )
    .ok()
}

#[test]
fn first_sight_records_the_identity_and_reports_nothing() {
    let conn = db("first");
    assert_eq!(
        check_identity(&conn, "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::FirstSight
    );
    assert_eq!(recorded(&conn, "/Volumes/Photos").as_deref(), Some("UUID-A"));
}

#[test]
fn the_same_volume_verifies_quietly() {
    let conn = db("same");
    check_identity(&conn, "/Volumes/Photos", "UUID-A").unwrap();
    assert_eq!(
        check_identity(&conn, "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::Unchanged
    );
}

#[test]
fn a_different_volume_is_reported_without_overwriting_the_record() {
    let conn = db("substituted");
    check_identity(&conn, "/Volumes/Photos", "UUID-A").unwrap();

    // A different drive is now mounted where the configured one used to be.
    let result = check_identity(&conn, "/Volumes/Photos", "UUID-B").unwrap();
    assert_eq!(
        result,
        IdentityCheck::Substituted { recorded: "UUID-A".to_string() }
    );

    // The record must survive: overwriting it would launder the substitution
    // into the new normal and the NEXT check would pass silently, with
    // destructive operations then running against the wrong drive.
    assert_eq!(recorded(&conn, "/Volumes/Photos").as_deref(), Some("UUID-A"));
    assert_eq!(
        check_identity(&conn, "/Volumes/Photos", "UUID-B").unwrap(),
        IdentityCheck::Substituted { recorded: "UUID-A".to_string() },
        "it stays reported until the right drive returns"
    );

    // The original drive coming back verifies again.
    assert_eq!(
        check_identity(&conn, "/Volumes/Photos", "UUID-A").unwrap(),
        IdentityCheck::Unchanged
    );
}

#[test]
fn identities_for_unconfigured_directories_are_pruned() {
    let conn = db("prune");
    check_identity(&conn, "/Volumes/Photos", "UUID-A").unwrap();
    check_identity(&conn, "/Volumes/Old", "UUID-OLD").unwrap();

    let pruned = prune_identities(&conn, &["/Volumes/Photos".to_string()]).unwrap();

    assert_eq!(pruned, 1);
    assert_eq!(recorded(&conn, "/Volumes/Photos").as_deref(), Some("UUID-A"));
    assert_eq!(recorded(&conn, "/Volumes/Old"), None);
    // A re-added path is first sight again, not a false substitution.
    assert_eq!(
        check_identity(&conn, "/Volumes/Old", "UUID-NEW").unwrap(),
        IdentityCheck::FirstSight
    );
}

#[test]
fn each_directory_keeps_its_own_identity() {
    let conn = db("per-dir");
    check_identity(&conn, "/Volumes/A", "UUID-A").unwrap();
    check_identity(&conn, "/Volumes/B", "UUID-B").unwrap();

    assert_eq!(
        check_identity(&conn, "/Volumes/A", "UUID-A").unwrap(),
        IdentityCheck::Unchanged
    );
    assert_eq!(
        check_identity(&conn, "/Volumes/B", "UUID-B").unwrap(),
        IdentityCheck::Unchanged
    );
}
