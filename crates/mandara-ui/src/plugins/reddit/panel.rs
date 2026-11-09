//! The panel: your communities, sorts, the post list and Discover. The
//! post reader is in [`super::reader`].

use std::rc::Rc;

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, ImageSource, StyledImage as _, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, img, list, prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::PanelCx;
use mandara_services::reddit::{Error, Kind, Post, Query, Sort, Span};
use mandara_theme::{Color, MandaraPalette};

use super::images::{Key, Request, Slot};
use super::model::{self, Model, Page, Shared};
use super::{Gate, Layout, Settings, current};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{alpha, icon_button, text, tokens};
use crate::plugins::panel_fx::{CHIP_H_PX, EXPIRY_MARGIN, WASH_SOFT, empty_state, glass_card, header, pill_button, readable, redraw_in, section, wash};
use crate::text_field::TextField;

/// A card's thumbnail, a compact row's, and the reader's picture width.
pub(super) const THUMB_PX: f32 = 64.0;
const COMPACT_THUMB_PX: f32 = 40.0;
pub(super) const MEDIA_W: f32 = 428.0;
const THUMB_RADIUS_PX: f32 = 12.0;
const BUTTON_PX: f32 = 30.0;
const BADGE_ICON_PX: f32 = 11.0;
const BADGE_PAD_X_PX: f32 = 6.0;
/// Placeholder cards while the first page loads.
const SKELETON_CARDS: usize = 4;
const SKELETON_LINE_PX: f32 = 10.0;
const SKELETON_TITLE_W: [f32; 2] = [0.9, 0.6];
/// Empty and error states fill the list's space.
const STATE_H_PX: f32 = 300.0;
/// The limited-mode countdown redraws this often, while it shows.
const WAIT_TICK: std::time::Duration = std::time::Duration::from_secs(1);
/// Communities shown as front pages; their posts name their community.
const FRONT_PAGES: [&str; 2] = ["popular", "all"];
/// Large counts read better short: 1.2k, 34k, 1.1M.
const THOUSAND: i64 = 1000;
const TEN_THOUSAND: i64 = 10_000;
const MILLION: i64 = 1_000_000;
const SECS_PER_MINUTE: i64 = 60;
const SECS_PER_HOUR: i64 = 3600;
const SECS_PER_DAY: i64 = 86_400;
const DAYS_PER_YEAR: i64 = 365;
/// Where to create the installed app; the plugin never registers one.
pub(super) const APPS_PAGE: &str = "https://www.reddit.com/prefs/apps";

pub(super) struct Fields {
    pub search: Entity<TextField>,
    pub add: Entity<TextField>,
    /// The panel's size, widened for the grid.
    pub size: Rc<std::cell::Cell<gpui::Size<gpui::Pixels>>>,
}

pub(super) fn search_field(cx: &mut App) -> Entity<TextField> {
    cx.new(|cx| TextField::new(String::new(), t("Search this community"), false, Rc::new(|value, _, cx| search(&value, cx)), cx))
}

pub(super) fn add_field(cx: &mut App) -> Entity<TextField> {
    cx.new(|cx| TextField::new(String::new(), t("Add a community, like r/linux"), false, Rc::new(|value, _, cx| add(&value, cx)), cx).clear_on_commit())
}

fn search(words: &str, cx: &mut App) {
    let shared = model::model();
    let want = shared.borrow().want;
    let Some(want) = want else { return };
    {
        let mut m = shared.borrow_mut();
        let Some(mut query) = m.feed.query.clone() else { return };
        let words = words.trim();
        query.search = (!words.is_empty()).then(|| words.chars().take(mandara_services::reddit::MAX_SEARCH).collect::<String>().into());
        m.show(query, want);
    }
    changed(&shared, cx);
}

fn add(name: &str, cx: &mut App) {
    if name.trim().is_empty() { return; }
    match super::subscribe(name, cx) {
        Ok(()) => { let _ = super::open(name, cx); }
        Err(error) => tracing::info!("reddit: {error}"),
    }
}

/// Start waiting work and redraw the panels.
pub(super) fn changed(shared: &Shared, cx: &mut App) {
    model::pump(shared, cx);
    let views = shared.borrow().views.clone();
    views.notify(cx);
}

fn update(cx: &mut App, f: impl FnOnce(&mut Model)) {
    let shared = model::model();
    f(&mut shared.borrow_mut());
    changed(&shared, cx);
}

/// Score and comment counts: 999, 1.2k, 12k, 1.1M.
pub(super) fn count(n: i64) -> String {
    let sign = if n < 0 { "-" } else { "" };
    let n = n.saturating_abs();
    if n < THOUSAND { format!("{sign}{n}") }
    else if n < TEN_THOUSAND { format!("{sign}{}.{}k", n / THOUSAND, n % THOUSAND / 100) }
    else if n < MILLION { format!("{sign}{}k", n / THOUSAND) }
    else { format!("{sign}{}.{}M", n / MILLION, n % MILLION / 100_000) }
}

/// A post's or comment's age: now, 5m, 3h, 2d, 1y.
pub(super) fn age(created: i64, now: i64) -> String {
    let secs = (now - created).max(0);
    if secs < SECS_PER_MINUTE { t("now").into() }
    else if secs < SECS_PER_HOUR { tf("{n}m", &[("n", &(secs / SECS_PER_MINUTE))]) }
    else if secs < SECS_PER_DAY { tf("{n}h", &[("n", &(secs / SECS_PER_HOUR))]) }
    else if secs < SECS_PER_DAY * DAYS_PER_YEAR { tf("{n}d", &[("n", &(secs / SECS_PER_DAY))]) }
    else { tf("{n}y", &[("n", &(secs / (SECS_PER_DAY * DAYS_PER_YEAR)))]) }
}

pub(super) fn sort_label(sort: Sort) -> &'static str {
    match sort { Sort::Hot => tk("Hot"), Sort::New => tk("New"), Sort::Top => tk("Top rated"), Sort::Rising => tk("Rising") }
}

fn span_label(span: Span) -> &'static str {
    match span { Span::Hour => tk("Hour"), Span::Day => tk("Day"), Span::Week => tk("Week"), Span::Month => tk("Month"), Span::Year => tk("Year"), Span::All => tk("All time") }
}

/// Is this post covered (blurred, its text hidden) until revealed?
pub(super) fn covered(post: &Post, settings: &Settings, revealed: bool) -> bool {
    !revealed && ((post.nsfw && settings.nsfw != Gate::Show) || (post.spoiler && settings.spoilers != Gate::Show))
}

/// Is this post left out of lists entirely?
pub(super) fn hidden(post: &Post, settings: &Settings) -> bool { post.nsfw && settings.nsfw == Gate::Hide }

/// A small tinted badge (NSFW, Spoiler, flair, OP).
pub(super) fn badge(icon_name: Option<&'static str>, label: impl Into<SharedString>, tint: Color, _palette: &MandaraPalette) -> gpui::Div {
    div().h(px(CHIP_H_PX - tokens::SM)).px(px(BADGE_PAD_X_PX)).rounded_full().flex_none().row().items_center().gap(px(tokens::XS))
        .bg(tint.with_alpha(alpha::SELECTED).hsla()).text_color(tint.hsla()).text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD)
        .when_some(icon_name, |el, name| el.child(icon(name, BADGE_ICON_PX, tint)))
        .child(label.into())
}

/// A muted icon and number (score, comments).
pub(super) fn stat(icon_name: &'static str, value: impl Into<SharedString>, p: &MandaraPalette) -> gpui::Div {
    div().row().items_center().gap(px(tokens::XS)).flex_none().font_features(crate::fonts::tabular_nums()).text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla())
        .child(icon(icon_name, BADGE_ICON_PX, p.on_surface_variant)).child(value.into())
}

/// The picture for `url` at `width × height` logical pixels, or a quiet
/// placeholder of the same size while it loads.
pub(super) fn picture(m: &mut Model, url: &str, width: f32, height: f32, blur: bool, scale: f32, radius: f32, p: &MandaraPalette) -> AnyElement {
    let (w, h) = ((width * scale).round().max(1.0) as u32, (height * scale).round().max(1.0) as u32);
    let key = Key::new(url, w, h, blur);
    let tile = div().w(px(width)).h(px(height)).flex_none().rounded(px(crate::style::radius(radius)));
    match m.images.get(|| Request { key, url: url.into(), width: w, height: h, blur }, key) {
        Some(Slot::Ready(image)) => img(ImageSource::Render(image.clone())).w(px(width)).h(px(height)).flex_none().rounded(px(crate::style::radius(radius))).object_fit(ObjectFit::Cover).into_any_element(),
        Some(Slot::Failed) => tile.bg(wash(WASH_SOFT, p).hsla()).flex().items_center().justify_center().child(icon("image", BADGE_ICON_PX, p.on_surface_variant)).into_any_element(),
        _ => tile.bg(wash(WASH_SOFT, p).hsla()).into_any_element(),
    }
}

fn icon_action(id: &'static str, name: &'static str, p: &MandaraPalette, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    div().id(id).focusable().tab_index(0).rounded_full()
        .child(icon_button(SharedString::from(format!("{id}-button")), name, BUTTON_PX, p.on_surface_variant, None, on_click))
        .into_any_element()
}

/// The whole panel.
pub(super) fn render(context: &PanelCx, fields: &Fields, p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let settings = current(cx);
    let shared = model::model();
    shared.borrow().views.track(window);
    let scale = window.scale_factor();
    let want = super::want(scale);
    // The grid widens the panel; other layouts keep its usual width.
    let width = px(super::panel_width(if shared.borrow().page == Page::Posts { settings.layout } else { Layout::Cards }));
    let mut panel_size = fields.size.get();
    if panel_size.width != width { panel_size.width = width; fields.size.set(panel_size); }
    let visible = context.progress() > 0.0;
    // A floating card is open without being the bar's panel.
    if visible { Model::follow_panel(&shared, true, cx); }
    {
        let mut m = shared.borrow_mut();
        m.want = Some(want);
        m.images.frame();
        if m.feed.query.is_none() && let Some(first) = settings.subscriptions.first() {
            m.show(Query { subreddit: first.clone(), sort: settings.sort, span: settings.span, search: None }, want);
        }
        // A rate-limited list asks again once Reddit's pause is over, only
        // while it is shown.
        if visible && m.page == Page::Posts && let Some(at) = m.feed.retry_at {
            let now = std::time::Instant::now();
            if now >= at { m.feed.retry_at = None; m.refresh(want); } else { redraw_in(at - now + EXPIRY_MARGIN, window, cx); }
        }
        // The queue's countdown ticks only while it is on screen.
        if visible && m.waiting_until.is_some() { redraw_in(WAIT_TICK, window, cx); }
        // An optional slow refresh, only while the panel shows the list.
        if visible && m.page == Page::Posts && let Some(every) = settings.refresh {
            match m.feed.loaded_at.map(|at| at.elapsed()) {
                Some(age) if age >= every => m.refresh(want),
                Some(age) => redraw_in(every - age + EXPIRY_MARGIN, window, cx),
                None => {}
            }
        }
    }
    let page = shared.borrow().page;
    let body = match page {
        Page::Post => super::reader::render(&shared, &settings, p, scale, window, cx),
        Page::Discover => discover(&shared, &settings, fields, p),
        Page::Posts => posts(&shared, &settings, fields, p, scale),
    };
    if visible { model::pump(&shared, cx); }
    let flash = shared.borrow().flash.clone().current(window, cx);
    div().relative().size_full().p(px(tokens::LG)).flex().flex_col().gap(px(tokens::MD))
        .on_key_down(|event, _, cx| {
            let key = event.keystroke.key.as_str();
            let back = key == "backspace" && event.keystroke.modifiers.alt || key == "left" && event.keystroke.modifiers.alt || key == "escape" && model::model().borrow().page == Page::Post;
            if back && model::model().borrow().page != Page::Posts {
                update(cx, |m| m.back_or_home());
                cx.stop_propagation();
            }
        })
        .child(body)
        .when_some(flash, |el, (ok, message)| el.child(crate::plugins::extras::toast(ok, message, tokens::LG, p)))
        .into_any_element()
}

/// The list page: header, communities, sorts, search and posts.
fn posts(shared: &Shared, settings: &Rc<Settings>, fields: &Fields, p: &MandaraPalette, scale: f32) -> AnyElement {
    let (query, searching, count, loading) = {
        let m = shared.borrow();
        (m.feed.query.clone(), m.searching, m.feed.len(), m.feed.loading)
    };
    let accent = readable(p.primary, p);
    let title: SharedString = query.as_ref().map_or_else(|| t("Reddit").into(), |q| q.subreddit.to_string().into());
    let subtitle: SharedString = match &query {
        Some(q) if q.search.is_some() => tf("Results for “{words}”", &[("words", &q.search.as_deref().unwrap_or_default())]).into(),
        Some(q) if count > 0 => tf("{sort} · {count} posts", &[("sort", &tr(sort_label(q.sort))), ("count", &count)]).into(),
        Some(q) => tr(sort_label(q.sort)).into(),
        None => t("Your communities").into(),
    };
    let trailing = div().row().items_center().gap(px(tokens::XS))
        .child(icon_action("reddit-search", "search", p, |window, cx| {
            let shared = model::model();
            let searching = { let mut m = shared.borrow_mut(); m.searching = !m.searching; m.searching };
            if !searching { search("", cx); }
            let _ = window;
            changed(&shared, cx);
        }))
        .child(icon_action("reddit-refresh", "refresh", p, |_, cx| {
            let shared = model::model();
            let want = shared.borrow().want;
            if let Some(want) = want { shared.borrow_mut().refresh(want); }
            changed(&shared, cx);
        }))
        .child(icon_action("reddit-layout", super::grid::layout_icon(settings.layout), p, |_, cx| super::cycle_layout(cx)))
        .child(icon_action("reddit-discover", "globe", p, |_, cx| update(cx, |m| m.page = Page::Discover)))
        .child(icon_action("reddit-settings", "sliders", p, |_, cx| crate::state::open_studio_at(Some(format!("plugins/{}/settings", super::ID)), cx)));
    let mut column = div().flex_1().min_h_0().flex().flex_col().gap(px(tokens::MD))
        .child(header("messages", accent, title, subtitle, p, Some(trailing.into_any_element())));
    let (limited, setup_open) = { let m = shared.borrow(); (settings.client_id.is_empty() || m.feed.limited, m.setup_open) };
    if limited {
        column = column.child(banner(settings.client_id.is_empty(), setup_open, p));
        if setup_open { return column.child(setup(p)).into_any_element(); }
    }
    column = column.child(communities(settings, query.as_ref(), p));
    if let Some(q) = &query { column = column.child(sorts(q, p)); }
    if searching { column = column.child(div().h(px(crate::plugins::panel_fx::PILL_H_PX + tokens::SM)).child(fields.search.clone())); }
    let content = {
        let mut m = shared.borrow_mut();
        if m.feed.len() == 0 {
            match (&m.feed.error, loading) {
                (_, true) => div().flex().flex_col().gap(px(tokens::SM)).children(waiting(&m, p)).child(skeleton(p)).into_any_element(),
                (Some(error), false) => problem(error, p, |_, cx| { let shared = model::model(); let want = shared.borrow().want; if let Some(want) = want { shared.borrow_mut().refresh(want); } changed(&shared, cx); }),
                (None, false) if query.is_none() => empty_state("messages", t("Add a community to start"), t("Open Discover to pick some"), STATE_H_PX, p).into_any_element(),
                (None, false) => empty_state("messages", t("No posts here yet"), t("Try another sort or community"), STATE_H_PX, p).into_any_element(),
            }
        } else {
            if settings.layout == Layout::Grid {
                let width = super::panel_width(Layout::Grid) - tokens::LG * 2.0;
                super::grid::grid_list(&mut m, shared, settings, p, scale, width)
            } else {
                post_list(&mut m, shared, settings, p, scale)
            }
        }
    };
    column.child(content).into_any_element()
}

/// The one-time setup: why an app is needed and where to make it.
fn setup(p: &MandaraPalette) -> AnyElement {
    let accent = readable(p.primary, p);
    let step = |n: usize, line: &'static str| div().row().gap(px(tokens::SM)).items_start()
        .child(div().size(px(CHIP_H_PX - tokens::XS)).flex_none().rounded_full().bg(accent.with_alpha(alpha::SELECTED).hsla()).flex().items_center().justify_center()
            .text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(n.to_string()))
        .child(div().flex_1().min_w_0().text_size(px(text::BODY_S)).child(crate::bidi::paragraph(tr(line))));
    glass_card(p).p(px(tokens::LG)).flex().flex_col().gap(px(tokens::MD))
        .child(div().row().items_center().gap(px(tokens::SM)).child(icon("key", metrics_tile(), accent))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(t("Connect your own Reddit app"))))
        .child(div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(t("For scores, comment counts, reply threads and more requests, connect a free app you create. It stays on this computer and reads public posts only."))))
        .child(step(1, tk("Open Reddit's app page and choose “create another app”.")))
        .child(step(2, tk("Pick “installed app”, give it any name and use http://localhost as the redirect address.")))
        .child(step(3, tk("Copy the id under the app's name into Studio → Plugins → Reddit → Client id.")))
        .child(div().row().gap(px(tokens::SM)).flex_wrap()
            .child(pill_button("reddit-apps", Some("external"), t("Open Reddit's app page"), accent, p, |_, _| open_link(APPS_PAGE.into())).focusable().tab_index(0))
            .child(pill_button("reddit-studio", Some("sliders"), t("Open settings"), p.on_surface_variant, p, |_, cx| crate::state::open_studio_at(Some(format!("plugins/{}/settings", super::ID)), cx)).focusable().tab_index(0)))
        .into_any_element()
}

fn metrics_tile() -> f32 { crate::kit::metrics::TILE_ICON }

/// Open a web address off the UI thread.
pub(super) fn open_link(url: String) {
    let spawned = std::thread::Builder::new().name("mandara-reddit-open".into()).spawn(move || {
        if let Err(error) = mandara_services::session::open_uri(&url) { tracing::warn!("reddit: opening {url}: {error}"); }
    });
    if let Err(error) = spawned { tracing::warn!("reddit: {error}"); }
}

/// Your communities as a scrolling row; the shown one is highlighted.
fn communities(settings: &Settings, query: Option<&Query>, p: &MandaraPalette) -> AnyElement {
    let accent = readable(p.primary, p);
    let current = query.map(|q| q.subreddit.as_str());
    // "Add" leads, so it never scrolls out of reach behind a long list.
    let mut row = div().id("reddit-communities").overflow_x_scroll().row().gap(px(tokens::XS)).flex_none()
        .child(div().id("reddit-sub-add").focusable().tab_index(0).rounded_full().flex_none().child(icon_button("reddit-sub-add-button", "plus", crate::plugins::panel_fx::PILL_H_PX, accent, Some(accent.with_alpha(alpha::SELECTED)), |_, cx| update(cx, |m| m.page = Page::Discover))));
    let mut shown_current = false;
    for (index, sub) in settings.subscriptions.iter().enumerate() {
        let selected = current.is_some_and(|name| sub.same(name));
        shown_current |= selected;
        let name = sub.clone();
        row = row.child(pill_button(SharedString::from(format!("reddit-sub-{index}")), None, sub.to_string(), if selected { accent } else { p.on_surface_variant }, p, move |_, cx| {
            let _ = super::open(name.as_str(), cx);
        }).focusable().tab_index(0));
    }
    if let (false, Some(q)) = (shown_current, query) {
        row = row.child(pill_button("reddit-sub-current", None, q.subreddit.to_string(), accent, p, |_, _| {}));
    }
    row.into_any_element()
}

/// Hot, new, top (with its range) and rising.
fn sorts(query: &Query, p: &MandaraPalette) -> AnyElement {
    let accent = readable(p.primary, p);
    let choose = |query: Query| move |_: &mut Window, cx: &mut App| {
        let shared = model::model();
        let want = shared.borrow().want;
        if let Some(want) = want { shared.borrow_mut().show(query.clone(), want); }
        changed(&shared, cx);
    };
    let mut column = div().flex().flex_col().gap(px(tokens::XS)).flex_none();
    let mut sorts = div().row().gap(px(tokens::XS)).flex_wrap();
    for sort in Sort::ALL {
        let mut next = query.clone();
        next.sort = sort;
        sorts = sorts.child(pill_button(SharedString::from(format!("reddit-sort-{}", sort.as_str())), None, tr(sort_label(sort)), if sort == query.sort { accent } else { p.on_surface_variant }, p, choose(next)).focusable().tab_index(0));
    }
    column = column.child(sorts);
    if query.sort == Sort::Top || query.search.is_some() {
        let mut spans = div().row().gap(px(tokens::XS)).flex_wrap();
        for span in Span::ALL {
            let mut next = query.clone();
            next.span = span;
            spans = spans.child(pill_button(SharedString::from(format!("reddit-span-{}", span.as_str())), None, tr(span_label(span)), if span == query.span { accent } else { p.on_surface_variant }, p, choose(next)).focusable().tab_index(0));
        }
        column = column.child(spans);
    }
    column.into_any_element()
}

/// Limited mode explained, with the way to full mode one click away.
fn banner(no_app: bool, open: bool, p: &MandaraPalette) -> AnyElement {
    let amber = readable(p.warning_amber, p);
    let line = if no_app {
        t("Without your own Reddit app, Mandara reads Reddit's public feeds: one request a minute, and no scores, comment counts or reply threads.")
    } else {
        t("Reddit didn't accept your app just now, so Mandara is reading its public feeds: one request a minute, without scores or reply threads.")
    };
    glass_card(p).flex_none().p(px(tokens::MD)).row().items_center().gap(px(tokens::MD))
        .child(icon("info", crate::kit::metrics::TILE_ICON, amber))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(tokens::XS))
            .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).text_color(amber.hsla()).child(t("Limited mode")))
            .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(crate::bidi::paragraph(line))))
        .child(pill_button("reddit-full-mode", Some(if open { "chevron-up" } else { "key" }), if open { t("Hide steps") } else { t("Set up full mode") }, readable(p.primary, p), p,
            |_, cx| update(cx, |m| m.setup_open = !m.setup_open)).focusable().tab_index(0))
        .into_any_element()
}

/// Limited mode's queue: when the waiting request goes out.
fn waiting(m: &Model, p: &MandaraPalette) -> Option<AnyElement> {
    let left = m.waiting_until?.saturating_duration_since(std::time::Instant::now()).as_secs().max(1);
    Some(div().row().items_center().gap(px(tokens::SM)).text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla())
        .child(icon("timer", BADGE_ICON_PX, p.on_surface_variant))
        .child(tf("Limited mode asks Reddit once a minute: next request in {seconds} s", &[("seconds", &left)]))
        .into_any_element())
}

/// Placeholder cards with the shape of real ones.
fn skeleton(p: &MandaraPalette) -> AnyElement {
    let fill = wash(WASH_SOFT, p).hsla();
    div().flex().flex_col().gap(px(tokens::SM)).children((0..SKELETON_CARDS).map(|_| {
        glass_card(p).p(px(tokens::MD)).row().gap(px(tokens::MD))
            .child(div().flex_1().flex().flex_col().gap(px(tokens::SM))
                .child(div().h(px(SKELETON_LINE_PX)).w(gpui::relative(SKELETON_TITLE_W[1] / 2.0)).rounded_full().bg(fill))
                .child(div().h(px(SKELETON_LINE_PX)).w(gpui::relative(SKELETON_TITLE_W[0])).rounded_full().bg(fill))
                .child(div().h(px(SKELETON_LINE_PX)).w(gpui::relative(SKELETON_TITLE_W[1])).rounded_full().bg(fill)))
            .child(div().size(px(THUMB_PX)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS_PX))).bg(fill))
    })).into_any_element()
}

/// An error as its own state, with what to do about it.
pub(super) fn problem(error: &Error, p: &MandaraPalette, retry: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let (icon_name, title): (&'static str, SharedString) = match error {
        Error::Setup => ("key", t("Connect your Reddit app").into()),
        Error::Rejected => ("key", t("Reddit didn't accept the client id").into()),
        Error::Wait(_) => ("timer", t("Taking a short break").into()),
        Error::Missing { private: true } => ("lock", t("Private community").into()),
        Error::Missing { private: false } => ("eye-off", t("Not available").into()),
        Error::Offline(_) => ("wifi-off", t("You're offline").into()),
        Error::Failed(_) => ("alert", t("Couldn't load this").into()),
    };
    div().flex().flex_col().items_center().gap(px(tokens::SM))
        .child(empty_state(icon_name, title, error.to_string(), STATE_H_PX, p))
        .when(!matches!(error, Error::Missing { .. }), |el| el.child(pill_button("reddit-retry", Some("refresh"), t("Try again"), readable(p.primary, p), p, retry).focusable().tab_index(0)))
        .into_any_element()
}

/// The posts, virtualised: only rows on screen are built.
fn post_list(m: &mut Model, shared: &Shared, settings: &Rc<Settings>, p: &MandaraPalette, scale: f32) -> AnyElement {
    let state = m.feed.list.clone();
    let shared = shared.clone();
    let settings = settings.clone();
    let palette = p.clone();
    let now = chrono::Utc::now().timestamp();
    let lang = mandara_i18n::language();
    list(state, move |index, _, cx| {
        let row = mandara_i18n::with_language(Some(lang), || {
            let mut m = shared.borrow_mut();
            if index >= m.feed.len() { return footer(&m, &palette); }
            // The page is shared, not copied: the card borrows its post.
            let Some((page, at)) = m.feed.locate(index) else { return div().into_any_element() };
            let post = &page.posts[at];
            if hidden(post, &settings) { return div().into_any_element(); }
            let revealed = m.revealed.contains(&post.id);
            card(&mut m, post, &settings, revealed, &palette, scale, now, index)
        });
        model::pump_soon(&shared, cx);
        row
    }).w_full().flex_1().into_any_element()
}

/// The end of the list: more, loading, an error or the end.
pub(super) fn footer(m: &Model, p: &MandaraPalette) -> AnyElement {
    let accent = readable(p.primary, p);
    let content = if m.feed.loading {
        waiting(m, p).unwrap_or_else(|| div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("Loading more posts…")).into_any_element())
    } else if let Some(error) = &m.feed.error {
        div().flex().flex_col().items_center().gap(px(tokens::XS))
            .child(div().text_size(px(text::LABEL)).text_color(readable(p.warning_amber, p).hsla()).child(error.to_string()))
            .child(pill_button("reddit-more-retry", Some("refresh"), t("Try again"), accent, p, |_, cx| more(cx)).focusable().tab_index(0))
            .into_any_element()
    } else if m.feed.more() {
        pill_button("reddit-more", Some("chevron-down"), t("Load more"), accent, p, |_, cx| more(cx)).focusable().tab_index(0).into_any_element()
    } else if m.feed.after.is_some() {
        pill_button("reddit-more-web", Some("external"), t("See more on Reddit"), accent, p, |_, _| {
            if let Some(q) = model::model().borrow().feed.query.as_ref() { open_link(format!("https://www.reddit.com/r/{}/", q.subreddit.as_str())); }
        }).focusable().tab_index(0).into_any_element()
    } else {
        div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("You're all caught up")).into_any_element()
    };
    div().w_full().py(px(tokens::MD)).flex().justify_center().child(content).into_any_element()
}

fn more(cx: &mut App) {
    let shared = model::model();
    let want = shared.borrow().want;
    if let Some(want) = want { shared.borrow_mut().load_more(want); }
    changed(&shared, cx);
}

/// One post in the list, as a card or a compact row.
fn card(m: &mut Model, post: &Post, settings: &Settings, revealed: bool, p: &MandaraPalette, scale: f32, now: i64, index: usize) -> AnyElement {
    let compact = settings.layout == Layout::Compact;
    let covered = covered(post, settings, revealed);
    let front = m.feed.query.as_ref().is_some_and(|q| FRONT_PAGES.iter().any(|name| q.subreddit.same(name)));
    let thumb_px = if compact { COMPACT_THUMB_PX } else { THUMB_PX };
    let thumb = post.thumb.as_ref().filter(|_| settings.images).map(|thumb| picture(m, &thumb.url, thumb_px, thumb_px, covered, scale, THUMB_RADIUS_PX, p));
    let mut meta = div().row().items_center().gap(px(tokens::XS)).text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).min_w_0();
    if front { meta = meta.child(div().font_weight(FontWeight::SEMIBOLD).flex_none().child(format!("r/{}", post.subreddit))).child("·"); }
    meta = meta.child(div().truncate().child(format!("u/{}", post.author))).child("·").child(div().flex_none().child(age(post.created, now)));
    let mut badges = div().row().items_center().gap(px(tokens::SM)).flex_wrap()
        .when(!post.limited, |el| el.child(stat("arrow-up", count(post.score), p)).child(stat("message", count(post.comments as i64), p)));
    if post.nsfw { badges = badges.child(badge(None, t("NSFW"), p.urgent_coral, p)); }
    if post.spoiler { badges = badges.child(badge(None, t("Spoiler"), p.warning_amber, p)); }
    if post.stickied { badges = badges.child(badge(Some("pin"), t("Pinned"), p.success_emerald, p)); }
    if let Some(flair) = &post.flair && !compact { badges = badges.child(badge(None, flair.to_string(), p.tertiary, p)); }
    match post.kind {
        Kind::Gallery => badges = badges.child(badge(Some("image"), tf("{count} images", &[("count", &post.media.len())]), p.secondary, p)),
        Kind::Video => badges = badges.child(badge(Some("play"), t("Video"), p.secondary, p)),
        Kind::Link if !compact && !post.domain.is_empty() => badges = badges.child(badge(Some("external"), post.domain.to_string(), p.secondary, p)),
        _ => {}
    }
    let title = div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).line_clamp(if compact { 2 } else { 3 }).child(crate::bidi::paragraph(post.title.to_string()));
    let id = post.id.clone();
    let open = move |_: &gpui::ClickEvent, _: &mut Window, cx: &mut App| {
        let shared = model::model();
        let want = shared.borrow().want;
        if let Some(want) = want { shared.borrow_mut().open_post(&id, want); }
        changed(&shared, cx);
    };
    let hover = p.primary.with_alpha(alpha::HOVER).hsla();
    let focus = p.primary.with_alpha(alpha::SELECTED).hsla();
    let row = if compact {
        div().id(SharedString::from(format!("reddit-post-{index}"))).focusable().tab_index(0).cursor_pointer()
            .px(px(tokens::SM)).py(px(tokens::SM)).rounded(px(crate::style::radius(tokens::MD))).row().items_center().gap(px(tokens::MD))
            .hover(move |s| s.bg(hover)).focus_visible(move |s| s.bg(focus))
            .when_some(thumb, |el, thumb| el.child(thumb))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(tokens::XS)).child(title).child(div().row().items_center().gap(px(tokens::SM)).child(badges).child(meta)))
            .on_click(open)
    } else {
        glass_card(p).id(SharedString::from(format!("reddit-post-{index}"))).focusable().tab_index(0).cursor_pointer()
            .p(px(tokens::MD)).row().items_start().gap(px(tokens::MD))
            .hover(move |s| s.bg(hover)).focus_visible(move |s| s.bg(focus))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(tokens::XS)).child(meta).child(title).child(badges))
            .when_some(thumb, |el, thumb| el.child(thumb))
            .on_click(open)
    };
    div().w_full().pb(px(if compact { tokens::XS } else { tokens::SM })).child(row).into_any_element()
}

/// Your communities (reorder, remove) and a starter list by topic.
fn discover(shared: &Shared, settings: &Rc<Settings>, fields: &Fields, p: &MandaraPalette) -> AnyElement {
    let accent = readable(p.primary, p);
    let back = icon_action("reddit-discover-back", crate::bidi::dir_icon("chevron-left"), p, |_, cx| update(cx, |m| m.page = Page::Posts));
    let subtitle = tf("{count} in your list", &[("count", &settings.subscriptions.len())]);
    let mut column = div().id("reddit-discover").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(tokens::MD))
        .child(div().row().items_center().gap(px(tokens::SM)).child(back).child(div().flex_1().min_w_0().child(header("globe", accent, t("Communities"), subtitle, p, None))))
        .child(div().h(px(crate::plugins::panel_fx::PILL_H_PX + tokens::SM)).flex_none().child(fields.add.clone()));
    let _ = shared;
    if !settings.subscriptions.is_empty() {
        column = column.child(section(tk("Your communities"), p, None));
        let last = settings.subscriptions.len() - 1;
        for (index, sub) in settings.subscriptions.iter().enumerate() {
            let (open, up, down, remove) = (sub.clone(), sub.clone(), sub.clone(), sub.clone());
            column = column.child(div().id(SharedString::from(format!("reddit-mine-{index}"))).flex_none().h(px(crate::kit::metrics::ROW_H)).px(px(tokens::SM))
                .rounded(px(crate::style::radius(tokens::MD))).row().items_center().gap(px(tokens::XS))
                .hover(|s| s.bg(p.on_surface.with_alpha(alpha::HOVER).hsla()))
                .child(div().id(SharedString::from(format!("reddit-mine-open-{index}"))).flex_1().min_w_0().cursor_pointer().focusable().tab_index(0)
                    .text_size(px(text::BODY)).truncate().child(sub.to_string()).on_click(move |_, _, cx| { let _ = super::open(open.as_str(), cx); }))
                .when(index > 0, |el| el.child(icon_button(SharedString::from(format!("reddit-up-{index}")), "chevron-up", BUTTON_PX, p.on_surface_variant, None, move |_, cx| super::move_subscription(up.as_str(), -1, cx))))
                .when(index < last, |el| el.child(icon_button(SharedString::from(format!("reddit-down-{index}")), "chevron-down", BUTTON_PX, p.on_surface_variant, None, move |_, cx| super::move_subscription(down.as_str(), 1, cx))))
                .child(icon_button(SharedString::from(format!("reddit-remove-{index}")), "trash", BUTTON_PX, p.on_surface_variant, None, move |_, cx| super::unsubscribe(remove.as_str(), cx))));
        }
    }
    for (group, category) in super::starter::CATEGORIES.iter().enumerate() {
        column = column.child(div().row().items_center().gap(px(tokens::SM)).flex_none().child(icon(category.icon, BADGE_ICON_PX + tokens::XS, accent)).child(section(category.name, p, None)));
        let mut chips = div().row().gap(px(tokens::XS)).flex_wrap().flex_none();
        for (index, name) in category.communities.iter().enumerate() {
            let on = model::subscribed(&settings.subscriptions, name);
            let name = *name;
            chips = chips.child(pill_button(SharedString::from(format!("reddit-starter-{group}-{index}")), Some(if on { "check" } else { "plus" }), format!("r/{name}"), if on { accent } else { p.on_surface_variant }, p, move |_, cx| {
                if model::subscribed(&current(cx).subscriptions, name) { super::unsubscribe(name, cx); } else if let Err(error) = super::subscribe(name, cx) { tracing::info!("reddit: {error}"); }
            }).focusable().tab_index(0));
        }
        column = column.child(chips);
    }
    column.into_any_element()
}

