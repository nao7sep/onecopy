//! Timestamp resolution — the pure decision layer over the three evidence
//! sources, in the fixed order: in-file metadata → filename tokenizer →
//! filesystem timestamps. Explicit offsets are facts and always win; every
//! naive value resolves through the single configured default timezone (no
//! per-file overrides — a design non-goal). Each source is good-range-checked
//! independently, so an implausible value (a 1970 mtime, a mis-set camera
//! year) rejects that source and the next one is tried; a file failing all
//! three is Undated.
//!
//! Pure: all inputs are values already extracted and stored as evidence, so a
//! settings change (timezone, good range) re-resolves everything from the DB
//! without touching a single file on disk.

use chrono::TimeZone;
use chrono_tz::Tz;

use crate::metadata::MetadataTimestamp;
use crate::timestamps::FilenameTimestamp;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResolvedSource {
    Metadata,
    Filename,
    FileSystem,
}

impl ResolvedSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolvedSource::Metadata => "metadata",
            ResolvedSource::Filename => "filename",
            ResolvedSource::FileSystem => "filesystem",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResolvedTimestamp {
    pub unix_ms: i64,
    pub source: ResolvedSource,
    pub date_only: bool,
}

/// The tunables the resolver reads (a projection of config).
pub struct ResolutionConfig {
    pub default_timezone: Tz,
    pub good_range_start_year: i32,
    /// "Now" is supplied by the caller so resolution stays pure and testable;
    /// the good range's upper bound is now + 1 day.
    pub now_ms: i64,
}

const DAY_MS: i64 = 86_400_000;

impl ResolutionConfig {
    fn good_range(&self) -> (i64, i64) {
        let start = chrono::NaiveDate::from_ymd_opt(self.good_range_start_year, 1, 1)
            .unwrap_or(chrono::NaiveDate::MIN)
            .and_hms_opt(0, 0, 0)
            .map(|ndt| ndt.and_utc().timestamp_millis())
            .unwrap_or(i64::MIN);
        (start, self.now_ms + DAY_MS)
    }

    fn plausible(&self, unix_ms: i64) -> bool {
        let (min, max) = self.good_range();
        (min..=max).contains(&unix_ms)
    }
}

/// Resolves one file's capture time from its evidence. `fs_mtime_ms` /
/// `fs_birthtime_ms` are the stat values; the earliest plausible of the two is
/// the filesystem candidate (creation time survives copies on Windows, mtime
/// is the honest one on macOS — taking the earliest plausible covers both
/// without a platform switch).
pub fn resolve(
    metadata: Option<MetadataTimestamp>,
    filename: Option<FilenameTimestamp>,
    fs_mtime_ms: Option<i64>,
    fs_birthtime_ms: Option<i64>,
    config: &ResolutionConfig,
) -> Option<ResolvedTimestamp> {
    // Source 1: in-file metadata.
    if let Some(ts) = metadata {
        let unix_ms = match ts {
            MetadataTimestamp::Absolute { unix_ms } => Some(unix_ms),
            MetadataTimestamp::Naive {
                year,
                month,
                day,
                hour,
                minute,
                second,
            } => naive_to_utc_ms(config.default_timezone, year, month, day, hour, minute, second),
        };
        if let Some(unix_ms) = unix_ms {
            if config.plausible(unix_ms) {
                return Some(ResolvedTimestamp {
                    unix_ms,
                    source: ResolvedSource::Metadata,
                    date_only: false,
                });
            }
        }
    }

    // Source 2: the filename tokenizer.
    if let Some(ts) = filename {
        let (unix_ms, date_only) = match ts {
            FilenameTimestamp::EpochMillis(ms) => (Some(ms), false),
            FilenameTimestamp::Naive {
                year,
                month,
                day,
                hour,
                minute,
                second,
                date_only,
            } => (
                naive_to_utc_ms(config.default_timezone, year, month, day, hour, minute, second),
                date_only,
            ),
        };
        if let Some(unix_ms) = unix_ms {
            if config.plausible(unix_ms) {
                return Some(ResolvedTimestamp {
                    unix_ms,
                    source: ResolvedSource::Filename,
                    date_only,
                });
            }
        }
    }

    // Source 3: filesystem timestamps — the earliest plausible.
    let fs_candidate = [fs_birthtime_ms, fs_mtime_ms]
        .into_iter()
        .flatten()
        .filter(|ms| config.plausible(*ms))
        .min();
    if let Some(unix_ms) = fs_candidate {
        return Some(ResolvedTimestamp {
            unix_ms,
            source: ResolvedSource::FileSystem,
            date_only: false,
        });
    }

    None // Undated
}

/// Interprets naive wall-clock fields in `tz`. A DST-ambiguous local time takes
/// the earlier instant; a nonexistent one (spring-forward gap) falls back to a
/// UTC interpretation rather than being dropped — a photo timestamp landing in
/// the gap is clock skew, not a reason to lose the source.
fn naive_to_utc_ms(
    tz: Tz,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<i64> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    let time = chrono::NaiveTime::from_hms_opt(hour, minute, second)?;
    let ndt = chrono::NaiveDateTime::new(date, time);
    match tz.from_local_datetime(&ndt).earliest() {
        Some(zoned) => Some(zoned.timestamp_millis()),
        None => Some(ndt.and_utc().timestamp_millis()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ResolutionConfig {
        ResolutionConfig {
            default_timezone: chrono_tz::Asia::Tokyo,
            good_range_start_year: 1995,
            // 2026-08-08T00:00:00Z as a fixed "now".
            now_ms: 1_786_492_800_000,
        }
    }

    fn utc_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
            .unwrap()
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn naive_metadata_resolves_through_the_default_timezone() {
        // 12:00 JST == 03:00 UTC.
        let got = resolve(
            Some(MetadataTimestamp::Naive {
                year: 2016,
                month: 3,
                day: 5,
                hour: 12,
                minute: 0,
                second: 0,
            }),
            None,
            None,
            None,
            &config(),
        )
        .unwrap();
        assert_eq!(got.unix_ms, utc_ms(2016, 3, 5, 3, 0, 0));
        assert_eq!(got.source, ResolvedSource::Metadata);
        assert!(!got.date_only);
    }

    #[test]
    fn absolute_metadata_passes_through_untouched() {
        let instant = utc_ms(2016, 3, 5, 3, 0, 0);
        let got = resolve(
            Some(MetadataTimestamp::Absolute { unix_ms: instant }),
            None,
            None,
            None,
            &config(),
        )
        .unwrap();
        assert_eq!(got.unix_ms, instant);
        assert_eq!(got.source, ResolvedSource::Metadata);
    }

    #[test]
    fn implausible_metadata_falls_through_to_the_filename() {
        // Camera year mis-set to 1980 (before the good range): the filename
        // evidence wins instead.
        let got = resolve(
            Some(MetadataTimestamp::Naive {
                year: 1980,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            }),
            Some(FilenameTimestamp::EpochMillis(utc_ms(2016, 3, 5, 3, 0, 0))),
            None,
            None,
            &config(),
        )
        .unwrap();
        assert_eq!(got.source, ResolvedSource::Filename);
        assert_eq!(got.unix_ms, utc_ms(2016, 3, 5, 3, 0, 0));
    }

    #[test]
    fn date_only_flag_survives_resolution() {
        let got = resolve(
            None,
            Some(FilenameTimestamp::Naive {
                year: 2016,
                month: 3,
                day: 5,
                hour: 0,
                minute: 0,
                second: 0,
                date_only: true,
            }),
            None,
            None,
            &config(),
        )
        .unwrap();
        assert!(got.date_only);
        assert_eq!(got.source, ResolvedSource::Filename);
    }

    #[test]
    fn filesystem_takes_the_earliest_plausible_of_the_two() {
        let birth = utc_ms(2016, 3, 5, 0, 0, 0);
        let modified = utc_ms(2020, 1, 1, 0, 0, 0);
        let got = resolve(None, None, Some(modified), Some(birth), &config()).unwrap();
        assert_eq!(got.unix_ms, birth);
        assert_eq!(got.source, ResolvedSource::FileSystem);
    }

    #[test]
    fn epoch_zero_mtime_is_rejected_as_implausible() {
        // The classic 1970 mtime: rejected; the other timestamp stands.
        let modified = utc_ms(2020, 1, 1, 0, 0, 0);
        let got = resolve(None, None, Some(modified), Some(0), &config()).unwrap();
        assert_eq!(got.unix_ms, modified);
    }

    #[test]
    fn future_timestamps_beyond_one_day_are_rejected() {
        let cfg = config();
        let too_far = cfg.now_ms + 3 * DAY_MS;
        assert!(resolve(None, None, Some(too_far), None, &cfg).is_none());
        // Within the one-day skew allowance is fine.
        let near = cfg.now_ms + DAY_MS / 2;
        assert!(resolve(None, None, Some(near), None, &cfg).is_some());
    }

    #[test]
    fn all_sources_failing_means_undated() {
        assert!(resolve(None, None, None, None, &config()).is_none());
    }
}
