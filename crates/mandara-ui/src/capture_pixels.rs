//! One frozen screen buffer shared by the snipping and eyedropper overlays.

use std::sync::Arc;

use gpui::RenderImage;
use mandara_services::screencopy::Capture;

/// Bytes in a tightly packed RGBA/BGRA pixel.
const CHANNELS: usize = 4;

/// Move RGBA into GPUI's BGRA layout without copying the frame.
/// Call off the UI thread; the capture retains only output geometry afterward.
pub(crate) fn freeze(capture: &mut Capture) -> Option<Arc<RenderImage>> {
    if capture.width == 0 || capture.height == 0 { return None; }
    let expected = (capture.width as usize).checked_mul(capture.height as usize)?.checked_mul(CHANNELS)?;
    if capture.rgba.len() != expected { return None; }
    let mut pixels = std::mem::take(&mut capture.rgba);
    for pixel in pixels.as_chunks_mut::<CHANNELS>().0 { pixel.swap(0, 2); }
    let buffer = image::RgbaImage::from_raw(capture.width, capture.height, pixels)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

/// Crop an output-local logical rectangle from frozen BGRA into straight RGBA.
/// Only the selected pixels are copied, including at fractional output scales.
pub(crate) fn crop(capture: &Capture, bgra: &[u8], (x, y, w, h): (f32, f32, f32, f32)) -> image::RgbaImage {
    let scale = capture.scale();
    let edge = |v: f32, max: u32| ((v * scale).round().max(0.0) as u32).min(max);
    let (x0, y0) = (edge(x, capture.width), edge(y, capture.height));
    let (x1, y1) = (edge(x + w, capture.width).max(x0), edge(y + h, capture.height).max(y0));
    let (width, height) = ((x1 - x0).max(1), (y1 - y0).max(1));
    let mut result = image::RgbaImage::new(width, height);
    if x0 >= capture.width || y0 >= capture.height { return result; }
    let stride = capture.width as usize * CHANNELS;
    let row = width as usize * CHANNELS;
    for (dy, target) in result.chunks_exact_mut(row).enumerate() {
        let offset = (y0 as usize + dy) * stride + x0 as usize * CHANNELS;
        if let Some(source) = bgra.get(offset..offset + row) {
            for (target, source) in target.as_chunks_mut::<CHANNELS>().0.iter_mut().zip(source.as_chunks::<CHANNELS>().0) {
                *target = [source[2], source[1], source[0], source[3]];
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freeze_reuses_the_frame_and_crop_preserves_colour_scale_and_origin() {
        let mut capture = Capture { output: "left".into(), logical: (-100, 0, 100, 50), width: 150, height: 75,
            rgba: (0..150 * 75).flat_map(|i| [(i % 150) as u8, (i / 150) as u8, 20, 128]).collect() };
        let pointer = capture.rgba.as_ptr();
        let reference = capture.crop_logical(10.0, 10.0, 20.0, 10.0);
        let frozen = freeze(&mut capture).unwrap();
        assert!(capture.rgba.is_empty());
        assert_eq!(frozen.as_bytes(0).unwrap().as_ptr(), pointer);
        assert_eq!(frozen.as_bytes(0).unwrap().len(), 150 * 75 * CHANNELS);
        let selected = crop(&capture, frozen.as_bytes(0).unwrap(), (10.0, 10.0, 20.0, 10.0));
        assert_eq!(selected.dimensions(), (30, 15));
        assert_eq!(selected, reference);
        assert_eq!(crop(&capture, frozen.as_bytes(0).unwrap(), (-10.0, -10.0, 20.0, 20.0)).dimensions(), (15, 15));
        assert_eq!(crop(&capture, frozen.as_bytes(0).unwrap(), (100.0, 50.0, 20.0, 20.0)).get_pixel(0, 0).0, [0; CHANNELS]);
    }

    /// Reject malformed frame lengths without taking ownership of the caller's pixels.
    #[test]
    fn freeze_rejects_incomplete_or_oversized_frames_without_consuming_them() {
        for bytes in [3, 5] {
            let mut capture = Capture { output: String::new(), logical: (0, 0, 1, 1), width: 1, height: 1, rgba: vec![0; bytes] };
            assert!(freeze(&mut capture).is_none());
            assert_eq!(capture.rgba.len(), bytes);
        }
    }
}
