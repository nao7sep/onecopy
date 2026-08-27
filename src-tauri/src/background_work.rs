//! Read projection for the fixed background-work classes. Durable debt comes
//! from derived-state receipts and ephemeral lifecycle comes from the one
//! coordinator-owned runtime snapshot; this module owns neither source.

use std::path::Path;

use serde::Serialize;

use crate::derived_state::{WorkCapabilities, WorkClass};
use crate::derived_runtime::RuntimeSnapshot;

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

pub fn snapshot(
    data_root: &Path,
    runtime: RuntimeSnapshot,
    capabilities: WorkCapabilities,
) -> Result<BackgroundWorkSnapshot, String> {
    let conn = crate::index_store::open(&data_root.join(crate::storage::INDEX_DB_FILE_NAME))?;
    let debts = crate::derived_state::work_debts(&conn, capabilities, runtime.similarity_dirty)?;
    let mut classes = Vec::with_capacity(WorkClass::ALL.len());
    for class in WorkClass::ALL {
        let debt = debts.get(class);
        let paused = runtime.master_paused || runtime.paused_classes & class.bit() != 0;
        let is_active = runtime.active.map(|value| value.class) == Some(class);
        let active_progress = runtime.active.filter(|value| value.class == class);
        let queued = debt.runnable + debt.blocked;
        let (state, reason) = if is_active && (paused || runtime.preempt_requested) {
            ("stopping", None)
        } else if is_active {
            ("running", None)
        } else if paused {
            ("paused", None)
        } else if debt.disabled {
            ("disabled", debt.reason)
        } else if debt.runnable == 0 && debt.blocked > 0 {
            ("unavailable", debt.reason)
        } else if debt.runnable == 0 && debt.failed > 0 {
            ("failed", Some("Open Issues to retry failed work"))
        } else if debt.runnable > 0 && runtime.busy {
            ("waiting", Some("Waiting for indexing or a file operation"))
        } else if debt.runnable > 0 && class.idle_only() && !runtime.idle {
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
            failed: debt.failed,
            done: active_progress.and_then(|value| value.done),
            total: active_progress.and_then(|value| value.total),
            reason,
        });
    }
    Ok(BackgroundWorkSnapshot {
        master_paused: runtime.master_paused,
        classes,
    })
}
