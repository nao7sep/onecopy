//! The saved appearance choice, applied natively (app-chrome conventions,
//! Theme). The window theme is the one theme authority: it paints each title
//! bar and drives each webview's prefers-color-scheme, which App.css's dark
//! block follows. The core records the choice, applies it to Main before Main
//! is shown, gives every later window the same theme when its page starts
//! loading, and re-applies it to every open window when a config save changes
//! it — so no page resolves System or applies a theme itself.

use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use tauri::window::Color;
use tauri::{AppHandle, Manager, Runtime, Theme, Webview, Window};

use crate::storage::CONFIG_FILE_NAME;

/// The window theme for a saved preference: `Some` for "light" or "dark",
/// `None` (follow the OS) for anything else — the rule the settings store's
/// load normalization applies.
pub fn window_theme_for(preference: &str) -> Option<Theme> {
    match preference {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        _ => None,
    }
}

/// The window theme a config document asks for.
pub fn config_window_theme(config: &Value) -> Option<Theme> {
    window_theme_for(config.get("theme")?.as_str()?)
}

/// Reads the saved choice without touching the file. A missing, unreadable, or
/// corrupt config follows the OS; its recovery stays with the startup path.
pub fn read_saved_window_theme(data_root: &Path) -> Option<Theme> {
    let bytes = std::fs::read(data_root.join(CONFIG_FILE_NAME)).ok()?;
    config_window_theme(&serde_json::from_slice(&bytes).ok()?)
}

/// The window background behind each page — App.css's --background in each
/// theme — so the frames before a page paints and the backing exposed while
/// resizing already match.
pub fn window_background(theme: Theme) -> Color {
    match theme {
        Theme::Dark => Color(0x02, 0x06, 0x18, 0xff),
        _ => Color(0xf8, 0xfa, 0xfc, 0xff),
    }
}

/// The theme every window follows, so a window opened later starts in it.
#[derive(Default)]
pub struct ThemeState(Mutex<Option<Theme>>);

impl ThemeState {
    pub fn current(&self) -> Option<Theme> {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set(&self, theme: Option<Theme>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = theme;
    }
}

fn apply_to_window<R: Runtime>(window: &Window<R>, theme: Option<Theme>) -> Result<Theme, String> {
    window.set_theme(theme).map_err(|error| error.to_string())?;
    let effective = match theme {
        Some(theme) => theme,
        None => window.theme().map_err(|error| error.to_string())?,
    };
    window
        .set_background_color(Some(window_background(effective)))
        .map_err(|error| error.to_string())?;
    Ok(effective)
}

/// Applies a theme (`None` follows the OS) to a webview's window and to the
/// webview's own background.
pub fn apply_to_webview<R: Runtime>(
    webview: &Webview<R>,
    theme: Option<Theme>,
) -> Result<(), String> {
    let effective = apply_to_window(&webview.window(), theme)?;
    webview
        .set_background_color(Some(window_background(effective)))
        .map_err(|error| error.to_string())
}

/// Records a newly saved choice and applies it to every open window. A window
/// that cannot take it is reported; the others still follow.
pub fn apply_everywhere<R: Runtime>(
    app: &AppHandle<R>,
    theme: Option<Theme>,
) -> Result<(), String> {
    app.state::<ThemeState>().set(theme);
    let failures: Vec<String> = app
        .webview_windows()
        .into_iter()
        .filter_map(|(label, window)| {
            apply_to_webview(window.as_ref(), theme)
                .err()
                .map(|error| format!("{label}: {error}"))
        })
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
