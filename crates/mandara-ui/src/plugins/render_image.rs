//! Reserve a retained image handle before allocating temporary decode buffers.

use std::sync::Arc;

use gpui::RenderImage;

pub(super) struct PreparedImage {
    image: Arc<RenderImage>,
}

impl PreparedImage {
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
