
use onecopy_lib::theme::{config_window_theme, window_background, window_theme_for, ThemeState};
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
fn later_windows_take_the_most_recently_recorded_theme() {
    let state = ThemeState::default();
    assert_eq!(state.current(), None);
    state.set(Some(Theme::Dark));
    assert_eq!(state.current(), Some(Theme::Dark));
    state.set(None);
    assert_eq!(state.current(), None);
}

// The window background is painted before the page, and while a window
// resizes, so it must be the stylesheet's own --background in each theme or
// the frame flashes a different colour. App.css owns the colour; it aliases
// Tailwind's palette, whose colours are OKLCH.
fn stylesheet_background(dark: bool) -> [f64; 3] {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let css = std::fs::read_to_string(root.join("src/App.css")).expect("App.css");
    let palette = std::fs::read_to_string(root.join("node_modules/tailwindcss/theme.css"))
        .expect("Tailwind's theme.css; run npm install first");
    let block = if dark {
        let media = css.find("@media (prefers-color-scheme: dark) {").expect("dark block");
        &css[media..]
    } else {
        &css[css.find("\n:root {").expect(":root block")..]
    };
    let token = |source: &str, name: &str| -> String {
        let start = source.find(&format!("{name}:")).unwrap_or_else(|| panic!("{name} is defined"));
        let value = &source[start + name.len() + 1..];
        value[..value.find(';').unwrap()].trim().to_string()
    };
    let mut value = token(block, "--background");
    while let Some(reference) = value.strip_prefix("var(").and_then(|rest| rest.strip_suffix(')')) {
        value = token(&format!("{css}\n{palette}"), reference);
    }
    let parts: Vec<f64> = value
        .strip_prefix("oklch(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("--background resolves to OKLCH, not {value}"))
        .split_whitespace()
        .map(|part| part.trim_end_matches('%').parse().unwrap())
        .collect();
    let (lightness, chroma, hue) = (parts[0] / 100.0, parts[1], parts[2].to_radians());
    let (a, b) = (chroma * hue.cos(), chroma * hue.sin());
    let l = (lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m = (lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s = (lightness - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ]
    .map(|linear| {
        let clipped = linear.clamp(0.0, 1.0);
        let srgb = if clipped <= 0.003_130_8 {
            12.92 * clipped
        } else {
            1.055 * clipped.powf(1.0 / 2.4) - 0.055
        };
        srgb * 255.0
    })
}

#[test]
fn each_window_background_is_the_stylesheets_background() {
    for (theme, dark) in [(Theme::Dark, true), (Theme::Light, false)] {
        let Color(red, green, blue, alpha) = window_background(theme);
        assert_eq!(alpha, 0xff, "{theme:?} is opaque");
        let expected = stylesheet_background(dark);
        for (channel, wanted) in [red, green, blue].into_iter().zip(expected) {
            assert!(
                (f64::from(channel) - wanted).abs() <= 1.0,
                "{theme:?}: {channel} against the stylesheet's {wanted:.1}"
            );
        }
    }
}
