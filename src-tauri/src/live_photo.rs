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
    let mut file = std::fs::File::open(path).ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    // Builders for synthetic box trees, so the parser is tested against the
    // documented grammar rather than only itself.
    fn boxed(box_type: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + payload.len());
        out.extend_from_slice(&(8 + payload.len() as u32).to_be_bytes());
        out.extend_from_slice(box_type);
        out.extend_from_slice(payload);
        out
    }

    fn keys_box(names: &[&str]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes()); // version/flags
        payload.extend_from_slice(&(names.len() as u32).to_be_bytes());
        for name in names {
            payload.extend_from_slice(&(8 + name.len() as u32).to_be_bytes());
            payload.extend_from_slice(b"mdta");
            payload.extend_from_slice(name.as_bytes());
        }
        boxed(b"keys", &payload)
    }

    fn ilst_entry(index: u32, value: &str) -> Vec<u8> {
        let mut data_payload = Vec::new();
        data_payload.extend_from_slice(&1u32.to_be_bytes()); // type: UTF-8
        data_payload.extend_from_slice(&0u32.to_be_bytes()); // locale
        data_payload.extend_from_slice(value.as_bytes());
        let data = boxed(b"data", &data_payload);
        let mut out = Vec::new();
        out.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
        out.extend_from_slice(&index.to_be_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn moov_with(keys: &[&str], entries: &[(u32, &str)], iso_fullbox_meta: bool) -> Vec<u8> {
        let mut ilst_payload = Vec::new();
        for (index, value) in entries {
            ilst_payload.extend_from_slice(&ilst_entry(*index, value));
        }
        let ilst = boxed(b"ilst", &ilst_payload);
        let keys = keys_box(keys);
        let mut meta_payload = Vec::new();
        if iso_fullbox_meta {
            meta_payload.extend_from_slice(&0u32.to_be_bytes()); // version/flags
        }
        meta_payload.extend_from_slice(&keys);
        meta_payload.extend_from_slice(&ilst);
        boxed(b"meta", &meta_payload)
    }

    #[test]
    fn extracts_the_identifier_from_a_quicktime_style_meta() {
        let moov = moov_with(
            &["com.apple.quicktime.creationdate", CONTENT_IDENTIFIER_KEY],
            &[(1, "2026-08-08T12:00:00+0900"), (2, "8FDD1AD2-2D2C-4E4C-99E8")],
            false,
        );
        assert_eq!(
            moov_content_identifier(&moov).as_deref(),
            Some("8FDD1AD2-2D2C-4E4C-99E8")
        );
    }

    #[test]
    fn extracts_from_an_iso_fullbox_meta() {
        let moov = moov_with(&[CONTENT_IDENTIFIER_KEY], &[(1, "UUID-1234")], true);
        assert_eq!(moov_content_identifier(&moov).as_deref(), Some("UUID-1234"));
    }

    #[test]
    fn absent_key_or_value_yields_none() {
        let moov = moov_with(&["com.apple.quicktime.creationdate"], &[(1, "x")], false);
        assert_eq!(moov_content_identifier(&moov), None);
        let empty = moov_with(&[CONTENT_IDENTIFIER_KEY], &[], false);
        assert_eq!(moov_content_identifier(&empty), None);
    }

    #[test]
    fn malformed_boxes_never_panic() {
        assert_eq!(moov_content_identifier(&[]), None);
        assert_eq!(moov_content_identifier(&[0, 0, 0, 3, b'x']), None);
        let mut truncated = moov_with(&[CONTENT_IDENTIFIER_KEY], &[(1, "u")], false);
        truncated.truncate(truncated.len() / 2);
        let _ = moov_content_identifier(&truncated); // no panic is the assertion
    }

    // Round-trip against a REAL file: ffmpeg (the managed install) writes the
    // mdta key with -movflags use_metadata_tags; the parser must read it back.
    // Run with `cargo test live_content_identifier -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[serial_test::serial(backup_store)]
    fn live_content_identifier_round_trip() {
        use crate::binaries_manager;
        let dir = tempfile::Builder::new()
            .prefix("onecopy-livephoto-")
            .tempdir()
            .unwrap();
        binaries_manager::install_or_update(dir.path(), |p, d| eprintln!("[{p}] {d}"))
            .expect("ffmpeg install");
        let ffmpeg = binaries_manager::ffmpeg_path(dir.path());

        let clip = dir.path().join("live.mov");
        let status = std::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=duration=2:size=320x240:rate=12")
            .args([
                "-metadata",
                &format!("{CONTENT_IDENTIFIER_KEY}=TEST-UUID-0001"),
                "-movflags",
                "use_metadata_tags",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&clip)
            .status()
            .unwrap();
        assert!(status.success(), "clip synthesis");

        assert_eq!(
            quicktime_content_identifier(&clip).as_deref(),
            Some("TEST-UUID-0001"),
            "the identifier written by ffmpeg must round-trip through the parser"
        );
    }
}
