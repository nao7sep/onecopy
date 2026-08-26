//! Live Photo identifiers from both resources: Apple MakerNote tag 17 in the
//! JPEG/HEIC still and `com.apple.quicktime.content.identifier` in the MOV's
//! movie-level metadata. Pairing requires exact identifier equality plus one
//! shared directory; neither filename stems nor extensions carry authority.
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

const APPLE_MAKER_NOTE_HEADER: &[u8] = b"Apple iOS\0";
const CONTENT_IDENTIFIER_TAG: u16 = 0x0011;
const TIFF_ASCII: u16 = 2;

/// Reasonable ceiling for an in-memory `moov`: movie metadata is small; a
/// larger claim is a malformed file, not a Live Photo.
const MOOV_CAP: u64 = 32 * 1024 * 1024;

/// Reads Apple MakerNote tag 17 from a JPEG/HEIC still, if present.
pub fn still_content_identifier(path: &Path) -> Option<String> {
    let exif = nom_exif::read_exif(crate::winpath::for_fs(path).as_ref()).ok()?;
    let maker_note = exif.get(nom_exif::ExifTag::MakerNote)?.as_undefined()?;
    apple_maker_note_content_identifier(maker_note)
}

/// Parses the TIFF-like IFD inside an Apple `Apple iOS\0` MakerNote. Apple
/// starts the IFD at byte 14 and keeps value offsets relative to the start of
/// the MakerNote. Unknown tags and types are ignored; malformed bounds fail
/// closed.
pub fn apple_maker_note_content_identifier(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(APPLE_MAKER_NOTE_HEADER) || bytes.len() < 16 {
        return None;
    }
    let little_endian = match bytes.get(12..14)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let read_u16 = |at: usize| {
        let value: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little_endian {
            u16::from_le_bytes(value)
        } else {
            u16::from_be_bytes(value)
        })
    };
    let read_u32 = |at: usize| {
        let value: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little_endian {
            u32::from_le_bytes(value)
        } else {
            u32::from_be_bytes(value)
        })
    };

    let count = usize::from(read_u16(14)?);
    for index in 0..count {
        let entry = 16usize.checked_add(index.checked_mul(12)?)?;
        let tag = read_u16(entry)?;
        let value_type = read_u16(entry.checked_add(2)?)?;
        let value_len = usize::try_from(read_u32(entry.checked_add(4)?)?).ok()?;
        if tag != CONTENT_IDENTIFIER_TAG || value_type != TIFF_ASCII || value_len == 0 {
            continue;
        }
        let value_field = entry.checked_add(8)?;
        let value = if value_len <= 4 {
            bytes.get(value_field..value_field.checked_add(value_len)?)?
        } else {
            let offset = usize::try_from(read_u32(value_field)?).ok()?;
            bytes.get(offset..offset.checked_add(value_len)?)?
        };
        let value = value.split(|byte| *byte == 0).next()?;
        let text = std::str::from_utf8(value).ok()?.trim();
        return (!text.is_empty()).then(|| text.to_string());
    }
    None
}

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
