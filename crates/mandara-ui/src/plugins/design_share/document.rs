//! A complete reading card, independent of its panel's scroll position.

use cosmic_text::Align;
use mandara_i18n::t;
use mandara_theme::{Color, MandaraPalette};

use crate::backdrop::paint::FontNames;
use crate::lock::paint::{Font, TextPainter, paragraph::Paragraph};

/// A clipboard export is deliberately bounded before expensive font shaping.
const MAX_TEXT_BYTES: usize = 1 << 20;
/// Keeps each software strip small, even at an unusual compositor scale.
pub(super) const MAX_WIDTH: u32 = 4096;
/// Extremely long entries are refused intact rather than silently cropped.
const MAX_HEIGHT: u32 = 131_072;

#[derive(Clone, Copy)]
pub(crate) enum Family {
    Ui,
    Scripture,
    Hadith,
}

pub(crate) struct Block {
    pub text: String,
    pub family: Family,
    pub size: f32,
    pub line_height: f32,
    pub weight: u16,
    pub tabular: bool,
    pub align: Option<Align>,
    pub color: Color,
    pub gap: f32,
}

/// The existing card's decorative language and physical layout.
pub(crate) enum Ornament {
    Plain,
    Crown {
        inset: f32,
        alpha: f32,
        inner_alpha: f32,
    },
    Manuscript {
        width_share: f32,
        top: f32,
        dawn_top: f32,
    },
}

pub(crate) struct Document {
    pub width: f32,
    pub scale: f32,
    pub radius: f32,
    /// Logical top, right, bottom and left padding.
    pub padding: [f32; 4],
    pub background: (Color, Color),
    pub angle: f32,
    pub accent: Color,
    pub lattice_alpha: f32,
    pub ornament: Ornament,
    pub palette: MandaraPalette,
    pub fonts: FontNames,
    pub blocks: Vec<Block>,
}

pub(super) struct LaidBlock {
    pub paragraph: Paragraph,
    pub origin: (f32, f32),
    pub color: Color,
}

/// A measured document: its pixel size and each shaped block's position.
pub(super) struct Layout {
    pub width: u32,
    pub height: u32,
    pub blocks: Vec<LaidBlock>,
}

/// The message for any document that can't become one bounded image.
fn oversized() -> String {
    t("This entry is too large to copy as one image").into()
}

impl Document {
    /// Rejects non-finite or negative geometry, widths outside
    /// `1..=MAX_WIDTH` pixels, empty documents and text over `MAX_TEXT_BYTES`
    /// before anything is allocated. Returns the pixel width.
    pub(super) fn validate(&self) -> Result<u32, String> {
        if !self.scale.is_finite()
            || self.scale <= 0.0
            || !self.width.is_finite()
            || self.width <= 0.0
            || self.padding.iter().any(|n| !n.is_finite() || *n < 0.0)
            || !self.radius.is_finite()
            || self.radius < 0.0
        {
            return Err(oversized());
        }
        let width = (self.width * self.scale).ceil();
        if !(1.0..=MAX_WIDTH as f32).contains(&width)
            || self.padding[1] + self.padding[3] >= self.width
        {
            return Err(oversized());
        }
        let bytes = self
            .blocks
            .iter()
            .try_fold(0usize, |bytes, block| bytes.checked_add(block.text.len()))
            .ok_or_else(oversized)?;
        if bytes > MAX_TEXT_BYTES || self.blocks.is_empty() {
            return Err(oversized());
        }
        Ok(width as u32)
    }

    /// Shapes every block once at the export scale and stacks them, checking
    /// `checkpoint` between blocks and the running height against
    /// `MAX_HEIGHT`.
    pub(super) fn layout(
        &self,
        painter: &mut TextPainter,
        mut checkpoint: impl FnMut() -> bool,
    ) -> Result<Layout, String> {
        let width = self.validate()?;
        let k = self.scale;
        let inner = (self.width - self.padding[1] - self.padding[3]) * k;
        let mut y = self.padding[0] * k;
        let mut blocks = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
            if !checkpoint() {
                return Err(t("Image copying cancelled").into());
            }
            let family = match block.family {
                Family::Ui => &self.fonts.ui,
                Family::Scripture => &self.fonts.scripture,
                Family::Hadith => &self.fonts.hadith,
            };
            let family = if family.is_empty() {
                &self.fonts.arabic
            } else {
                family
            };
            let font = Font {
                family,
                size: block.size * k,
                weight: block.weight,
                tabular: block.tabular,
                tracking: 0.0,
            };
            let paragraph = painter
                .paragraph(
                    &block.text,
                    &font,
                    inner,
                    block.line_height * k,
                    block.align,
                )
                .ok_or_else(oversized)?;
            let origin = (self.padding[3] * k, y);
            y += paragraph.height() + block.gap * k;
            if !y.is_finite() || y + self.padding[2] * k > MAX_HEIGHT as f32 {
                return Err(oversized());
            }
            blocks.push(LaidBlock {
                paragraph,
                origin,
                color: block.color,
            });
        }
        let height = (y + self.padding[2] * k).ceil();
        if !(1.0..=MAX_HEIGHT as f32).contains(&height) {
            return Err(oversized());
        }
        Ok(Layout {
            width,
            height: height as u32,
            blocks,
        })
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::plugins::design_share) fn document(text: &str) -> Document {
        let palette = mandara_theme::engine::default_palette(true);
        Document {
            width: 320.0,
            scale: 1.0,
            radius: 16.0,
            padding: [30.0; 4],
            background: (palette.surface_container, palette.surface_container_low),
            angle: 165.0,
            accent: palette.primary,
            lattice_alpha: 0.0,
            ornament: Ornament::Plain,
            fonts: FontNames::new("Inter", "Inter", "Alexandria", "JetBrains Mono"),
            blocks: vec![Block {
                text: text.into(),
                family: Family::Ui,
                size: 18.0,
                line_height: 30.0,
                weight: 400,
                tabular: false,
                align: None,
                color: palette.on_surface,
                gap: 8.0,
            }],
            palette,
        }
    }

    #[test]
    fn long_content_increases_the_complete_height_without_a_viewport_limit() {
        let mut painter = TextPainter::new();
        let short = document("First paragraph\n\nLast paragraph")
            .layout(&mut painter, || true)
            .unwrap();
        let text = "A complete paragraph.\n\n".repeat(80) + "THE FINAL LINE";
        let full = document(&text).layout(&mut painter, || true).unwrap();
        assert!(full.height > short.height && full.height > 4000);
        assert_eq!(full.blocks.len(), 1);
        assert!(full.blocks[0].origin.1 + full.blocks[0].paragraph.height() < full.height as f32);
    }

    #[test]
    fn geometry_and_work_limits_refuse_the_entire_export() {
        let mut doc = document("text");
        for width in [0.0, f32::NAN, MAX_WIDTH as f32 + 1.0] {
            doc.width = width;
            assert!(doc.validate().is_err());
        }
        doc.width = 320.0;
        doc.blocks[0].text = "x".repeat(MAX_TEXT_BYTES + 1);
        assert!(doc.validate().is_err());
        doc.blocks[0].text = "x".into();
        let mut painter = TextPainter::new();
        assert!(doc.layout(&mut painter, || false).is_err());
    }
}
