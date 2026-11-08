//! Reuse the complete card's corner coverage without translating it per strip.

use mandara_i18n::t;
use tiny_skia::{FillRule, Mask, Transform};

use super::super::document::{Document, Layout};

/// Include antialiasing immediately beyond the rounded corner.
const EDGE_ROOM: f32 = 2.0;

/// The card's rounded-corner coverage for the rows `top..top + mask.height()`.
struct Band {
    mask: Mask,
    top: u32,
}

/// The card's outline as two pre-rasterized bands (top and bottom
/// corners); every row between them is fully covered.
pub(super) struct Clip([Option<Band>; 2]);

impl Clip {
    /// Rasterizes the corner bands once per export, with the corner radius
    /// clamped to half the card.
    pub(super) fn new(doc: &Document, layout: &Layout) -> Result<Self, String> {
        let (width, height) = (layout.width, layout.height);
        let radius = (doc.radius * doc.scale)
            .min(width as f32 / 2.0)
            .min(height as f32 / 2.0);
        let corner = (radius + EDGE_ROOM).ceil() as u32;
        let top = corner.min(height);
        let bottom = corner.min(height - top);
        let error = || t("The image design could not be prepared").to_string();
        let path = crate::lock::paint::rounded_rect(0.0, 0.0, width as f32, height as f32, radius)
            .ok_or_else(error)?;
        let mut bands = [None, None];
        for (slot, (y, rows)) in bands.iter_mut().zip([(0, top), (height - bottom, bottom)]) {
            if rows == 0 {
                continue;
            }
            let mut mask = Mask::new(width, rows).ok_or_else(error)?;
            mask.fill_path(
                &path,
                FillRule::Winding,
                true,
                Transform::from_translate(0.0, -(y as f32)),
            );
            *slot = Some(Band { mask, top: y });
        }
        Ok(Self(bands))
    }

    /// Fills the reusable strip `mask` (starting at image row `top`): opaque,
    /// with the overlapping band rows copied in. No path is rasterized per
    /// strip.
    pub(super) fn fill(&self, mask: &mut Mask, top: u32) {
        mask.data_mut().fill(u8::MAX);
        let stride = mask.width() as usize;
        for band in self.0.iter().flatten() {
            let first = top.max(band.top);
            let last = (top + mask.height()).min(band.top + band.mask.height());
            if first >= last {
                continue;
            }
            let source = (first - band.top) as usize * stride;
            let target = (first - top) as usize * stride;
            let bytes = (last - first) as usize * stride;
            mask.data_mut()[target..target + bytes]
                .copy_from_slice(&band.mask.data()[source..source + bytes]);
        }
    }
}
