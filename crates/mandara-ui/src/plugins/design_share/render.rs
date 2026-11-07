//! Measure once, paint one reusable strip and stream every row into the PNG.

use std::io::Write as _;

use crate::lock::paint::TextPainter;
use mandara_i18n::{t, tf};

use super::{document::Document, paint::Decoration, png::Output};

mod clip;

/// At the maximum supported export width, one RGBA strip is two MiB.
const STRIP_ROWS: u32 = 128;

pub(super) fn png(doc: &Document, checkpoint: impl FnMut() -> bool) -> Result<Vec<u8>, String> {
    render(doc, STRIP_ROWS, checkpoint)
}

fn render(
    doc: &Document,
    strip_rows: u32,
    mut checkpoint: impl FnMut() -> bool,
) -> Result<Vec<u8>, String> {
    let mut output = Output::new();
    doc.validate()?;
    let mut text = TextPainter::new();
    text.set_arabic_font(&doc.fonts.arabic);
    let layout = doc.layout(&mut text, &mut checkpoint)?;
    let clip = clip::Clip::new(doc, &layout)?;
    let mut decoration = Decoration::new(doc, &layout)?;
    let strip_rows = strip_rows.min(layout.height).max(1);
    let mut pixels = tiny_skia::Pixmap::new(layout.width, strip_rows)
        .ok_or_else(|| t("The image design could not be prepared").to_string())?;
    let mut mask = tiny_skia::Mask::new(layout.width, strip_rows)
        .ok_or_else(|| t("The image design could not be prepared").to_string())?;
    {
        let mut encoder = png::Encoder::new(&mut output, layout.width, layout.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer = encoder.write_header().map_err(error)?;
        let mut stream = writer.stream_writer().map_err(error)?;
        for top in (0..layout.height).step_by(strip_rows as usize) {
            if !checkpoint() {
                return Err(t("Image copying cancelled").into());
            }
            let height = (layout.height - top).min(strip_rows);
            let length = layout.width as usize * height as usize * 4;
            let mut canvas = pixels.as_mut();
            super::paint::strip(doc, &layout, &mut decoration, &mut text, top, &mut canvas);
            clip.fill(&mut mask, top);
            canvas.apply_mask(&mask);
            // PNG stores straight alpha. Convert the reusable strip in place.
            for pixel in canvas.data_mut().as_chunks_mut::<4>().0 {
                let alpha = u32::from(pixel[3]);
                if alpha == 0 {
                    pixel.fill(0);
                } else if alpha < 255 {
                    for channel in &mut pixel[..3] {
                        *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                    }
                }
            }
            stream
                .write_all(&canvas.data_mut()[..length])
                .map_err(error)?;
        }
        stream.finish().map_err(error)?;
        writer.finish().map_err(error)?;
    }
    Ok(output.finish())
}

fn error(error: impl std::fmt::Display) -> String {
    tf("Could not copy the image: {error}", &[("error", &error)])
}

#[cfg(test)]
mod tests {
    use super::super::document::tests::document;
    use super::*;

    #[test]
    fn streamed_rows_match_one_strip_and_keep_the_final_paragraph() {
        let text = "Arabic: السَّلامُ عليكم\n\nA complete paragraph with words.\n".repeat(10)
            + "THE COMPLETE FINAL PARAGRAPH";
        let doc = document(&text);
        let narrow = render(&doc, 37, || true).unwrap();
        let complete = render(&doc, 8192, || true).unwrap();
        let a = image::load_from_memory(&narrow).unwrap().into_rgba8();
        let b = image::load_from_memory(&complete).unwrap().into_rgba8();
        assert_eq!(a.dimensions(), b.dimensions());
        let mismatch = a.as_raw().iter().zip(b.as_raw()).position(|(a, b)| a != b);
        assert!(mismatch.is_none(), "first differing channel: {mismatch:?}");
        assert!(a.height() > 1000);
        let tail = (a.height().saturating_sub(80)..a.height().saturating_sub(30)).any(|y| {
            (40..a.width() - 40).any(|x| {
                let pixel = a.get_pixel(x, y);
                pixel[0] > 130 && pixel[1] > 130 && pixel[2] > 130
            })
        });
        assert!(
            tail,
            "the final paragraph must be painted before the bottom padding"
        );
    }

    #[test]
    fn cancelling_mid_export_never_returns_a_partial_png() {
        let doc = document(&"long content ".repeat(100));
        let mut checkpoints = 0;
        assert!(
            render(&doc, 37, || {
                checkpoints += 1;
                checkpoints < 4
            })
            .is_err()
        );
        assert_eq!(checkpoints, 4);
    }

    #[test]
    fn the_longest_real_jannah_card_matches_one_complete_canvas() {
        for language in [mandara_i18n::Lang::En, mandara_i18n::Lang::Ar] {
            let _language = mandara_i18n::scoped(Some(language));
            for dark in [false, true] {
                for scale in [1.0, 1.5] {
                    let doc = crate::plugins::jannah::longest_export_document(dark, scale);
                    let striped = render(&doc, 37, || true).unwrap();
                    let full = render(&doc, u32::MAX, || true).unwrap();
                    let a = image::load_from_memory(&striped).unwrap().into_rgba8();
                    let b = image::load_from_memory(&full).unwrap().into_rgba8();
                    assert!(a.height() > 640);
                    assert_eq!(a.dimensions(), b.dimensions());
                    let mismatch = a.as_raw().iter().zip(b.as_raw()).position(|(a, b)| a != b);
                    assert!(
                        mismatch.is_none(),
                        "{} dark={dark} scale={scale}: first differing channel: {mismatch:?}",
                        language.code(),
                    );
                }
            }
        }
    }
}
