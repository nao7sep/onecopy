use super::*;


use serial_test::serial;

fn temp_dir(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("onecopy-storage-{label}-"))
        .tempdir()
        .unwrap()
}

#[test]
fn storage_fixture_is_removed_when_its_body_panics() {
    let mut path = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let dir = temp_dir("failed-body");
        path = Some(dir.path().to_path_buf());
        std::fs::write(dir.path().join("owned"), b"fixture bytes").unwrap();
        panic!("fixture body failed");
    }));
    assert!(result.is_err());
    assert!(!path.unwrap().exists());
}

#[test]
fn quarantine_name_follows_the_derived_grammar() {
    let q = quarantine_name(Path::new("/data/config.json"));
    let name = q.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("config-"), "{name}");
    assert!(name.ends_with("-utc.invalid"), "{name}");
    assert!(
        !name.contains(".json"),
        "role extension replaces the original: {name}"
    );
}

#[test]
#[serial(backup_store)]
fn read_json_optional_missing_valid_and_corrupt() {
    let dir_owner = temp_dir("read-optional");
    let dir = dir_owner.path();
    let path = dir.join("config.json");

    // Missing → None.
    let missing = read_json_optional(&path, crate::formats::CONFIG).unwrap();
    assert!(missing.value.is_none());
    assert!(missing.quarantined.is_none());

    // Valid → Some.
    write_atomic(&path, b"{\"formatVersion\": 1, \"a\": 1}").unwrap();
    assert_eq!(
        read_json_optional(&path, crate::formats::CONFIG).unwrap().value,
        Some(serde_json::json!({"a": 1}))
    );

    // Corrupt → quarantined aside (original bytes preserved) and None.
    std::fs::write(&path, b"{ not json").unwrap();
    let corrupt = read_json_optional(&path, crate::formats::CONFIG).unwrap();
    assert!(corrupt.value.is_none());
    assert!(corrupt.quarantined.is_some());
    assert!(
        !path.exists(),
        "corrupt file must be renamed aside, not left in place"
    );
    let quarantined: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".invalid"))
        .collect();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(
        std::fs::read(quarantined[0].path()).unwrap(),
        b"{ not json",
        "quarantine preserves the original bytes"
    );
}

#[test]
fn a_json_target_that_becomes_newer_at_publication_is_preserved_and_staging_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("config.json");
    let current = b"{\"formatVersion\":1,\"theme\":\"light\"}";
    std::fs::write(&target, current).unwrap();
    refuse_newer(&target, formats::CONFIG).unwrap();
    let future = b"{\"formatVersion\":2,\"futureSetting\":true}";

    let error = write_atomic_inner(&target, b"{\"formatVersion\":1,\"theme\":\"dark\"}", false, || {
        let staged = std::fs::read_dir(dir.path()).unwrap()
            .map(|entry| entry.unwrap().path()).find(|path| path.extension().is_some_and(|ext| ext == "tmp")).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), current);
        assert_eq!(std::fs::read(&staged).unwrap(), b"{\"formatVersion\":1,\"theme\":\"dark\"}");
        std::fs::write(&target, future).unwrap();
        refuse_newer(&target, formats::CONFIG)
    }).unwrap_err();

    assert!(error.contains("newer"), "{error}");
    assert_eq!(std::fs::read(&target).unwrap(), future);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn admission_failure_keeps_its_primary_error_and_cleans_the_owned_private_stage() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("state.json");
    std::fs::write(&target, b"original").unwrap();
    let error = write_atomic_inner(&target, b"replacement", false, || {
        Err("primary admission failure".to_string())
    }).unwrap_err();
    assert_eq!(error, "primary admission failure");
    assert_eq!(std::fs::read(&target).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn publication_failure_preserves_the_obstruction_and_removes_the_owned_stage() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("state.json");
    let error = write_atomic_inner(&target, b"replacement", false, || {
        let stage = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&stage).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(std::fs::read(stage).unwrap(), b"replacement");
        std::fs::create_dir(&target).unwrap();
        Ok(())
    }).unwrap_err();
    assert!(!error.is_empty());
    assert!(target.is_dir());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
#[cfg(windows)]
fn readonly_replacement_stages_are_cleaned_on_admission_and_publication_failure() {
    for refusal in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.json");
        std::fs::write(&target, b"original").unwrap();
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&target, permissions).unwrap();

        let error = write_atomic_inner(&target, b"replacement", false, || {
            let stage = std::fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|ext| ext == "tmp")).unwrap();
            assert!(std::fs::metadata(stage).unwrap().permissions().readonly());
            if refusal { Err("primary admission failure".to_string()) } else { Ok(()) }
        }).unwrap_err();

        if refusal { assert_eq!(error, "primary admission failure"); }
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        assert!(std::fs::metadata(&target).unwrap().permissions().readonly());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let mut permissions = std::fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&target, permissions).unwrap();
    }
}

#[test]
fn atomic_temp_name_is_stem_plus_nanoid_dot_tmp() {
    let name = atomic_temp_name("config.json").unwrap();
    assert!(name.starts_with("config-"), "{name:?}");
    assert!(name.ends_with(".tmp"), "{name:?}");
    let discriminator = &name["config-".len()..name.len() - ".tmp".len()];
    assert_eq!(discriminator.len(), 21);
    assert_ne!(
        atomic_temp_name("config.json").unwrap(),
        atomic_temp_name("config.json").unwrap()
    );
}

#[test]
fn new_config_confirms_direct_trash_by_default() {
    assert!(DefaultConfig::default().confirm_trash_delete);
}

// R5.5 C3: a config patch that sets `language` updates `LanguageState::current`,
// and the very next appearance-preferences read must return it -- not a
// cached or launch-time value. `with_language_fields` is exactly that
// projection, kept apart from the `app.state::<i18n::LanguageState>()` lookup
// so it is testable against a `LanguageState` built directly, no AppHandle.
#[test]
fn with_language_fields_reflects_the_current_value_after_it_changes() {
    let state = crate::i18n::LanguageState {
        system_language: "en",
        system_locale: Some("en-US".to_string()),
        current: std::sync::Mutex::new("en"),
    };

    let first = with_language_fields(serde_json::json!({ "uiFontFamily": null }), &state);
    assert_eq!(first["language"], "en");
    assert_eq!(first["systemLanguage"], "en");
    assert_eq!(first["systemLocale"], "en-US");
    // The unrelated field already on the document survives the projection.
    assert_eq!(first["uiFontFamily"], serde_json::Value::Null);

    // A saved language change (as `save_config` applies through
    // `state.set_current`) is what the NEXT read returns -- not what the
    // state held when the window launched.
    state.set_current("ja");
    let second = with_language_fields(serde_json::json!({}), &state);
    assert_eq!(second["language"], "ja");
    // The computer's own language and locale never follow a saved choice.
    assert_eq!(second["systemLanguage"], "en");
    assert_eq!(second["systemLocale"], "en-US");
}

#[cfg(target_os = "macos")]
#[test]
fn an_app_local_new_stage_does_not_keep_inherited_read_grants() {
    let dir = tempfile::tempdir().unwrap();
    assert!(std::process::Command::new("/bin/chmod").args(["+a", "everyone allow read,file_inherit"]).arg(dir.path()).status().unwrap().success());
    let target = dir.path().join("settings.json");
    write_atomic_inner(&target, b"protected bytes", false, || {
        let staged = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
        let listing = std::process::Command::new("/bin/ls").arg("-le").arg(staged).output().unwrap();
        assert!(listing.status.success());
        assert_eq!(String::from_utf8(listing.stdout).unwrap().lines().count(), 1, "the empty inherited ACL was removed before writing");
        Ok(())
    }).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"protected bytes");
    let result = write_atomic_inner(&target, b"replacement bytes", false, || Err("publication refused".into()));
    assert_eq!(result.unwrap_err(), "publication refused");
    assert_eq!(std::fs::read(&target).unwrap(), b"protected bytes");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}
