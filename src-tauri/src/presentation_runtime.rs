use std::collections::BTreeSet;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use tauri::{AppHandle, Manager};

/// Fullscreen membership is independent of application activation.
/// Activation controls only the process-wide menu bar and Dock policy.
#[derive(Default)]
pub struct PresentationState {
    windows: BTreeSet<String>,
    #[cfg(target_os = "macos")]
    normal_options: Option<objc2_app_kit::NSApplicationPresentationOptions>,
    #[cfg(target_os = "macos")]
    dock_screen_frames: Vec<(f64, f64, f64, f64)>,
}

impl PresentationState {
    pub fn contains(&self, label: &str) -> bool {
        self.windows.contains(label)
    }

    pub fn set_fullscreen(&mut self, label: &str, enabled: bool) {
        if enabled {
            self.windows.insert(label.to_owned());
        } else {
            self.windows.remove(label);
        }
    }

    pub fn hides_system_chrome(&self, application_active: bool) -> bool {
        application_active && !self.windows.is_empty()
    }
}

static STATE: LazyLock<Mutex<PresentationState>> =
    LazyLock::new(|| Mutex::new(PresentationState::default()));
#[cfg(target_os = "macos")]
static ACTIVATION_RECONCILE_PENDING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn apply_system_chrome(app: &AppHandle) -> Result<(), String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationPresentationOptions, NSWindow};

    let marker = MainThreadMarker::new()
        .ok_or_else(|| "macOS presentation state must run on the main thread".to_string())?;
    let application = NSApplication::sharedApplication(marker);
    let mut state = STATE
        .lock()
        .map_err(|_| "presentation state lock is poisoned".to_string())?;
    let Some(normal) = state.normal_options else {
        return Ok(());
    };
    let full_display_windows = app
        .webview_windows()
        .into_iter()
        .filter_map(|(label, window)| {
            let registered = state.windows.contains(&label);
            let comparison_spread = label
                .strip_prefix("comparison-")
                .is_some_and(|suffix| suffix.parse::<usize>().is_ok());
            (registered || comparison_spread)
                .then_some(window)
                .filter(|window| window.is_visible().unwrap_or(false))
        })
        .collect::<Vec<_>>();
    let visible_fullscreen = !full_display_windows.is_empty();
    let options = if state.hides_system_chrome(application.isActive() && visible_fullscreen) {
        let mut options = normal | NSApplicationPresentationOptions::AutoHideMenuBar;
        let covers_dock_display = full_display_windows.iter().any(|window| {
            let Ok(raw) = window.ns_window() else {
                return false;
            };
            // SAFETY: Tauri supplies the retained NSWindow pointer for this
            // live webview window, and reconciliation runs on AppKit's main
            // thread for the duration of this borrow.
            let native = unsafe { &*raw.cast::<NSWindow>() };
            native.screen().is_some_and(|screen| {
                state
                    .dock_screen_frames
                    .iter()
                    .any(|dock_frame| screen_frames_match(*dock_frame, screen_frame(&screen)))
            })
        });
        if covers_dock_display {
            options |= NSApplicationPresentationOptions::AutoHideDock;
        }
        options
    } else {
        normal
    };
    application.setPresentationOptions(options);
    if state.windows.is_empty() {
        state.normal_options = None;
        state.dock_screen_frames.clear();
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn screen_frame(screen: &objc2_app_kit::NSScreen) -> (f64, f64, f64, f64) {
    let frame = screen.frame();
    (
        frame.origin.x,
        frame.origin.y,
        frame.size.width,
        frame.size.height,
    )
}

#[cfg(target_os = "macos")]
fn screen_frames_match(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> bool {
    const TOLERANCE: f64 = 1.0;
    (a.0 - b.0).abs() <= TOLERANCE
        && (a.1 - b.1).abs() <= TOLERANCE
        && (a.2 - b.2).abs() <= TOLERANCE
        && (a.3 - b.3).abs() <= TOLERANCE
}

#[cfg(target_os = "macos")]
fn dock_screen_frames(marker: objc2::MainThreadMarker) -> Vec<(f64, f64, f64, f64)> {
    objc2_app_kit::NSScreen::screens(marker)
        .iter()
        .filter(|screen| {
            let frame = screen.frame();
            let visible = screen.visibleFrame();
            screen_reserves_dock_space(
                (
                    frame.origin.x,
                    frame.origin.y,
                    frame.size.width,
                    frame.size.height,
                ),
                (
                    visible.origin.x,
                    visible.origin.y,
                    visible.size.width,
                    visible.size.height,
                ),
            )
        })
        .map(|screen| screen_frame(&screen))
        .collect()
}

#[cfg(target_os = "macos")]
fn screen_reserves_dock_space(frame: (f64, f64, f64, f64), visible: (f64, f64, f64, f64)) -> bool {
    const TOLERANCE: f64 = 1.0;
    let (frame_x, frame_y, frame_width, _) = frame;
    let (visible_x, visible_y, visible_width, _) = visible;
    let frame_right = frame_x + frame_width;
    let visible_right = visible_x + visible_width;
    // The menu bar reduces only the top edge. A Dock that is not already set
    // to auto-hide reduces the left, right, or bottom edge of visibleFrame.
    visible_x > frame_x + TOLERANCE
        || visible_y > frame_y + TOLERANCE
        || visible_right < frame_right - TOLERANCE
}

#[cfg(all(test, target_os = "macos"))]
// These helpers interpret AppKit geometry and are meaningful only inside this
// private platform adapter; exposing them would enlarge the shipped API solely
// for tests.
mod tests {
    use super::{screen_frames_match, screen_reserves_dock_space};

    #[test]
    fn top_menu_bar_inset_is_not_mistaken_for_the_dock() {
        assert!(!screen_reserves_dock_space(
            (0.0, 0.0, 2560.0, 1440.0),
            (0.0, 0.0, 2560.0, 1415.0),
        ));
    }

    #[test]
    fn left_right_and_bottom_dock_insets_are_detected() {
        let frame = (0.0, 0.0, 2560.0, 1440.0);
        assert!(screen_reserves_dock_space(
            frame,
            (80.0, 0.0, 2480.0, 1415.0)
        ));
        assert!(screen_reserves_dock_space(
            frame,
            (0.0, 0.0, 2480.0, 1415.0)
        ));
        assert!(screen_reserves_dock_space(
            frame,
            (0.0, 80.0, 2560.0, 1335.0)
        ));
    }

    #[test]
    fn a_fullscreen_window_matches_the_captured_dock_display_by_frame() {
        assert!(screen_frames_match(
            (0.0, 0.0, 2560.0, 1440.0),
            (0.5, -0.5, 2560.0, 1440.0),
        ));
        assert!(!screen_frames_match(
            (0.0, 0.0, 2560.0, 1440.0),
            (2560.0, 0.0, 2560.0, 1440.0),
        ));
    }
}

/// Explicit entry/exit is the only path that changes window geometry.
/// macOS uses non-Spaces fullscreen; Windows uses the native fullscreen path.
pub fn set_desired(app: &AppHandle, label: &str, enable: bool) -> Result<(), String> {
    let window = app
        .get_webview_window(label)
        .ok_or_else(|| format!("no window labeled {label}"))?;
    let mut state = STATE
        .lock()
        .map_err(|_| "presentation state lock is poisoned".to_string())?;
    if state.contains(label) == enable {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        let marker = MainThreadMarker::new()
            .ok_or_else(|| "macOS fullscreen must run on the main thread".to_string())?;
        if enable && state.normal_options.is_none() {
            state.normal_options =
                Some(NSApplication::sharedApplication(marker).presentationOptions());
            state.dock_screen_frames = dock_screen_frames(marker);
        }
        window
            .set_simple_fullscreen(enable)
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    window
        .set_fullscreen(enable)
        .map_err(|error| error.to_string())?;
    state.set_fullscreen(label, enable);
    // Tauri queues native window mutations. Reconcile process options after
    // the event batch, including Tao's own entry/exit presentation writes.
    note_focus_transition();
    Ok(())
}

pub fn window_destroyed(label: &str) {
    if let Ok(mut state) = STATE.lock() {
        state.set_fullscreen(label, false);
    }
    note_focus_transition();
}

pub fn note_focus_transition() {
    #[cfg(target_os = "macos")]
    ACTIVATION_RECONCILE_PENDING.store(true, Ordering::SeqCst);
}

/// Focus changes never enter or leave fullscreen on any window.
pub fn reconcile_pending_activation(_app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if ACTIVATION_RECONCILE_PENDING.swap(false, Ordering::SeqCst) {
        return apply_system_chrome(_app);
    }
    Ok(())
}

pub fn shutdown(_app: &AppHandle) -> Result<(), String> {
    STATE
        .lock()
        .map_err(|_| "presentation state lock is poisoned".to_string())?
        .windows
        .clear();
    #[cfg(target_os = "macos")]
    apply_system_chrome(_app)?;
    Ok(())
}
