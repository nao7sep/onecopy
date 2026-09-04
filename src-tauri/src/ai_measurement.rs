//! Scoped observation for production AI operations. Ordinary application
//! work uses [`NOOP`]; test-only scenario execution may supply a collector
//! without introducing process-global state or a benchmark-specific path.

use std::time::{Duration, Instant};

pub trait Observer {
    fn enabled(&self) -> bool {
        false
    }

    fn phase(&self, _name: &'static str, _elapsed: Duration) {}
}

pub struct NoopObserver;

impl Observer for NoopObserver {}

pub static NOOP: NoopObserver = NoopObserver;

pub struct Span<'a> {
    observer: &'a dyn Observer,
    name: &'static str,
    started: Option<Instant>,
}

impl<'a> Span<'a> {
    pub fn begin(observer: &'a dyn Observer, name: &'static str) -> Self {
        Self {
            observer,
            name,
            started: observer.enabled().then(Instant::now),
        }
    }
}

impl Drop for Span<'_> {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            self.observer.phase(self.name, started.elapsed());
        }
    }
}
