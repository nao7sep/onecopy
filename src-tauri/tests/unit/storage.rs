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
