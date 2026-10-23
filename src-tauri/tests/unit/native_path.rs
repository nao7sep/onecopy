use super::*;

#[test]
fn literal_unicode_and_escape_lookalikes_keep_their_identity() {
    for text in ["/日本語/�.jpg", r"/photos/\xff.jpg", "C:\\照片\\🦉.jpg", ""] {
        assert_eq!(encode(Path::new(text)), text);
        assert_eq!(try_from_text(text).unwrap().as_ref(), Path::new(text));
        assert_eq!(display(text), text);
    }
}

#[test]
fn malformed_or_alias_escapes_never_resolve_to_a_usable_path() {
    for text in [
        "\0",
        "\0b",
        "\0bffé\0b",
        "\0b00",
        "\0b2f",
        "\0b2e\0b2e",
        "\0b61",
        "\0bc3\0ba9",
        "\0bFF",
        "\0w0000",
        "\0wd83d\0wde00",
        "\0w0041",
        "\0wD800",
    ] {
        assert!(try_from_text(text).is_none(), "{text:?}");
        assert_eq!(from_text(text).as_ref(), Path::new(text));
    }
}

#[test]
fn display_exposes_units_without_changing_ordinary_text() {
    assert_eq!(display("a\0bff/\0wd800.jpg"), "a\\xFF/\\uD800.jpg");
    assert_eq!(display("a\0x/b\0b"), "a\\0x/b\\0b");
}

#[cfg(unix)]
#[test]
fn every_unix_byte_round_trips_without_replacement_or_separator_aliases() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    for byte in 1..=255 {
        let native = OsString::from_vec(vec![b'/', b'a', byte, b'.', b'j', b'p', b'g']);
        let encoded = encode(&native);
        assert_eq!(try_from_text(&encoded).unwrap().as_os_str(), native);
        assert!(encoded.ends_with(".jpg"));
    }
    let first = OsString::from_vec(b"/a\xff.jpg".to_vec());
    let second = OsString::from_vec(b"/a\xfe.jpg".to_vec());
    assert_ne!(encode(&first), encode(&second));
    assert_ne!(encode(&first), "/a�.jpg");
    assert_eq!(encode(&first), "/a\0bff.jpg");
    assert_eq!(label(Path::new(&first)).to_string(), "/a\\xFF.jpg");
    assert!(try_from_text("/a\0wd800.jpg").is_none());
}

windows_test!(
    "Windows UTF-16 names",
    fn windows_unpaired_units_preserve_exact_names() {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;
        for unit in 0xd800..=0xdfff {
            let native = OsString::from_wide(&[b'a' as u16, unit, b'.' as u16, b'j' as u16]);
            let encoded = encode(&native);
            assert_eq!(try_from_text(&encoded).unwrap().as_os_str(), native);
            assert_ne!(encoded, "a�.j");
        }
        assert!(try_from_text("a\0bff.jpg").is_none());
    }
);
