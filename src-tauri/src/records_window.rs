//! The Records window shows `records.sqlite3`. It is a durable secondary
//! window with its own placement (window-conventions, Placement), and there is
//! only ever one: opening it again brings it forward. It holds no work, so it
//! never keeps the app from quitting, and activating the app still brings Main
//! back (lib.rs, `RunEvent::Reopen`).

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::{app_lifecycle, i18n, theme, window_placement};

pub const LABEL: &str = "records";
/// The window title's catalogue key; the tests check it in every language.
pub const TITLE_KEY: &str = "window.titleRecords";
/// Sent to the window after each commit that stored a record.
pub const CHANGED_EVENT: &str = "records://changed";

/// The designed initial size, in logical pixels.
const INITIAL_WIDTH: f64 = 1240.0;
const INITIAL_HEIGHT: f64 = 820.0;

/// Held while a window is being found or built, so two quick opens make one
/// window.
static OPENING: Mutex<()> = Mutex::new(());

fn title(app: &AppHandle, language: &str) -> String {
    i18n::catalogue(language).text(TITLE_KEY, &app.package_info().name)
}

fn bring_forward(window: &WebviewWindow) -> Result<(), String> {
    window
        .unminimize()
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus())
        .map_err(|error| error.to_string())
}

/// Opens the Records window, or brings the open one forward. It is built
/// hidden, placed, and then shown, so its first frame is already in place.
pub fn open(app: &AppHandle) -> Result<(), String> {
    let _opening = OPENING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(window) = app.get_webview_window(LABEL) {
        return bring_forward(&window);
    }
    let language = app.state::<i18n::LanguageState>().current();
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?view=records".into()))
        .title(title(app, language))
        .inner_size(INITIAL_WIDTH, INITIAL_HEIGHT)
        .visible(false)
        .build()
        .map_err(|error| error.to_string())?;
    let shown = (|| {
        theme::apply_to_webview(window.as_ref(), app.state::<theme::ThemeState>().current())?;
        let placement = app.state::<window_placement::RecordsPlacementState>();
        window_placement::place_records(&window.as_ref().window(), &placement.0);
        bring_forward(&window)
    })();
    if shown.is_err() {
        // A window that cannot be shown is not left behind, hidden, to be
        // "brought forward" by the next open.
        if let Err(error) = window.destroy() {
            crate::logging::warn(
                "unshown Records window could not be closed",
                serde_json::json!({ "error": { "message": error.to_string() } }),
            );
        }
    }
    shown
}

/// Gives the open window the interface language's title after a saved change.
pub fn retitle(app: &AppHandle, language: &str) -> Result<(), String> {
    match app.get_webview_window(LABEL) {
        Some(window) => window.set_title(&title(app, language)).map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

/// Tells the open window that a record was stored. It runs right after the
/// writer's commit, so the window's next read sees the record. It never logs:
/// a log line would itself be a stored record and signal again.
pub fn notify_changed(app: &AppHandle) {
    if app_lifecycle::shutting_down() || app.get_webview_window(LABEL).is_none() {
        return;
    }
    if let Err(error) = app.emit_to(LABEL, CHANGED_EVENT, ()) {
        eprintln!("[onecopy:records] the Records window could not be told about a new record: {error}");
    }
}
