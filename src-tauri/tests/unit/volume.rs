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
