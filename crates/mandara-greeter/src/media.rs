//! The login screen's view of the shared video pipeline
//! ([`mandara_media`]): its player hands frames to GPUI as `RenderImage`s.
//! Everything GPUI-specific about video lives here, so the pipeline itself
//! stays reusable (the desktop background converts pictures straight into
//! its `wl_shm` buffers instead).

use std::sync::Arc;

use gpui::RenderImage;

pub use mandara_media::player::FRAME_QUEUE;
pub use mandara_media::{ClipInfo, PlayerOptions, list_clips};

/// A player whose frames are GPUI images.
pub type Player = mandara_media::Player<Arc<RenderImage>>;
pub type PlayerEvent = mandara_media::PlayerEvent<Arc<RenderImage>>;
pub type VideoFrame = mandara_media::VideoFrame<Arc<RenderImage>>;

/// Wraps a converted BGRA frame for GPUI (which takes BGRA pixels in an
/// `RgbaImage` buffer).
fn render_image(bgra: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let buffer = image::RgbaImage::from_raw(w, h, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)])))
}

/// Starts playing a clip for the login screen.
pub fn spawn(opts: PlayerOptions) -> Player {
    Player::spawn(opts, render_image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_become_render_images_of_their_size() {
        let img = render_image(vec![0u8; 4 * 3 * 4], 4, 3).expect("frame");
        assert_eq!(img.size(0).width.0, 4);
        assert_eq!(img.size(0).height.0, 3);
        assert!(render_image(vec![0u8; 5], 4, 3).is_none(), "short buffers are skipped");
    }
}
