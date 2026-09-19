use super::*;

#[test]
fn offset_parser_handles_common_forms() {
    assert_eq!(parse_utc_offset_minutes("+09:00"), Some(540));
    assert_eq!(parse_utc_offset_minutes("-05:30"), Some(-330));
    assert_eq!(parse_utc_offset_minutes("Z"), Some(0));
    assert_eq!(parse_utc_offset_minutes("+0200"), Some(120));
    assert_eq!(parse_utc_offset_minutes("+02"), Some(120));
    assert_eq!(parse_utc_offset_minutes(""), None);
    assert_eq!(parse_utc_offset_minutes("+25:00"), None);
    assert_eq!(parse_utc_offset_minutes("09:00"), None);
}

#[test]
fn absolute_from_fields_subtracts_the_offset() {
    // 2016-03-05 12:00:00 at +09:00 == 03:00:00 UTC.
    let got = absolute_from_fields(2016, 3, 5, 12, 0, 0, 540).unwrap();
    let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
        .unwrap()
        .and_hms_opt(3, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    assert_eq!(got, MetadataTimestamp::Absolute { unix_ms: expected });
}

#[test]
fn unsupported_metadata_containers_yield_empty_metadata() {
    let dir = tempfile::Builder::new()
        .prefix("onecopy-meta-")
        .tempdir()
        .unwrap();
    let path = dir.path().join("not-a-photo.jpg");
    std::fs::write(&path, b"plainly not a jpeg").unwrap();
    let meta = read_image_metadata(&path).unwrap();
    assert!(meta.taken.is_none());
    assert!(meta.make.is_none());

    let video = dir.path().join("not-a-video.mp4");
    std::fs::write(&video, b"nope").unwrap();
    let meta = read_video_metadata(&video).unwrap();
    assert!(meta.taken.is_none());
    assert!(meta.duration_ms.is_none());
}
