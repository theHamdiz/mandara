//! A bounded software strip, using the reading cards' existing ornament geometry.

use mandara_theme::Color;
use tiny_skia::{GradientStop, LinearGradient, Paint, PixmapMut, Point, SpreadMode, Transform};

use super::document::{Document, Layout, Ornament};
use crate::lock::paint::{self as lp, TextPainter};
use crate::plugins::jannah::art;

/// Enough perceptual samples for these subtle two-tone card grounds at 8-bit output.
const GRADIENT_STEPS: usize = 64;

mod decoration;
mod ground;

use ground::Ground;

/// What every strip reuses for one export: the pre-rendered top ornament,
/// the background tile and up to four rim ornaments, each painted once.
pub(super) struct Decoration {
    top: Option<tiny_skia::Pixmap>,
    ground: Ground,
    rim: [Option<Raster>; 4],
}

/// A pre-rendered ornament and where its top-left sits in the whole image.
struct Raster {
    pixels: tiny_skia::Pixmap,
    origin: (i32, i32),
}

/// The same Oklab interpolation as the UI's two-stop gradient.
fn interpolate(from: Color, to: Color, t: f32) -> Color {
    let (a, b) = (from.to_oklab(), to.to_oklab());
    let l = a.l + (b.l - a.l) * t;
    let x = a.a + (b.a - a.a) * t;
    let y = a.b + (b.b - a.b) * t;
    Color::oklcha(
        l,
        x.hypot(y),
        y.atan2(x).to_degrees(),
        from.alpha() + (to.alpha() - from.alpha()) * t,
    )
}

/// The card's background gradient for a canvas whose first row is image
/// row `top`, sampled in Oklab like the panel (a solid fill if the
/// gradient degenerates).
fn gradient(doc: &Document, layout: &Layout, top: u32) -> Paint<'static> {
    let (w, h) = (layout.width as f32, layout.height as f32);
    let radians = doc.angle.to_radians();
    let direction = (radians.sin(), -radians.cos());
    let reach = (w * direction.0.abs() + h * direction.1.abs()) / 2.0;
    let centre = (w / 2.0, h / 2.0 - top as f32);
    let stops = (0..=GRADIENT_STEPS)
        .map(|step| {
            let t = step as f32 / GRADIENT_STEPS as f32;
            GradientStop::new(
                t,
                lp::skia(interpolate(doc.background.0, doc.background.1, t), 1.0),
            )
        })
        .collect();
    let shader = LinearGradient::new(
        Point::from_xy(
            centre.0 - direction.0 * reach,
            centre.1 - direction.1 * reach,
        ),
        Point::from_xy(
            centre.0 + direction.0 * reach,
            centre.1 + direction.1 * reach,
        ),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    match shader {
        Some(shader) => Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        },
        None => lp::solid(doc.background.0, 1.0),
    }
}

/// Fills `rect` (x, y, width, height in strip coordinates) with `color`.
fn rectangle(canvas: &mut PixmapMut<'_>, rect: (f32, f32, f32, f32), color: Color) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(rect.0, rect.1, rect.2, rect.3) {
        // The rectangle fast path rejects a fractional two-column sliver
        // with no whole interior column. A path keeps the same geometry.
        canvas.fill_path(
            &tiny_skia::PathBuilder::from_rect(rect),
            &lp::solid(color, 1.0),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Paints image rows `top..top + canvas.height()` into the cleared strip:
/// background, ornaments, text blocks, then the rim, all offset by `top`
/// so the strips join seamlessly.
pub(super) fn strip(
    doc: &Document,
    layout: &Layout,
    decoration: &mut Decoration,
    text: &mut TextPainter,
    top: u32,
    canvas: &mut PixmapMut<'_>,
) {
    canvas.fill(tiny_skia::Color::TRANSPARENT);
    let (w, h, k, y) = (
        layout.width as f32,
        layout.height as f32,
        doc.scale,
        -(top as f32),
    );
    decoration.ground.paint(canvas, top);
    if let Some(image) = &decoration.top {
        blit(canvas, image.as_ref(), 0, -(top as i32));
    }
    match doc.ornament {
        Ornament::Plain => {}
        Ornament::Crown {
            inset,
            alpha,
            inner_alpha,
        } => {
            let inset = inset * k;
            let crown_w = w - inset * 2.0;
            let scale = crown_w / art::CROWN_VIEW.0;
            let crown_h = art::CROWN_VIEW.1 * scale;
            let stroke = art::CROWN_STROKE.0 * scale;
            let left = inset + art::CROWN_FOOT * scale - stroke / 2.0;
            let base = h - inset;
            let side_y = inset + crown_h - 0.5 * k;
            let ink = doc.accent.with_alpha(alpha);
            for x in [left, w - left - stroke] {
                rectangle(
                    canvas,
                    (x, y + side_y, stroke, (base - side_y).max(0.0)),
                    ink,
                );
            }
            rectangle(canvas, (left, y + base, w - left * 2.0, stroke), ink);
            let inner_w = art::CROWN_STROKE.1 * scale;
            let inner_x = inset + (art::CROWN_FOOT + art::CROWN_INNER) * scale - inner_w / 2.0;
            let inner_h = (base - art::CROWN_INNER * scale - side_y).max(0.0);
            for x in [inner_x, w - inner_x - inner_w] {
                rectangle(
                    canvas,
                    (x, y + side_y, inner_w, inner_h),
                    ink.with_alpha(alpha * inner_alpha),
                );
            }
        }
        Ornament::Manuscript { .. } => {}
    }
    for block in &layout.blocks {
        block.paragraph.draw(
            text,
            canvas,
            (block.origin.0, block.origin.1 - top as f32),
            block.color,
        );
    }
    for rim in decoration.rim.iter().flatten() {
        blit(
            canvas,
            rim.pixels.as_ref(),
            rim.origin.0,
            rim.origin.1 - top as i32,
        );
    }
}

/// Integer placement of premultiplied pixels needs no pattern shader or resampling.
fn blit(canvas: &mut PixmapMut<'_>, source: tiny_skia::PixmapRef<'_>, x: i32, y: i32) {
    let (sw, sh, dw, dh) = (
        source.width() as i32,
        source.height() as i32,
        canvas.width() as i32,
        canvas.height() as i32,
    );
    let (left, first, right, last) = (x.max(0), y.max(0), (x + sw).min(dw), (y + sh).min(dh));
    if left >= right || first >= last {
        return;
    }
    let bytes = (right - left) as usize * 4;
    for row in first..last {
        let source_at = ((row - y) * sw + left - x) as usize * 4;
        let target_at = (row * dw + left) as usize * 4;
        let source = &source.data()[source_at..source_at + bytes];
        let target = &mut canvas.data_mut()[target_at..target_at + bytes];
        for (d, s) in target
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(source.as_chunks::<4>().0)
        {
            match s[3] {
                0 => {}
                u8::MAX => d.copy_from_slice(s),
                alpha => {
                    let inverse = 255 - u32::from(alpha);
                    for channel in 0..4 {
                        d[channel] = (u32::from(s[channel])
                            + (u32::from(d[channel]) * inverse + 127) / 255)
                            as u8;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perceptual_ground_preserves_its_exact_endpoints() {
        let a = Color::hex("#132f3e");
        let b = Color::hex("#e1e8dc");
        assert_eq!(interpolate(a, b, 0.0).to_rgba8(), a.to_rgba8());
        assert_eq!(interpolate(a, b, 1.0).to_rgba8(), b.to_rgba8());
    }

    #[test]
    fn fractional_crown_strokes_match_across_strip_boundaries() {
        let (width, height) = (80, 360);
        let rect = (27.003, 10.25, 1.274, 330.5);
        let color = Color::hex("#6dafc3");
        let mut whole = tiny_skia::Pixmap::new(width, height).unwrap();
        rectangle(&mut whole.as_mut(), rect, color);
        let mut combined = Vec::new();
        for top in (0..height).step_by(37) {
            let mut strip = tiny_skia::Pixmap::new(width, (height - top).min(37)).unwrap();
            rectangle(
                &mut strip.as_mut(),
                (rect.0, rect.1 - top as f32, rect.2, rect.3),
                color,
            );
            combined.extend_from_slice(strip.data());
        }
        let mismatch = combined.iter().zip(whole.data()).position(|(a, b)| a != b);
        assert!(mismatch.is_none(), "first differing channel: {mismatch:?}");
        assert!(whole.data().iter().any(|byte| *byte != 0));
    }

    #[test]
    fn integer_blits_preserve_premultiplied_source_over_at_every_alpha() {
        let mut source = tiny_skia::Pixmap::new(256, 1).unwrap();
        for (alpha, pixel) in source
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .enumerate()
        {
            let alpha = alpha as u8;
            *pixel = [alpha, alpha / 2, 0, alpha];
        }
        let mut actual = tiny_skia::Pixmap::new(256, 1).unwrap();
        for pixel in actual.data_mut().as_chunks_mut::<4>().0 {
            *pixel = [16, 32, 48, 128];
        }
        let mut expected = actual.clone();
        expected.draw_pixmap(
            0,
            0,
            source.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        blit(&mut actual.as_mut(), source.as_ref(), 0, 0);
        assert_eq!(actual.data(), expected.data());
    }
}
