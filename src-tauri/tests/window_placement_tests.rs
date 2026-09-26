use onecopy_lib::window_placement::{
    frame_fills_work_area, placement_after_close, restorable_overlap, ClosingState,
    NormalRectangle, Placement,
};

fn rectangle(x: i32, y: i32, width: u32, height: u32) -> NormalRectangle {
    NormalRectangle {
        x,
        y,
        width,
        height,
    }
}

#[test]
fn normal_close_replaces_the_complete_rectangle() {
    let closing = rectangle(-900, 40, 900, 1000);
    assert_eq!(
        placement_after_close(
            Some(Placement {
                normal: rectangle(10, 20, 800, 600),
                maximized: true
            }),
            ClosingState::Normal(closing),
            true,
        ),
        Some(Placement {
            normal: closing,
            maximized: false
        })
    );
}

#[test]
fn maximized_close_retains_normal_bounds_and_selects_platform_mode() {
    let previous = Placement {
        normal: rectangle(120, 80, 720, 640),
        maximized: false,
    };
    assert_eq!(
        placement_after_close(Some(previous), ClosingState::Maximized, true),
        Some(Placement {
            normal: previous.normal,
            maximized: true
        })
    );
    assert_eq!(
        placement_after_close(Some(previous), ClosingState::Maximized, false),
        Some(previous)
    );
}

#[test]
fn transient_close_retains_the_whole_record() {
    let previous = Placement {
        normal: rectangle(120, 80, 720, 640),
        maximized: true,
    };
    assert_eq!(
        placement_after_close(Some(previous), ClosingState::Transient, true),
        Some(previous)
    );
}

#[test]
fn only_a_work_area_sized_frame_is_mac_maximized() {
    let work_area = rectangle(0, 30, 2560, 1410);
    assert!(frame_fills_work_area(work_area, work_area));
    assert!(frame_fills_work_area(
        rectangle(-1, 31, 2559, 1409),
        work_area,
    ));
    assert!(!frame_fills_work_area(
        rectangle(0, 30, 1280, 1410),
        work_area,
    ));
}

/// R5.1 D12: a saved rectangle that only grazes the work area by a pixel or
/// two is not "restored safely" -- there is nothing to see or drag back.
#[test]
fn a_sliver_of_overlap_is_not_restorable() {
    let work_area = rectangle(0, 0, 1920, 1080);
    // Almost entirely off the left edge: 1px of width overlaps.
    assert!(!restorable_overlap(rectangle(-1919, 0, 1920, 1080), work_area));
    // Almost entirely off the top edge: 1px of height overlaps.
    assert!(!restorable_overlap(rectangle(0, -1079, 1920, 1080), work_area));
}

#[test]
fn a_substantial_corner_overlap_is_restorable() {
    let work_area = rectangle(0, 0, 1920, 1080);
    // Mostly off-screen to the upper-left, but a full-size grabbable corner
    // (well past the minimum overlap in both dimensions) remains visible.
    assert!(restorable_overlap(rectangle(-1800, -1000, 1920, 1080), work_area));
}

#[test]
fn a_zero_sized_rectangle_is_never_restorable() {
    let work_area = rectangle(0, 0, 1920, 1080);
    assert!(!restorable_overlap(rectangle(0, 0, 0, 1080), work_area));
    assert!(!restorable_overlap(rectangle(0, 0, 1920, 0), work_area));
}
