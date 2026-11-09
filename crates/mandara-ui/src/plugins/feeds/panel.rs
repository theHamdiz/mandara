//! The Feeds bar item and panel: a sidebar of everything, saved articles,
//! folders and feeds; the timeline as virtualized article cards with
//! source icons, relative times, unread dots and thumbnails; search and
//! an unread filter; and the reader, all keyboard-driven.

use std::cell::Cell;
use std::rc::Rc;

use chrono::TimeZone as _;
use gpui::{
    AnyElement, App, AppContext as _, Entity, FocusHandle, FontWeight, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, ObjectFit, ParentElement, ScrollStrategy, SharedString, StyledImage as _,
    StatefulInteractiveElement, Styled, Window, div, img, prelude::FluentBuilder as _, px, uniform_list,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_plugins::PanelCx;
use mandara_theme::{Color, MandaraPalette};

use super::super::extras::{CONTROL_H, Segment, monogram, name_color, search_box, segmented, toast};
use super::super::panel_fx::{WASH_FAINT, WASH_SOFT, alpha, empty_state, header, pill_button, readable, wash};
use super::images::{self, Fit};
use super::library::{Filter, FeedRow, Item, Scope};
use super::reader::{self, Body};
use super::{BarStyle, Pictures, cached_settings, hub};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{icon_button, metrics, reveal, text, tokens};
use crate::text_field::TextField;

// ── Layout ───────────────────────────────────────────────────────────────

const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const SIDEBAR_W: f32 = 200.0;
const SIDE_ROW_H: f32 = 30.0;
const SIDE_ROW_RADIUS: f32 = 10.0;
const SIDE_INDENT: f32 = 14.0;
const SIDE_ICON: f32 = 14.0;
const SIDE_GAP: f32 = 2.0;
const SIDE_SECTION_GAP: f32 = 10.0;
const FAVICON: f32 = 16.0;
const COUNT_PAD_X: f32 = 6.0;
const COUNT_H: f32 = 18.0;
/// Article cards: their height (gap included), padding and thumbnail.
const CARD_H: f32 = 96.0;
const CARD_GAP: f32 = 8.0;
const CARD_PAD_X: f32 = 14.0;
const CARD_PAD_Y: f32 = 11.0;
const CARD_RADIUS: f32 = 14.0;
const CARD_ACCENT_W: f32 = 3.0;
const THUMB: f32 = 70.0;
const THUMB_RADIUS: f32 = 10.0;
const UNREAD_DOT: f32 = 7.0;
const META_GAP: f32 = 6.0;
const TITLE_PX: f32 = 13.5;
const LINE_GAP: f32 = 3.0;
const SAVED_ICON: f32 = 13.0;
const TOOL_BTN: f32 = 30.0;
const ERROR_ICON: f32 = 14.0;
const READER_PAD: f32 = 4.0;
/// Bar item: icon and badge.
const BAR_ICON_PX: f32 = 16.0;
const BADGE_H: f32 = 12.0;
const BADGE_PAD_X: f32 = 3.0;
const BADGE_TEXT: f32 = 8.0;
/// The badge tucks this far over the icon's corner.
const BADGE_OVERLAP: f32 = -5.0;
/// The dot style's dot sits this far over the icon's corner.
const DOT_OFFSET: f32 = -2.0;
const BAR_DOT: f32 = 6.0;
const BAR_GAP: f32 = 5.0;
/// Counts above this show as "99+".
const MAX_SHOWN_COUNT: usize = 99;

const SELECTED_FILL: (f32, f32) = (0.16, 0.12);
const READ_ALPHA: f32 = 0.62;
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 3600;
const SECS_PER_DAY: i64 = 86_400;
/// Relative days are shown up to a week; older articles show their date.
const RELATIVE_DAYS: i64 = 7;

/// How long ago `ts` was, compactly (`now`, `5m`, `3h`, `2d`, a date).
pub(super) fn ago(ts: i64, now: i64) -> String {
    let secs = (now - ts).max(0);
    match secs {
        ..SECS_PER_MIN => t("now").to_string(),
        SECS_PER_MIN..SECS_PER_HOUR => tf("{n}m", &[("n", &(secs / SECS_PER_MIN))]),
        SECS_PER_HOUR..SECS_PER_DAY => tf("{n}h", &[("n", &(secs / SECS_PER_HOUR))]),
        _ if secs < RELATIVE_DAYS * SECS_PER_DAY => tf("{n}d", &[("n", &(secs / SECS_PER_DAY))]),
        _ => chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format(t("%a %-d %b")).to_string()).unwrap_or_default(),
    }
}

/// A feed's accent: your colour, or a stable one from its name.
fn accent(row: &FeedRow, palette: &MandaraPalette) -> Color {
    Color::from_hex(&row.sub.color).map(|c| readable(c, palette)).unwrap_or_else(|_| name_color(row.name(), palette))
}

fn count_text(n: usize) -> SharedString {
    if n > MAX_SHOWN_COUNT { SharedString::from(format!("{MAX_SHOWN_COUNT}+")) } else { SharedString::from(n.to_string()) }
}

/// The feed's icon (when pictures are on and it has one) or a monogram.
fn favicon(row: &FeedRow, side: f32, pictures: Pictures, images: &images::Shared, palette: &MandaraPalette, scale: f32) -> AnyElement {
    let tint = accent(row, palette);
    if pictures != Pictures::Off && let Some(src) = row.snapshot.as_ref().and_then(|s| s.feed.icon.as_deref()) {
        let edge = (side * scale).round() as u32;
        if let Some(image) = images.borrow_mut().get(src, edge, edge, Fit::Fill) {
            return img(ImageSource::Render(image)).size(px(side)).flex_none().rounded(px(crate::style::radius(side / 4.0))).object_fit(ObjectFit::Cover).into_any_element();
        }
    }
    let initial: String = row.name().chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
    monogram(&initial, tint, side)
}

// ── Bar ──────────────────────────────────────────────────────────────────

/// The bar item: the feed icon with the unread count in the chosen style.
pub(super) fn bar(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    hub().borrow().views.track(window);
    super::follow_background(cx);
    let style = cached_settings(cx).bar;
    let (unread, _) = super::unread_total(cx);
    let glyph = icon("rss", crate::style::bar_icon(BAR_ICON_PX), if unread > 0 { readable(palette.primary, palette) } else { palette.on_surface });
    let accent = readable(palette.primary, palette);
    match style {
        BarStyle::Count => div().row().items_center().gap(px(BAR_GAP)).child(glyph)
            .when(unread > 0, |el| el.child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(crate::fonts::tabular_nums()).child(count_text(unread))))
            .into_any_element(),
        BarStyle::Icon => div().relative().child(glyph)
            .when(unread > 0, |el| el.child(div().absolute().top(px(DOT_OFFSET)).right(px(DOT_OFFSET)).size(px(BAR_DOT)).rounded_full().bg(accent.hsla()).border_1().border_color(crate::shell::frame_color(palette).hsla())))
            .into_any_element(),
        // A superscript pill that overlaps the icon's corner inside the
        // item's own box (the bar clips anything above it).
        BarStyle::Badge => div().row().items_start().child(glyph)
            .when(unread > 0, |el| el.child(
                div().ml(px(BADGE_OVERLAP)).h(px(BADGE_H)).min_w(px(BADGE_H)).px(px(BADGE_PAD_X)).rounded_full()
                    .bg(accent.hsla()).text_color(palette.on_primary.hsla()).text_size(px(BADGE_TEXT)).font_weight(FontWeight::BOLD).font_features(crate::fonts::tabular_nums())
                    .flex().items_center().justify_center().border_1().border_color(crate::shell::frame_color(palette).hsla())
                    .child(count_text(unread)),
            ))
            .into_any_element(),
    }
}

// ── Panel ────────────────────────────────────────────────────────────────

/// One panel instance (each bar has its own search field and focus).
pub(super) struct View {
    context: PanelCx,
    search: Entity<TextField>,
    focus: FocusHandle,
    shown: Cell<bool>,
}

impl View {
    pub fn new(context: PanelCx, cx: &mut App) -> Self {
        let search = cx.new(|cx| TextField::new(String::new(), t("Search articles…"), true, Rc::new(|_, _, cx| {
            let hub = hub();
            hub.borrow_mut().ui.selected = None;
            let views = hub.borrow().views.clone();
            views.notify(cx);
        }), cx));
        Self { context, search, focus: cx.focus_handle(), shown: Cell::new(false) }
    }

    pub fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let hub = hub();
        hub.borrow().views.track(window);
        // Visible: keep (or load) the history and refresh what is due. Take
        // the keyboard as the panel opens, so the arrows work at once.
        if self.context.progress() > 0.0 {
            if !self.shown.replace(true) { self.focus.focus(window, cx); }
            super::opened(cx);
        } else if self.shown.replace(false) {
            super::closed(cx);
        }
        let settings = cached_settings(cx);
        let query = self.search.read(cx).value.clone();
        let accent = readable(palette.primary, palette);
        let unread = super::unread_total(cx).0;
        let refreshing = hub.borrow().refreshing;
        let subtitle: SharedString = match refreshing {
            Some((done, total)) => tf("Refreshing {done} of {total}…", &[("done", &done), ("total", &total)]).into(),
            None if settings.subs.is_empty() => t("Your news, together").into(),
            None if unread == 0 => t("All caught up").into(),
            None => tf("{count} unread", &[("count", &unread)]).into(),
        };
        let tools = div().row().items_center().gap(px(tokens::XS))
            .child(icon_button("feeds-refresh", "refresh", TOOL_BTN, if refreshing.is_some() { accent } else { palette.on_surface_variant }, None, |_, cx| super::refresh_all(cx)))
            .child(icon_button("feeds-all-read", "list-check", TOOL_BTN, palette.on_surface_variant, None, |_, cx| super::mark_scope_read(cx)))
            .child(icon_button("feeds-settings", "sliders", TOOL_BTN, palette.on_surface_variant, None, |_, cx| crate::state::open_studio_at(Some("plugins/feeds/settings".into()), cx)));
        let body = if settings.subs.is_empty() { welcome(palette) } else { self.body(&settings, &query, palette, window, cx) };
        let flash = hub.borrow().flash.clone();
        let flash = flash.current(window, cx);
        let focus = self.focus.clone();
        let search = self.search.clone();
        div().id("feeds-panel").size_full().p(px(PAD)).flex().flex_col().relative()
            .track_focus(&self.focus)
            .on_key_down(move |event: &KeyDownEvent, window, cx| keys(event, &focus, &search, window, cx))
            .child(div().flex_none().child(reveal(&self.context, 0, header("rss", accent, t("Feeds"), subtitle, palette, Some(tools.into_any_element())))))
            .child(div().flex_none().h(px(GAP)))
            .child({ let r = self.context.reveal(1); div().flex_1().min_h(px(0.0)).relative().top(px(r.offset)).opacity(r.opacity).child(div().size_full().child(body)) })
            .when_some(flash, |el, (ok, message)| el.child(toast(ok, message, PAD, palette)))
            .into_any_element()
    }

    fn body(&self, settings: &super::Settings, query: &str, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let hub = hub();
        let scale = window.scale_factor();
        let images = hub.borrow().images.clone();
        images.borrow_mut().frame();
        let sidebar = sidebar(settings.pictures, &images, palette, scale);
        let reading = hub.borrow().ui.reading.as_ref().map(|r| (r.url.clone(), r.link.clone()));
        let main = match reading {
            Some((url, link)) => reader_pane(&url, &link, settings.pictures, &images, palette, window, cx),
            None => self.list_pane(settings, query, &images, palette, cx),
        };
        images::start(&images, hub.borrow().views.clone(), cx);
        div().size_full().row().gap(px(GAP)).child(sidebar).child(div().flex_1().min_w(px(0.0)).h_full().child(main)).into_any_element()
    }

    fn list_pane(&self, settings: &super::Settings, query: &str, images: &images::Shared, palette: &MandaraPalette, cx: &mut App) -> AnyElement {
        let hub = hub();
        let (scope, filter) = { let h = hub.borrow(); (h.ui.scope.clone(), h.ui.filter) };
        let filters = segmented("feeds-filter", [(Filter::All, t("All")), (Filter::Unread, t("Unread"))].into_iter().map(|(value, label)| Segment {
            label: label.into(), icon: None, active: filter == value,
            on_click: Rc::new(move |_, cx| { let hub = super::hub(); { let mut h = hub.borrow_mut(); h.ui.filter = value; h.ui.selected = None; } let views = hub.borrow().views.clone(); views.notify(cx); }),
        }).collect(), palette);
        let controls = div().row().items_center().gap(px(tokens::SM)).child(search_box("feeds-search-clear", &self.search, query, palette, cx)).child(div().w(px(SEGMENT_W)).flex_none().child(filters));
        let mut pane = div().size_full().flex().flex_col().gap(px(tokens::SM)).child(controls);
        let (items, loading, problem) = {
            let mut h = hub.borrow_mut();
            let loading = h.lib.is_none();
            let refreshing = h.refreshing.is_some();
            match &mut h.lib {
                Some(lib) => {
                    let items = lib.list(&scope, filter, query);
                    let problem = match &scope {
                        Scope::Feed(url) => lib.feed_index(url).and_then(|i| lib.feeds[i].error().map(|e| (e.to_owned(), lib.feeds[i].snapshot.as_ref().map_or(0, |s| s.retry_at)))),
                        _ => {
                            let failing: Vec<&str> = lib.feeds.iter().filter(|row| row.error().is_some()).map(FeedRow::name).collect();
                            (!failing.is_empty()).then(|| (tf("Could not refresh {feeds}; trying again later", &[("feeds", &failing.join(", "))]), 0))
                        }
                    };
                    (items, refreshing && lib.timeline().is_empty(), problem)
                }
                None => (Rc::from([]), loading, None),
            }
        };
        if let Some((problem, retry_at)) = &problem {
            let warn = readable(palette.warning_amber, palette);
            let retry = (*retry_at > chrono::Utc::now().timestamp()).then(|| tf("Trying again at {time}", &[("time", &chrono::Local.timestamp_opt(*retry_at, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default())]));
            pane = pane.child(div().flex_none().px(px(tokens::MD)).py(px(tokens::SM)).rounded(px(crate::style::radius(SIDE_ROW_RADIUS))).bg(warn.with_alpha(alpha(SELECTED_FILL, palette)).hsla())
                .row().items_center().gap(px(tokens::SM)).text_size(px(text::LABEL)).text_color(warn.hsla())
                .child(icon("alert", ERROR_ICON, warn))
                .child(div().flex_1().min_w(px(0.0)).child(crate::bidi::paragraph(match retry { Some(retry) => format!("{problem} · {retry}"), None => problem.clone() }))));
        }
        let height = LIST_GUESS_H;
        let content = if items.is_empty() {
            let (glyph, title, hint) = if loading {
                ("rss", t("Opening your feeds…").to_string(), t("Articles you already have show at once; new ones follow.").to_string())
            } else if !query.trim().is_empty() {
                ("search", t("No matches").to_string(), tf("Nothing here matches “{query}”", &[("query", &query.trim())]))
            } else if scope == Scope::Starred {
                ("bookmark", t("Nothing saved yet").to_string(), t("Save articles to read them later; they stay even after they leave their feed.").to_string())
            } else if filter == Filter::Unread {
                ("check", t("All caught up").to_string(), t("No unread articles here.").to_string())
            } else if problem.is_some() {
                ("alert", t("This feed could not be read").to_string(), t("Check its address in Studio, or try again in a moment.").to_string())
            } else {
                ("rss", t("No articles yet").to_string(), t("Refresh to fetch the latest articles.").to_string())
            };
            div().flex_1().min_h(px(0.0)).flex().flex_col().items_center().justify_center().gap(px(tokens::SM))
                .child(empty_state(glyph, title, hint, height * EMPTY_SHARE, palette))
                .when(problem.is_some() && !loading, |el| el.child(pill_button("feeds-retry", Some("refresh"), t("Try again"), readable(palette.primary, palette), palette, |_, cx| super::refresh_all(cx))))
                .into_any_element()
        } else {
            self.cards(items, settings.pictures, images, palette, cx)
        };
        pane.child(div().flex_1().min_h(px(0.0)).child(content)).into_any_element()
    }

    fn cards(&self, items: Rc<[Item]>, pictures: Pictures, images: &images::Shared, palette: &MandaraPalette, _cx: &mut App) -> AnyElement {
        let hub = hub();
        let handle = hub.borrow().ui.list_scroll.clone();
        let selected = hub.borrow().ui.selected;
        let palette = palette.clone();
        let images = images.clone();
        let lang = mandara_i18n::language();
        let count = items.len();
        uniform_list("feeds-cards", count, move |range, window, cx| {
            let _lang = mandara_i18n::scoped(Some(lang));
            let scale = window.scale_factor();
            let now = chrono::Utc::now().timestamp();
            let hub = super::hub();
            let rows: Vec<AnyElement> = {
                let h = hub.borrow();
                let Some(lib) = h.lib.as_ref() else { return Vec::new() };
                range.filter_map(|index| items.get(index).copied().map(|item| (index, item)))
                    .map(|(index, item)| card(lib, item, index, selected == Some(index), now, pictures, &images, &palette, scale))
                    .collect()
            };
            images::start(&images, hub.borrow().views.clone(), cx);
            rows
        })
        .track_scroll(&handle)
        .size_full()
        .into_any_element()
    }
}

/// The filter's width beside the search box.
const SEGMENT_W: f32 = 150.0;
/// The list's expected height (empty states centre in a share of it).
const LIST_GUESS_H: f32 = 420.0;
const EMPTY_SHARE: f32 = 0.6;

/// One article card.
fn card(lib: &super::Library, item: Item, index: usize, selected: bool, now: i64, pictures: Pictures, images: &images::Shared, palette: &MandaraPalette, scale: f32) -> AnyElement {
    let Some((row, entry)) = lib.entry(item) else { return div().h(px(CARD_H)).into_any_element() };
    let read = lib.is_read(item);
    let saved = lib.is_starred(item);
    let tint = accent(row, palette);
    let fill = if selected { readable(palette.primary, palette).with_alpha(alpha(SELECTED_FILL, palette)) } else { wash(WASH_FAINT, palette) };
    let hover = wash(WASH_SOFT, palette);
    let fg = if read { palette.on_surface.with_alpha(READ_ALPHA) } else { palette.on_surface };
    let thumb = (pictures != Pictures::Off).then_some(entry.image.as_deref()).flatten().map(|src| {
        let edge = (THUMB * scale).round() as u32;
        match images.borrow_mut().get(src, edge, edge, Fit::Fill) {
            Some(image) => img(ImageSource::Render(image)).size(px(THUMB)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS))).object_fit(ObjectFit::Cover).into_any_element(),
            None => div().size(px(THUMB)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS))).bg(wash(WASH_SOFT, palette).hsla()).into_any_element(),
        }
    });
    let meta = div().row().items_center().gap(px(META_GAP)).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla())
        .when(!read, |el| el.child(div().size(px(UNREAD_DOT)).flex_none().rounded_full().bg(tint.hsla())))
        .child(favicon(row, FAVICON - 2.0, pictures, images, palette, scale))
        .child(div().min_w(px(0.0)).truncate().font_weight(FontWeight::MEDIUM).text_color(tint.hsla()).child(row.name().to_owned()))
        .child("·")
        .child(div().flex_none().font_features(crate::fonts::tabular_nums()).child(ago(entry.when(), now)))
        .child(div().flex_1())
        .when(saved, |el| el.child(icon("bookmark", SAVED_ICON, readable(palette.tertiary, palette))));
    let words = div().flex_1().min_w(px(0.0)).h_full().flex().flex_col().gap(px(LINE_GAP))
        .child(meta)
        .child(div().text_size(px(TITLE_PX)).font_weight(if read { FontWeight::NORMAL } else { FontWeight::SEMIBOLD }).text_color(fg.hsla()).line_clamp(2).child(crate::bidi::paragraph(entry.title.clone())))
        .when(!entry.summary.is_empty(), |el| el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.with_alpha(if read { READ_ALPHA } else { 1.0 }).hsla()).truncate().child(entry.summary.clone())));
    div().h(px(CARD_H)).pb(px(CARD_GAP)).child(
        div().id(("feeds-card", index)).size_full().relative().rounded(px(crate::style::radius(CARD_RADIUS))).bg(fill.hsla()).hover(move |s| if selected { s } else { s.bg(hover.hsla()) }).cursor_pointer().overflow_hidden()
            .pl(px(CARD_PAD_X)).pr(px(CARD_PAD_X)).py(px(CARD_PAD_Y)).row().items_center().gap(px(tokens::MD))
            .child(div().absolute().top(px(CARD_PAD_Y)).bottom(px(CARD_PAD_Y)).left(px(0.0)).w(px(CARD_ACCENT_W)).rounded_full().bg(tint.with_alpha(if read { 0.0 } else { 1.0 }).hsla()))
            .child(words)
            .children(thumb)
            .on_click(move |_, _, cx| {
                hub().borrow_mut().ui.selected = Some(index);
                super::open(item, cx);
            }),
    ).into_any_element()
}

/// A sidebar row: icon or favicon, label and count.
fn side_row(id: SharedString, leading: AnyElement, label: String, count: usize, active: bool, failing: bool, indent: bool, palette: &MandaraPalette, scope: Scope) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    div().id(id).h(px(SIDE_ROW_H)).flex_none().px(px(tokens::SM)).when(indent, |el| el.pad_start(px(tokens::SM + SIDE_INDENT)))
        .rounded(px(crate::style::radius(SIDE_ROW_RADIUS))).row().items_center().gap(px(tokens::SM)).cursor_pointer()
        .when(active, |el| el.bg(accent.with_alpha(alpha(SELECTED_FILL, palette)).hsla()))
        .when(!active, |el| el.hover(move |s| s.bg(hover.hsla())))
        .text_size(px(text::BODY_S)).font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
        .text_color(if active { accent.hsla() } else { palette.on_surface.hsla() })
        .child(leading)
        .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
        .when(failing, |el| el.child(icon("alert", ERROR_ICON - 2.0, readable(palette.warning_amber, palette))))
        .when(count > 0, |el| el.child(div().h(px(COUNT_H)).px(px(COUNT_PAD_X)).rounded_full().bg(wash(WASH_SOFT, palette).hsla())
            .flex().items_center().text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(count_text(count))))
        .on_click(move |_, _, cx| choose(scope.clone(), cx))
        .into_any_element()
}

fn choose(scope: Scope, cx: &mut App) {
    let hub = hub();
    {
        let mut h = hub.borrow_mut();
        h.ui.scope = scope;
        h.ui.selected = None;
        h.ui.reading = None;
        h.ui.list_scroll.scroll_to_item(0, ScrollStrategy::Top);
    }
    let views = hub.borrow().views.clone();
    views.notify(cx);
}

fn sidebar(pictures: Pictures, images: &images::Shared, palette: &MandaraPalette, scale: f32) -> AnyElement {
    let hub = hub();
    let h = hub.borrow();
    let scope = h.ui.scope.clone();
    let mut col = div().id("feeds-sidebar").w(px(SIDEBAR_W)).flex_none().h_full().overflow_y_scroll().flex().flex_col().gap(px(SIDE_GAP));
    let Some(lib) = h.lib.as_ref() else { return col.into_any_element() };
    let glyph = |name: &'static str| icon(name, SIDE_ICON, palette.on_surface_variant).into_any_element();
    col = col.child(side_row("feeds-side-all".into(), glyph("rss"), t("All articles").into(), lib.unread_in(&Scope::All), scope == Scope::All, false, false, palette, Scope::All))
        .child(side_row("feeds-side-saved".into(), glyph("bookmark"), t("Saved articles").into(), lib.starred(), scope == Scope::Starred, false, false, palette, Scope::Starred))
        .child(div().h(px(SIDE_SECTION_GAP)).flex_none());
    let feed_row = |index: usize, row: &FeedRow, indent: bool| {
        let target = Scope::Feed(row.sub.url.as_str().into());
        side_row(SharedString::from(format!("feeds-side-feed-{index}")), favicon(row, FAVICON, pictures, images, palette, scale), row.name().to_owned(), row.unread(), scope == target, row.error().is_some(), indent, palette, target)
    };
    for (index, row) in lib.feeds.iter().enumerate().filter(|(_, row)| row.sub.folder.is_empty()) { col = col.child(feed_row(index, row, false)); }
    for (n, folder) in lib.folders().into_iter().enumerate() {
        let target = Scope::Folder(folder.clone());
        col = col.child(div().h(px(SIDE_GAP * 2.0)).flex_none())
            .child(side_row(SharedString::from(format!("feeds-side-folder-{n}")), glyph("folder"), folder.to_string(), lib.unread_in(&target), scope == target, false, false, palette, target));
        for (index, row) in lib.feeds.iter().enumerate().filter(|(_, row)| *row.sub.folder == *folder) { col = col.child(feed_row(index, row, true)); }
    }
    col.into_any_element()
}

/// The reader pane: a toolbar (back, save, unread, copy, browser) over
/// the scrolling article.
fn reader_pane(url: &str, link: &str, pictures: Pictures, images: &images::Shared, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let hub = hub();
    let item = hub.borrow().lib.as_ref().and_then(|lib| lib.find(url, link));
    let Some(item) = item else {
        hub.borrow_mut().ui.reading = None;
        return div().into_any_element();
    };
    let (read, saved) = { let h = hub.borrow(); let lib = h.lib.as_ref().expect("found above"); (lib.is_read(item), lib.is_starred(item)) };
    let fg = palette.on_surface_variant;
    let accent = readable(palette.primary, palette);
    let copy = link.to_owned();
    let browse = link.to_owned();
    let toolbar = div().flex_none().h(px(CONTROL_H)).row().items_center().gap(px(tokens::XS))
        .child(icon_button("feeds-back", crate::bidi::dir_icon("chevron-left"), TOOL_BTN, fg, None, |_, cx| close_reader(cx)))
        .child(div().flex_1())
        .child(icon_button("feeds-save", "bookmark", TOOL_BTN, if saved { readable(palette.tertiary, palette) } else { fg }, saved.then(|| readable(palette.tertiary, palette).with_alpha(alpha(SELECTED_FILL, palette))), move |_, cx| super::set_starred(item, !saved, cx)))
        .child(icon_button("feeds-unread", if read { "circle" } else { "check" }, TOOL_BTN, fg, None, move |_, cx| super::set_read(item, !read, cx)))
        .child(icon_button("feeds-copy", "copy", TOOL_BTN, fg, None, move |_, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
            super::hub().borrow().flash.show(true, t("Link copied"));
        }))
        .child(icon_button("feeds-browser", "external", TOOL_BTN, accent, None, move |_, _| {
            if let Err(error) = super::open_in_browser(&browse) { super::hub().borrow().flash.show(false, error); }
        }));
    let mono = crate::fonts::fonts(cx).mono.clone();
    let width = (f32::from(window.viewport_size().width).min(super::PANEL_W) - PAD * 2.0 - SIDEBAR_W - GAP - READER_PAD * 2.0 - tokens::MD).max(metrics::ROW_H);
    let now = chrono::Utc::now().timestamp();
    let article = {
        let h = hub.borrow();
        let lib = h.lib.as_ref().expect("found above");
        let (row, entry) = lib.entry(item).expect("found above");
        let reading = h.ui.reading.as_ref();
        let doc = reading.and_then(|r| r.article.clone()).flatten();
        let loading = reading.is_some_and(|r| r.article.is_none());
        let when = match entry.published { Some(at) if now - at < SECS_PER_DAY => ago(at, now), Some(at) => chrono::Local.timestamp_opt(at, 0).single().map(|d| d.format(t("%a %-d %b")).to_string()).unwrap_or_default(), None => ago(entry.seen, now) };
        reader::body(Body { row, entry, doc: doc.as_deref(), loading, width, pictures, accent: accent_of(row, palette), when }, images, mono, palette, window)
    };
    let scroll = hub.borrow().ui.reader_scroll.clone();
    div().size_full().flex().flex_col().gap(px(tokens::SM)).child(toolbar)
        .child(div().id("feeds-reader").flex_1().min_h(px(0.0)).overflow_y_scroll().track_scroll(&scroll).px(px(READER_PAD)).child(article))
        .into_any_element()
}

fn accent_of(row: &FeedRow, palette: &MandaraPalette) -> Color { accent(row, palette) }

fn close_reader(cx: &mut App) {
    let hub = hub();
    hub.borrow_mut().ui.reading = None;
    let views = hub.borrow().views.clone();
    views.notify(cx);
}

/// No subscriptions yet: what Feeds is and where to add some.
fn welcome(palette: &MandaraPalette) -> AnyElement {
    div().size_full().flex().flex_col().items_center().justify_center().gap(px(tokens::MD))
        .child(empty_state("rss", t("No feeds yet"), t("Follow sites you like: add their addresses or pick from tested starter feeds in Studio."), LIST_GUESS_H * EMPTY_SHARE, palette))
        .child(pill_button("feeds-add", Some("plus"), t("Add feeds"), readable(palette.primary, palette), palette, |_, cx| crate::state::open_studio_at(Some("plugins/feeds/settings".into()), cx)).focusable().tab_index(0))
        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Or type a site's address in the launcher and choose Subscribe in Feeds.")))
        .into_any_element()
}

// ── Keyboard ─────────────────────────────────────────────────────────────

/// What a key does in the panel.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Key { Move(isize), Open, Back, Browser, ToggleRead, ToggleSaved, Refresh, Search, Copy, Ignore }

/// Keys act only when the panel (not the search field) has focus; Tab
/// from the search returns to the list.
pub(super) fn key(name: &str, panel: bool, search: bool, plain: bool, reading: bool) -> Key {
    if search { return if plain && matches!(name, "tab" | "down") { Key::Move(0) } else { Key::Ignore }; }
    if !panel || !plain { return Key::Ignore; }
    match name {
        "down" | "j" => Key::Move(1),
        "up" | "k" => Key::Move(-1),
        "enter" | "space" if !reading => Key::Open,
        "escape" | "backspace" if reading => Key::Back,
        "o" => Key::Browser,
        "m" => Key::ToggleRead,
        "s" => Key::ToggleSaved,
        "r" => Key::Refresh,
        "c" if reading => Key::Copy,
        "/" => Key::Search,
        _ => Key::Ignore,
    }
}

fn keys(event: &KeyDownEvent, focus: &FocusHandle, search: &Entity<TextField>, window: &mut Window, cx: &mut App) {
    let hub = hub();
    let reading = hub.borrow().ui.reading.is_some();
    let action = key(&event.keystroke.key, focus.is_focused(window), search.read(cx).is_focused(window), event.keystroke.modifiers == Default::default(), reading);
    if action == Key::Ignore { return; }
    cx.stop_propagation();
    let query = search.read(cx).value.clone();
    let (items, selected) = {
        let mut h = hub.borrow_mut();
        let (scope, filter) = (h.ui.scope.clone(), h.ui.filter);
        let items = h.lib.as_mut().map(|lib| lib.list(&scope, filter, &query)).unwrap_or_else(|| Rc::from([]));
        (items, h.ui.selected)
    };
    let current = selected.and_then(|i| items.get(i).copied().map(|item| (i, item)));
    match action {
        Key::Move(delta) => {
            if delta == 0 { focus.focus(window, cx); }
            if items.is_empty() { return; }
            let next = match selected { Some(i) => (i as isize + delta).clamp(0, items.len() as isize - 1) as usize, None => 0 };
            {
                let mut h = hub.borrow_mut();
                h.ui.selected = Some(next);
                h.ui.list_scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            }
            if reading && let Some(item) = items.get(next).copied() { super::open(item, cx); }
        }
        Key::Open => if let Some((_, item)) = current { super::open(item, cx); },
        Key::Back => close_reader(cx),
        Key::Browser => if let Some((_, item)) = current && let Some(link) = super::with_entry(item, |_, entry| entry.link.clone()) {
            match super::open_in_browser(&link) { Ok(()) => super::set_read(item, true, cx), Err(error) => hub.borrow().flash.show(false, error) }
        },
        Key::ToggleRead => if let Some((_, item)) = current { let read = hub.borrow().lib.as_ref().is_some_and(|lib| lib.is_read(item)); super::set_read(item, !read, cx); },
        Key::ToggleSaved => if let Some((_, item)) = current { let saved = hub.borrow().lib.as_ref().is_some_and(|lib| lib.is_starred(item)); super::set_starred(item, !saved, cx); },
        Key::Copy => if let Some((_, item)) = current && let Some(link) = super::with_entry(item, |_, entry| entry.link.clone()) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(link));
            hub.borrow().flash.show(true, t("Link copied"));
        },
        Key::Refresh => super::refresh_all(cx),
        Key::Search => search.update(cx, |field, cx| field.focus(window, cx)),
        Key::Ignore => {}
    }
    let views = hub.borrow().views.clone();
    views.notify(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_are_compact() {
        let now = 1_800_000_000;
        assert_eq!(ago(now - 5, now), "now");
        assert_eq!(ago(now - 5 * 60, now), "5m");
        assert_eq!(ago(now - 3 * 3600, now), "3h");
        assert_eq!(ago(now - 2 * 86_400, now), "2d");
        assert_eq!(ago(now + 60, now), "now", "a clock slightly ahead is not the future");
        assert!(!ago(now - 30 * 86_400, now).ends_with('d'));
    }

    #[test]
    fn keys_belong_to_the_panel_unless_the_search_has_focus() {
        assert_eq!(key("j", true, false, true, false), Key::Move(1));
        assert_eq!(key("enter", true, false, true, false), Key::Open);
        assert_eq!(key("escape", true, false, true, true), Key::Back);
        assert_eq!(key("escape", true, false, true, false), Key::Ignore, "escape closes the panel when no article is open");
        assert_eq!(key("j", false, true, true, false), Key::Ignore, "typing in the search is typing");
        assert_eq!(key("tab", false, true, true, false), Key::Move(0));
        assert_eq!(key("s", true, false, false, false), Key::Ignore, "shortcuts with modifiers are not ours");
        assert_eq!(key("c", true, false, true, true), Key::Copy);
        assert_eq!(count_text(150), "99+");
    }
}
