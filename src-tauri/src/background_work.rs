//! Runtime control and read projection for the fixed background-work classes.
//! This is not a job store: absent/stale outputs remain the reconstructible
//! queue in the index, while active and paused state exists only for this app
//! process.

use std::path::Path;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkClass {
    Previews,
    Snapshots,
    Similarity,
    Faces,
    Transcripts,
}

impl WorkClass {
    const ALL: [Self; 5] = [
        Self::Previews,
        Self::Snapshots,
        Self::Similarity,
        Self::Faces,
        Self::Transcripts,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Previews => "previews",
            Self::Snapshots => "snapshots",
            Self::Similarity => "similarity",
            Self::Faces => "faces",
            Self::Transcripts => "transcripts",
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }

    fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.id() == id)
    }

    fn idle_only(self) -> bool {
        matches!(self, Self::Snapshots | Self::Faces | Self::Transcripts)
    }
}

#[derive(Clone, Copy)]
struct ActiveWork {
    class: WorkClass,
    manual: bool,
    done: Option<u64>,
    total: Option<u64>,
}

#[derive(Default)]
struct RuntimeState {
    master_paused: bool,
    paused_classes: u8,
    active: Option<ActiveWork>,
    preempt_requested: bool,
}

impl RuntimeState {
    fn paused(&self, class: WorkClass) -> bool {
        self.master_paused || self.paused_classes & class.bit() != 0
    }
}

static RUNTIME: LazyLock<(Mutex<RuntimeState>, Condvar)> =
    LazyLock::new(|| (Mutex::new(RuntimeState::default()), Condvar::new()));

pub(crate) struct ActiveGuard {
    app: AppHandle,
    class: WorkClass,
}

impl ActiveGuard {
    fn begin(app: &AppHandle, class: WorkClass) -> Option<Self> {
        let mut runtime = RUNTIME.0.lock().ok()?;
        if runtime.paused(class) || runtime.active.is_some() {
            return None;
        }
        runtime.active = Some(ActiveWork {
            class,
            manual: false,
            done: None,
            total: None,
        });
        drop(runtime);
        emit_state_changed(app);
        Some(Self {
            app: app.clone(),
            class,
        })
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut runtime) = RUNTIME.0.lock() {
            if runtime.active.map(|active| active.class) == Some(self.class) {
                runtime.active = None;
                runtime.preempt_requested = false;
                RUNTIME.1.notify_all();
            }
        }
        emit_state_changed(&self.app);
    }
}

pub(crate) fn with_active<T>(
    app: &AppHandle,
    class: WorkClass,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    let Some(_active) = ActiveGuard::begin(app, class) else {
        return Ok(None);
    };
    work().map(Some)
}

pub struct ManualWorkGuard {
    _guard: ActiveGuard,
}

pub fn begin_manual(app: &AppHandle, class: &str) -> Result<ManualWorkGuard, String> {
    let class =
        WorkClass::parse(class).ok_or_else(|| format!("unknown background-work class: {class}"))?;
    let mut runtime = RUNTIME
        .0
        .lock()
        .map_err(|_| "background-work state is unavailable".to_string())?;
    if runtime.paused(class) {
        return Err(paused_message(class));
    }
    if runtime.active.map(|active| active.manual).unwrap_or(false) {
        return Err("Another requested media task is already running.".to_string());
    }
    if let Some(active) = runtime.active {
        runtime.preempt_requested = true;
        drop(runtime);
        if active.class == WorkClass::Transcripts {
            crate::transcription::request_cancel();
        }
        emit_state_changed(app);
        runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| "background-work state is unavailable".to_string())?;
        let (next, waited) = RUNTIME
            .1
            .wait_timeout_while(runtime, Duration::from_secs(10), |state| {
                state.active.is_some()
            })
            .map_err(|_| "background-work state is unavailable".to_string())?;
        runtime = next;
        if waited.timed_out() && runtime.active.is_some() {
            return Err("Background work is still stopping. Try again shortly.".to_string());
        }
        if runtime.paused(class) {
            return Err(paused_message(class));
        }
    }
    runtime.active = Some(ActiveWork {
        class,
        manual: true,
        done: None,
        total: None,
    });
    runtime.preempt_requested = false;
    drop(runtime);
    emit_state_changed(app);
    Ok(ManualWorkGuard {
        _guard: ActiveGuard {
            app: app.clone(),
            class,
        },
    })
}

fn paused_message(class: WorkClass) -> String {
    format!(
        "{} work is paused. Resume it from Background work.",
        class.id()
    )
}

pub(crate) fn is_paused(class: WorkClass) -> bool {
    RUNTIME
        .0
        .lock()
        .map(|runtime| runtime.paused(class))
        .unwrap_or(true)
}

/// Shared by every owned ffmpeg process. A pause kills its child within the
/// subprocess poll interval; in-process work stops at the next safe item edge.
pub fn cancelled() -> bool {
    if crate::scanner::cancelled() {
        return true;
    }
    RUNTIME
        .0
        .lock()
        .ok()
        .map(|runtime| {
            runtime.preempt_requested
                || runtime
                    .active
                    .map(|active| runtime.paused(active.class))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

pub fn set_paused(app: &AppHandle, class: Option<&str>, paused: bool) -> Result<(), String> {
    let active = {
        let mut runtime = RUNTIME
            .0
            .lock()
            .map_err(|_| "background-work state is unavailable".to_string())?;
        match class {
            None => runtime.master_paused = paused,
            Some(id) => {
                let class = WorkClass::parse(id)
                    .ok_or_else(|| format!("unknown background-work class: {id}"))?;
                if paused {
                    runtime.paused_classes |= class.bit();
                } else {
                    runtime.paused_classes &= !class.bit();
                }
            }
        }
        runtime.active
    };

    if paused
        && active
            .map(|active| class.is_none() || class == Some(active.class.id()))
            .unwrap_or(false)
        && active.map(|active| active.class) == Some(WorkClass::Transcripts)
    {
        crate::transcription::request_cancel();
    }
    emit_state_changed(app);
    crate::derived_work::wake(false);
    Ok(())
}

pub(crate) fn progress(app: &AppHandle, class: WorkClass, counts: Option<(u64, u64)>) {
    let (done, total) = counts.map_or((None, None), |(done, total)| (Some(done), Some(total)));
    if let Ok(mut runtime) = RUNTIME.0.lock() {
        if let Some(active) = runtime
            .active
            .as_mut()
            .filter(|active| active.class == class)
        {
            active.done = done;
            active.total = total;
        }
    }
    let _ = app.emit(
        "derived://progress",
        json!({ "class": class.id(), "done": done, "total": total }),
    );
    emit_state_changed(app);
}

pub fn report_manual_progress(app: &AppHandle, class: &str, done: u64, total: u64) {
    if let Some(class) = WorkClass::parse(class) {
        progress(app, class, Some((done, total)));
    }
}

pub(crate) fn emit_state_changed(app: &AppHandle) {
    let payload = RUNTIME
        .0
        .lock()
        .map(|runtime| {
            let paused_classes = WorkClass::ALL
                .into_iter()
                .filter(|class| runtime.paused_classes & class.bit() != 0)
                .map(WorkClass::id)
                .collect::<Vec<_>>();
            json!({
                "masterPaused": runtime.master_paused,
                "pausedClasses": paused_classes,
                "active": runtime.active.map(|active| json!({
                    "id": active.class.id(),
                    "done": active.done,
                    "total": active.total,
                    "stopping": runtime.preempt_requested || runtime.paused(active.class),
                })),
            })
        })
        .unwrap_or_else(|_| json!({}));
    let _ = app.emit("derived://state-changed", payload);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundWorkSnapshot {
    master_paused: bool,
    classes: Vec<BackgroundClassSnapshot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundClassSnapshot {
    id: &'static str,
    state: &'static str,
    queued: u64,
    failed: u64,
    done: Option<u64>,
    total: Option<u64>,
    reason: Option<&'static str>,
}

#[derive(Default)]
struct WorkDebt {
    runnable: u64,
    blocked: u64,
    reason: Option<&'static str>,
    disabled: bool,
}

fn count(conn: &Connection, sql: &str) -> Result<u64, String> {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|value| value.max(0) as u64)
        .map_err(|error| error.to_string())
}

fn failed_count(conn: &Connection, class: WorkClass) -> Result<u64, String> {
    let sql = match class {
        WorkClass::Previews => {
            "SELECT COUNT(*) FROM contents c WHERE c.kind IN ('image', 'video') \
             AND c.derived_at_utc = 'failed' AND EXISTS \
             (SELECT 1 FROM paths p WHERE p.content_hash = c.hash AND p.missing = 0)"
        }
        WorkClass::Snapshots => {
            "SELECT COUNT(*) FROM contents c WHERE c.kind = 'video' AND c.strip_frames < 0 \
             AND EXISTS (SELECT 1 FROM paths p WHERE p.content_hash = c.hash AND p.missing = 0)"
        }
        WorkClass::Faces => {
            "SELECT COUNT(*) FROM analysis_receipts r JOIN contents c ON c.hash = r.content_hash \
             WHERE r.face_state = 'failed' AND EXISTS \
             (SELECT 1 FROM paths p WHERE p.content_hash = c.hash AND p.missing = 0)"
        }
        WorkClass::Transcripts => {
            "SELECT COUNT(*) FROM analysis_receipts r JOIN contents c ON c.hash = r.content_hash \
             WHERE r.transcript_state = 'failed' AND EXISTS \
             (SELECT 1 FROM paths p WHERE p.content_hash = c.hash AND p.missing = 0)"
        }
        WorkClass::Similarity => return Ok(0),
    };
    count(conn, sql)
}

fn whisper_available(data_root: &Path) -> bool {
    crate::binaries_manager::spec_of("whisper-large-v3-turbo")
        .map(|spec| {
            crate::binaries_manager::state_of(data_root, spec).status
                != crate::binaries::BinaryStatus::NotInstalled
        })
        .unwrap_or(false)
}

fn work_debt(
    conn: &Connection,
    settings: &crate::derived_work::Settings,
    class: WorkClass,
    similarity_dirty: bool,
) -> Result<WorkDebt, String> {
    let live = "EXISTS (SELECT 1 FROM paths p WHERE p.content_hash = c.hash AND p.missing = 0)";
    let stale = format!(
        "c.derived_version < {} AND c.derived_at_utc NOT IN ('failed', '{}')",
        crate::preview::DERIVE_VERSION,
        crate::preview::NEEDS_FFMPEG,
    );
    match class {
        WorkClass::Previews => {
            let image_pending = if settings.ffmpeg.is_some() {
                format!(
                    "(c.derived_at_utc IS NULL OR c.derived_at_utc = '{}' OR ({stale}))",
                    crate::preview::NEEDS_FFMPEG,
                )
            } else {
                format!("(c.derived_at_utc IS NULL OR ({stale}))")
            };
            let images = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM contents c WHERE c.kind = 'image' \
                     AND {image_pending} AND {live}"
                ),
            )?;
            let videos = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM contents c WHERE c.kind = 'video' \
                     AND (c.derived_at_utc IS NULL OR ({stale})) AND {live}"
                ),
            )?;
            let waiting_images = if settings.ffmpeg.is_none() {
                count(
                    conn,
                    &format!(
                        "SELECT COUNT(*) FROM contents c WHERE c.kind = 'image' \
                         AND c.derived_at_utc = '{}' AND {live}",
                        crate::preview::NEEDS_FFMPEG,
                    ),
                )?
            } else {
                0
            };
            Ok(if settings.ffmpeg.is_some() {
                WorkDebt {
                    runnable: images + videos,
                    ..WorkDebt::default()
                }
            } else {
                WorkDebt {
                    runnable: images,
                    blocked: videos + waiting_images,
                    reason: (videos + waiting_images > 0).then_some("Waiting for ffmpeg"),
                    disabled: false,
                }
            })
        }
        WorkClass::Snapshots => {
            let pending = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM contents c WHERE c.kind = 'video' \
                     AND c.strip_frames IS NULL AND c.duration_ms IS NOT NULL \
                     AND c.derived_at_utc NOT IN ('failed', '{}') AND {live}",
                    crate::preview::NEEDS_FFMPEG,
                ),
            )?;
            Ok(if settings.ffmpeg.is_some() {
                WorkDebt {
                    runnable: pending,
                    ..WorkDebt::default()
                }
            } else {
                WorkDebt {
                    blocked: pending,
                    reason: (pending > 0).then_some("Waiting for ffmpeg"),
                    ..WorkDebt::default()
                }
            })
        }
        WorkClass::Similarity => Ok(WorkDebt {
            runnable: u64::from(similarity_dirty),
            ..WorkDebt::default()
        }),
        WorkClass::Faces => {
            if !settings.face_enabled {
                return Ok(WorkDebt {
                    disabled: true,
                    reason: Some("Turn on face scoring in Settings"),
                    ..WorkDebt::default()
                });
            }
            let pending = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM contents c \
                     LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
                     WHERE c.kind = 'image' AND r.face_state IS NULL \
                       AND c.derived_at_utc IS NOT NULL \
                       AND c.derived_at_utc NOT IN ('failed', '{}') AND {live}",
                    crate::preview::NEEDS_FFMPEG,
                ),
            )?;
            Ok(if settings.face_models.is_some() {
                WorkDebt {
                    runnable: pending,
                    ..WorkDebt::default()
                }
            } else {
                WorkDebt {
                    blocked: pending,
                    reason: (pending > 0).then_some("Waiting for face models"),
                    ..WorkDebt::default()
                }
            })
        }
        WorkClass::Transcripts => {
            let pending = count(
                conn,
                &format!(
                    "SELECT COUNT(*) FROM contents c \
                     LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
                     WHERE c.kind = 'video' AND c.duration_ms IS NOT NULL \
                       AND r.transcript_state IS NULL AND {live}"
                ),
            )?;
            let available = settings.ffmpeg.is_some() && whisper_available(&settings.data_root);
            Ok(if available {
                WorkDebt {
                    runnable: pending,
                    ..WorkDebt::default()
                }
            } else {
                WorkDebt {
                    blocked: pending,
                    reason: (pending > 0).then_some("Waiting for ffmpeg and transcription model"),
                    ..WorkDebt::default()
                }
            })
        }
    }
}

pub fn snapshot(
    data_root: &Path,
    similarity_dirty: bool,
) -> Result<BackgroundWorkSnapshot, String> {
    let config = crate::storage::read_config_for_setup(data_root)?;
    let settings = crate::derived_work::settings_from_config(config.as_ref(), data_root);
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let (master_paused, paused_classes, active, preempt_requested) = RUNTIME
        .0
        .lock()
        .map(|runtime| {
            (
                runtime.master_paused,
                runtime.paused_classes,
                runtime.active,
                runtime.preempt_requested,
            )
        })
        .map_err(|_| "background-work state is unavailable".to_string())?;
    let busy = !crate::derived_work::available();
    let idle = crate::derived_work::is_idle();
    let mut classes = Vec::with_capacity(WorkClass::ALL.len());
    for class in WorkClass::ALL {
        let debt = work_debt(&conn, &settings, class, similarity_dirty)?;
        let failed = failed_count(&conn, class)?;
        let paused = master_paused || paused_classes & class.bit() != 0;
        let is_active = active.map(|value| value.class) == Some(class);
        let active_progress = active.filter(|value| value.class == class);
        let queued = debt.runnable + debt.blocked;
        let (state, reason) = if is_active && (paused || preempt_requested) {
            ("stopping", None)
        } else if is_active {
            ("running", None)
        } else if paused {
            ("paused", None)
        } else if debt.disabled {
            ("disabled", debt.reason)
        } else if debt.runnable == 0 && debt.blocked > 0 {
            ("unavailable", debt.reason)
        } else if debt.runnable == 0 && failed > 0 {
            ("failed", Some("Open Issues to retry failed work"))
        } else if debt.runnable > 0 && busy {
            ("waiting", Some("Waiting for indexing or a file operation"))
        } else if debt.runnable > 0 && class.idle_only() && !idle {
            ("waiting", Some("Waiting until the app is idle"))
        } else if debt.runnable > 0 {
            ("queued", debt.reason)
        } else {
            ("up-to-date", debt.reason)
        };
        classes.push(BackgroundClassSnapshot {
            id: class.id(),
            state,
            queued,
            failed,
            done: active_progress.and_then(|value| value.done),
            total: active_progress.and_then(|value| value.total),
            reason,
        });
    }
    Ok(BackgroundWorkSnapshot {
        master_paused,
        classes,
    })
}
