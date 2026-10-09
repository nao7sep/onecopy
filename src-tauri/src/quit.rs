//! Native quit routes and actual OS session-end takeover. Main holds the only
//! unsaved playback patch; its normal close never destroys it before saving.
use std::sync::{atomic::{AtomicBool, Ordering}, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use serde_json::json;

const SAVE_WAIT: Duration = Duration::from_millis(1500);
static SESSION_ENDING: AtomicBool = AtomicBool::new(false);
static SESSION_COMMITTED: AtomicBool = AtomicBool::new(false);
static SAVED: (Mutex<(u64, bool)>, Condvar) = (Mutex::new((0, false)), Condvar::new());

/// Whether the OS is ending the session, which skips optional exit work.
pub(crate) fn session_ending() -> bool {
    SESSION_ENDING.load(Ordering::SeqCst)
}

pub(crate) fn request_quit(app: &AppHandle) {
    if SESSION_ENDING.load(Ordering::SeqCst) || crate::app_lifecycle::shutting_down() { return; }
    if app.get_webview_window("main").is_none() {
        // No renderer owns pending work: Main's ordinary close is intercepted
        // and saves before shutdown; do not recreate the workspace to quit.
        crate::app_lifecycle::quiesce(app);
    } else if let Err(error) = app.emit_to("main", "app://quit-request", ()) {
        crate::logging::warn("quit request delivery failed; quitting cancelled", json!({ "error": error.to_string() }));
    }
}

fn prepare_session_end(app: &AppHandle) {
    if SESSION_ENDING.swap(true, Ordering::SeqCst) { return; }
    let id = {
        let mut saved = SAVED.0.lock().unwrap_or_else(|p| p.into_inner());
        saved.0 += 1;
        saved.1 = false;
        saved.0
    };
    if app.get_webview_window("main").is_none() {
        session_end_saved(id);
    } else if let Err(error) = app.emit_to("main", "app://session-ending", id) {
        crate::logging::warn("session-end save delivery failed", json!({ "error": error.to_string() }));
        session_end_saved(id);
    }
}

pub(crate) fn session_end_saved(id: u64) {
    let mut saved = SAVED.0.lock().unwrap_or_else(|p| p.into_inner());
    if saved.0 != id { return; }
    saved.1 = true;
    SAVED.1.notify_all();
}

fn commit_session_end(app: &AppHandle) {
    crate::app_lifecycle::start_exit_deadline(crate::app_lifecycle::SESSION_EXIT_DEADLINE);
    prepare_session_end(app);
    if SESSION_COMMITTED.swap(true, Ordering::SeqCst) { return; }
    let app = app.clone();
    let fallback = app.clone();
    if std::thread::Builder::new().name("onecopy-session-end".into()).spawn(move || {
        let saved = SAVED.0.lock().unwrap_or_else(|p| p.into_inner());
        let _ = SAVED.1.wait_timeout_while(saved, SAVE_WAIT, |saved| !saved.1).unwrap_or_else(|p| p.into_inner());
        crate::app_lifecycle::quiesce(&app);
    }).is_err() {
        crate::app_lifecycle::quiesce(&fallback);
    }
}

#[cfg(windows)]
fn cancel_session_end(app: &AppHandle) {
    if SESSION_COMMITTED.load(Ordering::SeqCst) { return; }
    SAVED.0.lock().unwrap_or_else(|p| p.into_inner()).0 += 1;
    SESSION_ENDING.store(false, Ordering::SeqCst);
    let _ = app.emit_to("main", "app://session-end-cancelled", ());
}

pub(crate) fn install(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    macos::install(app);
    #[cfg(windows)]
    windows::install(app);
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = app;
}

pub(crate) fn window_loaded(window: &tauri::Window) {
    #[cfg(windows)]
    windows::install_window(window);
    #[cfg(not(windows))]
    let _ = window;
}

pub(crate) fn finish_session_end(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    macos::finish(app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(any(target_os = "macos", test))]
fn session_quit(event: Option<u32>, has_reason: bool) -> bool {
    event == Some(u32::from_be_bytes(*b"quit")) && has_reason
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::sync::OnceLock;
    use objc2::{class, ffi, msg_send, runtime::{AnyObject, Bool, Imp, Sel}, sel};
    static APP: OnceLock<AppHandle> = OnceLock::new();
    static AWAIT_REPLY: AtomicBool = AtomicBool::new(false);

    pub(super) fn install(app: &AppHandle) {
        if APP.set(app.clone()).is_err() { return; }
        // SAFETY: setup runs on the main thread; Tao's delegate has no
        // applicationShouldTerminate method. This is its AppKit signature.
        let added = unsafe {
            let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let delegate: *mut AnyObject = msg_send![application, delegate];
            if delegate.is_null() { false } else {
                let method: extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize = should_terminate;
                let imp: Imp = std::mem::transmute(method);
                ffi::class_addMethod(ffi::object_getClass(delegate) as *mut _, sel!(applicationShouldTerminate:), imp, c"Q@:@".as_ptr()).as_bool()
            }
        };
        if !added { crate::logging::warn("quit hook not installed", json!({})); }
    }

    extern "C-unwind" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
        let Some(app) = APP.get() else { return 0; };
        // SAFETY: read the Apple Event on AppKit's main thread. AERegistry.h
        // specifies 'why?' as a parameter; some callers use an attribute.
        let (event_id, reason) = unsafe {
            let manager: *mut AnyObject = msg_send![class!(NSAppleEventManager), sharedAppleEventManager];
            let event: *mut AnyObject = msg_send![manager, currentAppleEvent];
            if event.is_null() { (None, false) } else {
                let event_id: u32 = msg_send![event, eventID];
                let keyword = u32::from_be_bytes(*b"why?");
                let attribute: *mut AnyObject = msg_send![event, attributeDescriptorForKeyword: keyword];
                let parameter: *mut AnyObject = msg_send![event, paramDescriptorForKeyword: keyword];
                (Some(event_id), !attribute.is_null() || !parameter.is_null())
            }
        };
        if crate::app_lifecycle::exit_ready() { return 1; }
        if session_quit(event_id, reason) {
            AWAIT_REPLY.store(true, Ordering::SeqCst);
            commit_session_end(app);
            2 // NSTerminateLater; reply once cleanup settles.
        } else {
            request_quit(app);
            0 // NSTerminateCancel while the user save/decision is pending.
        }
    }

    pub(super) fn finish(app: &AppHandle) {
        if !AWAIT_REPLY.load(Ordering::SeqCst) { return; }
        let _ = app.run_on_main_thread(|| {
            if !AWAIT_REPLY.swap(false, Ordering::SeqCst) { return; }
            // SAFETY: exactly one main-thread answer to NSTerminateLater.
            unsafe {
                let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
                let _: () = msg_send![application, replyToApplicationShouldTerminate: Bool::YES];
            }
        });
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{sync::OnceLock, time::Instant};
    use windows_sys::Win32::{Foundation::{HWND, LPARAM, LRESULT, WPARAM}, UI::{Shell::{SetWindowSubclass, RemoveWindowSubclass, DefSubclassProc}, WindowsAndMessaging::{DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, PostQuitMessage, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION, WM_QUIT}}};
    static APP: OnceLock<AppHandle> = OnceLock::new();
    const ID: usize = 0x4f4351;

    pub(super) fn install(app: &AppHandle) {
        APP.get_or_init(|| app.clone());
        for window in app.webview_windows().values() {
            install_window(&window.as_ref().window());
        }
    }

    pub(super) fn install_window(window: &tauri::Window) {
        APP.get_or_init(|| window.app_handle().clone());
        match window.hwnd() {
            Ok(hwnd) => {
                // SAFETY: live HWND belongs to this setup/page-load thread;
                // reinstalling the same callback and ID updates that subclass.
                if unsafe { SetWindowSubclass(hwnd.0, Some(on_message), ID, 0) } == 0 {
                    crate::logging::warn("quit hook not installed", json!({}));
                }
            },
            Err(error) => crate::logging::warn("quit hook not installed", json!({ "error": error.to_string() })),
        }
    }

    unsafe extern "system" fn on_message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        if let Some(app) = APP.get() {
            if message == WM_QUERYENDSESSION {
                // Tentative: do not close mutation admission or arm a forced
                // exit until Windows confirms ENDSESSION(TRUE).
                prepare_session_end(app);
                return 1;
            }
            if message == WM_ENDSESSION {
                if wparam == 0 { cancel_session_end(app); }
                else {
                    commit_session_end(app);
                    pump_until_exit();
                }
            }
        }
        // SAFETY: normal subclass forwarding; remove our callback at the
        // window's final destruction, before Tao releases its HWND.
        unsafe {
            if message == WM_NCDESTROY { RemoveWindowSubclass(hwnd, Some(on_message), ID); }
            DefSubclassProc(hwnd, message, wparam, lparam)
        }
    }

    fn pump_until_exit() {
        let deadline = Instant::now() + crate::app_lifecycle::SESSION_EXIT_DEADLINE;
        // SAFETY: message pumping stays on the HWND-owning thread, allowing
        // renderer save IPC to finish while ENDSESSION is being answered.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while !crate::app_lifecycle::exit_ready() && Instant::now() < deadline {
                let wait = deadline.saturating_duration_since(Instant::now()).as_millis().min(20) as u32;
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait, QS_ALLINPUT);
                loop {
                    if crate::app_lifecycle::exit_ready() || Instant::now() >= deadline { return; }
                    if PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) == 0 { break; }
                    if message.message == WM_QUIT { PostQuitMessage(message.wParam as i32); return; }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/quit.rs"]
mod tests;
