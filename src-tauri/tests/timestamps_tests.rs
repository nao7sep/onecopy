// Tests exercising the crate's public API from outside shipped source
// (tests-folder conventions, Rust form).

use onecopy_lib::timestamps::*;

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
