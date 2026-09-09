//! Causal activity history. The Rust core validates, orders,
//! and persists diagnostic events. Neither this database nor its UI
//! participates in application behavior.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const ACTIVITY_DB_FILE_NAME: &str = "activity.sqlite3";
const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 500;
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
    Anchor,
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
    Destination,
    Settings,
    Transcript,
    ManagedTools,
    Media,
    Watcher,
    Identity,
    Delivery,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActivitySubject {
    InstallTools,
    CheckToolUpdates,
    CopyFiles,
    MoveFiles,
    DeleteFiles,
    EmptyDeletedFiles,
    Previews,
    Snapshots,
    Similarity,
    Faces,
    VideoTranscription,
    AudioTranscription,
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
    pub subject: Option<ActivitySubject>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub event_id: i64,
    pub session_id: String,
    pub sequence: u64,
    pub event_time_utc: String,
    pub monotonic_ms: u64,
    #[serde(flatten)]
    pub draft: ActivityDraft,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    pub debug_enabled: bool,
    pub session_id: Option<String>,
    pub monotonic_now_ms: u64,
    pub events: Vec<ActivityEvent>,
    pub next_cursor: Option<i64>,
}

pub struct ActivityRecorder {
    session_id: String,
    started: Instant,
    state: Mutex<RecorderState>,
}

struct RecorderState {
    next_sequence: u64,
    connection: Connection,
}

impl ActivityRecorder {
    pub fn new(session_id: String, database_path: PathBuf) -> Result<Self, String> {
        let connection = open_database(&database_path)?;
        let next_sequence = connection
            .query_row(
                "SELECT MAX(sequence) FROM activity_events WHERE session_id = ?1",
                [&session_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(|error| error.to_string())?
            .unwrap_or_default() as u64;
        Ok(Self {
            session_id,
            started: Instant::now(),
            state: Mutex::new(RecorderState {
                next_sequence,
                connection,
            }),
        })
    }

    pub fn record_at(
        &self,
        draft: ActivityDraft,
        event_time_utc: String,
        monotonic_ms: u64,
    ) -> Result<ActivityEvent, String> {
        self.record_with_visibility(draft, event_time_utc, monotonic_ms, true)
    }

    fn record_with_visibility(
        &self,
        draft: ActivityDraft,
        event_time_utc: String,
        monotonic_ms: u64,
        user_visible: bool,
    ) -> Result<ActivityEvent, String> {
        validate_draft(&draft)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "activity history is unavailable".to_string())?;
        state.next_sequence = state.next_sequence.wrapping_add(1);
        let sequence = state.next_sequence;
        let draft_json = serde_json::to_string(&draft).map_err(|error| error.to_string())?;
        state.connection.execute(
            "INSERT INTO activity_events (session_id, sequence, event_time_utc, monotonic_ms, operation_id, owner, kind, draft_json, user_visible) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                self.session_id.as_str(),
                sequence as i64,
                event_time_utc.as_str(),
                monotonic_ms as i64,
                draft.operation_id.as_deref(),
                serde_json::to_string(&draft.owner).map_err(|error| error.to_string())?,
                serde_json::to_string(&draft.kind).map_err(|error| error.to_string())?,
                draft_json,
                user_visible,
            ],
        ).map_err(|error| error.to_string())?;
        Ok(ActivityEvent {
            event_id: state.connection.last_insert_rowid(),
            session_id: self.session_id.clone(),
            sequence,
            event_time_utc,
            monotonic_ms,
            draft,
        })
    }

    fn record_now(
        &self,
        draft: ActivityDraft,
        user_visible: bool,
    ) -> Result<ActivityEvent, String> {
        self.record_with_visibility(
            draft,
            crate::logging::now_iso_millis(),
            self.started.elapsed().as_millis() as u64,
            user_visible,
        )
    }

    pub fn page(
        &self,
        before: Option<i64>,
        limit: usize,
    ) -> Result<(Vec<ActivityEvent>, Option<i64>), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "activity history is unavailable".to_string())?;
        read_page(&state.connection, before, limit)
    }

    pub fn operations(
        &self,
        before: Option<i64>,
        after: Option<i64>,
        limit: usize,
    ) -> Result<crate::activity_history::OperationPage, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "activity history is unavailable".to_string())?;
        crate::activity_history::operations(
            &state.connection,
            before,
            after,
            limit,
            self.session_id.clone(),
            self.started.elapsed().as_millis() as u64,
        )
    }

    pub fn events(
        &self,
        operation: i64,
        before: Option<i64>,
        limit: usize,
    ) -> Result<(Vec<ActivityEvent>, Option<i64>), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "activity history is unavailable".to_string())?;
        crate::activity_history::events(&state.connection, operation, before, limit)
    }
}

fn open_database(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    static JOURNAL: crate::sqlite::JournalSetup = crate::sqlite::JournalSetup::new();
    JOURNAL.configure(&connection, std::time::Duration::from_secs(5))?;
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| error.to_string())?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS activity_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            event_time_utc TEXT NOT NULL,
            monotonic_ms INTEGER NOT NULL,
            operation_id TEXT,
            owner TEXT NOT NULL,
            kind TEXT NOT NULL,
            draft_json TEXT NOT NULL,
            UNIQUE(session_id, sequence)
        );
        CREATE INDEX IF NOT EXISTS activity_events_operation ON activity_events(session_id, operation_id, id);
        CREATE INDEX IF NOT EXISTS activity_events_time ON activity_events(id DESC);"
    ).map_err(|error| error.to_string())?;
    crate::activity_history::initialize(&connection)?;
    Ok(connection)
}

fn read_page(
    connection: &Connection,
    before: Option<i64>,
    limit: usize,
) -> Result<(Vec<ActivityEvent>, Option<i64>), String> {
    let limit = limit.clamp(1, MAX_PAGE_SIZE);
    let before = before.unwrap_or(i64::MAX);
    let mut statement = connection.prepare(
        "SELECT id, session_id, sequence, event_time_utc, monotonic_ms, draft_json FROM activity_events WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
    ).map_err(|error| error.to_string())?;
    let mut events = statement
        .query_map(params![before, (limit + 1) as i64], |row| {
            let draft_json: String = row.get(5)?;
            let draft = serde_json::from_str::<ActivityDraft>(&draft_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(ActivityEvent {
                event_id: row.get(0)?,
                session_id: row.get(1)?,
                sequence: row.get::<_, i64>(2)? as u64,
                event_time_utc: row.get(3)?,
                monotonic_ms: row.get::<_, i64>(4)? as u64,
                draft,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let has_more = events.len() > limit;
    events.truncate(limit);
    let next_cursor = if has_more {
        events.last().map(|event| event.event_id)
    } else {
        None
    };
    Ok((events, next_cursor))
}

fn validate_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':'))
}

fn validate_draft(draft: &ActivityDraft) -> Result<(), String> {
    if let Some(hash) = &draft.target_hash {
        if hash.is_empty()
            || hash.len() > 128
            || !hash
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
        {
            return Err("activity targets must be indexed content identities".into());
        }
    }
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

pub fn init(database_path: PathBuf) {
    let Some(session_id) = crate::logging::session_id() else {
        return;
    };
    match ActivityRecorder::new(session_id.to_string(), database_path) {
        Ok(recorder) => {
            if RECORDER.set(recorder).is_err() {
                crate::logging::warn("activity history already initialized", json!({}));
            }
        }
        Err(error) => crate::logging::warn(
            "activity history unavailable",
            json!({ "error": { "message": error } }),
        ),
    }
}

pub fn record(draft: ActivityDraft) -> Result<Option<ActivityEvent>, String> {
    // Rejected late callbacks are diagnostic evidence, not a new outcome for
    // the operation they no longer own.
    let user_visible = matches!(
        draft.owner,
        ActivityOwner::ManagedTools | ActivityOwner::Settings
    ) && draft.kind != ActivityKind::Stale;
    if !crate::logging::debug_enabled() && !user_visible {
        return Ok(None);
    }
    record_visible(draft, user_visible)
}

/// Work owners publish facts here, independently of the debug firehose.
pub fn record_work(draft: ActivityDraft) -> Result<Option<ActivityEvent>, String> {
    record_visible(draft, true)
}

fn record_visible(
    draft: ActivityDraft,
    user_visible: bool,
) -> Result<Option<ActivityEvent>, String> {
    let Some(recorder) = RECORDER.get() else {
        return Ok(None);
    };
    let event = recorder.record_now(draft, user_visible)?;
    crate::logging::debug("activity", json!({ "activity": event }));
    Ok(Some(event))
}

impl ActivityDraft {
    pub fn new(owner: ActivityOwner, kind: ActivityKind) -> Self {
        Self {
            owner,
            kind,
            subject: None,
            operation_id: None,
            cause_id: None,
            generation: None,
            previous: None,
            current: None,
            reason: None,
            lane: None,
            item_count: None,
            queued: None,
            done: None,
            total: None,
            target_hash: None,
        }
    }
}

/// A scoped diagnostic span, not a job or resource claim. Drop cannot imply
/// success: an unwound or otherwise unclosed span reports an unknown outcome.
pub struct WorkTrace {
    draft: ActivityDraft,
    finished: bool,
    progress: std::sync::Arc<Mutex<TraceProgress>>,
}

struct TraceProgress {
    last_recorded: Instant,
    counts: Option<(u64, u64)>,
}

fn report_progress(draft: &ActivityDraft, progress: &Mutex<TraceProgress>, done: u64, total: u64) {
    let Ok(mut progress) = progress.lock() else {
        return;
    };
    progress.counts = Some((done.min(total), total));
    if progress.last_recorded.elapsed() < std::time::Duration::from_secs(1) {
        return;
    }
    progress.last_recorded = Instant::now();
    let mut draft = draft.clone();
    draft.kind = ActivityKind::Progressed;
    draft.done = Some(done.min(total));
    draft.total = Some(total);
    observe_work(draft);
}

impl WorkTrace {
    pub fn begin(
        owner: ActivityOwner,
        subject: Option<ActivitySubject>,
        target: Option<&str>,
    ) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut draft = ActivityDraft::new(owner, ActivityKind::Started);
        draft.operation_id = Some(format!(
            "work:{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        draft.subject = subject;
        draft.target_hash = target.map(str::to_owned);
        draft.current = Some(ActivityState::Running);
        observe_work(draft.clone());
        Self {
            draft,
            finished: false,
            progress: std::sync::Arc::new(Mutex::new(TraceProgress {
                last_recorded: Instant::now() - std::time::Duration::from_secs(1),
                counts: None,
            })),
        }
    }

    pub fn progress(&self, done: u64, total: u64) {
        report_progress(&self.draft, &self.progress, done, total);
    }

    pub fn progress_reporter(&self) -> impl Fn(u64, u64) + Send + 'static {
        let draft = self.draft.clone();
        let progress = self.progress.clone();
        move |done, total| {
            report_progress(&draft, &progress, done, total);
        }
    }

    pub fn finish(&mut self, state: ActivityState, target: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let mut draft = self.draft.clone();
        draft.kind = match state {
            ActivityState::Succeeded => ActivityKind::Completed,
            ActivityState::Failed => ActivityKind::Failed,
            ActivityState::Cancelled => ActivityKind::Cancelled,
            ActivityState::Paused => ActivityKind::Paused,
            _ => ActivityKind::Closed,
        };
        draft.previous = Some(ActivityState::Running);
        draft.current = Some(state);
        if let Ok(progress) = self.progress.lock() {
            if let Some((done, total)) = progress.counts {
                draft.done = Some(done);
                draft.total = Some(total);
            }
        }
        if let Some(target) = target {
            draft.target_hash = Some(target.into());
        }
        observe_work(draft);
    }

    pub fn result<T>(&mut self, result: &Result<T, String>) {
        self.finish(
            match result {
                Ok(_) => ActivityState::Succeeded,
                Err(error) if error.starts_with(crate::scanner::CANCELLED) => {
                    ActivityState::Cancelled
                }
                Err(_) => ActivityState::Failed,
            },
            None,
        );
    }
}

impl Drop for WorkTrace {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut draft = self.draft.clone();
        draft.kind = ActivityKind::Closed;
        draft.current = None;
        observe_work(draft);
    }
}

pub fn observe_work(draft: ActivityDraft) {
    if let Err(error) = record_work(draft) {
        crate::logging::warn(
            "activity recording failed",
            json!({"error": {"message": error}}),
        );
    }
}

pub fn operations(
    before: Option<i64>,
    after: Option<i64>,
    limit: Option<usize>,
) -> Result<crate::activity_history::OperationPage, String> {
    RECORDER
        .get()
        .ok_or("Activity history is unavailable.")?
        .operations(before, after, limit.unwrap_or(DEFAULT_PAGE_SIZE))
}

pub fn events(
    operation: i64,
    before: Option<i64>,
    limit: Option<usize>,
) -> Result<ActivityPage, String> {
    let recorder = RECORDER.get().ok_or("Activity history is unavailable.")?;
    let (events, next_cursor) =
        recorder.events(operation, before, limit.unwrap_or(DEFAULT_PAGE_SIZE))?;
    Ok(ActivityPage {
        debug_enabled: crate::logging::debug_enabled(),
        session_id: Some(recorder.session_id.clone()),
        monotonic_now_ms: recorder.started.elapsed().as_millis() as u64,
        events,
        next_cursor,
    })
}

pub fn record_app_admitted() {
    let _ = record(ActivityDraft {
        kind: ActivityKind::Admitted,
        owner: ActivityOwner::App,
        subject: None,
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
        target_hash: None,
    });
}

pub fn record_shutdown() {
    let _ = record(ActivityDraft {
        kind: ActivityKind::Shutdown,
        owner: ActivityOwner::App,
        subject: None,
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
        target_hash: None,
    });
}

#[cfg(test)]
#[path = "../tests/unit/activity.rs"]
mod tests;
