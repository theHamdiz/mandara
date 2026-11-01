//! The frame around a painted widget (the shared shape controls of
//! [`mandara_config::widget_style`]): the plate in any shape, an outline, a
//! header line, and a chrome — a classic window, washi tape or a polaroid.
//!
//! The content keeps its own layout; the frame is laid out around it, so
//! every widget kind gets every shape. Non-rectangular plates grow until the
//! content fits inside them (a circle around a wide clock becomes a circle
//! around its diagonal), so text never spills over the edge.

use mandara_config::desktop_widgets::{Chrome, Look, Plate, PlateShape};
use mandara_config::widget_style::lobes;
use mandara_core::shape::Shape;

use super::layout::{Composition, InkRole, Mark, MarkKind, Measure, caps, face, style, tracked};

/// Width of the glass plate's specular rim, reference px.
const PLATE_RIM_PX: f32 = 1.0;
/// Gap between a header (and its rule) and the content, reference px.
const HEADER_GAP_PX: f32 = 10.0;
const HEADER_RULE_PX: f32 = 1.0;
const HEADER_RULE_ALPHA: f32 = 0.35;
const HEADER_TRACKING: f32 = 0.12;
/// How much bigger than the content box each shape is drawn so the content
/// stays inside it (multiplier of the content's diagonal or size).
const SQUIRCLE_GROW: f32 = 1.12;
const POLYGON_GROW: f32 = 1.45;
/// Window chrome: title bar height, close box, pinstripe pitch and alpha.
const TITLE_BAR_PX: f32 = 22.0;
const CLOSE_BOX_PX: f32 = 11.0;
const CLOSE_BOX_INSET_PX: f32 = 8.0;
const PINSTRIPE_PITCH_PX: f32 = 2.0;
const PINSTRIPE_ALPHA: f32 = 0.22;
const TITLE_SIZE_PX: f32 = 13.0;
const TITLE_PAD_PX: f32 = 8.0;
/// The bar's divider under the title.
const TITLE_RULE_ALPHA: f32 = 0.45;
/// Polaroid: border at the sides / top as a fraction of the padding, and
/// the deeper bottom border.
const POLAROID_SIDE: f32 = 0.5;
const POLAROID_BOTTOM: f32 = 1.7;
const POLAROID_CAPTION_PX: f32 = 18.0;
/// Washi tape: strip size, its opacity and stripe count.
const TAPE_W_PX: f32 = 96.0;
const TAPE_H_PX: f32 = 26.0;
const TAPE_ALPHA: f32 = 0.62;
const TAPE_STRIPES: usize = 5;
const TAPE_STRIPE_ALPHA: f32 = 0.35;
const TAPE_STRIPE_W_PX: f32 = 1.5;
/// Teeth on each torn end of a strip.
const TAPE_TEETH: usize = 4;
const TAPE_TOOTH_PX: f32 = 3.0;
/// Seed of the deckled edge (the same tear every repaint).
const DECKLE_SEED: u32 = 0x4D41_4E44;

/// The shared shape for `look`'s plate in a box of `size`, at `k`
/// physical px per reference px.
pub fn plate_shape(look: &Look, k: f32) -> Shape {
    shape_of(look.plate_shape, look, k)
}

/// The shared shape `kind` with `look`'s parameters.
pub fn shape_of(kind: PlateShape, look: &Look, k: f32) -> Shape {
    let radius = look.plate_radius() * k;
    match kind {
        PlateShape::Rect => Shape::Rect { radius },
        PlateShape::Circle => Shape::Circle,
        PlateShape::Squircle => Shape::Squircle,
        PlateShape::Cookie => Shape::Cookie { lobes: lobes(look.plate_lobes), depth: look.plate_depth() },
        PlateShape::Flower => Shape::Flower { petals: lobes(look.plate_lobes), depth: look.plate_depth() },
        PlateShape::Pentagon => Shape::Polygon { sides: PENTAGON, radius },
        PlateShape::Chamfer => Shape::Chamfer { cut: look.plate_chamfer() * k },
        PlateShape::Skew => Shape::Skew { degrees: look.plate_skew(), radius },
        PlateShape::Deckle => Shape::Deckle { depth: look.plate_depth() / 4.0, seed: DECKLE_SEED },
    }
}

/// Sides of the pentagon shape.
const PENTAGON: u32 = 5;

/// The box a `kind` plate needs around content of `(w, h)` so the content
/// stays inside the shape: `(width, height)`.
pub fn grown((w, h): (f32, f32), kind: PlateShape, look: &Look, pad: f32, k: f32) -> (f32, f32) {
    let diag = (w * w + h * h).sqrt();
    match kind {
        // A round corner cuts `r (1 − 1/√2)` into the box: padding thinner
        // than that would let the corner bite into what sits in it.
        PlateShape::Rect => {
            let r = (look.plate_radius() * k).min(w.min(h) / 2.0);
            let extra = 2.0 * (r * (1.0 - std::f32::consts::FRAC_1_SQRT_2) - pad).max(0.0);
            (w + extra, h + extra)
        }
        PlateShape::Deckle => (w, h),
        PlateShape::Circle => (diag, diag),
        PlateShape::Cookie | PlateShape::Flower => {
            let d = diag / (1.0 - look.plate_depth());
            (d, d)
        }
        PlateShape::Squircle => (w * SQUIRCLE_GROW, h * SQUIRCLE_GROW),
        PlateShape::Pentagon => (w * POLYGON_GROW, h * POLYGON_GROW),
        PlateShape::Chamfer => {
            let c = look.plate_chamfer() * k;
            (w + c, h + c)
        }
        // The lean eats into the width at the top and the bottom.
        PlateShape::Skew => (w + h * look.plate_skew().to_radians().tan().abs(), h),
    }
}

/// `comp` with `look`'s header, plate, outline and chrome around it
/// (sizes in reference px × `k`), header lines aligned by `align`.
pub fn frame(mut comp: Composition, look: &Look, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let outline = look.outline_width() * k;
    let has_frame = look.plate != Plate::None || outline > 0.0 || look.chrome != Chrome::None;
    let header_inside = !look.header.trim().is_empty() && matches!(look.chrome, Chrome::None | Chrome::Tape);
    if comp.marks.is_empty() || (!has_frame && !header_inside) {
        return comp;
    }
    let px = |v: f32| v * k;
    if header_inside {
        add_header(&mut comp, look, k, align, m);
    }
    if !has_frame {
        return comp;
    }
    let (cw, ch) = comp.size;
    let pad = look.plate_padding() * k;
    // The content box with the padding, then grown to fit the shape.
    let (mut top, mut bottom, side) = match look.chrome {
        Chrome::Polaroid => (pad * POLAROID_SIDE, pad * POLAROID_BOTTOM + px(POLAROID_CAPTION_PX), pad * POLAROID_SIDE),
        _ => (pad, pad, pad),
    };
    if look.chrome == Chrome::Window {
        top += px(TITLE_BAR_PX);
    }
    // Shapes are drawn around the content; a chrome keeps its plate square.
    let kind = if matches!(look.chrome, Chrome::Window | Chrome::Polaroid) { PlateShape::Rect } else { look.plate_shape };
    let (iw, ih) = (cw + 2.0 * side, ch + top + bottom);
    let (gw, gh) = grown((iw, ih), kind, look, pad, k);
    let (gx, gy) = ((gw - iw) / 2.0, (gh - ih) / 2.0);
    top += gy;
    bottom += gy;
    let (x0, y0) = (-side - gx, -top);
    let (w, h) = (gw, ch + top + bottom);
    let shape = shape_of(kind, look, k);
    let mut under = Vec::new();
    if look.plate != Plate::None || matches!(look.chrome, Chrome::Window | Chrome::Polaroid) {
        let opacity = match (look.plate, look.chrome) {
            (_, Chrome::Polaroid) | (Plate::Solid, _) | (Plate::None, _) => 1.0,
            _ => look.plate_opacity(),
        };
        let rim = if look.plate == Plate::Glass { px(PLATE_RIM_PX) } else { 0.0 };
        let ink = if look.chrome == Chrome::Polaroid { InkRole::Paper } else { InkRole::Plate };
        under.push(Mark::new(MarkKind::Plate { x: x0, y: y0, w, h, radius: look.plate_radius() * k, rim, shape }, ink).dim(opacity));
    }
    let mut over = Vec::new();
    match look.chrome {
        Chrome::Window => window_chrome(&mut under, &mut over, look, (x0, y0, w), k, m),
        Chrome::Polaroid => {
            let caption = look.header.trim();
            if !caption.is_empty() {
                let s = style(look.font, px(POLAROID_CAPTION_PX), face::MEDIUM);
                let (tw, th) = m.measure(caption, &s);
                let band = bottom - gy;
                over.push(Mark::new(MarkKind::Text { text: caption.to_string(), style: s, x: x0 + (w - tw) / 2.0, y: ch + (band - th) / 2.0, w: tw, h: th }, InkRole::OnPaper));
            }
        }
        Chrome::Tape => tape(&mut over, look, (x0, y0, w), k),
        Chrome::None => {}
    }
    if outline > 0.0 {
        let inset = outline / 2.0;
        over.push(Mark::new(MarkKind::Shaped { x: x0 + inset, y: y0 + inset, w: w - outline, h: h - outline, shape: shrink(shape, inset), width: outline }, InkRole::Outline));
    }
    under.append(&mut comp.marks);
    under.append(&mut over);
    comp.marks = under;
    comp.normalize();
    comp
}

/// `shape` for a box `inset` px smaller on every side (keeps an outline's
/// rounded corners concentric with the plate's).
fn shrink(shape: Shape, inset: f32) -> Shape {
    match shape {
        Shape::Rect { radius } => Shape::Rect { radius: (radius - inset).max(0.0) },
        Shape::Polygon { sides, radius } => Shape::Polygon { sides, radius: (radius - inset).max(0.0) },
        Shape::Skew { degrees, radius } => Shape::Skew { degrees, radius: (radius - inset).max(0.0) },
        Shape::Chamfer { cut } => Shape::Chamfer { cut: (cut - inset).max(0.0) },
        s => s,
    }
}

/// The header line and its rule above the content.
fn add_header(comp: &mut Composition, look: &Look, k: f32, align: f32, m: &mut dyn Measure) {
    let px = |v: f32| v * k;
    let text = caps(look.header.trim());
    let s = tracked(style(look.font, px(look.header_size()), face::SEMIBOLD), HEADER_TRACKING);
    let (tw, th) = m.measure(&text, &s);
    let rule = if look.header_rule { px(HEADER_RULE_PX) + px(HEADER_GAP_PX) / 2.0 } else { 0.0 };
    let below = th + px(HEADER_GAP_PX) / 2.0 + rule;
    comp.shift(0.0, below);
    let width = comp.size.0.max(tw);
    comp.marks.push(Mark::new(MarkKind::Text { text, style: s, x: (width - tw) * align, y: 0.0, w: tw, h: th }, InkRole::Accent));
    if look.header_rule {
        let y = th + px(HEADER_GAP_PX) / 2.0;
        comp.marks.push(Mark::new(MarkKind::Line { a: (0.0, y), b: (width, y), width: px(HEADER_RULE_PX) }, InkRole::Fg).dim(HEADER_RULE_ALPHA));
    }
    comp.normalize();
}

/// A classic window's title bar: pinstripes, a close box at the reading
/// start and the header as the title.
fn window_chrome(under: &mut Vec<Mark>, over: &mut Vec<Mark>, look: &Look, (x0, y0, w): (f32, f32, f32), k: f32, m: &mut dyn Measure) {
    let px = |v: f32| v * k;
    let bar = px(TITLE_BAR_PX);
    let title = look.header.trim();
    let s = style(look.font, px(TITLE_SIZE_PX), face::BOLD);
    let (tw, th) = if title.is_empty() { (0.0, 0.0) } else { m.measure(title, &s) };
    let gap = (x0 + (w - tw) / 2.0 - px(TITLE_PAD_PX), x0 + (w + tw) / 2.0 + px(TITLE_PAD_PX));
    let close = (x0 + px(CLOSE_BOX_INSET_PX), y0 + (bar - px(CLOSE_BOX_PX)) / 2.0);
    let close_end = close.0 + px(CLOSE_BOX_PX) + px(TITLE_PAD_PX) / 2.0;
    let (left, right) = (x0 + px(CLOSE_BOX_INSET_PX) / 2.0, x0 + w - px(CLOSE_BOX_INSET_PX) / 2.0);
    let segments: Vec<(f32, f32)> = if title.is_empty() { vec![(left, close.0 - px(TITLE_PAD_PX) / 4.0), (close_end, right)] } else { vec![(left, close.0 - px(TITLE_PAD_PX) / 4.0), (close_end, gap.0), (gap.1, right)] };
    let mut y = y0 + px(PINSTRIPE_PITCH_PX) * 2.0;
    while y < y0 + bar - px(PINSTRIPE_PITCH_PX) * 1.5 {
        for &(a, b) in segments.iter().filter(|(a, b)| b - a > px(1.0)) {
            under.push(Mark::new(MarkKind::Line { a: (a, y), b: (b, y), width: px(1.0) }, InkRole::Fg).dim(PINSTRIPE_ALPHA));
        }
        y += px(PINSTRIPE_PITCH_PX);
    }
    over.push(Mark::new(MarkKind::Rect { x: close.0, y: close.1, w: px(CLOSE_BOX_PX), h: px(CLOSE_BOX_PX), radius: 0.0, width: px(1.0) }, InkRole::Fg));
    if !title.is_empty() {
        over.push(Mark::new(MarkKind::Text { text: title.to_string(), style: s, x: x0 + (w - tw) / 2.0, y: y0 + (bar - th) / 2.0, w: tw, h: th }, InkRole::Fg));
    }
    over.push(Mark::new(MarkKind::Line { a: (x0, y0 + bar), b: (x0 + w, y0 + bar), width: px(1.0) }, InkRole::Fg).dim(TITLE_RULE_ALPHA));
}

/// Two strips of washi tape across the top corners, turned by the tape
/// angle, with torn ends and a few stripes.
fn tape(over: &mut Vec<Mark>, look: &Look, (x0, y0, w): (f32, f32, f32), k: f32) {
    let px = |v: f32| v * k;
    let (tw, th) = (px(TAPE_W_PX), px(TAPE_H_PX));
    let angle = look.tape_angle();
    for (corner, turn) in [((x0, y0), -angle), ((x0 + w, y0), angle)] {
        let strip = torn_strip(tw, th, px(TAPE_TOOTH_PX));
        let place = |p: (f32, f32)| mandara_core::shape::rotate((corner.0 + p.0 - tw / 2.0, corner.1 + p.1 - th / 2.0), corner, turn);
        over.push(Mark::new(MarkKind::Poly { pts: strip.iter().map(|p| place(*p)).collect() }, InkRole::Chrome).dim(TAPE_ALPHA));
        for i in 1..TAPE_STRIPES {
            let x = tw * i as f32 / TAPE_STRIPES as f32;
            let lean = th * 0.5;
            over.push(Mark::new(MarkKind::Line { a: place((x - lean / 2.0, 0.0)), b: place((x + lean / 2.0, th)), width: px(TAPE_STRIPE_W_PX) }, InkRole::Paper).dim(TAPE_STRIPE_ALPHA * TAPE_ALPHA));
        }
    }
}

/// A `w × h` strip whose short ends are zig-zag torn.
fn torn_strip(w: f32, h: f32, tooth: f32) -> Vec<(f32, f32)> {
    let mut pts = vec![(0.0, 0.0), (w, 0.0)];
    for i in 1..=TAPE_TEETH * 2 {
        let y = h * i as f32 / (TAPE_TEETH * 2) as f32;
        pts.push((if i % 2 == 1 { w - tooth } else { w }, y));
    }
    pts.push((0.0, h));
    for i in (1..TAPE_TEETH * 2).rev() {
        let y = h * i as f32 / (TAPE_TEETH * 2) as f32;
        pts.push((if i % 2 == 1 { tooth } else { 0.0 }, y));
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn content() -> Composition {
        Composition::of(vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: 200.0, h: 80.0, radius: 0.0, width: 0.0 }, InkRole::Fg)])
    }

    #[test]
    fn defaults_add_nothing() {
        let c = frame(content(), &Look::default(), 1.0, 0.0, &mut Fixed);
        assert_eq!(c, content());
    }

    #[test]
    fn a_plain_plate_is_the_old_rounded_plate() {
        let look = Look { plate: Plate::Glass, ..Look::default() };
        let c = frame(content(), &look, 1.0, 0.0, &mut Fixed);
        let MarkKind::Plate { w, h, shape, .. } = &c.marks[0].kind else { panic!("{:?}", c.marks[0]) };
        let pad = look.plate_padding();
        assert_eq!((*w, *h), (200.0 + 2.0 * pad, 80.0 + 2.0 * pad));
        assert_eq!(*shape, Shape::Rect { radius: look.plate_radius() });
    }

    #[test]
    fn a_circle_plate_holds_the_whole_content() {
        let look = Look { plate: Plate::Solid, plate_shape: PlateShape::Circle, plate_padding: 0.0, ..Look::default() };
        let c = frame(content(), &look, 1.0, 0.0, &mut Fixed);
        let MarkKind::Plate { x, y, w, h, .. } = c.marks[0].kind.clone() else { panic!() };
        assert!((w - h).abs() < 0.01, "a circle");
        let (cx, cy, r) = (x + w / 2.0, y + h / 2.0, w / 2.0);
        // Every corner of the content is inside it.
        let b = c.marks[1].bounds();
        for p in [(b.0, b.1), (b.2, b.1), (b.0, b.3), (b.2, b.3)] {
            assert!(((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt() <= r + 0.01, "{p:?}");
        }
    }

    #[test]
    fn header_outline_and_chromes_frame_the_content() {
        let look = Look { header: "sys.clock // الساعة".into(), outline_width: 2.0, ..Look::default() };
        let c = frame(content(), &look, 1.0, 0.0, &mut Fixed);
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text.starts_with("SYS.CLOCK"))));
        assert!(c.marks.iter().any(|m| m.ink == InkRole::Outline));
        for chrome in [Chrome::Window, Chrome::Tape, Chrome::Polaroid] {
            let look = Look { chrome, header: "Note".into(), ..Look::default() };
            let c = frame(content(), &look, 1.0, 0.0, &mut Fixed);
            assert!(c.size.0 >= 200.0 && c.size.1 > 80.0, "{chrome:?}: {:?}", c.size);
            assert!(c.marks.iter().all(|m| {
                let b = m.bounds();
                b.0 >= -0.01 && b.1 >= -0.01 && b.2 <= c.size.0 + 0.01 && b.3 <= c.size.1 + 0.01
            }));
        }
    }
}
