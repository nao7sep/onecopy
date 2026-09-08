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

#[derive(Default)]
pub struct SourceCheckState {
    phase: Phase,
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

    pub fn begin(&mut self, explicit: bool, foreground_pending: bool) -> bool {
        if !matches!(self.phase, Phase::Idle | Phase::Queued) {
            return false;
        }
        if !explicit && (self.phase != Phase::Queued || foreground_pending) {
            return false;
        }
        self.phase = Phase::Running;
        true
    }

    pub fn stop(&mut self) -> bool {
        let requested = self.running();
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

    pub fn finish(&mut self, result: ResultState) {
        self.phase = if self.phase == Phase::Yielding && matches!(result, ResultState::Stopped) {
            Phase::Queued
        } else {
            Phase::Idle
        };
        self.last_result = result;
    }
}
