//! Process-lifetime admission authority.
//!
//! Worker modules retain ownership of their cancellation, handles, and joins.
//! This module owns only the irreversible transition that tells every owner no
//! new work may be admitted once final shutdown begins.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

struct Lifecycle {
    shutting_down: AtomicBool,
    publication: Mutex<()>,
}

impl Lifecycle {
    const fn new() -> Self {
        Self {
            shutting_down: AtomicBool::new(false),
            publication: Mutex::new(()),
        }
    }

    fn begin_shutdown(&self) -> bool {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !self.shutting_down.swap(true, Ordering::SeqCst)
    }

    fn shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    fn publish_if_running<T>(&self, publish: impl FnOnce() -> T) -> Option<T> {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutting_down() {
            return None;
        }
        Some(publish())
    }
}

static APP: Lifecycle = Lifecycle::new();

/// Closes process-wide work admission. The first caller owns shutdown setup;
/// later Tauri exit events observe the same irreversible state.
pub(crate) fn begin_shutdown() -> bool {
    APP.begin_shutdown()
}

pub(crate) fn shutting_down() -> bool {
    APP.shutting_down()
}

/// Linearizes runtime-to-webview publication with final shutdown. A publish
/// that owns this permit finishes before shutdown begins; once shutdown owns
/// it, no later runtime event reaches a tearing-down webview. The deliberately
/// final `app://exit-quiescing` and shutdown media-release events are emitted
/// directly by the exit owner.
pub(crate) fn publish_if_running<T>(publish: impl FnOnce() -> T) -> Option<T> {
    APP.publish_if_running(publish)
}

// This process singleton is intentionally private application plumbing. A
// separate Lifecycle proves its transition without poisoning the shared test
// process or widening the shipped crate's public API solely for a test.
#[cfg(test)]
mod tests {
    use super::Lifecycle;

    #[test]
    fn final_shutdown_closes_admission_once_and_never_reopens_it() {
        let lifecycle = Lifecycle::new();

        assert!(!lifecycle.shutting_down());
        assert_eq!(lifecycle.publish_if_running(|| 7), Some(7));
        assert!(lifecycle.begin_shutdown());
        assert!(lifecycle.shutting_down());
        assert!(!lifecycle.begin_shutdown());
        assert!(lifecycle.shutting_down());
        assert_eq!(lifecycle.publish_if_running(|| 7), None);
    }
}
