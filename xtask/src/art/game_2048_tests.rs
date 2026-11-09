use super::*;

/// More than fifteen levels of average RGB error makes dark contours visible.
const MAX_TILE_RGB_ERROR: u32 = 45;
/// Allow isolated palette rounding without accepting a cluster of dark pixels.
const MAX_OUTLIER_FRACTION: f64 = 0.01;

fn icon_overlay() -> (Record, RgbaImage) {
    let record = catalog().unwrap().into_iter().find(|record| record.id == "2048").unwrap();
    let source = std::fs::read(assets().join("plugins/2048/icon.svg")).unwrap();
    let icon = render_svg(&source, ICON_ALONE, ICON_ALONE).unwrap();
    let mut expected = RgbaImage::new(COVER_SIZE.0, COVER_SIZE.1);
    float_layer(&mut expected, &icon, ICON_ALONE_CENTER, record.angle);
    (record, expected)
}

fn assert_clean_tile(expected: &RgbaImage, cover: &RgbaImage) {
    // These areas showed waves inside the first tile, cracks in the gap,
    // and a smudge beside the merged tile. They contain no glyph edges.
    for (name, x, y, width, height) in [
        ("first tile", 1018, 354, 44, 40),
        ("tile gap", 1094, 425, 34, 26),
        ("beside the merged tile", 1270, 470, 25, 70),
    ] {
        let mut outliers = 0;
        for y in y..y + height {
            for x in x..x + width {
                let original = expected.get_pixel(x, y);
                assert_eq!(original[3], u8::MAX, "{name}: the tile body must be opaque");
                let saved = cover.get_pixel(x, y);
                let error: u32 = original.0[..3].iter().zip(&saved.0[..3])
                    .map(|(a, b)| u32::from(a.abs_diff(*b))).sum();
                outliers += usize::from(error > MAX_TILE_RGB_ERROR);
            }
        }
        let fraction = outliers as f64 / f64::from(width * height);
        assert!(fraction <= MAX_OUTLIER_FRACTION, "{name}: {outliers} discolored pixels ({:.1}%)", fraction * 100.0);
    }
}

#[test]
fn cover_2048_keeps_the_tile_interiors_free_of_quantization_smudges() {
    let (_, expected) = icon_overlay();
    let cover = load_rgba(&assets().join("plugins/2048/cover.png")).unwrap();
    assert_clean_tile(&expected, &cover);
}

#[test]
fn quantization_does_not_spread_dark_contours_across_the_2048_tile() {
    let (record, expected) = icon_overlay();
    let mut cover = load_rgba(&assets().join("artwork").join(format!("{}.png", record.palette))).unwrap();
    let motif = render_svg(motif_svg(&record.motif, &record.palette).unwrap().as_bytes(), COVER_SIZE.0, COVER_SIZE.1).unwrap();
    over(&mut cover, &motif, 0, 0);
    over(&mut cover, &expected, 0, 0);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cover.png");
    save_cover_png(&cover, &path).unwrap();
    assert_clean_tile(&expected, &load_rgba(&path).unwrap());
}
