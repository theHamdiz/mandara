//! The reader: one surah at a time in a virtualized list (`gpui::list`,
//! rows of any height, only those on screen laid out). Its head is a
//! banner with the surah's name and facts, then the basmala where it
//! belongs, then the ayahs: in reading mode flowing Arabic paragraphs in
//! the mushaf's manner (each ayah closed by its ornate number ﴿n﴾ in gold),
//! in study mode each ayah with its meaning below. A tap on an ayah opens
//! its actions (play from here, bookmark, copy, copy with its meaning,
//! quran.com). While a recitation plays, a soft gold glow sits behind the
//! ayah being recited and fades across to the next one; the list follows
//! it. Scrolling keeps the last-read place; previous / next surah sit in
//! the head and at the end.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, FontWeight, HighlightStyle, InteractiveElement, IntoElement, ListAlignment, ListOffset, ListState, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window,
    canvas, div, list, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_services::quran::player::Status;
use mandara_services::quran::{Quran, Revelation, Selection};
use mandara_theme::MandaraPalette;

use super::super::extras::{Segment, caption, segmented};
use super::super::panel_fx::{alpha, elevation, glow, pill_button, wash};
use super::panel::{ACTION_BTN, CHIP_FILL, Ctx, GAP, INNER_W, NAV_BTN, Ui, dawn, hero_bg, kicker, lattice, refresh};
use super::{ARABIC_LINE_EM, Book, GLOW_MOVE, Shared, arabic_ui, ayah_mark, book, changed, copy_text, digits, gold, link, open_link, play, play_from, read_at, set_study, surah_title, toggle_bookmark};
use crate::bidi::{Bidi as _, ParagraphLayout};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, text, tokens};

/// Reading mode gathers ayahs into paragraphs of about this many
/// characters (an ayah is never split).
const BLOCK_CHARS: usize = 320;
/// Rows laid out beyond the visible area, px.
const OVERDRAW_PX: f32 = 700.0;
/// The head row and its Arabic name.
const TOP_H: f32 = 44.0;
const TOP_NAME_PX: f32 = 19.0;
const MODE_W: f32 = 168.0;
/// Reading mode's line height (em): the mushaf's open leading.
const READING_LINE_EM: f32 = 2.2;
/// The banner.
const BANNER_PAD: f32 = 18.0;
const BANNER_NAME_PX: f32 = 31.0;
const BANNER_CHIP_H: f32 = 24.0;
/// The basmala is set a little larger than the text.
const BISMILLAH_SCALE: f32 = 1.08;
/// Study rows.
const STUDY_PAD: f32 = 12.0;
const STUDY_RADIUS: f32 = 14.0;
const MEANING_LINE_EM: f32 = 1.55;
/// The glow: its fill and halo alphas `(dark, light)`, the halo's blur,
/// how far it reaches past the words, its corners.
const GLOW_FILL: (f32, f32) = (0.15, 0.2);
const GLOW_HALO: (f32, f32) = (0.3, 0.26);
const GLOW_BLUR: f32 = 16.0;
const GLOW_PAD_X: f32 = 4.0;
const GLOW_RADIUS: f32 = 9.0;
/// The tapped ayah's tint `(dark, light)`.
const SELECT_FILL: (f32, f32) = (0.12, 0.1);
/// The action sheet.
const SHEET_PAD: f32 = 12.0;
const SHEET_INSET: f32 = 6.0;
const SHEET_ELEVATION: f32 = 0.9;

/// One row of the reader.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Banner,
    Bismillah,
    /// Reading mode: ayahs `first..=last` (global numbers) as a paragraph.
    Block(u16, u16),
    /// Study mode: one ayah with its meaning.
    Ayah(u16),
    /// Next surah (or back to the beginning).
    End,
}

impl Item {
    fn globals(self) -> Option<(u16, u16)> {
        match self {
            Item::Block(a, b) => Some((a, b)),
            Item::Ayah(g) => Some((g, g)),
            _ => None,
        }
    }
}

/// The rows of surah `n`.
pub(super) fn items(q: &Quran, n: u16, study: bool) -> Vec<Item> {
    let Some(s) = q.surah(n) else { return Vec::new() };
    let mut out = vec![Item::Banner];
    if s.has_bismillah() {
        out.push(Item::Bismillah);
    }
    if study {
        out.extend(s.globals().map(Item::Ayah));
    } else {
        let mut start = s.first;
        let mut chars = 0;
        for g in s.globals() {
            let len = q.ayah_at(g).map_or(0, |a| a.text.chars().count());
            if chars > 0 && chars + len > BLOCK_CHARS {
                out.push(Item::Block(start, g - 1));
                start = g;
                chars = 0;
            }
            chars += len;
        }
        out.push(Item::Block(start, s.first + s.ayahs - 1));
    }
    out.push(Item::End);
    out
}

/// The row holding global ayah `g` (its surah's banner for its first ayah).
pub(super) fn item_of(items: &[Item], g: u16) -> Option<usize> {
    items.iter().position(|it| it.globals().is_some_and(|(a, b)| (a..=b).contains(&g)))
}

/// The first ayah row `ix` shows (the banner and basmala: the surah's
/// first ayah; the end: its last).
pub(super) fn first_ayah(items: &[Item], ix: usize) -> Option<u16> {
    match items.get(ix)? {
        Item::Banner | Item::Bismillah => items.iter().find_map(|it| it.globals()).map(|(a, _)| a),
        Item::End => items.iter().rev().find_map(|it| it.globals()).map(|(_, b)| b),
        it => it.globals().map(|(a, _)| a),
    }
}

/// What the rows were built for: surah, mode, size and the book.
type Key = (u16, bool, u32, usize);

pub(super) struct Reader {
    /// The surah shown (0 before the reader was first opened).
    pub surah: u16,
    list: ListState,
    rows: Rc<RefCell<Vec<Item>>>,
    key: Option<Key>,
    /// An ayah to scroll to once the rows are built.
    pending: Option<u16>,
    /// The tapped ayah (its actions show).
    pub selected: Option<u16>,
    /// The recited ayah last scrolled into view.
    revealed: Option<u16>,
}

impl Reader {
    pub(super) fn new(shared: &Shared) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX));
        let rows: Rc<RefCell<Vec<Item>>> = Rc::default();
        let (r, weak) = (rows.clone(), Rc::downgrade(shared));
        // Scrolling keeps your place.
        list.set_scroll_handler(move |ev, _, cx| {
            let Some(shared) = weak.upgrade() else { return };
            if let Some(g) = first_ayah(&r.borrow(), ev.visible_range.start) {
                read_at(&shared, g, cx);
            }
        });
        Self { surah: 0, list, rows, key: None, pending: None, selected: None, revealed: None }
    }

    /// Shows the surah of global ayah `g`, scrolled to it.
    pub(super) fn open(&mut self, book: &Book, g: u16) {
        if let Some(r) = book.q.at(g) {
            self.surah = r.surah;
            self.pending = Some(g);
            self.selected = None;
        }
    }

    pub(super) fn render(c: &Ctx, height: f32, window: &mut Window) -> AnyElement {
        let study = c.shared.borrow().state.study;
        let (playback, glow_at) = {
            let m = c.shared.borrow();
            (m.playback.clone(), m.glow)
        };
        let reciting = playback.current().filter(|_| playback.status != Status::Stopped);
        let (list_state, selected, surah) = {
            let mut ui = c.ui.borrow_mut();
            let r = &mut ui.reader;
            // Follow the recitation into the next surah when the reader was
            // following it.
            if let Some(g) = reciting
                && r.revealed != Some(g)
                && let Some(at) = c.book.q.at(g)
            {
                let following = r.revealed.and_then(|p| c.book.q.at(p)).is_some_and(|p| p.surah == r.surah);
                if at.surah != r.surah && following {
                    r.surah = at.surah;
                }
                if at.surah == r.surah {
                    r.pending = Some(g);
                }
                r.revealed = Some(g);
            }
            let key: Key = (r.surah, study, c.arabic_px.to_bits(), Arc::as_ptr(&c.book) as usize);
            if r.key != Some(key) {
                // Same surah in another mode or size: stay at the same ayah.
                let keep = r.key.filter(|k| k.0 == r.surah).and_then(|_| first_ayah(&r.rows.borrow(), r.list.logical_scroll_top().item_ix));
                let rows = items(&c.book.q, r.surah, study);
                r.list.reset(rows.len());
                *r.rows.borrow_mut() = rows;
                r.key = Some(key);
                if r.pending.is_none() {
                    r.pending = keep.or_else(|| c.book.q.surah(r.surah).map(|s| s.first));
                }
            }
            if let Some(g) = r.pending.take() {
                let rows = r.rows.borrow();
                let first = c.book.q.surah(r.surah).map(|s| s.first);
                let ix = if Some(g) == first { Some(0) } else { item_of(&rows, g) };
                if let Some(ix) = ix {
                    if reciting == Some(g) {
                        r.list.scroll_to_reveal_item(ix);
                    } else {
                        r.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(0.0) });
                    }
                }
            }
            (r.list.clone(), r.selected, r.surah)
        };
        // The glow fades from the ayah before to this one.
        let k = glow_at.map_or(1.0, |(_, at)| (at.elapsed().as_secs_f32() / GLOW_MOVE.as_secs_f32()).min(1.0));
        if k < 1.0 && !mandara_core::spring::motion_profile().reduce {
            window.request_animation_frame();
        } else if glow_at.is_some() {
            c.shared.borrow_mut().glow = None;
        }
        let k = if mandara_core::spring::motion_profile().reduce { 1.0 } else { k };
        let prev = glow_at.and_then(|(p, _)| p).filter(|_| k < 1.0);
        let rc = Rc::new(RowCx {
            palette: c.palette.clone(),
            book: c.book.clone(),
            shared: c.shared.clone(),
            ui: c.ui.clone(),
            scripture: c.scripture.clone(),
            arabic_px: c.arabic_px,
            radius: c.radius,
            reciting,
            paused: playback.status == Status::Paused,
            prev,
            k,
            selected,
            rows: c.ui.borrow().reader.rows.clone(),
        });
        let body = list(list_state, {
            let rc = rc.clone();
            move |ix, window, cx| row(&rc, ix, window, cx)
        })
        .w_full()
        .flex_1();
        div()
            .relative()
            .h(px(height))
            .flex()
            .flex_col()
            .gap(px(tokens::SM))
            .child(top(c, surah, study))
            .child(body)
            .when_some(selected, |el, g| el.child(sheet(c, g)))
            .into_any_element()
    }
}

/// What every row needs (rows render inside the list's own callback).
struct RowCx {
    palette: MandaraPalette,
    book: Arc<Book>,
    shared: Shared,
    ui: Rc<RefCell<Ui>>,
    scripture: SharedString,
    arabic_px: f32,
    radius: f32,
    /// The ayah being recited, and whether it is paused.
    reciting: Option<u16>,
    paused: bool,
    /// The ayah the glow is leaving, and how far it has moved (0–1).
    prev: Option<u16>,
    k: f32,
    selected: Option<u16>,
    rows: Rc<RefCell<Vec<Item>>>,
}

impl RowCx {
    /// How strongly ayah `g` glows (0–1).
    fn glow_of(&self, g: u16) -> f32 {
        let on = if self.reciting == Some(g) { self.k } else { 0.0 };
        let off = if self.prev == Some(g) { 1.0 - self.k } else { 0.0 };
        on.max(off) * if self.paused { 0.6 } else { 1.0 }
    }
}

fn select(rc: &RowCx, g: u16, cx: &mut App) {
    {
        let mut ui = rc.ui.borrow_mut();
        ui.reader.selected = if ui.reader.selected == Some(g) { None } else { Some(g) };
    }
    refresh(&rc.shared, cx);
}

fn row(rc: &Rc<RowCx>, ix: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = (window, cx);
    let item = rc.rows.borrow().get(ix).copied();
    match item {
        Some(Item::Banner) => banner(rc),
        Some(Item::Bismillah) => bismillah(rc),
        Some(Item::Block(a, b)) => block(rc, a, b),
        Some(Item::Ayah(g)) => study(rc, g),
        Some(Item::End) => end(rc),
        None => div().into_any_element(),
    }
}

// ── The head ─────────────────────────────────────────────────────────────

/// Previous / next surah around its name, the mode and play.
fn top(c: &Ctx, n: u16, study: bool) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let ink = palette.on_surface_variant;
    let Some(s) = c.book.q.surah(n) else { return div().into_any_element() };
    let go = |to: u16| {
        let (shared, ui) = (c.shared.clone(), c.ui.clone());
        move |_: &mut Window, cx: &mut App| {
            if let Some(b) = book(&shared) {
                let first = b.q.surah(to).map(|s| s.first).unwrap_or(1);
                ui.borrow_mut().reader.open(&b, first);
            }
            refresh(&shared, cx);
        }
    };
    let detail = if arabic_ui() {
        digits(tf("Surah {n} · {count} ayahs", &[("n", &n), ("count", &s.ayahs)]))
    } else {
        digits(tf("{n} · {name} · {count} ayahs", &[("n", &n), ("name", &s.transliteration), ("count", &s.ayahs)]))
    };
    let mode = {
        let seg = |label: &'static str, glyph: &'static str, to: bool| {
            let shared = c.shared.clone();
            Segment { label: t(label).into(), icon: Some(glyph), active: study == to, on_click: Rc::new(move |_, cx| set_study(&shared, to, cx)) }
        };
        div().w(px(MODE_W)).flex_none().child(segmented("quran-mode", vec![seg(tk("Reading"), "book", false), seg(tk("Study"), "note", true)], palette))
    };
    let playing_here = {
        let m = c.shared.borrow();
        m.playback.current().and_then(|x| c.book.q.at(x)).is_some_and(|r| r.surah == n) && matches!(m.playback.status, Status::Playing | Status::Loading)
    };
    let (shared, ui) = (c.shared.clone(), c.ui.clone());
    let play_btn = icon_button("quran-play-surah", if playing_here { "pause" } else { "play" }, ACTION_BTN, g, Some(g.with_alpha(alpha(CHIP_FILL, palette))), move |_, cx| {
        if playing_here {
            super::toggle_pause(&shared);
        } else if let Err(e) = play(&shared, Selection::Surah(n), cx) {
            ui.borrow().flash.show(false, e);
        }
        changed(&shared, cx);
    });
    div()
        .h(px(TOP_H))
        .flex_none()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .when(n > 1, |el| el.child(icon_button("quran-prev-surah", crate::bidi::dir_icon("chevron-left"), NAV_BTN, ink, None, go(n - 1))))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().font_family(c.scripture.clone()).text_size(px(TOP_NAME_PX)).line_height(gpui::relative(1.3)).text_color(g.hsla()).truncate().child(s.short_name().to_string()))
                .child(caption(detail, palette).font_features(tabular_nums()).truncate()),
        )
        .when(n < mandara_services::quran::text::SURAHS, |el| el.child(icon_button("quran-next-surah", crate::bidi::dir_icon("chevron-right"), NAV_BTN, ink, None, go(n + 1))))
        .child(mode)
        .child(play_btn)
        .into_any_element()
}

// ── Rows ─────────────────────────────────────────────────────────────────

/// The surah's banner: its name in calligraphy under the dawn light, what
/// it is, and play.
fn banner(rc: &RowCx) -> AnyElement {
    let palette = &rc.palette;
    let g = gold(palette);
    let n = rc.ui.borrow().reader.surah;
    let Some(s) = rc.book.q.surah(n) else { return div().into_any_element() };
    let juz = rc.book.q.ayah_at(s.first).map(|a| a.juz).unwrap_or(1);
    let chip = |label: String| {
        div()
            .h(px(BANNER_CHIP_H))
            .px(px(tokens::SM + 2.0))
            .rounded_full()
            .flex()
            .items_center()
            .bg(g.with_alpha(alpha(CHIP_FILL, palette)).hsla())
            .text_size(px(text::LABEL))
            .font_weight(FontWeight::SEMIBOLD)
            .font_features(tabular_nums())
            .text_color(g.hsla())
            .child(label)
    };
    let rev = match s.revelation {
        Revelation::Makki => t("Makki"),
        Revelation::Madani => t("Madani"),
    };
    let width = INNER_W;
    div()
        .pb(px(GAP))
        .child(
            div()
                .relative()
                .w_full()
                .rounded(px(rc.radius))
                .overflow_hidden()
                .bg(hero_bg(palette))
                .shadow(super::super::panel_fx::rim(palette))
                .child(lattice(width, palette))
                .child(dawn(width, BANNER_PAD + BANNER_NAME_PX * 0.9, palette))
                .child(
                    div()
                        .relative()
                        .p(px(BANNER_PAD))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(tokens::XS + 2.0))
                        .child(kicker(digits(tf("SURAH {n}", &[("n", &n)])), palette))
                        .child(div().font_family(rc.scripture.clone()).text_size(px(BANNER_NAME_PX)).line_height(gpui::relative(1.6)).text_color(g.hsla()).child(crate::bidi::paragraph(s.name.clone())))
                        .when(!arabic_ui(), |el| el.child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(format!("{} · {}", s.transliteration, s.meaning))))
                        .child(
                            div()
                                .pt(px(tokens::XS))
                                .row()
                                .items_center()
                                .gap(px(tokens::XS))
                                .child(chip(rev.into()))
                                .child(chip(digits(tf("{n} ayahs", &[("n", &s.ayahs)]))))
                                .child(chip(digits(tf("Juz {n}", &[("n", &juz)])))),
                        ),
                ),
        )
        .into_any_element()
}

fn bismillah(rc: &RowCx) -> AnyElement {
    div()
        .w_full()
        .pb(px(tokens::SM))
        .font_family(rc.scripture.clone())
        .text_size(px(rc.arabic_px * BISMILLAH_SCALE))
        .line_height(gpui::relative(ARABIC_LINE_EM))
        .text_center()
        .text_color(rc.palette.on_surface.hsla())
        .child(crate::bidi::paragraph(rc.book.q.bismillah.clone()))
        .into_any_element()
}

/// Rounded rectangles reaching a little past `rect`.
fn halo_rect(rect: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(gpui::point(rect.origin.x - px(GLOW_PAD_X), rect.origin.y), gpui::size(rect.size.width + px(GLOW_PAD_X * 2.0), rect.size.height))
}

/// Reading mode: ayahs as one flowing paragraph, each a link to its
/// actions, the glow and the tapped tint painted behind.
fn block(rc: &Rc<RowCx>, a: u16, b: u16) -> AnyElement {
    let palette = &rc.palette;
    let g_ = gold(palette);
    let q = &rc.book.q;
    let mut text = String::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for g in a..=b {
        let (Some(r), Some(ay)) = (q.at(g), q.ayah_at(g)) else { continue };
        let start = text.len();
        text.push_str(&ay.text);
        // A no-break space: the number stays with the last word.
        text.push('\u{00A0}');
        let mark = text.len();
        text.push_str(&ayah_mark(r.ayah));
        ranges.push(start..text.len());
        highlights.push((mark..text.len(), HighlightStyle { color: Some(g_.hsla()), ..Default::default() }));
        if g < b {
            text.push(' ');
        }
    }
    // What to paint behind: (range, glow strength, tapped).
    let mut behind: Vec<(Range<usize>, f32, bool)> = Vec::new();
    for (i, range) in ranges.iter().enumerate() {
        let g = a + i as u16;
        let k = rc.glow_of(g);
        let tapped = rc.selected == Some(g);
        if k > 0.0 || tapped {
            behind.push((range.clone(), k, tapped));
        }
    }
    let layout = ParagraphLayout::default();
    let paint_layout = layout.clone();
    let (fill, halo, tint) = (g_.with_alpha(alpha(GLOW_FILL, palette)), g_.with_alpha(alpha(GLOW_HALO, palette)), palette.primary.with_alpha(alpha(SELECT_FILL, palette)));
    let under = canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            for (range, k, tapped) in &behind {
                for rect in paint_layout.selection_rects(range.clone()) {
                    let r = halo_rect(rect);
                    if *k > 0.0 {
                        window.paint_drop_shadows(r, gpui::Corners::all(px(GLOW_RADIUS)), &[glow(halo.with_alpha(halo.alpha() * k), GLOW_BLUR, 0.0, 0.0)]);
                        window.paint_quad(gpui::fill(r, fill.with_alpha(fill.alpha() * k).hsla()).corner_radii(px(GLOW_RADIUS)));
                    }
                    if *tapped {
                        window.paint_quad(gpui::fill(r, tint.hsla()).corner_radii(px(GLOW_RADIUS)));
                    }
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full();
    let weak = Rc::downgrade(rc);
    let on_click: crate::bidi::LinkHandler = Rc::new(move |i, _, cx| {
        if let Some(rc) = weak.upgrade() {
            select(&rc, a + i as u16, cx);
        }
    });
    div()
        .relative()
        .w_full()
        .pb(px(tokens::SM))
        .child(under)
        .child(
            div()
                .w_full()
                .font_family(rc.scripture.clone())
                .text_size(px(rc.arabic_px))
                .line_height(gpui::relative(READING_LINE_EM))
                .text_color(palette.on_surface.hsla())
                .child(crate::bidi::linked_paragraph_with_layout(text, highlights, ranges, on_click, &layout)),
        )
        .into_any_element()
}

/// Study mode: one ayah, its number, its meaning below.
fn study(rc: &Rc<RowCx>, g: u16) -> AnyElement {
    let palette = &rc.palette;
    let g_ = gold(palette);
    let (Some(r), Some(ay)) = (rc.book.q.at(g), rc.book.q.ayah_at(g)) else { return div().into_any_element() };
    let k = rc.glow_of(g);
    let tapped = rc.selected == Some(g);
    let bookmarked = rc.shared.borrow().state.is_bookmarked(r);
    let meaning = rc.book.meaning_of(g).filter(|_| !arabic_ui()).map(str::to_string);
    let (fill, halo) = (g_.with_alpha(alpha(GLOW_FILL, palette) * k), g_.with_alpha(alpha(GLOW_HALO, palette) * k));
    let weak = Rc::downgrade(rc);
    let head = div()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(g_.hsla()).child(digits(r.to_string())))
        .when(ay.sajda, |el| el.child(caption(t("Prostration"), palette)))
        .child(div().flex_1())
        .when(bookmarked, |el| el.child(icon("pin", text::LABEL + 1.0, g_)));
    div()
        .pb(px(tokens::SM))
        .child(
            div()
                .id(("quran-study", usize::from(g)))
                .w_full()
                .p(px(STUDY_PAD))
                .rounded(px(STUDY_RADIUS))
                .cursor_pointer()
                .when(k > 0.0, |el| el.bg(fill.hsla()).shadow(vec![glow(halo, GLOW_BLUR, 0.0, 0.0)]))
                .when(tapped, |el| el.bg(palette.primary.with_alpha(alpha(SELECT_FILL, palette)).hsla()))
                .when(k == 0.0 && !tapped, |el| el.hover(|st| st.bg(wash((0.03, 0.035), palette).hsla())))
                .flex()
                .flex_col()
                .gap(px(tokens::XS + 2.0))
                .child(head)
                .child(
                    div()
                        .w_full()
                        .font_family(rc.scripture.clone())
                        .text_size(px(rc.arabic_px))
                        .line_height(gpui::relative(ARABIC_LINE_EM))
                        .text_color(palette.on_surface.hsla())
                        .child(crate::bidi::styled_paragraph(
                            format!("{}\u{00A0}{}", ay.text, ayah_mark(r.ayah)),
                            vec![(ay.text.len() + '\u{00A0}'.len_utf8()..ay.text.len() + '\u{00A0}'.len_utf8() + ayah_mark(r.ayah).len(), HighlightStyle { color: Some(g_.hsla()), ..Default::default() })],
                        )),
                )
                .when_some(meaning, |el, m| el.child(div().w_full().text_size(px(text::BODY)).line_height(gpui::relative(MEANING_LINE_EM)).text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(m))))
                .on_click(move |_, _, cx| {
                    if let Some(rc) = weak.upgrade() {
                        select(&rc, g, cx);
                    }
                }),
        )
        .into_any_element()
}

/// The end of a surah: the next one (or back to the beginning).
fn end(rc: &RowCx) -> AnyElement {
    let palette = &rc.palette;
    let g = gold(palette);
    let n = rc.ui.borrow().reader.surah;
    let last = n >= mandara_services::quran::text::SURAHS;
    let to = if last { 1 } else { n + 1 };
    let label = if last { tf("Back to {name}", &[("name", &surah_title(&rc.book, 1))]) } else { tf("Next: {name}", &[("name", &surah_title(&rc.book, to))]) };
    let (shared, ui) = (rc.shared.clone(), rc.ui.clone());
    div()
        .w_full()
        .py(px(GAP))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(tokens::SM))
        .when(last, |el| el.child(caption(t("The end of the Qur'an. May Allah accept your reading."), palette)))
        .child(pill_button("quran-next", Some(crate::bidi::dir_icon("chevron-right")), label, g, palette, move |_, cx| {
            if let Some(b) = book(&shared) {
                let first = b.q.surah(to).map(|s| s.first).unwrap_or(1);
                ui.borrow_mut().reader.open(&b, first);
            }
            refresh(&shared, cx);
        }))
        .into_any_element()
}

// ── The tapped ayah's actions ────────────────────────────────────────────

fn sheet(c: &Ctx, g: u16) -> AnyElement {
    let palette = c.palette;
    let gc = gold(palette);
    let Some(r) = c.book.q.at(g) else { return div().into_any_element() };
    let bookmarked = c.shared.borrow().state.is_bookmarked(r);
    let has_meaning = c.book.meaning_of(g).is_some() && !arabic_ui();
    let close = {
        let (shared, ui) = (c.shared.clone(), c.ui.clone());
        icon_button("quran-sheet-close", "close", ACTION_BTN - 4.0, palette.on_surface_variant, None, move |_, cx| {
            ui.borrow_mut().reader.selected = None;
            refresh(&shared, cx);
        })
    };
    let flash = |ui: &Rc<RefCell<Ui>>, ok: bool, msg: String| ui.borrow().flash.show(ok, msg);
    let (s1, ui1) = (c.shared.clone(), c.ui.clone());
    let play_btn = pill_button("quran-sheet-play", Some("play"), t("Play from here"), gc, palette, move |_, cx| {
        if let Err(e) = play_from(&s1, g, cx) {
            flash(&ui1, false, e);
        }
        ui1.borrow_mut().reader.selected = None;
        changed(&s1, cx);
    });
    let (s2, ui2) = (c.shared.clone(), c.ui.clone());
    let pin = pill_button("quran-sheet-pin", Some("pin"), if bookmarked { t("Bookmarked") } else { t("Bookmark") }, if bookmarked { gc } else { palette.on_surface_variant }, palette, move |_, cx| {
        let on = toggle_bookmark(&s2, r, cx);
        flash(&ui2, true, if on { t("Bookmarked").into() } else { t("Bookmark removed").into() });
    });
    let (book1, ui3) = (c.book.clone(), c.ui.clone());
    let copy = pill_button("quran-sheet-copy", Some("copy"), t("Copy"), palette.on_surface_variant, palette, move |_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text(&book1, g, false)));
        flash(&ui3, true, t("Copied with its reference").into());
    });
    let with_meaning = has_meaning.then(|| {
        let (book2, ui4) = (c.book.clone(), c.ui.clone());
        pill_button("quran-sheet-copy-meaning", Some("copy"), t("Copy with meaning"), palette.on_surface_variant, palette, move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_text(&book2, g, true)));
            flash(&ui4, true, t("Copied with its meaning").into());
        })
    });
    let web = pill_button("quran-sheet-web", Some("external"), "quran.com", palette.on_surface_variant, palette, move |_, _| open_link(link(r)));
    div()
        .absolute()
        .bottom(px(SHEET_INSET))
        .left(px(SHEET_INSET))
        .right(px(SHEET_INSET))
        .child(
            div()
                .w_full()
                .p(px(SHEET_PAD))
                .rounded(px(c.radius))
                .bg(palette.surface_container_high.hsla())
                .shadow(elevation(palette, SHEET_ELEVATION))
                .flex()
                .flex_col()
                .gap(px(tokens::SM))
                .child(div().row().items_center().gap(px(tokens::SM)).child(kicker(c.book.reference(g), palette).font_features(tabular_nums())).child(div().flex_1()).child(close))
                .child(div().row().flex_wrap().items_center().gap(px(tokens::XS + 2.0)).child(play_btn).child(pin).child(copy).when_some(with_meaning, |el, b| el.child(b)).child(web)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::quran::text::{Ayah, FORMAT, Ref, Surah, TEXT_EDITION};

    fn quran() -> Quran {
        let counts = [7u16, 20];
        let mut surahs = Vec::new();
        let mut first = 1;
        for (i, n) in counts.iter().enumerate() {
            surahs.push(Surah { number: i as u16 + 1, name: "سورة".into(), transliteration: "S".into(), meaning: "M".into(), revelation: Revelation::Makki, ayahs: *n, first });
            first += n;
        }
        // Long ayahs in the second surah, so it splits into paragraphs.
        let ayahs = (1..first).map(|g| Ayah { text: "ك".repeat(if g > 7 { 300 } else { 20 }), juz: 1, page: 1, hizb_quarter: 1, sajda: false }).collect();
        Quran { format: FORMAT, edition: TEXT_EDITION.into(), bismillah: "ب".into(), surahs, ayahs, juz_starts: vec![Ref::new(1, 1)] }
    }

    #[test]
    fn rows_cover_every_ayah_once() {
        let q = quran();
        for study in [false, true] {
            for n in 1..=2u16 {
                let rows = items(&q, n, study);
                let s = q.surah(n).unwrap();
                assert_eq!(rows[0], Item::Banner);
                assert_eq!(rows.contains(&Item::Bismillah), n != 1, "the opening's basmala is its first ayah");
                assert_eq!(*rows.last().unwrap(), Item::End);
                let covered: Vec<u16> = rows.iter().filter_map(|r| r.globals()).flat_map(|(a, b)| a..=b).collect();
                assert_eq!(covered, s.globals().collect::<Vec<_>>());
            }
        }
        // Reading mode gathers short ayahs and splits long runs.
        assert_eq!(items(&q, 1, false).iter().filter(|r| matches!(r, Item::Block(..))).count(), 1);
        let long = items(&q, 2, false);
        assert!(long.iter().filter(|r| matches!(r, Item::Block(..))).count() >= 10);
        assert!(long.iter().all(|r| !matches!(r, Item::Block(a, b) if a != b && (b - a + 1) as usize * 300 > BLOCK_CHARS)));
    }

    #[test]
    fn rows_find_their_ayahs() {
        let q = quran();
        let rows = items(&q, 2, true);
        assert_eq!(item_of(&rows, 8), Some(2));
        assert_eq!(item_of(&rows, 27), Some(21));
        assert_eq!(item_of(&rows, 1), None);
        assert_eq!(first_ayah(&rows, 0), Some(8));
        assert_eq!(first_ayah(&rows, 5), Some(11));
        assert_eq!(first_ayah(&rows, rows.len() - 1), Some(27));
        assert_eq!(first_ayah(&rows, 99), None);
    }
}
