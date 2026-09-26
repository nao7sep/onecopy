//! The one rule for publishing progress events: an update is published when
//! its step changes (a phase, or a failure count), when it completes, or when
//! the interval has passed since the last published update. Every progress
//! stream (index work, file operations, managed-tool installs) reads it.

use std::time::{Duration, Instant};

pub const INTERVAL: Duration = Duration::from_millis(125);

pub struct ProgressThrottle<K> {
    last_step: Option<K>,
    last_publish: Option<Instant>,
}

impl<K: PartialEq> Default for ProgressThrottle<K> {
    fn default() -> Self {
        Self {
            last_step: None,
            last_publish: None,
        }
    }
}

impl<K: PartialEq> ProgressThrottle<K> {
    /// Whether this update is published; a published update becomes the
    /// reference for the next one.
    pub fn admit(&mut self, step: K, completed: bool, now: Instant) -> bool {
        let due = self.last_step.as_ref() != Some(&step)
            || completed
            || self
                .last_publish
                .is_none_or(|last| now.duration_since(last) >= INTERVAL);
        if due {
            self.last_step = Some(step);
            self.last_publish = Some(now);
        }
        due
    }
}
