use super::*;


use serial_test::serial;

fn temp_dir(label: &str) -> PathBuf {
    tempfile::Builder::new()
        .prefix(&format!("onecopy-storage-{label}-"))
        .tempdir()
        .unwrap()
        .keep()
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
    let dir = temp_dir("read-optional");
    let path = dir.join("config.json");

    // Missing → None.
    let missing = read_json_optional(&path).unwrap();
    assert!(missing.value.is_none());
    assert!(missing.quarantined.is_none());

    // Valid → Some.
    write_atomic(&path, b"{\"a\": 1}").unwrap();
    assert_eq!(
        read_json_optional(&path).unwrap().value,
        Some(serde_json::json!({"a": 1}))
    );

    // Corrupt → quarantined aside (original bytes preserved) and None.
    std::fs::write(&path, b"{ not json").unwrap();
    let corrupt = read_json_optional(&path).unwrap();
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

    // A saved language change (as `patch_config` applies through
    // `state.set_current`) is what the NEXT read returns -- not what the
    // state held when the window launched.
    state.set_current("ja");
    let second = with_language_fields(serde_json::json!({}), &state);
    assert_eq!(second["language"], "ja");
    // The computer's own language and locale never follow a saved choice.
    assert_eq!(second["systemLanguage"], "en");
    assert_eq!(second["systemLocale"], "en-US");
}
