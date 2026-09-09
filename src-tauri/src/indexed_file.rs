//! Read-only resolution of a logical item to its current deterministic file.
//!
//! Webviews identify indexed content by hash or path id; filesystem paths
//! never cross into commands as authority. Hash resolution follows the same
//! canonical projection as item presentation, so filename, attributes,
//! text, and external delegation all describe one representative copy.

use std::path::PathBuf;

use rusqlite::Connection;

pub fn live_path(
    conn: &Connection,
    hash: Option<&str>,
    path_id: Option<i64>,
) -> Result<PathBuf, String> {
    let path: String = match (hash.filter(|value| !value.is_empty()), path_id) {
        (Some(hash), None) => conn
            .query_row(
                "SELECT p.abs_path FROM logical_contents l \
                 JOIN paths p ON p.id = l.representative_path_id \
                 WHERE l.content_hash = ?1 AND p.missing = 0",
                [hash],
                |row| row.get(0),
            )
            .map_err(|_| "no live copy of this item".to_string())?,
        (None, Some(path_id)) => conn
            .query_row(
                "SELECT abs_path FROM paths WHERE id = ?1 AND missing = 0",
                [path_id],
                |row| row.get(0),
            )
            .map_err(|_| "no live copy of this item".to_string())?,
        _ => return Err("item needs exactly one hash or pathId".to_string()),
    };
    Ok(PathBuf::from(path))
}
