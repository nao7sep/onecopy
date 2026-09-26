// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::library_settings;
use onecopy_lib::scanner::{self, ResolveScope, ScanSettings};
use onecopy_lib::visibility::Policy;
use onecopy_lib::{extensions, index_store};
use rusqlite::Connection;
use serde_json::json;

struct Fixture {
    dir: tempfile::TempDir,
    conn: Connection,
}

fn settings(fixture: &Fixture, timezone: &str) -> ScanSettings {
    scanner::settings_from_config(
        Some(&json!({
            "defaultTimezone": timezone,
            "sourceDirs": [fixture.dir.path().join("root").to_string_lossy()],
        })),
        fixture.dir.path(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
    )
}

/// One photo whose naive filename date resolves through the default
/// timezone, indexed and resolved under Tokyo.
fn indexed_under_tokyo() -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-library-settings-")
        .tempdir()
        .unwrap();
    let root = dir.path().join("root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("IMG_20160305_123456.jpg"), b"not-a-real-jpeg").unwrap();
    let conn = index_store::open(&dir.path().join("index.sqlite3")).unwrap();
    let fixture = Fixture { dir, conn };
    let owned = |list: &[&str]| list.iter().map(|value| value.to_string()).collect();
    let lists = scanner::ScanLists {
        images: owned(extensions::IMAGE_EXTENSIONS),
        videos: vec![],
        audio: vec![],
        companions: vec![],
    };
    scanner::walk_root(&fixture.conn, &root, &lists).unwrap();
    let cache = onecopy_lib::preview::CachePaths::new(fixture.dir.path().join("cache"));
    scanner::hash_pending(&fixture.conn, &cache).unwrap();
    scanner::extract_pending(&fixture.conn).unwrap();
    let tokyo = settings(&fixture, "Asia/Tokyo");
    scanner::resolve_from_evidence(&fixture.conn, &tokyo.resolution, ResolveScope::PendingOnly)
        .unwrap();
    library_settings::adopt_unrecorded(&fixture.conn, &tokyo).unwrap();
    fixture
}

fn resolved_utc_ms(conn: &Connection) -> i64 {
    conn.query_row("SELECT resolved_utc_ms FROM paths", [], |row| row.get(0))
        .unwrap()
}

fn naive_as_utc_ms() -> i64 {
    chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
        .unwrap()
        .and_hms_opt(12, 34, 56)
        .unwrap()
        .and_utc()
        .timestamp_millis()
}

#[test]
fn a_changed_timezone_stays_owed_until_applied() {
    let fixture = indexed_under_tokyo();
    let policy = Policy::from_config(&json!({})).unwrap();
    let tokyo = settings(&fixture, "Asia/Tokyo");
    let utc = settings(&fixture, "UTC");
    assert!(!library_settings::owed(&fixture.conn, &tokyo, &policy).unwrap());

    // Settings saved UTC but its apply was refused as busy: nothing applied.
    assert!(library_settings::owed(&fixture.conn, &utc, &policy).unwrap());
    assert_ne!(resolved_utc_ms(&fixture.conn), naive_as_utc_ms());

    // The maintenance owner's next turn applies it once.
    assert_eq!(library_settings::apply(&fixture.conn, &utc, &policy, &|_| {}).unwrap(), 1);
    assert_eq!(resolved_utc_ms(&fixture.conn), naive_as_utc_ms());
    assert!(!library_settings::owed(&fixture.conn, &utc, &policy).unwrap());
    assert_eq!(library_settings::apply(&fixture.conn, &utc, &policy, &|_| {}).unwrap(), 0);
}

#[test]
fn a_changed_visibility_policy_is_owed_until_applied() {
    let fixture = indexed_under_tokyo();
    let tokyo = settings(&fixture, "Asia/Tokyo");
    let applied = Policy::from_config(&json!({})).unwrap();
    onecopy_lib::visibility_index::apply_policy(&fixture.conn, &applied).unwrap();
    let saved = Policy::from_config(&json!({ "ignoredFileNames": ["IMG_20160305_123456.jpg"] })).unwrap();

    assert!(library_settings::owed(&fixture.conn, &tokyo, &saved).unwrap());
    library_settings::apply(&fixture.conn, &tokyo, &saved, &|_| {}).unwrap();
    assert!(!library_settings::owed(&fixture.conn, &tokyo, &saved).unwrap());
}

#[test]
fn launch_adoption_never_replaces_recorded_settings() {
    let fixture = indexed_under_tokyo();
    let policy = Policy::from_config(&json!({})).unwrap();
    let utc = settings(&fixture, "UTC");

    // A relaunch with an apply still owed must keep it owed.
    library_settings::adopt_unrecorded(&fixture.conn, &utc).unwrap();
    assert!(library_settings::owed(&fixture.conn, &utc, &policy).unwrap());
}
