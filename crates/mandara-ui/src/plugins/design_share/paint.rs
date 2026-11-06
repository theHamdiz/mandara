//! A bounded software strip, using the reading cards' existing ornament geometry.

use mandara_i18n::t;
use mandara_theme::Color;
use tiny_skia::{GradientStop, LinearGradient, Paint, PixmapMut, Point, SpreadMode, Transform};

use super::document::{Document, Layout, Ornament};
use crate::lock::paint::{self as lp, TextPainter};
use crate::plugins::jannah::art;

/// Enough perceptual samples for these subtle two-tone card grounds at 8-bit output.
const GRADIENT_STEPS: usize = 64;

pub(super) struct Decoration {
    lattice: Option<resvg::usvg::Tree>,
    head: Option<resvg::usvg::Tree>,
    ground: Paint<'static>,
}

fn tinted(source: Option<String>, color: Color) -> Result<resvg::usvg::Tree, String> {
    let [r, g, b, a] = color.to_rgba8();
    let source = source
        .ok_or_else(|| t("The image design could not be prepared").to_string())?
        .replace("black", &format!("#{r:02x}{g:02x}{b:02x}{a:02x}"));
    resvg::usvg::Tree::from_str(&source, &resvg::usvg::Options::default())
        .map_err(|error| error.to_string())
}

impl Decoration {
    pub fn new(doc: &Document, layout: &Layout) -> Result<Self, String> {
        let lattice = (doc.lattice_alpha > 0.0)
            .then(|| {
                tinted(
                    art::asset("jannah-lattice"),
                    doc.accent.with_alpha(doc.lattice_alpha),
                )
            })
            .transpose()?;
        let head = match doc.ornament {
            Ornament::Plain => None,
            Ornament::Crown { alpha, .. } => Some(tinted(
                art::asset("jannah-crown"),
                doc.accent.with_alpha(alpha),
            )?),
            Ornament::Manuscript { .. } => Some(tinted(
                crate::plugins::hadith::art::asset("hadith-headpiece"),
                doc.accent,
            )?),
        };
        Ok(Self {
            lattice,
            head,
            ground: ground(doc, layout, 0),
        })
    }
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

fn ground(doc: &Document, layout: &Layout, top: u32) -> Paint<'static> {
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

fn rectangle(canvas: &mut PixmapMut<'_>, rect: (f32, f32, f32, f32), color: Color) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(rect.0, rect.1, rect.2, rect.3) {
        canvas.fill_rect(rect, &lp::solid(color, 1.0), Transform::identity(), None);
    }
}

pub(super) fn strip(
    doc: &Document,
    layout: &Layout,
    decoration: &Decoration,
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
    let shape = lp::rounded_rect(0.0, y, w, h, doc.radius * k);
    if let Some(path) = lp::rounded_rect(0.0, 0.0, w, h, doc.radius * k) {
        canvas.fill_path(
            &path,
            &decoration.ground,
            tiny_skia::FillRule::Winding,
            Transform::from_translate(0.0, y),
            None,
        );
    }
    if let Some(lattice) = &decoration.lattice {
        resvg::render(
            lattice,
            Transform::from_row(
                w / art::LATTICE_VIEW,
                0.0,
                0.0,
                w / art::LATTICE_VIEW,
                0.0,
                y,
            ),
            canvas,
        );
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
            let dawn = crate::plugins::jannah::panel::DAWN_GLOW;
            lp::glow(
                canvas,
                w / 2.0,
                y + inset + crown_h * crate::plugins::jannah::panel::DAWN_TOP_SHARE,
                dawn.1 * k,
                lp::skia(doc.accent, dawn.0),
            );
            if let Some(head) = &decoration.head {
                resvg::render(
                    head,
                    Transform::from_row(scale, 0.0, 0.0, scale, inset, y + inset),
                    canvas,
                );
            }
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
        Ornament::Manuscript {
            width_share,
            top,
            dawn_top,
        } => {
            let dawn = crate::plugins::jannah::panel::DAWN_GLOW;
            lp::glow(
                canvas,
                w / 2.0,
                y + dawn_top * k,
                dawn.1 * k,
                lp::skia(doc.accent, dawn.0),
            );
            if let Some(head) = &decoration.head {
                let width = w * width_share;
                let scale = width / crate::plugins::hadith::art::HEADPIECE_VIEW.0;
                resvg::render(
                    head,
                    Transform::from_row(scale, 0.0, 0.0, scale, (w - width) / 2.0, y + top * k),
                    canvas,
                );
            }
        }
    }
    for block in &layout.blocks {
        block.paragraph.draw(
            text,
            canvas,
            (block.origin.0, block.origin.1 - top as f32),
            block.color,
        );
    }
    lp::glass_rim(
        canvas,
        &shape,
        0.0,
        y,
        w,
        h,
        doc.palette.specular_rim_light,
        doc.palette.specular_rim_shadow,
        1.0,
        k,
    );
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
}
