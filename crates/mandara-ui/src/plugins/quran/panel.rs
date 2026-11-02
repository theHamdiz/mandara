//! The Qur'an panel: a header (with the recitation's controls while one
//! plays), the tabs (Home · Surahs · Read · Saved · Search) and a body that
//! springs to its content. Until the content is downloaded the body is the
//! download card (a progress ring with MB, what is being fetched and from
//! where, Retry on failure, the offline note). Home holds "continue
//! reading" with the khatm ring, the ayah of the day under Jannah's arch
//! and the juz grid; Surahs and Search are virtualized lists (search runs
//! off the UI thread); the reader is in [`super::reader`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Size, StatefulInteractiveElement, Styled, Window,
    canvas, div, prelude::FluentBuilder as _, px, size, svg, uniform_list,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{PanelCx, PanelSpec};
use mandara_services::quran::player::Status;
use mandara_services::quran::search::{self, Hits};
use mandara_services::quran::store::{self, Stage};
use mandara_services::quran::text::{AYAHS, JUZS, SURAHS};
use mandara_services::quran::{Ref, Revelation};
use mandara_theme::{Color, MandaraPalette};

use super::super::extras::{CONTROL_H, Flash, Segment, caption, search_box, segmented, toast};
use super::super::fn_view;
use super::super::jannah::art as jart;
use super::super::jannah::panel::{ARCH_ALPHA, ARCH_INNER_SHARE, ARCH_INSET, DAWN_GLOW, DAWN_H, DAWN_TOP_SHARE, DAWN_W, HERO_ANGLE_DEG, HERO_DARK, HERO_LIGHT, HERO_PRIMARY_SHARE, LATTICE_ALPHA, tone};
use super::super::panel_fx::{HEADER_H, PanelShape, WASH_SOFT, alpha, empty_state, glass_card, glow, header, pill_button, readable, rim, section, wash};
use super::reader::Reader;
use super::{ARABIC_LINE_EM, Book, Data, Open, Shared, arabic_ui, ayah_mark, book, changed, continue_at, copy_text, digits, ensure, gold, model, play, play_from, settings, toggle_bookmark};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;

// ── Layout ───────────────────────────────────────────────────────────────

pub(super) const PANEL_W: f32 = 480.0;
pub(super) const PAD: f32 = 14.0;
pub(super) const GAP: f32 = 12.0;
/// Everything above the body: padding, header, tabs and their gaps.
const CHROME_H: f32 = PAD * 2.0 + HEADER_H + GAP + CONTROL_H + GAP;
/// The body springs to its content between these heights; the list tabs
/// (Surahs, Read, Search) are this tall.
const BODY_MIN: f32 = 180.0;
const BODY_MAX: f32 = 600.0;
pub(super) const LIST_H: f32 = 580.0;
/// The body's first height on Home, before it is measured.
const BODY_GUESS: f32 = 560.0;
/// The download card's height.
const DOWNLOAD_H: f32 = 380.0;
/// Panel height changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;
/// Inner width of the body.
pub(super) const INNER_W: f32 = PANEL_W - PAD * 2.0;

// Hero ("continue reading").
const HERO_PAD: f32 = 18.0;
const HERO_NAME_PX: f32 = 32.0;
const RING_PX: f32 = 88.0;
const RING_STROKE: f32 = 6.0;
const RING_TRACK: (f32, f32) = (0.16, 0.2);
const LABEL_TRACKING_GAP: f32 = 2.0;
/// The download ring.
const DOWNLOAD_RING_PX: f32 = 118.0;
const DOWNLOAD_RING_STROKE: f32 = 7.0;
const MB: f64 = 1_000_000.0;

// The ayah of the day under the arch.
const TODAY_ARABIC_PX: f32 = 24.0;
const CONTENT_TOP_SHARE: f32 = 0.55;
const CONTENT_SIDE: f32 = 16.0;
const CONTENT_BOTTOM: f32 = 18.0;
const BLOCK_GAP: f32 = 10.0;
const MEANING_LINE_EM: f32 = 1.5;
pub(super) const CHIP_H: f32 = 26.0;
pub(super) const CHIP_PAD_X: f32 = 10.0;
pub(super) const CHIP_FILL: (f32, f32) = (0.16, 0.14);
pub(super) const ACTION_BTN: f32 = 30.0;
pub(super) const NAV_BTN: f32 = 28.0;

// Juz grid.
const JUZ_COLS: usize = 6;
const JUZ_TILE_H: f32 = 56.0;
const JUZ_GAP: f32 = 6.0;
const JUZ_WORD_PX: f32 = 13.0;
const JUZ_RADIUS: f32 = 12.0;
const JUZ_DONE: (f32, f32) = (0.1, 0.12);
const JUZ_HERE: (f32, f32) = (0.22, 0.24);

// Lists.
pub(super) const ROW_H: f32 = 62.0;
pub(super) const ROW_PAD_X: f32 = 10.0;
pub(super) const ROW_RADIUS: f32 = 12.0;
pub(super) const ROW_HOVER: (f32, f32) = (0.045, 0.04);
const MEDALLION_PX: f32 = 38.0;
const MEDALLION_ALPHA: f32 = 0.75;
const SURAH_ARABIC_PX: f32 = 21.0;
const LIST_ARABIC_PX: f32 = 17.0;
/// One list line of Arabic: tall enough for its marks above and below.
const LIST_ARABIC_LINE_PX: f32 = LIST_ARABIC_PX * 1.75;
const CHIPS_H: f32 = 28.0;
const FILTER_CHIP_PAD: f32 = 12.0;
const CHIP_GAP: f32 = 6.0;
/// Transport buttons in the header.
const TRANSPORT_BTN: f32 = 30.0;

// ── State ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Tab {
    Home,
    Surahs,
    Reader,
    Saved,
    Search,
}

pub(super) struct Ui {
    pub tab: Tab,
    pub reader: Reader,
    /// The surah list's filter.
    revelation: Option<Revelation>,
    /// The last search: its query and results (`None` while it runs).
    searched: Option<(String, Option<Rc<Hits>>)>,
    search_gen: u64,
    pub flash: Flash,
    /// Peeking (a look only, never saved, your place never moves): the
    /// ayah of the day this many days from today's, and the ayah this many
    /// after your place.
    day_peek: i64,
    next_peek: i64,
}

struct View {
    p: PanelCx,
    surah_field: Entity<TextField>,
    search_field: Entity<TextField>,
    ui: Rc<RefCell<Ui>>,
    size: Rc<Cell<Size<Pixels>>>,
}

/// Every surface's context for one render.
pub(super) struct Ctx<'a> {
    pub palette: &'a MandaraPalette,
    pub shared: &'a Shared,
    pub ui: &'a Rc<RefCell<Ui>>,
    pub book: Arc<Book>,
    pub scripture: SharedString,
    pub radius: f32,
    pub arabic_px: f32,
}

pub(super) fn build(p: PanelCx, window: &mut Window, cx: &mut App) -> PanelSpec {
    let shared = model(cx);
    let views = shared.borrow().views.clone();
    let field = |placeholder: &'static str, cx: &mut App| {
        let views = views.clone();
        cx.new(|cx| TextField::new(String::new(), t(placeholder), true, Rc::new(move |_, _, cx| views.notify(cx)), cx))
    };
    let surah_field = field(tk("Find a surah by name or number…"), cx);
    let search_field = field(tk("Search Arabic or meaning…"), cx);
    let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(CHROME_H + BODY_GUESS))));
    let ui = Rc::new(RefCell::new(Ui { tab: Tab::Home, reader: Reader::new(&shared), revelation: None, searched: None, search_gen: 0, flash: Flash::default(), day_peek: 0, next_peek: 0 }));
    let view = View { p, surah_field, search_field, ui, size: size_cell.clone() };
    PanelSpec { view: fn_view(None, super::plugin_lang::in_language(super::own_lang, move |palette, window, cx| view.render(palette, window, cx)), window, cx), size: size_cell }
}

/// Re-renders every Qur'an view (the panel among them).
pub(super) fn refresh(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

/// The hero backdrop: Jannah's emerald → teal, tinted by the theme.
pub(super) fn hero_bg(palette: &MandaraPalette) -> gpui::Background {
    let tones = if palette.is_dark { HERO_DARK } else { HERO_LIGHT };
    let emerald = palette.success_emerald.mix(palette.primary, HERO_PRIMARY_SHARE);
    oklab_gradient(HERO_ANGLE_DEG, tone(emerald, tones[0]), tone(palette.tertiary.mix(emerald, HERO_PRIMARY_SHARE), tones[1]))
}

/// The faint star lattice laid over a hero, `width` wide.
pub(super) fn lattice(width: f32, palette: &MandaraPalette) -> impl IntoElement {
    svg().path(jart::LATTICE_PATH).absolute().top_0().left_0().size(px(width)).text_color(gold(palette).with_alpha(alpha(LATTICE_ALPHA, palette)).hsla())
}

/// Dawn light: a soft gold glow `top` px down, centred in `width`.
pub(super) fn dawn(width: f32, top: f32, palette: &MandaraPalette) -> impl IntoElement {
    div()
        .absolute()
        .top(px(top - DAWN_H / 2.0))
        .left(px((width - DAWN_W) / 2.0))
        .w(px(DAWN_W))
        .h(px(DAWN_H))
        .rounded_full()
        .shadow(vec![glow(gold(palette).with_alpha(DAWN_GLOW.0), DAWN_GLOW.1, DAWN_GLOW.2, 0.0)])
}

/// A small gold label in capitals.
pub(super) fn kicker(s: impl Into<SharedString>, palette: &MandaraPalette) -> gpui::Div {
    div().text_size(px(text::LABEL)).font_weight(FontWeight::BOLD).text_color(gold(palette).hsla()).child(s.into())
}

/// The reference chip (gold): a click copies the citation.
pub(super) fn ref_chip(id: impl Into<gpui::ElementId>, book: &Book, g: u16, ui: &Rc<RefCell<Ui>>, palette: &MandaraPalette) -> AnyElement {
    let g_ = gold(palette);
    let Some(r) = book.q.at(g) else { return div().into_any_element() };
    let citation = format!("{} · {}", book.reference(g), super::link(r));
    let ui = ui.clone();
    div()
        .id(id)
        .h(px(CHIP_H))
        .px(px(CHIP_PAD_X))
        .min_w(px(0.0))
        .flex_none()
        .rounded_full()
        .cursor_pointer()
        .bg(g_.with_alpha(alpha(CHIP_FILL, palette)).hsla())
        .row()
        .items_center()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular_nums())
        .text_color(g_.hsla())
        .child(div().truncate().child(book.reference(g)))
        .on_click({
            // Translated now, in the plugin's language (the click comes later).
            let cited = t("Citation copied");
            move |_, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(citation.clone()));
                ui.borrow_mut().flash.show(true, cited);
            }
        })
        .into_any_element()
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let shared = model(cx);
        shared.borrow().views.track(window);
        ensure(&shared, cx);
        let s = settings(cx);
        let book = book(&shared);
        if let (Some(Open::Reader(g)), Some(b)) = (shared.borrow().open_request, book.as_ref()) {
            let mut ui = self.ui.borrow_mut();
            ui.tab = Tab::Reader;
            ui.reader.open(b, g);
        }
        if book.is_some() {
            shared.borrow_mut().open_request = None;
        }
        // A recitation error shows once.
        if let Some(e) = shared.borrow_mut().play_error.take() {
            self.ui.borrow().flash.show(false, e);
        }
        let accent = gold(palette);
        let trailing = transport(&shared, palette).or_else(|| meaning_retry(&shared, palette));
        let head = header("book", accent, t("Qur'an"), subtitle(&shared, book.as_deref()), palette, trailing);
        let Some(book) = book else {
            let body = download_card(&shared, palette, PanelShape::of(cx).inner(PAD));
            self.size.set(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + DOWNLOAD_H)));
            return div().relative().size_full().text_start().p(px(PAD)).flex().flex_col().child(reveal(&self.p, 0, head)).child(div().h(px(GAP))).child(reveal(&self.p, 1, body)).into_any_element();
        };
        let f = fonts(cx);
        let c = Ctx { palette, shared: &shared, ui: &self.ui, book, scripture: f.scripture.clone(), radius: PanelShape::of(cx).inner(PAD), arabic_px: s.arabic_size.reader_px() };
        let tab = self.ui.borrow().tab;
        let tabs = {
            let seg = |label: &'static str, glyph: &'static str, which: Tab| {
                let (ui, shared) = (self.ui.clone(), shared.clone());
                Segment {
                    label: t(label).into(),
                    icon: Some(glyph),
                    active: tab == which,
                    on_click: Rc::new(move |_, cx| {
                        let mut u = ui.borrow_mut();
                        if which == Tab::Reader
                            && u.reader.surah == 0
                            && let Some(b) = super::book(&shared)
                        {
                            u.reader.open(&b, continue_at(&shared));
                        }
                        u.tab = which;
                        drop(u);
                        refresh(&shared, cx);
                    }),
                }
            };
            segmented(
                "quran-tabs",
                vec![seg(tk("Home"), "home", Tab::Home), seg(tk("Surahs"), "list-check", Tab::Surahs), seg(tk("Read"), "book", Tab::Reader), seg(tk("Saved"), "pin", Tab::Saved), seg(tk("Search"), "search", Tab::Search)],
                palette,
            )
        };
        let fixed = matches!(tab, Tab::Surahs | Tab::Reader | Tab::Search);
        let body: AnyElement = match tab {
            Tab::Home => home(&c),
            Tab::Surahs => self.surahs(&c, cx),
            Tab::Reader => {
                let mut ui = self.ui.borrow_mut();
                if ui.reader.surah == 0 {
                    let g = continue_at(&shared);
                    ui.reader.open(&c.book, g);
                }
                drop(ui);
                Reader::render(&c, LIST_H, window)
            }
            Tab::Saved => saved(&c),
            Tab::Search => self.search(&c, cx),
        };
        // The body measures itself; the panel springs to it.
        let size_cell = self.size.clone();
        let measure = canvas(
            move |bounds, _, _| {
                let content = f32::from(bounds.size.height);
                let body = if fixed { LIST_H } else { content.clamp(BODY_MIN, BODY_MAX) };
                let height = CHROME_H + body;
                if (f32::from(size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
                    size_cell.set(size(px(PANEL_W), px(height)));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let flash = self.ui.borrow().flash.current(window, cx);
        let p = &self.p;
        let body = if fixed {
            div().flex_1().min_h(px(0.0)).child(reveal(p, 2, div().relative().h(px(LIST_H)).child(body).child(measure))).into_any_element()
        } else {
            div().id("quran-body").flex_1().min_h(px(0.0)).overflow_y_scroll().child(reveal(p, 2, div().relative().flex_none().flex().flex_col().child(body).child(measure))).into_any_element()
        };
        div()
            .relative()
            .size_full()
            .text_start()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(p, 1, tabs))
            .child(div().h(px(GAP)))
            .child(body)
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }

    /// The surah list: a filter field, Makki / Madani chips and the list.
    fn surahs(&self, c: &Ctx, cx: &mut App) -> AnyElement {
        let palette = c.palette;
        let query = self.surah_field.read(cx).value.clone();
        let rev = self.ui.borrow().revelation;
        let fq = mandara_services::jannah::search::fold(query.trim()).replace('-', " ");
        let found = (!fq.is_empty()).then(|| c.book.q.find_surah(&query)).flatten();
        let matches = |n: u16| {
            let Some(s) = c.book.q.surah(n) else { return false };
            let text_ok = fq.is_empty()
                || n.to_string() == fq
                || [s.transliteration.as_str(), s.meaning.as_str(), s.short_name()].iter().any(|k| mandara_services::jannah::search::fold(k).replace('-', " ").contains(&fq))
                || found == Some(n);
            text_ok && rev.is_none_or(|r| s.revelation == r)
        };
        let list: Rc<[u16]> = (1..=SURAHS).filter(|n| matches(*n)).collect();
        let chip = |id: &'static str, label: &'static str, which: Option<Revelation>| {
            let (ui, shared) = (c.ui.clone(), c.shared.clone());
            filter_chip(id, t(label), rev == which, palette, move |cx| {
                ui.borrow_mut().revelation = which;
                refresh(&shared, cx);
            })
        };
        let chips = div()
            .row()
            .items_center()
            .gap(px(CHIP_GAP))
            .child(chip("quran-rev-all", tk("All"), None))
            .child(chip("quran-rev-makki", tk("Makki"), Some(Revelation::Makki)))
            .child(chip("quran-rev-madani", tk("Madani"), Some(Revelation::Madani)))
            .child(div().flex_1())
            .child(caption(digits(tf("{n} surahs", &[("n", &list.len())])), palette).font_features(tabular_nums()));
        let list_h = LIST_H - CONTROL_H - CHIPS_H - GAP * 2.0;
        let body: AnyElement = if list.is_empty() {
            empty_state("search", t("No surah matches"), t("Try a number, a name or its meaning"), list_h, palette).into_any_element()
        } else {
            let (palette, shared, ui, book, scripture) = (palette.clone(), c.shared.clone(), c.ui.clone(), c.book.clone(), c.scripture.clone());
            let here = c.book.q.at(continue_at(c.shared)).map(|r| r.surah);
            // Rows are built at layout, outside the render's language scope.
            let lang = mandara_i18n::language();
            uniform_list("quran-surahs", list.len(), move |range, _, _| {
                let _lang = mandara_i18n::scoped(Some(lang));
                range.filter_map(|i| list.get(i).map(|n| surah_row(*n, &book, here == Some(*n), &palette, &scripture, &shared, &ui))).collect::<Vec<_>>()
            })
                .h(px(list_h))
                .w_full()
                .into_any_element()
        };
        div().flex().flex_col().gap(px(GAP)).child(div().flex().child(search_box("quran-surah-clear", &self.surah_field, &query, palette))).child(chips).child(body).into_any_element()
    }

    /// Search over the whole Qur'an.
    fn search(&self, c: &Ctx, cx: &mut App) -> AnyElement {
        let palette = c.palette;
        let query = self.search_field.read(cx).value.clone();
        let hits = self.run_search(c, query.trim(), cx);
        let pending = !query.trim().is_empty() && hits.is_none();
        let list_h = LIST_H - CONTROL_H - CHIPS_H - GAP * 2.0;
        let count: AnyElement = match &hits {
            Some(h) if h.total > h.ayahs.len() => caption(digits(tf("{n} ayahs · showing the first {shown}", &[("n", &h.total), ("shown", &h.ayahs.len())])), palette).into_any_element(),
            Some(h) => caption(digits(tf("{n} ayahs", &[("n", &h.total)])), palette).into_any_element(),
            None if pending => caption(t("Searching…"), palette).into_any_element(),
            None => caption(t("Arabic without diacritics, words of the meaning, or 2:255"), palette).into_any_element(),
        };
        let body: AnyElement = match hits {
            None if !pending => empty_state("search", t("Search the whole Qur'an"), t("Type a word in Arabic or in your language"), list_h, palette).into_any_element(),
            None => div().h(px(list_h)).into_any_element(),
            Some(h) if h.ayahs.is_empty() => empty_state("search", t("Nothing matches"), t("Try fewer or other words"), list_h, palette).into_any_element(),
            Some(h) => {
                let (palette, shared, ui, book, scripture) = (palette.clone(), c.shared.clone(), c.ui.clone(), c.book.clone(), c.scripture.clone());
                let lang = mandara_i18n::language();
                uniform_list("quran-search", h.ayahs.len(), move |range, _, _| {
                    let _lang = mandara_i18n::scoped(Some(lang));
                    range
                        .filter_map(|i| h.ayahs.get(i).map(|g| (i, *g)))
                        .map(|(i, g)| {
                            let (shared2, ui2) = (shared.clone(), ui.clone());
                            ayah_row(("quran-hit", i), g, &book, &palette, &scripture, None, move |cx| open_reader(&shared2, &ui2, g, cx))
                        })
                        .collect::<Vec<_>>()
                })
                .h(px(list_h))
                .w_full()
                .into_any_element()
            }
        };
        div().flex().flex_col().gap(px(GAP)).child(div().flex().child(search_box("quran-search-clear", &self.search_field, &query, palette))).child(div().h(px(CHIPS_H)).row().items_center().child(count)).child(body).into_any_element()
    }

    /// The results of `query` (`None`: no query, or still searching),
    /// starting the search off the UI thread when the query changed.
    fn run_search(&self, c: &Ctx, query: &str, cx: &mut App) -> Option<Rc<Hits>> {
        if query.is_empty() {
            self.ui.borrow_mut().searched = None;
            return None;
        }
        if let Some((q, res)) = &self.ui.borrow().searched
            && q == query
        {
            return res.clone();
        }
        let generation = {
            let mut ui = self.ui.borrow_mut();
            ui.search_gen += 1;
            ui.searched = Some((query.to_string(), None));
            ui.search_gen
        };
        let (book, q) = (c.book.clone(), query.to_string());
        let task = cx.background_spawn(async move { search::search(&book.index, &book.q, &q, super::SEARCH_MAX) });
        let (ui, shared) = (self.ui.clone(), c.shared.clone());
        cx.spawn(async move |cx| {
            let hits = Rc::new(task.await);
            cx.update(|cx| {
                {
                    let mut u = ui.borrow_mut();
                    if u.search_gen != generation {
                        return;
                    }
                    if let Some((_, res)) = &mut u.searched {
                        *res = Some(hits);
                    }
                }
                refresh(&shared, cx);
            });
        })
        .detach();
        None
    }
}

/// The header's second line: what is being recited, else the source.
fn subtitle(shared: &Shared, book: Option<&Book>) -> String {
    let m = shared.borrow();
    let s = &m.playback;
    if m.pending
        && let (Some(p), Some(_)) = (m.progress, book)
    {
        return tf("Downloading the meaning · {pct}%", &[("pct", &digits((p.done * 100 / p.total.max(1)).to_string()))]);
    }
    if let (Some(g), Some(b)) = (s.current(), book) {
        let who = mandara_services::quran::player::reciter_or_default(&s.reciter);
        let name = if arabic_ui() { who.arabic } else { who.name };
        return match s.status {
            Status::Paused => tf("Paused at {reference} · {reciter}", &[("reference", &b.reference(g)), ("reciter", &name)]),
            _ => tf("Reciting {reference} · {reciter}", &[("reference", &b.reference(g)), ("reciter", &name)]),
        };
    }
    match book.and_then(|b| b.meaning.as_ref()) {
        Some(m) => tf("Tanzil Uthmani text · {translator}", &[("translator", &m.name)]),
        None if m.meaning_error.is_some() && !arabic_ui() => t("Tanzil Uthmani text · the meaning couldn't be downloaded").into(),
        None => t("Tanzil Uthmani text").into(),
    }
}

/// Try the meaning again (after it failed to download).
fn meaning_retry(shared: &Shared, palette: &MandaraPalette) -> Option<AnyElement> {
    let m = shared.borrow();
    if m.meaning_error.is_none() || m.pending || arabic_ui() || !matches!(m.data, Data::Ready(_)) {
        return None;
    }
    drop(m);
    let shared = shared.clone();
    Some(icon_button("quran-meaning-retry", "refresh", TRANSPORT_BTN, palette.on_surface_variant, None, move |_, cx| super::retry(&shared, cx)).into_any_element())
}

/// Pause / resume and stop while a recitation exists.
fn transport(shared: &Shared, palette: &MandaraPalette) -> Option<AnyElement> {
    let status = shared.borrow().playback.status;
    if status == Status::Stopped {
        return None;
    }
    let g = gold(palette);
    let playing = matches!(status, Status::Playing | Status::Loading);
    let (s1, s2) = (shared.clone(), shared.clone());
    Some(
        div()
            .row()
            .items_center()
            .gap(px(tokens::XS))
            .child(icon_button("quran-pause", if playing { "pause" } else { "play" }, TRANSPORT_BTN, g, Some(g.with_alpha(alpha(CHIP_FILL, palette))), move |_, cx| {
                if super::toggle_pause(&s1) {
                    changed(&s1, cx);
                }
            }))
            .child(icon_button("quran-stop", "stop", TRANSPORT_BTN, palette.on_surface_variant, None, move |_, cx| {
                s2.borrow().player.stop();
                changed(&s2, cx);
            }))
            .into_any_element(),
    )
}

/// Opens the reader at global ayah `g`.
pub(super) fn open_reader(shared: &Shared, ui: &Rc<RefCell<Ui>>, g: u16, cx: &mut App) {
    if let Some(b) = book(shared) {
        let mut u = ui.borrow_mut();
        u.tab = Tab::Reader;
        u.reader.open(&b, g);
    }
    refresh(shared, cx);
}

/// A filter chip (gold when active).
fn filter_chip(id: &'static str, label: &'static str, active: bool, palette: &MandaraPalette, on_click: impl Fn(&mut App) + 'static) -> AnyElement {
    let g = gold(palette);
    div()
        .id(id)
        .h(px(CHIPS_H))
        .px(px(FILTER_CHIP_PAD))
        .flex_none()
        .rounded_full()
        .cursor_pointer()
        .flex()
        .items_center()
        .text_size(px(text::LABEL))
        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .when(active, |el| el.bg(g.with_alpha(alpha(CHIP_FILL, palette)).hsla()).text_color(g.hsla()))
        .when(!active, |el| el.bg(wash(WASH_SOFT, palette).hsla()).text_color(palette.on_surface_variant.hsla()))
        .child(label)
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

// ── The download card ────────────────────────────────────────────────────

fn mb(bytes: u64) -> String {
    digits(format!("{:.1}", bytes as f64 / MB))
}

fn download_card(shared: &Shared, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let g = gold(palette);
    let m = shared.borrow();
    let progress = m.progress;
    let (title, line, fail): (String, String, Option<String>) = match &m.data {
        Data::Working => match progress {
            Some(p) => (
                t("Downloading the Qur'an").into(),
                match p.stage {
                    Stage::Index => t("The surah index"),
                    Stage::Text => t("The text: Tanzil's Uthmani script"),
                    Stage::Meaning => t("The meaning in your language"),
                    Stage::Saving => t("Checking every surah and saving"),
                }
                .into(),
                None,
            ),
            None => (t("Opening the Qur'an…").into(), t("Reading it from this computer").into(), None),
        },
        Data::Failed(e) => (t("The download didn't finish").into(), t("Nothing was changed; your files are as they were").into(), Some(e.clone())),
        Data::Offline => (t("You're offline").into(), t("Offline mode is on (Studio → Privacy): the Qur'an downloads once it's off").into(), None),
        _ => {
            let total = store::META_BYTES + store::TEXT_BYTES + if arabic_ui() { 0 } else { store::MEANING_BYTES };
            (t("The Qur'an isn't downloaded").into(), tf("The text and the meaning in your language, about {size} MB, kept on this computer", &[("size", &mb(total))]), None)
        }
    };
    let working = matches!(m.data, Data::Working);
    let can_download = matches!(m.data, Data::Failed(_) | Data::Removed | Data::Idle);
    drop(m);
    let frac = progress.map(|p| p.done as f32 / p.total.max(1) as f32).unwrap_or(0.0);
    let center: AnyElement = match progress {
        Some(p) if working => div()
            .flex()
            .flex_col()
            .items_center()
            .child(div().text_size(px(text::TITLE + 3.0)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(digits(format!("{}%", (frac * 100.0).floor() as u32))))
            .child(caption(tf("{done} of {total} MB", &[("done", &mb(p.done)), ("total", &mb(p.total))]), palette).font_features(tabular_nums()))
            .into_any_element(),
        _ => icon(if matches!(shared.borrow().data, Data::Offline) { "cloud" } else { "book" }, 30.0, g).into_any_element(),
    };
    let ring = div()
        .relative()
        .size(px(DOWNLOAD_RING_PX))
        .flex_none()
        .child(crate::icons::ring(if working { frac } else { 0.0 }, DOWNLOAD_RING_PX, DOWNLOAD_RING_STROKE, g, g.with_alpha(alpha(RING_TRACK, palette))))
        .child(div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().child(center));
    let action: Option<AnyElement> = if can_download {
        let shared = shared.clone();
        Some(pill_button("quran-download", Some("download"), if fail.is_some() { t("Try again") } else { t("Download") }, g, palette, move |_, cx| super::retry(&shared, cx)).into_any_element())
    } else {
        None
    };
    div()
        .relative()
        .w_full()
        .h(px(DOWNLOAD_H))
        .rounded(px(radius))
        .overflow_hidden()
        .bg(hero_bg(palette))
        .shadow(rim(palette))
        .child(lattice(INNER_W, palette))
        .child(dawn(INNER_W, DOWNLOAD_RING_PX * 0.55, palette))
        .child(
            div()
                .size_full()
                .p(px(HERO_PAD))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(GAP))
                .child(ring)
                .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).text_center().child(title))
                .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).text_center().child(crate::bidi::paragraph(line)))
                .when_some(fail, |el, e| el.child(div().max_w(px(INNER_W - HERO_PAD * 4.0)).text_size(px(text::LABEL)).text_color(readable(palette.urgent_coral, palette).hsla()).text_center().line_clamp(3).child(e)))
                .when_some(action, |el, a| el.child(a))
                .child(caption(t("From api.alquran.cloud: the Tanzil Project's text, copied exactly"), palette).text_center()),
        )
        .into_any_element()
}

// ── Home ─────────────────────────────────────────────────────────────────

fn home(c: &Ctx) -> AnyElement {
    let day_peek = c.ui.borrow().day_peek;
    let today = today_card(c).map(|card| {
        let bar = peek_bar(c, "quran-day-peek", day_peek, t("Peek at the coming days' ayahs"), tk("Back to today"), |u| &mut u.day_peek);
        div().flex().flex_col().gap(px(tokens::SM)).child(card).child(bar)
    });
    div().flex().flex_col().gap(px(GAP)).child(continue_hero(c)).when_some(today, |el, x| el.child(x)).child(juz_grid(c)).into_any_element()
}

/// A peek bar over `field` of the panel's state (a look only).
fn peek_bar(c: &Ctx, id: &'static str, offset: i64, here: &'static str, back: &'static str, field: fn(&mut Ui) -> &mut i64) -> AnyElement {
    let (ui, shared) = (c.ui.clone(), c.shared.clone());
    let step: super::plugin_lang::Step = Rc::new(move |by, cx| {
        *field(&mut ui.borrow_mut()) += by;
        refresh(&shared, cx);
    });
    super::plugin_lang::peek_bar(id, offset, here.into(), None, back, gold(c.palette), c.palette, step)
}

/// The ayah `ahead` after your place, shown in the "continue reading" card
/// while you peek (your place stays where it is).
fn next_ayah(c: &Ctx, at: u16, ahead: i64) -> Option<AnyElement> {
    let palette = c.palette;
    let g = u16::try_from(i64::from(at) + ahead).ok().filter(|g| (1..=AYAHS).contains(g))?;
    let a = c.book.q.ayah_at(g)?;
    let r = c.book.q.at(g)?;
    let meaning = c.book.meaning_of(g).filter(|_| !arabic_ui()).map(str::to_string);
    Some(
        glass_card(palette)
            .rounded(px(c.radius))
            .p(px(tokens::MD))
            .flex()
            .flex_col()
            .gap(px(tokens::XS))
            .child(caption(c.book.reference(g), palette).font_features(tabular_nums()))
            .child(
                div()
                    .w_full()
                    .font_family(c.scripture.clone())
                    .text_size(px(TODAY_ARABIC_PX))
                    .line_height(gpui::relative(ARABIC_LINE_EM))
                    .text_right()
                    .text_color(palette.on_surface.hsla())
                    .child(crate::bidi::paragraph(format!("{}\u{00A0}{}", a.text, ayah_mark(r.ayah)))),
            )
            .when_some(meaning, |el, m| el.child(div().text_size(px(text::BODY_S)).line_height(gpui::relative(MEANING_LINE_EM)).text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(m))))
            .into_any_element(),
    )
}

/// "Continue reading": where you are, the khatm ring and the way back in.
fn continue_hero(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let at = continue_at(c.shared);
    let next_peek = c.ui.borrow().next_peek;
    let r = c.book.q.at(at).unwrap_or(Ref::new(1, 1));
    let s = c.book.q.surah(r.surah);
    let (progress, khatms, fresh) = {
        let m = c.shared.borrow();
        (m.state.progress(), m.state.khatms, m.state.last.is_none())
    };
    let juz = c.book.q.ayah(r).map(|a| a.juz).unwrap_or(1);
    let name = s.map(|s| s.short_name().to_string()).unwrap_or_default();
    let title: String = match s {
        Some(s) if !arabic_ui() => s.title(),
        Some(s) => s.name.clone(),
        None => String::new(),
    };
    let detail = digits(tf("Ayah {ayah} of {ayahs} · Juz {juz}", &[("ayah", &r.ayah), ("ayahs", &s.map(|s| s.ayahs).unwrap_or(0)), ("juz", &juz)]));
    let ring = div()
        .relative()
        .size(px(RING_PX))
        .flex_none()
        .child(crate::icons::ring(progress, RING_PX, RING_STROKE, g, g.with_alpha(alpha(RING_TRACK, palette))))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(div().text_size(px(text::TITLE + 2.0)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(digits(format!("{}%", (progress * 100.0).floor() as u32))))
                .child(caption(t("khatm"), palette)),
        );
    let (s1, s2, ui) = (c.shared.clone(), c.shared.clone(), c.ui.clone());
    let listen = pill_button("quran-listen", Some("play"), t("Listen"), g, palette, move |_, cx| {
        if let Err(e) = play_from(&s2, at, cx) {
            ui.borrow().flash.show(false, e);
        }
        changed(&s2, cx);
    });
    let ui = c.ui.clone();
    let go = pill_button("quran-continue", Some("book"), if fresh { t("Begin") } else { t("Continue") }, g, palette, move |_, cx| open_reader(&s1, &ui, at, cx));
    div()
        .relative()
        .w_full()
        .rounded(px(c.radius))
        .overflow_hidden()
        .bg(hero_bg(palette))
        .shadow(rim(palette))
        .child(lattice(INNER_W, palette))
        .child(
            div()
                .relative()
                .p(px(HERO_PAD))
                .flex()
                .flex_col()
                .gap(px(tokens::SM))
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(LABEL_TRACKING_GAP))
                        .child(kicker(if fresh { t("BEGIN YOUR KHATM") } else { t("CONTINUE READING") }, palette))
                        .child(div().flex_1())
                        .when(khatms > 0, |el| el.child(caption(digits(tf("{n} khatms completed", &[("n", &khatms)])), palette).font_features(tabular_nums()))),
                )
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(GAP))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(tokens::XS))
                                .child(div().font_family(c.scripture.clone()).text_size(px(HERO_NAME_PX)).line_height(gpui::relative(1.5)).text_color(g.hsla()).child(crate::bidi::paragraph(name)))
                                .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                                .child(div().text_size(px(text::BODY_S)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(detail)),
                        )
                        .child(ring),
                )
                .child(div().pt(px(tokens::XS)).row().items_center().gap(px(tokens::SM)).child(go).child(listen))
                .when_some((next_peek != 0).then(|| next_ayah(c, at, next_peek)).flatten(), |el, x| el.child(x))
                .child(peek_bar(c, "quran-next-peek", next_peek, t("Peek at the next ayahs"), tk("Back to your place"), |u| &mut u.next_peek)),
        )
        .into_any_element()
}

/// The ayah of the day under Jannah's arch.
fn today_card(c: &Ctx) -> Option<AnyElement> {
    let palette = c.palette;
    let g_ = gold(palette);
    let day_peek = c.ui.borrow().day_peek;
    let g = c.book.ayah_of_day(day_peek)?;
    let r = c.book.q.at(g)?;
    let a = c.book.q.ayah_at(g)?;
    let arch = g_.with_alpha(alpha(ARCH_ALPHA, palette));
    // The crown spans the card inside the inset at its own aspect.
    let crown_w = INNER_W - ARCH_INSET * 2.0;
    let scale = crown_w / jart::CROWN_VIEW.0;
    let crown_h = jart::CROWN_VIEW.1 * scale;
    let stroke = 2.4 * scale;
    let side = |x: f32, w: f32, color: Color, bottom: f32| div().absolute().left(px(x)).top(px(ARCH_INSET + crown_h - 0.5)).bottom(px(bottom)).w(px(w)).bg(color.hsla());
    let outer_l = ARCH_INSET + jart::CROWN_FOOT * scale - stroke / 2.0;
    let inner_off = (jart::CROWN_FOOT + jart::CROWN_INNER) * scale - 1.3 * scale / 2.0;
    let inner = arch.with_alpha(arch.alpha() * ARCH_INNER_SHARE);
    let inner_w = 1.3 * scale;
    let meaning = c.book.meaning_of(g).filter(|_| !arabic_ui()).map(str::to_string);
    let bookmarked = c.shared.borrow().state.is_bookmarked(r);
    let ink = palette.on_surface_variant;
    let (s1, s2, s3, ui1, ui2, ui3) = (c.shared.clone(), c.shared.clone(), c.shared.clone(), c.ui.clone(), c.ui.clone(), c.ui.clone());
    let copied = copy_text(&c.book, g, true);
    let actions = div()
        .w_full()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .child(ref_chip("quran-today-ref", &c.book, g, c.ui, palette))
        .child(div().flex_1())
        .child(icon_button("quran-today-play", "play", ACTION_BTN, ink, None, move |_, cx| {
            if let Err(e) = play(&s1, mandara_services::quran::Selection::Ayah(r), cx) {
                ui1.borrow().flash.show(false, e);
            }
            changed(&s1, cx);
        }))
        .child(icon_button("quran-today-pin", "pin", ACTION_BTN, if bookmarked { g_ } else { ink }, bookmarked.then(|| g_.with_alpha(alpha(CHIP_FILL, palette))), move |_, cx| {
            toggle_bookmark(&s2, r, cx);
        }))
        .child(icon_button("quran-today-copy", "copy", ACTION_BTN, ink, None, {
            let copied_msg = t("Copied with its reference");
            move |_, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
                ui2.borrow().flash.show(true, copied_msg);
            }
        }))
        .child(icon_button("quran-today-read", "book", ACTION_BTN, ink, None, move |_, cx| open_reader(&s3, &ui3, g, cx)));
    let content = div()
        .relative()
        .pt(px(ARCH_INSET + crown_h * CONTENT_TOP_SHARE))
        .px(px(ARCH_INSET + (jart::CROWN_FOOT + jart::CROWN_INNER) * scale + CONTENT_SIDE))
        .pb(px(ARCH_INSET + CONTENT_BOTTOM))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(BLOCK_GAP))
        .child(kicker(
            if day_peek == 0 {
                t("AYAH OF THE DAY").to_string()
            } else {
                tf("AYAH FOR {date}", &[("date", &super::plugin_lang::date(chrono::Local::now().date_naive() + chrono::Duration::days(day_peek), tk("%a %-d %b")).to_uppercase())])
            },
            palette,
        ))
        .child(
            div()
                .w_full()
                .font_family(c.scripture.clone())
                .text_size(px(TODAY_ARABIC_PX))
                .line_height(gpui::relative(ARABIC_LINE_EM))
                .text_center()
                .text_color(palette.on_surface.hsla())
                .child(crate::bidi::paragraph(format!("{}\u{00A0}{}", a.text, ayah_mark(r.ayah)))),
        )
        .when_some(meaning, |el, m| {
            el.child(div().w_full().text_center().text_size(px(text::BODY)).line_height(gpui::relative(MEANING_LINE_EM)).text_color(palette.on_surface.hsla()).child(crate::bidi::paragraph(m)))
        })
        .child(actions);
    Some(
        div()
            .relative()
            .w_full()
            .rounded(px(c.radius))
            .overflow_hidden()
            .bg(hero_bg(palette))
            .shadow(rim(palette))
            .child(lattice(INNER_W, palette))
            .child(dawn(INNER_W, ARCH_INSET + crown_h * DAWN_TOP_SHARE, palette))
            .child(svg().path(jart::CROWN_PATH).absolute().top(px(ARCH_INSET)).left(px(ARCH_INSET)).w(px(crown_w)).h(px(crown_h)).text_color(arch.hsla()))
            .child(side(outer_l, stroke, arch, ARCH_INSET))
            .child(side(INNER_W - outer_l - stroke, stroke, arch, ARCH_INSET))
            .child(side(ARCH_INSET + inner_off, inner_w, inner, ARCH_INSET + jart::CROWN_INNER * scale))
            .child(side(INNER_W - ARCH_INSET - inner_off - inner_w, inner_w, inner, ARCH_INSET + jart::CROWN_INNER * scale))
            .child(div().absolute().left(px(outer_l)).w(px(INNER_W - 2.0 * outer_l)).bottom(px(ARCH_INSET)).h(px(stroke)).bg(arch.hsla()))
            .child(content)
            .into_any_element(),
    )
}

/// The 30 ajza': where each begins, which this khatm has passed and where
/// you are.
fn juz_grid(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let (furthest, here) = {
        let m = c.shared.borrow();
        (m.state.furthest, c.book.q.at(continue_at(c.shared)).and_then(|r| c.book.q.ayah(r)).map(|a| a.juz))
    };
    let start_of = |j: u8| c.book.q.juz_start(j).and_then(|r| c.book.q.global(r)).unwrap_or(AYAHS);
    let tile_w = (INNER_W - JUZ_GAP * (JUZ_COLS - 1) as f32) / JUZ_COLS as f32;
    let tiles = (1..=JUZS).map(|j| {
        let first = start_of(j);
        let next = if j < JUZS { start_of(j + 1) } else { AYAHS + 1 };
        let done = furthest + 1 >= next;
        let current = here == Some(j);
        let (shared, ui) = (c.shared.clone(), c.ui.clone());
        let hover = wash(WASH_SOFT, palette);
        let bg = if current { g.with_alpha(alpha(JUZ_HERE, palette)) } else if done { g.with_alpha(alpha(JUZ_DONE, palette)) } else { wash(WASH_SOFT, palette) };
        div()
            .id(("quran-juz", usize::from(j)))
            .w(px(tile_w))
            .h(px(JUZ_TILE_H))
            .flex_none()
            .rounded(px(JUZ_RADIUS))
            .cursor_pointer()
            .bg(bg.hsla())
            .hover(move |st| st.bg(if current || done { bg.with_alpha(bg.alpha() * 1.35) } else { hover.mix(g, 0.12) }.hsla()))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(if current || done { g.hsla() } else { palette.on_surface.hsla() }).child(digits(j.to_string())))
            .child(div().max_w(px(tile_w - tokens::SM)).truncate().font_family(c.scripture.clone()).text_size(px(JUZ_WORD_PX)).text_color(palette.on_surface_variant.hsla()).child(c.book.juz_words.get(usize::from(j) - 1).cloned().unwrap_or_default()))
            .on_click(move |_, _, cx| open_reader(&shared, &ui, first, cx))
    });
    let done = (1..=JUZS).filter(|j| furthest + 1 >= if *j < JUZS { start_of(j + 1) } else { AYAHS + 1 }).count();
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(section(t("JUZ"), palette, Some(caption(digits(tf("{n} of 30 this khatm", &[("n", &done)])), palette).font_features(tabular_nums()).into_any_element())))
        .child(div().row().flex_wrap().gap(px(JUZ_GAP)).children(tiles))
        .into_any_element()
}

// ── Lists ────────────────────────────────────────────────────────────────

/// One surah: its number in a star medallion, names, meaning, where it was
/// revealed and its ayahs; a click opens it in the reader.
fn surah_row(n: u16, book: &Arc<Book>, here: bool, palette: &MandaraPalette, scripture: &SharedString, shared: &Shared, ui: &Rc<RefCell<Ui>>) -> AnyElement {
    let Some(s) = book.q.surah(n) else { return div().h(px(ROW_H)).into_any_element() };
    let g = gold(palette);
    let hover = wash(ROW_HOVER, palette);
    let rev = match s.revelation {
        Revelation::Makki => t("Makki"),
        Revelation::Madani => t("Madani"),
    };
    let count = digits(tf("{n} ayahs", &[("n", &s.ayahs)]));
    let medallion = div()
        .relative()
        .size(px(MEDALLION_PX))
        .flex_none()
        .child(svg().path(super::art::STAR_PATH).absolute().top_0().left_0().size(px(MEDALLION_PX)).text_color(g.with_alpha(MEDALLION_ALPHA).hsla()))
        .child(div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(g.hsla()).child(digits(n.to_string())));
    let (first, second, trailing): (AnyElement, String, Option<String>) = if arabic_ui() {
        (div().font_family(scripture.clone()).text_size(px(SURAH_ARABIC_PX)).text_color(if here { g.hsla() } else { palette.on_surface.hsla() }).child(s.short_name().to_string()).into_any_element(), format!("{rev} · {count}"), None)
    } else {
        (
            div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().text_color(if here { g.hsla() } else { palette.on_surface.hsla() }).child(s.transliteration.clone()).into_any_element(),
            format!("{} · {rev} · {count}", s.meaning),
            Some(s.short_name().to_string()),
        )
    };
    let (shared, ui) = (shared.clone(), ui.clone());
    let first_g = s.first;
    div()
        .id(("quran-surah", usize::from(n)))
        .h(px(ROW_H))
        .px(px(ROW_PAD_X))
        .rounded(px(ROW_RADIUS))
        .cursor_pointer()
        .hover(move |st| st.bg(hover.hsla()))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(medallion)
        .child(div().flex_1().min_w(px(0.0)).flex().flex_col().child(first).child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).truncate().child(second)))
        .when_some(trailing, |el, a| el.child(div().flex_none().font_family(scripture.clone()).text_size(px(SURAH_ARABIC_PX)).text_color(if here { g.hsla() } else { palette.on_surface.hsla() }).child(a)))
        .on_click(move |_, _, cx| open_reader(&shared, &ui, first_g, cx))
        .into_any_element()
}

/// One ayah: its Arabic on one right-aligned line, the reference and the
/// meaning below; `trailing` sits at the end of the second line.
pub(super) fn ayah_row(
    id: impl Into<gpui::ElementId>,
    g: u16,
    book: &Book,
    palette: &MandaraPalette,
    scripture: &SharedString,
    trailing: Option<AnyElement>,
    on_click: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let Some(a) = book.q.ayah_at(g) else { return div().h(px(ROW_H)).into_any_element() };
    let gc = gold(palette);
    let hover = wash(ROW_HOVER, palette);
    let meaning = book.meaning_of(g).filter(|_| !arabic_ui()).map(str::to_string);
    div()
        .id(id)
        .h(px(ROW_H))
        .px(px(ROW_PAD_X))
        .rounded(px(ROW_RADIUS))
        .cursor_pointer()
        .hover(move |st| st.bg(hover.hsla()))
        .flex()
        .flex_col()
        .justify_center()
        // One right-aligned line of Arabic, laid out right to left, the
        // rest of it clipped.
        .child(
            div()
                .w_full()
                .h(px(LIST_ARABIC_LINE_PX))
                .overflow_hidden()
                .font_family(scripture.clone())
                .text_size(px(LIST_ARABIC_PX))
                .line_height(px(LIST_ARABIC_LINE_PX))
                .text_right()
                .child(crate::bidi::paragraph(a.text.clone())),
        )
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(div().flex_none().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(gc.hsla()).child(book.reference(g)))
                .when_some(meaning, |el, m| el.child(div().flex_1().min_w(px(0.0)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(m)))
                .when_some(trailing, |el, x| el.child(div().flex_1()).child(x)),
        )
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

/// Your bookmarks, newest first.
fn saved(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let marks: Vec<u16> = c.shared.borrow().state.bookmarks.iter().filter_map(|b| c.book.q.global(b.at_ref)).collect();
    if marks.is_empty() {
        return empty_state("pin", t("No bookmarks yet"), t("Tap an ayah in the reader and bookmark it to keep it here"), BODY_MIN, palette).into_any_element();
    }
    glass_card(palette)
        .rounded(px(c.radius))
        .p(px(tokens::XS))
        .flex()
        .flex_col()
        .children(marks.iter().enumerate().map(|(i, g)| {
            let g = *g;
            let r = c.book.q.at(g).unwrap_or(Ref::new(1, 1));
            let (shared, ui, shared2) = (c.shared.clone(), c.ui.clone(), c.shared.clone());
            let remove = icon_button(("quran-unpin", i), "pin-off", ACTION_BTN - 6.0, palette.on_surface_variant, None, move |_, cx| {
                toggle_bookmark(&shared2, r, cx);
            })
            .into_any_element();
            ayah_row(("quran-saved", i), g, &c.book, palette, &c.scripture, Some(remove), move |cx| open_reader(&shared, &ui, g, cx))
        }))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_opens_at_a_sane_height() {
        const { assert!(BODY_GUESS <= BODY_MAX && LIST_H <= BODY_MAX && BODY_MIN < BODY_MAX) };
        const { assert!(LIST_H > CONTROL_H + CHIPS_H + GAP * 2.0 + ROW_H * 6.0) };
        const { assert!(JUZ_COLS * 5 == JUZS as usize) };
        assert_eq!(mb(2_110_000), if arabic_ui() { "٢.١".to_string() } else { "2.1".to_string() });
    }
}
