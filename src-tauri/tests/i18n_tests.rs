// Integration tests for the natively resolved interface language: which tag the
// computer's languages resolve to, how a saved choice is normalized and taken
// from the settings, and that the native menu's text exists in every catalogue.

use std::fs;
use std::path::Path;

use onecopy_lib::i18n::{catalogue, config_preference, normalize_preference, system_language, LANGUAGES};
use onecopy_lib::menu::KEYS;
use onecopy_lib::startup::LAUNCH_FAILURE_KEYS;
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
    for tag in LANGUAGES {
        assert!(catalogue(tag).has("nativeMenu.about"), "{tag} is not embedded");
    }
}

#[test]
fn every_menu_key_is_in_every_language() {
    for language in LANGUAGES {
        let text = catalogue(language);
        for key in KEYS {
            assert!(text.has(key), "{language} lacks {key}");
        }
    }
}

#[test]
fn every_launch_failure_key_is_in_every_language() {
    for language in LANGUAGES {
        let text = catalogue(language);
        for key in LAUNCH_FAILURE_KEYS {
            assert!(text.has(key), "{language} lacks {key}");
        }
    }
}

#[test]
fn catalogue_text_fills_in_the_app_name() {
    assert_eq!(catalogue("en").text("nativeMenu.quit", "OneCopy"), "Quit OneCopy");
}

#[test]
fn catalogue_text_with_fills_in_a_second_placeholder() {
    // The launch-failure dialog's log-location sentence (Finding D): a path is
    // appended as recorded, not translated, alongside the app name.
    assert_eq!(
        catalogue("en").text_with("launch.failedLogDir", "OneCopy", "logDir", "/tmp/onecopy/logs"),
        "Application logs, when there are any, are in /tmp/onecopy/logs.",
    );
}
