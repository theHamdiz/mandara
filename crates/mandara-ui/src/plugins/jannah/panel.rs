//! The Jannah panel: a header, the tabs (Today · Paths · Qur'an · Hadith ·
//! Saved) and a body that springs to its content. Today shows the hero (the
//! reminder under an arch, dawn light in its crown, a faint star lattice),
//! today's step and this week's dots. Paths, the reader and the About sheet
//! push onto a small stack inside the body. The Qur'an and Hadith lists are
//! virtualized (`uniform_list`) and searched off the UI thread.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::Local;
use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Size, StatefulInteractiveElement,
    Styled, Window, canvas, div, prelude::FluentBuilder as _, px, size, svg, uniform_list,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{PanelCx, PanelSpec};
use mandara_services::jannah::catalog::{self, Item, ItemId, catalog};
use mandara_services::jannah::meanings;
use mandara_services::jannah::paths::{PATHS, Path, PathGroup};
use mandara_services::jannah::Collection;
use mandara_theme::{Color, MandaraPalette};

use super::super::extras::{CONTROL_H, Flash, Segment, caption, search_box, segmented, toast};
use super::super::fn_view;
use super::super::panel_fx::{HEADER_H, PanelShape, alpha, empty_state, glass_card, glow, header, pill_button, readable, rim, section, wash, WASH_SOFT};
use super::art;
use super::{ARABIC_LINE_EM, Fetch, Shared, arabic_ui, chip_text, ensure_meanings, ensure_timer, gold, meaning_of, model, nudge, open_link, settings, toggle_saved, wants_meaning};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 440.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
/// Everything above the body: padding, header, tabs and their gaps.
const CHROME_H: f32 = PAD * 2.0 + HEADER_H + GAP + CONTROL_H + GAP;
/// The body springs to its content between these heights.
const BODY_MIN: f32 = 150.0;
const BODY_MAX: f32 = 600.0;
/// The body's first height on Today, before it is measured.
const BODY_GUESS: f32 = 470.0;
/// The Qur'an and Hadith lists: their height and row height.
const LIST_H: f32 = 470.0;
const ROW_H: f32 = 64.0;
/// Panel height changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;
/// Inner width of the body.
const INNER_W: f32 = PANEL_W - PAD * 2.0;

// Hero.
/// Hero tones `(lightness, chroma share, chroma cap)` on dark and light.
pub(crate) const HERO_DARK: [(f32, f32, f32); 2] = [(0.31, 0.75, 0.09), (0.23, 0.7, 0.07)];
pub(crate) const HERO_LIGHT: [(f32, f32, f32); 2] = [(0.94, 0.4, 0.05), (0.965, 0.32, 0.04)];
pub(crate) const HERO_ANGLE_DEG: f32 = 165.0;
/// How much of the primary tints the emerald.
pub(crate) const HERO_PRIMARY_SHARE: f32 = 0.35;
/// The arch: inset from the card's edge, its alpha, the inner arch's.
pub(crate) const ARCH_INSET: f32 = 10.0;
pub(crate) const ARCH_ALPHA: (f32, f32) = (0.62, 0.7);
pub(crate) const ARCH_INNER_SHARE: f32 = 0.5;
/// The lattice's alpha `(dark, light)`.
pub(crate) const LATTICE_ALPHA: (f32, f32) = (0.11, 0.13);
/// Dawn light: a fill-less source (size, offset into the crown) whose
/// wide blurred shadow is the glow (alpha, blur, spread): soft at every
/// edge, never a visible shape.
pub(crate) const DAWN_W: f32 = 84.0;
pub(crate) const DAWN_H: f32 = 18.0;
pub(crate) const DAWN_TOP_SHARE: f32 = 0.5;
pub(crate) const DAWN_GLOW: (f32, f32, f32) = (0.34, 64.0, 4.0);
/// The words start this share of the crown down; side and bottom room.
const CONTENT_TOP_SHARE: f32 = 0.6;
const CONTENT_SIDE: f32 = 18.0;
const CONTENT_BOTTOM: f32 = 18.0;
const BLOCK_GAP: f32 = 10.0;
/// Gap between a line and its caption.
const LINE_GAP: f32 = 2.0;
/// Corner buttons (previous / next) and the action buttons.
const NAV_BTN: f32 = 28.0;
const ACTION_BTN: f32 = 30.0;
/// The reference chip.
const CHIP_H: f32 = 26.0;
const CHIP_PAD_X: f32 = 10.0;
pub(crate) const CHIP_FILL: (f32, f32) = (0.16, 0.14);
/// Meaning: line height and the alpha of its credit.
pub(crate) const MEANING_LINE_EM: f32 = 1.5;
/// The reader's chain of narrators.
const CHAIN_PX: f32 = 14.5;
/// Slide on previous / next: distance, how long it may run, fade.
const SLIDE_PX: f32 = 36.0;
const SLIDE_FOR: Duration = Duration::from_millis(700);
const SLIDE_FADE: f32 = 0.85;
const SLIDE_REST_PX: f32 = 0.3;

// Step.
const STEP_PAD: f32 = 14.0;
const STEP_GLOW_FOR: Duration = Duration::from_millis(1600);
const STEP_GLOW: (f32, f32) = (0.45, 28.0);
const CHECK_FILL: (f32, f32) = (0.2, 0.16);
const DOT_PX: f32 = 9.0;
const DOT_TODAY_PX: f32 = 13.0;
const DOT_GAP: f32 = 14.0;
const DOT_OFF: (f32, f32) = (0.16, 0.14);
/// Arabic evidence excerpts are cut to this many characters.
const ARABIC_EXCERPT: usize = 140;

// Lists.
const LIST_ARABIC_PX: f32 = 16.5;
/// One list line of Arabic: tall enough for its marks above and below.
const LIST_ARABIC_LINE_PX: f32 = LIST_ARABIC_PX * 1.75;
const HEAD_ARABIC_PX: f32 = 20.0;
const ROW_PAD_X: f32 = 10.0;
const ROW_RADIUS: f32 = 12.0;
const ROW_HOVER: (f32, f32) = (0.045, 0.04);
const CHIPS_H: f32 = 28.0;
const FILTER_CHIP_PAD: f32 = 10.0;
const CHIP_GAP: f32 = 6.0;
const PATH_ICON: f32 = 15.0;
const PATH_ROW_PAD: f32 = 10.0;

// ── State ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Tab {
    Today,
    Paths,
    Quran,
    Hadith,
    Saved,
}

/// A page pushed over the tabs.
enum Sheet {
    /// Items full-size, previous / next within `list`.
    Reader { list: Rc<[ItemId]>, at: usize },
    Path(&'static Path),
    About,
}

/// One row of a virtualized list.
#[derive(Clone, Copy)]
enum Row {
    /// A surah (number) or a collection heading, with its count.
    Surah(u16, usize),
    Collection(Collection, usize),
    Item(ItemId),
}

struct Ui {
    tab: Tab,
    stack: Vec<Sheet>,
    theme: Option<&'static str>,
    collection: Option<&'static str>,
    /// The last search: its query and results (`None` while it runs).
    searched: Option<(String, Option<Rc<[ItemId]>>)>,
    search_gen: u64,
    /// Previous / next slide: when, and from which side.
    slide: Option<(Instant, f32)>,
    /// The step's glow after a tick.
    glow: Option<Instant>,
    flash: Flash,
    /// The Qur'an and Hadith filter chips (wheel and drag scroll them).
    chips_q: crate::hstrip::HStrip,
    chips_h: crate::hstrip::HStrip,
}

struct View {
    p: PanelCx,
    field: Entity<TextField>,
    ui: Rc<RefCell<Ui>>,
    size: Rc<Cell<Size<Pixels>>>,
}

/// Every surface's context for one render.
struct Ctx<'a> {
    palette: &'a MandaraPalette,
    shared: &'a Shared,
    ui: &'a Rc<RefCell<Ui>>,
    scripture: SharedString,
    radius: f32,
    arabic_px: f32,
    show_meaning: bool,
}

pub(super) fn build(p: PanelCx, window: &mut Window, cx: &mut App) -> PanelSpec {
    let shared = model(cx);
    let views = shared.borrow().views.clone();
    let field = cx.new(|cx| TextField::new(String::new(), t("Search Arabic or meaning…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
    let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(CHROME_H + BODY_GUESS))));
    let ui = Rc::new(RefCell::new(Ui { tab: Tab::Today, stack: Vec::new(), theme: None, collection: None, searched: None, search_gen: 0, slide: None, glow: None, flash: Flash::default(), chips_q: Default::default(), chips_h: Default::default() }));
    let view = View { p, field, ui, size: size_cell.clone() };
    PanelSpec { view: fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx), size: size_cell }
}

/// Re-renders every Jannah view (the panel among them).
fn refresh(shared: &Shared, cx: &mut App) {
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

/// Hero backdrop tone from a theme colour.
pub(crate) fn tone(c: Color, (l, share, cap): (f32, f32, f32)) -> Color {
    c.with_lightness(l).with_chroma((c.chroma() * share).min(cap))
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let shared = model(cx);
        shared.borrow().views.track(window);
        ensure_timer(&shared, cx);
        let s = settings(cx);
        let show_meaning = wants_meaning(&s);
        if show_meaning {
            ensure_meanings(&shared, cx);
        }
        if let Some(id) = shared.borrow_mut().open_request.take() {
            let mut ui = self.ui.borrow_mut();
            ui.stack = vec![Sheet::Reader { list: Rc::from([id]), at: 0 }];
        }
        let f = fonts(cx);
        let c = Ctx { palette, shared: &shared, ui: &self.ui, scripture: f.scripture.clone(), radius: PanelShape::of(cx).inner(PAD), arabic_px: s.arabic_size.hero_px(), show_meaning };
        let accent = gold(palette);
        let about = {
            let (ui, shared) = (self.ui.clone(), shared.clone());
            icon_button("jannah-about", "info", NAV_BTN, palette.on_surface_variant, None, move |_, cx| {
                ui.borrow_mut().stack.push(Sheet::About);
                refresh(&shared, cx);
            })
            .into_any_element()
        };
        let head = header("arch", accent, t("Jannah"), t("A daily reminder of the final home"), palette, Some(about));
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
            segmented(
                "jannah-tabs",
                vec![seg(tk("Today"), "sun", Tab::Today), seg(tk("Paths"), "leaf", Tab::Paths), seg(tk("Qur'an"), "book", Tab::Quran), seg(tk("Hadith"), "note", Tab::Hadith), seg(tk("Favorites"), "heart", Tab::Saved)],
                palette,
            )
        };
        let fixed = self.ui.borrow().stack.is_empty() && matches!(tab, Tab::Quran | Tab::Hadith);
        let body: AnyElement = if self.ui.borrow().stack.is_empty() {
            match tab {
                Tab::Today => today(&c, window, cx),
                Tab::Paths => paths(&c),
                Tab::Quran | Tab::Hadith => self.list(&c, tab == Tab::Quran, cx),
                Tab::Saved => saved(&c),
            }
        } else {
            sheet(&c, window)
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
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(p, 1, tabs))
            .child(div().h(px(GAP)))
            .child(
                div()
                    .id("jannah-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(reveal(p, 2, div().relative().flex_none().flex().flex_col().child(body).child(measure))),
            )
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }

    /// The Qur'an or Hadith tab: search, filter chips and the list.
    fn list(&self, c: &Ctx, quran: bool, cx: &mut App) -> AnyElement {
        let palette = c.palette;
        let query = self.field.read(cx).value.clone();
        let results = self.search(c.shared, query.trim(), cx);
        let (theme, collection) = {
            let ui = c.ui.borrow();
            (ui.theme, ui.collection)
        };
        let cat = catalog();
        let keep = |item: &Item| {
            theme.is_none_or(|th| item.themes().iter().any(|x| x.id() == th))
                && (quran || collection.is_none_or(|col| item.hadith().is_some_and(|h| h.collection.id() == col)))
                && results.as_ref().is_none_or(|r| r.contains(&item.id))
        };
        let ids: Vec<&Item> = if quran { cat.verse_ids().collect::<Vec<_>>() } else { cat.hadith_ids().collect() }.into_iter().filter_map(|id| cat.get(id)).filter(|i| keep(i)).collect();
        let rows: Rc<[Row]> = group_rows(&ids, quran).into();
        let list_ids: Rc<[ItemId]> = ids.iter().map(|i| i.id).collect();
        let pending = !query.trim().is_empty() && results.is_none();
        let chips = filter_chips(c, quran);
        let count = rows.len();
        let body: AnyElement = if ids.is_empty() && !pending {
            empty_state(if quran { "book" } else { "note" }, t("Nothing matches"), t("Try other words or another theme"), LIST_H - CHIPS_H - CONTROL_H - GAP * 2.0, palette).into_any_element()
        } else {
            let (palette, shared, ui, scripture) = (palette.clone(), c.shared.clone(), c.ui.clone(), c.scripture.clone());
            uniform_list(if quran { "jannah-quran" } else { "jannah-hadith" }, count, move |range, _, _| {
                let m = shared.borrow();
                range
                    .filter_map(|i| rows.get(i).copied().map(|r| (i, r)))
                    .map(|(i, r)| match r {
                        Row::Surah(n, k) => surah_head(n, k, &palette, &scripture),
                        Row::Collection(col, k) => collection_head(col, k, &palette, &scripture),
                        Row::Item(id) => {
                            let at = list_ids.iter().position(|x| *x == id).unwrap_or_default();
                            item_row(("jannah-list-row", i), id, &m, &palette, &scripture, &shared, &ui, list_ids.clone(), at)
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .h(px(LIST_H - CHIPS_H - CONTROL_H - GAP * 2.0))
            .w_full()
            .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(div().flex().child(search_box("jannah-clear", &self.field, &query, palette)))
            .child(chips)
            .child(div().relative().child(body).when(pending, |el| el.child(div().absolute().top_0().left_0().right_0().flex().justify_center().child(caption(t("Searching…"), palette)))))
            .into_any_element()
    }

    /// The results of `query` (`None`: no query, or still searching),
    /// starting the search off the UI thread when the query changed.
    fn search(&self, shared: &Shared, query: &str, cx: &mut App) -> Option<Rc<[ItemId]>> {
        if query.is_empty() {
            self.ui.borrow_mut().searched = None;
            return None;
        }
        if let Some((q, res)) = &self.ui.borrow().searched
            && q == query
        {
            return res.clone();
        }
        let index = super::search_index(shared, cx)?;
        let generation = {
            let mut ui = self.ui.borrow_mut();
            ui.search_gen += 1;
            ui.searched = Some((query.to_string(), None));
            ui.search_gen
        };
        let q = query.to_string();
        let task = cx.background_spawn(async move { mandara_services::jannah::search::search(&index, &q) });
        let (ui, shared) = (self.ui.clone(), shared.clone());
        cx.spawn(async move |cx| {
            let hits: Rc<[ItemId]> = task.await.into();
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

// ── Today ────────────────────────────────────────────────────────────────

fn today(c: &Ctx, window: &mut Window, cx: &mut App) -> AnyElement {
    let s = settings(cx);
    let item = super::today(c.shared, &s, None).and_then(|id| catalog().get(id));
    let mut col = div().flex().flex_col().gap(px(GAP));
    match item {
        Some(item) => {
            let (prev, next) = {
                let (shared, ui) = (c.shared.clone(), c.ui.clone());
                let (shared2, ui2) = (c.shared.clone(), c.ui.clone());
                let prev: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
                    ui.borrow_mut().slide = Some((Instant::now(), -1.0));
                    nudge(&shared, -1, cx);
                });
                let next: Rc<dyn Fn(&mut App)> = Rc::new(move |cx| {
                    ui2.borrow_mut().slide = Some((Instant::now(), 1.0));
                    nudge(&shared2, 1, cx);
                });
                (prev, next)
            };
            col = col.child(hero(c, item, false, Some((prev, next)), window));
        }
        None => col = col.child(empty_state("arch", t("Nothing to show"), t("Choose Qur'an, hadith or both in Studio"), BODY_MIN, c.palette)),
    }
    col.child(step_card(c, window, cx)).into_any_element()
}

/// The verse's or hadith's words with a verse's ornate number.
fn words(item: &Item) -> String {
    match item.verse_ref() {
        Some((_, a)) => format!("{} {}", item.arabic, catalog::ayah_mark(a)),
        None => item.arabic.to_string(),
    }
}

/// The hero card: the arch with its crown, sides and base, dawn light and
/// the lattice; the Arabic, its meaning and the reference row. `nav` adds
/// previous / next in the top corners; `full` shows a hadith's chain too.
fn hero(c: &Ctx, item: &Item, full: bool, nav: Option<(Rc<dyn Fn(&mut App)>, Rc<dyn Fn(&mut App)>)>, window: &mut Window) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let tones = if palette.is_dark { HERO_DARK } else { HERO_LIGHT };
    let emerald = palette.success_emerald.mix(palette.primary, HERO_PRIMARY_SHARE);
    let (bg_a, bg_b) = (tone(emerald, tones[0]), tone(palette.tertiary.mix(emerald, HERO_PRIMARY_SHARE), tones[1]));
    let arch = g.with_alpha(alpha(ARCH_ALPHA, palette));
    // The crown spans the card inside the inset at its own aspect.
    let crown_w = INNER_W - ARCH_INSET * 2.0;
    let scale = crown_w / art::CROWN_VIEW.0;
    let crown_h = art::CROWN_VIEW.1 * scale;
    let stroke = 2.4 * scale;
    let side = |x: f32, w: f32, color: Color, bottom: f32| div().absolute().left(px(x)).top(px(ARCH_INSET + crown_h - 0.5)).bottom(px(bottom)).w(px(w)).bg(color.hsla());
    let outer_l = ARCH_INSET + art::CROWN_FOOT * scale - stroke / 2.0;
    let outer_r = INNER_W - outer_l - stroke;
    let inner_off = (art::CROWN_FOOT + art::CROWN_INNER) * scale - 1.3 * scale / 2.0;
    let inner = arch.with_alpha(arch.alpha() * ARCH_INNER_SHARE);
    let inner_w = 1.3 * scale;
    // Slide on previous / next.
    let mut offset = 0.0;
    let mut fade = 1.0;
    let slide = c.ui.borrow().slide;
    if let Some((at, dir)) = slide {
        let t_ = at.elapsed().as_secs_f32();
        let (x, _) = mandara_core::SpringConfig::GENTLE.solve(SLIDE_PX * dir, 0.0, t_);
        if at.elapsed() < SLIDE_FOR && x.abs() > SLIDE_REST_PX && !mandara_core::spring::motion_profile().reduce {
            offset = x;
            fade = 1.0 - (x.abs() / SLIDE_PX).min(1.0) * SLIDE_FADE;
            window.request_animation_frame();
        } else {
            c.ui.borrow_mut().slide = None;
        }
    }
    let m = c.shared.borrow();
    let meaning = if c.show_meaning { meaning_of(&m, item) } else { None };
    let fetch = m.fetch.clone();
    let saved = m.state.is_saved(&item.key);
    drop(m);
    let arabic = div()
        .w_full()
        .font_family(c.scripture.clone())
        .text_size(px(c.arabic_px))
        .line_height(gpui::relative(ARABIC_LINE_EM))
        .text_center()
        .text_color(palette.on_surface.hsla())
        .child(crate::bidi::paragraph(words(item)));
    let meaning_el: Option<AnyElement> = match (&meaning, c.show_meaning, item.is_verse()) {
        (Some(m), _, _) => Some(meaning_block(m, palette)),
        (None, true, true) => Some(meaning_status(c, &fetch, palette)),
        _ => None,
    };
    let chain = (full && !item.is_verse() && item.full_arabic() != item.arabic).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(tokens::XS))
            .child(caption(t("With its chain of narrators"), palette))
            .child(div().font_family(c.scripture.clone()).text_size(px(CHAIN_PX)).line_height(gpui::relative(ARABIC_LINE_EM)).text_right().text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(item.full_arabic())))
    });
    let actions = reference_row(c, item, meaning.as_ref().map(|m| m.text.clone()), saved);
    let content = div()
        .relative()
        .left(px(offset))
        .opacity(fade)
        .pt(px(ARCH_INSET + crown_h * CONTENT_TOP_SHARE))
        .px(px(ARCH_INSET + (art::CROWN_FOOT + art::CROWN_INNER) * scale + CONTENT_SIDE))
        .pb(px(ARCH_INSET + CONTENT_BOTTOM))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(BLOCK_GAP))
        .child(arabic)
        .when_some(meaning_el, |el, m| el.child(m))
        .when_some(chain, |el, ch| el.child(ch))
        .child(actions);
    let mut card = div()
        .relative()
        .w_full()
        .rounded(px(c.radius))
        .overflow_hidden()
        .bg(oklab_gradient(HERO_ANGLE_DEG, bg_a, bg_b))
        .shadow(rim(palette))
        // The faint lattice, fading downward.
        .child(svg().path(art::LATTICE_PATH).absolute().top_0().left_0().size(px(INNER_W)).text_color(g.with_alpha(alpha(LATTICE_ALPHA, palette)).hsla()))
        // Dawn light in the crown.
        .child(
            div()
                .absolute()
                .top(px(ARCH_INSET + crown_h * DAWN_TOP_SHARE - DAWN_H / 2.0))
                .left(px((INNER_W - DAWN_W) / 2.0))
                .w(px(DAWN_W))
                .h(px(DAWN_H))
                .rounded_full()
                .shadow(vec![glow(g.with_alpha(DAWN_GLOW.0), DAWN_GLOW.1, DAWN_GLOW.2, 0.0)]),
        )
        .child(svg().path(art::CROWN_PATH).absolute().top(px(ARCH_INSET)).left(px(ARCH_INSET)).w(px(crown_w)).h(px(crown_h)).text_color(arch.hsla()))
        .child(side(outer_l, stroke, arch, ARCH_INSET))
        .child(side(INNER_W - outer_l - stroke, stroke, arch, ARCH_INSET))
        .child(side(ARCH_INSET + inner_off, inner_w, inner, ARCH_INSET + art::CROWN_INNER * scale))
        .child(side(INNER_W - ARCH_INSET - inner_off - inner_w, inner_w, inner, ARCH_INSET + art::CROWN_INNER * scale))
        .child(div().absolute().left(px(outer_l)).w(px(outer_r - outer_l + stroke)).bottom(px(ARCH_INSET)).h(px(stroke)).bg(arch.hsla()))
        .child(content);
    if let Some((prev, next)) = nav {
        let ink = palette.on_surface_variant;
        // Reading order: "previous" sits at the start corner.
        let (start, end) = (crate::bidi::dir_icon("chevron-left"), crate::bidi::dir_icon("chevron-right"));
        card = card
            .child(div().absolute().top(px(ARCH_INSET)).inset_start(px(ARCH_INSET)).child(icon_button("jannah-prev", start, NAV_BTN, ink, None, move |_, cx| prev(cx))))
            .child(div().absolute().top(px(ARCH_INSET)).inset_end(px(ARCH_INSET)).child(icon_button("jannah-next", end, NAV_BTN, ink, None, move |_, cx| next(cx))));
    }
    card.into_any_element()
}

/// The meaning, its translator or source, and the AI label when it is one.
fn meaning_block(m: &meanings::Meaning, palette: &MandaraPalette) -> AnyElement {
    let foot: Option<AnyElement> = if m.ai {
        Some(
            div()
                .row()
                .items_center()
                .gap(px(tokens::XS))
                .child(icon("info", text::LABEL, palette.on_surface_variant))
                .child(caption(t("AI rendering of the meaning"), palette))
                .into_any_element(),
        )
    } else if !m.credit.is_empty() {
        Some(caption(format!("— {}", m.credit), palette).into_any_element())
    } else {
        None
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(tokens::XS))
        .child(div().w_full().text_center().text_size(px(text::BODY)).line_height(gpui::relative(MEANING_LINE_EM)).text_color(palette.on_surface.hsla()).child(crate::bidi::paragraph(m.text.clone())))
        .when_some(foot, |el, f| el.child(f))
        .into_any_element()
}

/// Why a verse shows no meaning (loading, offline, failed).
fn meaning_status(c: &Ctx, fetch: &Fetch, palette: &MandaraPalette) -> AnyElement {
    match fetch {
        Fetch::Offline => caption(t("Offline mode is on: the Arabic only"), palette).into_any_element(),
        Fetch::Failed(_) => {
            let shared = c.shared.clone();
            div()
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(caption(t("The meaning couldn't be loaded"), palette))
                .child(
                    div()
                        .id("jannah-retry")
                        .cursor_pointer()
                        .text_size(px(text::LABEL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(readable(palette.primary, palette).hsla())
                        .child(t("Try again"))
                        .on_click(move |_, _, cx| super::retry_meanings(&shared, cx)),
                )
                .into_any_element()
        }
        _ => caption(t("Loading the meaning…"), palette).into_any_element(),
    }
}

/// The reference chip (copies a citation), the link, save and copy.
fn reference_row(c: &Ctx, item: &Item, meaning: Option<String>, saved: bool) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let arabic = arabic_ui();
    let citation = catalog::citation(item, arabic);
    let copied = catalog::copy_text(item, meaning.as_deref(), arabic);
    let link = catalog::link(item);
    let key = item.key.clone();
    let (ui1, ui2, shared) = (c.ui.clone(), c.ui.clone(), c.shared.clone());
    let chip = div()
        .id("jannah-ref")
        .h(px(CHIP_H))
        .px(px(CHIP_PAD_X))
        .min_w(px(0.0))
        .rounded_full()
        .cursor_pointer()
        .bg(g.with_alpha(alpha(CHIP_FILL, palette)).hsla())
        .row()
        .items_center()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular_nums())
        .text_color(g.hsla())
        .child(div().truncate().child(chip_text(item)))
        .on_click(move |_, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(citation.clone()));
            ui1.borrow_mut().flash.show(true, t("Citation copied"));
        });
    let ink = palette.on_surface_variant;
    div()
        .w_full()
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .child(chip)
        .child(icon_button("jannah-link", "external", ACTION_BTN, ink, None, move |_, _| open_link(link.clone())))
        .child(div().flex_1())
        .child(icon_button("jannah-save", "heart", ACTION_BTN, if saved { g } else { ink }, saved.then(|| g.with_alpha(alpha(CHIP_FILL, palette))), move |_, cx| {
            toggle_saved(&shared, &key, cx);
        }))
        .child(icon_button("jannah-copy", "copy", ACTION_BTN, ink, None, move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()));
            ui2.borrow_mut().flash.show(true, t("Copied with its reference"));
        }))
        .into_any_element()
}

/// A round check: filled gold with a tick when `done`, a soft ring else.
pub(super) fn check(done: bool, side: f32, palette: &MandaraPalette) -> AnyElement {
    let g = gold(palette);
    div()
        .size(px(side))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .when(done, |el| el.bg(g.hsla()).child(icon("check", side * 0.55, palette.surface_container)))
        .when(!done, |el| el.bg(g.with_alpha(alpha(CHECK_FILL, palette)).hsla()).child(icon("check", side * 0.5, g.with_alpha(0.55))))
        .into_any_element()
}

/// The first words of a path's first evidence: its meaning in your
/// language, or the Arabic in Arabic (or when the meaning isn't there).
fn evidence_excerpt(c: &Ctx, p: &Path) -> Option<(String, bool, String)> {
    let id = catalog::evidence(p).into_iter().next()?;
    let item = catalog().get(id)?;
    let reference = catalog::reference(item, arabic_ui());
    let meaning = if c.show_meaning { meaning_of(&c.shared.borrow(), item) } else { None };
    Some(match meaning {
        Some(m) => (meanings::excerpt(&m.text), false, reference),
        None => {
            let mut a: String = item.arabic.chars().take(ARABIC_EXCERPT).collect();
            if item.arabic.chars().count() > ARABIC_EXCERPT {
                a = a.rsplit_once(' ').map(|(head, _)| head.to_string()).unwrap_or(a);
                a.push_str(" …");
            }
            (a, true, reference)
        }
    })
}

fn step_card(c: &Ctx, window: &mut Window, cx: &mut App) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let Some(p) = super::today_step() else { return div().into_any_element() };
    let date = Local::now().date_naive();
    let (done, week) = {
        let m = c.shared.borrow();
        (m.state.done_on(date), m.state.week(date, crate::plugins::clock::week_start_days(cx)))
    };
    let (_, group) = catalog::group_label(&p.group);
    let excerpt = evidence_excerpt(c, p);
    // The calm glow after ticking it.
    let glow_shadow = c.ui.borrow().glow.and_then(|at| {
        let k = 1.0 - at.elapsed().as_secs_f32() / STEP_GLOW_FOR.as_secs_f32();
        (k > 0.0).then_some(k)
    });
    if glow_shadow.is_some() {
        window.request_animation_frame();
    } else {
        c.ui.borrow_mut().glow = None;
    }
    let action: AnyElement = if done {
        div()
            .row()
            .items_center()
            .gap(px(tokens::SM))
            .child(check(true, CONTROL_H - tokens::SM, palette))
            .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(g.hsla()).child(t("May Allah accept it from you")))
            .into_any_element()
    } else {
        let (ui, shared) = (c.ui.clone(), c.shared.clone());
        pill_button("jannah-did", Some("check"), t("I did this today"), g, palette, move |_, cx| {
            ui.borrow_mut().glow = Some(Instant::now());
            super::mark_step_done(cx);
            refresh(&shared, cx);
        })
        .into_any_element()
    };
    let done_days = week.iter().filter(|(_, d)| *d).count();
    let dots = div().row().items_center().gap(px(DOT_GAP)).children(week.iter().map(|(d, on)| {
        let today = *d == date;
        let side = if today { DOT_TODAY_PX } else { DOT_PX };
        div()
            .size(px(side))
            .rounded_full()
            .when(*on, |el| el.bg(g.hsla()))
            .when(!*on, |el| el.bg(wash(DOT_OFF, palette).hsla()))
            .when(today && !*on, |el| el.bg(g.with_alpha(alpha(CHECK_FILL, palette)).hsla()))
    }));
    let week_line = if done_days > 0 { tf("{n} good steps this week", &[("n", &done_days)]) } else { t("Every day is a new beginning").to_string() };
    let mut card = glass_card(palette)
        .rounded(px(c.radius))
        .p(px(STEP_PAD))
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(
            div()
                .row()
                .items_center()
                .justify_between()
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::BOLD).text_color(g.hsla()).child(t("TODAY'S STEP")))
                .child(caption(t(group), palette)),
        )
        .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(t(p.title)))
        .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(t(p.line))))
        .when_some(excerpt, |el, (text_, is_arabic, reference)| {
            el.child(
                div()
                    .pl(px(tokens::SM))
                    .flex()
                    .flex_col()
                    .gap(px(LINE_GAP))
                    .child(if is_arabic {
                        div().font_family(c.scripture.clone()).text_size(px(text::BODY + 2.0)).line_height(gpui::relative(ARABIC_LINE_EM)).text_right().child(crate::bidi::paragraph(text_))
                    } else {
                        div().italic().text_size(px(text::BODY_S)).text_color(palette.on_surface.hsla()).child(crate::bidi::paragraph(format!("“{text_}”")))
                    })
                    .child(caption(reference, palette)),
            )
        })
        .child(div().pt(px(tokens::XS)).child(action))
        .child(div().pt(px(tokens::SM)).row().items_center().justify_between().child(dots).child(caption(week_line, palette).font_features(tabular_nums())));
    if let Some(k) = glow_shadow {
        let mut shadows = rim(palette);
        shadows.push(glow(g.with_alpha(STEP_GLOW.0 * k), STEP_GLOW.1, 0.0, 0.0));
        card = card.shadow(shadows);
    }
    card.into_any_element()
}

// ── Paths ────────────────────────────────────────────────────────────────

fn group_icon(g: &PathGroup) -> &'static str {
    match g {
        PathGroup::Worship => "crescent",
        PathGroup::Character => "sparkles",
        PathGroup::Family => "home",
        PathGroup::Others => "user",
        PathGroup::Heart => "heart",
        PathGroup::Avoid => "shield",
    }
}

fn paths(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let mut col = div().flex().flex_col().gap(px(GAP));
    for group in catalog::GROUPS.iter() {
        let list: Vec<&'static Path> = PATHS.iter().filter(|p| catalog::same_group(&p.group, group)).collect();
        if list.is_empty() {
            continue;
        }
        let (id, label) = catalog::group_label(group);
        let tint = if catalog::is_avoid(group) { palette.on_surface_variant } else { g };
        let rows = list.into_iter().enumerate().map(|(i, p)| {
            let (ui, shared) = (c.ui.clone(), c.shared.clone());
            let hover = wash(ROW_HOVER, palette);
            div()
                .id(SharedString::from(format!("jannah-path-{id}-{i}")))
                .px(px(PATH_ROW_PAD))
                .py(px(tokens::SM))
                .rounded(px(ROW_RADIUS))
                .cursor_pointer()
                .hover(move |st| st.bg(hover.hsla()))
                .row()
                .items_center()
                .gap(px(tokens::MD))
                .child(icon(group_icon(group), PATH_ICON, tint))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(t(p.title)))
                        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).line_clamp(2).child(crate::bidi::paragraph(t(p.line)))),
                )
                .child(icon(crate::bidi::dir_icon("chevron-right"), PATH_ICON, palette.on_surface_variant))
                .on_click(move |_, _, cx| {
                    ui.borrow_mut().stack.push(Sheet::Path(p));
                    refresh(&shared, cx);
                })
        });
        col = col.child(section(t(label), palette, Some(caption(tf("{n} paths", &[("n", &PATHS.iter().filter(|p| catalog::same_group(&p.group, group)).count())]), palette).font_features(tabular_nums()).into_any_element()))).child(
            glass_card(palette).rounded(px(c.radius)).p(px(tokens::XS)).flex().flex_col().children(rows),
        );
    }
    col.into_any_element()
}

// ── Lists ────────────────────────────────────────────────────────────────

/// Items grouped under their surah (verses) or collection (hadith).
fn group_rows(items: &[&Item], quran: bool) -> Vec<Row> {
    let mut rows = Vec::with_capacity(items.len() + 16);
    let mut i = 0;
    while i < items.len() {
        let key = |it: &Item| if quran { it.verse_ref().map(|(s, _)| s.to_string()).unwrap_or_default() } else { it.hadith().map(|h| h.collection.id().to_string()).unwrap_or_default() };
        let k = key(items[i]);
        let n = items[i..].iter().take_while(|it| key(it) == k).count();
        if quran {
            rows.push(Row::Surah(items[i].verse_ref().map(|(s, _)| s).unwrap_or_default(), n));
        } else {
            if let Some(h) = items[i].hadith() {
                rows.push(Row::Collection(h.collection, n));
            }
        }
        rows.extend(items[i..i + n].iter().map(|it| Row::Item(it.id)));
        i += n;
    }
    rows
}

fn surah_head(n: u16, count: usize, palette: &MandaraPalette, scripture: &SharedString) -> AnyElement {
    let (name, arabic) = catalog::surah_names(n);
    let meaning = mandara_services::jannah::data::surah(n).map(|s| s.meaning.clone()).unwrap_or_default();
    let g = gold(palette);
    div()
        .h(px(ROW_H))
        .px(px(ROW_PAD_X))
        .row()
        .items_end()
        .pb(px(tokens::SM))
        .gap(px(tokens::MD))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::BOLD).font_features(tabular_nums()).text_color(g.hsla()).child(n.to_string()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(name))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(if arabic_ui() { tf("{n} verses", &[("n", &count)]) } else { format!("{meaning} · {}", tf("{n} verses", &[("n", &count)])) })),
        )
        .child(div().font_family(scripture.clone()).text_size(px(HEAD_ARABIC_PX)).text_color(g.hsla()).child(arabic))
        .into_any_element()
}

fn collection_head(col: Collection, count: usize, palette: &MandaraPalette, scripture: &SharedString) -> AnyElement {
    let g = gold(palette);
    let (name, arabic) = catalog::collection_name(col);
    div()
        .h(px(ROW_H))
        .px(px(ROW_PAD_X))
        .row()
        .items_end()
        .pb(px(tokens::SM))
        .gap(px(tokens::MD))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(t(name)))
                .child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(tf("{n} hadith", &[("n", &count)]))),
        )
        .child(div().font_family(scripture.clone()).text_size(px(HEAD_ARABIC_PX)).text_color(g.hsla()).child(arabic))
        .into_any_element()
}

/// One item: its Arabic on one line, its reference and meaning below; a
/// click opens the reader on `list` at `at`.
#[expect(clippy::too_many_arguments, reason = "a row needs the model, the theme, the font and where it opens; bundling them would only rename the list")]
fn item_row(
    id: impl Into<gpui::ElementId>,
    item_id: ItemId,
    m: &super::Model,
    palette: &MandaraPalette,
    scripture: &SharedString,
    shared: &Shared,
    ui: &Rc<RefCell<Ui>>,
    list: Rc<[ItemId]>,
    at: usize,
) -> AnyElement {
    let Some(item) = catalog().get(item_id) else { return div().h(px(ROW_H)).into_any_element() };
    let g = gold(palette);
    let meaning = meaning_of(m, item).filter(|_| !arabic_ui()).map(|x| x.text);
    let hover = wash(ROW_HOVER, palette);
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
        // One right-aligned line of Arabic, laid out right to left (the bidi
        // paragraph), the rest of it clipped.
        .child(
            div()
                .w_full()
                .h(px(LIST_ARABIC_LINE_PX))
                .overflow_hidden()
                .font_family(scripture.clone())
                .text_size(px(LIST_ARABIC_PX))
                .line_height(px(LIST_ARABIC_LINE_PX))
                .text_right()
                .child(crate::bidi::paragraph(item.arabic.to_string())),
        )
        .child(
            div()
                .w_full()
                .min_w(px(0.0))
                .row()
                .items_center()
                .gap(px(tokens::SM))
                .child(div().flex_none().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(g.hsla()).child(catalog::reference(item, arabic_ui())))
                .when_some(meaning, |el, mm| el.child(div().flex_1().min_w(px(0.0)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(mm))),
        )
        .on_click(move |_, _, cx| {
            ui.borrow_mut().stack.push(Sheet::Reader { list: list.clone(), at });
            refresh(&shared, cx);
        })
        .into_any_element()
}

/// Theme chips (and the collection chips on the Hadith tab).
fn filter_chips(c: &Ctx, quran: bool) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let (theme, collection) = {
        let ui = c.ui.borrow();
        (ui.theme, ui.collection)
    };
    let strip = { let u = c.ui.borrow(); if quran { u.chips_q.clone() } else { u.chips_h.clone() } };
    let chip = |id: SharedString, label: SharedString, active: bool, set: Rc<dyn Fn(&mut Ui)>| {
        let (ui, shared, strip) = (c.ui.clone(), c.shared.clone(), strip.clone());
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
    let mut chips: Vec<AnyElement> = Vec::new();
    if !quran {
        for (key, label) in [(None, tk("All")), (Some("bukhari"), tk("Bukhari")), (Some("muslim"), tk("Muslim"))] {
            chips.push(chip(format!("jannah-col-{}", key.unwrap_or("all")).into(), t(label).into(), collection == key, Rc::new(move |u: &mut Ui| u.collection = key)));
        }
        chips.push(div().w(px(1.0)).h(px(CHIPS_H * 0.6)).flex_none().bg(wash(WASH_SOFT, palette).hsla()).into_any_element());
    }
    chips.push(chip("jannah-theme-all".into(), t("All topics").into(), theme.is_none(), Rc::new(|u: &mut Ui| u.theme = None)));
    for (id, label) in THEMES {
        chips.push(chip(format!("jannah-theme-{id}").into(), t(label).into(), theme == Some(id), Rc::new(move |u: &mut Ui| u.theme = if u.theme == Some(id) { None } else { Some(id) })));
    }
    let id = if quran { "jannah-chips-q" } else { "jannah-chips-h" };
    strip.with_chevrons(id, strip.row(id).row().items_center().gap(px(CHIP_GAP)).children(chips), CHIPS_H, palette)
}

/// The themes with their chip labels (catalog keys).
const THEMES: [(&str, &str); 10] = [
    ("description", tk("What it is like")),
    ("deeds", tk("Deeds")),
    ("barriers", tk("What keeps from it")),
    ("people", tk("Its people")),
    ("mercy", tk("Mercy")),
    ("levels", tk("Levels")),
    ("vision", tk("Seeing Allah")),
    ("eternity", tk("Forever")),
    ("family", tk("Family and kin")),
    ("adam", tk("Adam")),
];

// ── Saved ────────────────────────────────────────────────────────────────

fn saved(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let m = c.shared.borrow();
    let ids: Vec<ItemId> = m.state.saved.iter().filter_map(|s| catalog().find(&s.key)).collect();
    if ids.is_empty() {
        return empty_state("heart", t("Nothing saved yet"), t("Tap the heart on a verse or hadith to keep it here"), BODY_MIN, palette).into_any_element();
    }
    let list: Rc<[ItemId]> = ids.clone().into();
    glass_card(palette)
        .rounded(px(c.radius))
        .p(px(tokens::XS))
        .flex()
        .flex_col()
        .children(ids.iter().enumerate().map(|(i, id)| item_row(("jannah-saved", i), *id, &m, palette, &c.scripture, c.shared, c.ui, list.clone(), i)))
        .into_any_element()
}

// ── Sheets ───────────────────────────────────────────────────────────────

/// The back button and a title over a sheet.
fn sheet_head(c: &Ctx, title: SharedString, trailing: Option<AnyElement>) -> AnyElement {
    let (ui, shared) = (c.ui.clone(), c.shared.clone());
    div()
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon_button("jannah-back", crate::bidi::dir_icon("chevron-left"), NAV_BTN, c.palette.on_surface_variant, None, move |_, cx| {
            ui.borrow_mut().stack.pop();
            refresh(&shared, cx);
        }))
        .child(div().flex_1().min_w(px(0.0)).text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
        .when_some(trailing, |el, t_| el.child(t_))
        .into_any_element()
}

fn sheet(c: &Ctx, window: &mut Window) -> AnyElement {
    enum Top {
        Reader(Rc<[ItemId]>, usize),
        Path(&'static Path),
        About,
    }
    let top = match c.ui.borrow().stack.last() {
        Some(Sheet::Reader { list, at }) => Top::Reader(list.clone(), *at),
        Some(Sheet::Path(p)) => Top::Path(p),
        Some(Sheet::About) | None => Top::About,
    };
    match top {
        Top::Reader(list, at) => reader(c, list, at, window),
        Top::Path(p) => path_sheet(c, p),
        Top::About => about(c),
    }
}

fn reader(c: &Ctx, list: Rc<[ItemId]>, at: usize, window: &mut Window) -> AnyElement {
    let at = at.min(list.len().saturating_sub(1));
    let Some(item) = list.get(at).and_then(|id| catalog().get(*id)) else { return div().into_any_element() };
    let n = list.len();
    let position = (n > 1).then(|| caption(tf("{n} of {total}", &[("n", &(at + 1)), ("total", &n)]), c.palette).font_features(tabular_nums()).into_any_element());
    let nav = (n > 1).then(|| {
        let go = |step: isize, ui: Rc<RefCell<Ui>>, shared: Shared| -> Rc<dyn Fn(&mut App)> {
            Rc::new(move |cx| {
                if let Some(Sheet::Reader { list, at }) = ui.borrow_mut().stack.last_mut() {
                    *at = (*at as isize + step).rem_euclid(list.len() as isize) as usize;
                }
                ui.borrow_mut().slide = Some((Instant::now(), step as f32));
                refresh(&shared, cx);
            })
        };
        (go(-1, c.ui.clone(), c.shared.clone()), go(1, c.ui.clone(), c.shared.clone()))
    });
    div()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(sheet_head(c, catalog::reference(item, arabic_ui()).into(), position))
        .child(hero(c, item, true, nav, window))
        .into_any_element()
}

fn path_sheet(c: &Ctx, p: &'static Path) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let (_, group) = catalog::group_label(&p.group);
    let ids: Rc<[ItemId]> = catalog::evidence(p).into();
    let m = c.shared.borrow();
    div()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(sheet_head(c, t(p.title).into(), None))
        .child(
            glass_card(palette)
                .rounded(px(c.radius))
                .p(px(STEP_PAD))
                .flex()
                .flex_col()
                .gap(px(tokens::SM))
                .child(div().row().items_center().gap(px(tokens::SM)).child(icon(group_icon(&p.group), PATH_ICON, g)).child(caption(t(group), palette)))
                .child(div().text_size(px(text::BODY)).child(crate::bidi::paragraph(t(p.line)))),
        )
        .child(section(t("EVIDENCE"), palette, Some(caption(ids.len().to_string(), palette).font_features(tabular_nums()).into_any_element())))
        .child(glass_card(palette).rounded(px(c.radius)).p(px(tokens::XS)).flex().flex_col().children(ids.iter().enumerate().map(|(i, id)| item_row(("jannah-evidence", i), *id, &m, palette, &c.scripture, c.shared, c.ui, ids.clone(), i))))
        .into_any_element()
}

fn about(c: &Ctx) -> AnyElement {
    let palette = c.palette;
    let g = gold(palette);
    let lang = mandara_i18n::language();
    let translator = meanings::edition(lang.code()).map(|e| e.translator);
    let para = |s: String| div().text_size(px(text::BODY_S)).line_height(gpui::relative(MEANING_LINE_EM)).child(crate::bidi::paragraph(s));
    let link = |id: &'static str, label: &'static str, url: &'static str| pill_button(id, Some("external"), label, g, palette, move |_, _| open_link(url.to_string()));
    let ai = meanings::AI_LANGS.contains(&lang.code());
    div()
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(sheet_head(c, t("About Jannah").into(), None))
        .child(
            glass_card(palette)
                .rounded(px(c.radius))
                .p(px(STEP_PAD))
                .flex()
                .flex_col()
                .gap(px(tokens::MD))
                .child(para(t("The Qur'an's text is the Tanzil Project's Uthmani text, copied exactly.").into()))
                .child(div().row().gap(px(tokens::SM)).child(link("jannah-tanzil", "tanzil.net", "https://tanzil.net")))
                .when_some(translator, |el, who| {
                    el.child(para(tf("Meanings of the Qur'an: {translator}, from Tanzil through api.alquran.cloud, downloaded once and kept on this computer.", &[("translator", &who)])))
                })
                .child(para(t("Hadith: Sahih al-Bukhari and Sahih Muslim from the public-domain hadith-api collection, with its translations.").into()))
                .child(div().row().gap(px(tokens::SM)).child(link("jannah-hadith-api", "hadith-api", "https://github.com/fawazahmed0/hadith-api")))
                .when(ai, |el| {
                    el.child(
                        div()
                            .row()
                            .gap(px(tokens::SM))
                            .child(icon("info", text::BODY, palette.on_surface_variant))
                            .child(div().flex_1().child(para(t("In your language, the hadith meanings are AI renderings of the meaning, not published translations.").into()))),
                    )
                })
                .child(caption(t("Verses and hadith are shown exactly as in their sources. When in doubt, check a reference with a scholar or a printed edition."), palette)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::jannah::Theme;

    #[test]
    fn lists_group_under_their_surah_or_collection() {
        let c = catalog();
        let verses: Vec<&Item> = c.verse_ids().filter_map(|id| c.get(id)).collect();
        let rows = group_rows(&verses, true);
        let heads = rows.iter().filter(|r| matches!(r, Row::Surah(..))).count();
        let surahs: std::collections::BTreeSet<u16> = verses.iter().filter_map(|v| v.verse_ref().map(|(s, _)| s)).collect();
        assert_eq!(heads, surahs.len());
        assert_eq!(rows.len(), verses.len() + heads);
        assert!(matches!(rows[0], Row::Surah(_, n) if n >= 1));
        let hadith: Vec<&Item> = c.hadith_ids().filter_map(|id| c.get(id)).collect();
        let rows = group_rows(&hadith, false);
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Collection(..))).count(), 2);
    }

    #[test]
    fn the_panel_opens_at_a_sane_height() {
        const { assert!(BODY_GUESS < BODY_MAX) };
        const { assert!(LIST_H > CHIPS_H + CONTROL_H + GAP * 2.0 + ROW_H * 4.0) };
        assert_eq!(THEMES.len(), Theme::ALL.len());
        assert!(Theme::ALL.iter().all(|th| THEMES.iter().any(|(id, _)| *id == th.id())));
    }
}
