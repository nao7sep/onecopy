//! Fullscreen in OneCopy is a borderless window over its display's full frame,
//! raised above the Dock and the menu bar. It is never macOS Spaces
//! fullscreen, which slides windows sideways, and never the app-wide Dock or
//! menu-bar options, which would reach displays no fullscreen window uses; a
//! display without one keeps its Dock and menu bar untouched.
//!
//! macOS: a fullscreen window sits at the status level, the lowest that covers
//! both the Dock (20) and the menu bar (24), while OneCopy is active, and at
//! the normal level while another app is, so it never floats over that app.
//! A decorated window (Main, as Comparison's first surface) loses its title
//! bar for the duration and gets its exact earlier frame back.
//!
//! Windows: the focused surface uses tao's borderless fullscreen; Comparison's
//! other displays stay monitor-sized windows. Every fullscreen window is
//! topmost while OneCopy is active, which covers the taskbar even after a
//! click on another display or while another OneCopy window is in front, and
//! drops topmost while another app is, so it sits behind that app.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use tauri::{AppHandle, Emitter, Manager};

/// A window frame in the platform's own window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    /// Just above the Dock and the menu bar.
    Raised,
}

/// A fullscreen window covers the Dock and menu bar only while OneCopy is the
/// active app; otherwise it would stay over whatever app the user switched to.
pub fn level(fullscreen: bool, application_active: bool) -> Level {
    if fullscreen && application_active {
        Level::Raised
    } else {
        Level::Normal
    }
}

/// The level a window takes when its fullscreen change lands. The change is
/// queued, and OneCopy may have stopped being active since it was asked for,
/// so activation is read then, never assumed; leaving never needs it.
pub fn landing_level(
    enable: bool,
    application_active: impl FnOnce() -> Result<bool, String>,
) -> Result<Level, String> {
    if !enable {
        return Ok(Level::Normal);
    }
    Ok(level(true, application_active()?))
}

/// Which kind of fullscreen surface a window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The window that holds the keyboard: Main in Comparison, the fullscreen view.
    Focused,
    /// One of Comparison's other displays.
    Spread,
}

/// Which windows are fullscreen, as which surface, and the frame each
/// decorated one returns to. Main's saved placement consults it too: its
/// fullscreen frame is transient, and macOS reports a raised borderless window
/// as an ordinary one.
#[derive(Default)]
pub struct FullscreenState {
    windows: BTreeMap<String, (Surface, Option<Frame>)>,
    active: Option<bool>,
}

impl FullscreenState {
    /// Records entry with the frame to restore; false when already fullscreen.
    pub fn enter(&mut self, label: &str, surface: Surface, restore: Option<Frame>) -> bool {
        if self.windows.contains_key(label) {
            return false;
        }
        self.windows.insert(label.to_owned(), (surface, restore));
        true
    }

    /// Records exit; `None` when the window was not fullscreen, otherwise the
    /// frame it entered from, if it had one to restore.
    pub fn leave(&mut self, label: &str) -> Option<Option<Frame>> {
        self.windows.remove(label).map(|(_, restore)| restore)
    }

    pub fn contains(&self, label: &str) -> bool {
        self.windows.contains_key(label)
    }

    pub fn windows(&self) -> Vec<(String, Surface)> {
        self.windows
            .iter()
            .map(|(label, (surface, _))| (label.clone(), *surface))
            .collect()
    }

    /// Notes whether OneCopy is the active app; returns the new value only
    /// when it differs from the last observation.
    pub fn observe_activation(&mut self, active: bool) -> Option<bool> {
        (self.active.replace(active) != Some(active)).then_some(active)
    }
}

/// Settles one activation check: each fullscreen window takes its level on its
/// own, so one failed window leaves the others settled, and a change of
/// activation reaches Main even after a window failed, since it is observed
/// once and never resent. Returns every failure, for the log.
pub fn settle_activation(
    changed: Option<bool>,
    windows: &[(String, Surface)],
    active: bool,
    mut relevel: impl FnMut(&str, Surface, Level) -> Result<(), String>,
    notify: impl FnOnce(bool) -> Result<(), String>,
) -> Vec<String> {
    let mut failures: Vec<String> = windows
        .iter()
        .filter_map(|(label, surface)| {
            relevel(label, *surface, level(true, active))
                .err()
                .map(|error| format!("{label}: {error}"))
        })
        .collect();
    if let Some(active) = changed {
        if let Err(error) = notify(active) {
            failures.push(format!("{ACTIVATION_EVENT}: {error}"));
        }
    }
    failures
}

static STATE: LazyLock<Mutex<FullscreenState>> =
    LazyLock::new(|| Mutex::new(FullscreenState::default()));
static ACTIVATION_CHECK_PENDING: AtomicBool = AtomicBool::new(true);

/// The event Main hears when OneCopy stops or starts being the active app.
pub const ACTIVATION_EVENT: &str = "app://activation";

fn state() -> Result<std::sync::MutexGuard<'static, FullscreenState>, String> {
    STATE
        .lock()
        .map_err(|_| "fullscreen state lock is poisoned".to_string())
}

pub fn is_fullscreen(label: &str) -> bool {
    STATE.lock().is_ok_and(|state| state.contains(label))
}

/// Enters or leaves fullscreen. Must run on the main thread.
pub fn set(app: &AppHandle, label: &str, enable: bool, surface: Surface) -> Result<(), String> {
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| format!("no window labeled {label}"))?;
    #[cfg(target_os = "macos")]
    {
        macos::set(&window, enable, surface)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let changed = if enable {
            state()?.enter(label, surface, None)
        } else {
            state()?.leave(label).is_some()
        };
        if changed {
            let raised = landing_level(enable, || application_active(app))? == Level::Raised;
            match surface {
                Surface::Focused => window.set_fullscreen(enable),
                Surface::Spread => Ok(()),
            }
            .and_then(|()| window.set_always_on_top(raised))
            .map_err(|error| error.to_string())?;
        }
        note_focus_transition();
        Ok(())
    }
}

pub fn window_destroyed(label: &str) {
    if let Ok(mut state) = STATE.lock() {
        state.leave(label);
    }
    note_focus_transition();
}

pub fn note_focus_transition() {
    ACTIVATION_CHECK_PENDING.store(true, Ordering::SeqCst);
}

/// After a focus change settles: re-levels every fullscreen window for the
/// app's activation, and tells Main when the activation itself changed.
pub fn reconcile_activation(app: &AppHandle) -> Result<(), String> {
    if !ACTIVATION_CHECK_PENDING.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    let active = application_active(app)?;
    let (changed, windows) = {
        let mut state = state()?;
        (state.observe_activation(active), state.windows())
    };
    let failures = settle_activation(
        changed,
        &windows,
        active,
        |label, _surface, level| {
            let Some(window) = app.get_webview_window(label) else {
                return Ok(());
            };
            #[cfg(target_os = "macos")]
            {
                macos::apply_level(&window, level)
            }
            #[cfg(not(target_os = "macos"))]
            {
                window
                    .set_always_on_top(level == Level::Raised)
                    .map_err(|error| error.to_string())
            }
        },
        |active| {
            app.emit_to("main", ACTIVATION_EVENT, active)
                .map_err(|error| error.to_string())
        },
    );
    for failure in failures {
        crate::logging::warn(
            "fullscreen window could not follow activation",
            serde_json::json!({ "error": { "message": failure } }),
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn application_active(_app: &AppHandle) -> Result<bool, String> {
    let marker = objc2::MainThreadMarker::new()
        .ok_or_else(|| "macOS activation must be read on the main thread".to_string())?;
    Ok(objc2_app_kit::NSApplication::sharedApplication(marker).isActive())
}

#[cfg(windows)]
fn application_active(_app: &AppHandle) -> Result<bool, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: both calls take no pointers but the out-parameter, which lives
    // on this stack frame for the call's duration.
    let owner = unsafe {
        let mut process = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), &mut process);
        process
    };
    Ok(owner == std::process::id())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn application_active(app: &AppHandle) -> Result<bool, String> {
    Ok(app
        .webview_windows()
        .values()
        .any(|window| window.is_focused().unwrap_or(false)))
}

/// Every OneCopy window refuses Spaces fullscreen, so the green button only
/// zooms and nothing can slide a window into its own Space. tao leaves the
/// collection behaviour unset, which makes resizable windows fullscreen-capable.
pub fn refuse_spaces_fullscreen(window: &tauri::Window) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                if let Ok(raw) = target.ns_window() {
                    // SAFETY: Tauri supplies the retained NSWindow of this live
                    // window, borrowed on AppKit's main thread.
                    let native = unsafe { &*raw.cast::<objc2_app_kit::NSWindow>() };
                    native.setCollectionBehavior(
                        native.collectionBehavior()
                            | objc2_app_kit::NSWindowCollectionBehavior::FullScreenNone,
                    );
                }
            })
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSNormalWindowLevel, NSStatusWindowLevel, NSWindow, NSWindowLevel};
    use tauri::{Manager, WebviewWindow};

    use super::{landing_level, state, Frame, Level, Surface};

    fn with_native<T>(window: &WebviewWindow, f: impl FnOnce(&NSWindow) -> T) -> Result<T, String> {
        let raw = window.ns_window().map_err(|error| error.to_string())?;
        // SAFETY: Tauri supplies the retained NSWindow of this live window, and
        // every caller runs on AppKit's main thread for the borrow.
        Ok(f(unsafe { &*raw.cast::<NSWindow>() }))
    }

    fn native_level(level: Level) -> NSWindowLevel {
        match level {
            Level::Raised => NSStatusWindowLevel,
            Level::Normal => NSNormalWindowLevel,
        }
    }

    pub fn apply_level(window: &WebviewWindow, level: Level) -> Result<(), String> {
        with_native(window, |native| native.setLevel(native_level(level)))
    }

    fn frame_of(native: &NSWindow) -> Frame {
        let frame = native.frame();
        Frame {
            x: frame.origin.x,
            y: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height,
        }
    }

    fn set_frame(native: &NSWindow, target: Frame) {
        let mut frame = native.frame();
        frame.origin.x = target.x;
        frame.origin.y = target.y;
        frame.size.width = target.width;
        frame.size.height = target.height;
        native.setFrame_display(frame, true);
    }

    pub fn set(window: &WebviewWindow, enable: bool, surface: Surface) -> Result<(), String> {
        let label = window.label().to_owned();
        let decorated = window.is_decorated().map_err(|error| error.to_string())?;
        let restore = if enable {
            let restore = decorated.then(|| with_native(window, frame_of)).transpose()?;
            if !state()?.enter(&label, surface, restore) {
                return Ok(());
            }
            restore
        } else {
            match state()?.leave(&label) {
                Some(restore) => restore,
                None => return Ok(()),
            }
        };
        if restore.is_some() {
            window
                .set_decorations(!enable)
                .map_err(|error| error.to_string())?;
        }
        // tao applies a style-mask change on the main queue later and then
        // makes its own view first responder, which leaves the web view
        // without keys until a click. Queued behind it on the same serial
        // queue, the frame, level and web-view focus land after it.
        let window = window.clone();
        dispatch2::DispatchQueue::main().exec_async(move || {
            let landing = landing_level(enable, || {
                super::application_active(window.app_handle())
            });
            let applied = landing.and_then(|landing| with_native(&window, |native| {
                if enable {
                    if let Some(screen) = native.screen() {
                        let frame = screen.frame();
                        set_frame(
                            native,
                            Frame {
                                x: frame.origin.x,
                                y: frame.origin.y,
                                width: frame.size.width,
                                height: frame.size.height,
                            },
                        );
                    }
                } else if let Some(frame) = restore {
                    set_frame(native, frame);
                }
                native.setHasShadow(!enable);
                native.setLevel(native_level(landing));
            }));
            let focused = applied.and_then(|()| {
                if restore.is_some() && window.is_focused().unwrap_or(false) {
                    window.as_ref().set_focus().map_err(|error| error.to_string())
                } else {
                    Ok(())
                }
            });
            if let Err(error) = focused {
                crate::logging::warn(
                    "fullscreen window change failed",
                    serde_json::json!({ "window": window.label(), "error": { "message": error } }),
                );
            }
        });
        super::note_focus_transition();
        Ok(())
    }
}
