//! Fun on the desktop: a little cat, soap bubbles, a photo frame and
//! stickers. The cat and the bubbles move only while the desktop shows
//! (a few frames a second for the cat); the photo changes on its timer or
//! on a click.

use std::f32::consts::{PI, TAU};

use chrono::{DateTime, Local, Timelike as _};
use mandara_config::desktop_widgets::{BubblesWidget, PetWidget, PhotoWidget, StickerWidget};
use mandara_config::widget_kinds::{count, ranges, within};
use mandara_core::shape::Shape;
use mandara_i18n::tk;

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, Picture, face, style};
use super::lingo::Lingo;

/// Frames per second of the cat (awake, asleep) and of the bubbles.
pub const PET_FPS: u32 = 12;
pub const PET_SLEEP_FPS: u32 = 4;
pub const BUBBLES_FPS: u32 = 24;
/// The cat sleeps between these hours.
const SLEEP_FROM: u32 = 0;
const SLEEP_UNTIL: u32 = 6;
/// It reacts this long after a click, ms.
pub const POKE_MS: i64 = 3000;
/// Breathing period, blink period and blink length, seconds.
const BREATH_S: f32 = 3.2;
const BLINK_EVERY_S: f32 = 4.3;
const BLINK_S: f32 = 0.16;
const BREATH: f32 = 0.025;
/// Head bob along with music: beats per second and height.
const BOB_HZ: f32 = 2.0;
const BOB: f32 = 0.025;
const TAIL_HZ: f32 = 0.35;
const PASTEL_TURN: u16 = 25;
const MS_PER_S: f32 = 1000.0;
/// Bubbles: sideways wobble (of the size) and its frequency, the shine.
const WOBBLE: f32 = 0.6;
const WOBBLE_HZ: f32 = 0.4;
const RIM_ALPHA: f32 = 0.55;
const FILL_ALPHA: f32 = 0.06;
const SHINE_ALPHA: f32 = 0.7;
const SHINE: f32 = 0.22;
const RIM_W: f32 = 1.5;
/// Photo frame placeholder text size.
const NOTE: f32 = 16.0;

/// A deterministic `0..1` for `(seed, i)`.
fn rand01(seed: u64, i: u64) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ i.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

/// Whether the cat sleeps at `now`.
pub fn asleep(now: &DateTime<Local>) -> bool {
    (SLEEP_FROM..SLEEP_UNTIL).contains(&now.hour())
}

/// The cat at `now` (`poked`: ms since its last click, `music`: something
/// plays). Everything is live (its own plane, redrawn per frame).
pub fn pet(w: &PetWidget, now: &DateTime<Local>, poked: Option<i64>, music: bool, k: f32, m: &mut dyn Measure) -> Composition {
    let s = within(w.size, ranges::PET_SIZE_PX, ranges::PET_SIZE_DEFAULT_PX) * k;
    let t = (now.timestamp_millis() % 1_000_000) as f32 / MS_PER_S;
    let sleeping = asleep(now) && poked.is_none();
    let happy = poked.is_some();
    let body_ink = InkRole::Pastel(PASTEL_TURN);
    let line = InkRole::OnPaper;
    let breath = 1.0 + BREATH * (t / BREATH_S * TAU).sin();
    let bob = if music && !sleeping { BOB * s * (t * BOB_HZ * TAU).sin().abs() } else { 0.0 };
    let mut marks = Vec::new();
    // The box it lives in (with room for hearts and letters above).
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: s * 1.5, h: s * 1.35, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0));
    let ground = s * 1.3;
    let cx = s * 0.7;
    // Tail: a curve swaying behind the body.
    let sway = if sleeping { 0.15 } else { (t * TAIL_HZ * TAU).sin() };
    let tail: Vec<(f32, f32)> = (0..=16)
        .map(|i| {
            let f = i as f32 / 16.0;
            (cx + s * 0.28 + s * 0.42 * f, ground - s * 0.12 - s * 0.5 * f * f + s * 0.12 * sway * f * (PI * f).sin())
        })
        .collect();
    marks.push(Mark::new(MarkKind::Polyline { pts: tail.clone(), width: s * 0.11 }, line));
    marks.push(Mark::new(MarkKind::Polyline { pts: tail, width: s * 0.08 }, body_ink));
    // Body: a loaf when asleep, sitting up otherwise.
    let (bw, bh) = if sleeping { (s * 0.95, s * 0.45 * breath) } else { (s * 0.66, s * 0.62 * breath) };
    let body = Shape::Circle;
    marks.push(Mark::new(MarkKind::Shaped { x: cx - bw / 2.0, y: ground - bh, w: bw, h: bh, shape: body, width: s * 0.03 }, line));
    marks.push(Mark::new(MarkKind::Shaped { x: cx - bw / 2.0, y: ground - bh, w: bw, h: bh, shape: body, width: 0.0 }, body_ink));
    // Head.
    let hr = s * 0.25;
    let head = if sleeping { (cx - bw * 0.28, ground - bh * 0.95) } else { (cx, ground - bh - hr * 0.55 - bob) };
    for (side, ink, grow) in [(1.0, line, s * 0.03), (1.0, body_ink, 0.0)] {
        for dir in [-1.0f32, 1.0] {
            let base = head.0 + dir * hr * 0.55 * side;
            let ear = vec![(base - hr * 0.33 - grow, head.1 - hr * 0.5), (base + hr * 0.33 + grow, head.1 - hr * 0.5), (base + dir * hr * 0.12, head.1 - hr * 1.15 - grow)];
            marks.push(Mark::new(MarkKind::Poly { pts: ear }, ink));
        }
    }
    marks.push(Mark::new(MarkKind::Disc { c: head, r: hr + s * 0.015 }, line));
    marks.push(Mark::new(MarkKind::Disc { c: head, r: hr }, body_ink));
    // Eyes: open, blinking, closed when asleep, happy arcs when clicked.
    let blink = !sleeping && !happy && (t % BLINK_EVERY_S) < BLINK_S;
    let eye_y = head.1 - hr * 0.05;
    for dir in [-1.0f32, 1.0] {
        let e = (head.0 + dir * hr * 0.42, eye_y);
        if happy {
            marks.push(Mark::new(MarkKind::Arc { c: (e.0, e.1 + hr * 0.08), r: hr * 0.16, width: s * 0.025, start: PI * 1.1, sweep: PI * 0.8 }, line));
        } else if sleeping || blink {
            marks.push(Mark::new(MarkKind::Arc { c: (e.0, e.1 - hr * 0.08), r: hr * 0.16, width: s * 0.025, start: PI * 0.1, sweep: PI * 0.8 }, line));
        } else {
            marks.push(Mark::new(MarkKind::Disc { c: e, r: hr * 0.13 }, line));
            marks.push(Mark::new(MarkKind::Disc { c: (e.0 + hr * 0.04, e.1 - hr * 0.05), r: hr * 0.04 }, body_ink));
        }
    }
    // Nose and whiskers.
    let nose = (head.0, head.1 + hr * 0.25);
    marks.push(Mark::new(MarkKind::Poly { pts: vec![(nose.0 - hr * 0.09, nose.1 - hr * 0.05), (nose.0 + hr * 0.09, nose.1 - hr * 0.05), (nose.0, nose.1 + hr * 0.06)] }, InkRole::Accent));
    for dir in [-1.0f32, 1.0] {
        for dy in [-0.04f32, 0.08] {
            marks.push(Mark::new(MarkKind::Line { a: (nose.0 + dir * hr * 0.3, nose.1 + hr * dy), b: (nose.0 + dir * hr * 1.05, nose.1 + hr * (dy * 2.0 - 0.05)), width: s * 0.012 }, line).dim(face::DIM));
        }
    }
    let float = |phase: f32| ((t + phase) % 2.0) / 2.0;
    let fs = style(w.look.font, s * 0.16, face::BOLD);
    if sleeping {
        for (i, phase) in [0.0f32, 0.7, 1.4].into_iter().enumerate() {
            let f = float(phase);
            let text = "z".to_string();
            let (zw, zh) = m.measure(&text, &fs);
            marks.push(Mark::new(MarkKind::Text { text, style: style(w.look.font, s * (0.1 + 0.04 * i as f32), face::BOLD), x: head.0 + hr * (0.8 + f), y: head.1 - hr * (1.0 + 1.6 * f), w: zw, h: zh }, InkRole::Fg).dim(1.0 - f));
        }
    } else if happy {
        for (i, phase) in [0.0f32, 0.5, 1.0].into_iter().enumerate() {
            let f = float(phase);
            let c = (head.0 + hr * (i as f32 - 1.0) * 1.1, head.1 - hr * (1.3 + 1.2 * f));
            let r = s * 0.035;
            marks.push(Mark::new(MarkKind::Disc { c: (c.0 - r * 0.6, c.1), r }, InkRole::Accent).dim(1.0 - f));
            marks.push(Mark::new(MarkKind::Disc { c: (c.0 + r * 0.6, c.1), r }, InkRole::Accent).dim(1.0 - f));
            marks.push(Mark::new(MarkKind::Poly { pts: vec![(c.0 - r * 1.55, c.1 + r * 0.3), (c.0 + r * 1.55, c.1 + r * 0.3), (c.0, c.1 + r * 1.9)] }, InkRole::Accent).dim(1.0 - f));
        }
        if !w.name.trim().is_empty() {
            let ns = style(w.look.font, s * 0.13, face::SEMIBOLD);
            let (nw, nh) = m.measure(w.name.trim(), &ns);
            marks.push(Mark::new(MarkKind::Text { text: w.name.trim().to_string(), style: ns, x: cx - nw / 2.0, y: ground + s * 0.02, w: nw, h: nh }, InkRole::Fg));
        }
    } else if music {
        let f = float(0.0);
        let text = "♪".to_string();
        let (nw, nh) = m.measure(&text, &fs);
        marks.push(Mark::new(MarkKind::Text { text, style: fs, x: head.0 - hr * (1.4 + 0.3 * f), y: head.1 - hr * (0.6 + 1.2 * f), w: nw, h: nh }, InkRole::Accent).dim(1.0 - f));
    }
    for mk in &mut marks {
        mk.live = true;
    }
    let mut c = Composition::of(marks);
    c.hits.push(Hit { rect: (0.0, 0.0, c.size.0, c.size.1), action: Action::Next });
    c
}

/// Soap bubbles rising and wobbling through their box at `now`.
pub fn bubbles(w: &BubblesWidget, now: &DateTime<Local>, k: f32) -> Composition {
    let (bw, bh) = (within(w.width, ranges::AREA_PX, ranges::BUBBLES_WIDTH_DEFAULT_PX) * k, within(w.height, ranges::AREA_PX, ranges::BUBBLES_HEIGHT_DEFAULT_PX) * k);
    let (lo, hi) = (within(w.min_size, ranges::BUBBLE_PX, ranges::BUBBLE_MIN_DEFAULT_PX), within(w.max_size, ranges::BUBBLE_PX, ranges::BUBBLE_MAX_DEFAULT_PX));
    let (lo, hi) = (lo.min(hi) * k, lo.max(hi) * k);
    let speed = within(w.speed, ranges::BUBBLE_SPEED, ranges::BUBBLE_SPEED_DEFAULT) * k;
    let t = (now.timestamp_millis() % 10_000_000) as f32 / MS_PER_S;
    let mut marks = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: bw, h: bh, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0).live()];
    for i in 0..count(w.count, ranges::BUBBLES) as u64 {
        let size = lo + (hi - lo) * rand01(1, i);
        let r = size / 2.0;
        let travel = bh + size;
        let v = speed * (0.6 + 0.8 * rand01(2, i));
        let y = bh + r - ((t * v + rand01(3, i) * travel) % travel);
        let x = r + (bw - size) * rand01(4, i) + WOBBLE * r * (t * WOBBLE_HZ * TAU + rand01(5, i) * TAU).sin();
        marks.push(Mark::new(MarkKind::Disc { c: (x, y), r }, InkRole::Accent).dim(FILL_ALPHA).live());
        marks.push(Mark::new(MarkKind::Arc { c: (x, y), r, width: RIM_W * k, start: 0.0, sweep: TAU }, InkRole::Fg).dim(RIM_ALPHA).live());
        marks.push(Mark::new(MarkKind::Arc { c: (x, y), r: r * 0.7, width: RIM_W * k * 1.5, start: PI * 1.1, sweep: PI * 0.35 }, InkRole::Paper).dim(SHINE_ALPHA).live());
        marks.push(Mark::new(MarkKind::Disc { c: (x + r * 0.35, y + r * 0.4), r: r * SHINE * 0.4 }, InkRole::Paper).dim(SHINE_ALPHA / 2.0).live());
    }
    Composition::of(marks)
}

/// The photo frame: the picture covering its box, clipped to the crop
/// shape; a click shows the next one.
pub fn photo(w: &PhotoWidget, pic: Option<&Picture>, folder: &str, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let (pw, ph) = (within(w.width, ranges::AREA_PX, ranges::PHOTO_W_DEFAULT_PX) * k, within(w.height, ranges::AREA_PX, ranges::PHOTO_H_DEFAULT_PX) * k);
    let shape = match shape_of(w.crop, &w.look, k) {
        Shape::Rect { radius } => Shape::Rect { radius: radius.min(pw.min(ph) / 4.0) },
        s => s,
    };
    let mut marks = Vec::new();
    match pic {
        Some(p) => marks.push(Mark::new(MarkKind::Image { x: 0.0, y: 0.0, w: pw, h: ph, pic: p.clone(), shape, turn: 0.0 }, InkRole::Fg)),
        None => {
            marks.push(Mark::new(MarkKind::Shaped { x: 0.0, y: 0.0, w: pw, h: ph, shape, width: 0.0 }, InkRole::Fg).dim(FILL_ALPHA * 2.0));
            let text = l.word(tk("No pictures in {folder}")).replace("{folder}", folder);
            let st = style(w.look.font, NOTE * k, face::MEDIUM);
            let text = super::layout::fit(&text, &st, pw * 0.9, m);
            let (tw, th) = m.measure(&text, &st);
            marks.push(Mark::new(MarkKind::Text { text, style: st, x: (pw - tw) / 2.0, y: (ph - th) / 2.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM));
        }
    }
    Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, pw, ph), action: Action::Next }])
}

/// The sticker's cut-out border: the picture's shape grown by `r` px
/// (a disc around every opaque pixel).
pub fn grow_alpha(src: &tiny_skia::Pixmap, r: u32) -> Option<tiny_skia::Pixmap> {
    let (w, h) = (src.width() + 2 * r, src.height() + 2 * r);
    let mut out = tiny_skia::Pixmap::new(w, h)?;
    let a: Vec<u8> = src.pixels().iter().map(|p| p.alpha()).collect();
    let (sw, sh, r) = (src.width() as i64, src.height() as i64, r as i64);
    let reach: Vec<i64> = (-r..=r).map(|dy| (((r * r - dy * dy) as f32).sqrt()) as i64).collect();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let mut best = 0u8;
            'disc: for (i, dy) in (-r..=r).enumerate() {
                let sy = y - r + dy;
                if !(0..sh).contains(&sy) {
                    continue;
                }
                for dx in -reach[i]..=reach[i] {
                    let sx = x - r + dx;
                    if (0..sw).contains(&sx) {
                        best = best.max(a[(sy * sw + sx) as usize]);
                        if best == u8::MAX {
                            break 'disc;
                        }
                    }
                }
            }
            if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(best, best, best, best) {
                out.pixels_mut()[(y * w as i64 + x) as usize] = p;
            }
        }
    }
    Some(out)
}

/// Most pixels the cut-out border reaches on the copy it is grown on.
const GROW_REACH: f32 = 4.0;

/// `src`'s alpha shrunk by `f` (the largest of each `f × f` block).
fn shrink_alpha(src: &tiny_skia::Pixmap, f: u32) -> Option<tiny_skia::Pixmap> {
    if f <= 1 {
        return Some(src.clone());
    }
    let (w, h) = (src.width().div_ceil(f), src.height().div_ceil(f));
    let mut out = tiny_skia::Pixmap::new(w, h)?;
    for (i, px) in src.pixels().iter().enumerate() {
        let (x, y) = (i as u32 % src.width() / f, i as u32 / src.width() / f);
        let o = &mut out.pixels_mut()[(y * w + x) as usize];
        let a = px.alpha().max(o.alpha());
        if let Some(p) = tiny_skia::PremultipliedColorU8::from_rgba(a, a, a, a) {
            *o = p;
        }
    }
    Some(out)
}

/// A sticker: the picture over its grown, paper-white outline.
pub fn sticker(w: &StickerWidget, pic: Option<&Picture>, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let size = within(w.size, ranges::AREA_PX, ranges::STICKER_SIZE_DEFAULT_PX) * k;
    let Some(p) = pic else {
        let text = if w.image.trim().is_empty() { l.word(tk("Pick a picture for the sticker")) } else { l.word(tk("Can't open {file}")).replace("{file}", w.image.trim()) };
        let st = style(w.look.font, NOTE * k, face::MEDIUM);
        let (tw, th) = m.measure(&text, &st);
        return Composition::of(vec![Mark::new(MarkKind::Text { text, style: st, x: 0.0, y: 0.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM)]);
    };
    let (iw, ih) = (p.pixmap.width() as f32, p.pixmap.height() as f32);
    let scale = size / iw.max(ih).max(1.0);
    let (dw, dh) = (iw * scale, ih * scale);
    let border = within(w.border, ranges::STICKER_BORDER_PX, ranges::STICKER_BORDER_DEFAULT_PX) * k;
    let mut marks = Vec::new();
    if border > 0.0 {
        // Grown on a smaller copy of the picture's shape (the border is
        // soft-edged anyway): a few pixels of reach at most.
        let r_src = (border / scale).max(1.0);
        let f = (r_src / GROW_REACH).ceil().max(1.0) as u32;
        let r = (r_src / f as f32).round().max(1.0) as u32;
        if let Some(grown) = shrink_alpha(&p.pixmap, f).and_then(|small| grow_alpha(&small, r)) {
            let rr = (r * f) as f32 * scale;
            let mask = Picture { key: p.key ^ (u64::from(r) << 8 | u64::from(f)), pixmap: std::sync::Arc::new(grown) };
            marks.push(Mark::new(MarkKind::Mask { x: -rr, y: -rr, w: dw + 2.0 * rr, h: dh + 2.0 * rr, mask }, InkRole::Paper));
        }
    }
    marks.push(Mark::new(MarkKind::Image { x: 0.0, y: 0.0, w: dw, h: dh, pic: p.clone(), shape: Shape::SQUARE, turn: 0.0 }, InkRole::Fg));
    Composition::of(marks)
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;
    use chrono::TimeZone as _;

    fn l() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    #[test]
    fn the_cat_sleeps_at_night_and_is_all_live() {
        let night = Local.with_ymd_and_hms(2026, 3, 1, 2, 0, 0).unwrap();
        let day = Local.with_ymd_and_hms(2026, 3, 1, 14, 0, 0).unwrap();
        assert!(asleep(&night) && !asleep(&day));
        for (now, poke, music) in [(night, None, false), (day, None, true), (day, Some(100), false)] {
            let c = pet(&PetWidget { name: "Pip".into(), ..Default::default() }, &now, poke, music, 1.0, &mut Fixed);
            assert!(c.marks.iter().all(|m| m.live) && c.hits.len() == 1);
        }
    }

    #[test]
    fn bubbles_stay_in_their_box() {
        let now = Local.with_ymd_and_hms(2026, 3, 1, 14, 0, 7).unwrap();
        let w = BubblesWidget::default();
        let c = bubbles(&w, &now, 1.0);
        assert!(c.size.0 <= ranges::BUBBLES_WIDTH_DEFAULT_PX + ranges::BUBBLE_MAX_DEFAULT_PX * WOBBLE + 2.0, "{:?}", c.size);
    }

    #[test]
    fn a_grown_outline_reaches_round_the_picture() {
        let mut pm = tiny_skia::Pixmap::new(10, 10).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let g = grow_alpha(&pm, 3).unwrap();
        assert_eq!((g.width(), g.height()), (16, 16));
        assert_eq!(g.pixel(8, 1).unwrap().alpha(), 255, "above the middle");
        assert_eq!(g.pixel(0, 0).unwrap().alpha(), 0, "the corner stays round");
        assert!(photo(&PhotoWidget::default(), None, "~/Pictures", &l(), 1.0, &mut Fixed).hits.len() == 1);
    }
}
