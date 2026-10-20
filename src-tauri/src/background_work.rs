//! Read projection for the fixed background-work classes. Durable debt comes
//! from derived-state receipts and ephemeral lifecycle comes from the one
//! coordinator-owned runtime snapshot; this module owns neither source.

use std::path::Path;

use serde::Serialize;

use crate::derived_runtime::RuntimeSnapshot;
use crate::derived_state::{WorkCapabilities, WorkClass, RECHECK_SECTION, WAITING_FOR_INDEXING};

/// What a class row says about its work. The frontend overlays its own live
/// states (running, stopping, paused) on these.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClassState {
    Disabled,
    Unavailable,
    Failed,
    Waiting,
    Queued,
    UpToDate,
}

impl ClassState {
    #[cfg(test)]
    pub(crate) const ALL: [Self; 6] =
        [Self::Disabled, Self::Unavailable, Self::Failed, Self::Waiting, Self::Queued, Self::UpToDate];
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundWorkSnapshot {
    worker_running: bool,
    paused_classes: Vec<&'static str>,
    classes: Vec<BackgroundClassSnapshot>,
    active_item: Option<BackgroundActiveItemSnapshot>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundActiveItemSnapshot {
    id: &'static str,
    hash: Option<String>,
    done: Option<u64>,
    total: Option<u64>,
    stopping: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackgroundClassSnapshot {
    id: &'static str,
    state: ClassState,
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
    let debts = crate::derived_state::work_debts(&conn, capabilities)?;
    let mut classes = Vec::with_capacity(WorkClass::ALL.len());
    for class in WorkClass::ALL {
        let debt = debts.get(class);
        let queued = debt.runnable + debt.blocked;
        // Keep availability intact; runtime overlays it without destroying it.
        let (state, reason) = if debt.disabled {
            (ClassState::Disabled, debt.reason)
        } else if debt.unavailable || (debt.runnable == 0 && debt.blocked > 0) {
            (ClassState::Unavailable, debt.reason)
        } else if debt.runnable == 0 && debt.failed > 0 {
            (ClassState::Failed, Some(RECHECK_SECTION))
        } else if debt.runnable > 0 && runtime.busy {
            (ClassState::Waiting, Some(WAITING_FOR_INDEXING))
        } else if debt.runnable > 0 {
            (ClassState::Queued, debt.reason)
        } else {
            (ClassState::UpToDate, debt.reason)
        };
        classes.push(BackgroundClassSnapshot {
            id: class.id(),
            state,
            queued,
            failed: debt.failed,
            done: None,
            total: None,
            reason,
        });
    }
    Ok(BackgroundWorkSnapshot {
        worker_running: runtime.worker_running,
        paused_classes: WorkClass::ALL.into_iter().filter(|class| runtime.paused_classes & class.bit() != 0).map(WorkClass::id).collect(),
        classes,
        active_item: runtime.active.map(|active| BackgroundActiveItemSnapshot {
            id: active.class.id(),
            hash: active.hash,
            done: active.done,
            total: active.total,
            stopping: active.stopping,
        }),
    })
}
