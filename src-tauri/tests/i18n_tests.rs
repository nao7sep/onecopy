#[path = "support/catalogue.rs"]
mod source_catalogue;

// Integration tests for the natively resolved interface language: which tag the
// computer's languages resolve to, how a saved choice is normalized and taken
// from the settings, and that the native menu's text exists in every catalogue.

use std::fs;
use std::path::Path;

use onecopy_lib::i18n::{catalogue, config_preference, normalize_preference, system_language, LANGUAGES};
use serde_json::json;

#[test]
fn system_language_takes_the_first_supported_preferred_locale() {
    assert_eq!(system_language(["ja-JP", "en-US"]), "ja");
    assert_eq!(system_language(["nl-NL", "de-DE"]), "de");
    assert_eq!(system_language(["en-GB"]), "en");
    assert_eq!(system_language(["es-419"]), "es");
    assert_eq!(system_language(["ru_RU.UTF-8"]), "ru");
}

#[test]
fn every_chinese_locale_resolves_to_simplified_chinese() {
    for locale in ["zh-CN", "zh-Hans-CN", "zh-TW", "zh-Hant-TW", "zh-HK", "zh-Hant-HK", "zh"] {
        assert_eq!(system_language([locale]), "zh-Hans", "{locale}");
    }
}

#[test]
fn every_portuguese_locale_resolves_to_brazilian_portuguese() {
    assert_eq!(system_language(["pt-PT"]), "pt-BR");
    assert_eq!(system_language(["pt-BR"]), "pt-BR");
}

#[test]
fn an_unsupported_or_empty_list_falls_back_to_english() {
    assert_eq!(system_language(["nl-NL", "sv-SE"]), "en");
    assert_eq!(system_language(["C", "POSIX"]), "en");
    assert_eq!(system_language(std::iter::empty()), "en");
}

#[test]
fn preference_normalizes_like_the_frontend() {
    assert_eq!(normalize_preference(Some("ja")), Some("ja"));
    assert_eq!(normalize_preference(Some("zh-Hans")), Some("zh-Hans"));
    assert_eq!(normalize_preference(Some("system")), None);
    assert_eq!(normalize_preference(Some("zh-hans")), None);
    assert_eq!(normalize_preference(Some("xx")), None);
    assert_eq!(normalize_preference(None), None);
}

#[test]
fn the_saved_choice_is_the_settings_language() {
    assert_eq!(config_preference(&json!({ "language": "de", "theme": "dark" })), Some("de"));
    assert_eq!(config_preference(&json!({ "language": "system" })), None);
    assert_eq!(config_preference(&json!({ "language": 7 })), None);
    assert_eq!(config_preference(&json!({ "theme": "dark" })), None);
}

#[test]
fn embeds_exactly_the_catalogues_in_the_locales_folder() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/i18n/locales");
    let mut on_disk: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.strip_suffix(".json").map(str::to_string)
        })
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = LANGUAGES.iter().map(|tag| tag.to_string()).collect();
    listed.sort();
    assert_eq!(listed, on_disk);
    for language in LANGUAGES {
        let source = source_catalogue::entries(language);
        let native = catalogue(language);
        for (key, value) in source.as_object().unwrap() {
            if let Some(template) = value.as_str() {
                assert_eq!(native.text(key, "OneCopy"), template.replace("{app}", "OneCopy"),
                    "{language}: embedded text for {key}");
            }
        }
    }
}

// Read literal references from both platform branches without maintaining a
// second key inventory or requiring a running native application to build menus.
fn referenced_keys<'a>(source: &'a str, prefix: &str) -> std::collections::BTreeSet<&'a str> {
    let keys: std::collections::BTreeSet<_> = source.match_indices(prefix)
        .filter(|(start, _)| *start > 0 && source.as_bytes()[start - 1] == b'"')
        .filter_map(|(start, _)| source[start..].split_once('"').map(|(key, _)| key))
        .filter(|key| key.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')))
        .collect();
    assert!(!keys.is_empty(), "no native references for {prefix}");
    keys
}

fn assert_keys_in_catalogues<'a>(keys: impl IntoIterator<Item = &'a str>) {
    let keys: Vec<_> = keys.into_iter().collect();
    for language in LANGUAGES {
        let source = source_catalogue::entries(language);
        let native = catalogue(language);
        for key in &keys {
            let template = source.get(*key).and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("{language} lacks {key}"));
            assert_eq!(native.text(key, "OneCopy"), template.replace("{app}", "OneCopy"),
                "{language}: embedded text for {key}");
        }
    }
}

#[test]
fn every_menu_key_is_in_every_language() {
    assert_keys_in_catalogues(referenced_keys(include_str!("../src/menu.rs"), "nativeMenu."));
}

#[test]
fn the_records_window_title_is_in_every_language() {
    assert_keys_in_catalogues([onecopy_lib::records_window::TITLE_KEY]);
}

#[test]
fn every_launch_failure_key_is_in_every_language() {
    assert_keys_in_catalogues(referenced_keys(include_str!("../src/startup.rs"), "launch."));
}

#[test]
fn catalogue_text_fills_in_the_app_name() {
    for language in LANGUAGES {
        let quit = catalogue(language).text("nativeMenu.quit", "OneCopy");
        assert!(quit.contains("OneCopy"), "{language}: {quit}");
        assert!(!quit.contains("{app}"), "{language}: {quit}");
    }
}

#[test]
fn catalogue_text_with_fills_in_a_second_placeholder() {
    // The launch-failure dialog's log-location sentence: a path is
    // appended as recorded, not translated, alongside the app name.
    for language in LANGUAGES {
        let sentence =
            catalogue(language).text_with("launch.failedLogDir", "OneCopy", "logDir", "/tmp/onecopy/logs");
        assert!(sentence.contains("/tmp/onecopy/logs"), "{language}: {sentence}");
        assert!(!sentence.contains("{logDir}"), "{language}: {sentence}");
    }
}

// AppKit takes its own language from the defaults' argument domain. The
// override has to reach a real defaults lookup and keep whatever the launch
// arguments already put in that domain.
#[cfg(target_os = "macos")]
#[test]
fn appkit_reads_the_interface_language_and_the_launch_arguments_survive() {
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSArgumentDomain, NSDictionary, NSString, NSUserDefaults};

    let defaults = NSUserDefaults::standardUserDefaults();
    // SAFETY: NSArgumentDomain is a constant NSString that Foundation defines.
    let domain_name = unsafe { NSArgumentDomain };
    let argument = NSString::from_str("OneCopyTestArgument");
    let value = NSString::from_str("kept");
    let value: &AnyObject = value.as_ref();
    let launch_arguments = NSDictionary::from_slices(&[&*argument], &[value]);
    // SAFETY: NSString keys mapped to a property-list value.
    unsafe { defaults.setVolatileDomain_forName(&launch_arguments, domain_name) };

    onecopy_lib::i18n::align_appkit("ja");

    let languages = defaults
        .stringArrayForKey(&NSString::from_str("AppleLanguages"))
        .expect("AppleLanguages is set");
    let languages: Vec<String> = languages.to_vec().iter().map(|language| language.to_string()).collect();
    assert_eq!(languages, ["ja"]);
    assert_eq!(defaults.stringForKey(&argument).map(|kept| kept.to_string()).as_deref(), Some("kept"));
}
