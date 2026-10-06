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
#[serial(backup_store)]
fn appearance_reads_only_preferences_from_the_settings_in_memory() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join(CONFIG_FILE_NAME);
    let bytes = br#"{"formatVersion":1,"theme":"dark","uiFontFamily":"Iosevka","enlargeSmallImages":false,"enlargeSmallImagesInPreview":true,"videoTranscriptionEnabled":false,"audioTranscriptionEnabled":true,"sourceDirs":["/private"],"verifyAfterCopy":false}"#;
    std::fs::write(&config, bytes).unwrap();
    std::fs::write(root.path().join(STATE_FILE_NAME), b"{ invalid state").unwrap();
    assert_eq!(
        appearance_preferences(root.path()).unwrap(),
        serde_json::json!({
            "uiFontFamily": "Iosevka",
            "enlargeSmallImages": false,
            "videoTranscriptionEnabled": false,
            "audioTranscriptionEnabled": true,
        })
    );
    assert_eq!(std::fs::read(&config).unwrap(), bytes);
    assert_eq!(
        std::fs::read(root.path().join(STATE_FILE_NAME)).unwrap(),
        b"{ invalid state"
    );
    // A save reaches the next read through the held settings.
    save_config(root.path(), &serde_json::json!({ "uiFontFamily": "" })).unwrap();
    assert_eq!(appearance_preferences(root.path()).unwrap()["uiFontFamily"], "");
}

#[test]
fn default_config_serializes_with_camel_case_and_expected_defaults() {
    let value = serde_json::to_value(DefaultConfig::default()).unwrap();
    assert_eq!(value["goodRangeStartYear"], serde_json::json!(1995));
    assert_eq!(value["similarity"]["maxGapSeconds"], serde_json::json!(90));
    assert_eq!(value["previewLongEdgePx"], serde_json::json!(1600));
    assert_eq!(value["videoAutoplay"], serde_json::json!(true));
    assert_eq!(value["audioAutoplay"], serde_json::json!(true));
    assert_eq!(value["soundEnabled"], serde_json::json!(true));
    assert_eq!(value["playbackVolume"], serde_json::json!(1.0));
    assert_eq!(value["enlargeSmallImages"], serde_json::json!(true));
    assert!(value.get("enlargeSmallImagesInPreview").is_none());
    assert!(value.get("enlargeSmallImagesInQuickView").is_none());
    assert_eq!(
        value["textPreviewMaxBytes"],
        serde_json::json!(2 * 1024 * 1024)
    );
    assert_eq!(value["textFallbackEncoding"], serde_json::json!("utf-8"));
    assert_eq!(value["pairingEnabled"], serde_json::json!(true));
    assert_eq!(value["videoSnapshotsEnabled"], serde_json::json!(true));
    assert_eq!(
        value["similarPhotoAnalysisEnabled"],
        serde_json::json!(true)
    );
    assert_eq!(value["scoreFaces"], serde_json::json!(true));
    assert_eq!(value["videoTranscriptionEnabled"], serde_json::json!(true));
    assert_eq!(value["audioTranscriptionEnabled"], serde_json::json!(true));
    assert_eq!(value["aiAcceleration"]["face-scoring"], "none");
    assert_eq!(
        value["aiAcceleration"]["transcription"],
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "metal"
        } else {
            "none"
        }
    );
    assert_eq!(value["checkSourceFoldersAtLaunch"], serde_json::json!(true));
    assert_eq!(value["checkGithubReleasesAtLaunch"], serde_json::json!(true));
    assert_eq!(value["uiFontFamily"], serde_json::json!(""));
    assert!(value.get("verifyAfterCopy").is_none());
    assert_eq!(value["showFaceStars"], serde_json::json!(true));
    assert_eq!(value["maximumImagesInComparison"], serde_json::json!(16));
    assert_eq!(value["notificationDisplaySeconds"], serde_json::json!(6));
    assert_eq!(value["screenPriority"], serde_json::json!([]));
    assert_eq!(
        value["destinationConflictRenameStyle"],
        if cfg!(target_os = "windows") {
            serde_json::json!("parenthesized-number")
        } else {
            serde_json::json!("space-number")
        }
    );
    assert!(value["defaultTimezone"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(value.get("cacheDir").is_none());
    // Spec, not configuration: extension lists (and the other dead keys)
    // are never materialized into the user-editable file.
    for absent in [
        "imageExtensions",
        "videoExtensions",
        "companionExtensions",
        "filenamePatterns",
        "scenesGridColumns",
        "scenesGridRows",
    ] {
        assert!(value.get(absent).is_none(), "{absent} must not be seeded");
    }
}

#[test]
#[serial(backup_store)]
fn loading_config_removes_the_obsolete_copy_verification_preference() {
    let root = temp_dir("obsolete-copy-verification");
    let path = root.join(CONFIG_FILE_NAME);
    std::fs::write(
        &path,
        "{\"formatVersion\":1,\"verifyAfterCopy\":false,\"pairingEnabled\":true}\n",
    )
    .unwrap();

    let loaded = config(&root).unwrap();
    assert!(loaded.get("verifyAfterCopy").is_none());
    let untouched: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(untouched["verifyAfterCopy"], false);
    save_config(&root, &serde_json::json!({ "theme": "dark" })).unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored, serde_json::json!({ "formatVersion": 1, "theme": "dark" }), "a copy equal to its built-in is not kept");
}

#[test]
fn cache_is_always_managed_under_the_app_root_and_legacy_external_data_is_untouched() {
    let root = temp_dir("fixed-cache");
    let external = temp_dir("legacy-external-cache");
    let marker = external.join("keep-me.webp");
    std::fs::write(&marker, b"old cache bytes").unwrap();
    let config = serde_json::json!({ "cacheDir": external });

    let settings = onecopy_lib::scanner::settings_from_config(Some(&config), &root, 0);

    assert_eq!(settings.cache_root, root.join(CACHE_DIR_NAME));
    assert_eq!(std::fs::read(marker).unwrap(), b"old cache bytes");
}

#[test]
fn scanner_projects_the_pairing_switch() {
    let root = temp_dir("pairing-switch");
    let disabled = serde_json::json!({ "pairingEnabled": false });
    let enabled = serde_json::json!({ "pairingEnabled": true });

    assert!(!onecopy_lib::scanner::settings_from_config(Some(&disabled), &root, 0).pairing_enabled);
    assert!(onecopy_lib::scanner::settings_from_config(Some(&enabled), &root, 0).pairing_enabled);
    assert!(onecopy_lib::scanner::settings_from_config(None, &root, 0).pairing_enabled);
}

#[test]
#[serial(backup_store)]
fn patch_merges_shallow_and_survives_interleaved_writers() {
    let dir = temp_dir("patch");
    let target = dir.join("registry.json");
    write_atomic(&target, b"{\"formatVersion\": 1, \"a\": 1, \"list\": [\"x\"]}").unwrap();

    // GENUINELY interleaved: two threads, each reading before either writes.
    // The sequential calls below cannot reach the lost update the name claims —
    // it needs overlapping read windows, and patch_state saves
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
                    patch_json_store(&target, onecopy_lib::formats::STATE, &serde_json::json!({ key: value })).unwrap();
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
    let after1 = patch_json_store(&target, onecopy_lib::formats::STATE, &serde_json::json!({ "list": ["x", "y"] })).unwrap().merged;
    assert_eq!(after1["a"], 1);
    let after2 = patch_json_store(&target, onecopy_lib::formats::STATE, &serde_json::json!({ "b": true })).unwrap().merged;
    assert_eq!(after2["list"], serde_json::json!(["x", "y"]));
    assert_eq!(after2["a"], 1);
    assert_eq!(after2["b"], true);

    // Null is a stored value, not a deletion.
    let after3 = patch_json_store(&target, onecopy_lib::formats::STATE, &serde_json::json!({ "a": null })).unwrap().merged;
    assert!(after3["a"].is_null());
    assert!(after3.get("a").is_some());

    // A missing file starts from an empty document.
    let fresh = patch_json_store(
        &dir.join("state.json"),
        onecopy_lib::formats::STATE,
        &serde_json::json!({ "zoomLevel": 1.2 }),
    )
    .unwrap();
    assert_eq!(fresh.merged, serde_json::json!({ "zoomLevel": 1.2 }));
    assert!(fresh.quarantined.is_none(), "a missing file is first-run, not corruption");
}
#[test]
#[serial(backup_store)]
fn saving_over_a_corrupt_config_keeps_only_sets_that_differ() {
    let dir = temp_dir("save-corrupt-config");
    let target = dir.join(CONFIG_FILE_NAME);
    std::fs::write(&target, b"not json").unwrap();

    let outcome = save_config(&dir, &serde_json::json!({ "theme": "dark" })).unwrap();

    assert_eq!(outcome.effective["theme"], "dark");
    assert_eq!(outcome.effective["goodRangeStartYear"], 1995);
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(stored, serde_json::json!({ "formatVersion": 1, "theme": "dark" }));
    // The outcome carries the record — a mid-session quarantine has no load
    // result to ride home on, so the save itself must hand it back.
    let record = outcome.quarantined.expect("the save reports its own quarantine");
    assert_eq!(record.file, "config.json");
    assert!(record.quarantined_to.ends_with(".invalid"));
}

#[test]
#[serial(backup_store)]
fn saving_over_a_non_object_config_preserves_it_first() {
    let dir = temp_dir("save-wrong-envelope-config");
    let target = dir.join(CONFIG_FILE_NAME);
    std::fs::write(&target, b"[\"preserve\", 7]\n").unwrap();

    let outcome = save_config(&dir, &serde_json::json!({ "theme": "dark" })).unwrap();

    assert_eq!(outcome.effective["theme"], "dark");
    let record = outcome
        .quarantined
        .expect("the invalid envelope is reported by the save path");
    assert_eq!(
        std::fs::read(&record.quarantined_to).unwrap(),
        b"[\"preserve\", 7]\n",
        "the valid JSON with the wrong envelope is preserved verbatim"
    );
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn first_run_writes_nothing_and_a_save_writes_every_set_that_differs() {
    let dir = temp_dir("sparse");
    let loaded = load_from_root(&dir).unwrap();
    let path = dir.join(CONFIG_FILE_NAME);
    assert!(!path.exists());
    assert_eq!(loaded.config["previewLongEdgePx"], 1600);
    let stored = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    save_config(&dir, &serde_json::json!({ "previewLongEdgePx": 2000 })).unwrap();
    assert_eq!(stored(&path), serde_json::json!({ "formatVersion": 1, "previewLongEdgePx": 2000 }));
    assert_eq!(load_from_root(&dir).unwrap().config["theme"], "system");
    // Unknown keys are dropped, and a set saved equal to its built-in leaves.
    std::fs::write(&path, br#"{"formatVersion":1,"version":1,"previewLongEdgePx":2000}"#).unwrap();
    load_from_root(&dir).unwrap();
    save_config(&dir, &serde_json::json!({ "theme": "dark", "playbackVolume": 1 })).unwrap();
    assert_eq!(stored(&path), serde_json::json!({ "formatVersion": 1, "previewLongEdgePx": 2000, "theme": "dark" }));
    save_config(&dir, &serde_json::json!({ "previewLongEdgePx": 1600, "theme": "system" })).unwrap();
    assert_eq!(stored(&path), serde_json::json!({ "formatVersion": 1 }), "the file stays, holding no set");
    assert!(save_config(&dir, &serde_json::json!({ "theme": "purple" })).is_err());
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn resetting_similarity_removes_the_whole_set_from_the_file() {
    let dir = temp_dir("reset-set");
    let path = dir.join(CONFIG_FILE_NAME);
    let similarity = serde_json::json!({ "maxGapSeconds": 12, "phashMaxDistance": 19, "phashMaxDistanceBurst": 27, "diameterMultiplier": 4 });
    save_config(&dir, &serde_json::json!({ "similarity": similarity, "theme": "dark" })).unwrap();
    assert_eq!(load_from_root(&dir).unwrap().config["similarity"], similarity);
    let result = save_config(&dir, &serde_json::json!({ "similarity": effective_config(None)["similarity"] })).unwrap();
    assert_eq!(result.effective["similarity"], effective_config(None)["similarity"]);
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored, serde_json::json!({ "formatVersion": 1, "theme": "dark" }));
}

#[test]
fn malformed_sets_fall_back_whole_without_reinterpreting_legacy_keys() {
    let stored = serde_json::json!({ "similarity": { "maxGapSeconds": 12 }, "aiAcceleration": { "transcription": "cuda", "face-scoring": "none" }, "theme": "invalid", "sourceDirs": [12], "similarityMaxGapSeconds": 12 });
    assert_eq!(effective_config(Some(&stored)), effective_config(None));
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
fn a_corrupt_config_is_set_aside_reported_and_not_reseeded() {
    let root = temp_dir("quarantine-config");
    let config = root.join("config.json");
    std::fs::write(&config, b"{ not json").unwrap();

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

    // Reading built-ins does not write a replacement.
    assert!(!config.exists(), "quarantine leaves no replacement config");
    let started_with = loaded.config;
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
    std::fs::write(&config, br#"{"formatVersion": 1, "sourceDirs": ["/photos"]}"#).unwrap();
    std::fs::write(root.join("state.json"), b"not json at all").unwrap();

    let loaded = load_from_root(&root).unwrap();

    assert_eq!(loaded.quarantines.len(), 1);
    assert_eq!(loaded.quarantines[0].file, "state.json");
    // Each store recovers on its own branch: one being set aside must not
    // touch, reset or re-seed its neighbour.
    assert_eq!(
        std::fs::read(&config).unwrap(),
        br#"{"formatVersion": 1, "sourceDirs": ["/photos"]}"#,
        "the good config is left exactly as the user left it"
    );
    assert_eq!(
        loaded.config["sourceDirs"],
        serde_json::json!(["/photos"])
    );
    assert!(loaded.state.is_none(), "view state starts fresh");
}

// Theme and language are read from the held settings before the window
// shows, some of it before this process owns the instance lock, so that read
// never sets a store aside: the lock owner's load does, and reports it.
#[test]
#[serial(quarantine_journal)]
fn the_settings_held_before_the_window_shows_never_touch_the_file() {
    let root = temp_dir("held-before-window");
    let config = root.join(CONFIG_FILE_NAME);
    std::fs::write(&config, br#"{"formatVersion":1,"theme":"light","language":"ko"}"#).unwrap();
    let held = held_config(&root);
    assert_eq!((held["theme"].as_str(), held["language"].as_str()), (Some("light"), Some("ko")));

    let corrupt = temp_dir("held-before-window-corrupt");
    std::fs::write(corrupt.join(CONFIG_FILE_NAME), b"{corrupt").unwrap();
    assert_eq!(held_config(&corrupt), effective_config(None));
    assert_eq!(std::fs::read(corrupt.join(CONFIG_FILE_NAME)).unwrap(), b"{corrupt");
    let loaded = load_from_root(&corrupt).unwrap();
    assert_eq!(loaded.quarantines.len(), 1, "the owner's load sets it aside and reports it");
}

// A worker can be the first to read the settings for a root. Its read sets a
// corrupt store aside like the startup load, and it does not drain the
// pending list, so the record waits for Main's `load_from_root` to report.
#[test]
#[serial(quarantine_journal)]
fn a_settings_read_that_loads_leaves_its_quarantine_pending_for_load_from_root() {
    let root = temp_dir("quarantine-worker-read");
    let config_path = root.join("config.json");
    std::fs::write(&config_path, b"{ not json").unwrap();

    assert_eq!(*config(&root).unwrap(), effective_config(None));
    assert!(!config_path.exists(), "no replacement is seeded");

    let loaded = load_from_root(&root).unwrap();
    assert_eq!(
        loaded.quarantines.len(),
        1,
        "the worker's read must not drain the quarantine meant for Main"
    );
    assert_eq!(loaded.quarantines[0].file, "config.json");
}

#[test]
fn the_effective_config_is_the_stored_values_over_the_defaults() {
    let defaults = effective_config(None);
    assert_eq!(defaults, serde_json::to_value(DefaultConfig::default()).unwrap());
    // A config written before a key existed resolves that key to its default:
    // new installations confirm a direct single-item Delete (file-operations.md).
    let older = serde_json::json!({ "sourceDirs": ["/photos"], "videoAutoplay": false });
    let effective = effective_config(Some(&older));
    assert_eq!(effective["confirmTrashDelete"], serde_json::json!(true));
    assert_eq!(effective["videoAutoplay"], serde_json::json!(false));
    assert_eq!(effective["sourceDirs"], serde_json::json!(["/photos"]));
    assert_eq!(effective_config(Some(&serde_json::json!([]))), defaults);
}

/// The frontend suite reads its configuration from this fixture, so its tests
/// run on the core's defaults instead of a table of their own. The fields a
/// machine or platform decides are pinned to fixed values there.
#[test]
fn the_frontend_config_fixture_matches_the_core_defaults() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/effective-config.json");
    let mut expected = effective_config(None);
    for (key, value) in [
        ("defaultTimezone", serde_json::json!("UTC")),
        ("destinationConflictRenameStyle", serde_json::json!("space-number")),
        ("aiAcceleration", serde_json::json!({})),
    ] {
        expected[key] = value;
    }
    let fixture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        fixture, expected,
        "regenerate tests/fixtures/effective-config.json from DefaultConfig"
    );
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn every_json_store_records_its_format_version_and_reads_without_it() {
    let dir = temp_dir("format-version");
    save_config(&dir, &serde_json::json!({ "theme": "dark" })).unwrap();
    patch_json_store(&dir.join(STATE_FILE_NAME), onecopy_lib::formats::STATE, &serde_json::json!({ "zoomLevel": 1.2 })).unwrap();
    save_window_state(&dir, &serde_json::json!({ "x": 1 })).unwrap();
    save_preview_window_state(&dir, &serde_json::json!({ "x": 2 })).unwrap();
    save_records_window_state(&dir, &serde_json::json!({ "x": 3 })).unwrap();
    for name in [CONFIG_FILE_NAME, STATE_FILE_NAME, WINDOW_FILE_NAME, PREVIEW_WINDOW_FILE_NAME, RECORDS_WINDOW_FILE_NAME] {
        let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap();
        assert_eq!(stored["formatVersion"], 1, "{name}");
    }
    assert_eq!(read_state(&dir).unwrap().0, Some(serde_json::json!({ "zoomLevel": 1.2 })));
    assert_eq!(read_window_state_for_setup(&dir).unwrap(), Some(serde_json::json!({ "x": 1 })));
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn a_json_store_without_its_marker_is_unreadable_and_set_aside() {
    let dir = temp_dir("unmarked");
    std::fs::write(dir.join(STATE_FILE_NAME), br#"{"zoomLevel":1.5}"#).unwrap();
    std::fs::write(dir.join(CONFIG_FILE_NAME), br#"{"theme":"dark"}"#).unwrap();
    assert_eq!(config_newer(&dir).unwrap(), None);
    let loaded = load_from_root(&dir).unwrap();
    assert_eq!(loaded.state, None);
    assert_eq!(loaded.config["theme"], "system", "nothing is inferred from the shape");
    let mut files: Vec<_> = loaded.quarantines.iter().map(|record| record.file.as_str()).collect();
    files.sort_unstable();
    assert_eq!(files, ["config.json", "state.json"]);
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn settings_written_by_a_newer_onecopy_stop_the_load_by_name_and_are_left_as_they_are() {
    let dir = temp_dir("newer-config");
    let path = dir.join(CONFIG_FILE_NAME);
    let bytes = br#"{"formatVersion":2,"theme":"dark"}"#;
    std::fs::write(&path, bytes).unwrap();
    let newer = config_newer(&dir).unwrap().expect("a newer settings file is named");
    assert_eq!((newer.file.as_str(), newer.version, newer.supported), ("config.json", 2, 1));
    assert_eq!(held_config(&dir)["theme"], "system", "the built-ins answer before the load");
    let error = config(&dir).expect_err("a newer settings file is not loaded");
    assert!(error.contains("newer"), "{error}");
    assert!(save_config(&dir, &serde_json::json!({ "theme": "light" })).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "never quarantined, reset or written to");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing is set aside");
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn volatile_state_written_by_a_newer_onecopy_reads_as_absent_and_is_not_saved_over() {
    let dir = temp_dir("newer-state");
    let state = br#"{"formatVersion":2,"zoomLevel":1.5}"#;
    let window = br#"{"formatVersion":2,"x":1}"#;
    std::fs::write(dir.join(STATE_FILE_NAME), state).unwrap();
    std::fs::write(dir.join(WINDOW_FILE_NAME), window).unwrap();
    let loaded = load_from_root(&dir).unwrap();
    assert_eq!(loaded.state, None);
    assert!(loaded.quarantines.is_empty(), "a newer store is not quarantined");
    assert_eq!(read_window_state_for_setup(&dir).unwrap(), None);
    assert!(patch_json_store(&dir.join(STATE_FILE_NAME), onecopy_lib::formats::STATE, &serde_json::json!({ "zoomLevel": 1.0 })).is_err());
    assert!(save_window_state(&dir, &serde_json::json!({ "x": 2 })).is_err());
    assert_eq!(std::fs::read(dir.join(STATE_FILE_NAME)).unwrap(), state);
    assert_eq!(std::fs::read(dir.join(WINDOW_FILE_NAME)).unwrap(), window);
}
