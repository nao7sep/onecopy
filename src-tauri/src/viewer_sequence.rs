//! One disposable, disk-backed Quick View sequence.
//!
//! A one-item Main selection freezes the whole ordered section, which can be
//! millions of identities. Keeping that list in the webview or a Rust Vec
//! makes ordinary viewing scale with library size. This runtime streams the
//! frozen order into one temporary SQLite table and retains only its token and
//! current ordinal in memory. The file is replaced for each viewer session
//! and removed on close.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use chrono_tz::Tz;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::queries::{
    self, ItemProjectionContext, PositionedSectionIdentity, SectionIdentity, SectionItem,
    SectionSort,
};

const FILE_NAME: &str = "viewer-sequence.sqlite3";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Section,
    Selection,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Move {
    Previous,
    Next,
    First,
    Last,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub token: String,
    pub member: SectionIdentity,
    pub item: SectionItem,
    pub detail: queries::ItemDetail,
    pub index: u64,
    pub length: u64,
    pub section_index: u64,
    pub scope: Scope,
}

struct Sequence {
    token: String,
    path: PathBuf,
    conn: Connection,
    current_ordinal: i64,
    scope: Scope,
}

struct PendingFile {
    path: PathBuf,
    published: bool,
}

impl Drop for PendingFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

static ACTIVE: LazyLock<Mutex<Option<Sequence>>> = LazyLock::new(|| Mutex::new(None));

#[allow(clippy::too_many_arguments)]
pub fn start(
    data_root: &Path,
    index_conn: &Connection,
    kind: crate::queries::SectionKind,
    month: &str,
    display_tz: Tz,
    sort: SectionSort,
    mut selected: Vec<PositionedSectionIdentity>,
    anchor: &SectionIdentity,
    projection: ItemProjectionContext,
) -> Result<Snapshot, String> {
    // Close any previous sequence up front (brief lock), then build the new
    // one WITHOUT holding ACTIVE: streaming a whole section -- or a large
    // explicit selection -- into the temp store can take a while for a large
    // library, and holding the lock across that blocked a concurrent move or
    // close on another blocking-pool thread until it finished (C-L4). The
    // temp file is private until it is swapped in below, so nothing else can
    // observe it half-built.
    {
        let mut active = ACTIVE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        close_locked(&mut active)?;
    }

    // Generated up front and used in the temp file's own name: two `start`
    // calls can now build concurrently (neither holds ACTIVE while
    // streaming), so they must never share one path the way the single prior
    // sequence's fixed `FILE_NAME` did.
    let token = crate::nanoid::generate()?;
    let temp = data_root.join(crate::binaries_manager::TEMP_DIR_NAME);
    std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
    let path = temp.join(format!("{FILE_NAME}.{token}"));
    remove_file_if_present(&path)?;
    let mut pending = PendingFile {
        path: path.clone(),
        published: false,
    };
    let mut conn = Connection::open(&path).map_err(|error| error.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; \
         CREATE TABLE members (\
           ordinal INTEGER PRIMARY KEY,\
           hash TEXT,\
           path_id INTEGER NOT NULL,\
           section_index INTEGER NOT NULL\
         );",
    )
    .map_err(|error| error.to_string())?;

    let scope = if selected.len() == 1 {
        Scope::Section
    } else {
        Scope::Selection
    };
    let anchor_key = identity_key(anchor);
    let transaction = conn.transaction().map_err(|error| error.to_string())?;
    let mut insert = transaction
        .prepare(
            "INSERT INTO members (ordinal, hash, path_id, section_index) VALUES (?1, ?2, ?3, ?4)",
        )
        .map_err(|error| error.to_string())?;
    let mut anchor_ordinal = None;
    match scope {
        Scope::Section => {
            queries::visit_section_identities(
                index_conn,
                kind,
                month,
                display_tz,
                sort,
                |ordinal, identity| {
                    insert
                        .execute(params![
                            ordinal as i64,
                            identity.hash,
                            identity.path_id,
                            ordinal as i64
                        ])
                        .map_err(|error| error.to_string())?;
                    if identity_key(identity) == anchor_key {
                        anchor_ordinal = Some(ordinal as i64);
                    }
                    Ok(())
                },
            )?;
        }
        Scope::Selection => {
            selected.sort_by_key(|member| member.index);
            for (ordinal, member) in selected.into_iter().enumerate() {
                let identity = SectionIdentity {
                    hash: member.hash,
                    path_id: member.path_id,
                };
                insert
                    .execute(params![
                        ordinal as i64,
                        identity.hash,
                        identity.path_id,
                        member.index as i64
                    ])
                    .map_err(|error| error.to_string())?;
                if identity_key(&identity) == anchor_key {
                    anchor_ordinal = Some(ordinal as i64);
                }
            }
        }
    }
    drop(insert);
    transaction.commit().map_err(|error| error.to_string())?;
    let current_ordinal = anchor_ordinal
        .ok_or_else(|| "the selected item is no longer in the viewer sequence".to_string())?;

    // Swap the finished sequence in under the lock. A concurrent `start`
    // racing in during the unlocked build above is defended against by
    // closing again here: whichever one installs last wins the singleton,
    // and the loser's own temp file is still cleaned up by its `PendingFile`
    // guard once that call unwinds (each build now has its own path).
    let mut active = ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    close_locked(&mut active)?;
    *active = Some(Sequence {
        token: token.clone(),
        path,
        conn,
        current_ordinal,
        scope,
    });
    pending.published = true;
    let snapshot = snapshot_locked(
        active.as_mut().expect("viewer sequence was just installed"),
        index_conn,
        projection,
    );
    if snapshot.is_err() {
        close_locked(&mut active)?;
    }
    snapshot
}

pub fn move_current(
    token: &str,
    movement: Move,
    index_conn: &Connection,
    projection: ItemProjectionContext,
) -> Result<Snapshot, String> {
    let mut active = ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sequence = matching_sequence(&mut active, token)?;
    let ordinal = match movement {
        Move::Previous => neighbor_ordinal(&sequence.conn, sequence.current_ordinal, false)?,
        Move::Next => neighbor_ordinal(&sequence.conn, sequence.current_ordinal, true)?,
        Move::First => edge_ordinal(&sequence.conn, false)?,
        Move::Last => edge_ordinal(&sequence.conn, true)?,
    };
    let previous_ordinal = sequence.current_ordinal;
    if let Some(ordinal) = ordinal {
        sequence.current_ordinal = ordinal;
    }
    let snapshot = snapshot_locked(sequence, index_conn, projection);
    if snapshot.is_err() {
        sequence.current_ordinal = previous_ordinal;
    }
    snapshot
}

pub fn reconcile(
    token: &str,
    index_db: &Path,
    index_conn: &Connection,
    projection: ItemProjectionContext,
) -> Result<Option<Snapshot>, String> {
    let mut active = ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sequence = matching_sequence(&mut active, token)?;
    sequence
        .conn
        .execute(
            "ATTACH DATABASE ?1 AS library",
            [index_db.to_string_lossy().as_ref()],
        )
        .map_err(|error| error.to_string())?;
    // A provisional key names its member's own path, so that path carries
    // the identity it was promoted to; follow it before pruning.
    let result = sequence
        .conn
        .execute(
            "UPDATE members SET hash = (\
               SELECT p.content_hash FROM library.paths p WHERE p.id = members.path_id\
             ) \
             WHERE substr(hash, 1, 1) = 'p' AND EXISTS (\
               SELECT 1 FROM library.paths p \
               WHERE p.id = members.path_id AND p.content_hash IS NOT NULL \
                 AND p.content_hash != members.hash\
             )",
            [],
        )
        .and_then(|_| {
            sequence.conn.execute(
                "DELETE FROM members \
                 WHERE (hash IS NOT NULL AND NOT EXISTS (\
                          SELECT 1 FROM library.review_contents l \
                          WHERE l.content_hash = members.hash AND l.live_copy_count > 0\
                        )) \
                    OR (hash IS NULL AND NOT EXISTS (\
                          SELECT 1 FROM library.paths p \
                          WHERE p.id = members.path_id AND p.missing = 0 \
                            AND p.review_visible = 1 AND p.companion_of IS NULL \
                            AND p.content_hash IS NULL\
                        ))",
                [],
            )
        });
    let detach = sequence.conn.execute("DETACH DATABASE library", []);
    result.map_err(|error| error.to_string())?;
    detach.map_err(|error| error.to_string())?;
    if member_at(&sequence.conn, sequence.current_ordinal)?.is_none() {
        let replacement = neighbor_ordinal(&sequence.conn, sequence.current_ordinal, true)?.or(
            neighbor_ordinal(&sequence.conn, sequence.current_ordinal, false)?,
        );
        let Some(replacement) = replacement else {
            return Ok(None);
        };
        sequence.current_ordinal = replacement;
    }
    match snapshot_locked(sequence, index_conn, projection) {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error) if error == "viewer sequence is empty" => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn close(token: Option<&str>) -> Result<(), String> {
    let mut active = ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if token.is_some_and(|expected| {
        active
            .as_ref()
            .is_some_and(|sequence| sequence.token != expected)
    }) {
        return Ok(());
    }
    close_locked(&mut active)
}

fn snapshot_locked(
    sequence: &mut Sequence,
    index_conn: &Connection,
    projection: ItemProjectionContext,
) -> Result<Snapshot, String> {
    loop {
        let Some((member, section_index)) = member_at(&sequence.conn, sequence.current_ordinal)?
        else {
            return Err("viewer sequence is empty".to_string());
        };
        let member = follow_promotion(sequence, index_conn, member)?;
        if let Some(item) = queries::item_by_identity(index_conn, &member, projection)? {
            let index = sequence
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM members WHERE ordinal < ?1",
                    [sequence.current_ordinal],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| error.to_string())?
                .max(0) as u64;
            let length = sequence
                .conn
                .query_row("SELECT COUNT(*) FROM members", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())?
                .max(0) as u64;
            return Ok(Snapshot {
                token: sequence.token.clone(),
                detail: queries::item_detail(
                    index_conn,
                    member.hash.as_deref(),
                    Some(member.path_id),
                )?,
                member,
                item,
                index,
                length,
                section_index,
                scope: sequence.scope,
            });
        }
        sequence
            .conn
            .execute(
                "DELETE FROM members WHERE ordinal = ?1",
                [sequence.current_ordinal],
            )
            .map_err(|error| error.to_string())?;
        sequence.current_ordinal =
            neighbor_ordinal(&sequence.conn, sequence.current_ordinal, true)?
                .or(neighbor_ordinal(
                    &sequence.conn,
                    sequence.current_ordinal,
                    false,
                )?)
                .ok_or_else(|| "viewer sequence is empty".to_string())?;
    }
}

/// A member frozen under a provisional key keeps its item when that key is
/// promoted to the exact content identity: the key names the member's own
/// path, whose row now carries the promoted identity. The frozen row follows
/// it so later moves and reconciles see the live identity.
fn follow_promotion(
    sequence: &Sequence,
    index_conn: &Connection,
    mut member: SectionIdentity,
) -> Result<SectionIdentity, String> {
    let Some(frozen) = member
        .hash
        .clone()
        .filter(|hash| crate::scanner::is_provisional(hash))
    else {
        return Ok(member);
    };
    let live: Option<String> = index_conn
        .query_row(
            "SELECT content_hash FROM paths WHERE id = ?1",
            [member.path_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    if let Some(live) = live.filter(|live| *live != frozen) {
        sequence
            .conn
            .execute(
                "UPDATE members SET hash = ?1 WHERE ordinal = ?2",
                params![live, sequence.current_ordinal],
            )
            .map_err(|error| error.to_string())?;
        member.hash = Some(live);
    }
    Ok(member)
}

fn matching_sequence<'a>(
    active: &'a mut Option<Sequence>,
    token: &str,
) -> Result<&'a mut Sequence, String> {
    active
        .as_mut()
        .filter(|sequence| sequence.token == token)
        .ok_or_else(|| "viewer sequence is no longer active".to_string())
}

fn member_at(conn: &Connection, ordinal: i64) -> Result<Option<(SectionIdentity, u64)>, String> {
    conn.query_row(
        "SELECT hash, path_id, section_index FROM members WHERE ordinal = ?1",
        [ordinal],
        |row| {
            Ok((
                SectionIdentity {
                    hash: row.get(0)?,
                    path_id: row.get(1)?,
                },
                row.get::<_, i64>(2)?.max(0) as u64,
            ))
        },
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn neighbor_ordinal(conn: &Connection, ordinal: i64, next: bool) -> Result<Option<i64>, String> {
    let (operator, direction) = if next { (">", "ASC") } else { ("<", "DESC") };
    conn.query_row(
        &format!(
            "SELECT ordinal FROM members WHERE ordinal {operator} ?1 ORDER BY ordinal {direction} LIMIT 1"
        ),
        [ordinal],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn edge_ordinal(conn: &Connection, last: bool) -> Result<Option<i64>, String> {
    let direction = if last { "DESC" } else { "ASC" };
    conn.query_row(
        &format!("SELECT ordinal FROM members ORDER BY ordinal {direction} LIMIT 1"),
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| error.to_string())
}

fn identity_key(identity: &SectionIdentity) -> String {
    crate::indexed_file::item_key(identity.hash.as_deref(), identity.path_id)
}

fn close_locked(active: &mut Option<Sequence>) -> Result<(), String> {
    let path = active.take().map(|sequence| sequence.path);
    if let Some(path) = path {
        remove_file_if_present(&path)?;
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
