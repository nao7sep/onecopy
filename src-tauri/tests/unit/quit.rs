use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use super::{session_message, terminate_reply, SessionEnd, SessionMessage, TerminateReply};

const QUIT: Option<u32> = Some(u32::from_be_bytes(*b"quit"));

#[test]
fn only_a_quit_apple_event_with_a_session_reason_is_os_takeover() {
    assert_eq!(terminate_reply(false, QUIT, true), TerminateReply::Later);
    assert_eq!(terminate_reply(false, QUIT, false), TerminateReply::Cancel, "Dock quit is ordinary user quit");
    assert_eq!(terminate_reply(false, None, true), TerminateReply::Cancel);
    assert_eq!(terminate_reply(false, Some(u32::from_be_bytes(*b"open")), true), TerminateReply::Cancel);
}

#[test]
fn a_finished_exit_sequence_lets_any_terminate_proceed_at_once() {
    assert_eq!(terminate_reply(true, QUIT, true), TerminateReply::Now);
    assert_eq!(terminate_reply(true, None, false), TerminateReply::Now);
}

#[test]
fn the_terminate_replies_are_appkits_values() {
    assert_eq!(TerminateReply::Cancel as usize, 0);
    assert_eq!(TerminateReply::Now as usize, 1);
    assert_eq!(TerminateReply::Later as usize, 2);
}

#[test]
fn windows_session_messages_allow_the_query_and_end_only_when_confirmed() {
    assert_eq!(session_message(0x0011, 0), Some(SessionMessage::Query));
    assert_eq!(session_message(0x0016, 1), Some(SessionMessage::Ending));
    assert_eq!(session_message(0x0016, 0), Some(SessionMessage::Cancelled));
    assert_eq!(session_message(0x0010, 0), None, "WM_CLOSE is not a session end");
}

#[test]
fn one_session_end_asks_main_for_one_save() {
    let session = SessionEnd::new();
    let delivered = Cell::new(Vec::new());
    for _ in 0..3 {
        session.prepare(|id| { let mut ids = delivered.take(); ids.push(id); delivered.set(ids); true });
    }
    assert_eq!(delivered.take().len(), 1);
    assert!(session.ending());
}

#[test]
fn a_save_request_nothing_can_answer_releases_the_wait_at_once() {
    let session = SessionEnd::new();
    session.prepare(|_| false);
    // The wait checks this before it sleeps.
    assert!(session.saved.lock().unwrap().1);
}

#[test]
fn main_acknowledging_its_save_releases_the_waiting_exit() {
    let session = Arc::new(SessionEnd::new());
    let id = Arc::new(std::sync::Mutex::new(0));
    let recorded = id.clone();
    session.prepare(move |delivered| { *recorded.lock().unwrap() = delivered; true });
    let acknowledging = session.clone();
    let id = *id.lock().unwrap();
    let answer = std::thread::spawn(move || acknowledging.saved(id));
    session.wait_saved(Duration::from_secs(60));
    answer.join().unwrap();
    assert!(session.saved.lock().unwrap().1);
}

#[test]
fn a_silent_main_cannot_hold_the_exit_past_the_save_wait() {
    let session = SessionEnd::new();
    session.prepare(|_| true);
    session.wait_saved(Duration::from_millis(10));
    assert!(!session.saved.lock().unwrap().1);
}

#[test]
fn an_acknowledgement_from_a_cancelled_session_cannot_release_a_later_save() {
    let session = SessionEnd::new();
    let mut first = 0;
    session.prepare(|id| { first = id; true });
    assert!(session.cancel());
    assert!(!session.ending());
    let mut second = 0;
    session.prepare(|id| { second = id; true });
    assert_ne!(first, second);
    session.saved(first);
    assert!(!session.saved.lock().unwrap().1);
    session.saved(second);
    assert!(session.saved.lock().unwrap().1);
}

#[test]
fn a_committed_session_end_owns_one_exit_and_cannot_be_cancelled() {
    let session = SessionEnd::new();
    session.prepare(|_| true);
    assert!(session.commit());
    assert!(!session.commit(), "a later window's WM_ENDSESSION joins the exit already running");
    assert!(!session.cancel());
    assert!(session.ending());
}

// Tao's hidden event-target window exits the process on WM_ENDSESSION. Every
// top-level window of the UI thread is hooked so none can skip the save.
windows_test! {
    "Windows only: subclasses real Win32 windows",
    fn every_top_level_window_of_the_thread_gets_the_session_end_hook() {
        use windows_sys::Win32::UI::Shell::GetWindowSubclass;
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WM_ENDSESSION, WM_QUERYENDSESSION, WS_POPUP};
        assert_eq!((super::WM_QUERYENDSESSION, super::WM_ENDSESSION), (WM_QUERYENDSESSION, WM_ENDSESSION));
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: plain top-level windows of a system class, owned and
        // destroyed by this test thread.
        let windows: Vec<_> = (0..2)
            .map(|_| unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) })
            .collect();
        assert!(windows.iter().all(|hwnd| !hwnd.is_null()));
        super::windows::subclass_thread_windows();
        for hwnd in &windows {
            let mut data = 0;
            // SAFETY: queries this thread's live window.
            let hooked = unsafe { GetWindowSubclass(*hwnd, Some(super::windows::on_message), super::windows::ID, &mut data) };
            assert_ne!(hooked, 0);
            unsafe { DestroyWindow(*hwnd) };
        }
    }
}
