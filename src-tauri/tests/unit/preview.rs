use super::*;


#[test]
fn orientation_transforms_swap_dimensions_where_they_should() {
    let img = DynamicImage::new_rgb8(40, 20);
    assert_eq!(apply_orientation(img.clone(), 1).dimensions_tuple(), (40, 20));
    assert_eq!(apply_orientation(img.clone(), 3).dimensions_tuple(), (40, 20));
    assert_eq!(apply_orientation(img.clone(), 6).dimensions_tuple(), (20, 40));
    assert_eq!(apply_orientation(img, 8).dimensions_tuple(), (20, 40));
}

#[test]
fn sharp_images_score_higher_than_their_blurred_versions() {
    let sharp = image::RgbImage::from_fn(200, 200, |x, _| {
        if (x / 10) % 2 == 0 {
            image::Rgb([255, 255, 255])
        } else {
            image::Rgb([0, 0, 0])
        }
    });
    let sharp_dyn = DynamicImage::ImageRgb8(sharp);
    let blurred = sharp_dyn.blur(4.0);
    let s_sharp = laplacian_variance(&sharp_dyn.to_luma8());
    let s_blur = laplacian_variance(&blurred.to_luma8());
    assert!(
        s_sharp > s_blur * 2.0,
        "sharp {s_sharp} should clearly exceed blurred {s_blur}"
    );
}

// Small helper so the orientation test reads naturally.
trait DimTuple {
    fn dimensions_tuple(&self) -> (u32, u32);
}
impl DimTuple for DynamicImage {
    fn dimensions_tuple(&self) -> (u32, u32) {
        (self.width(), self.height())
    }
}
