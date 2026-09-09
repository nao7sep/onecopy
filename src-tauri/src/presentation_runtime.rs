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
    use objc2_app_kit::{NSApplication, NSApplicationPresentationOptions};

    let marker = MainThreadMarker::new()
        .ok_or_else(|| "macOS presentation state must run on the main thread".to_string())?;
    let application = NSApplication::sharedApplication(marker);
    let mut state = STATE
        .lock()
        .map_err(|_| "presentation state lock is poisoned".to_string())?;
    let Some(normal) = state.normal_options else {
        return Ok(());
    };
    let visible_fullscreen = state.windows.iter().any(|label| {
        app.get_webview_window(label)
            .is_some_and(|window| window.is_visible().unwrap_or(false))
    });
    let options = if state.hides_system_chrome(application.isActive() && visible_fullscreen) {
        NSApplicationPresentationOptions::AutoHideDock
            | NSApplicationPresentationOptions::AutoHideMenuBar
    } else {
        normal
    };
    application.setPresentationOptions(options);
    if state.windows.is_empty() {
        state.normal_options = None;
    }
    Ok(())
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
