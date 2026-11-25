//! Pixel and region contracts for the geometry used by screenshot OCR.

use image::{Rgb, RgbImage};
use pure_onnx_ocr::{DetPostProcessor, DetPostProcessorConfig, crop_quad};
use tract_onnx::prelude::tract_ndarray::Array2;

#[test]
fn an_interior_crop_preserves_its_pixels() {
    let color = Rgb([18, 86, 154]);
    let image = RgbImage::from_pixel(20, 12, color);
    let quad = [(3.0, 3.0), (15.0, 3.0), (15.0, 9.0), (3.0, 9.0)];
    let cropped = crop_quad(&image, &quad).unwrap();
    assert_eq!(cropped.dimensions(), (12, 6));
    assert!(cropped.pixels().all(|pixel| pixel.0.into_iter().zip(color.0).all(|(actual, expected)| actual.abs_diff(expected) <= 1)));
}

#[test]
fn a_crop_outside_the_image_keeps_the_black_border() {
    let image = RgbImage::from_pixel(12, 8, Rgb([255, 255, 255]));
    let quad = [(20.0, 20.0), (28.0, 20.0), (28.0, 24.0), (20.0, 24.0)];
    let cropped = crop_quad(&image, &quad).unwrap();
    assert_eq!(cropped.dimensions(), (8, 4));
    assert!(cropped.pixels().all(|pixel| *pixel == Rgb([0, 0, 0])));
}

#[test]
fn a_vertical_line_is_rotated_without_reversing_its_content() {
    let image = RgbImage::from_fn(12, 20, |x, y| Rgb([x as u8 * 10, y as u8 * 10, 42]));
    let quad = [(2.0, 2.0), (8.0, 2.0), (8.0, 16.0), (2.0, 16.0)];
    let cropped = crop_quad(&image, &quad).unwrap();
    assert_eq!(cropped.dimensions(), (14, 6));
    let expected = [50, 90, 42];
    assert!(cropped.get_pixel(7, 2).0.into_iter().zip(expected).all(|(actual, expected)| actual.abs_diff(expected) <= 1));
}

#[test]
fn detection_keeps_separate_text_regions_and_their_bounds() {
    let mut scores = Array2::zeros((14, 24));
    for y in 3..9 {
        for x in 2..10 { scores[[y, x]] = 0.9; }
        for x in 14..22 { scores[[y, x]] = 0.8; }
    }
    let processor = DetPostProcessor::new(DetPostProcessorConfig {
        threshold: 0.5,
        min_area: 4.0,
        box_threshold: 0.5,
        max_candidates: 0,
    });
    let contours = processor.process_probability_map(&scores).unwrap();
    assert_eq!(contours.len(), 2);
    let mut bounds: Vec<_> = contours.iter().map(|contour| {
        let min_x = contour.points.iter().map(|p| p.x).min().unwrap();
        let max_x = contour.points.iter().map(|p| p.x).max().unwrap();
        let min_y = contour.points.iter().map(|p| p.y).min().unwrap();
        let max_y = contour.points.iter().map(|p| p.y).max().unwrap();
        (min_x, min_y, max_x, max_y)
    }).collect();
    bounds.sort_unstable();
    assert_eq!(bounds, [(2, 3, 9, 8), (14, 3, 21, 8)]);
}
