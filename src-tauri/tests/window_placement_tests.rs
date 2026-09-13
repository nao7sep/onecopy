use onecopy_lib::window_placement::{
    frame_fills_work_area, placement_after_close, ClosingState, NormalRectangle, Placement,
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
