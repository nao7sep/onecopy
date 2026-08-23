//! In-file metadata extraction — resolution source #1. nom-exif covers
//! JPEG/HEIC/PNG stills and MOV/MP4 tracks; kamadak-exif is the fallback for
//! TIFF-based RAW containers (ARW/CR2/DNG/NEF…), read metadata-only with no
//! pixel decoding. Failures are soft: a file without readable metadata simply
//! yields an empty `MediaMetadata`, and the resolution layer moves on to the
//! filename tokenizer; an I/O-level error propagates so the scanner can record
//! an issue.

use std::io::BufReader;
use std::path::Path;

use nom_exif::{EntryValue, ExifTag, TrackInfoTag};

/// A capture timestamp as the file states it. `Naive` awaits the configured
/// default timezone; `Absolute` carried its own offset (OffsetTimeOriginal, or
/// a QuickTime date with offset) and is already a UTC instant. Serialized into
/// the evidence table so settings changes re-resolve without file reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum MetadataTimestamp {
    Naive {
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    },
    Absolute {
        unix_ms: i64,
    },
}

#[derive(Default, Debug)]
pub struct MediaMetadata {
    pub taken: Option<MetadataTimestamp>,
    pub make: Option<String>,
    pub model: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

/// Reads still-image metadata (EXIF). nom-exif first (JPEG/HEIC/PNG and some
/// RAW containers), kamadak-exif as the TIFF-family fallback. A parse failure
/// is an empty result, not an error — many files simply carry no EXIF.
pub fn read_image_metadata(path: &Path) -> MediaMetadata {
    if let Ok(exif) = nom_exif::read_exif(path) {
        return from_nom_exif(&exif);
    }
    read_kamadak(path).unwrap_or_default()
}

/// Reads video track metadata (QuickTime/MP4/…). A parse failure is an empty
/// result.
pub fn read_video_metadata(path: &Path) -> MediaMetadata {
    let Ok(track) = nom_exif::read_track(path) else {
        return MediaMetadata::default();
    };

    let text = |tag: TrackInfoTag| match track.get(tag) {
        Some(EntryValue::Text(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        _ => None,
    };
    let u32_of = |tag: TrackInfoTag| match track.get(tag) {
        Some(EntryValue::U32(v)) => Some(*v),
        Some(EntryValue::U64(v)) => u32::try_from(*v).ok(),
        _ => None,
    };

    MediaMetadata {
        // QuickTime dates carry an offset (Apple's creationdate key when
        // present, else UTC creation_time) — nom-exif hands us a zoned value,
        // so the result is always Absolute.
        taken: match track.get(TrackInfoTag::CreateDate) {
            Some(EntryValue::DateTime(dt)) => Some(MetadataTimestamp::Absolute {
                unix_ms: dt.timestamp_millis(),
            }),
            Some(EntryValue::NaiveDateTime(ndt)) => Some(naive_from_chrono(*ndt)),
            _ => None,
        },
        make: text(TrackInfoTag::Make),
        model: text(TrackInfoTag::Model),
        width: u32_of(TrackInfoTag::Width),
        height: u32_of(TrackInfoTag::Height),
        duration_ms: match track.get(TrackInfoTag::DurationMs) {
            Some(EntryValue::U64(v)) => Some(*v),
            Some(EntryValue::U32(v)) => Some(u64::from(*v)),
            _ => None,
        },
    }
}

fn from_nom_exif(exif: &nom_exif::Exif) -> MediaMetadata {
    let text = |tag: ExifTag| match exif.get(tag) {
        Some(EntryValue::Text(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        _ => None,
    };
    let dimension = |tag: ExifTag| match exif.get(tag) {
        Some(EntryValue::U32(v)) => Some(*v),
        Some(EntryValue::U16(v)) => Some(u32::from(*v)),
        Some(EntryValue::U64(v)) => u32::try_from(*v).ok(),
        _ => None,
    };

    let taken = match exif.get(ExifTag::DateTimeOriginal) {
        // Already zoned (nom-exif merges a known offset, or the value carried
        // one): an absolute instant.
        Some(EntryValue::DateTime(dt)) => Some(MetadataTimestamp::Absolute {
            unix_ms: dt.timestamp_millis(),
        }),
        // Naive wall-clock: combine with OffsetTimeOriginal when present,
        // otherwise leave naive for the default timezone.
        Some(EntryValue::NaiveDateTime(ndt)) => match text(ExifTag::OffsetTimeOriginal)
            .as_deref()
            .and_then(parse_utc_offset_minutes)
        {
            Some(offset_min) => Some(absolute_from_naive(*ndt, offset_min)),
            None => Some(naive_from_chrono(*ndt)),
        },
        _ => None,
    };

    MediaMetadata {
        taken,
        make: text(ExifTag::Make),
        model: text(ExifTag::Model),
        width: dimension(ExifTag::ExifImageWidth),
        height: dimension(ExifTag::ExifImageHeight),
        duration_ms: None,
    }
}

/// kamadak-exif fallback for TIFF-family containers, metadata-only.
fn read_kamadak(path: &Path) -> Option<MediaMetadata> {
    let file = std::fs::File::open(crate::winpath::for_fs(path).as_ref()).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;

    let field_text = |tag: exif::Tag| {
        exif.get_field(tag, exif::In::PRIMARY)
            .map(|f| f.display_value().to_string())
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
    };

    let taken = exif
        .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
        .and_then(|field| match &field.value {
            exif::Value::Ascii(vecs) => vecs.first().cloned(),
            _ => None,
        })
        .and_then(|ascii| exif::DateTime::from_ascii(&ascii).ok())
        .map(|dt| {
            let naive = MetadataTimestamp::Naive {
                year: i32::from(dt.year),
                month: u32::from(dt.month),
                day: u32::from(dt.day),
                hour: u32::from(dt.hour),
                minute: u32::from(dt.minute),
                second: u32::from(dt.second),
            };
            // kamadak surfaces the offset (from OffsetTimeOriginal) on the
            // parsed DateTime when the container recorded one.
            match dt.offset {
                Some(offset_min) => match naive {
                    MetadataTimestamp::Naive {
                        year,
                        month,
                        day,
                        hour,
                        minute,
                        second,
                    } => absolute_from_fields(
                        year,
                        month,
                        day,
                        hour,
                        minute,
                        second,
                        i32::from(offset_min),
                    )
                    .unwrap_or(naive),
                    absolute => absolute,
                },
                None => naive,
            }
        });

    Some(MediaMetadata {
        taken,
        make: field_text(exif::Tag::Make),
        model: field_text(exif::Tag::Model),
        width: None,
        height: None,
        duration_ms: None,
    })
}

fn naive_from_chrono(ndt: chrono::NaiveDateTime) -> MetadataTimestamp {
    use chrono::{Datelike, Timelike};
    MetadataTimestamp::Naive {
        year: ndt.year(),
        month: ndt.month(),
        day: ndt.day(),
        hour: ndt.hour(),
        minute: ndt.minute(),
        second: ndt.second(),
    }
}

// Naive local wall-clock minus its offset-east-of-UTC is the UTC instant.
fn absolute_from_naive(ndt: chrono::NaiveDateTime, offset_minutes: i32) -> MetadataTimestamp {
    MetadataTimestamp::Absolute {
        unix_ms: ndt.and_utc().timestamp_millis() - i64::from(offset_minutes) * 60_000,
    }
}

fn absolute_from_fields(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    offset_minutes: i32,
) -> Option<MetadataTimestamp> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let time = chrono::NaiveTime::from_hms_opt(hour, minute, second)?;
    let ndt = chrono::NaiveDateTime::new(date, time);
    Some(MetadataTimestamp::Absolute {
        unix_ms: ndt.and_utc().timestamp_millis() - i64::from(offset_minutes) * 60_000,
    })
}

/// Parses an EXIF OffsetTimeOriginal string (`+09:00`, `-05:30`, `Z`) to
/// minutes east of UTC.
pub fn parse_utc_offset_minutes(text: &str) -> Option<i32> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("Z") {
        return Some(0);
    }
    let (sign, rest) = match text.split_at_checked(1)? {
        ("+", rest) => (1, rest),
        ("-", rest) => (-1, rest),
        _ => return None,
    };
    let (hours, minutes) = match rest.split_once(':') {
        Some((h, m)) => (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?),
        None if rest.len() == 4 => (
            rest[..2].parse::<i32>().ok()?,
            rest[2..].parse::<i32>().ok()?,
        ),
        None => (rest.parse::<i32>().ok()?, 0),
    };
    if !(0..=14).contains(&hours) || !(0..60).contains(&minutes) {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_parser_handles_common_forms() {
        assert_eq!(parse_utc_offset_minutes("+09:00"), Some(540));
        assert_eq!(parse_utc_offset_minutes("-05:30"), Some(-330));
        assert_eq!(parse_utc_offset_minutes("Z"), Some(0));
        assert_eq!(parse_utc_offset_minutes("+0200"), Some(120));
        assert_eq!(parse_utc_offset_minutes("+02"), Some(120));
        assert_eq!(parse_utc_offset_minutes(""), None);
        assert_eq!(parse_utc_offset_minutes("+25:00"), None);
        assert_eq!(parse_utc_offset_minutes("09:00"), None);
    }

    #[test]
    fn absolute_from_fields_subtracts_the_offset() {
        // 2016-03-05 12:00:00 at +09:00 == 03:00:00 UTC.
        let got = absolute_from_fields(2016, 3, 5, 12, 0, 0, 540).unwrap();
        let expected = chrono::NaiveDate::from_ymd_opt(2016, 3, 5)
            .unwrap()
            .and_hms_opt(3, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(got, MetadataTimestamp::Absolute { unix_ms: expected });
    }

    #[test]
    fn unreadable_files_yield_empty_metadata_not_errors() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-meta-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("not-a-photo.jpg");
        std::fs::write(&path, b"plainly not a jpeg").unwrap();
        let meta = read_image_metadata(&path);
        assert!(meta.taken.is_none());
        assert!(meta.make.is_none());

        let video = dir.path().join("not-a-video.mp4");
        std::fs::write(&video, b"nope").unwrap();
        let meta = read_video_metadata(&video);
        assert!(meta.taken.is_none());
        assert!(meta.duration_ms.is_none());
    }
}
