// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use std::path::PathBuf;

use onecopy_lib::storage::*;
use serial_test::serial;

fn temp_dir(label: &str) -> PathBuf {
    let dir = tempfile::Builder::new()
        .prefix(&format!("onecopy-storage-{label}-"))
        .tempdir()
        .unwrap()
        .keep();
    dir
}

#[test]
fn default_config_serializes_with_camel_case_and_expected_defaults() {
    let value = serde_json::to_value(DefaultConfig::default()).unwrap();
    assert_eq!(value["goodRangeStartYear"], serde_json::json!(1995));
    assert_eq!(value["similarityMaxGapSeconds"], serde_json::json!(90));
    assert_eq!(value["previewLongEdgePx"], serde_json::json!(1600));
    assert_eq!(value["pairingEnabled"], serde_json::json!(true));
    assert_eq!(value["verifyAfterCopy"], serde_json::json!(true));
    assert!(value["defaultTimezone"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(value["cacheDir"].is_null());
    // Spec, not configuration: extension lists (and the other dead keys)
    // are never materialized into the user-editable file.
    for absent in ["imageExtensions", "videoExtensions", "companionExtensions", "filenamePatterns", "screenPriority"] {
        assert!(value.get(absent).is_none(), "{absent} must not be seeded");
    }
}

#[test]
#[serial(backup_store)]
fn patch_merges_shallow_and_survives_interleaved_writers() {
    let dir = temp_dir("patch");
    let target = dir.join("config.json");
    write_atomic(&target, b"{\"a\": 1, \"list\": [\"x\"]}").unwrap();

    // Writer 1 patches one key; writer 2 patches another with a stale
    // mental model — neither loses the other's write.
    let after1 = patch_json_store(&target, &serde_json::json!({ "list": ["x", "y"] })).unwrap();
    assert_eq!(after1["a"], 1);
    let after2 = patch_json_store(&target, &serde_json::json!({ "b": true })).unwrap();
    assert_eq!(after2["list"], serde_json::json!(["x", "y"]));
    assert_eq!(after2["a"], 1);
    assert_eq!(after2["b"], true);

    // Null is a stored value, not a deletion.
    let after3 = patch_json_store(&target, &serde_json::json!({ "a": null })).unwrap();
    assert!(after3["a"].is_null());
    assert!(after3.get("a").is_some());

    // A missing file starts from an empty document.
    let fresh = patch_json_store(&dir.join("state.json"), &serde_json::json!({ "zoomLevel": 1.2 })).unwrap();
    assert_eq!(fresh, serde_json::json!({ "zoomLevel": 1.2 }));
}


#[test]
#[serial(backup_store)]
fn materialize_writes_only_when_absent() {
    let dir = temp_dir("materialize");
    materialize_config_if_missing(&dir).unwrap();
    let path = dir.join(CONFIG_FILE_NAME);
    let first = std::fs::read_to_string(&path).unwrap();
    assert!(first.contains("goodRangeStartYear"));

    // A user-edited file is never touched by a second materialization.
    std::fs::write(&path, "{\"custom\": true}\n").unwrap();
    materialize_config_if_missing(&dir).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"custom\": true}\n");
}


#[test]
#[serial(backup_store)]
fn write_atomic_replaces_and_leaves_no_temps() {
    let dir = temp_dir("atomic");
    let path = dir.join("f.json");
    write_atomic(&path, b"first").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
    write_atomic(&path, b"second longer contents").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second longer contents");

    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
}

