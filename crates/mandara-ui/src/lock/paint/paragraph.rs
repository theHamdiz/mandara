//! Full paragraph layout for software exports, without ellipsis or a viewport limit.

use cosmic_text::{Align, Buffer, Renderer as _};
use tiny_skia::PixmapMut;

use super::{Font, TextPainter, composite_glyph};
use mandara_theme::Color;

/// An independently owned shaped paragraph; release it with the export job.
pub struct Paragraph {
    buffer: Buffer,
    height: f32,
}

impl Paragraph {
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Draw only intersecting line runs into a bounded strip. Coordinates may
    /// extend above or below the strip; glyph coverage is clipped at its edges.
    pub fn draw(
        &self,
        painter: &mut TextPainter,
        canvas: &mut PixmapMut<'_>,
        origin: (f32, f32),
        color: Color,
    ) {
        let [r, g, b, a] = color.to_rgba8();
        let color = cosmic_text::Color::rgba(r, g, b, a);
        let dimensions = (canvas.width() as i32, canvas.height() as i32);
        let translation = (origin.0.floor() as i32, origin.1.floor() as i32);
        let fractional = (
            origin.0 - translation.0 as f32,
            origin.1 - translation.1 as f32,
        );
        let bytes = canvas.data_mut();
        let mut renderer = cosmic_text::LegacyRenderer {
            font_system: &mut painter.fonts,
            cache: &mut painter.glyphs,
            callback: |x, y, w, h, color| {
                composite_glyph(bytes, dimensions, (x, y, w, h), color, 1.0)
            },
        };
        for run in self.buffer.layout_runs() {
            // One line of extra room includes accents and joining-script marks
            // that extend outside their nominal line box.
            if origin.1 + run.line_top > dimensions.1 as f32 + run.line_height
                || origin.1 + run.line_top + run.line_height * 2.0 < 0.0
            {
                continue;
            }
            for glyph in run.glyphs {
                // Hint before translating into a strip: truncating a negative
                // strip-local baseline would otherwise move a glyph by one pixel.
                let mut physical = glyph.physical((fractional.0, fractional.1 + run.line_y), 1.0);
                physical.x += translation.0;
                physical.y += translation.1;
                renderer.glyph(physical, glyph.color_opt.unwrap_or(color));
            }
        }
    }
}

impl TextPainter {
    /// Uses the same font fallback and shaping as other software surfaces.
    /// WordOrGlyph also wraps an unbroken word without dropping its tail.
    pub fn paragraph(
        &mut self,
        text: &str,
        font: &Font<'_>,
        width: f32,
        line_height: f32,
        alignment: Option<Align>,
    ) -> Option<Paragraph> {
        if !width.is_finite()
            || width <= 0.0
            || !line_height.is_finite()
            || line_height <= 0.0
            || !font.size.is_finite()
            || font.size <= 0.0
        {
            return None;
        }
        let buffer = self.shape_buffer(text, font, Some(width), line_height, alignment);
        let height = buffer
            .layout_runs()
            .map(|run| run.line_top + run.line_height)
            .fold(line_height, f32::max);
        height.is_finite().then_some(Paragraph { buffer, height })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Font<'static> {
        Font {
            family: "Inter",
            size: 20.0,
            weight: 400,
            tabular: false,
            tracking: 0.0,
        }
    }

    #[test]
    fn all_paragraphs_and_unbroken_words_survive_wrapping() {
        let mut painter = TextPainter::new();
        let text = format!(
            "First paragraph\n\n{}\nآخر فقرة عربية بالتشكيلِ",
            "unbroken".repeat(70)
        );
        let paragraph = painter
            .paragraph(&text, &font(), 140.0, 32.0, None)
            .unwrap();
        let retained = paragraph
            .buffer
            .lines
            .iter()
            .map(|line| line.text())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(retained, text);
        let runs: Vec<_> = paragraph.buffer.layout_runs().collect();
        assert!(runs.len() > 30);
        assert!(runs.iter().any(|run| run.rtl));
        assert!(runs.iter().all(|run| run.line_w <= 141.0));
        assert!(paragraph.height() > 30.0 * 32.0);
    }

    #[test]
    fn strip_rendering_matches_one_complete_canvas_without_cutting_joining_marks() {
        let mut painter = TextPainter::new();
        let text =
            "سَلَامٌ عليكم ورحمة الله\nEnglish paragraph with several wrapped lines.\n\nفقرة أخيرة";
        let paragraph = painter
            .paragraph(text, &font(), 150.0, 34.0, Some(Align::Center))
            .unwrap();
        let height = paragraph.height().ceil() as u32 + 30;
        let mut whole = tiny_skia::Pixmap::new(180, height).unwrap();
        paragraph.draw(
            &mut painter,
            &mut whole.as_mut(),
            (15.0, 15.0),
            Color::hex("#ffffff"),
        );
        let mut combined = Vec::new();
        for top in (0..height).step_by(37) {
            let mut strip = tiny_skia::Pixmap::new(180, (height - top).min(37)).unwrap();
            paragraph.draw(
                &mut painter,
                &mut strip.as_mut(),
                (15.0, 15.0 - top as f32),
                Color::hex("#ffffff"),
            );
            combined.extend_from_slice(strip.data());
        }
        let mismatch = combined.iter().zip(whole.data()).position(|(a, b)| a != b);
        assert!(mismatch.is_none(), "first differing channel: {mismatch:?}");
        assert!(whole.data().iter().any(|byte| *byte != 0));
    }

    #[test]
    fn invalid_layout_dimensions_are_refused() {
        let mut painter = TextPainter::new();
        for (width, line_height) in [
            (0.0, 20.0),
            (f32::NAN, 20.0),
            (100.0, 0.0),
            (100.0, f32::INFINITY),
        ] {
            assert!(
                painter
                    .paragraph("text", &font(), width, line_height, None)
                    .is_none()
            );
        }
    }
}
