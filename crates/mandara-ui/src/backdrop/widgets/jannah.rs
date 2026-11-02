//! The Jannah plugin's desktop widgets, from [`super::JannahInfo`] (today's
//! item as the plugin picked it for the desktop, and today's step):
//!
//! * `verse`: the calligraphic card under a pointed arch (crown, sides and
//!   an inner arch, a faint eight-pointed-star lattice in the crown, a star
//!   finial), the Arabic in the Qur'anic Naskh with the verse's ornate
//!   number, the meaning and the reference;
//! * `quote`: the same item in the Quote widget's design (its big mark,
//!   sizes and attribution line), with an Arabic ornate mark (﴿ for a verse,
//!   « for a hadith) and the reference as the attribution;
//! * `step`: today's step as a tile with its check and the week's dots.
//!
//! The Arabic block is always right to left (its own alignment mirrored
//! for a left-to-right interface); the meaning and the reference follow the
//! widget's language. Arabic lines get extra leading so tashkeel never
//! touches the line above. Everything is redrawn only when the item, the
//! step or its check changes.

use mandara_config::desktop_widgets::{FontRole, JannahLayout, JannahWidget};
use mandara_config::widget_kinds::{ranges, within};
use mandara_i18n::tk;

use super::JannahInfo;
use super::actions::Action;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, TextStyle, caps, face, style, tracked, wrap};
use super::lingo::Lingo;
use super::productive::{AUTHOR_TRACKING, QUOTE_AUTHOR, QUOTE_GAP, QUOTE_MARK};

/// Arabic lines advance this many line heights (room for tashkeel).
const ARABIC_LEADING: f32 = 1.32;
/// How far below the top of the ornate opening mark the Arabic starts (of
/// the mark's height).
const ARABIC_MARK_DROP: f32 = 1.2;
/// Alignments: centred, right edge; and how close counts as centred.
const CENTRED: f32 = 0.5;
const RIGHT: f32 = 1.0;
const ALIGN_EPSILON: f32 = 0.01;
/// The Arabic is set this much larger than the text size (Naskh reads
/// small), the meaning this much smaller.
const ARABIC_SCALE: f32 = 1.15;
const MEANING_SCALE: f32 = 0.6;
/// Most lines of the Arabic and of the meaning.
const ARABIC_LINES: usize = 7;
const MEANING_LINES: usize = 6;
/// Tracking of small caps labels.
const LABEL_TRACKING: f32 = 0.14;
/// Verse card: the crown's height over the width, the inner arch's inset,
/// stroke widths and alphas, side padding, gaps, the finial star.
const CROWN_RATIO: f32 = 0.34;
const INNER_INSET: f32 = 12.0;
const OUTER_W: f32 = 2.2;
const INNER_W: f32 = 1.1;
const OUTER_ALPHA: f32 = 0.75;
const INNER_ALPHA: f32 = 0.35;
const SIDE_PAD: f32 = 42.0;
const BOTTOM_PAD: f32 = 30.0;
const BLOCK_GAP: f32 = 16.0;
const FINIAL_R: f32 = 9.0;
const FINIAL_ALPHA: f32 = 0.8;
const DIVIDER_R: f32 = 5.0;
/// Content starts this share of the crown down.
const CONTENT_START: f32 = 0.78;
/// The crown's lattice: star size, spacing and alpha.
const LATTICE_R: f32 = 7.0;
const LATTICE_STEP: f32 = 26.0;
const LATTICE_ALPHA: f32 = 0.1;
/// Points per half of the sampled arch.
const ARCH_STEPS: usize = 40;
/// Step tile: width, title / line / label sizes, the check and the dots.
const STEP_W: f32 = 320.0;
const STEP_TITLE: f32 = 26.0;
const STEP_LINE: f32 = 15.0;
const STEP_LABEL: f32 = 12.0;
const STEP_GAP: f32 = 10.0;
const CHECK_R: f32 = 13.0;
const CHECK_W: f32 = 2.0;
const DOT_R: f32 = 4.5;
const DOT_TODAY_R: f32 = 6.0;
const DOT_GAP: f32 = 12.0;
const DOT_OFF_ALPHA: f32 = 0.22;
const RING_OFF_ALPHA: f32 = 0.5;
/// The tick inside the check circle, as fractions of its radius.
const TICK: [(f32, f32); 3] = [(-0.45, 0.02), (-0.1, 0.38), (0.5, -0.36)];
/// Caption under an AI rendering, relative to the meaning's size.
const AI_CAPTION: f32 = 0.78;

/// Lays out the Jannah widget `w`.
pub fn jannah(w: &JannahWidget, info: &JannahInfo, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    match w.layout {
        JannahLayout::Verse => verse(w, info, l, k, m),
        JannahLayout::Quote => quote(w, info, l, k, align, m),
        JannahLayout::Step => step(w, info, l, k, m),
    }
}

/// The reference in the widget's language (Arabic names and Eastern digits
/// in Arabic).
fn reference(info: &JannahInfo, l: &Lingo) -> String {
    if l.arabic() { info.reference_ar.clone() } else { l.digits(info.reference.clone()) }
}

/// The Arabic text, the verse's ornate number at its end.
fn arabic_text(info: &JannahInfo) -> String {
    match &info.ayah_mark {
        // A word joiner, not a space: the verse number never wraps onto a
        // line of its own.
        Some(mark) => format!("{}\u{2060}{mark}", info.arabic),
        None => info.arabic.clone(),
    }
}

/// Arabic lines placed in `width` (right to left, each aligned by `align`
/// where 1 is the right edge), from `y`; returns the bottom.
pub(crate) fn arabic_block(text: &str, st: &TextStyle, width: f32, align: f32, mut y: f32, marks: &mut Vec<Mark>, m: &mut dyn Measure) -> f32 {
    for line in wrap(text, st, width, ARABIC_LINES, m) {
        let (lw, lh) = m.measure(&line, st);
        let lead = lh * (ARABIC_LEADING - 1.0);
        marks.push(Mark::new(MarkKind::Text { text: line, style: st.clone(), x: (width - lw) * align, y: y + lead / 2.0, w: lw, h: lh }, InkRole::Fg));
        y += lh + lead;
    }
    y
}

/// Plain lines (the meaning) in `width`, aligned by `align`; returns the
/// bottom.
pub(crate) fn text_block(text: &str, st: &TextStyle, width: f32, align: f32, max: usize, ink: InkRole, alpha: f32, mut y: f32, marks: &mut Vec<Mark>, m: &mut dyn Measure) -> f32 {
    for line in wrap(text, st, width, max, m) {
        let (lw, lh) = m.measure(&line, st);
        marks.push(Mark::new(MarkKind::Text { text: line, style: st.clone(), x: (width - lw) * align, y, w: lw, h: lh }, ink).dim(alpha));
        y += lh;
    }
    y
}

/// The Arabic block's alignment: it starts at the right edge, as right-to-
/// left text does, whichever side the card is anchored to; only a centred
/// card centres it.
pub(crate) fn arabic_align(align: f32) -> f32 {
    if (align - CENTRED).abs() < ALIGN_EPSILON { CENTRED } else { RIGHT }
}

/// The meaning (when there is one) with the AI caption; returns the
/// bottom.
fn meaning(info: &JannahInfo, w: &JannahWidget, size: f32, width: f32, align: f32, l: &Lingo, mut y: f32, marks: &mut Vec<Mark>, m: &mut dyn Measure) -> f32 {
    let Some(text) = info.meaning.as_deref().filter(|t| !t.trim().is_empty()) else { return y };
    let st = style(w.look.font, size, face::REGULAR);
    y = text_block(text, &st, width, align, MEANING_LINES, InkRole::Fg, face::DIM, y, marks, m);
    if info.ai {
        let cs = style(w.look.font, size * AI_CAPTION, face::MEDIUM);
        let caption = l.word(tk("AI rendering of the meaning"));
        let (cw, ch) = m.measure(&caption, &cs);
        marks.push(Mark::new(MarkKind::Text { text: caption, style: cs, x: (width - cw) * align, y: y + ch * 0.3, w: cw, h: ch }, InkRole::Accent).dim(face::DIM));
        y += ch * 1.3;
    }
    y
}

/// Today's item in the Quote widget's design.
fn quote(w: &JannahWidget, info: &JannahInfo, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(within(w.width, ranges::WIDTH_PX, ranges::QUOTE_WIDTH_DEFAULT_PX));
    let size = px(within(w.text_size, ranges::TEXT_PX, ranges::QUOTE_TEXT_DEFAULT_PX));
    let ar_align = arabic_align(align);
    let mut marks = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0)];
    let mut y = 0.0;
    if w.mark {
        // An Arabic opening mark at the Arabic block's start: the ornate
        // bracket for a verse, the guillemet for a hadith.
        let ms = style(FontRole::Scripture, size * QUOTE_MARK, face::REGULAR);
        let glyph = if info.ayah_mark.is_some() { "\u{FD3F}" } else { "«" };
        let (gw, gh) = m.measure(glyph, &ms);
        marks.push(Mark::new(MarkKind::Text { text: glyph.into(), style: ms, x: (width - gw) * ar_align, y: 0.0, w: gw, h: gh }, InkRole::Accent));
        // The ornate marks fill their line, unlike a Latin quotation mark
        // riding high: the Arabic starts below it.
        y += gh * ARABIC_MARK_DROP;
    }
    let ast = style(FontRole::Scripture, size * ARABIC_SCALE, face::REGULAR);
    y = arabic_block(&arabic_text(info), &ast, width, ar_align, y, &mut marks, m);
    if info.meaning.is_some() {
        y += px(QUOTE_GAP);
        y = meaning(info, w, size * MEANING_SCALE, width, align, l, y, &mut marks, m);
    }
    y += px(QUOTE_GAP);
    let a = format!("— {}", reference(info, l));
    let st = tracked(style(w.look.font, size * QUOTE_AUTHOR, face::SEMIBOLD), AUTHOR_TRACKING);
    let (aw, ah) = m.measure(&a, &st);
    marks.push(Mark::new(MarkKind::Text { text: a, style: st, x: (width - aw) * align, y, w: aw, h: ah }, InkRole::Accent).dim(face::DIM));
    y += ah;
    Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, width, y), action: Action::Panel(crate::plugins::jannah::ID.into()) }])
}

/// The calligraphic card under a pointed arch.
fn verse(w: &JannahWidget, info: &JannahInfo, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    use crate::plugins::jannah::art;
    let px = |v: f32| v * k;
    let width = px(within(w.width, ranges::WIDTH_PX, ranges::QUOTE_WIDTH_DEFAULT_PX));
    let size = px(within(w.text_size, ranges::TEXT_PX, ranges::QUOTE_TEXT_DEFAULT_PX));
    let crown_h = width * CROWN_RATIO;
    let inner_w = width - 2.0 * px(SIDE_PAD);
    let mut content = Vec::new();
    // Centred content, laid out in its own box, then moved in.
    let ast = style(FontRole::Scripture, size * ARABIC_SCALE, face::REGULAR);
    let mut y = arabic_block(&arabic_text(info), &ast, inner_w, 0.5, 0.0, &mut content, m);
    y += px(BLOCK_GAP);
    content.push(Mark::new(MarkKind::Poly { pts: art::star((inner_w / 2.0, y), px(DIVIDER_R)) }, InkRole::Accent).dim(FINIAL_ALPHA));
    y += px(DIVIDER_R) + px(BLOCK_GAP);
    if info.meaning.is_some() {
        y = meaning(info, w, size * MEANING_SCALE, inner_w, 0.5, l, y, &mut content, m);
        y += px(BLOCK_GAP);
    }
    let rs = tracked(style(w.look.font, size * QUOTE_AUTHOR, face::SEMIBOLD), LABEL_TRACKING);
    let r = caps(&reference(info, l));
    let (rw, rh) = m.measure(&r, &rs);
    content.push(Mark::new(MarkKind::Text { text: r, style: rs, x: (inner_w - rw) / 2.0, y, w: rw, h: rh }, InkRole::Accent));
    y += rh;
    let top = crown_h * CONTENT_START;
    let height = top + y + px(BOTTOM_PAD);

    let mut marks = Vec::new();
    // The lattice, faint, inside the crown.
    let step = px(LATTICE_STEP);
    let rows = (crown_h / step).ceil() as i32;
    let cols = (width / step).ceil() as i32;
    for row in 0..rows {
        for col in 0..=cols {
            let shift = if row % 2 == 1 { step / 2.0 } else { 0.0 };
            let c = (col as f32 * step + shift, crown_h - row as f32 * step - step / 2.0);
            // Only stars well inside the arch's opening.
            if c.0 < px(INNER_INSET) * 2.0 || c.0 > width - px(INNER_INSET) * 2.0 || !inside_crown(c, width, crown_h) {
                continue;
            }
            marks.push(Mark::new(MarkKind::Polyline { pts: closed(art::star(c, px(LATTICE_R))), width: px(INNER_W) }, InkRole::Accent).dim(LATTICE_ALPHA));
        }
    }
    // The arch: crown and sides, then the inner arch.
    let half = px(OUTER_W) / 2.0;
    let mut outer = vec![(half, height)];
    outer.extend(art::crown(half, half, width - 2.0 * half, crown_h, ARCH_STEPS));
    outer.push((width - half, height));
    marks.push(Mark::new(MarkKind::Polyline { pts: outer, width: px(OUTER_W) }, InkRole::Accent).dim(OUTER_ALPHA));
    let i = px(INNER_INSET);
    let mut inner = vec![(i, height - i)];
    inner.extend(art::crown(i, i * 1.6, width - 2.0 * i, crown_h - i * 0.6, ARCH_STEPS));
    inner.push((width - i, height - i));
    marks.push(Mark::new(MarkKind::Polyline { pts: inner, width: px(INNER_W) }, InkRole::Accent).dim(INNER_ALPHA));
    marks.push(Mark::new(MarkKind::Line { a: (0.0, height - half), b: (width, height - half), width: px(OUTER_W) }, InkRole::Accent).dim(OUTER_ALPHA));
    // A star finial under the point.
    marks.push(Mark::new(MarkKind::Poly { pts: art::star((width / 2.0, i * 1.6 + px(FINIAL_R) * 2.2), px(FINIAL_R)) }, InkRole::Accent).dim(FINIAL_ALPHA));
    for mut mk in content {
        mk.shift(px(SIDE_PAD), top);
        marks.push(mk);
    }
    Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, width, height), action: Action::Panel(crate::plugins::jannah::ID.into()) }])
}

/// Whether `c` lies under the crown of an arch `width` wide and `h` tall
/// (between its two curves).
fn inside_crown(c: (f32, f32), width: f32, h: f32) -> bool {
    let pts = crate::plugins::jannah::art::crown(0.0, 0.0, width, h, ARCH_STEPS);
    // The arch's height over `c.x`: the crown point whose x is nearest.
    pts.iter().filter(|p| (p.0 - c.0).abs() < width / ARCH_STEPS as f32).map(|p| p.1).fold(f32::INFINITY, f32::min) + LATTICE_R * 2.0 < c.1
}

fn closed(mut pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    if let Some(first) = pts.first().copied() {
        pts.push(first);
    }
    pts
}

/// Today's step as a tile with its check and the week.
fn step(w: &JannahWidget, info: &JannahInfo, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(STEP_W);
    // Start edge: left, or right in a right-to-left language.
    let align = if l.rtl { 1.0 } else { 0.0 };
    let mut marks = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0)];
    let ks = tracked(style(w.look.font, px(STEP_LABEL), face::BOLD), LABEL_TRACKING);
    let mut y = text_block(&caps(&l.word(tk("Today's step"))), &ks, width, align, 1, InkRole::Accent, 1.0, 0.0, &mut marks, m);
    y += px(STEP_GAP) / 2.0;
    y = text_block(&l.word(&info.step_title), &style(w.look.font, px(STEP_TITLE), face::SEMIBOLD), width, align, 2, InkRole::Fg, 1.0, y, &mut marks, m);
    y += px(STEP_GAP) / 2.0;
    y = text_block(&l.word(&info.step_line), &style(w.look.font, px(STEP_LINE), face::REGULAR), width, align, 3, InkRole::Fg, face::DIM, y, &mut marks, m);
    y += px(STEP_GAP) * 1.5;
    // The check and its words.
    let r = px(CHECK_R);
    let row_top = y;
    let cx = if l.rtl { width - r } else { r };
    let c = (cx, y + r);
    if info.step_done {
        marks.push(Mark::new(MarkKind::Disc { c, r }, InkRole::Accent));
        let tick: Vec<(f32, f32)> = TICK.iter().map(|(dx, dy)| (c.0 + dx * r, c.1 + dy * r)).collect();
        marks.push(Mark::new(MarkKind::Polyline { pts: tick, width: px(CHECK_W) }, InkRole::OnAccent));
    } else {
        marks.push(Mark::new(MarkKind::Arc { c, r: r - px(CHECK_W) / 2.0, width: px(CHECK_W), start: 0.0, sweep: std::f32::consts::TAU }, InkRole::Fg).dim(RING_OFF_ALPHA));
    }
    let label = if info.step_done { l.word(tk("Done today")) } else { l.word(tk("I did this today")) };
    let ls = style(w.look.font, px(STEP_LINE), face::MEDIUM);
    let (lw, lh) = m.measure(&label, &ls);
    let lx = if l.rtl { width - 2.0 * r - px(STEP_GAP) - lw } else { 2.0 * r + px(STEP_GAP) };
    marks.push(Mark::new(MarkKind::Text { text: label, style: ls, x: lx, y: c.1 - lh / 2.0, w: lw, h: lh }, if info.step_done { InkRole::Accent } else { InkRole::Fg }));
    let row_w = 2.0 * r + px(STEP_GAP) + lw;
    let row_x = if l.rtl { width - row_w } else { 0.0 };
    let hits = vec![Hit { rect: (row_x, row_top, row_w, 2.0 * r), action: Action::JannahStep }];
    y += 2.0 * r + px(STEP_GAP) * 1.5;
    // The week: seven dots, filled on the days a step was done.
    let gap = px(DOT_GAP) + 2.0 * px(DOT_R);
    for (i, done) in info.week.iter().enumerate() {
        let today = i == info.today;
        let rr = if today { px(DOT_TODAY_R) } else { px(DOT_R) };
        let x = px(DOT_TODAY_R) + i as f32 * gap;
        let x = if l.rtl { width - x } else { x };
        let mark = Mark::new(MarkKind::Disc { c: (x, y + px(DOT_TODAY_R)), r: rr }, if *done { InkRole::Accent } else { InkRole::Fg });
        marks.push(if *done { mark } else { mark.dim(DOT_OFF_ALPHA) });
    }
    Composition::with_hits(marks, hits)
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn info() -> JannahInfo {
        JannahInfo {
            arabic: "كَلِمَاتٌ عَرَبِيَّةٌ طَوِيلَةٌ ".repeat(6),
            ayah_mark: Some("\u{FD3F}٢٥\u{FD3E}".into()),
            meaning: Some("A meaning in the reader's language, long enough to wrap onto a second line.".into()),
            ai: true,
            reference: "Al-Baqara 2:25".into(),
            reference_ar: "البقرة ٢:٢٥".into(),
            step_title: "Keep your tongue".into(),
            step_line: "Guard what you say.".into(),
            step_done: false,
            week: [true, false, true, false, false, false, false],
            today: 2,
        }
    }

    fn lingo(rtl: bool) -> Lingo {
        Lingo { rtl, ..Lingo::of(&Default::default(), rtl, None) }
    }

    #[test]
    fn every_layout_lays_out_in_both_directions() {
        for layout in [JannahLayout::Verse, JannahLayout::Quote, JannahLayout::Step] {
            for rtl in [false, true] {
                let c = jannah(&JannahWidget { layout, ..Default::default() }, &info(), &lingo(rtl), 1.0, 0.0, &mut Fixed);
                assert!(c.size.0 > 100.0 && c.size.1 > 100.0, "{layout:?} {:?}", c.size);
                assert!(!c.hits.is_empty());
            }
        }
    }

    #[test]
    fn the_arabic_is_scripture_right_to_left_with_room_for_marks() {
        let w = JannahWidget { layout: JannahLayout::Quote, ..Default::default() };
        let c = quote(&w, &info(), &lingo(false), 1.0, 0.0, &mut Fixed);
        let arabic: Vec<(f32, f32, f32, f32)> = c
            .marks
            .iter()
            .filter_map(|m| match &m.kind {
                MarkKind::Text { style, x, y, w, h, .. } if style.role == FontRole::Scripture => Some((*x, *y, *w, *h)),
                _ => None,
            })
            .collect();
        assert!(arabic.len() >= 3, "the mark and wrapped lines");
        // Left-aligned widget: the Arabic hugs the right edge.
        let width = ranges::QUOTE_WIDTH_DEFAULT_PX;
        assert!(arabic.iter().skip(1).all(|(x, _, w, _)| (x + w - width).abs() < 0.5));
        // Lines are spaced wider than their height.
        let lines = &arabic[1..];
        assert!(lines.windows(2).all(|p| p[1].1 - p[0].1 > p[0].3 * 1.2));
        // The attribution is the reference, after a dash.
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "— Al-Baqara 2:25")));
        // Arabic interface: the Arabic reference.
        let ar = Lingo { lang: Some(mandara_i18n::Lang::Ar), ..lingo(true) };
        let c = quote(&w, &info(), &ar, 1.0, 1.0, &mut Fixed);
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "— البقرة ٢:٢٥")));
    }

    #[test]
    fn the_step_check_is_clickable() {
        let c = step(&JannahWidget { layout: JannahLayout::Step, ..Default::default() }, &info(), &lingo(false), 1.0, &mut Fixed);
        let hit = c.hits.iter().find(|h| h.action == Action::JannahStep).unwrap();
        assert_eq!(c.hit((hit.rect.0 + 2.0, hit.rect.1 + 2.0)), Some(&Action::JannahStep));
        let done = JannahInfo { step_done: true, ..info() };
        let c = step(&JannahWidget::default(), &done, &lingo(true), 1.0, &mut Fixed);
        assert!(c.marks.iter().any(|m| matches!(m.kind, MarkKind::Disc { .. }) && m.ink == InkRole::Accent));
        assert!(c.marks.iter().any(|m| matches!(m.kind, MarkKind::Polyline { .. }) && m.ink == InkRole::OnAccent));
    }
}
