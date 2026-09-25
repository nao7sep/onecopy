//! Pure lifecycle of a finite source-check request, including waiting in
//! place while a foreground action owns the index.

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
    /// Parked at a safe point while a foreground action owns the index; the
    /// walk continues from there.
    Waiting,
    Stopping,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Explicit,
    Automatic,
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
        self.phase == Phase::Waiting
    }
    pub fn cancelled(&self) -> bool {
        matches!(self.phase, Phase::Stopping | Phase::Idle)
    }

    pub fn begin(&mut self, request: Request) -> bool {
        if self.phase != Phase::Idle {
            return false;
        }
        self.notify_completion = request == Request::Explicit;
        self.phase = Phase::Running;
        true
    }

    pub fn stop(&mut self) -> bool {
        let requested = self.running();
        self.notify_completion = false;
        if requested {
            self.phase = Phase::Stopping;
        }
        requested
    }

    /// A foreground action parks or releases the walk. It never overrides an
    /// explicit Stop.
    pub fn set_waiting(&mut self, waiting: bool) {
        self.phase = match (self.phase, waiting) {
            (Phase::Running, true) => Phase::Waiting,
            (Phase::Waiting, false) => Phase::Running,
            (phase, _) => phase,
        };
    }

    /// Claims the explicit request's one completion notice at its terminal
    /// boundary.
    pub fn finish(&mut self, result: ResultState) -> bool {
        self.phase = Phase::Idle;
        self.last_result = result;
        let notify = self.notify_completion
            && matches!(
                result,
                ResultState::Completed | ResultState::CompletedWithIssues
            );
        self.notify_completion = false;
        notify
    }
}
