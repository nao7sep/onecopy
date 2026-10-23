#[cfg(target_os = "macos")]
#[test]
fn plist_extraction_finds_the_keyed_string() {
    let plist = r#"<dict>
        <key>VolumeName</key><string>Macintosh HD</string>
        <key>VolumeUUID</key>
        <string>  AAAA-BBBB  </string>
    </dict>"#;
    assert_eq!(
        super::extract_plist_string(plist, "VolumeUUID").as_deref(),
        Some("AAAA-BBBB")
    );
    assert_eq!(super::extract_plist_string(plist, "Missing"), None);
}

#[test]
fn an_unreadable_recorded_identity_does_not_hide_other_source_results_or_replace_the_record() {
    let data = tempfile::tempdir().unwrap();
    let sources = tempfile::tempdir().unwrap();
    let roots: Vec<String> = ["uncertain", "healthy", "substituted", "unidentified", "missing"]
        .iter().map(|name| sources.path().join(name).to_string_lossy().into_owned()).collect();
    for root in &roots[..4] { std::fs::create_dir(root).unwrap(); }
    crate::storage::save_config(data.path(), &serde_json::json!({ "sourceDirs": roots })).unwrap();
    for root in &roots[..3] { super::check_identity(data.path(), root, "original").unwrap(); }
    let status = super::probe_source_dirs_with(data.path(), &|path| {
        match path.file_name().unwrap().to_str().unwrap() {
            "healthy" => Some("original".to_string()),
            "substituted" => Some("replacement".to_string()),
            _ => None,
        }
    }).unwrap();
    assert_eq!(status.uncertain, roots[..1]);
    assert_eq!(status.substituted, roots[2..3]);
    assert_eq!(status.unidentified, roots[3..4]);
    assert_eq!(status.missing, roots[4..5]);
    assert!(serde_json::to_value(&status).unwrap().get("uncertain").is_none());
    assert!(super::verify_source_dirs_with(data.path(), &|_| None).is_err());
    let restored = super::probe_source_dirs_with(data.path(), &|_| Some("original".into())).unwrap();
    assert!(restored.uncertain.is_empty());
    assert!(restored.substituted.is_empty(), "neither uncertainty nor substitution replaced a record");
}
