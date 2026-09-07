#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "macos")]
use std::sync::{LazyLock, Mutex};

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::Manager;

#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
struct PresentationState {
    desired: Option<String>,
    applied: Option<String>,
}

#[cfg(any(target_os = "macos", test))]
impl PresentationState {
    fn set_desired(&mut self, label: &str, enable: bool) {
        if enable {
            self.desired = Some(label.to_string());
        } else if self.desired.as_deref() == Some(label) {
            self.desired = None;
        }
    }

    fn target(&self, application_active: bool) -> Option<String> {
        if !application_active {
            return None;
        }
        self.desired.clone()
    }
}

#[cfg(target_os = "macos")]
static STATE: LazyLock<Mutex<PresentationState>> =
    LazyLock::new(|| Mutex::new(PresentationState::default()));
#[cfg(target_os = "macos")]
static ACTIVATION_RECONCILE_PENDING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn application_active() -> Result<bool, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let marker = MainThreadMarker::new()
        .ok_or_else(|| "macOS presentation state must be reconciled on the main thread".to_string())?;
    Ok(NSApplication::sharedApplication(marker).isActive())
}

#[cfg(target_os = "macos")]
fn apply(app: &AppHandle, active: bool) -> Result<(), String> {
    let mut state = STATE
        .lock()
        .map_err(|_| "presentation state lock is poisoned".to_string())?;
    let target = state.target(active);
    if state.applied == target {
        return Ok(());
    }

    if let Some(label) = state.applied.clone() {
        let window = app
            .get_webview_window(&label)
            .ok_or_else(|| format!("no window labeled {label}"))?;
        window
            .set_simple_fullscreen(false)
            .map_err(|error| error.to_string())?;
        state.applied = None;
    }

    if let Some(label) = target {
        let window = app
            .get_webview_window(&label)
            .ok_or_else(|| format!("no window labeled {label}"))?;
        window
            .set_simple_fullscreen(true)
            .map_err(|error| error.to_string())?;
        state.applied = Some(label);
    }
    Ok(())
}

/// Updates durable presentation intent, then applies it only while OneCopy is
/// active. Exactly one window owns Tauri's process-global macOS presentation
/// options even when Comparison spans several borderless display windows.
pub fn set_desired(app: &AppHandle, label: &str, enable: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if enable && app.get_webview_window(label).is_none() {
            return Err(format!("no window labeled {label}"));
        }
        STATE
            .lock()
            .map_err(|_| "presentation state lock is poisoned".to_string())?
            .set_desired(label, enable);
        apply(app, application_active()?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, label, enable);
        Ok(())
    }
}

/// Marks an app-window focus transition for reconciliation after the event
/// loop has delivered the complete activation/deactivation sequence.
pub fn note_focus_transition() {
    #[cfg(target_os = "macos")]
    {
        ACTIVATION_RECONCILE_PENDING.store(true, Ordering::SeqCst);
    }
}

/// Reconciles at MainEventsCleared rather than inside windowDidResignKey.
/// AppKit may still consider the application active during that earlier
/// callback; after the event batch its application-active bit reliably
/// distinguishes leaving OneCopy from moving among OneCopy windows.
pub fn reconcile_pending_activation(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if ACTIVATION_RECONCILE_PENDING.swap(false, Ordering::SeqCst) {
            apply(app, application_active()?)
        } else {
            Ok(())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

/// Clears intent and restores system chrome before longer shutdown work.
pub fn shutdown(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        STATE
            .lock()
            .map_err(|_| "presentation state lock is poisoned".to_string())?
            .desired = None;
        apply(app, false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PresentationState;

    #[test]
    fn a_new_request_transfers_process_global_presentation_ownership() {
        let mut state = PresentationState::default();
        state.set_desired("comparison-1", true);
        state.applied = state.target(true);
        state.set_desired("viewer", true);

        assert_eq!(state.target(true).as_deref(), Some("viewer"));
    }

    #[test]
    fn deactivation_suspends_application_presentation_without_losing_intent() {
        let mut state = PresentationState::default();
        state.set_desired("viewer", true);
        state.applied = state.target(true);

        assert_eq!(state.target(false), None);
        assert_eq!(state.desired.as_deref(), Some("viewer"));
        state.applied = state.target(false);
        assert_eq!(state.target(true).as_deref(), Some("viewer"));
    }

    #[test]
    fn a_stale_exit_does_not_revoke_the_current_owner() {
        let mut state = PresentationState::default();
        state.set_desired("comparison-1", true);
        state.applied = state.target(true);
        state.set_desired("viewer", true);
        state.set_desired("comparison-1", false);

        assert_eq!(state.target(true).as_deref(), Some("viewer"));
    }
}
