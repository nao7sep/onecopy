//! One backend policy for safe actions on current-state Issues.
//!
//! Output owners still implement their own retry/reset semantics and the
//! scanner implements filesystem probes. This module owns the user-visible
//! action contract and routes only those explicitly safe operations; it is
//! deliberately not a queue or a generic job abstraction.

use rusqlite::Connection;
use serde::Serialize;

pub const DERIVED_WORKER_FAILED: &str = "derived-worker-failed";

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IssueRecovery {
    pub action: &'static str,
    pub label: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum RecheckResult {
    Started,
    Busy,
    StillFailing,
    NotRecoverable,
}

pub fn projection(
    conn: &Connection,
    issue_id: i64,
    kind: &str,
    has_path: bool,
    active_recheck_issue: Option<i64>,
) -> Result<Option<IssueRecovery>, String> {
    if kind == DERIVED_WORKER_FAILED {
        return Ok(Some(IssueRecovery {
            action: "retry",
            label: "Restart",
            status: if crate::derived_work::started() {
                "running"
            } else {
                "available"
            },
        }));
    }
    if has_path && crate::scanner::filesystem_issue_recheckable(kind) {
        return Ok(Some(IssueRecovery {
            action: "recheck",
            label: "Recheck",
            status: if active_recheck_issue == Some(issue_id) {
                "running"
            } else {
                "available"
            },
        }));
    }
    crate::derived_state::issue_recovery(conn, issue_id)
}

pub fn issue_has_kind(conn: &Connection, issue_id: i64, kind: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE id = ?1 AND kind = ?2)",
        rusqlite::params![issue_id, kind],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}

pub fn contains_kind(conn: &Connection, kind: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE kind = ?1)",
        [kind],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}
