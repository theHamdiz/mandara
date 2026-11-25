//! Shrinking pictures for drawing (login-screen stills, plugin covers)
//! allocates no float copy of the whole picture, and the row-at-a-time
//! triangle filter gives `image`'s pixels.

use image::{DynamicImage, RgbImage, imageops::FilterType};

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Bytes per pixel of `image`'s intermediate `Rgba<f32>` copy.
const FLOAT_RGBA_BYTES: usize = 16;

fn gradient(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x + y) % 256) as u8])))
}

#[test]
fn a_small_shrink_matches_image_without_its_float_copy() {
    // Studio's login preview brings a 1080p still to ~1650 × 928.
    let (w, h) = (1650, 928);
    let source = gradient(1920, 1080);
    let control = allocations(|| drop(source.resize_exact(w, h, FilterType::Triangle)));
    let current = allocations(|| drop(mandara_ui::wallpaper::shrink(&source, w, h)));
    let float_copy = 1920 * h as usize * FLOAT_RGBA_BYTES;
    assert!(control.1 >= float_copy, "the two-pass resize stays a live positive control: {control:?}");
    eprintln!("1920x1080 -> {w}x{h}: in place {current:?} vs two-pass {control:?} (calls, bytes)");
    assert!(current.1 + float_copy / 2 < control.1, "in-place shrink {current:?} vs two-pass {control:?}");
    let ours = mandara_ui::wallpaper::shrink(&source, w, h).to_rgb8();
    let theirs = source.resize_exact(w, h, FilterType::Triangle).to_rgb8();
    assert!(ours == theirs, "the triangle filter is pixel-identical");
}

#[test]
fn plugin_covers_decode_at_their_drawn_size_without_a_float_copy() {
    let cover = mandara_ui::plugin_art::cover("weather").expect("embedded cover");
    let (w, h) = (320, 180);
    let bytes = cover.bytes();
    // The former path: decode, two-pass fill, then RGBA.
    let control = allocations(|| drop(image::load_from_memory(bytes).unwrap().resize_to_fill(w, h, FilterType::Triangle).into_rgba8()));
    let source_w = image::load_from_memory(bytes).unwrap().width() as usize;
    let float_copy = source_w * h as usize * FLOAT_RGBA_BYTES;
    assert!(control.1 >= float_copy, "{control:?}");
    let mut thumb = None;
    let current = allocations(|| thumb = mandara_ui::plugin_art::cover_thumb("weather", w, h));
    eprintln!("cover {w}x{h}: in place {current:?} vs two-pass {control:?} (calls, bytes)");
    assert!(current.1 + float_copy / 2 < control.1, "{current:?} vs {control:?}");
    let thumb = thumb.expect("decodes");
    assert_eq!((thumb.size(0).width.0, thumb.size(0).height.0), (w as i32, h as i32));
}
