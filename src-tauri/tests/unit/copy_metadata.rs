use super::*;

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn a_kept_time_needs_nothing_more() {
    assert_eq!(representable_fallback(at(1_500_000_000), at(1_500_000_000)), None);
    // FAT rounds to the even second.
    assert_eq!(representable_fallback(at(1_500_000_001), at(1_500_000_000)), None);
}

#[test]
fn a_time_before_1980_becomes_the_earliest_every_fat_volume_holds() {
    // FAT32 on macOS stored 1969 as 2105; exFAT stored 1980.
    for stored in [at(4_263_000_000), at(315_532_800)] {
        assert_eq!(representable_fallback(at(0), stored), Some(at(FAT_EARLIEST_SECS)));
    }
}

#[test]
fn a_time_after_2107_becomes_the_latest_every_fat_volume_holds() {
    assert_eq!(representable_fallback(at(4_400_000_000), at(343_900_000)), Some(at(FAT_LATEST_SECS)));
}

#[test]
fn a_time_inside_the_range_that_was_not_kept_is_left_alone() {
    // Not a range problem; nothing to choose instead.
    assert_eq!(representable_fallback(at(1_500_000_000), at(1_400_000_000)), None);
}
