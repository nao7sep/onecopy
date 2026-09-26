//! Read-only resolution of a logical item to its current deterministic file.
//!
//! Webviews identify indexed content by hash or path id; filesystem paths
//! never cross into commands as authority. Hash resolution follows the same
//! canonical projection as item presentation, so filename, attributes,
//! text, and external delegation all describe one representative copy.

use std::path::PathBuf;

use rusqlite::Connection;

/// The one spelling of a logical item's key across the boundary: its content
/// hash, or `path-<id>` for an item without one. Webviews match media-use
/// release keys against their players' keys in this spelling.
pub fn item_key(hash: Option<&str>, path_id: i64) -> String {
    hash.map_or_else(|| format!("{PATH_KEY_PREFIX}{path_id}"), str::to_owned)
}

/// The item a key names: `(hash, path id)`, exactly one of them set; `None`
/// when a `path-` key does not carry a number.
pub fn parse_item_key(key: &str) -> Option<(Option<&str>, Option<i64>)> {
    match key.strip_prefix(PATH_KEY_PREFIX) {
        Some(id) => id.parse().ok().map(|id| (None, Some(id))),
        None => Some((Some(key), None)),
    }
}

const PATH_KEY_PREFIX: &str = "path-";

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
