//! Scoped observation for production AI operations. Ordinary application
//! work uses [`NOOP`]; test-only scenario execution may supply a collector
//! without introducing process-global state or a benchmark-specific path.

#[cfg(feature = "ai-test-support")]
use std::time::{Duration, Instant};

#[cfg(feature = "ai-test-support")]
pub trait Observer {
    fn enabled(&self) -> bool {
        false
    }

    fn phase(&self, _name: &'static str, _elapsed: Duration) {}
}

#[cfg(not(feature = "ai-test-support"))]
pub trait Observer {}

pub struct NoopObserver;

impl Observer for NoopObserver {}

pub static NOOP: NoopObserver = NoopObserver;

#[cfg(feature = "ai-test-support")]
pub struct Span<'a> {
    observer: &'a dyn Observer,
    name: &'static str,
    started: Option<Instant>,
}

#[cfg(feature = "ai-test-support")]
impl<'a> Span<'a> {
    pub fn begin(observer: &'a dyn Observer, name: &'static str) -> Self {
        Self {
            observer,
            name,
            started: observer.enabled().then(Instant::now),
        }
    }
}

#[cfg(feature = "ai-test-support")]
impl Drop for Span<'_> {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            self.observer.phase(self.name, started.elapsed());
        }
    }
}

#[cfg(not(feature = "ai-test-support"))]
pub struct Span;

#[cfg(not(feature = "ai-test-support"))]
impl Span {
    pub fn begin(_observer: &dyn Observer, _name: &'static str) -> Self {
        Self
    }
}
