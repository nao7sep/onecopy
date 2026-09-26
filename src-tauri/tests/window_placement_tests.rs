use onecopy_lib::window_placement::{
    closing_state_for, frame_fills_work_area, placement_after_close, restorable_overlap,
    ClosingState, NormalRectangle, Placement,
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

/// R2-05 / viewing-sessions.md D1: on macOS, `Window::is_fullscreen` alone
/// under-reports the app's own simple fullscreen (Comparison's Main-filling
/// spread, or a fullscreen Preview), so a display-filling frame must also be
/// caught by the app's own presentation registry, not the platform flag only.
#[test]
fn a_display_filling_frame_registered_as_a_presentation_is_transient_even_when_the_platform_reports_no_fullscreen(
) {
    let whole_screen = rectangle(0, 0, 2560, 1440);
    assert_eq!(
        closing_state_for(false, false, true, false, whole_screen),
        ClosingState::Transient,
    );
}

#[test]
fn an_ordinary_frame_with_no_presentation_registered_is_normal() {
    let rect = rectangle(120, 80, 900, 700);
    assert_eq!(
        closing_state_for(false, false, false, false, rect),
        ClosingState::Normal(rect),
    );
}

#[test]
fn minimized_or_platform_fullscreen_remain_transient() {
    let rect = rectangle(120, 80, 900, 700);
    assert_eq!(
        closing_state_for(true, false, false, false, rect),
        ClosingState::Transient,
    );
    assert_eq!(
        closing_state_for(false, true, false, false, rect),
        ClosingState::Transient,
    );
}

#[test]
fn maximized_wins_over_normal_when_no_presentation_is_registered() {
    let rect = rectangle(0, 30, 2560, 1410);
    assert_eq!(
        closing_state_for(false, false, false, true, rect),
        ClosingState::Maximized,
    );
}
