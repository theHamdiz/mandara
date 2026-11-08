//! A reusable background tile with a fixed origin in the complete image.

use mandara_i18n::t;
use tiny_skia::{FillRule, Paint, Path, Pixmap, PixmapMut, Transform};

use super::super::document::{Document, Layout};

/// A fixed grid keeps floating-point gradient evaluation identical for every caller.
const ROWS: u32 = 128;

/// The card's rounded gradient background, rendered one `ROWS`-row tile
/// at a time and kept until a strip needs the next tile.
pub(super) struct Ground {
    pixels: Pixmap,
    paint: Paint<'static>,
    path: Path,
    height: u32,
    cached: Option<u32>,
}

impl Ground {
    /// Allocates the tile (at most `ROWS` rows) and the card's outline.
    pub(super) fn new(doc: &Document, layout: &Layout) -> Result<Self, String> {
        let error = || t("The image design could not be prepared").to_string();
        Ok(Self {
            pixels: Pixmap::new(layout.width, layout.height.min(ROWS)).ok_or_else(error)?,
            paint: super::gradient(doc, layout, 0),
            path: crate::lock::paint::rounded_rect(
                0.0,
                0.0,
                layout.width as f32,
                layout.height as f32,
                doc.radius * doc.scale,
            )
            .ok_or_else(error)?,
            height: layout.height,
            cached: None,
        })
    }

    /// Copies the background for image rows `top..` into `canvas`, rendering
    /// a tile only when the rows move into a new one.
    pub(super) fn paint(&mut self, canvas: &mut PixmapMut<'_>, top: u32) {
        let stride = self.pixels.width() as usize * 4;
        let mut row = 0;
        while row < canvas.height() && top + row < self.height {
            let global = top + row;
            let tile_top = global / ROWS * ROWS;
            if self.cached != Some(tile_top) {
                self.pixels.fill(tiny_skia::Color::TRANSPARENT);
                self.pixels.as_mut().fill_path(
                    &self.path,
                    &self.paint,
                    FillRule::Winding,
                    Transform::from_translate(0.0, -(tile_top as f32)),
                    None,
                );
                self.cached = Some(tile_top);
            }
            let inside = global - tile_top;
            let count = (self.pixels.height() - inside)
                .min(canvas.height() - row)
                .min(self.height - global);
            let source = inside as usize * stride;
            let target = row as usize * stride;
            let bytes = count as usize * stride;
            canvas.data_mut()[target..target + bytes]
                .copy_from_slice(&self.pixels.data()[source..source + bytes]);
            row += count;
        }
    }
}
