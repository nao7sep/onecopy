use super::*;

#[test]
fn release_tags_are_strict_and_compare_by_semantic_precedence() {
    let installed = Version::parse("1.9.9").unwrap();
    assert_eq!(
        compare_tag("v1.10.0", &installed),
        Ok((true, "1.10.0".into()))
    );
    assert_eq!(
        compare_tag("v1.9.9", &installed),
        Ok((false, "1.9.9".into()))
    );
    assert_eq!(
        compare_tag("v1.8.0", &installed),
        Ok((false, "1.8.0".into()))
    );
    for invalid in ["1.10.0", "v1.10", "v1.10.0-beta.1", "v01.10.0"] {
        assert!(compare_tag(invalid, &installed).is_err(), "{invalid}");
    }
}

#[test]
fn endpoint_is_fixed_repository_metadata() {
    assert_eq!(
        LATEST_RELEASE_API,
        "https://api.github.com/repos/nao7sep/onecopy/releases/latest"
    );
    assert_eq!(GITHUB_ACCEPT, "application/vnd.github+json");
    assert_eq!(GITHUB_API_VERSION, "2022-11-28");
    assert_eq!(USER_AGENT, "OneCopy");
    assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(10));
}

#[test]
fn response_validation_requires_one_string_tag_name() {
    assert_eq!(
        parse_latest_tag(br#"{"tag_name":"v1.2.3","body":"ignored"}"#),
        Ok("v1.2.3".into())
    );
    for invalid in [
        br#"{}"#.as_slice(),
        br#"{"tag_name":7}"#.as_slice(),
        b"not-json".as_slice(),
    ] {
        assert!(parse_latest_tag(invalid).is_err());
    }
}
