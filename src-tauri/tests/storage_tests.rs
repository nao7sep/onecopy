// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).


use onecopy_lib::formats;
use onecopy_lib::storage::*;
use serial_test::serial;

fn temp_dir(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("onecopy-storage-{label}-"))
        .tempdir()
        .unwrap()
}

/// A saved settings file holding `sets`, beside its format marker and the
/// interpretation zone every save writes (the computer's zone when none was
/// chosen).
fn saved_file(sets: serde_json::Value) -> serde_json::Value {
    let mut file = serde_json::json!({ "defaultTimezone": computer_time_zone().name() });
    file[formats::JSON_KEY] = formats::CONFIG.into();
    file.as_object_mut().unwrap().extend(sets.as_object().unwrap().clone());
    file
}

/// The built-in value of one setting.
fn builtin(key: &str) -> serde_json::Value {
    effective_config(None)[key].clone()
}

#[test]
#[serial(backup_store)]
fn appearance_reads_only_preferences_from_the_settings_in_memory() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join(CONFIG_FILE_NAME);
    let bytes = br#"{"formatVersion":1,"theme":"dark","uiFontFamily":"Iosevka","enlargeSmallImages":false,"videoTranscriptionEnabled":false,"audioTranscriptionEnabled":true,"sourceDirs":["/private"],"verifyAfterCopy":false}"#;
    std::fs::write(&config, bytes).unwrap();
    std::fs::write(root.path().join(STATE_FILE_NAME), b"{ invalid state").unwrap();
    assert_eq!(
        appearance_preferences(root.path()).unwrap(),
        serde_json::json!({
            "uiFontFamily": "Iosevka",
            "enlargeSmallImages": false,
            "videoTranscriptionEnabled": false,
            "audioTranscriptionEnabled": true,
            "displayTimezone": builtin("displayTimezone"),
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
#[serial(backup_store, quarantine_journal)]
fn default_config_serializes_with_camel_case_keys_and_values_the_settings_accept() {
    let value = serde_json::to_value(DefaultConfig::default()).unwrap();
    // camelCase, the spelling the frontend reads the settings by.
    for key in value.as_object().unwrap().keys() {
        assert!(
            key.starts_with(|c: char| c.is_ascii_lowercase()) && key.chars().all(|c| c.is_ascii_alphanumeric()),
            "{key} is not camelCase"
        );
    }
    // Every built-in is a value Settings itself would save.
    let dir_owner = temp_dir("defaults-accepted");
    save_config(dir_owner.path(), &value).expect("every built-in passes the settings' own validation");
    assert!(value.get("verifyAfterCopy").is_none());
    // The interpretation zone's built-in is the computer's zone as a concrete
    // name, whatever that zone is.
    assert_eq!(value["defaultTimezone"], computer_time_zone().name());
    assert_ne!(value["defaultTimezone"], "system");
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
    let root_owner = temp_dir("obsolete-copy-verification");
    let root = root_owner.path();
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
    assert_eq!(stored, saved_file(serde_json::json!({ "theme": "dark" })), "a copy equal to its built-in is not kept");
}

#[test]
fn cache_is_always_managed_under_the_app_root_and_legacy_external_data_is_untouched() {
    let root_owner = temp_dir("fixed-cache");
    let root = root_owner.path();
    let external_owner = temp_dir("legacy-external-cache");
    let external = external_owner.path();
    let marker = external.join("keep-me.webp");
    std::fs::write(&marker, b"old cache bytes").unwrap();
    let config = serde_json::json!({ "cacheDir": external });

    let settings = onecopy_lib::scanner::settings_from_config(Some(&config), &root, 0);

    assert_eq!(settings.cache_root, root.join(CACHE_DIR_NAME));
    assert_eq!(std::fs::read(marker).unwrap(), b"old cache bytes");
}

#[test]
fn scanner_ignores_the_retired_pairing_switch() {
    let root_owner = temp_dir("pairing-switch");
    let root = root_owner.path();
    let disabled = serde_json::json!({ "pairingEnabled": false });
    let enabled = serde_json::json!({ "pairingEnabled": true });

    assert!(onecopy_lib::scanner::settings_from_config(Some(&disabled), &root, 0).pairing_enabled);
    assert!(onecopy_lib::scanner::settings_from_config(Some(&enabled), &root, 0).pairing_enabled);
    assert!(onecopy_lib::scanner::settings_from_config(None, &root, 0).pairing_enabled);
}

#[test]
#[serial(backup_store)]
fn patch_merges_shallow_and_survives_interleaved_writers() {
    let dir_owner = temp_dir("patch");
    let dir = dir_owner.path();
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
        let joined: Vec<_> = handles.into_iter().map(|handle| handle.join()).collect();
        assert!(joined.into_iter().all(|result| result.is_ok()));
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
    let dir_owner = temp_dir("save-corrupt-config");
    let dir = dir_owner.path();
    let target = dir.join(CONFIG_FILE_NAME);
    std::fs::write(&target, b"not json").unwrap();

    let outcome = save_config(&dir, &serde_json::json!({ "theme": "dark" })).unwrap();

    assert_eq!(outcome.effective["theme"], "dark");
    assert_eq!(outcome.effective["similarPhotoGrouping"], builtin("similarPhotoGrouping"));
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(stored, saved_file(serde_json::json!({ "theme": "dark" })));
    // The outcome carries the record — a mid-session quarantine has no load
    // result to ride home on, so the save itself must hand it back.
    let record = outcome.quarantined.expect("the save reports its own quarantine");
    assert_eq!(record.file, CONFIG_FILE_NAME);
    assert!(record.quarantined_to.ends_with(".invalid"));
}

#[test]
#[serial(backup_store)]
fn saving_over_a_non_object_config_preserves_it_first() {
    let dir_owner = temp_dir("save-wrong-envelope-config");
    let dir = dir_owner.path();
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
    let dir_owner = temp_dir("sparse");
    let dir = dir_owner.path();
    let loaded = load_from_root(&dir).unwrap();
    let path = dir.join(CONFIG_FILE_NAME);
    assert!(!path.exists());
    assert_eq!(loaded.config["notificationDisplaySeconds"], builtin("notificationDisplaySeconds"));
    let stored = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    save_config(&dir, &serde_json::json!({ "notificationDisplaySeconds": 10 })).unwrap();
    assert_eq!(stored(&path), saved_file(serde_json::json!({ "notificationDisplaySeconds": 10 })));
    assert_eq!(load_from_root(&dir).unwrap().config["theme"], builtin("theme"));
    // Unknown keys are dropped, and a set saved equal to its built-in leaves.
    std::fs::write(&path, br#"{"formatVersion":1,"version":1,"notificationDisplaySeconds":10}"#).unwrap();
    load_from_root(&dir).unwrap();
    save_config(&dir, &serde_json::json!({ "theme": "dark", "playbackVolume": builtin("playbackVolume") })).unwrap();
    assert_eq!(stored(&path), saved_file(serde_json::json!({ "notificationDisplaySeconds": 10, "theme": "dark" })));
    save_config(&dir, &serde_json::json!({ "notificationDisplaySeconds": builtin("notificationDisplaySeconds"), "theme": builtin("theme") })).unwrap();
    assert_eq!(stored(&path), saved_file(serde_json::json!({})), "the file stays, holding only the zone");
    assert!(save_config(&dir, &serde_json::json!({ "theme": "purple" })).is_err());
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn every_save_writes_the_interpretation_zone_even_while_it_equals_the_computers_zone() {
    let dir_owner = temp_dir("zone-kept");
    let dir = dir_owner.path();
    let path = dir.join(CONFIG_FILE_NAME);
    let stored = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    };
    // The built-in is the computer's zone now, and Settings' defaults say so.
    let computer = computer_time_zone().name().to_string();
    let loaded = load_from_root(&dir).unwrap();
    assert!(!path.exists(), "first run writes nothing");
    assert_eq!(loaded.config["defaultTimezone"], computer.as_str());
    assert_eq!(loaded.config_defaults["defaultTimezone"], computer.as_str());
    // A save that does not name the zone pins the one dates are read in now.
    save_config(&dir, &serde_json::json!({ "notificationDisplaySeconds": 10 })).unwrap();
    assert_eq!(stored(&path)["defaultTimezone"], computer.as_str());
    // A chosen zone stays through later saves and the next launch's read, so
    // a move of the computer does not re-date the library.
    save_config(&dir, &serde_json::json!({ "defaultTimezone": "Pacific/Auckland" })).unwrap();
    save_config(&dir, &serde_json::json!({ "notificationDisplaySeconds": builtin("notificationDisplaySeconds") })).unwrap();
    let mut chosen = serde_json::json!({ "defaultTimezone": "Pacific/Auckland" });
    chosen[formats::JSON_KEY] = formats::CONFIG.into();
    assert_eq!(stored(&path), chosen);
    let relaunched_owner = temp_dir("zone-kept-relaunch");
    let relaunched = relaunched_owner.path();
    std::fs::copy(&path, relaunched.join(CONFIG_FILE_NAME)).unwrap();
    save_config(relaunched, &serde_json::json!({ "theme": "dark" })).unwrap();
    assert_eq!(stored(&relaunched.join(CONFIG_FILE_NAME))["defaultTimezone"], "Pacific/Auckland");
    // System stays a choice of its own.
    save_config(&dir, &serde_json::json!({ "defaultTimezone": "system" })).unwrap();
    assert_eq!(stored(&path)["defaultTimezone"], "system");
    assert_eq!(load_from_root(&dir).unwrap().config["defaultTimezone"], "system");
    assert!(save_config(&dir, &serde_json::json!({ "defaultTimezone": "Mars/Olympus" })).is_err());
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn the_display_zone_is_system_unless_a_known_zone_is_saved() {
    let dir_owner = temp_dir("display-zone");
    let dir = dir_owner.path();
    let path = dir.join(CONFIG_FILE_NAME);
    let system = builtin("displayTimezone");
    assert_eq!(load_from_root(&dir).unwrap().config["displayTimezone"], system);
    save_config(&dir, &serde_json::json!({ "displayTimezone": "Asia/Tokyo" })).unwrap();
    assert_eq!(load_from_root(&dir).unwrap().config["displayTimezone"], "Asia/Tokyo");
    save_config(&dir, &serde_json::json!({ "displayTimezone": "system" })).unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored, saved_file(serde_json::json!({})), "System is the built-in");
    assert!(save_config(&dir, &serde_json::json!({ "displayTimezone": "Mars/Olympus" })).is_err());
    // An unknown or wrong-shaped stored value reads as System.
    for value in [serde_json::json!("Mars/Olympus"), serde_json::json!(9)] {
        let effective = effective_config(Some(&serde_json::json!({ "displayTimezone": value })));
        assert_eq!(effective["displayTimezone"], system, "{value}");
    }
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn resetting_similarity_removes_the_whole_set_from_the_file() {
    let dir_owner = temp_dir("reset-set");
    let dir = dir_owner.path();
    let path = dir.join(CONFIG_FILE_NAME);
    save_config(&dir, &serde_json::json!({ "similarPhotoGrouping": "looser", "theme": "dark" })).unwrap();
    assert_eq!(load_from_root(&dir).unwrap().config["similarPhotoGrouping"], "looser");
    let result = save_config(&dir, &serde_json::json!({ "similarPhotoGrouping": builtin("similarPhotoGrouping") })).unwrap();
    assert_eq!(result.effective["similarPhotoGrouping"], builtin("similarPhotoGrouping"));
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(stored, saved_file(serde_json::json!({ "theme": "dark" })));
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn ignored_file_names_are_saved_trimmed_and_leave_the_file_when_they_equal_the_built_in_as_a_set() {
    let dir_owner = temp_dir("ignored-names");
    let dir = dir_owner.path();
    let path = dir.join(CONFIG_FILE_NAME);
    let stored = || -> serde_json::Value { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };
    let saved = save_config(&dir, &serde_json::json!({ "ignoredFileNames": [" .DS_Store ", "", "   ", "local.ini\t"] })).unwrap();
    assert_eq!(saved.effective["ignoredFileNames"], serde_json::json!([".DS_Store", "local.ini"]));
    assert_eq!(stored(), saved_file(serde_json::json!({ "ignoredFileNames": [".DS_Store", "local.ini"] })));
    // Reordered, with stray whitespace, a blank row and a repeat in another
    // case: the built-in.
    let names: Vec<String> = builtin("ignoredFileNames")
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_string())
        .collect();
    let mut reordered: Vec<String> = names.iter().rev().map(|name| format!(" {name}\t")).collect();
    reordered.insert(1, String::new());
    reordered.push(names[0].to_uppercase());
    let saved = save_config(&dir, &serde_json::json!({ "ignoredFileNames": reordered })).unwrap();
    assert_eq!(stored(), saved_file(serde_json::json!({})));
    assert_eq!(saved.effective["ignoredFileNames"], effective_config(None)["ignoredFileNames"]);
    assert!(save_config(&dir, &serde_json::json!({ "ignoredFileNames": ["ok.txt", ".."] })).is_err());
    assert_eq!(stored(), saved_file(serde_json::json!({})), "a refused save writes nothing");
}

#[test]
fn a_stored_ignored_name_list_is_held_cleaned_or_falls_back_to_the_built_in() {
    let builtin = effective_config(None)["ignoredFileNames"].clone();
    let held = |names: serde_json::Value| effective_config(Some(&serde_json::json!({ "ignoredFileNames": names })))["ignoredFileNames"].clone();
    assert_eq!(held(serde_json::json!([" local.ini ", ""])), serde_json::json!(["local.ini"]));
    for invalid in [serde_json::json!(["local.ini", "dir/file"]), serde_json::json!(["."]), serde_json::json!("local.ini")] {
        assert_eq!(held(invalid.clone()), builtin, "{invalid}");
    }
}

#[test]
fn retired_settings_cannot_change_runtime_behavior() {
    let legacy = serde_json::json!({
        "videoAutoplay": false, "audioAutoplay": false, "showFaceStars": false,
        "previewLongEdgePx": 12, "thumbnailEdgePx": 1, "videoStripSecondsPerFrame": 1,
        "videoStripMinFrames": 100, "videoStripMaxFrames": 100, "goodRangeStartYear": 2020,
        "textPreviewMaxBytes": 1, "checkGithubReleasesAtLaunch": false, "pairingEnabled": false, "destinationConflictRenameStyle": "other",
        "similarity": { "maxGapSeconds": 1, "phashMaxDistance": 99, "phashMaxDistanceBurst": 99, "diameterMultiplier": 99 }
    });
    assert_eq!(effective_config(Some(&legacy)), effective_config(None));
    let root_owner = temp_dir("retired-config");
    let root = root_owner.path();
    // The retired keys leave the runtime exactly where no configuration does.
    let derived = onecopy_lib::derived_work::settings_from_config(Some(&legacy), &root);
    let unconfigured = onecopy_lib::derived_work::settings_from_config(None, &root);
    assert_eq!(derived.preview_long_edge, unconfigured.preview_long_edge);
    assert_eq!(derived.thumb_edge, unconfigured.thumb_edge);
    assert_eq!(derived.strip.seconds_per_frame, unconfigured.strip.seconds_per_frame);
    assert_eq!(derived.similarity.phash_max_distance, unconfigured.similarity.phash_max_distance);
    let scan = onecopy_lib::scanner::settings_from_config(Some(&legacy), &root, 0);
    assert!(scan.pairing_enabled);
    assert_eq!(
        scan.resolution.good_range_start_year,
        onecopy_lib::scanner::settings_from_config(None, &root, 0).resolution.good_range_start_year
    );
}

#[test]
fn malformed_sets_fall_back_whole_without_reinterpreting_legacy_keys() {
    let stored = serde_json::json!({ "similarity": { "maxGapSeconds": 12 }, "aiAcceleration": { "transcription": "cuda", "face-scoring": "none" }, "theme": "invalid", "sourceDirs": [12], "similarityMaxGapSeconds": 12 });
    assert_eq!(effective_config(Some(&stored)), effective_config(None));
}

#[test]
#[serial(backup_store)]
fn identical_atomic_writes_leave_the_existing_file_and_times_untouched() {
    for recorded in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unchanged.txt");
        std::fs::write(&path, b"same exact bytes").unwrap();
        let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        std::fs::File::options().write(true).open(&path).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(past)).unwrap();
        let before = std::fs::metadata(&path).unwrap();

        if recorded {
            write_atomic(&path, b"same exact bytes").unwrap();
        } else {
            write_atomic_unrecorded(&path, b"same exact bytes").unwrap();
        }

        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(after.modified().unwrap(), before.modified().unwrap());
        #[cfg(unix)] {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(after.ino(), before.ino());
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
#[cfg(unix)]
fn changed_atomic_writes_preserve_permissions_but_get_a_new_modified_time() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restricted.txt");
    std::fs::write(&path, b"old").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    std::fs::File::options().write(true).open(&path).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(past)).unwrap();

    write_atomic_unrecorded(&path, b"changed").unwrap();

    let metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o640);
    assert!(metadata.modified().unwrap() > past);
    assert_eq!(std::fs::read(&path).unwrap(), b"changed");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
#[serial(backup_store)]
fn write_atomic_replaces_and_leaves_no_temps() {
    let dir_owner = temp_dir("atomic");
    let dir = dir_owner.path();
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
    let root_owner = temp_dir("quarantine-config");
    let root = root_owner.path();
    let config = root.join(CONFIG_FILE_NAME);
    std::fs::write(&config, b"{ not json").unwrap();

    let loaded = load_from_root(&root).unwrap();

    // Reported: one record, naming the file and where its bytes went.
    assert_eq!(loaded.quarantines.len(), 1);
    let record = &loaded.quarantines[0];
    assert_eq!(record.file, CONFIG_FILE_NAME);
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
        started_with["similarPhotoGrouping"],
        builtin("similarPhotoGrouping"),
        "and those defaults are the canonical ones"
    );

    // Drained: a second load reports nothing, so the notice cannot re-appear
    // for a file that is already dealt with.
    assert!(load_from_root(&root).unwrap().quarantines.is_empty());
}

#[test]
#[serial(quarantine_journal)]
fn a_corrupt_state_is_reported_without_disturbing_a_good_config() {
    let root_owner = temp_dir("quarantine-state");
    let root = root_owner.path();
    let config = root.join(CONFIG_FILE_NAME);
    std::fs::write(&config, br#"{"formatVersion": 1, "sourceDirs": ["/photos"]}"#).unwrap();
    std::fs::write(root.join(STATE_FILE_NAME), b"not json at all").unwrap();

    let loaded = load_from_root(&root).unwrap();

    assert_eq!(loaded.quarantines.len(), 1);
    assert_eq!(loaded.quarantines[0].file, STATE_FILE_NAME);
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
    let root_owner = temp_dir("held-before-window");
    let root = root_owner.path();
    let config = root.join(CONFIG_FILE_NAME);
    std::fs::write(&config, br#"{"formatVersion":1,"theme":"light","language":"ko"}"#).unwrap();
    let held = held_config(&root);
    assert_eq!((held["theme"].as_str(), held["language"].as_str()), (Some("light"), Some("ko")));

    let corrupt_owner = temp_dir("held-before-window-corrupt");
    let corrupt = corrupt_owner.path();
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
    let root_owner = temp_dir("quarantine-worker-read");
    let root = root_owner.path();
    let config_path = root.join(CONFIG_FILE_NAME);
    std::fs::write(&config_path, b"{ not json").unwrap();

    assert_eq!(*config(&root).unwrap(), effective_config(None));
    assert!(!config_path.exists(), "no replacement is seeded");

    let loaded = load_from_root(&root).unwrap();
    assert_eq!(
        loaded.quarantines.len(),
        1,
        "the worker's read must not drain the quarantine meant for Main"
    );
    assert_eq!(loaded.quarantines[0].file, CONFIG_FILE_NAME);
}

#[test]
fn the_effective_config_is_the_stored_values_over_the_defaults() {
    let defaults = effective_config(None);
    assert_eq!(defaults, serde_json::to_value(DefaultConfig::default()).unwrap());
    // A config written before a key existed resolves that key to its default.
    let older = serde_json::json!({ "sourceDirs": ["/photos"], "autoplay": false });
    let effective = effective_config(Some(&older));
    assert_eq!(effective["confirmTrashDelete"], defaults["confirmTrashDelete"]);
    assert_eq!(effective["autoplay"], serde_json::json!(false));
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
    expected["aiAcceleration"] = serde_json::json!({});
    expected["defaultTimezone"] = serde_json::json!("UTC");
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
    let dir_owner = temp_dir("format-version");
    let dir = dir_owner.path();
    save_config(&dir, &serde_json::json!({ "theme": "dark" })).unwrap();
    patch_json_store(&dir.join(STATE_FILE_NAME), onecopy_lib::formats::STATE, &serde_json::json!({ "zoomLevel": 1.2 })).unwrap();
    save_window_state(&dir, &serde_json::json!({ "x": 1 })).unwrap();
    save_preview_window_state(&dir, &serde_json::json!({ "x": 2 })).unwrap();
    save_records_window_state(&dir, &serde_json::json!({ "x": 3 })).unwrap();
    for (name, version) in [
        (CONFIG_FILE_NAME, formats::CONFIG),
        (STATE_FILE_NAME, formats::STATE),
        (WINDOW_FILE_NAME, formats::WINDOW_PLACEMENT),
        (PREVIEW_WINDOW_FILE_NAME, formats::WINDOW_PLACEMENT),
        (RECORDS_WINDOW_FILE_NAME, formats::WINDOW_PLACEMENT),
    ] {
        let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap();
        assert_eq!(stored[formats::JSON_KEY], version, "{name}");
    }
    assert_eq!(read_state(&dir).unwrap().0, Some(serde_json::json!({ "zoomLevel": 1.2 })));
    assert_eq!(read_window_state_for_setup(&dir).unwrap(), Some(serde_json::json!({ "x": 1 })));
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn a_json_store_without_its_marker_is_unreadable_and_set_aside() {
    let dir_owner = temp_dir("unmarked");
    let dir = dir_owner.path();
    std::fs::write(dir.join(STATE_FILE_NAME), br#"{"zoomLevel":1.5}"#).unwrap();
    std::fs::write(dir.join(CONFIG_FILE_NAME), br#"{"theme":"dark"}"#).unwrap();
    assert_eq!(config_newer(&dir).unwrap(), None);
    let loaded = load_from_root(&dir).unwrap();
    assert_eq!(loaded.state, None);
    assert_eq!(loaded.config["theme"], builtin("theme"), "nothing is inferred from the shape");
    let mut files: Vec<_> = loaded.quarantines.iter().map(|record| record.file.as_str()).collect();
    files.sort_unstable();
    let mut expected = [CONFIG_FILE_NAME, STATE_FILE_NAME];
    expected.sort_unstable();
    assert_eq!(files, expected);
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn settings_written_by_a_newer_onecopy_stop_the_load_by_name_and_are_left_as_they_are() {
    let dir_owner = temp_dir("newer-config");
    let dir = dir_owner.path();
    let path = dir.join(CONFIG_FILE_NAME);
    let mut newer_file = serde_json::json!({ "theme": "dark" });
    newer_file[formats::JSON_KEY] = (formats::CONFIG + 1).into();
    let bytes = newer_file.to_string().into_bytes();
    std::fs::write(&path, &bytes).unwrap();
    let newer = config_newer(&dir).unwrap().expect("a newer settings file is named");
    assert_eq!(
        (newer.file.as_str(), newer.version, newer.supported),
        (CONFIG_FILE_NAME, formats::CONFIG + 1, formats::CONFIG)
    );
    assert_eq!(held_config(&dir)["theme"], builtin("theme"), "the built-ins answer before the load");
    let error = config(&dir).expect_err("a newer settings file is not loaded");
    assert_eq!(error, newer.to_string(), "the load stops on the store that names the file");
    assert!(save_config(&dir, &serde_json::json!({ "theme": "light" })).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes, "never quarantined, reset or written to");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing is set aside");
}

#[test]
#[serial(backup_store, quarantine_journal)]
fn volatile_state_written_by_a_newer_onecopy_reads_as_absent_and_is_not_saved_over() {
    let dir_owner = temp_dir("newer-state");
    let dir = dir_owner.path();
    let newer = |version: i64, mut fields: serde_json::Value| {
        fields[formats::JSON_KEY] = (version + 1).into();
        fields.to_string().into_bytes()
    };
    let state = newer(formats::STATE, serde_json::json!({ "zoomLevel": 1.5 }));
    let window = newer(formats::WINDOW_PLACEMENT, serde_json::json!({ "x": 1 }));
    std::fs::write(dir.join(STATE_FILE_NAME), &state).unwrap();
    std::fs::write(dir.join(WINDOW_FILE_NAME), &window).unwrap();
    let loaded = load_from_root(&dir).unwrap();
    assert_eq!(loaded.state, None);
    assert!(loaded.quarantines.is_empty(), "a newer store is not quarantined");
    assert_eq!(read_window_state_for_setup(&dir).unwrap(), None);
    assert!(patch_json_store(&dir.join(STATE_FILE_NAME), onecopy_lib::formats::STATE, &serde_json::json!({ "zoomLevel": 1.0 })).is_err());
    assert!(save_window_state(&dir, &serde_json::json!({ "x": 2 })).is_err());
    assert_eq!(std::fs::read(dir.join(STATE_FILE_NAME)).unwrap(), state);
    assert_eq!(std::fs::read(dir.join(WINDOW_FILE_NAME)).unwrap(), window);
}

#[test]
#[serial(quarantine_journal)]
fn malformed_optional_section_is_absent_at_both_state_read_boundaries() {
    let root = temp_dir("optional-section");
    for section in [serde_json::Value::Null, serde_json::json!(42),
        serde_json::json!({"kind":"image"}),
        serde_json::json!({"kind":"unknown","month":"2026-10"})] {
        let value = serde_json::json!({"formatVersion":1,"lastSection":section,"zoomLevel":1.2});
        std::fs::write(root.path().join(STATE_FILE_NAME), serde_json::to_vec(&value).unwrap()).unwrap();
        for state in [read_state(root.path()).unwrap().0, load_from_root(root.path()).unwrap().state] {
            let state = state.unwrap();
            assert!(state.get("lastSection").is_none());
            assert_eq!(state["zoomLevel"], 1.2);
        }
    }
    for kind in ["image", "video", "other"] {
        let section = serde_json::json!({"kind":kind,"month":"2026-10"});
        let value = serde_json::json!({"formatVersion":1,"lastSection":section});
        std::fs::write(root.path().join(STATE_FILE_NAME), serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(read_state(root.path()).unwrap().0.unwrap()["lastSection"], section);
        assert_eq!(load_from_root(root.path()).unwrap().state.unwrap()["lastSection"], section);
    }
}
