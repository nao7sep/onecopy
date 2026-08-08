//! The filename timestamp tokenizer — resolution source #2, between in-file
//! metadata and filesystem timestamps.
//!
//! One tolerant matcher instead of a pattern list (the design's call): a date is
//! 8 contiguous digits (`yyyymmdd`), 14 contiguous digits (`yyyymmddhhmmss`), or
//! `yyyy·MM·dd` with one consistent separator; a time is an optional 4/6/9-digit
//! run (`hhmm` / `hhmmss` / `hhmmssfff`, Pixel-style milliseconds ignored) or
//! separated pairs, with optional AM/PM; missing seconds mean :00 and a missing
//! time entirely means 00:00 flagged date-only. A standalone 13/10-digit run is
//! a unix epoch (millis/seconds). All fields are calendar-validated (leap years
//! included); the good-range check belongs to the resolution layer, which also
//! applies the configured default timezone to naive results.
//!
//! Covered real-world shapes (unit-tested below): `IMG_20160305_123456`,
//! `20160305_1234`, `PXL_20210704_170937123`, Dropbox `2016-03-05 12.34.56`,
//! Telegram `photo_2016-03-05_12-34-56`, macOS `Screen Shot 2016-03-05 at
//! 12.34.56 PM`, Windows `Screenshot 2016-03-05 123456`, WhatsApp
//! `IMG-20160305-WA0012` (date-only), LINE epoch-millis names.

/// A timestamp recovered from a file name. `Naive` carries local wall-clock
/// fields awaiting the configured default timezone; `EpochMillis` is already an
/// absolute UTC instant. Serialized into the evidence table so settings changes
/// re-resolve without file reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum FilenameTimestamp {
    Naive {
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        date_only: bool,
    },
    EpochMillis(i64),
}

// Years a filename date may plausibly claim. Deliberately wide — the
// user-configurable good range narrows further at the resolution layer.
const MIN_YEAR: i32 = 1900;
const MAX_YEAR: i32 = 2100;

// Epoch plausibility window: 1995-01-01 .. 2100-01-01 as unix seconds.
const EPOCH_MIN_SECS: i64 = 788_918_400;
const EPOCH_MAX_SECS: i64 = 4_102_444_800;

const DATE_SEPARATORS: &[char] = &['-', '.', '_', ' ', '/'];
const TIME_SEPARATORS: &[char] = &['-', '.', '_', ' ', ':'];

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn valid_date(year: i32, month: u32, day: u32) -> bool {
    (MIN_YEAR..=MAX_YEAR).contains(&year)
        && (1..=12).contains(&month)
        && day >= 1
        && day <= days_in_month(year, month)
}

fn valid_time(hour: u32, minute: u32, second: u32) -> bool {
    hour < 24 && minute < 60 && second < 60
}

/// A maximal run of ASCII digits with its byte span in the stem.
struct DigitRun {
    start: usize,
    end: usize, // exclusive
    len: usize,
}

fn digit_runs(s: &str) -> Vec<DigitRun> {
    let bytes = s.as_bytes();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            runs.push(DigitRun {
                start,
                end: i,
                len: i - start,
            });
        } else {
            i += 1;
        }
    }
    runs
}

fn parse_u32(s: &str) -> u32 {
    s.parse().unwrap_or(0)
}

/// Extracts a timestamp from a file name (extension ignored). Priority: a
/// contiguous or separated calendar date (with optional time) wins over an
/// epoch interpretation; the first valid candidate in the name is taken.
pub fn from_filename(file_name: &str) -> Option<FilenameTimestamp> {
    let stem = file_name
        .rsplit_once('.')
        .map(|(stem, _ext)| stem)
        .unwrap_or(file_name);

    // Pass 1: calendar dates anchored on digit runs.
    for run in digit_runs(stem) {
        match run.len {
            14 => {
                // yyyymmddhhmmss in one run.
                let text = &stem[run.start..run.end];
                let (y, mo, d) = (
                    text[0..4].parse::<i32>().unwrap_or(0),
                    parse_u32(&text[4..6]),
                    parse_u32(&text[6..8]),
                );
                let (h, mi, se) = (
                    parse_u32(&text[8..10]),
                    parse_u32(&text[10..12]),
                    parse_u32(&text[12..14]),
                );
                if valid_date(y, mo, d) && valid_time(h, mi, se) {
                    return Some(FilenameTimestamp::Naive {
                        year: y,
                        month: mo,
                        day: d,
                        hour: h,
                        minute: mi,
                        second: se,
                        date_only: false,
                    });
                }
            }
            8 => {
                // yyyymmdd, optionally followed (after one connector) by a time run.
                let text = &stem[run.start..run.end];
                let (y, mo, d) = (
                    text[0..4].parse::<i32>().unwrap_or(0),
                    parse_u32(&text[4..6]),
                    parse_u32(&text[6..8]),
                );
                if valid_date(y, mo, d) {
                    let (time, _) = time_after(stem, run.end);
                    let (h, mi, se, date_only) = time.unwrap_or((0, 0, 0, true));
                    return Some(FilenameTimestamp::Naive {
                        year: y,
                        month: mo,
                        day: d,
                        hour: h,
                        minute: mi,
                        second: se,
                        date_only,
                    });
                }
            }
            4 => {
                // Possible separated date: yyyy·MM·dd with one consistent separator.
                if let Some((date, after)) = separated_date(stem, run.start) {
                    let (y, mo, d) = date;
                    let (time, _) = time_after(stem, after);
                    let (h, mi, se, date_only) = time.unwrap_or((0, 0, 0, true));
                    return Some(FilenameTimestamp::Naive {
                        year: y,
                        month: mo,
                        day: d,
                        hour: h,
                        minute: mi,
                        second: se,
                        date_only,
                    });
                }
            }
            _ => {}
        }
    }

    // Pass 2: epoch runs (13-digit millis, then 10-digit seconds).
    for run in digit_runs(stem) {
        let text = &stem[run.start..run.end];
        if run.len == 13 {
            if let Ok(ms) = text.parse::<i64>() {
                if (EPOCH_MIN_SECS * 1000..EPOCH_MAX_SECS * 1000).contains(&ms) {
                    return Some(FilenameTimestamp::EpochMillis(ms));
                }
            }
        } else if run.len == 10 {
            if let Ok(secs) = text.parse::<i64>() {
                if (EPOCH_MIN_SECS..EPOCH_MAX_SECS).contains(&secs) {
                    return Some(FilenameTimestamp::EpochMillis(secs * 1000));
                }
            }
        }
    }

    None
}

/// Tries `yyyy·MM·dd` starting at a 4-digit year run: one separator character
/// (from the date set), the same separator between month and day, 1–2 digit
/// month and day. Returns the date and the byte offset just past the day.
fn separated_date(s: &str, year_start: usize) -> Option<((i32, u32, u32), usize)> {
    let bytes = s.as_bytes();
    let year: i32 = s.get(year_start..year_start + 4)?.parse().ok()?;

    let sep_pos = year_start + 4;
    let sep = *bytes.get(sep_pos)? as char;
    if !DATE_SEPARATORS.contains(&sep) {
        return None;
    }

    let (month, after_month) = read_number(bytes, sep_pos + 1, 2)?;
    if *bytes.get(after_month)? as char != sep {
        return None;
    }
    let (day, after_day) = read_number(bytes, after_month + 1, 2)?;

    // The day must not run into more digits (e.g. `2016-03-051` is not a date).
    if bytes.get(after_day).is_some_and(|b| b.is_ascii_digit()) {
        return None;
    }

    if valid_date(year, month, day) {
        Some(((year, month, day), after_day))
    } else {
        None
    }
}

/// Reads a 1..=max_digits number at `pos`. Returns (value, end offset).
fn read_number(bytes: &[u8], pos: usize, max_digits: usize) -> Option<(u32, usize)> {
    let mut end = pos;
    while end < bytes.len() && end - pos < max_digits && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == pos {
        return None;
    }
    let text = std::str::from_utf8(&bytes[pos..end]).ok()?;
    Some((text.parse().ok()?, end))
}

/// Looks for a time immediately after a date (starting at byte `from`):
/// one connector (a single symbol, or ` at ` for macOS screenshots), then a
/// 4/6/9-digit run (`hhmm`/`hhmmss`/`hhmmssfff`) or separated `hh·mm(·ss)` with
/// one consistent separator, then an optional AM/PM marker. Returns
/// `(Some((h, m, s, false)), end)` on success, `(None, from)` otherwise.
#[allow(clippy::type_complexity)]
fn time_after(s: &str, from: usize) -> (Option<(u32, u32, u32, bool)>, usize) {
    let bytes = s.as_bytes();

    // Connector: " at " or exactly one non-alphanumeric symbol.
    let time_start = if s[from..].starts_with(" at ") {
        from + 4
    } else {
        match bytes.get(from) {
            Some(b) if !b.is_ascii_alphanumeric() => from + 1,
            _ => return (None, from),
        }
    };

    let run_len = {
        let mut n = 0;
        while bytes
            .get(time_start + n)
            .is_some_and(|b| b.is_ascii_digit())
        {
            n += 1;
        }
        n
    };

    let parsed = match run_len {
        4 | 6 | 9 => {
            // Contiguous hhmm / hhmmss / hhmmssfff (trailing millis ignored).
            let text = &s[time_start..time_start + run_len];
            let h = parse_u32(&text[0..2]);
            let mi = parse_u32(&text[2..4]);
            let se = if run_len >= 6 { parse_u32(&text[4..6]) } else { 0 };
            // End past the whole run (a 9-digit run's trailing millis included),
            // so AM/PM probing starts after the digits.
            Some((h, mi, se, time_start + run_len))
        }
        1 | 2 => separated_time(bytes, time_start),
        _ => None,
    };

    let Some((hour, minute, second, end)) = parsed else {
        return (None, from);
    };

    // Optional AM/PM within the next few characters (e.g. `12.34.56 PM`).
    let (hour, end) = apply_am_pm(s, hour, end);

    if valid_time(hour, minute, second) {
        (Some((hour, minute, second, false)), end)
    } else {
        (None, from)
    }
}

/// `hh·mm(·ss)` with one consistent separator from the time set.
fn separated_time(bytes: &[u8], start: usize) -> Option<(u32, u32, u32, usize)> {
    let (hour, after_hour) = read_number(bytes, start, 2)?;
    let sep = *bytes.get(after_hour)? as char;
    if !TIME_SEPARATORS.contains(&sep) {
        return None;
    }
    let (minute, after_minute) = read_number(bytes, after_hour + 1, 2)?;
    // Optional seconds with the same separator.
    if bytes.get(after_minute).map(|b| *b as char) == Some(sep) {
        if let Some((second, after_second)) = read_number(bytes, after_minute + 1, 2) {
            if !bytes.get(after_second).is_some_and(|b| b.is_ascii_digit()) {
                return Some((hour, minute, second, after_second));
            }
        }
    }
    if bytes.get(after_minute).is_some_and(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((hour, minute, 0, after_minute))
}

/// Applies a trailing AM/PM marker (optionally preceded by one space) to a
/// 12-hour value. 12 AM → 0; PM adds 12 except for 12 PM.
fn apply_am_pm(s: &str, hour: u32, end: usize) -> (u32, usize) {
    let rest = &s[end.min(s.len())..];
    let trimmed = rest.strip_prefix(' ').unwrap_or(rest);
    let offset = rest.len() - trimmed.len();
    let upper: String = trimmed.chars().take(2).collect::<String>().to_ascii_uppercase();
    if upper == "AM" {
        let hour = if hour == 12 { 0 } else { hour };
        (hour, end + offset + 2)
    } else if upper == "PM" {
        let hour = if hour == 12 { 12 } else { hour + 12 };
        (hour, end + offset + 2)
    } else {
        (hour, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        date_only: bool,
    ) -> FilenameTimestamp {
        FilenameTimestamp::Naive {
            year,
            month,
            day,
            hour,
            minute,
            second,
            date_only,
        }
    }

    #[test]
    fn android_camera_img_yyyymmdd_hhmmss() {
        assert_eq!(
            from_filename("IMG_20160305_123456.jpg"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
    }

    #[test]
    fn contiguous_fourteen_digit_datetime() {
        assert_eq!(
            from_filename("20160305123456.jpg"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
    }

    #[test]
    fn date_with_four_digit_time_defaults_seconds_to_zero() {
        assert_eq!(
            from_filename("20160305_1234.jpg"),
            Some(naive(2016, 3, 5, 12, 34, 0, false))
        );
    }

    #[test]
    fn pixel_style_millisecond_time_run() {
        assert_eq!(
            from_filename("PXL_20210704_170937123.jpg"),
            Some(naive(2021, 7, 4, 17, 9, 37, false))
        );
    }

    #[test]
    fn dropbox_camera_upload() {
        assert_eq!(
            from_filename("2016-03-05 12.34.56.jpg"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
    }

    #[test]
    fn telegram_photo_name() {
        assert_eq!(
            from_filename("photo_2016-03-05_12-34-56.jpg"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
    }

    #[test]
    fn macos_screenshot_with_pm() {
        assert_eq!(
            from_filename("Screen Shot 2016-03-05 at 12.34.56 PM.png"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
        assert_eq!(
            from_filename("Screenshot 2024-01-05 at 9.05.07 AM.png"),
            Some(naive(2024, 1, 5, 9, 5, 7, false))
        );
    }

    #[test]
    fn twelve_am_and_twelve_pm_convert_correctly() {
        assert_eq!(
            from_filename("Shot 2016-03-05 at 12.10.00 AM.png"),
            Some(naive(2016, 3, 5, 0, 10, 0, false))
        );
        assert_eq!(
            from_filename("Shot 2016-03-05 at 12.10.00 PM.png"),
            Some(naive(2016, 3, 5, 12, 10, 0, false))
        );
    }

    #[test]
    fn windows_screenshot_space_and_contiguous_time() {
        assert_eq!(
            from_filename("Screenshot 2016-03-05 123456.png"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
    }

    #[test]
    fn whatsapp_date_only_marks_date_only() {
        // After the date's `-` connector comes `WA0012`, which starts with
        // letters, so the time probe finds no digit run and the result is
        // date-only — the WA counter is never mistaken for a time.
        assert_eq!(
            from_filename("IMG-20160305-WA0012.jpg"),
            Some(naive(2016, 3, 5, 0, 0, 0, true))
        );
    }

    #[test]
    fn line_epoch_millis_name() {
        assert_eq!(
            from_filename("1457145296057.jpg"),
            Some(FilenameTimestamp::EpochMillis(1_457_145_296_057))
        );
    }

    #[test]
    fn epoch_seconds_name() {
        assert_eq!(
            from_filename("1457145296.jpg"),
            Some(FilenameTimestamp::EpochMillis(1_457_145_296_000))
        );
    }

    #[test]
    fn calendar_validation_rejects_impossible_dates() {
        assert_eq!(from_filename("IMG_20161305_123456.jpg"), None); // month 13
        assert_eq!(from_filename("IMG_20160230_123456.jpg"), None); // Feb 30
        assert_eq!(from_filename("20230229_1200.jpg"), None); // not a leap year
        assert_eq!(
            from_filename("20240229_1200.jpg"),
            Some(naive(2024, 2, 29, 12, 0, 0, false)) // a real leap day
        );
    }

    #[test]
    fn invalid_time_degrades_to_date_only_for_contiguous_runs() {
        // 8-digit date followed by a nonsense 6-digit "time" (256199): the time
        // probe fails validation, so the date stands alone as date-only.
        assert_eq!(
            from_filename("20160305_256199.jpg"),
            Some(naive(2016, 3, 5, 0, 0, 0, true))
        );
    }

    #[test]
    fn plain_counters_and_model_numbers_do_not_match() {
        assert_eq!(from_filename("DSC_1234.jpg"), None);
        assert_eq!(from_filename("P1050001.jpg"), None);
        assert_eq!(from_filename("IMG_2019.jpg"), None); // a bare year is not a date
        assert_eq!(from_filename("photo.jpg"), None);
        assert_eq!(from_filename("catalogue-scan-42.png"), None);
    }

    #[test]
    fn day_running_into_more_digits_is_not_a_date() {
        assert_eq!(from_filename("2016-03-051.jpg"), None);
    }

    #[test]
    fn epoch_out_of_plausible_range_is_rejected() {
        assert_eq!(from_filename("0000000000.jpg"), None);
        assert_eq!(from_filename("9999999999999.jpg"), None);
    }

    #[test]
    fn date_wins_over_epoch_when_both_present() {
        assert_eq!(
            from_filename("backup-1457145296057-20160305_123456.jpg"),
            Some(naive(2016, 3, 5, 12, 34, 56, false))
        );
    }

    #[test]
    fn single_digit_month_and_day_with_separators() {
        assert_eq!(
            from_filename("2016-3-5 9.05.07.jpg"),
            Some(naive(2016, 3, 5, 9, 5, 7, false))
        );
    }

    #[test]
    fn mixed_date_separators_are_rejected() {
        // `2016-03_05` mixes separators; the day probe fails on the mismatch.
        assert_eq!(from_filename("2016-03_05.jpg"), None);
    }
}
