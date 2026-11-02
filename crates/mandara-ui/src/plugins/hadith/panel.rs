//! The Hadith panel (about 480 px wide): a header, the tabs (Home ·
//! Search · Bookmarks) and a body that springs to its content.
//!
//! * Home: the download card while something downloads (a ring of dots
//!   with the MB, what is being fetched and from where, Retry on failure,
//!   offline said plainly), today's hadith on the reading card, "Continue
//!   where you left off", and the ten collections as cards with their
//!   counts and download state (download or remove each, with progress).
//! * Browsing pushes pages over the tabs with a breadcrumb: a collection's
//!   books, a book's hadith (virtualized lists), then the reader with
//!   previous / next in the book.
//! * Search runs off the UI thread over the downloaded collections, with
//!   collection and grade chips; Bookmarks lists what you kept.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Size, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder as _, px, size, uniform_list,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{PanelCx, PanelSpec};
use mandara_services::hadith::collections::{self, Collection, info};
use mandara_services::hadith::search::Query;
use mandara_services::hadith::store::{self, Loaded};
use mandara_services::hadith::{Grade, GradeClass, Library, refs};
use mandara_theme::MandaraPalette;

use super::super::extras::{CONTROL_H, Flash, Segment, caption, search_box, segmented, toast};
use super::super::fn_view;
use super::super::jannah::gold;
use super::super::panel_fx::{HEADER_H, PanelShape, WASH_SOFT, alpha, dot_ring, empty_state, glass_card, header, meter, partner, pill_button, readable, section, wash};
use super::reader::{self, Mode, Shown};
use super::{ARABIC_LINE_EM, Settings, Shared, arabic_ui, citation, downloaded, grade_color, grade_label, library, model, on_disk, settings, short_ref, status, title, wants_meaning};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 480.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
/// Inner width of the body.
const INNER_W: f32 = PANEL_W - PAD * 2.0;
/// Everything above the body: padding, header, tabs and their gaps.
const CHROME_H: f32 = PAD * 2.0 + HEADER_H + GAP + CONTROL_H + GAP;
/// The body springs to its content between these heights.
const BODY_MIN: f32 = 160.0;
const BODY_MAX: f32 = 640.0;
const BODY_GUESS: f32 = 560.0;
/// Panel height changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;
/// Virtualized lists: their height and rows.
const LIST_H: f32 = 440.0;
const BOOK_ROW_H: f32 = 54.0;
const ROW_H: f32 = 66.0;
const ROW_PAD_X: f32 = 10.0;
const ROW_RADIUS: f32 = 12.0;
const ROW_HOVER: (f32, f32) = (0.045, 0.04);
const LIST_ARABIC_PX: f32 = 16.5;
/// One list line of Arabic: tall enough for its marks.
const LIST_ARABIC_LINE_PX: f32 = LIST_ARABIC_PX * 1.75;
const NUMBER_BADGE: f32 = 32.0;
const READ_DOT: f32 = 6.0;
/// Gap between a row's Arabic line and its caption line.
const LINE_GAP: f32 = 2.0;
/// Collection cards: two columns, their padding and the Arabic name.
const CARD_GAP: f32 = 10.0;
const CARD_W: f32 = (INNER_W - CARD_GAP) / 2.0;
const CARD_PAD: f32 = 12.0;
const CARD_ARABIC_PX: f32 = 18.0;
const CARD_FOOT_H: f32 = 28.0;
const CARD_METER_H: f32 = 4.0;
/// A remove press arms for this long.
const ARM_FOR: Duration = Duration::from_secs(3);
/// The download ring.
const RING_D: f32 = 64.0;
const RING_DOTS: usize = 36;
const RING_DOT: f32 = 3.2;
/// The collection page's head.
const HEAD_ARABIC_PX: f32 = 26.0;
/// Filter chips.
const CHIPS_H: f32 = 28.0;
const CHIP_PAD: f32 = 10.0;
const CHIP_GAP: f32 = 6.0;
/// Search shows at most this many results.
const SEARCH_MAX: usize = 500;
/// Icons in rows and the breadcrumb.
const ROW_ICON: f32 = 15.0;
const NAV_BTN: f32 = 28.0;
/// A collection card's Remove button.
const TRASH_BTN: f32 = 24.0;
/// Panel nav: the reader's previous / next row.
const NAV_ROW_GAP: f32 = 8.0;

// ── State ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Tab {
    Home,
    Search,
    Bookmarks,
}

/// A page pushed over the tabs.
#[derive(Clone)]
enum Page {
    Collection(Collection),
    /// A book (its index in the library).
    Book(Collection, u32),
    /// Hadith of `list` (library indices) full-size, at `at`.
    Reader { c: Collection, list: Rc<[u32]>, at: usize },
    About,
}

/// One search, its key and its results (`None` while it runs).
type SearchKey = (String, Option<Collection>, Option<GradeClass>);

pub(super) struct Ui {
    tab: Tab,
    stack: Vec<Page>,
    search: Option<(SearchKey, Option<(Rc<[(Collection, u32)]>, usize)>)>,
    search_gen: u64,
    filter_collection: Option<Collection>,
    filter_grade: Option<GradeClass>,
    /// Previous / next slide: when, and from which side.
    pub slide: Option<(Instant, f32)>,
    pub flash: Flash,
    /// The reader's chain, shown or folded (`None`: the setting decides).
    show_chain: Option<bool>,
    /// A collection whose Remove was pressed once.
    armed: Option<(Collection, Instant)>,
    /// Peeking: the hero shows the hadith this many slots from today's (a
    /// look only, never saved, never marked read).
    peek: i64,
    chips: crate::hstrip::HStrip,
}

struct View {
    p: PanelCx,
    field: Entity<TextField>,
    ui: Rc<RefCell<Ui>>,
    size: Rc<Cell<Size<Pixels>>>,
}

/// One render's context.
pub(super) struct Ctx<'a> {
    pub palette: &'a MandaraPalette,
    pub shared: &'a Shared,
    pub ui: &'a Rc<RefCell<Ui>>,
    pub hadith_font: SharedString,
    pub radius: f32,
    pub width: f32,
    pub arabic_px: f32,
    pub show_meaning: bool,
    pub settings: &'a Settings,
}

pub(super) fn build(p: PanelCx, window: &mut Window, cx: &mut App) -> PanelSpec {
    let shared = model(cx);
    let views = shared.borrow().views.clone();
    let field = cx.new(|cx| TextField::new(String::new(), t("Search Arabic, meaning or reference…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
    let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(CHROME_H + BODY_GUESS))));
    let ui = Rc::new(RefCell::new(Ui {
        tab: Tab::Home,
        stack: Vec::new(),
        search: None,
        search_gen: 0,
        filter_collection: None,
        filter_grade: None,
        slide: None,
        flash: Flash::default(),
        show_chain: None,
        armed: None,
        peek: 0,
        chips: Default::default(),
    }));
    let view = View { p, field, ui, size: size_cell.clone() };
    PanelSpec { view: fn_view(None, super::plugin_lang::in_language(super::own_lang, move |palette, window, cx| view.render(palette, window, cx)), window, cx), size: size_cell }
}

/// Re-renders every Hadith view (the panel among them).
fn refresh(shared: &Shared, cx: &mut App) {
    super::notify(shared, cx);
}

/// `7,580` (Eastern digits in Arabic).
fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if arabic_ui() { super::super::panel_fx::arabic_digits(&out) } else { out }
}

fn mb(bytes: u64) -> String {
    crate::widgets::system::format_bytes(bytes)
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let shared = model(cx);
        shared.borrow().views.track(window);
        super::ensure_timer(&shared, cx);
        super::ensure_today(&shared, cx);
        let s = settings(cx);
        self.take_open_request(&shared, cx);
        let f = fonts(cx);
        let c = Ctx {
            palette,
            shared: &shared,
            ui: &self.ui,
            hadith_font: f.hadith.clone(),
            radius: PanelShape::of(cx).inner(PAD),
            width: INNER_W,
            arabic_px: s.arabic_size.hero_px(),
            show_meaning: wants_meaning(&s),
            settings: &s,
        };
        let accent = gold(palette);
        let about = {
            let (ui, shared) = (self.ui.clone(), shared.clone());
            icon_button("hadith-about", "info", NAV_BTN, palette.on_surface_variant, None, move |_, cx| {
                ui.borrow_mut().stack.push(Page::About);
                refresh(&shared, cx);
            })
            .into_any_element()
        };
        let head = header("hadith", accent, t("Hadith"), t("The Prophet's sayings ﷺ, collection by collection"), palette, Some(about));
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
                        u.tab = which;
                        u.stack.clear();
                        drop(u);
                        refresh(&shared, cx);
                    }),
                }
            };
            segmented("hadith-tabs", vec![seg(tk("Home"), "home", Tab::Home), seg(tk("Search"), "search", Tab::Search), seg(tk("Bookmarks"), "bookmark", Tab::Bookmarks)], palette)
        };
        let page = self.ui.borrow().stack.last().cloned();
        let fixed = matches!(page, Some(Page::Collection(_) | Page::Book(..))) || (page.is_none() && matches!(tab, Tab::Search | Tab::Bookmarks));
        let body: AnyElement = match page {
            Some(page) => self.page(&c, page, window, cx),
            None => match tab {
                Tab::Home => home(&c, window, cx),
                Tab::Search => self.search_tab(&c, cx),
                Tab::Bookmarks => bookmarks(&c, cx),
            },
        };
        // The body measures itself; the panel springs to it.
        let size_cell = self.size.clone();
        let measure = canvas(
            move |bounds, _, _| {
                let content = f32::from(bounds.size.height);
                let body = if fixed { content } else { content.clamp(BODY_MIN, BODY_MAX) };
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
            .child(div().id("hadith-body").flex_1().min_h(px(0.0)).overflow_y_scroll().child(reveal(p, 2, div().relative().flex_none().flex().flex_col().child(body).child(measure))))
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }

    /// A command or a surface asked to open a hadith: once its collection
    /// is loaded, the reader opens on it (in its book).
    fn take_open_request(&self, shared: &Shared, cx: &mut App) {
        let Some((c, reference)) = shared.borrow().open_request.clone() else { return };
        // What is on disk isn't known yet: wait for it.
        if shared.borrow().statuses_lang.is_none() {
            return;
        }
        if !downloaded(&shared.borrow(), c) {
            shared.borrow_mut().open_request = None;
            self.ui.borrow_mut().flash.show(false, tf("{collection} isn't downloaded", &[("collection", &title(c))]));
            return;
        }
        let Some(loaded) = library(shared, c, cx) else { return };
        shared.borrow_mut().open_request = None;
        let lib = &loaded.library;
        let Some(index) = lib.find(&reference) else {
            self.ui.borrow_mut().flash.show(false, t("That hadith isn't in the collection"));
            return;
        };
        let book = lib.get(index).map_or(0, |h| h.book);
        let list: Rc<[u32]> = lib.books.get(book as usize).map(|b| b.hadith.clone().into()).unwrap_or_else(|| Rc::from([index]));
        let at = list.iter().position(|i| *i == index).unwrap_or_default();
        let mut ui = self.ui.borrow_mut();
        ui.tab = Tab::Home;
        ui.show_chain = None;
        ui.stack = vec![Page::Collection(c), Page::Book(c, book), Page::Reader { c, list, at }];
    }

    fn page(&self, c: &Ctx, page: Page, window: &mut Window, cx: &mut App) -> AnyElement {
        match page {
            Page::Collection(col) => collection_page(c, col, cx),
            Page::Book(col, book) => book_page(c, col, book, cx),
            Page::Reader { c: col, list, at } => reader_page(c, col, list, at, window, cx),
            Page::About => about(c),
        }
    }

    // ── Search ───────────────────────────────────────────────────────────

    fn search_tab(&self, c: &Ctx, cx: &mut App) -> AnyElement {
        let palette = c.palette;
        let query = self.field.read(cx).value.clone();
        let have = on_disk(&c.shared.borrow());
        if have.is_empty() {
            return empty_state("search", t("Nothing to search yet"), t("Download a collection on Home first"), LIST_H, palette).into_any_element();
        }
        let results = self.search(c, query.trim(), &have, cx);
        let chips = filter_chips(c, &have);
        let list_h = LIST_H - CHIPS_H - CONTROL_H - GAP * 2.0;
        let (body, note): (AnyElement, Option<String>) = match &results {
            _ if query.trim().is_empty() => (empty_state("search", t("Search the library"), t("Arabic with or without diacritics, words of the meaning, or a reference like bukhari 1"), list_h, palette).into_any_element(), None),
            None => (div().h(px(list_h)).into_any_element(), Some(t("Searching…").into())),
            Some((hits, _)) if hits.is_empty() => (empty_state("search", t("Nothing matches"), t("Try other words, another collection or any grade"), list_h, palette).into_any_element(), None),
            Some((hits, total)) => {
                let hits = hits.clone();
                let note = if *total > hits.len() { tf("First {shown} of {n} matches", &[("shown", &count(hits.len())), ("n", &count(*total))]) } else { tf("{n} matches", &[("n", &count(*total))]) };
                (hadith_list(c, "hadith-results", hits, list_h, true, cx), Some(note))
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(div().flex().child(search_box("hadith-clear", &self.field, &query, palette)))
            .child(chips)
            .child(div().relative().child(body).when_some(note, |el, n| el.child(div().absolute().bottom_0().left_0().right_0().flex().justify_center().child(caption(n, palette).font_features(tabular_nums())))))
            .into_any_element()
    }

    /// The results of the current query and filters (`None`: still
    /// searching), starting the search off the UI thread when they changed.
    fn search(&self, c: &Ctx, query: &str, have: &[Collection], cx: &mut App) -> Option<(Rc<[(Collection, u32)]>, usize)> {
        if query.is_empty() {
            self.ui.borrow_mut().search = None;
            return None;
        }
        let key: SearchKey = {
            let ui = self.ui.borrow();
            (query.to_string(), ui.filter_collection, ui.filter_grade)
        };
        if let Some((k, res)) = &self.ui.borrow().search
            && *k == key
        {
            return res.clone();
        }
        let generation = {
            let mut ui = self.ui.borrow_mut();
            ui.search_gen += 1;
            ui.search = Some((key.clone(), None));
            ui.search_gen
        };
        let list: Vec<Collection> = have.iter().copied().filter(|x| key.1.is_none_or(|f| f == *x)).collect();
        let q = Query { text: key.0.clone(), collections: None, grade: key.2 };
        let lang = super::ui_code(cx);
        let task = cx.background_spawn(async move {
            let mut hits = Vec::new();
            let mut total = 0;
            for col in list {
                let Ok(loaded) = store::load(col, lang) else { continue };
                let found = mandara_services::hadith::search::search(&loaded.index(), &q, usize::MAX);
                total += found.len();
                hits.extend(found.into_iter().take(SEARCH_MAX.saturating_sub(hits.len())).map(|i| (col, i)));
            }
            (hits, total)
        });
        let (ui, shared) = (self.ui.clone(), c.shared.clone());
        cx.spawn(async move |cx| {
            let (hits, total) = task.await;
            cx.update(|cx| {
                {
                    let mut u = ui.borrow_mut();
                    if u.search_gen != generation {
                        return;
                    }
                    if let Some((_, res)) = &mut u.search {
                        *res = Some((hits.into(), total));
                    }
                }
                refresh(&shared, cx);
            });
        })
        .detach();
        None
    }
}

// ── Home ─────────────────────────────────────────────────────────────────

fn home(c: &Ctx, window: &mut Window, cx: &mut App) -> AnyElement {
    let mut col = div().flex().flex_col().gap(px(GAP));
    if let Some(card) = download_card(c) {
        col = col.child(card);
    }
    let peek = c.ui.borrow().peek;
    let today = super::today(c.shared, cx);
    let has_today = today.is_some();
    let shown = if peek == 0 { today } else { super::peek(c.shared, peek, cx) };
    match shown {
        Some(card) => {
            let book = if card.book_name.is_empty() { tf("Book {n}", &[("n", &card.book)]) } else { card.book_name.clone() };
            let kicker = match peek {
                0 => t("HADITH OF THE DAY"),
                p if p > 0 => t("COMING NEXT"),
                _ => t("EARLIER"),
            };
            let label = format!("{kicker} · {}", book.to_uppercase());
            // The lead grade's own words and grader (none for the two Sahih).
            let grades: Vec<Grade> = card.grade.iter().filter(|g| !g.grade.is_empty()).map(|g| Grade::new(&g.grader, &g.grade)).collect();
            let shown = Shown {
                collection: card.collection,
                reference: &card.reference,
                key: &card.key,
                url: &card.url,
                words: &card.words,
                chain: None,
                meaning: card.meaning.as_deref(),
                meaning_full: card.meaning_full.as_deref(),
                narrator: card.narrator.as_deref(),
                english: card.english,
                translator: None,
                class: card.grade.as_ref().map(|g| g.class),
                grades: &grades,
                label,
            };
            col = col.child(reader::card(c, &shown, Mode::Hero, window));
        }
        // A peek still being picked.
        None if peek != 0 => col = col.child(div().h(px(BODY_MIN)).flex().items_center().justify_center().child(caption(t("Looking…"), c.palette))),
        None if !on_disk(&c.shared.borrow()).is_empty() || c.shared.borrow().current.is_some() => {
            col = col.child(empty_state("hadith", t("The hadith of the day is on its way"), t("It comes from your daily collections once one is downloaded"), BODY_MIN, c.palette));
        }
        None => {}
    }
    if has_today {
        let (ui, shared) = (c.ui.clone(), c.shared.clone());
        let step: super::plugin_lang::Step = Rc::new(move |by, cx| {
            ui.borrow_mut().peek += by;
            refresh(&shared, cx);
        });
        col = col.child(super::plugin_lang::peek_bar("hadith-peek", peek, t("Peek at the coming days' hadith").into(), None, tk("Back to today"), gold(c.palette), c.palette, step));
    }
    if let Some(cont) = continue_card(c) {
        col = col.child(cont);
    }
    col.child(section(t("COLLECTIONS"), c.palette, Some(caption(t("Downloaded on demand"), c.palette).into_any_element()))).child(collection_grid(c)).into_any_element()
}

/// The download card: progress, a failure with Retry, or the first
/// download offered (with offline mode said plainly).
fn download_card(c: &Ctx) -> Option<AnyElement> {
    let palette = c.palette;
    let g = gold(palette);
    let m = c.shared.borrow();
    let offline = mandara_services::offline::is_offline();
    let source = || caption(t("From the public-domain hadith-api collection, via jsDelivr"), palette);
    let card = |ring: AnyElement, title_: String, line: String, extra: Option<AnyElement>| {
        glass_card(palette)
            .rounded(px(c.radius))
            .p(px(CARD_PAD + 2.0))
            .row()
            .items_center()
            .gap(px(tokens::MD + 2.0))
            .child(ring)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(tokens::XS))
                    .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title_))
                    .child(div().text_size(px(text::BODY_S)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(line))
                    .child(source())
                    .when_some(extra, |el, e| el.child(div().pt(px(tokens::XS)).child(e))),
            )
            .into_any_element()
    };
    if let Some(p) = &m.current {
        let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
        let pct = format!("{}%", (frac * 100.0).round() as u32);
        let ring = dot_ring(frac, RING_D, RING_DOTS, RING_DOT, g, partner(g), wash(WASH_SOFT, palette), div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(pct).into_any_element()).into_any_element();
        let what = if p.step == "verify" {
            t("Checking the files").to_string()
        } else if p.step.starts_with("ara-") {
            t("Arabic text").to_string()
        } else {
            t("Meanings").to_string()
        };
        let line = format!("{what} · {}", tf("{done} of {total}", &[("done", &mb(p.done)), ("total", &mb(p.total))]));
        let next = m.queue.first().map(|q| caption(tf("Then {collection}", &[("collection", &title(*q))]), palette).into_any_element());
        return Some(card(ring, tf("Downloading {collection}", &[("collection", &title(p.collection))]), line, next));
    }
    let coral = readable(palette.urgent_coral, palette);
    if let Some((col, why)) = m.errors.iter().next().map(|(k, v)| (*k, v.clone())) {
        let ring = div().size(px(RING_D)).flex_none().rounded_full().bg(coral.with_alpha(alpha(WASH_SOFT, palette) * 2.0).hsla()).flex().items_center().justify_center().child(icon("alert", text::TITLE + 4.0, coral)).into_any_element();
        let shared = c.shared.clone();
        let retry = pill_button("hadith-retry", Some("refresh"), t("Try again"), g, palette, move |_, cx| {
            super::queue(&shared, &[col], cx);
        })
        .into_any_element();
        return Some(card(ring, tf("Couldn't download {collection}", &[("collection", &title(col))]), why, Some(retry)));
    }
    if m.statuses_lang.is_some() && on_disk(&m).is_empty() {
        let want = if c.settings.collections.is_empty() { collections::defaults() } else { c.settings.collections.clone() };
        let bytes: u64 = want.iter().map(|x| collections::download_size(*x, collections::translation(*x, c.settings.lang.code()).map(|(l, _)| l))).sum();
        let ring = div().size(px(RING_D)).flex_none().rounded_full().bg(g.with_alpha(alpha(WASH_SOFT, palette) * 2.0).hsla()).flex().items_center().justify_center().child(icon("download", text::TITLE + 4.0, g)).into_any_element();
        let names = want.iter().map(|x| title(*x)).collect::<Vec<_>>().join(" · ");
        let action: AnyElement = if offline {
            caption(t("Offline mode is on: nothing is downloaded until you turn it off (Studio → Privacy)"), palette).into_any_element()
        } else {
            let shared = c.shared.clone();
            pill_button("hadith-get", Some("download"), tf("Download · {size}", &[("size", &mb(bytes))]), g, palette, move |_, cx| {
                super::queue(&shared, &want, cx);
            })
            .into_any_element()
        };
        drop(m);
        return Some(card(ring, t("Your hadith library").into(), names, Some(action)));
    }
    None
}

/// "Continue where you left off", when you read something.
fn continue_card(c: &Ctx) -> Option<AnyElement> {
    let palette = c.palette;
    let g = gold(palette);
    let last = c.shared.borrow().state.last.clone()?;
    let (col, r) = refs::parse(&last.key)?;
    let key = last.key.clone();
    let shared = c.shared.clone();
    let hover = wash(ROW_HOVER, palette);
    Some(
        glass_card(palette)
            .rounded(px(c.radius))
            .child(
                div()
                    .id("hadith-continue")
                    .p(px(CARD_PAD))
                    .row()
                    .items_center()
                    .gap(px(tokens::MD))
                    .cursor_pointer()
                    .hover(move |st| st.bg(hover.hsla()))
                    .child(super::super::panel_fx::icon_chip("bookmark", g, NUMBER_BADGE))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Continue where you left off")))
                            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).truncate().child(citation(col, &r))),
                    )
                    .child(icon(crate::bidi::dir_icon("chevron-right"), ROW_ICON, palette.on_surface_variant))
                    .on_click(move |_, _, cx| super::open_key(&shared, &key, cx)),
            )
            .into_any_element(),
    )
}

fn collection_grid(c: &Ctx) -> AnyElement {
    let cards: Vec<AnyElement> = Collection::ALL.into_iter().map(|col| collection_card(c, col)).collect();
    div().w_full().flex().flex_wrap().gap(px(CARD_GAP)).children(cards).into_any_element()
}

/// One collection: its names and count, and its state (download, progress,
/// failed, read progress with Remove).
fn collection_card(c: &Ctx, col: Collection) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let i = info(col);
    let m = c.shared.borrow();
    let st = status(&m, col).cloned();
    let have = st.as_ref().is_some_and(|s| s.arabic);
    let current = m.current.as_ref().filter(|p| p.collection == col).cloned();
    let queued = m.queue.contains(&col);
    let failed = m.errors.contains_key(&col);
    let read = m.state.read_count(col);
    drop(m);
    let lang = c.settings.lang.code();
    let translation = collections::translation(col, lang).map(|(l, _)| l);
    let ink = palette.on_surface_variant;
    let foot: AnyElement = if let Some(p) = current {
        let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(tokens::XS))
            .child(meter(frac, CARD_METER_H, g, partner(g), palette))
            .child(caption(tf("{done} of {total}", &[("done", &mb(p.done)), ("total", &mb(p.total))]), palette).font_features(tabular_nums()))
            .into_any_element()
    } else if queued {
        caption(t("Waiting…"), palette).into_any_element()
    } else if failed {
        let shared = c.shared.clone();
        pill_button(SharedString::from(format!("hadith-retry-{}", col.id())), Some("refresh"), t("Retry"), readable(palette.urgent_coral, palette), palette, move |_, cx| {
            cx.stop_propagation();
            super::queue(&shared, &[col], cx);
        })
        .into_any_element()
    } else if !have {
        let shared = c.shared.clone();
        let size = mb(collections::download_size(col, translation));
        pill_button(SharedString::from(format!("hadith-get-{}", col.id())), Some("download"), size, g, palette, move |_, cx| {
            super::queue(&shared, &[col], cx);
        })
        .into_any_element()
    } else if st.as_ref().is_some_and(|s| !s.complete()) {
        let shared = c.shared.clone();
        pill_button(SharedString::from(format!("hadith-meaning-{}", col.id())), Some("download"), t("Meanings"), g, palette, move |_, cx| {
            cx.stop_propagation();
            super::queue(&shared, &[col], cx);
        })
        .into_any_element()
    } else {
        let frac = read as f32 / i.hadith.max(1) as f32;
        let armed = c.ui.borrow().armed.is_some_and(|(x, at)| x == col && at.elapsed() < ARM_FOR);
        let (ui, shared) = (c.ui.clone(), c.shared.clone());
        let remove = if armed {
            pill_button(SharedString::from(format!("hadith-rm-{}", col.id())), Some("trash"), t("Remove?"), readable(palette.urgent_coral, palette), palette, move |_, cx| {
                cx.stop_propagation();
                ui.borrow_mut().armed = None;
                let flash = ui.borrow().flash.clone();
                let removed = t("Removed · {size} freed");
                super::remove(&shared, vec![col], cx, move |r, _| match r {
                    Ok(freed) => flash.show(true, mandara_i18n::fill(removed, &[("size", &mb(freed))])),
                    Err(e) => flash.show(false, e),
                });
            })
            .into_any_element()
        } else {
            icon_button(SharedString::from(format!("hadith-rm-{}", col.id())), "trash", TRASH_BTN, ink, None, move |window, cx| {
                cx.stop_propagation();
                ui.borrow_mut().armed = Some((col, Instant::now()));
                super::super::panel_fx::redraw_in(ARM_FOR, window, cx);
                refresh(&shared, cx);
            })
            .into_any_element()
        };
        div()
            .w_full()
            .row()
            .items_center()
            .gap(px(tokens::SM))
            .child(div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(tokens::XS)).child(meter(frac, CARD_METER_H, g, partner(g), palette)).child(caption(tf("{n} read", &[("n", &count(read))]), palette).font_features(tabular_nums())))
            .child(remove)
            .into_any_element()
    };
    let hover = wash(ROW_HOVER, palette);
    let (ui, shared) = (c.ui.clone(), c.shared.clone());
    let graded = if i.graded { format!(" · {}", t("graded")) } else if i.sahih { format!(" · {}", t("all sahih")) } else { String::new() };
    glass_card(palette)
        .w(px(CARD_W))
        .rounded(px(c.radius))
        .child(
            div()
                .id(SharedString::from(format!("hadith-card-{}", col.id())))
                .p(px(CARD_PAD))
                .flex()
                .flex_col()
                .gap(px(tokens::XS))
                .when(have, |el| el.cursor_pointer().hover(move |st| st.bg(hover.hsla())))
                .child(div().w_full().font_family(c.hadith_font.clone()).text_size(px(CARD_ARABIC_PX)).line_height(gpui::relative(ARABIC_LINE_EM * 0.8)).text_right().text_color(g.hsla()).truncate().child(crate::bidi::paragraph(i.arabic)))
                .when(!arabic_ui(), |el| el.child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(t(i.title))))
                .child(caption(format!("{}{graded}", tf("{n} hadith", &[("n", &count(i.hadith))])), palette).font_features(tabular_nums()))
                .child(div().pt(px(tokens::XS)).min_h(px(CARD_FOOT_H)).flex().items_center().child(foot))
                .on_click(move |_, _, cx| {
                    if have {
                        let mut u = ui.borrow_mut();
                        u.stack = vec![Page::Collection(col)];
                        drop(u);
                        refresh(&shared, cx);
                    }
                }),
        )
        .into_any_element()
}

// ── Browsing ─────────────────────────────────────────────────────────────

/// The back button and the breadcrumb (each step but the last opens).
fn crumbs(c: &Ctx, steps: Vec<(String, Option<Vec<Page>>)>, trailing: Option<AnyElement>) -> AnyElement {
    let palette = c.palette;
    let (ui, shared) = (c.ui.clone(), c.shared.clone());
    let back = icon_button("hadith-back", crate::bidi::dir_icon("chevron-left"), NAV_BTN, palette.on_surface_variant, None, move |_, cx| {
        ui.borrow_mut().stack.pop();
        refresh(&shared, cx);
    });
    let last = steps.len().saturating_sub(1);
    let mut trail = div().flex_1().min_w(px(0.0)).row().items_center().gap(px(tokens::XS)).overflow_hidden();
    for (i, (label, to)) in steps.into_iter().enumerate() {
        if i > 0 {
            trail = trail.child(icon(crate::bidi::dir_icon("chevron-right"), text::LABEL, palette.on_surface_variant));
        }
        let el = div()
            .id(("hadith-crumb", i))
            .min_w(px(0.0))
            .truncate()
            .text_size(px(if i == last { text::TITLE } else { text::BODY_S }))
            .font_weight(if i == last { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .text_color(if i == last { palette.on_surface.hsla() } else { palette.on_surface_variant.hsla() })
            .when(i < last, |el| el.flex_none())
            .child(label);
        trail = trail.child(match to {
            Some(stack) => {
                let (ui, shared) = (c.ui.clone(), c.shared.clone());
                el.cursor_pointer()
                    .on_click(move |_, _, cx| {
                        ui.borrow_mut().stack = stack.clone();
                        refresh(&shared, cx);
                    })
                    .into_any_element()
            }
            None => el.into_any_element(),
        });
    }
    div().row().items_center().gap(px(tokens::SM)).child(back).child(trail).when_some(trailing, |el, t_| el.child(t_)).into_any_element()
}

/// A short name for a collection in the breadcrumb.
fn crumb_name(col: Collection) -> String {
    if arabic_ui() { info(col).arabic.to_string() } else { t(info(col).short).to_string() }
}

fn book_name(lib: &Library, book: u32) -> String {
    lib.books.get(book as usize).map(|b| if b.name.is_empty() { tf("Book {n}", &[("n", &b.number)]) } else { b.name.clone() }).unwrap_or_default()
}

/// Shown while a collection is being opened (or why it couldn't be).
fn opening(c: &Ctx, col: Collection) -> AnyElement {
    let why = c.shared.borrow().load_errors.get(&col).cloned();
    match why {
        Some(e) => empty_state("alert", tf("{collection} couldn't be opened", &[("collection", &title(col))]), e, BODY_MIN, c.palette).into_any_element(),
        None => empty_state("hadith", tf("Opening {collection}…", &[("collection", &title(col))]), t("A moment while the collection is read"), BODY_MIN, c.palette).into_any_element(),
    }
}

fn collection_page(c: &Ctx, col: Collection, cx: &mut App) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let head = crumbs(c, vec![(t("Library").into(), Some(Vec::new())), (crumb_name(col), None)], None);
    let Some(loaded) = library(c.shared, col, cx) else { return div().flex().flex_col().gap(px(GAP)).child(head).child(opening(c, col)).into_any_element() };
    let lib = loaded.library.clone();
    let i = info(col);
    let read = c.shared.borrow().state.read_count(col);
    let frac = read as f32 / lib.hadith.len().max(1) as f32;
    let place = c.shared.borrow().state.places.get(col.id()).map(|p| p.key.clone());
    let resume = place.and_then(|k| refs::parse(&k)).map(|(x, r)| {
        let (shared, key) = (c.shared.clone(), refs::key(x, &r));
        pill_button("hadith-resume", Some("bookmark"), tf("Continue at {reference}", &[("reference", &short_ref(x, &r))]), g, palette, move |_, cx| super::open_key(&shared, &key, cx)).into_any_element()
    });
    let card = glass_card(palette)
        .rounded(px(c.radius))
        .p(px(CARD_PAD + 2.0))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(tokens::XS))
        .child(div().font_family(c.hadith_font.clone()).text_size(px(HEAD_ARABIC_PX)).line_height(gpui::relative(ARABIC_LINE_EM * 0.8)).text_color(g.hsla()).child(crate::bidi::paragraph(i.arabic)))
        .when(!arabic_ui(), |el| el.child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(t(i.title))))
        .child(caption(format!("{} · {}", tf("{n} books", &[("n", &count(lib.books.len()))]), tf("{n} hadith", &[("n", &count(lib.hadith.len()))])), palette).font_features(tabular_nums()))
        .child(div().w_full().pt(px(tokens::SM)).row().items_center().gap(px(tokens::SM)).child(div().flex_1().child(meter(frac, CARD_METER_H, g, partner(g), palette))).child(caption(tf("{n} read", &[("n", &count(read))]), palette).font_features(tabular_nums())))
        .when_some(resume, |el, r| el.child(div().pt(px(tokens::XS)).child(r)));
    let rows: Rc<[u32]> = (0..lib.books.len() as u32).collect();
    let (palette_c, shared, ui) = (palette.clone(), c.shared.clone(), c.ui.clone());
    // Rows are built at layout, outside the render's language scope.
    let lang = mandara_i18n::language();
    let list = uniform_list("hadith-books", rows.len(), move |range, _, _| {
        let _lang = mandara_i18n::scoped(Some(lang));
        range
            .filter_map(|i| rows.get(i).copied())
            .map(|b| book_row(&lib, col, b, &palette_c, &shared, &ui))
            .collect::<Vec<_>>()
    })
    .h(px(LIST_H - BOOK_ROW_H * 2.0))
    .w_full();
    div().flex().flex_col().gap(px(GAP)).child(head).child(card).child(section(t("BOOKS"), palette, None)).child(list).into_any_element()
}

fn book_row(lib: &Arc<Library>, col: Collection, b: u32, palette: &MandaraPalette, shared: &Shared, ui: &Rc<RefCell<Ui>>) -> AnyElement {
    let Some(book) = lib.books.get(b as usize) else { return div().h(px(BOOK_ROW_H)).into_any_element() };
    let g = gold(palette);
    let hover = wash(ROW_HOVER, palette);
    let n = if arabic_ui() { super::super::panel_fx::arabic_digits(&book.number.to_string()) } else { book.number.to_string() };
    let range = match (book.hadith.first().and_then(|i| lib.get(*i)), book.hadith.last().and_then(|i| lib.get(*i))) {
        (Some(a), Some(z)) if a.reference != z.reference => format!(" · {}–{}", a.reference, z.reference),
        _ => String::new(),
    };
    let (ui, shared) = (ui.clone(), shared.clone());
    div()
        .id(("hadith-book", b as usize))
        .h(px(BOOK_ROW_H))
        .px(px(ROW_PAD_X))
        .rounded(px(ROW_RADIUS))
        .cursor_pointer()
        .hover(move |st| st.bg(hover.hsla()))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(div().size(px(NUMBER_BADGE)).flex_none().rounded(px(ROW_RADIUS - 2.0)).bg(g.with_alpha(alpha(super::super::jannah::panel::CHIP_FILL, palette)).hsla()).flex().items_center().justify_center().text_size(px(text::LABEL)).font_weight(FontWeight::BOLD).font_features(tabular_nums()).text_color(g.hsla()).child(n))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(book_name(lib, b)))
                .child(caption(format!("{}{range}", tf("{n} hadith", &[("n", &count(book.hadith.len()))])), palette).font_features(tabular_nums())),
        )
        .child(icon(crate::bidi::dir_icon("chevron-right"), ROW_ICON, palette.on_surface_variant))
        .on_click(move |_, _, cx| {
            ui.borrow_mut().stack.push(Page::Book(col, b));
            refresh(&shared, cx);
        })
        .into_any_element()
}

fn book_page(c: &Ctx, col: Collection, book: u32, cx: &mut App) -> AnyElement {
    let Some(loaded) = library(c.shared, col, cx) else {
        let head = crumbs(c, vec![(t("Library").into(), Some(Vec::new())), (crumb_name(col), Some(vec![Page::Collection(col)]))], None);
        return div().flex().flex_col().gap(px(GAP)).child(head).child(opening(c, col)).into_any_element();
    };
    let lib = &loaded.library;
    let name = book_name(lib, book);
    let ids: Vec<(Collection, u32)> = lib.books.get(book as usize).map(|b| b.hadith.iter().map(|i| (col, *i)).collect()).unwrap_or_default();
    let n = caption(count(ids.len()), c.palette).font_features(tabular_nums()).into_any_element();
    let head = crumbs(c, vec![(t("Library").into(), Some(Vec::new())), (crumb_name(col), Some(vec![Page::Collection(col)])), (name, None)], Some(n));
    let list = hadith_list(c, "hadith-book-list", ids.into(), LIST_H, false, cx);
    div().flex().flex_col().gap(px(GAP)).child(head).child(list).into_any_element()
}

/// A virtualized list of hadith (any collections); a click opens the
/// reader on the hadith's book (`in_book`) or on the list itself.
fn hadith_list(c: &Ctx, id: &'static str, items: Rc<[(Collection, u32)]>, height: f32, labelled: bool, cx: &mut App) -> AnyElement {
    // Every collection in the list must be loaded to draw it.
    let mut libs: Vec<(Collection, Arc<Loaded>)> = Vec::new();
    for col in Collection::ALL {
        if items.iter().any(|(x, _)| *x == col) {
            match library(c.shared, col, cx) {
                Some(l) => libs.push((col, l)),
                None => return opening(c, col),
            }
        }
    }
    let (palette, shared, ui, font) = (c.palette.clone(), c.shared.clone(), c.ui.clone(), c.hadith_font.clone());
    let show_meaning = c.show_meaning;
    let lang = mandara_i18n::language();
    uniform_list(id, items.len(), move |range, _, _| {
        let _lang = mandara_i18n::scoped(Some(lang));
        let m = shared.borrow();
        range
            .filter_map(|i| items.get(i).copied().map(|x| (i, x)))
            .map(|(i, (col, index))| {
                let Some(lib) = libs.iter().find(|(x, _)| *x == col).map(|(_, l)| &l.library) else { return div().h(px(ROW_H)).into_any_element() };
                let read = m.state.is_read(col, index);
                hadith_row((id, i), lib, index, read, labelled, show_meaning, &palette, &font, &shared, &ui, &items, i)
            })
            .collect::<Vec<_>>()
    })
    .h(px(height))
    .w_full()
    .into_any_element()
}

/// One hadith: one line of its words, right-aligned; its reference, grade
/// dot and meaning below.
#[expect(clippy::too_many_arguments, reason = "a row needs the library, the theme, the font and where it opens; bundling them would only rename the list")]
fn hadith_row(
    id: impl Into<gpui::ElementId>,
    lib: &Library,
    index: u32,
    read: bool,
    labelled: bool,
    show_meaning: bool,
    palette: &MandaraPalette,
    font: &SharedString,
    shared: &Shared,
    ui: &Rc<RefCell<Ui>>,
    items: &Rc<[(Collection, u32)]>,
    at: usize,
) -> AnyElement {
    let Some(h) = lib.get(index) else { return div().h(px(ROW_H)).into_any_element() };
    let col = lib.collection;
    let g = gold(palette);
    let hover = wash(ROW_HOVER, palette);
    let reference = if labelled { short_ref(col, &h.reference) } else if arabic_ui() { super::super::panel_fx::arabic_digits(&h.reference) } else { h.reference.clone() };
    let meaning = h.meaning_body().filter(|_| show_meaning).map(|m| m.chars().take(160).collect::<String>());
    let class = h.class(col);
    // From a single book: the reader walks the list; from search or
    // bookmarks: the hadith's own book.
    let list: Rc<[u32]> = if labelled { lib.books.get(h.book as usize).map(|b| b.hadith.clone().into()).unwrap_or_else(|| Rc::from([index])) } else { items.iter().map(|(_, i)| *i).collect() };
    let pos = if labelled { list.iter().position(|i| *i == index).unwrap_or_default() } else { at };
    let (ui, shared) = (ui.clone(), shared.clone());
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
        .gap(px(LINE_GAP))
        .child(div().w_full().h(px(LIST_ARABIC_LINE_PX)).overflow_hidden().font_family(font.clone()).text_size(px(LIST_ARABIC_PX)).line_height(px(LIST_ARABIC_LINE_PX)).text_right().child(crate::bidi::paragraph(h.words().to_string())))
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .when(read, |el| el.child(div().size(px(READ_DOT)).flex_none().rounded_full().bg(g.with_alpha(0.7).hsla())))
                .child(div().flex_none().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(g.hsla()).child(reference))
                .when_some(class, |el, cl| el.child(div().size(px(READ_DOT)).flex_none().rounded_full().bg(grade_color(cl, palette).hsla())))
                .when_some(meaning, |el, mm| el.child(div().flex_1().min_w(px(0.0)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(mm))),
        )
        .on_click(move |_, _, cx| {
            let mut u = ui.borrow_mut();
            u.show_chain = None;
            u.stack.push(Page::Reader { c: col, list: list.clone(), at: pos });
            drop(u);
            refresh(&shared, cx);
        })
        .into_any_element()
}

fn reader_page(c: &Ctx, col: Collection, list: Rc<[u32]>, at: usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(loaded) = library(c.shared, col, cx) else { return opening(c, col) };
    let lib = &loaded.library;
    let at = at.min(list.len().saturating_sub(1));
    let Some(index) = list.get(at).copied() else { return div().into_any_element() };
    let Some(h) = lib.get(index) else { return div().into_any_element() };
    let key = lib.key(index).unwrap_or_default();
    let url = lib.url(index).unwrap_or_default();
    super::visit(c.shared, col, index, &key, cx);
    let book = h.book;
    let name = book_name(lib, book);
    let n = list.len();
    let position = (n > 1).then(|| caption(tf("{n} of {total}", &[("n", &count(at + 1)), ("total", &count(n))]), c.palette).font_features(tabular_nums()).into_any_element());
    let head = crumbs(
        c,
        vec![(crumb_name(col), Some(vec![Page::Collection(col)])), (name.clone(), Some(vec![Page::Collection(col), Page::Book(col, book)])), (short_ref(col, &h.reference), None)],
        position,
    );
    let chain_shown = c.ui.borrow().show_chain.unwrap_or(!c.settings.hide_chain);
    let (before, after) = h.chain();
    let has_chain = !before.is_empty() || !after.is_empty();
    let toggle: Rc<dyn Fn(&mut App)> = {
        let (ui, shared) = (c.ui.clone(), c.shared.clone());
        Rc::new(move |cx| {
            ui.borrow_mut().show_chain = Some(!chain_shown);
            refresh(&shared, cx);
        })
    };
    let in_book = h.in_book.map(|k| tf("Book {book}, hadith {n}", &[("book", &lib.books.get(book as usize).map_or(0, |b| b.number)), ("n", &k)])).unwrap_or_else(|| name.clone());
    let label = in_book.to_uppercase();
    let meaning = lib.meaning.as_ref();
    let shown = Shown {
        collection: col,
        reference: &h.reference,
        key: &key,
        url: &url,
        words: h.words(),
        chain: chain_shown.then_some((before, after)),
        meaning: h.meaning_body(),
        meaning_full: h.meaning.as_deref(),
        narrator: h.narrator.as_deref(),
        english: meaning.is_some_and(|m| m.fallback),
        translator: meaning.and_then(|m| m.translator.as_deref()),
        class: h.class(col),
        grades: &h.grades,
        label,
    };
    let page = reader::card(c, &shown, Mode::Reader { chain_shown, toggle, has_chain }, window);
    let nav = (n > 1).then(|| {
        let go = |step: isize| {
            let (ui, shared) = (c.ui.clone(), c.shared.clone());
            move |_: &mut Window, cx: &mut App| {
                if let Some(Page::Reader { list, at, .. }) = ui.borrow_mut().stack.last_mut() {
                    *at = (*at as isize + step).rem_euclid(list.len() as isize) as usize;
                }
                ui.borrow_mut().slide = Some((Instant::now(), step as f32));
                refresh(&shared, cx);
            }
        };
        let g = gold(c.palette);
        div()
            .w_full()
            .row()
            .items_center()
            .justify_between()
            .gap(px(NAV_ROW_GAP))
            .child(pill_button("hadith-prev", Some(crate::bidi::dir_icon("chevron-left")), t("Previous"), g, c.palette, go(-1)))
            .child(pill_button("hadith-next", Some(crate::bidi::dir_icon("chevron-right")), t("Next"), g, c.palette, go(1)))
    });
    div().flex().flex_col().gap(px(GAP)).child(head).child(page).when_some(nav, |el, n| el.child(n)).into_any_element()
}

// ── Filters ──────────────────────────────────────────────────────────────

/// Collection chips (the downloaded ones) and grade chips.
fn filter_chips(c: &Ctx, have: &[Collection]) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let (collection, grade) = {
        let ui = c.ui.borrow();
        (ui.filter_collection, ui.filter_grade)
    };
    let strip = c.ui.borrow().chips.clone();
    let chip = |id: SharedString, label: SharedString, active: bool, tint: Option<mandara_theme::Color>, set: Rc<dyn Fn(&mut Ui)>| {
        let (ui, shared, strip) = (c.ui.clone(), c.shared.clone(), strip.clone());
        let accent = tint.unwrap_or(g);
        div()
            .id(id)
            .h(px(CHIPS_H))
            .px(px(CHIP_PAD))
            .flex_none()
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(tokens::XS + 1.0))
            .text_size(px(text::LABEL))
            .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .when(active, |el| el.bg(accent.with_alpha(alpha(super::super::jannah::panel::CHIP_FILL, palette)).hsla()).text_color(accent.hsla()))
            .when(!active, |el| el.bg(wash(WASH_SOFT, palette).hsla()).text_color(palette.on_surface_variant.hsla()))
            .when_some(tint, |el, t_| el.child(div().size(px(READ_DOT)).rounded_full().bg(t_.hsla())))
            .child(label)
            .on_click(move |_, _, cx| {
                // A press that dragged the strip is not a pick.
                if strip.dragged() {
                    return;
                }
                set(&mut ui.borrow_mut());
                refresh(&shared, cx);
            })
            .into_any_element()
    };
    let mut chips: Vec<AnyElement> = vec![chip("hadith-col-all".into(), t("All collections").into(), collection.is_none(), None, Rc::new(|u: &mut Ui| u.filter_collection = None))];
    for col in have.iter().copied() {
        chips.push(chip(format!("hadith-col-{}", col.id()).into(), crumb_name(col).into(), collection == Some(col), None, Rc::new(move |u: &mut Ui| u.filter_collection = if u.filter_collection == Some(col) { None } else { Some(col) })));
    }
    chips.push(div().w(px(1.0)).h(px(CHIPS_H * 0.6)).flex_none().bg(wash(WASH_SOFT, palette).hsla()).into_any_element());
    chips.push(chip("hadith-grade-any".into(), t("Any grade").into(), grade.is_none(), None, Rc::new(|u: &mut Ui| u.filter_grade = None)));
    for class in [GradeClass::Sahih, GradeClass::Hasan, GradeClass::Daif] {
        chips.push(chip(
            format!("hadith-grade-{}", class.id()).into(),
            t(grade_label(class)).into(),
            grade == Some(class),
            Some(grade_color(class, palette)),
            Rc::new(move |u: &mut Ui| u.filter_grade = if u.filter_grade == Some(class) { None } else { Some(class) }),
        ));
    }
    strip.with_chevrons("hadith-chips", strip.row("hadith-chips").row().items_center().gap(px(CHIP_GAP)).children(chips), CHIPS_H, palette)
}

// ── Bookmarks ────────────────────────────────────────────────────────────

fn bookmarks(c: &Ctx, cx: &mut App) -> AnyElement {
    let palette = c.palette;
    let marks: Vec<(Collection, String)> = c.shared.borrow().state.bookmarks.iter().filter_map(|b| refs::parse(&b.key)).collect();
    if marks.is_empty() {
        return empty_state("bookmark", t("No bookmarks yet"), t("Tap the ribbon on a hadith to keep it here"), LIST_H, palette).into_any_element();
    }
    let have = on_disk(&c.shared.borrow());
    let mut items: Vec<(Collection, u32)> = Vec::new();
    let mut away = 0;
    for (col, r) in &marks {
        if !have.contains(col) {
            away += 1;
            continue;
        }
        let Some(loaded) = library(c.shared, *col, cx) else { return opening(c, *col) };
        match loaded.library.find(r) {
            Some(i) => items.push((*col, i)),
            None => away += 1,
        }
    }
    let note = (away > 0).then(|| caption(tf("{n} more in collections that aren't downloaded", &[("n", &count(away))]), palette).into_any_element());
    let list_h = LIST_H - if note.is_some() { GAP * 2.0 } else { 0.0 };
    div()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(section(t("BOOKMARKS"), palette, Some(caption(count(items.len()), palette).font_features(tabular_nums()).into_any_element())))
        .child(hadith_list(c, "hadith-bookmarks", items.into(), list_h, true, cx))
        .when_some(note, |el, n| el.child(n))
        .into_any_element()
}

// ── About ────────────────────────────────────────────────────────────────

fn about(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let para = |s: String| div().text_size(px(text::BODY_S)).line_height(gpui::relative(super::super::jannah::panel::MEANING_LINE_EM)).child(crate::bidi::paragraph(s));
    let head = crumbs(c, vec![(t("About the library").into(), None)], None);
    div()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(head)
        .child(
            glass_card(palette)
                .rounded(px(c.radius))
                .p(px(CARD_PAD + 2.0))
                .flex()
                .flex_col()
                .gap(px(tokens::MD))
                .child(para(t("The collections come from the hadith-api dataset, released into the public domain (the Unlicense). Each one is downloaded when you ask for it, checked, and kept on this computer in ~/.local/share/mandara/hadith.").into()))
                .child(div().row().gap(px(tokens::SM)).child(pill_button("hadith-source", Some("external"), "hadith-api", g, palette, |_, _| super::super::jannah::open_link(store::HOME.to_string()))))
                .child(para(t("The Arabic and the meanings are shown exactly as the dataset gives them, with its grades and graders. Where your language has no translation of a collection, the English one is shown and labelled.").into()))
                .child(para(t("References and links follow sunnah.com. Muwatta Malik is numbered as in the dataset and links to its book.").into()))
                .child(caption(t("When in doubt, check a hadith with a scholar or a printed edition."), palette)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_fits() {
        const { assert!(BODY_GUESS < BODY_MAX) };
        const { assert!(LIST_H > CHIPS_H + CONTROL_H + GAP * 2.0 + ROW_H * 4.0) };
        const { assert!(CARD_W * 2.0 + CARD_GAP <= INNER_W) };
    }

    #[test]
    fn counts_group_thousands() {
        if !arabic_ui() {
            assert_eq!(count(7580), "7,580");
            assert_eq!(count(42), "42");
            assert_eq!(count(1_234_567), "1,234,567");
        }
    }
}
