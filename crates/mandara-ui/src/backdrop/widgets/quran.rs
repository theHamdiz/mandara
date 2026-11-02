//! The Qur'an plugin's desktop widgets, from [`super::QuranInfo`]:
//!
//! * `ayah`: the ayah of the day in Jannah's design family, drawn by
//!   Jannah's own painter: under its pointed arch (the default) or in the
//!   quote card's design, with the ornate number, the meaning and the
//!   reference; a click opens the Qur'an panel;
//! * `progress`: the khatm as a ring (its share in the middle), beside it
//!   the surah you're reading in calligraphy, the ayah and juz, a
//!   "continue" line and, while a recitation plays, what is recited; a
//!   click opens the reader where you left off.
//!
//! The ring starts at the reading start (12 o'clock, clockwise; mirrored
//! for a right-to-left language). Redrawn only when the day, the khatm or
//! the recited ayah changes.

use std::f32::consts::{FRAC_PI_2, TAU};

use mandara_config::desktop_widgets::{FontRole, JannahLayout, JannahWidget, QuranLayout, QuranWidget};
use mandara_i18n::tk;

use super::actions::Action;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, TextStyle, caps, face, style, tracked};
use super::lingo::Lingo;
use super::{JannahInfo, QuranInfo};

/// Progress widget: ring radius and stroke, the track's alpha, the gap to
/// the text, sizes of the share, its label, the kicker, the surah name,
/// the lines; the leading of the Arabic name.
const RING_R: f32 = 62.0;
const RING_W: f32 = 7.0;
const TRACK_ALPHA: f32 = 0.2;
const RING_GAP: f32 = 26.0;
const SHARE_PX: f32 = 30.0;
const SHARE_LABEL_PX: f32 = 12.0;
const KICKER_PX: f32 = 12.0;
const NAME_PX: f32 = 34.0;
const LINE_PX: f32 = 15.0;
const LINE_GAP: f32 = 6.0;
const NAME_LEADING: f32 = 1.25;
/// Tracking of the small caps labels.
const LABEL_TRACKING: f32 = 0.14;
/// The text column's width.
const TEXT_W: f32 = 230.0;

/// Lays out the Qur'an widget `w`.
pub fn quran(w: &QuranWidget, info: &QuranInfo, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    match w.layout {
        QuranLayout::Ayah | QuranLayout::Quote => ayah(w, info, l, k, align, m),
        QuranLayout::Progress => progress(w, info, l, k, m),
    }
}

/// The ayah of the day through Jannah's painter (arch or quote card); a
/// click opens the Qur'an panel instead of Jannah's.
fn ayah(w: &QuranWidget, info: &QuranInfo, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let jw = JannahWidget { layout: if w.layout == QuranLayout::Quote { JannahLayout::Quote } else { JannahLayout::Verse }, width: w.width, text_size: w.text_size, mark: w.mark, look: w.look.clone(), locale: w.locale.clone() };
    let ji = JannahInfo {
        arabic: info.arabic.clone(),
        ayah_mark: Some(info.ayah_mark.clone()),
        meaning: info.meaning.clone(),
        ai: false,
        reference: info.reference.clone(),
        reference_ar: info.reference_ar.clone(),
        ..JannahInfo::default()
    };
    let mut c = super::jannah::jannah(&jw, &ji, l, k, align, m);
    for h in &mut c.hits {
        h.action = Action::Panel(crate::plugins::quran::ID.into());
    }
    c
}

/// One line of the text column: placed at `y` in a column `width` wide
/// (aligned by `align`, 1 = the right edge) with `lead` line heights.
#[expect(clippy::too_many_arguments, reason = "a line needs its text, look, ink and place; bundling them would only rename the list")]
fn put(text: String, st: TextStyle, ink: InkRole, dim: f32, (width, align, lead): (f32, f32, f32), y: &mut f32, col: &mut Vec<Mark>, m: &mut dyn Measure) {
    let (tw, th) = m.measure(&text, &st);
    let tw = tw.min(width);
    col.push(Mark::new(MarkKind::Text { text, style: st, x: (width - tw) * align, y: *y + th * (lead - 1.0) / 2.0, w: tw, h: th }, ink).dim(dim));
    *y += th * lead;
}

/// The khatm ring beside where you read.
fn progress(w: &QuranWidget, info: &QuranInfo, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let (r, rw) = (px(RING_R), px(RING_W));
    let ring_d = 2.0 * r + rw;
    let text_w = px(TEXT_W);
    let width = ring_d + px(RING_GAP) + text_w;
    // The ring sits at the reading start.
    let ring_x = if l.rtl { width - ring_d } else { 0.0 };
    let text_x = if l.rtl { 0.0 } else { ring_d + px(RING_GAP) };
    let align = if l.rtl { 1.0 } else { 0.0 };
    let mut marks = Vec::new();
    // The text column, laid out from the top, then centred on the ring.
    let mut col: Vec<Mark> = Vec::new();
    let mut y = 0.0;
    let ks = tracked(style(w.look.font, px(KICKER_PX), face::BOLD), LABEL_TRACKING);
    let kicker = if info.reciting.is_some() { l.word(tk("Reciting")) } else { l.word(tk("Continue reading")) };
    put(caps(&kicker), ks, InkRole::Accent, 1.0, (text_w, align, 1.0), &mut y, &mut col, m);
    y += px(LINE_GAP);
    put(info.surah_ar.clone(), style(FontRole::Scripture, px(NAME_PX), face::REGULAR), InkRole::Accent, 1.0, (text_w, align, NAME_LEADING), &mut y, &mut col, m);
    if !l.arabic() {
        put(info.surah.clone(), style(w.look.font, px(LINE_PX + 2.0), face::SEMIBOLD), InkRole::Fg, 1.0, (text_w, align, 1.0), &mut y, &mut col, m);
        y += px(LINE_GAP) / 2.0;
    }
    let detail = l.digits(l.word(tk("Ayah {ayah} of {ayahs} · Juz {juz}")).replace("{ayah}", &info.ayah.to_string()).replace("{ayahs}", &info.ayahs.to_string()).replace("{juz}", &info.juz.to_string()));
    put(detail, style(w.look.font, px(LINE_PX), face::REGULAR), InkRole::Fg, face::DIM, (text_w, align, 1.0), &mut y, &mut col, m);
    if let Some(at) = &info.reciting {
        y += px(LINE_GAP);
        put(l.digits(at.clone()), style(w.look.font, px(LINE_PX), face::MEDIUM), InkRole::Accent, 1.0, (text_w, align, 1.0), &mut y, &mut col, m);
    }
    let height = ring_d.max(y);
    let top = (height - y) / 2.0;
    for mut mk in col {
        mk.shift(text_x, top);
        marks.push(mk);
    }
    // The ring: its track, then the khatm's share from 12 o'clock.
    let c = (ring_x + ring_d / 2.0, height / 2.0);
    marks.push(Mark::new(MarkKind::Arc { c, r, width: rw, start: 0.0, sweep: TAU }, InkRole::Accent).dim(TRACK_ALPHA));
    let share = info.progress.clamp(0.0, 1.0);
    if share > 0.002 {
        let sweep = TAU * share * if l.rtl { -1.0 } else { 1.0 };
        marks.push(Mark::new(MarkKind::Arc { c, r, width: rw, start: -FRAC_PI_2, sweep }, InkRole::Accent));
    }
    let pct = l.digits(format!("{}%", (share * 100.0).floor() as u32));
    let ps = style(w.look.font, px(SHARE_PX), face::SEMIBOLD);
    let (pw, ph) = m.measure(&pct, &ps);
    let ls = style(w.look.font, px(SHARE_LABEL_PX), face::MEDIUM);
    let label = if info.khatms > 0 { l.digits(l.word(tk("khatm {n}")).replace("{n}", &(info.khatms + 1).to_string())) } else { l.word(tk("khatm")) };
    let (lw, lh) = m.measure(&label, &ls);
    let block = ph + lh;
    marks.push(Mark::new(MarkKind::Text { text: pct, style: ps, x: c.0 - pw / 2.0, y: c.1 - block / 2.0, w: pw, h: ph }, InkRole::Fg));
    marks.push(Mark::new(MarkKind::Text { text: label, style: ls, x: c.0 - lw / 2.0, y: c.1 - block / 2.0 + ph, w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
    Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, width, height), action: Action::QuranContinue }])
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn info() -> QuranInfo {
        QuranInfo {
            arabic: "كَلِمَاتٌ عَرَبِيَّةٌ طَوِيلَةٌ ".repeat(5),
            ayah_mark: "\u{FD3F}٢٥٥\u{FD3E}".into(),
            meaning: Some("A meaning in the reader's language, long enough to wrap.".into()),
            reference: "Al-Baqara 2:255".into(),
            reference_ar: "البقرة ٢:٢٥٥".into(),
            progress: 0.42,
            khatms: 1,
            surah_ar: "البقرة".into(),
            surah: "Al-Baqara".into(),
            ayah: 255,
            ayahs: 286,
            juz: 3,
            reciting: Some("2:255".into()),
            lang: None,
        }
    }

    fn lingo(rtl: bool) -> Lingo {
        Lingo { rtl, ..Lingo::of(&Default::default(), rtl, None) }
    }

    #[test]
    fn every_layout_lays_out_in_both_directions() {
        for layout in [QuranLayout::Ayah, QuranLayout::Quote, QuranLayout::Progress] {
            for rtl in [false, true] {
                let c = quran(&QuranWidget { layout, ..Default::default() }, &info(), &lingo(rtl), 1.0, 0.0, &mut Fixed);
                assert!(c.size.0 > 100.0 && c.size.1 > 100.0, "{layout:?} {:?}", c.size);
                assert!(!c.hits.is_empty());
                // Every click opens the Qur'an, never Jannah.
                assert!(c.hits.iter().all(|h| matches!(&h.action, Action::QuranContinue) || h.action == Action::Panel(crate::plugins::quran::ID.into())), "{layout:?}");
            }
        }
    }

    #[test]
    fn the_ring_shows_the_khatm() {
        let c = progress(&QuranWidget { layout: QuranLayout::Progress, ..Default::default() }, &info(), &lingo(false), 1.0, &mut Fixed);
        let arcs: Vec<f32> = c.marks.iter().filter_map(|m| if let MarkKind::Arc { sweep, .. } = m.kind { Some(sweep) } else { None }).collect();
        assert_eq!(arcs.len(), 2, "the track and the share");
        assert!((arcs[1] - TAU * 0.42).abs() < 1e-4);
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "42%")));
        assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, style, .. } if text == "البقرة" && style.role == FontRole::Scripture)));
        let none = QuranInfo { progress: 0.0, ..info() };
        let c = progress(&QuranWidget::default(), &none, &lingo(true), 1.0, &mut Fixed);
        assert_eq!(c.marks.iter().filter(|m| matches!(m.kind, MarkKind::Arc { .. })).count(), 1);
        assert_eq!(c.hits[0].action, Action::QuranContinue);
    }
}
