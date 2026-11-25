//! The Hadith plugin's desktop widgets, from [`super::HadithInfo`]:
//!
//! * `hadith`: the hadith of the day in the Quote widget's design family
//!   (Jannah's quote layout): a small caps label with the grade in its
//!   colour, the guillemet, the words in Amiri right to left (long hadith
//!   end after a whole word with an ellipsis and "Read more"), the meaning, the
//!   narrator and the reference as the attribution;
//! * `collection`: a tile with your reading progress through a chosen
//!   collection (its names, the share read as a bar, where you were).
//!
//! A click on today's hadith opens it in the Hadith reader (the tile opens the
//! panel). Redrawn only when the card or the progress changes.

use mandara_config::desktop_widgets::{FontRole, HadithLayout, HadithWidget};
use mandara_config::widget_kinds::{ranges, within};
use mandara_i18n::{fill, tk};

use super::HadithInfo;
use super::actions::Action;
use super::jannah::{arabic_align, arabic_block, open_action, read_more, text_block, text_block_cut};
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, caps, face, style, tracked};
use super::lingo::Lingo;
use super::productive::{AUTHOR_TRACKING, QUOTE_AUTHOR, QUOTE_GAP, QUOTE_MARK};

/// The Arabic is set this much larger than the text size, the meaning and
/// the label smaller.
const ARABIC_SCALE: f32 = 1.15;
const MEANING_SCALE: f32 = 0.6;
const LABEL_SCALE: f32 = 0.42;
/// Most lines of the meaning.
const MEANING_LINES: usize = 5;
/// The guillemet's size (of the quote mark's) and the Arabic's start
/// below its top (of its height).
const MARK_SCALE: f32 = 0.6;
const MARK_DROP: f32 = 1.0;
const LABEL_TRACKING: f32 = 0.14;
/// Progress tile: width, sizes, the bar and gaps.
const TILE_W: f32 = 320.0;
const TILE_TITLE: f32 = 24.0;
const TILE_ARABIC: f32 = 28.0;
const TILE_LABEL: f32 = 12.0;
const TILE_LINE: f32 = 15.0;
const TILE_PERCENT: f32 = 40.0;
const TILE_GAP: f32 = 10.0;
const BAR_H: f32 = 8.0;
const BAR_TRACK_ALPHA: f32 = 0.18;
const PERCENT: f32 = 100.0;

/// Lays out the Hadith widget `w`.
pub fn hadith(w: &HadithWidget, info: &HadithInfo, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Option<Composition> {
    match w.layout {
        HadithLayout::Hadith => today(w, info, l, k, align, m),
        HadithLayout::Collection => Some(progress(w, info, l, k, m)),
    }
}

/// The hadith of the day.
fn today(w: &HadithWidget, info: &HadithInfo, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Option<Composition> {
    let card = info.today.as_ref()?;
    let px = |v: f32| v * k;
    let width = px(within(w.width, ranges::WIDTH_PX, ranges::QUOTE_WIDTH_DEFAULT_PX));
    let size = px(within(w.text_size, ranges::TEXT_PX, ranges::QUOTE_TEXT_DEFAULT_PX));
    let ar_align = arabic_align(align);
    let mut marks = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0)];
    // The label, and the grade in its colour at the other edge.
    let ls = tracked(style(w.look.font, size * LABEL_SCALE, face::BOLD), LABEL_TRACKING);
    let label = caps(&l.word(tk("Hadith of the day")));
    let (lw, lh) = m.measure(&label, &ls);
    let start = |wd: f32| if l.rtl { width - wd } else { 0.0 };
    let end = |wd: f32| if l.rtl { 0.0 } else { width - wd };
    marks.push(Mark::new(MarkKind::Text { text: label, style: ls.clone(), x: start(lw), y: 0.0, w: lw, h: lh }, InkRole::Accent));
    if let Some((_, words, rgb)) = &card.grade {
        let g = caps(&if words.is_empty() { l.word(tk("Sahih")) } else { words.clone() });
        let (gw, gh) = m.measure(&g, &ls);
        marks.push(Mark::new(MarkKind::Text { text: g, style: ls, x: end(gw), y: 0.0, w: gw, h: gh }, InkRole::Rgb(*rgb)));
    }
    let mut y = lh + px(QUOTE_GAP) / 2.0;
    // The guillemet at the Arabic's start.
    let ms = style(FontRole::Hadith, size * QUOTE_MARK * MARK_SCALE, face::REGULAR);
    let (gw, gh) = m.measure("«", &ms);
    marks.push(Mark::new(MarkKind::Text { text: "«".into(), style: ms, x: (width - gw) * ar_align, y, w: gw, h: gh }, InkRole::Accent));
    y += gh * MARK_DROP;
    let ast = style(FontRole::Hadith, size * ARABIC_SCALE, face::REGULAR);
    let (mut y, mut cut) = arabic_block(&card.words, &ast, width, ar_align, y, &mut marks, m);
    if let Some(meaning) = card.meaning.as_deref().filter(|t| !t.trim().is_empty()) {
        y += px(QUOTE_GAP);
        let ms = style(w.look.font, size * MEANING_SCALE, face::REGULAR);
        if let Some(n) = &card.narrator {
            let ns = style(w.look.font, size * MEANING_SCALE * 0.8, face::SEMIBOLD);
            y = text_block(&fill(&l.word(tk("Narrated by {narrator}")), &[("narrator", n)]), &ns, width, align, 1, InkRole::Accent, face::DIM, y, &mut marks, m);
        }
        let meaning_cut;
        (y, meaning_cut) = text_block_cut(meaning, &ms, width, align, MEANING_LINES, InkRole::Fg, face::DIM, y, &mut marks, m);
        cut |= meaning_cut;
        if card.english {
            let es = style(w.look.font, size * MEANING_SCALE * 0.75, face::MEDIUM);
            y = text_block(&l.word(tk("Meaning in English")), &es, width, align, 1, InkRole::Accent, face::DIM, y, &mut marks, m);
        }
    }
    y += px(QUOTE_GAP);
    let a = format!("— {}", if l.arabic() { card.reference_ar.clone() } else { l.digits(card.reference.clone()) });
    let st = tracked(style(w.look.font, size * QUOTE_AUTHOR, face::SEMIBOLD), AUTHOR_TRACKING);
    let (aw, ah) = m.measure(&a, &st);
    marks.push(Mark::new(MarkKind::Text { text: a, style: st.clone(), x: (width - aw) * align, y, w: aw, h: ah }, InkRole::Accent).dim(face::DIM));
    y += ah;
    if cut {
        y = read_more(&st, width, align, y, l, &mut marks, m);
    }
    Some(Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, width, y), action: open_action(crate::plugins::hadith::ID, &card.open) }]))
}

/// Reading progress through one collection.
fn progress(w: &HadithWidget, info: &HadithInfo, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let width = px(TILE_W);
    let align = if l.rtl { 1.0 } else { 0.0 };
    let p = info.progress.iter().find(|p| p.collection == w.collection);
    let mut marks = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0)];
    let ks = tracked(style(w.look.font, px(TILE_LABEL), face::BOLD), LABEL_TRACKING);
    let mut y = text_block(&caps(&l.word(tk("Reading progress"))), &ks, width, align, 1, InkRole::Accent, 1.0, 0.0, &mut marks, m);
    y += px(TILE_GAP) / 2.0;
    let Some(p) = p else { return Composition::with_hits(marks, Vec::new()) };
    y = text_block(&p.arabic, &style(FontRole::Hadith, px(TILE_ARABIC), face::REGULAR), width, if l.rtl { 1.0 } else { 0.0 }, 1, InkRole::Accent, 1.0, y, &mut marks, m);
    if !l.arabic() {
        y = text_block(&l.word(&p.title), &style(w.look.font, px(TILE_TITLE), face::SEMIBOLD), width, align, 2, InkRole::Fg, 1.0, y, &mut marks, m);
    }
    y += px(TILE_GAP);
    let frac = if p.total > 0 { (p.read as f32 / p.total as f32).clamp(0.0, 1.0) } else { 0.0 };
    let pct = l.digits(format!("{}%", (frac * PERCENT).round() as u32));
    y = text_block(&pct, &style(w.look.font, px(TILE_PERCENT), face::SEMIBOLD), width, align, 1, InkRole::Fg, 1.0, y, &mut marks, m);
    y += px(TILE_GAP) / 2.0;
    // The bar: a track and the share read, from the reading start.
    let bar_h = px(BAR_H);
    marks.push(Mark::new(MarkKind::Rect { x: 0.0, y, w: width, h: bar_h, radius: bar_h / 2.0, width: 0.0 }, InkRole::Fg).dim(BAR_TRACK_ALPHA));
    if frac > 0.0 {
        let fw = (width * frac).max(bar_h);
        marks.push(Mark::new(MarkKind::Rect { x: if l.rtl { width - fw } else { 0.0 }, y, w: fw, h: bar_h, radius: bar_h / 2.0, width: 0.0 }, InkRole::Accent));
    }
    y += bar_h + px(TILE_GAP);
    let line = if !p.downloaded {
        l.word(tk("Not downloaded yet: open the Hadith panel"))
    } else {
        let read = fill(&l.word(tk("{read} of {total} read")), &[("read", &l.digits(p.read.to_string())), ("total", &l.digits(p.total.to_string()))]);
        match &p.last {
            Some(at) => format!("{read} · {}", l.digits(at.clone())),
            None => read,
        }
    };
    let ls = style(w.look.font, px(TILE_LINE), face::REGULAR);
    let y_end = text_block(&line, &ls, width, align, 2, InkRole::Fg, face::DIM, y, &mut marks, m);
    Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, width, y_end), action: Action::Panel(crate::plugins::hadith::ID.into()) }])
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::super::{HadithCard, HadithProgress};
    use super::*;
    use mandara_config::widget_kinds::HadithCollection;
    use mandara_services::hadith::GradeClass;

    fn info(words: String) -> HadithInfo {
        HadithInfo {
            today: Some(HadithCard {
                words,
                meaning: Some("A meaning in the reader's language, long enough to wrap onto a second line.".into()),
                english: true,
                narrator: Some("Someone".into()),
                reference: "Sahih al-Bukhari 1".into(),
                reference_ar: "صحيح البخاري ١".into(),
                grade: Some((GradeClass::Sahih, String::new(), 0x2f_b5_7e)),
                open: "bukhari:1".into(),
            }),
            progress: vec![HadithProgress { collection: HadithCollection::Muslim, title: "Sahih Muslim".into(), arabic: "صحيح مسلم".into(), read: 368, total: 7360, downloaded: true, last: Some("Muslim 8a".into()) }],
            lang: None,
        }
    }

    fn lingo(rtl: bool) -> Lingo {
        Lingo { rtl, ..Lingo::of(&Default::default(), rtl, None) }
    }

    fn texts(c: &Composition) -> Vec<String> {
        c.marks.iter().filter_map(|m| if let MarkKind::Text { text, .. } = &m.kind { Some(text.clone()) } else { None }).collect()
    }

    #[test]
    fn both_layouts_lay_out_in_both_directions() {
        for layout in [HadithLayout::Hadith, HadithLayout::Collection] {
            for rtl in [false, true] {
                let w = HadithWidget { layout, collection: HadithCollection::Muslim, ..Default::default() };
                let c = hadith(&w, &info("كَلِمَاتٌ طَيِّبَةٌ".into()), &lingo(rtl), 1.0, 0.0, &mut Fixed).unwrap();
                assert!(c.size.0 > 100.0 && c.size.1 > 50.0, "{layout:?} {:?}", c.size);
                let want = if layout == HadithLayout::Hadith { Action::Read("hadith".into(), "bukhari:1".into()) } else { Action::Panel("hadith".into()) };
                assert_eq!(c.hits.first().map(|h| h.action.clone()), Some(want), "today's hadith opens in the reader");
            }
        }
        // No hadith picked yet: nothing drawn.
        let none = HadithInfo { today: None, progress: Vec::new(), lang: None };
        assert!(hadith(&HadithWidget::default(), &none, &lingo(false), 1.0, 0.0, &mut Fixed).is_none());
    }

    #[test]
    fn long_hadith_end_gracefully() {
        let w = HadithWidget::default();
        let short = hadith(&w, &info("كَلِمَاتٌ طَيِّبَةٌ".into()), &lingo(false), 1.0, 0.0, &mut Fixed).unwrap();
        assert!(!texts(&short).iter().any(|t| t.contains("Read more")));
        let long = hadith(&w, &info("كَلِمَاتٌ عَرَبِيَّةٌ طَوِيلَةٌ ".repeat(60)), &lingo(false), 1.0, 0.0, &mut Fixed).unwrap();
        let t = texts(&long);
        assert!(t.iter().any(|x| x.ends_with('…')) && t.iter().any(|x| x.contains("Read more")));
        assert!(t.iter().any(|x| x == "— Sahih al-Bukhari 1"));
        assert!(long.marks.iter().any(|m| m.ink == InkRole::Rgb(0x2f_b5_7e)), "the grade in its colour");
    }

    #[test]
    fn the_tile_shows_the_share_read() {
        let w = HadithWidget { layout: HadithLayout::Collection, collection: HadithCollection::Muslim, ..Default::default() };
        let c = progress(&w, &info(String::new()), &lingo(false), 1.0, &mut Fixed);
        assert!(texts(&c).iter().any(|t| t == "5%"));
        assert!(c.marks.iter().any(|m| matches!(m.kind, MarkKind::Rect { w, .. } if w > 0.0 && w < TILE_W) && m.ink == InkRole::Accent));
    }
}
