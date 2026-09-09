//! Pure lifecycle of a finite source-check request, including temporary yielding.

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResultState {
    #[default]
    Stopped,
    Completed,
    CompletedWithIssues,
    Failed,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Running,
    Stopping,
    Yielding,
    Queued,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Explicit,
    Automatic,
    Resume,
}

#[derive(Default)]
pub struct SourceCheckState {
    phase: Phase,
    notify_completion: bool,
    pub last_result: ResultState,
}

impl SourceCheckState {
    pub fn running(&self) -> bool {
        self.phase != Phase::Idle
    }
    pub fn stopping(&self) -> bool {
        self.phase == Phase::Stopping
    }
    pub fn waiting(&self) -> bool {
        self.phase == Phase::Queued
    }
    pub fn cancelled(&self) -> bool {
        matches!(self.phase, Phase::Stopping | Phase::Yielding | Phase::Idle)
    }

    pub fn begin(&mut self, request: Request, foreground_pending: bool) -> bool {
        if !matches!(self.phase, Phase::Idle | Phase::Queued) {
            return false;
        }
        if request == Request::Resume && (self.phase != Phase::Queued || foreground_pending) {
            return false;
        }
        if request != Request::Resume {
            self.notify_completion = request == Request::Explicit;
        }
        self.phase = Phase::Running;
        true
    }

    pub fn stop(&mut self) -> bool {
        let requested = self.running();
        self.notify_completion = false;
        self.phase = match self.phase {
            Phase::Idle | Phase::Queued => Phase::Idle,
            _ => Phase::Stopping,
        };
        requested
    }

    pub fn preempt(&mut self) {
        // A foreground interruption must never override an explicit Stop.
        if self.phase == Phase::Running {
            self.phase = Phase::Yielding;
        }
    }

    /// Claims the explicit request's one completion notice at its terminal
    /// boundary. A yielded attempt retains that intent for its continuation.
    pub fn finish(&mut self, result: ResultState) -> bool {
        self.phase = if self.phase == Phase::Yielding && matches!(result, ResultState::Stopped) {
            Phase::Queued
        } else {
            Phase::Idle
        };
        self.last_result = result;
        let notify = self.notify_completion
            && matches!(
                result,
                ResultState::Completed | ResultState::CompletedWithIssues
            );
        if self.phase == Phase::Idle {
            self.notify_completion = false;
        }
        notify
    }
}
