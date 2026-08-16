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

    // GENUINELY interleaved: two threads, each reading before either writes.
    // The sequential calls below cannot reach the lost update the name claims —
    // it needs overlapping read windows, and both patch_config and patch_state
    // are Tauri commands dispatched on a thread pool, so that overlap is real.
    {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = [("threadA", 1), ("threadB", 2)]
            .into_iter()
            .map(|(key, value)| {
                let target = target.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    // Line both threads up so neither can finish before the
                    // other starts.
                    barrier.wait();
                    patch_json_store(&target, &serde_json::json!({ key: value })).unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        // Read the FILE, not a return value: a lost update is a fact about
        // what landed on disk.
        let merged: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(merged["threadA"], 1, "thread A's key survived");
        assert_eq!(merged["threadB"], 2, "thread B's key survived");
        assert_eq!(merged["a"], 1, "the pre-existing document survived both");
    }

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
#[serial(backup_store, quarantine_journal)]
fn patching_corrupt_config_reseeds_before_merging() {
    let dir = temp_dir("patch-corrupt-config");
    let target = dir.join(CONFIG_FILE_NAME);
    std::fs::write(&target, b"not json").unwrap();
    let _ = drain_quarantines();

    let merged = patch_json_store(&target, &serde_json::json!({ "theme": "dark" })).unwrap();

    assert_eq!(merged["theme"], "dark");
    assert_eq!(merged["goodRangeStartYear"], 1995);
    assert_eq!(merged["sourceDirs"], serde_json::json!([]));
    assert_eq!(drain_quarantines().len(), 1);
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


// A corrupt store's recovery, end to end: branch, preserved bytes and report.
#[test]
#[serial(quarantine_journal)]
fn a_corrupt_config_is_set_aside_reported_and_reseeded_in_the_same_load() {
    let root = temp_dir("quarantine-config");
    let config = root.join("config.json");
    std::fs::write(&config, b"{ not json").unwrap();
    let _ = drain_quarantines(); // the journal is process-wide

    let loaded = load_from_root(&root).unwrap();

    // Reported: one record, naming the file and where its bytes went.
    assert_eq!(loaded.quarantines.len(), 1);
    let record = &loaded.quarantines[0];
    assert_eq!(record.file, "config.json");
    assert!(record.quarantined_to.ends_with(".invalid"), "{record:?}");

    // Preserved: the original bytes are readable at that exact path.
    assert_eq!(
        std::fs::read(&record.quarantined_to).unwrap(),
        b"{ not json",
        "the report must name the file that actually holds the bytes"
    );

    // Reseeded IN THIS LOAD: setup's materialization already ran before the
    // load, so without the re-check config.json would stay missing until
    // some later save happened to write it.
    assert!(config.is_file(), "the store comes back seeded, not absent");
    let started_with = loaded.config.expect("the app runs on the seeded defaults");
    assert_eq!(
        started_with["goodRangeStartYear"],
        serde_json::json!(1995),
        "and those defaults are the canonical ones"
    );

    // Drained: a second load reports nothing, so the notice cannot re-appear
    // for a file that is already dealt with.
    assert!(load_from_root(&root).unwrap().quarantines.is_empty());
}

#[test]
#[serial(quarantine_journal)]
fn a_corrupt_state_is_reported_without_disturbing_a_good_config() {
    let root = temp_dir("quarantine-state");
    let config = root.join("config.json");
    std::fs::write(&config, br#"{"sourceDirs": ["/photos"]}"#).unwrap();
    std::fs::write(root.join("state.json"), b"not json at all").unwrap();
    let _ = drain_quarantines();

    let loaded = load_from_root(&root).unwrap();

    assert_eq!(loaded.quarantines.len(), 1);
    assert_eq!(loaded.quarantines[0].file, "state.json");
    // Each store recovers on its own branch: one being set aside must not
    // touch, reset or re-seed its neighbour.
    assert_eq!(
        std::fs::read(&config).unwrap(),
        br#"{"sourceDirs": ["/photos"]}"#,
        "the good config is left exactly as the user left it"
    );
    assert_eq!(
        loaded.config.unwrap()["sourceDirs"],
        serde_json::json!(["/photos"])
    );
    assert!(loaded.state.is_none(), "view state starts fresh");
}
