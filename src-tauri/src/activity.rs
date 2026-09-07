//! Developer-only causal activity history. The Rust core owns identity,
//! ordering, retention, and logging; the webview may submit typed facts and
//! render snapshots but never owns a second history or scheduling state.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::json;

const CAPACITY: usize = 500;
const MAX_ID_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityKind {
    Admitted,
    Queued,
    Started,
    Progressed,
    Replaced,
    Coalesced,
    Stale,
    Paused,
    Resumed,
    Stopping,
    Cancelled,
    Completed,
    Failed,
    Opened,
    Closed,
    Changed,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityOwner {
    App,
    Section,
    Selection,
    Viewport,
    Priority,
    SourceCheck,
    FileInformation,
    BackgroundWork,
    Mutation,
    Preview,
    QuickView,
    Fullscreen,
    Comparison,
    Settings,
    ManagedTools,
    Media,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityState {
    Idle,
    Queued,
    Running,
    Waiting,
    Stopping,
    Paused,
    Enabled,
    Disabled,
    Open,
    Closed,
    Succeeded,
    Failed,
    Cancelled,
    Stale,
    Coalesced,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityReason {
    User,
    SectionChange,
    ViewportChange,
    SelectionChange,
    PriorityChange,
    Pause,
    Preemption,
    Superseded,
    StaleResponse,
    Shutdown,
    Dependency,
    Completion,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivityLane {
    Image,
    Video,
    Other,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivityDraft {
    pub kind: ActivityKind,
    pub owner: ActivityOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<ActivityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<ActivityState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ActivityReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<ActivityLane>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub session_id: String,
    pub sequence: u64,
    pub event_time_utc: String,
    pub monotonic_ms: u64,
    #[serde(flatten)]
    pub draft: ActivityDraft,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySnapshot {
    pub debug_enabled: bool,
    pub session_id: Option<String>,
    pub monotonic_now_ms: u64,
    pub events: Vec<ActivityEvent>,
}

pub struct ActivityRecorder {
    session_id: String,
    started: Instant,
    capacity: usize,
    state: Mutex<RecorderState>,
}

#[derive(Default)]
struct RecorderState {
    next_sequence: u64,
    events: VecDeque<ActivityEvent>,
}

impl ActivityRecorder {
    pub fn new(session_id: String, capacity: usize) -> Self {
        Self {
            session_id,
            started: Instant::now(),
            capacity,
            state: Mutex::new(RecorderState::default()),
        }
    }

    pub fn record_at(
        &self,
        draft: ActivityDraft,
        event_time_utc: String,
        monotonic_ms: u64,
    ) -> Result<ActivityEvent, String> {
        validate_draft(&draft)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "activity history is unavailable".to_string())?;
        Ok(self.push(&mut state, draft, event_time_utc, monotonic_ms))
    }

    fn record_now(&self, draft: ActivityDraft) -> Result<ActivityEvent, String> {
        validate_draft(&draft)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "activity history is unavailable".to_string())?;
        Ok(self.push(
            &mut state,
            draft,
            crate::logging::now_iso_millis(),
            self.started.elapsed().as_millis() as u64,
        ))
    }

    fn push(
        &self,
        state: &mut RecorderState,
        draft: ActivityDraft,
        event_time_utc: String,
        monotonic_ms: u64,
    ) -> ActivityEvent {
        state.next_sequence = state.next_sequence.wrapping_add(1);
        let event = ActivityEvent {
            session_id: self.session_id.clone(),
            sequence: state.next_sequence,
            event_time_utc,
            monotonic_ms,
            draft,
        };
        if self.capacity > 0 {
            if state.events.len() == self.capacity {
                state.events.pop_front();
            }
            state.events.push_back(event.clone());
        }
        event
    }

    pub fn snapshot(&self) -> Vec<ActivityEvent> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .events
            .iter()
            .cloned()
            .collect()
    }
}

fn validate_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

fn validate_draft(draft: &ActivityDraft) -> Result<(), String> {
    for id in [&draft.operation_id, &draft.cause_id].into_iter().flatten() {
        if !validate_id(id) {
            return Err("activity identifiers must be 1-64 URL-safe characters".to_string());
        }
    }
    if let (Some(done), Some(total)) = (draft.done, draft.total) {
        if done > total {
            return Err("activity progress cannot exceed its total".to_string());
        }
    }
    Ok(())
}

static RECORDER: OnceLock<ActivityRecorder> = OnceLock::new();

pub fn init() {
    if !crate::logging::debug_enabled() {
        return;
    }
    let Some(session_id) = crate::logging::session_id() else {
        return;
    };
    if RECORDER
        .set(ActivityRecorder::new(session_id.to_string(), CAPACITY))
        .is_err()
    {
        crate::logging::warn("activity history already initialized", json!({}));
    }
}

pub fn record(draft: ActivityDraft) -> Result<Option<ActivityEvent>, String> {
    let Some(recorder) = RECORDER.get() else {
        return Ok(None);
    };
    let event = recorder.record_now(draft)?;
    crate::logging::debug("activity", json!({ "activity": event }));
    Ok(Some(event))
}

pub fn snapshot() -> ActivitySnapshot {
    let recorder = RECORDER.get();
    ActivitySnapshot {
        debug_enabled: crate::logging::debug_enabled(),
        session_id: recorder.map(|value| value.session_id.clone()),
        monotonic_now_ms: recorder
            .map(|value| value.started.elapsed().as_millis() as u64)
            .unwrap_or_default(),
        events: recorder.map(ActivityRecorder::snapshot).unwrap_or_default(),
    }
}

pub fn record_app_admitted() {
    let _ = record(ActivityDraft {
        kind: ActivityKind::Admitted,
        owner: ActivityOwner::App,
        operation_id: None,
        cause_id: None,
        generation: None,
        previous: Some(ActivityState::Idle),
        current: Some(ActivityState::Running),
        reason: Some(ActivityReason::Completion),
        lane: None,
        item_count: None,
        queued: None,
        done: None,
        total: None,
    });
}

pub fn record_shutdown() {
    let _ = record(ActivityDraft {
        kind: ActivityKind::Shutdown,
        owner: ActivityOwner::App,
        operation_id: None,
        cause_id: None,
        generation: None,
        previous: Some(ActivityState::Running),
        current: Some(ActivityState::Stopping),
        reason: Some(ActivityReason::Shutdown),
        lane: None,
        item_count: None,
        queued: None,
        done: None,
        total: None,
    });
}
