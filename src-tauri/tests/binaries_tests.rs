// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::binaries::*;

#[test]
fn status_derivation_covers_all_four_states() {
    let facts = |installed: Option<&str>, latest: Option<&str>| BinaryFacts {
        installed_version: installed.map(String::from),
        latest_known_version: latest.map(String::from),
        last_checked_at_utc: None,
    };
    assert_eq!(
        derive_status(false, &facts(Some("7.1"), Some("7.1"))),
        BinaryStatus::NotInstalled
    );
    assert_eq!(
        derive_status(true, &facts(Some("7.0"), Some("7.1"))),
        BinaryStatus::UpdateAvailable
    );
    assert_eq!(
        derive_status(true, &facts(Some("7.1"), Some("7.1"))),
        BinaryStatus::UpToDate
    );
    assert_eq!(
        derive_status(true, &facts(Some("7.1"), None)),
        BinaryStatus::InstalledUnchecked
    );
    assert_eq!(
        derive_status(true, &facts(None, None)),
        BinaryStatus::InstalledUnchecked
    );
}

#[test]
fn martin_version_parses_the_epoch_version_segment() {
    assert_eq!(
        parse_martin_build_version("/download/release/1719302400_7.0.1/ffmpeg.zip"),
        Some("7.0.1".to_string())
    );
    assert_eq!(
        parse_martin_build_version("/x/1719302400_7.0.1-tessus/ffmpeg.zip"),
        Some("7.0.1-tessus".to_string())
    );
    assert_eq!(parse_martin_build_version("/plain/ffmpeg.zip"), None);
    assert_eq!(parse_martin_build_version("/notepoch_x/ffmpeg.zip"), None);
}

#[test]
fn sums_parsing_matches_exact_names_with_optional_star() {
    let sums = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  ffmpeg-master-latest-win64-gpl.zip
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *other.zip
not-a-digest  whatever.zip";
    assert_eq!(
        parse_sums(sums, BTBN_WIN64_ASSET).as_deref(),
        Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
    );
    assert_eq!(
        parse_sums(sums, "other.zip").as_deref(),
        Some("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210")
    );
    assert_eq!(parse_sums(sums, "missing.zip"), None);
    assert_eq!(parse_sums(sums, "whatever.zip"), None);
}

#[test]
fn a_present_but_unusable_binary_is_not_installed() {
    // The status drives the UI's "up to date" claim and the derive passes'
    // ffmpeg gate, so presence has to mean USABLE. A zero-byte placeholder or
    // a file that lost its executable bit (an unzip without permissions, a
    // copy across a filesystem that drops the mode) would otherwise report
    // installed and fail at the first invocation.
    let dir = tempfile::tempdir().unwrap();

    let missing = dir.path().join("absent");
    assert!(!is_usable_binary(&missing), "an absent file is not usable");

    let empty = dir.path().join("empty");
    std::fs::write(&empty, b"").unwrap();
    assert!(!is_usable_binary(&empty), "a zero-byte file is not a binary");

    let real = dir.path().join("ffmpeg");
    std::fs::write(&real, b"#!/bin/sh\nexit 0\n").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            !is_usable_binary(&real),
            "a non-executable file must not report installed"
        );
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(is_usable_binary(&real), "an executable file is usable");

    // A directory at the binary's path is not a binary either.
    let dir_at_path = dir.path().join("as-dir");
    std::fs::create_dir_all(&dir_at_path).unwrap();
    assert!(!is_usable_binary(&dir_at_path));
}
