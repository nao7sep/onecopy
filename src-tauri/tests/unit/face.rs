use super::{expression_crop_bounds, Face};

#[test]
fn expression_crop_omits_degenerate_and_outside_boxes() {
    let face = Face { confidence: 1.0, x1: 0.5, y1: 0.5, x2: 0.5, y2: 0.5 };
    assert_eq!(expression_crop_bounds(100, 100, &face), None);
    let outside = Face { x1: 1.1, x2: 1.2, y1: 0.1, y2: 0.3, ..face };
    assert_eq!(expression_crop_bounds(100, 100, &outside), None);
    let usable = Face { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, ..face };
    assert_eq!(expression_crop_bounds(100, 80, &usable), Some((0, 0, 100, 80)));
}
