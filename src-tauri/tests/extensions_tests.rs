// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::extensions::*;

fn owned(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn classify_covers_every_kind() {
    let (i, v, a, c) = (
        owned(IMAGE_EXTENSIONS),
        owned(VIDEO_EXTENSIONS),
        owned(AUDIO_EXTENSIONS),
        owned(COMPANION_EXTENSIONS),
    );
    assert_eq!(classify("jpg", &i, &v, &a, &c), Kind::Image);
    assert_eq!(classify("hif", &i, &v, &a, &c), Kind::Image);
    assert_eq!(classify("mov", &i, &v, &a, &c), Kind::Video);
    assert_eq!(classify("m4a", &i, &v, &a, &c), Kind::Audio);
    assert_eq!(classify("arw", &i, &v, &a, &c), Kind::Companion);
    assert_eq!(classify("pdf", &i, &v, &a, &c), Kind::Other);
    assert_eq!(classify("", &i, &v, &a, &c), Kind::Other);
}

#[test]
fn built_in_lists_are_lowercase_and_disjoint() {
    let all: Vec<&&str> = IMAGE_EXTENSIONS
        .iter()
        .chain(VIDEO_EXTENSIONS)
        .chain(AUDIO_EXTENSIONS)
        .chain(COMPANION_EXTENSIONS)
        .collect();
    for ext in &all {
        assert_eq!(**ext, ext.to_ascii_lowercase(), "extension must be lowercase");
        assert!(!ext.starts_with('.'), "extension must not carry a dot");
    }
    let unique: std::collections::HashSet<&&str> = all.iter().copied().collect();
    assert_eq!(unique.len(), all.len(), "an extension must appear in exactly one list");
}

#[test]
fn lowercase_ext_normalizes_and_handles_missing() {
    assert_eq!(lowercase_ext("IMG_1234.JPG"), "jpg");
    assert_eq!(lowercase_ext("clip.MOV"), "mov");
    assert_eq!(lowercase_ext("noext"), "");
    assert_eq!(lowercase_ext(".hidden"), "");
}
