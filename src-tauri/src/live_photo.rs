//! Live Photo support, MOV side: extracts `com.apple.quicktime.content.identifier`
//! from a QuickTime/MP4 file's movie-level metadata (the `moov → meta → keys +
//! ilst` structure nom-exif parses internally but does not expose). The image
//! side (the matching identifier in the HEIC/JPG Apple MakerNote) needs real
//! Live Photo fixtures to verify against and stays a separate task.
//!
//! Box grammar: `[size: u32 BE][type: 4 ASCII bytes][payload]`; size 1 means a
//! 64-bit size follows, size 0 means to-end. `meta` is a plain container in
//! QuickTime but a FullBox (4 leading version/flags bytes) in ISO MP4 — the
//! walker tolerates both by probing. `keys` lists `[size][namespace][name]`
//! entries defining 1-based key indexes; `ilst` children carry the key index
//! as their box "type", each wrapping a `data` box whose payload (after 8
//! bytes of type/locale) is the value.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const CONTENT_IDENTIFIER_KEY: &str = "com.apple.quicktime.content.identifier";

/// Reasonable ceiling for an in-memory `moov`: movie metadata is small; a
/// larger claim is a malformed file, not a Live Photo.
const MOOV_CAP: u64 = 32 * 1024 * 1024;

/// Reads the content identifier from a video file, if present.
pub fn quicktime_content_identifier(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(crate::winpath::for_fs(path).as_ref()).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mut position = 0u64;

    // Top-level scan for `moov` without reading the (potentially huge) mdat.
    while position + 8 <= file_len {
        file.seek(SeekFrom::Start(position)).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let box_type = &header[4..8];
        let (payload_offset, box_size) = match size32 {
            0 => (8u64, file_len - position),
            1 => {
                let mut large = [0u8; 8];
                file.read_exact(&mut large).ok()?;
                (16u64, u64::from_be_bytes(large))
            }
            n => (8u64, u64::from(n)),
        };
        if box_size < payload_offset {
            return None; // malformed
        }
        if box_type == b"moov" {
            let payload_len = (box_size - payload_offset).min(MOOV_CAP);
            let mut payload = vec![0u8; payload_len as usize];
            file.read_exact(&mut payload).ok()?;
            return moov_content_identifier(&payload);
        }
        position = position.checked_add(box_size)?;
    }
    None
}

/// The pure half: finds the identifier inside a `moov` payload. `meta` sits
/// directly under `moov` in iPhone recordings but under `moov → udta` in
/// ffmpeg-muxed files (the older QuickTime convention) — both are real.
pub fn moov_content_identifier(moov: &[u8]) -> Option<String> {
    let meta = find_box(moov, b"meta")
        .or_else(|| find_box(moov, b"udta").and_then(|udta| find_box(udta, b"meta")))?;
    // QuickTime: children start immediately. ISO MP4: 4 version/flags bytes
    // first. Probe: a valid child starts with a plausible size + ASCII type.
    let meta_children = if looks_like_box(meta) {
        meta
    } else if meta.len() > 4 && looks_like_box(&meta[4..]) {
        &meta[4..]
    } else {
        return None;
    };

    let keys = find_box(meta_children, b"keys")?;
    let identifier_index = key_index(keys, CONTENT_IDENTIFIER_KEY)?;
    let ilst = find_box(meta_children, b"ilst")?;
    ilst_value(ilst, identifier_index)
}

fn looks_like_box(bytes: &[u8]) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let size = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let plausible_size = size >= 8 && size <= bytes.len();
    // 0xA9 is QuickTime's '©' prefix for legacy metadata atoms.
    let ascii_type = bytes[4..8]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b' ' || *b == 0xA9);
    plausible_size && ascii_type
}

/// First direct child box with the given type; returns its payload.
fn find_box<'a>(container: &'a [u8], box_type: &[u8; 4]) -> Option<&'a [u8]> {
    let mut offset = 0usize;
    while offset + 8 <= container.len() {
        let size = u32::from_be_bytes([
            container[offset],
            container[offset + 1],
            container[offset + 2],
            container[offset + 3],
        ]) as usize;
        if size < 8 || offset + size > container.len() {
            return None; // malformed or 64-bit sizes (not used in meta trees)
        }
        if &container[offset + 4..offset + 8] == box_type {
            return Some(&container[offset + 8..offset + size]);
        }
        offset += size;
    }
    None
}

/// The 1-based index of `wanted` in a `keys` payload
/// (`[version/flags u32][count u32]` then `[size][namespace][name]` entries).
fn key_index(keys: &[u8], wanted: &str) -> Option<u32> {
    if keys.len() < 8 {
        return None;
    }
    let count = u32::from_be_bytes([keys[4], keys[5], keys[6], keys[7]]);
    let mut offset = 8usize;
    for index in 1..=count {
        if offset + 8 > keys.len() {
            return None;
        }
        let size = u32::from_be_bytes([
            keys[offset],
            keys[offset + 1],
            keys[offset + 2],
            keys[offset + 3],
        ]) as usize;
        if size < 8 || offset + size > keys.len() {
            return None;
        }
        let name = &keys[offset + 8..offset + size];
        if name == wanted.as_bytes() {
            return Some(index);
        }
        offset += size;
    }
    None
}

/// The string value of the ilst item whose box type equals `index`, read from
/// its inner `data` box (payload after 8 bytes of type/locale).
fn ilst_value(ilst: &[u8], index: u32) -> Option<String> {
    let index_type = index.to_be_bytes();
    let item = find_box(ilst, &index_type)?;
    let data = find_box(item, b"data")?;
    if data.len() < 8 {
        return None;
    }
    let value = &data[8..];
    let text = std::str::from_utf8(value).ok()?.trim_end_matches('\0');
    (!text.is_empty()).then(|| text.to_string())
}
