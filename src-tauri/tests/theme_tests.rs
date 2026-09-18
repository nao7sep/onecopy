use std::fs;

use onecopy_lib::storage::CONFIG_FILE_NAME;
use onecopy_lib::theme::{
    config_window_theme, read_saved_window_theme, window_background, window_theme_for, ThemeState,
};
use serde_json::json;
use tauri::window::Color;
use tauri::Theme;

#[test]
fn explicit_light_and_dark_pin_the_window_theme() {
    assert_eq!(window_theme_for("light"), Some(Theme::Light));
    assert_eq!(window_theme_for("dark"), Some(Theme::Dark));
    for preference in ["system", "", "Dark", "sepia"] {
        assert_eq!(window_theme_for(preference), None, "{preference}");
    }
}

#[test]
fn the_config_theme_field_decides_and_anything_else_follows_the_os() {
    assert_eq!(
        config_window_theme(&json!({ "theme": "dark" })),
        Some(Theme::Dark)
    );
    assert_eq!(config_window_theme(&json!({ "theme": "system" })), None);
    assert_eq!(config_window_theme(&json!({ "theme": true })), None);
    assert_eq!(config_window_theme(&json!({})), None);
    assert_eq!(config_window_theme(&json!([])), None);
}

#[test]
fn reading_the_saved_theme_never_changes_the_config() {
    let root = tempfile::tempdir().expect("temp dir");
    let config = root.path().join(CONFIG_FILE_NAME);
    assert_eq!(read_saved_window_theme(root.path()), None);
    assert!(!config.exists());

    fs::write(&config, "{corrupt").expect("write config");
    assert_eq!(read_saved_window_theme(root.path()), None);
    assert_eq!(
        fs::read_to_string(&config).expect("read config"),
        "{corrupt"
    );

    fs::write(&config, r#"{"theme":"light","sourceDirs":[]}"#).expect("write config");
    assert_eq!(read_saved_window_theme(root.path()), Some(Theme::Light));
}

#[test]
fn later_windows_take_the_most_recently_recorded_theme() {
    let state = ThemeState::default();
    assert_eq!(state.current(), None);
    state.set(Some(Theme::Dark));
    assert_eq!(state.current(), Some(Theme::Dark));
    state.set(None);
    assert_eq!(state.current(), None);
}

#[test]
fn each_theme_has_its_own_window_background() {
    assert_eq!(
        window_background(Theme::Dark),
        Color(0x02, 0x06, 0x18, 0xff)
    );
    assert_eq!(
        window_background(Theme::Light),
        Color(0xf8, 0xfa, 0xfc, 0xff)
    );
}
