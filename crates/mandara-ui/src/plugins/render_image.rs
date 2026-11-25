//! Reserve a retained image handle before allocating temporary decode buffers.

use std::sync::Arc;

use gpui::RenderImage;

/// A render-image handle allocated before a decode's temporary buffers,
/// so the retained allocation doesn't land above them on the heap.
pub(super) struct PreparedImage {
    image: Arc<RenderImage>,
}

impl PreparedImage {
    /// Allocates the empty handle; call it on the worker before decoding.
    pub(super) fn new() -> Self {
        Self {
            image: Arc::new(RenderImage::new(
                smallvec::SmallVec::<[image::Frame; 1]>::new(),
            )),
        }
    }

    /// The handle stays private until its owned pixel buffer is ready.
    pub(super) fn finish(mut self, mut rgba: image::RgbaImage) -> Arc<RenderImage> {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        *Arc::get_mut(&mut self.image).expect("a prepared image is private until complete") =
            RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]);
        self.image
    }
}

/// The largest side a downloaded picture may have before it is decoded.
const DECODE_MAX_DIM: u32 = 12_000;
/// The most a single decode may allocate.
const DECODE_MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// Decode a downloaded picture with size and allocation limits set before
/// the decoder runs, so a small, highly compressed file can't expand into
/// hundreds of MiB.
pub(super) fn decode_limited(bytes: &[u8]) -> Option<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(DECODE_MAX_DIM);
    limits.max_image_height = Some(DECODE_MAX_DIM);
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    reader.limits(limits);
    reader.decode().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filling_the_reserved_handle_moves_pixels_without_reallocating_either() {
        let prepared = PreparedImage::new();
        let header = Arc::as_ptr(&prepared.image);
        assert_eq!(prepared.image.frame_count(), 0);
        let pixels =
            image::RgbaImage::from_raw(2, 1, vec![12, 34, 56, 78, 90, 123, 234, 0]).unwrap();
        let allocation = pixels.as_ptr();
        let rendered = prepared.finish(pixels);
        assert_eq!(Arc::as_ptr(&rendered), header);
        assert_eq!(rendered.as_bytes(0).unwrap().as_ptr(), allocation);
        assert_eq!(
            rendered.as_bytes(0).unwrap(),
            [56, 34, 12, 78, 234, 123, 90, 0]
        );
        assert_eq!(rendered.frame_count(), 1);
        assert_eq!(rendered.size(0).width.0, 2);
    }
}
