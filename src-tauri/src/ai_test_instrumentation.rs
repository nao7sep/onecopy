//! Feature-gated benchmark timing. It is compiled only into `app-e2e` builds
//! and writes path-free structured facts to the operator-owned result file.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use serde::Serialize;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    sequence: u64,
    feature: &'static str,
    phase: &'static str,
    wall_ms: f64,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    requested_acceleration: Option<String>,
    effective_acceleration: Option<String>,
    events: Vec<Event>,
}

static SNAPSHOT: LazyLock<Mutex<Snapshot>> = LazyLock::new(|| Mutex::new(Snapshot::default()));
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn publish(snapshot: &Snapshot) {
    let Some(path) = std::env::var_os("ONECOPY_AI_TIMING_FILE").map(std::path::PathBuf::from)
    else {
        return;
    };
    let Ok(mut bytes) = serde_json::to_vec_pretty(snapshot) else {
        return;
    };
    bytes.push(b'\n');
    let partial = path.with_extension(format!("{}.partial", std::process::id()));
    if std::fs::write(&partial, bytes).is_ok() {
        let _ = crate::fs_publish::replace_existing(&partial, &path);
    }
}

pub fn acceleration(
    requested: crate::ai_acceleration::Mode,
    effective: crate::ai_acceleration::Mode,
) {
    let mut snapshot = SNAPSHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    snapshot.requested_acceleration = Some(requested.id().to_string());
    snapshot.effective_acceleration = Some(effective.id().to_string());
    publish(&snapshot);
}

pub struct Span {
    feature: &'static str,
    phase: &'static str,
    started: Instant,
}

impl Span {
    pub fn begin(feature: &'static str, phase: &'static str) -> Self {
        Self {
            feature,
            phase,
            started: Instant::now(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        let event = Event {
            sequence: SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1,
            feature: self.feature,
            phase: self.phase,
            wall_ms: self.started.elapsed().as_secs_f64() * 1_000.0,
        };
        let mut snapshot = SNAPSHOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        snapshot.events.push(event);
        publish(&snapshot);
    }
}
