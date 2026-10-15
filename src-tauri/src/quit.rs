//! Native quit routes and actual OS session-end takeover. Main holds the only
//! unsaved playback patch; its normal close never destroys it before saving.
use std::sync::{atomic::{AtomicBool, Ordering}, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use serde_json::json;

const SAVE_WAIT: Duration = Duration::from_millis(1500);

/// The OS session-end handshake with Main's required save. One save request
/// per session end; an acknowledgement releases only its own request; a
/// committed session end is final, so a later cancellation cannot undo it.
struct SessionEnd {
    ending: AtomicBool,
    committed: AtomicBool,
    saved: Mutex<(u64, bool)>,
    changed: Condvar,
}

impl SessionEnd {
    const fn new() -> Self {
        Self {
            ending: AtomicBool::new(false),
            committed: AtomicBool::new(false),
            saved: Mutex::new((0, false)),
            changed: Condvar::new(),
        }
    }

    fn ending(&self) -> bool {
        self.ending.load(Ordering::SeqCst)
    }

    /// Asks for the required save once per session end. `deliver` hands the
    /// request id to Main and returns false when nothing can answer it, which
    /// releases the wait at once instead of spending the OS's allowance.
    fn prepare(&self, deliver: impl FnOnce(u64) -> bool) {
        if self.ending.swap(true, Ordering::SeqCst) { return; }
        let id = {
            let mut saved = self.saved.lock().unwrap_or_else(|p| p.into_inner());
            saved.0 += 1;
            saved.1 = false;
            saved.0
        };
        if !deliver(id) { self.saved(id); }
    }

    fn saved(&self, id: u64) {
        let mut saved = self.saved.lock().unwrap_or_else(|p| p.into_inner());
        if saved.0 != id { return; }
        saved.1 = true;
        self.changed.notify_all();
    }

    /// Whether this caller is the first to commit, and so owns the exit.
    fn commit(&self) -> bool {
        !self.committed.swap(true, Ordering::SeqCst)
    }

    fn wait_saved(&self, limit: Duration) {
        let saved = self.saved.lock().unwrap_or_else(|p| p.into_inner());
        let _ = self.changed.wait_timeout_while(saved, limit, |saved| !saved.1).unwrap_or_else(|p| p.into_inner());
    }

    /// Withdraws a tentative session end. False once it was committed.
    #[cfg(any(windows, test))]
    fn cancel(&self) -> bool {
        if self.committed.load(Ordering::SeqCst) { return false; }
        self.saved.lock().unwrap_or_else(|p| p.into_inner()).0 += 1;
        self.ending.store(false, Ordering::SeqCst);
        true
    }
}

static SESSION: SessionEnd = SessionEnd::new();

/// Whether the OS is ending the session, which skips optional exit work.
pub(crate) fn session_ending() -> bool {
    SESSION.ending()
}

pub(crate) fn request_quit(app: &AppHandle) {
    if SESSION.ending() || crate::app_lifecycle::shutting_down() { return; }
    if app.get_webview_window("main").is_none() {
        // No renderer owns pending work: Main's ordinary close is intercepted
        // and saves before shutdown; do not recreate the workspace to quit.
        crate::app_lifecycle::quiesce(app);
    } else if let Err(error) = app.emit_to("main", "app://quit-request", ()) {
        crate::logging::warn("quit request delivery failed; quitting cancelled", json!({ "error": error.to_string() }));
    }
}

fn prepare_session_end(app: &AppHandle) {
    SESSION.prepare(|id| {
        if app.get_webview_window("main").is_none() { return false; }
        match app.emit_to("main", "app://session-ending", id) {
            Ok(()) => true,
            Err(error) => {
                crate::logging::warn("session-end save delivery failed", json!({ "error": error.to_string() }));
                false
            },
        }
    });
}

pub(crate) fn session_end_saved(id: u64) {
    SESSION.saved(id);
}

/// A quit question shown on its own, owned by no window, for when Main is
/// minimized or hidden: a question never brings the workspace back to get an
/// owner (window-conventions, modal-dialog-conventions). Returns the index of
/// the chosen label; the first label is the safe one, and a dismissed or
/// failed dialog answers it. The renderer settles the question itself when
/// the OS ends the session, so this dialog never holds the exit: on macOS it
/// is a system alert on its own thread, on Windows a task dialog on its own.
///
/// rfd directly, as the startup halt uses it: the dialog plugin's
/// custom-label mapping reports a dismissed or failed dialog as its second
/// label, so either Return or a failure would choose the destructive answer.
pub(crate) async fn standalone_question(title: String, body: String, labels: Vec<String>) -> usize {
    let buttons = match labels.as_slice() {
        [first, second] => rfd::MessageButtons::OkCancelCustom(first.clone(), second.clone()),
        [first, second, third] => rfd::MessageButtons::YesNoCancelCustom(first.clone(), second.clone(), third.clone()),
        _ => return 0,
    };
    let answer = rfd::AsyncMessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title(title)
        .set_description(body)
        .set_buttons(buttons)
        .show()
        .await;
    chosen(&labels, &answer)
}

/// The first label is the dialog's default button on both platforms, and
/// anything but a labelled button press answers it.
fn chosen(labels: &[String], answer: &rfd::MessageDialogResult) -> usize {
    match answer {
        rfd::MessageDialogResult::Custom(label) => labels.iter().position(|candidate| candidate == label).unwrap_or(0),
        _ => 0,
    }
}

fn commit_session_end(app: &AppHandle) {
    crate::app_lifecycle::start_exit_deadline(crate::app_lifecycle::SESSION_EXIT_DEADLINE);
    prepare_session_end(app);
    if !SESSION.commit() { return; }
    let app = app.clone();
    let fallback = app.clone();
    if std::thread::Builder::new().name("onecopy-session-end".into()).spawn(move || {
        SESSION.wait_saved(SAVE_WAIT);
        crate::app_lifecycle::quiesce(&app);
    }).is_err() {
        crate::app_lifecycle::quiesce(&fallback);
    }
}

#[cfg(windows)]
fn cancel_session_end(app: &AppHandle) {
    if SESSION.cancel() {
        let _ = app.emit_to("main", "app://session-end-cancelled", ());
    }
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

/// AppKit's `NSApplicationTerminateReply` values.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminateReply {
    Cancel = 0,
    Now = 1,
    Later = 2,
}

/// The answer to `applicationShouldTerminate:`. A quit Apple event carrying a
/// session reason ('why?': logout, restart, shutdown) is the OS ending the
/// session: never cancelled, answered later once the save and cleanup settle.
/// Any other terminate (Dock Quit, Cmd+Q from AppKit) is cancelled while the
/// ordinary quit asks Main, unless the exit sequence is already ready.
#[cfg(any(target_os = "macos", test))]
fn terminate_reply(exit_ready: bool, event: Option<u32>, has_reason: bool) -> TerminateReply {
    if exit_ready { return TerminateReply::Now; }
    if event == Some(u32::from_be_bytes(*b"quit")) && has_reason { TerminateReply::Later } else { TerminateReply::Cancel }
}

/// WinUser.h's session-end messages, kept here so the decision below is
/// testable on every platform.
#[cfg(any(windows, test))]
const WM_QUERYENDSESSION: u32 = 0x0011;
#[cfg(any(windows, test))]
const WM_ENDSESSION: u32 = 0x0016;

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum SessionMessage {
    /// Windows asks whether the session may end; always allowed.
    Query,
    /// Another app vetoed, so the session goes on.
    Cancelled,
    /// The session ends once this message returns.
    Ending,
}

#[cfg(any(windows, test))]
fn session_message(message: u32, wparam: usize) -> Option<SessionMessage> {
    match message {
        WM_QUERYENDSESSION => Some(SessionMessage::Query),
        WM_ENDSESSION if wparam == 0 => Some(SessionMessage::Cancelled),
        WM_ENDSESSION => Some(SessionMessage::Ending),
        _ => None,
    }
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
        let reply = terminate_reply(crate::app_lifecycle::exit_ready(), event_id, reason);
        match reply {
            TerminateReply::Now => {},
            TerminateReply::Later => {
                AWAIT_REPLY.store(true, Ordering::SeqCst);
                commit_session_end(app);
            },
            TerminateReply::Cancel => request_quit(app),
        }
        reply as usize
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
    use windows_sys::{core::BOOL, Win32::{Foundation::{HWND, LPARAM, LRESULT, WPARAM}, System::Threading::GetCurrentThreadId, UI::{Shell::{SetWindowSubclass, RemoveWindowSubclass, DefSubclassProc}, WindowsAndMessaging::{DispatchMessageW, EnumThreadWindows, MsgWaitForMultipleObjects, PeekMessageW, PostQuitMessage, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT, WM_NCDESTROY, WM_QUIT}}}};
    static APP: OnceLock<AppHandle> = OnceLock::new();
    pub(super) const ID: usize = 0x4f4351;

    pub(super) fn install(app: &AppHandle) {
        APP.get_or_init(|| app.clone());
        subclass_thread_windows();
    }

    /// Hooks every top-level window of the calling thread. Windows sends the
    /// session-end messages to each of them, and tao's hidden event-target
    /// window answers `WM_ENDSESSION` by exiting the process at once; hooked,
    /// it reaches tao only after the required save and cleanup settle.
    pub(super) fn subclass_thread_windows() {
        unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> BOOL {
            subclass(hwnd);
            1
        }
        // SAFETY: enumerates this thread's own windows; the callback only
        // subclasses each one on this, its owning, thread.
        unsafe { EnumThreadWindows(GetCurrentThreadId(), Some(each), 0) };
    }

    pub(super) fn install_window(window: &tauri::Window) {
        APP.get_or_init(|| window.app_handle().clone());
        match window.hwnd() {
            Ok(hwnd) => subclass(hwnd.0),
            Err(error) => crate::logging::warn("quit hook not installed", json!({ "error": error.to_string() })),
        }
    }

    fn subclass(hwnd: HWND) {
        // SAFETY: live HWND belongs to this setup/page-load thread;
        // reinstalling the same callback and ID updates that subclass.
        if unsafe { SetWindowSubclass(hwnd, Some(on_message), ID, 0) } == 0 {
            crate::logging::warn("quit hook not installed", json!({}));
        }
    }

    pub(super) unsafe extern "system" fn on_message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        if let Some(app) = APP.get() {
            match session_message(message, wparam) {
                // Tentative: do not close mutation admission or arm a forced
                // exit until Windows confirms ENDSESSION(TRUE). Never a veto.
                Some(SessionMessage::Query) => {
                    prepare_session_end(app);
                    return 1;
                },
                Some(SessionMessage::Cancelled) => cancel_session_end(app),
                Some(SessionMessage::Ending) => {
                    commit_session_end(app);
                    pump_until_exit();
                },
                None => {},
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
