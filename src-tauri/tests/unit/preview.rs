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

#[test]
fn a_downscaled_copy_never_outranks_its_original_on_sharpness() {
    // Fine detail a camera recorded; an export halves it.
    let original = DynamicImage::ImageRgb8(image::RgbImage::from_fn(1600, 1200, |x, y| {
        let v = if ((x / 8) + (y / 8)) % 2 == 0 { 220 } else { 30 };
        image::Rgb([v, v, v])
    }));
    let copy = original.resize_exact(640, 480, image::imageops::FilterType::CatmullRom);
    // At their own sizes the copy looks sharper, which ranked it first.
    assert!(laplacian_variance(&copy.to_luma8()) > laplacian_variance(&original.to_luma8()));
    let (original, copy) = (sharpness(&original), sharpness(&copy));
    assert!(copy <= original * 1.1, "copy {copy} outranks original {original}");
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

#[test]
fn decoder_panics_record_one_failure_and_continue_in_serial_and_parallel() {
    for capacity in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let cache = CachePaths::new(dir.path().join("cache"));
        let mut rows = Vec::new();
        for hash in ["bad001", "good01"] {
            let path = dir.path().join(format!("{hash}.jpg")).to_string_lossy().into_owned();
            std::fs::write(&path, b"synthetic").unwrap();
            conn.execute("INSERT INTO contents (hash, byte_size, kind) VALUES (?1, 1, 'image')", [hash]).unwrap();
            conn.execute("INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash) VALUES (?1, ?2, ?3, 'image', ?3)",
                rusqlite::params![path, dir.path().to_string_lossy(), hash]).unwrap();
            rows.push((hash.to_string(), path));
        }
        let stats = derive_candidate_rows_with(&conn, &cache, None, None, None, rows, capacity, &|| false,
            &|hash, _, _| {
                if hash == "bad001" { panic!("synthetic corrupt decoder input"); }
                Ok((None, DerivedFacts { width: 10, height: 10, sharpness: 1.0, phash: 1 }))
            }).unwrap();
        assert_eq!((stats.failed, stats.derived), (1, 1));
        let issues: i64 = conn.query_row("SELECT count(*) FROM active_issues WHERE kind = 'decode-error'", [], |row| row.get(0)).unwrap();
        assert_eq!(issues, 1);
        assert!(crate::derived_state::image_candidates(&conn, false, None, None).unwrap().is_empty(), "ordinary browsing must not retry the panic");
    }
}
