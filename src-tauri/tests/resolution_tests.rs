// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::metadata::MetadataTimestamp;
use onecopy_lib::timestamps::FilenameTimestamp;

// A day in millis (the good-range slack unit; fixed conversion, safe to
// restate here).
const DAY_MS: i64 = 24 * 3600 * 1000;

use onecopy_lib::resolution::*;

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
