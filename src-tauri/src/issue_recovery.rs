//! One backend policy for safe actions on current-state Issues.
//!
//! Output owners still implement their own retry/reset semantics and the
//! scanner implements filesystem probes. This module owns the user-visible
//! action contract and routes only those explicitly safe operations; it is
//! deliberately not a queue or a generic job abstraction.

use rusqlite::Connection;
use serde::Serialize;

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
