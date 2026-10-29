//! Notification center: bell with unread count in the bar; panel and
//! sidebar card listing notifications grouped by app (app icon, count,
//! per-app clear), glass notification cards with actions and dismiss, a
//! do-not-disturb pill and clear-all. Popups (toasts) are drawn by the
//! shell host from the same state with [`notification_card`].

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FontStyle, FontWeight, HighlightStyle, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, UnderlineStyle, Window, div, img, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::notifications::AutoDnd;
use mandara_services::{Notification, NotificationCommand, NotificationsState, Urgency};
use mandara_theme::MandaraPalette;

use super::panel_fx::{
    self, ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, EXPIRY_MARGIN, HEADER_H, LINK_H_PX, LINK_HOVER_ALPHA, PILL_H_PX, PanelShape, empty_state, header, pill_button, readable,
    redraw_in,
};
use super::{fn_view, json};
use crate::fonts::tabular_nums;
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{card, icon_button, reveal, text, tokens};

pub const ID: &str = "notifications";

const PANEL_W: f32 = 404.0;
const PAD: f32 = 14.0;
const EMPTY_H: f32 = 180.0;
/// Panel height before the first layout pass, and its cap (it scrolls).
const INITIAL_PANEL_H: f32 = 300.0;
const MAX_PANEL_H: f32 = 660.0;
/// Corner radius of the sidebar card's inner banner.
const SIDEBAR_RADIUS_PX: f32 = 14.0;
/// Cards shown per app group.
const MAX_PER_GROUP: usize = 6;
/// Action buttons shown per notification.
const MAX_ACTIONS: usize = 3;
/// Opening progress past which the panel counts as seen / below which it
/// counts as closed again.
const SEEN_PROGRESS: f32 = 0.5;
const CLOSED_PROGRESS: f32 = 0.1;

// Relative timestamps ("now", "5m", "3h", then a date).
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 3600;
const SECS_PER_DAY: i64 = 86_400;

/// Resolved notification / app icons kept (cleared when full).
const ICON_CACHE_CAP: usize = 128;
/// Cards the notification center shows per page ("Show more" adds a page;
/// the count resets when the panel closes).
const PAGE_SIZE: usize = 12;
/// Cards on the sidebar card (no paging there).
const SIDEBAR_LIMIT: usize = 6;
/// Height of the "Show more" row.
const MORE_H: f32 = 28.0;
/// Height of the inline-reply composer.
const REPLY_H: f32 = 84.0;
/// Notification images are decoded as thumbnails at most this many pixels
/// on a side (the 38 px card icon at up to 3× scale) — never full size: an
/// `image-path` hint can point at a whole screenshot.
const THUMB_PX: u32 = 128;
/// Thumbnails kept decoded (LRU; the oldest is dropped, CPU and GPU).
const THUMB_CACHE_CAP: usize = 48;
/// Thumbnails unused for this long are dropped on the next sweep.
const THUMB_IDLE: Duration = Duration::from_secs(300);

// Notification card.
const CARD_RADIUS_PX: f32 = 16.0;
const CARD_PAD_PX: f32 = 12.0;
const CARD_GAP_PX: f32 = 12.0;
const CARD_ICON_PX: f32 = 38.0;
/// App icon corner radius as a fraction of its side; glyph fallback size.
const ICON_RADIUS_FRAC: f32 = 0.28;
const ICON_GLYPH_FRAC: f32 = 0.5;
/// Fallback icon gradient alphas `(accent, partner)`.
const ICON_FILL: (f32, f32) = (0.2, 0.1);
/// Critical notifications: coral wash `(top, bottom)` alphas.
const CRITICAL_FILL: (f32, f32) = (0.2, 0.1);
const TEXT_GAP_PX: f32 = 3.0;
const TITLE_GAP_PX: f32 = 8.0;
/// Title row keeps clear of the dismiss button.
const DISMISS_CLEARANCE_PX: f32 = 22.0;
const UNREAD_DOT_PX: f32 = 7.0;
const BODY_LINE_PX: f32 = 17.0;
const BODY_LINES_COMPACT: usize = 2;
const BODY_LINES_FULL: usize = 5;
/// Toast body lines in compact mode.
const BODY_LINES_TOAST_COMPACT: usize = 1;
/// Toast size multiplier bounds.
pub const TOAST_SCALE: (f32, f32) = (0.75, 1.5);
/// App name line above the summary.
const APP_NAME_TEXT_PX: f32 = text::LABEL;
/// `value` hint progress bar.
const PROGRESS_H_PX: f32 = 4.0;
const PERCENT: f32 = 100.0;
const ANGLE_RIGHT_DEG: f32 = 90.0;
const ACTIONS_TOP_PX: f32 = 4.0;
const ACTION_GAP_PX: f32 = 6.0;
const ACTION_PAD_X_PX: f32 = 12.0;
/// Action button size: half a step above the label size.
const ACTION_TEXT_PX: f32 = text::LABEL + 0.5;
const ACTION_FILL_ALPHA: f32 = 0.14;
const ACTION_HOVER_ALPHA: f32 = 0.22;
const DISMISS_INSET_PX: f32 = 6.0;
const DISMISS_PX: f32 = 24.0;

// Card height estimate (sizes the panel before layout).
const EST_PAD_H: f32 = 24.0;
const EST_TITLE_H: f32 = 18.0;
const EST_BODY_GAP_H: f32 = 3.0;
/// Characters per body line at the card width.
const EST_CHARS_PER_LINE: f32 = 44.0;
const EST_ACTIONS_H: f32 = 32.0;
const EST_MIN_CARD_H: f32 = 62.0;
/// Gap between cards in a group.
const GROUP_GAP_PX: f32 = 6.0;

// Group header.
const GROUP_HEAD_H: f32 = 22.0;
const GROUP_HEAD_GAP_PX: f32 = 8.0;
const GROUP_HEAD_PAD_X_PX: f32 = 4.0;
const GROUP_ICON_PX: f32 = 16.0;
const GROUP_ICON_RADIUS_PX: f32 = 4.0;
const GROUP_GLYPH_PX: f32 = 13.0;
const GROUP_APP_TEXT_PX: f32 = text::LABEL + 0.5;
const COUNT_PAD_X_PX: f32 = 6.0;
const COUNT_TEXT_PX: f32 = 10.0;
const COUNT_FILL_ALPHA: f32 = 0.07;
const CLEAR_PAD_X_PX: f32 = 8.0;

// Header controls and the "another daemon" banner.
const CONTROLS_GAP_PX: f32 = 6.0;
const CLEAR_ALL_PX: f32 = 30.0;
const BANNER_H: f32 = 56.0;
const BANNER_PAD_X_PX: f32 = 12.0;
const BANNER_PAD_Y_PX: f32 = 9.0;
const BANNER_GAP_PX: f32 = 10.0;
const BANNER_FILL_ALPHA: f32 = 0.12;
const BANNER_ICON_PX: f32 = 15.0;

// Bar item.
const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 4.0;
const BADGE_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 4.0;
const BADGE_TEXT_PX: f32 = 10.0;
/// Unread counts above this show as this value.
const BADGE_MAX: usize = 99;

pub fn send(cx: &mut App, command: NotificationCommand) {
    if let Some(s) = hub(cx).read(cx).notification_service.clone() {
        s.send(command);
    }
}

/// A notification id from a command argument (ids are `u32` on D-Bus).
fn notification_id(n: i64) -> Result<u32, String> {
    u32::try_from(n).map_err(|_| format!("invalid notification id {n}"))
}

fn request(cx: &mut App, command: NotificationCommand) -> Result<Output, String> {
    let s = hub(cx).read(cx).notification_service.clone().ok_or("notification service unavailable")?;
    Ok(Output::Pending(s.request(command)))
}

pub fn ago(ts: i64) -> String {
    let secs = Local::now().timestamp() - ts;
    match secs {
        ..SECS_PER_MIN => t("now").to_string(),
        SECS_PER_MIN..SECS_PER_HOUR => tf("{n}m", &[("n", &(secs / SECS_PER_MIN))]),
        SECS_PER_HOUR..SECS_PER_DAY => tf("{n}h", &[("n", &(secs / SECS_PER_HOUR))]),
        _ => Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d %b").to_string()).unwrap_or_default(),
    }
}

/// Seconds until the [`ago`] label of a notification `age` seconds old
/// next changes (`None` once it shows a date, which never ticks).
fn next_label_change(age: i64) -> Option<i64> {
    match age.max(0) {
        a @ ..SECS_PER_HOUR => Some(SECS_PER_MIN - a % SECS_PER_MIN),
        a @ SECS_PER_HOUR..SECS_PER_DAY => Some(SECS_PER_HOUR - a % SECS_PER_HOUR),
        _ => None,
    }
}

/// How long until the soonest relative timestamp in `items` changes. Views
/// re-render then (only while visible) instead of on a fixed timer that
/// also runs while hidden.
fn relabel_delay(items: &[Notification]) -> Option<Duration> {
    let now = Local::now().timestamp();
    items.iter().filter_map(|n| next_label_change(now - n.timestamp)).min().map(|secs| Duration::from_secs(secs.unsigned_abs()) + EXPIRY_MARGIN)
}

thread_local! {
    /// `(icon, app)` hash → resolved image path. Theme lookups read files
    /// (GTK settings, icon dirs), so they must not run per card per frame.
    static ICONS: RefCell<HashMap<u64, Option<Arc<Path>>>> = RefCell::new(HashMap::new());
}

/// Cached icon resolution for a notification icon spec and its app.
fn resolve_icon(spec: &str, app: &str) -> Option<Arc<Path>> {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (spec, app).hash(&mut h);
    let key = h.finish();
    if let Some(hit) = ICONS.with(|c| c.borrow().get(&key).cloned()) {
        return hit;
    }
    let found = if spec.starts_with('/') {
        Path::new(spec).exists().then(|| Arc::from(Path::new(spec)))
    } else {
        crate::icon_theme::resolve(spec, None, false).or_else(|| crate::apps::icon_for_app_id(app, false)).map(|r| Arc::from(r.path.as_path()))
    };
    ICONS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= ICON_CACHE_CAP {
            c.clear();
        }
        c.insert(key, found.clone());
    });
    found
}

/// Decodes an image file into a small BGRA thumbnail (runs on GPUI's
/// background executor through the asset cache).
struct Thumbnail;

impl gpui::Asset for Thumbnail {
    type Source = Arc<Path>;
    type Output = Result<Arc<gpui::RenderImage>, gpui::ImageCacheError>;

    fn load(source: Self::Source, _: &mut App) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move {
            let fail = |e: String| gpui::ImageCacheError::Asset(SharedString::from(e));
            let decoded = image::open(&source).map_err(|e| fail(format!("{}: {e}", source.display())))?;
            let mut thumb = decoded.thumbnail(THUMB_PX, THUMB_PX).into_rgba8();
            // GPUI's renderer expects BGRA.
            for px in thumb.pixels_mut() {
                px.0.swap(0, 2);
            }
            Ok(Arc::new(gpui::RenderImage::new(smallvec::smallvec![image::Frame::new(thumb)])))
        }
    }
}

thread_local! {
    /// Thumbnails in use, least recently used first, with their last use.
    static THUMBS: RefCell<VecDeque<(Arc<Path>, Instant)>> = RefCell::new(VecDeque::with_capacity(THUMB_CACHE_CAP));
}

/// Drops a thumbnail from GPUI's asset cache and every window's atlas.
fn evict_thumbnail(path: &Arc<Path>, cx: &mut App) {
    if let Some(Ok(image)) = cx.fetch_asset::<Thumbnail>(path) {
        cx.drop_image(image, None);
    }
    cx.remove_asset::<Thumbnail>(path);
}

/// Marks `path` used now; evicts idle thumbnails and the oldest beyond the cap.
fn touch_thumbnail(path: &Arc<Path>, cx: &mut App) {
    let now = Instant::now();
    let evicted: Vec<Arc<Path>> = THUMBS.with(|t| {
        let mut t = t.borrow_mut();
        match t.iter().position(|(p, _)| p == path) {
            Some(i) => {
                if let Some(mut entry) = t.remove(i) {
                    entry.1 = now;
                    t.push_back(entry);
                }
            }
            None => t.push_back((path.clone(), now)),
        }
        let mut out = Vec::new();
        while t.len() > THUMB_CACHE_CAP || t.front().is_some_and(|(_, used)| now.duration_since(*used) > THUMB_IDLE) {
            match t.pop_front() {
                Some((p, _)) => out.push(p),
                None => break,
            }
        }
        out
    });
    for p in &evicted {
        evict_thumbnail(p, cx);
    }
}

/// An image source that decodes `path` as a bounded thumbnail (SVGs are
/// vector: GPUI rasterizes those at their drawn size already).
fn thumbnail_source(path: Arc<Path>) -> gpui::ImageSource {
    let vector = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    if vector {
        return path.into();
    }
    gpui::ImageSource::Custom(Arc::new(move |window, cx| {
        touch_thumbnail(&path, cx);
        window.use_asset::<Thumbnail>(&path, cx)
    }))
}

/// Icon for a notification: a file path / URI, a theme icon name, or the
/// app's icon, falling back to a bell.
pub fn notification_icon(n: &Notification, size_px: f32, palette: &MandaraPalette) -> AnyElement {
    let path = n.icon.strip_prefix("file://").unwrap_or(&n.icon);
    match resolve_icon(path, &n.app_name) {
        // The image carries the radius itself (parents clip rectangles only).
        Some(p) => img(thumbnail_source(p)).size(px(size_px)).flex_none().rounded(px(crate::style::radius(size_px * ICON_RADIUS_FRAC))).object_fit(ObjectFit::Cover).into_any_element(),
        // Shell-posted notifications name a Mandara icon (e.g. "timer").
        None => {
            let accent = readable(palette.primary, palette);
            div()
                .size(px(size_px))
                .flex_none()
                .rounded(px(crate::style::radius(size_px * ICON_RADIUS_FRAC)))
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(ICON_FILL.0), panel_fx::partner(accent).with_alpha(ICON_FILL.1)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(crate::icons::ICON_NAMES.iter().find(|n| **n == path).copied().unwrap_or("bell"), size_px * ICON_GLYPH_FRAC, accent))
                .into_any_element()
        }
    }
}

/// How a card draws: the panel and sidebar list, or a popup toast.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CardStyle {
    /// Body lines before it clamps.
    pub body_lines: usize,
    /// Size multiplier (toasts follow `notifications.scale`).
    pub scale: f32,
    pub show_actions: bool,
    pub show_app_name: bool,
    /// Bold / italic / links from `body-markup`.
    pub markup: bool,
    /// Privacy mode: the body stays hidden (toasts).
    pub hide_body: bool,
    /// Clicking the body runs the default action.
    pub click_action: bool,
}

impl CardStyle {
    /// The notification center and sidebar.
    pub fn list(cfg: &mandara_config::alerts::NotificationsConfig) -> Self {
        Self { body_lines: BODY_LINES_COMPACT, scale: 1.0, show_actions: true, show_app_name: false, markup: cfg.markup, hide_body: false, click_action: true }
    }

    /// Popup toasts.
    pub fn toast(cfg: &mandara_config::alerts::NotificationsConfig) -> Self {
        Self {
            body_lines: if cfg.compact { BODY_LINES_TOAST_COMPACT } else { BODY_LINES_COMPACT },
            scale: cfg.scale.clamp(TOAST_SCALE.0, TOAST_SCALE.1),
            show_actions: cfg.show_actions,
            show_app_name: cfg.show_app_name,
            markup: cfg.markup,
            hide_body: cfg.privacy,
            click_action: cfg.click_action,
        }
    }
}

/// The body with its markup spans as GPUI highlights; links are tinted and
/// underlined (clicking the body runs the default action, so links open
/// from the notification center via the app).
fn body_text(n: &Notification, accent: mandara_theme::Color, markup: bool) -> AnyElement {
    if !markup || n.markup.is_empty() {
        return crate::bidi::paragraph(n.body.clone());
    }
    let len = n.body.len();
    let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = n
        .markup
        .iter()
        .filter(|s| s.start < s.end && s.end <= len && n.body.is_char_boundary(s.start) && n.body.is_char_boundary(s.end))
        .map(|s| {
            let mut style = HighlightStyle::default();
            if s.bold {
                style.font_weight = Some(FontWeight::BOLD);
            }
            if s.italic {
                style.font_style = Some(FontStyle::Italic);
            }
            if s.underline || s.link.is_some() {
                style.underline = Some(UnderlineStyle { thickness: px(1.0), color: None, wavy: false });
            }
            if s.link.is_some() {
                style.color = Some(accent.hsla());
            }
            (s.start..s.end, style)
        })
        .collect();
    crate::bidi::styled_paragraph(n.body.clone(), highlights)
}

/// One notification card (shared by the panel and the sidebar).
pub fn notification_card(n: &Notification, palette: &MandaraPalette, compact: bool) -> AnyElement {
    let cfg = mandara_config::alerts::NotificationsConfig::default();
    let mut style = CardStyle::list(&cfg);
    if !compact {
        style.body_lines = BODY_LINES_FULL;
    }
    notification_card_styled(n, palette, &style)
}

/// One notification card drawn with `style` (toasts use their settings).
pub fn notification_card_styled(n: &Notification, palette: &MandaraPalette, style: &CardStyle) -> AnyElement {
    let id = n.id;
    let s = style.scale;
    let z = |v: f32| px(v * s);
    let critical = n.urgency == Urgency::Critical;
    let accent = readable(palette.primary, palette);
    let mut actions = div().flex().flex_wrap().gap(z(ACTION_GAP_PX));
    let reply = n.can_reply();
    for (i, (key, label)) in n.actions.iter().filter(|(k, _)| k != "default" && k != mandara_services::notifications::INLINE_REPLY).take(MAX_ACTIONS).enumerate() {
        let key = key.clone();
        actions = actions.child(
            div()
                .id(ElementId::NamedChild(Arc::new(ElementId::from(("act", id))), SharedString::from(i.to_string())))
                .px(z(ACTION_PAD_X_PX))
                .h(z(PILL_H_PX))
                .rounded_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .bg(accent.with_alpha(ACTION_FILL_ALPHA).hsla())
                .text_color(accent.hsla())
                .text_size(z(ACTION_TEXT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .hover(move |s| s.bg(accent.with_alpha(ACTION_HOVER_ALPHA).hsla()))
                .child(label.clone())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    send(cx, NotificationCommand::Invoke { id, action: key.clone() })
                }),
        );
    }
    if reply {
        // Inline replies are typed in the notification center.
        actions = actions.child(
            div()
                .id(("reply", id))
                .px(z(ACTION_PAD_X_PX))
                .h(z(PILL_H_PX))
                .rounded_full()
                .flex()
                .items_center()
                .gap(z(TITLE_GAP_PX / 2.0))
                .cursor_pointer()
                .border_1()
                .border_color(accent.with_alpha(ACTION_HOVER_ALPHA).hsla())
                .text_color(accent.hsla())
                .text_size(z(ACTION_TEXT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .hover(move |s| s.bg(accent.with_alpha(ACTION_FILL_ALPHA).hsla()))
                .child(t("Reply"))
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    begin_reply(id, cx);
                }),
        );
    }
    let has_actions = style.show_actions && (reply || n.actions.iter().any(|(k, _)| k != "default"));
    let has_default = style.click_action && n.actions.iter().any(|(k, _)| k == "default");
    let base = panel_fx::glass_card(palette).rounded(z(CARD_RADIUS_PX));
    let base = if critical {
        base.bg(oklab_gradient(ANGLE_DOWN_DEG, palette.urgent_coral.with_alpha(CRITICAL_FILL.0), palette.urgent_coral.with_alpha(CRITICAL_FILL.1)))
    } else {
        base
    };
    let show_body = !n.body.is_empty() && !style.hide_body;
    base.id(("notification", id))
        .relative()
        .p(z(CARD_PAD_PX))
        .flex()
        .gap(z(CARD_GAP_PX))
        .when(has_default, |el| el.cursor_pointer())
        .on_click(move |_, _, cx| {
            if has_default {
                send(cx, NotificationCommand::Invoke { id, action: "default".into() });
            }
        })
        .child(notification_icon(n, CARD_ICON_PX * s, palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(z(TEXT_GAP_PX))
                .when(style.show_app_name && !n.app_name.is_empty(), |el| {
                    el.child(div().pr(z(DISMISS_CLEARANCE_PX)).truncate().text_size(z(APP_NAME_TEXT_PX)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(n.app_name.clone()))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(z(TITLE_GAP_PX))
                        .pr(z(DISMISS_CLEARANCE_PX))
                        .when(!n.read, |el| el.child(div().size(z(UNREAD_DOT_PX)).flex_none().rounded_full().bg(accent.hsla())))
                        .child(div().flex_1().min_w(px(0.0)).truncate().font_weight(FontWeight::SEMIBOLD).text_size(z(text::BODY)).text_color(palette.on_surface.hsla()).child(n.summary.clone()))
                        .when(n.repeats > 0, |el| {
                            el.child(div().flex_none().px(z(COUNT_PAD_X_PX)).rounded_full().bg(palette.on_surface.with_alpha(COUNT_FILL_ALPHA).hsla()).text_size(z(COUNT_TEXT_PX)).font_features(tabular_nums()).child(format!("×{}", n.repeats + 1)))
                        })
                        .child(div().text_size(z(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(ago(n.timestamp))),
                )
                .when(show_body, |el| {
                    el.child(div().text_size(z(text::BODY_S)).line_height(z(BODY_LINE_PX)).text_color(palette.on_surface_variant.hsla()).line_clamp(style.body_lines).child(body_text(n, accent, style.markup)))
                })
                .when_some(n.progress, |el, pct| {
                    let v = f32::from(pct) / PERCENT;
                    el.child(
                        div()
                            .mt(z(TEXT_GAP_PX))
                            .w_full()
                            .h(z(PROGRESS_H_PX))
                            .rounded_full()
                            .bg(palette.on_surface.with_alpha(COUNT_FILL_ALPHA).hsla())
                            .child(div().h_full().w(gpui::relative(v)).rounded_full().bg(oklab_gradient(ANGLE_RIGHT_DEG, accent, panel_fx::partner(accent)))),
                    )
                })
                .when(has_actions, |el| el.child(div().pt(z(ACTIONS_TOP_PX)).child(actions))),
        )
        .child(
            div().absolute().top(z(DISMISS_INSET_PX)).right(z(DISMISS_INSET_PX)).child(icon_button(("dismiss", id), "close", DISMISS_PX * s, palette.on_surface_variant, None, move |_, cx| {
                cx.stop_propagation();
                send(cx, NotificationCommand::Dismiss(id))
            })),
        )
        .into_any_element()
}

/// Rough rendered height of a card (for sizing the panel to its content).
fn card_height(n: &Notification) -> f32 {
    /// A body takes at least one and at most the compact clamp of lines.
    const BODY_LINES: (f32, f32) = (1.0, BODY_LINES_COMPACT as f32);
    let body = if n.body.is_empty() {
        0.0
    } else {
        EST_BODY_GAP_H + BODY_LINE_PX * (n.body.chars().count() as f32 / EST_CHARS_PER_LINE).ceil().clamp(BODY_LINES.0, BODY_LINES.1)
    };
    let actions = if n.actions.iter().any(|(k, _)| k != "default") { EST_ACTIONS_H } else { 0.0 };
    (EST_PAD_H + EST_TITLE_H + body + actions).max(EST_MIN_CARD_H)
}

fn group_header(gi: usize, app: &str, list: &[&Notification], palette: &MandaraPalette) -> impl IntoElement {
    let app_clear = app.to_string();
    let accent = readable(palette.primary, palette);
    let app_icon = list.first().and_then(|n| resolve_icon("", &n.app_name));
    div()
        .h(px(GROUP_HEAD_H))
        .flex()
        .items_center()
        .gap(px(GROUP_HEAD_GAP_PX))
        .px(px(GROUP_HEAD_PAD_X_PX))
        .child(match app_icon {
            Some(p) => img(thumbnail_source(p)).size(px(GROUP_ICON_PX)).flex_none().rounded(px(crate::style::radius(GROUP_ICON_RADIUS_PX))).object_fit(ObjectFit::Contain).into_any_element(),
            None => icon("bell", GROUP_GLYPH_PX, palette.on_surface_variant).into_any_element(),
        })
        .child(div().text_size(px(GROUP_APP_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(if app.is_empty() {
            SharedString::from(t("System"))
        } else {
            SharedString::from(app.to_string())
        }))
        .child(
            div()
                .px(px(COUNT_PAD_X_PX))
                .rounded_full()
                .bg(palette.on_surface.with_alpha(COUNT_FILL_ALPHA).hsla())
                .text_size(px(COUNT_TEXT_PX))
                .font_features(tabular_nums())
                .text_color(palette.on_surface_variant.hsla())
                .child(list.len().to_string()),
        )
        .child(div().flex_1())
        .child(
            div()
                .id(("clear-app", gi))
                .px(px(CLEAR_PAD_X_PX))
                .h(px(LINK_H_PX))
                .rounded_full()
                .flex()
                .items_center()
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .text_color(accent.hsla())
                .hover(move |s| s.bg(accent.with_alpha(LINK_HOVER_ALPHA).hsla()))
                .child(t("Clear"))
                .on_click(move |_, _, cx| send(cx, NotificationCommand::DismissApp(app_clear.clone()))),
        )
}

/// The grouped list used by the panel and the sidebar card. Returns the
/// element and its estimated height.
/// The grouped list used by the panel and the sidebar card, showing at
/// most `limit` cards. Returns the element, its estimated height and
/// whether more cards exist past the limit.
fn center(state: &NotificationsState, palette: &MandaraPalette, p: Option<&PanelCx>, radius: f32, limit: usize) -> (AnyElement, f32, bool) {
    let dnd = state.dnd;
    let quiet = state.quiet();
    let accent = readable(palette.primary, palette);
    let unread = state.unread();
    let subtitle: SharedString = if dnd && let Some(until) = state.dnd_until {
        tf("Do not disturb until {time}", &[("time", &Local.timestamp_opt(until, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default())]).into()
    } else if dnd {
        t("Do not disturb is on").into()
    } else if let Some(reason) = state.auto_dnd.first() {
        auto_reason(*reason).into()
    } else if state.items.is_empty() {
        t("You're all caught up").into()
    } else if unread > 0 {
        tf("{n} unread", &[("n", &unread)]).into()
    } else {
        tf("{n} notifications", &[("n", &state.items.len())]).into()
    };
    let controls = div()
        .flex()
        .items_center()
        .gap(px(CONTROLS_GAP_PX))
        .child(pill_button("notif-dnd", Some("moon"), t("Do not disturb"), if dnd { accent } else { palette.on_surface_variant }, palette, move |_, cx| {
            send(cx, NotificationCommand::SetDnd(!dnd))
        }))
        .when(!state.items.is_empty(), |el| el.child(icon_button("notif-clear", "trash", CLEAR_ALL_PX, palette.on_surface_variant, None, |_, cx| send(cx, NotificationCommand::DismissAll))));
    let head = header(if quiet { "moon" } else { "bell" }, accent, t("Notifications"), subtitle, palette, Some(controls.into_any_element()));
    let mut h = HEADER_H;
    let mut col = div().flex().flex_col().gap(px(tokens::MD)).child(match p {
        Some(p) => reveal(p, 0, head),
        None => head.into_any_element(),
    });
    if !state.active {
        h += tokens::MD + BANNER_H;
        col = col.child(
            div()
                .px(px(BANNER_PAD_X_PX))
                .py(px(BANNER_PAD_Y_PX))
                .rounded(px(radius))
                .bg(palette.warning_amber.with_alpha(BANNER_FILL_ALPHA).hsla())
                .flex()
                .gap(px(BANNER_GAP_PX))
                .child(icon("bell", BANNER_ICON_PX, readable(palette.warning_amber, palette)))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(text::BODY_S))
                        .text_color(palette.on_surface_variant.hsla())
                        .child(crate::bidi::paragraph(t("Another notification daemon (e.g. mako) is running. Mandara takes over app notifications as soon as it exits."))),
                ),
        );
    }
    if state.items.is_empty() {
        h += tokens::MD + EMPTY_H;
        let empty = empty_state("bell", t("Nothing new"), t("Notifications from your apps land here"), EMPTY_H, palette);
        return (col.child(empty).into_any_element(), h, false);
    }
    // Group by app, preserving newest-first order of first appearance.
    let mut groups: Vec<(&str, Vec<&Notification>)> = Vec::new();
    for n in &state.items {
        match groups.iter_mut().find(|(app, _)| *app == n.app_name) {
            Some((_, list)) => list.push(n),
            None => groups.push((&n.app_name, vec![n])),
        }
    }
    let mut budget = limit;
    let mut hidden = false;
    for (gi, (app, list)) in groups.into_iter().enumerate() {
        let take = list.len().min(MAX_PER_GROUP).min(budget);
        hidden |= take < list.len().min(MAX_PER_GROUP);
        if take == 0 {
            continue;
        }
        budget -= take;
        let shown = &list[..take];
        h += tokens::MD + GROUP_HEAD_H + shown.iter().map(|n| card_height(n) + GROUP_GAP_PX).sum::<f32>();
        let mut group = div().flex().flex_col().gap(px(GROUP_GAP_PX)).child(group_header(gi, app, &list, palette));
        for n in shown {
            group = group.child(notification_card(n, palette, true));
        }
        col = col.child(match p {
            Some(p) => reveal(p, gi + 1, group),
            None => group.into_any_element(),
        });
    }
    (col.into_any_element(), h, hidden)
}

/// The "Show more" row under a paged list.
fn show_more(shown: Rc<Cell<usize>>, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    div()
        .h(px(MORE_H))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .id("notif-more")
                .px(px(CLEAR_PAD_X_PX))
                .h(px(LINK_H_PX))
                .rounded_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(accent.hsla())
                .hover(move |s| s.bg(accent.with_alpha(LINK_HOVER_ALPHA).hsla()))
                .child(t("Show more"))
                .on_click(move |_, window, _| {
                    shown.set(shown.get() + PAGE_SIZE);
                    window.refresh();
                }),
        )
        .into_any_element()
}

/// Why do not disturb switched itself on, for the panel subtitle.
fn auto_reason(reason: AutoDnd) -> String {
    match reason {
        AutoDnd::ScreenShare => t("Quiet while your screen is shared").to_string(),
        AutoDnd::Fullscreen => t("Quiet while a fullscreen app is focused").to_string(),
        AutoDnd::QuietHours => t("Quiet hours").to_string(),
    }
}

thread_local! {
    /// The notification being replied to, its draft, and the field.
    static REPLY: RefCell<Option<Reply>> = const { RefCell::new(None) };
}

struct Reply {
    id: u32,
    draft: Rc<RefCell<String>>,
    field: Option<Entity<crate::text_field::TextField>>,
    focused: bool,
}

/// Starts an inline reply: opens the notification center with a composer.
pub fn begin_reply(id: u32, cx: &mut App) {
    REPLY.with(|r| *r.borrow_mut() = Some(Reply { id, draft: Rc::default(), field: None, focused: false }));
    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
}

fn end_reply() {
    REPLY.with(|r| *r.borrow_mut() = None);
}

fn send_reply(cx: &mut App) {
    let target = REPLY.with(|r| r.borrow().as_ref().map(|r| (r.id, r.draft.borrow().trim().to_string())));
    if let Some((id, text)) = target
        && !text.is_empty()
    {
        send(cx, NotificationCommand::Reply { id, text });
        end_reply();
    }
}

/// The inline-reply composer shown at the top of the notification center.
fn reply_composer(state: &NotificationsState, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let id = REPLY.with(|r| r.borrow().as_ref().map(|r| r.id))?;
    let Some(n) = state.items.iter().find(|n| n.id == id && n.can_reply()) else {
        end_reply();
        return None;
    };
    let (field, focus_now) = REPLY.with(|r| {
        let mut r = r.borrow_mut();
        let reply = r.as_mut()?;
        let draft = reply.draft.clone();
        let field = reply
            .field
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    crate::text_field::TextField::new(String::new(), t("Write a reply…"), true, Rc::new(move |value, _, _| *draft.borrow_mut() = value), cx)
                })
            })
            .clone();
        let focus_now = !reply.focused;
        reply.focused = true;
        Some((field, focus_now))
    })?;
    if focus_now {
        field.update(cx, |f, cx| f.focus(window, cx));
    }
    let accent = readable(palette.primary, palette);
    Some(
        panel_fx::glass_card(palette)
            .id("notif-reply")
            .p(px(CARD_PAD_PX))
            .flex()
            .flex_col()
            .gap(px(TEXT_GAP_PX * 2.0))
            .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(tf("Reply to {app}: {summary}", &[("app", &n.app_name), ("summary", &n.summary)])))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(ACTION_GAP_PX))
                    .on_key_down(|ev: &gpui::KeyDownEvent, _, cx| {
                        if ev.keystroke.key == "enter" {
                            send_reply(cx);
                        } else if ev.keystroke.key == "escape" {
                            end_reply();
                        }
                    })
                    .child(div().flex_1().min_w(px(0.0)).child(field))
                    .child(pill_button("notif-reply-send", Some("arrow-up"), t("Send"), accent, palette, |_, cx| send_reply(cx)))
                    .child(icon_button("notif-reply-cancel", "close", DISMISS_PX, palette.on_surface_variant, None, |_, _| end_reply())),
            )
            .into_any_element(),
    )
}

/// A duration such as `30m`, `2h`, `1h30m` or `90` (minutes).
fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    if let Ok(minutes) = s.parse::<i64>() {
        return (minutes > 0).then_some(minutes * SECS_PER_MIN);
    }
    let (mut total, mut num) = (0i64, String::new());
    for c in s.chars() {
        match c {
            '0'..='9' => num.push(c),
            'h' | 'm' | 's' | 'd' => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match c {
                    'd' => SECS_PER_DAY,
                    'h' => SECS_PER_HOUR,
                    'm' => SECS_PER_MIN,
                    _ => 1,
                };
            }
            _ => return None,
        }
    }
    (num.is_empty() && total > 0).then_some(total)
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Notifications"), tk("Notification center with history and do-not-disturb"), Some(Zone::Right)), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let st = &hub(cx).read(cx).notifications;
                let unread = st.unread();
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon(if st.quiet() { "moon" } else { "bell" }, crate::style::bar_icon(BAR_ICON_PX), if unread > 0 { palette.primary } else { palette.on_surface }))
                    .when(unread > 0, |el| {
                        el.child(
                            div()
                                .min_w(px(BADGE_PX))
                                .h(px(BADGE_PX))
                                .px(px(BADGE_PAD_X_PX))
                                .rounded_full()
                                .bg(palette.primary.hsla())
                                .text_color(palette.on_primary.hsla())
                                .text_size(px(BADGE_TEXT_PX))
                                .font_weight(FontWeight::BOLD)
                                .font_features(tabular_nums())
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(unread.min(BADGE_MAX).to_string()),
                        )
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        let p = panel_cx.clone();
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_PANEL_H))));
        let sc = size_cell.clone();
        let marked = Rc::new(Cell::new(false));
        // Cards shown in the panel; panel-only state, reset on close.
        let page = Rc::new(Cell::new(PAGE_SIZE));
        let panel = fn_view(
            None,
            move |palette, window, cx| {
                // Opening the panel marks everything read.
                let unread = hub(cx).read(cx).notifications.unread();
                if p.progress() > SEEN_PROGRESS && unread > 0 && !marked.get() {
                    marked.set(true);
                    send(cx, NotificationCommand::MarkAllRead);
                } else if p.progress() < CLOSED_PROGRESS {
                    marked.set(false);
                    page.set(PAGE_SIZE);
                }
                let hub = hub(cx);
                if p.progress() > 0.0
                    && let Some(delay) = relabel_delay(&hub.read(cx).notifications.items)
                {
                    redraw_in(delay, window, cx);
                }
                let r = PanelShape::of(cx).inner(PAD);
                if p.progress() < CLOSED_PROGRESS {
                    end_reply();
                }
                let composer = reply_composer(&hub.read(cx).notifications.clone(), palette, window, cx);
                // Borrow this frame's history instead of cloning it.
                let (body, h, more) = center(&hub.read(cx).notifications, palette, Some(&p), r, page.get());
                let h = if more { h + MORE_H } else { h } + if composer.is_some() { REPLY_H + tokens::MD } else { 0.0 };
                sc.set(size(px(PANEL_W), px((PAD * 2.0 + h).min(MAX_PANEL_H))));
                div()
                    .id("notif-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .p(px(PAD))
                    .when_some(composer, |el, c| el.child(div().pb(px(tokens::MD)).child(c)))
                    .child(body)
                    .when(more, |el| el.child(show_more(page.clone(), palette)))
                    .into_any_element()
            },
            window,
            cx,
        );
        let sidebar = fn_view(
            None,
            |palette, window, cx| {
                let hub = hub(cx);
                // Only rendered while the sidebar shows: tick then, not on a timer.
                if let Some(delay) = relabel_delay(&hub.read(cx).notifications.items) {
                    redraw_in(delay, window, cx);
                }
                card(palette).child(center(&hub.read(cx).notifications, palette, None, SIDEBAR_RADIUS_PX, SIDEBAR_LIMIT).0).into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            sidebar: Some(sidebar),
            on_click: None,
            on_secondary_click: Some(Rc::new(|_, cx| {
                let dnd = hub(cx).read(cx).notifications.dnd;
                send(cx, NotificationCommand::SetDnd(!dnd));
            })),
        }
    });

    r.command(ID, CommandSpec::new("notifications list", tk("Notification history, newest first"), |_, cx| Ok(json(&hub(cx).read(cx).notifications.items))));
    r.command(
        ID,
        CommandSpec::new_output("notifications clear", tk("Clear all, or one app's notifications"), |a, cx| {
            match a.get(0) {
                Some(_) => request(cx, NotificationCommand::DismissApp(a.rest(0))),
                None => request(cx, NotificationCommand::DismissAll),
            }
        })
        .arg(ArgSpec::optional("app", ArgKind::String, tk("app name"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notifications dismiss", tk("Dismiss one notification by id"), |a, cx| request(cx, NotificationCommand::Dismiss(notification_id(a.int(0)?)?)))
            .arg(ArgSpec::required("id", ArgKind::Integer, tk("notification id"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notifications invoke", tk("Invoke a notification action"), |a, cx| {
            request(cx, NotificationCommand::Invoke { id: notification_id(a.int(0)?)?, action: a.get(1).unwrap_or("default").to_string() })
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, tk("notification id")))
        .arg(ArgSpec::optional("action", ArgKind::String, tk("action key (default: \"default\")"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("notifications dnd", tk("Do not disturb on / off / toggle, optionally for a while (30m, 2h, 1h30m)"), |a, cx| {
            let current = hub(cx).read(cx).notifications.dnd;
            let on = match a.str(0)? {
                "on" => true,
                "off" => false,
                _ => !current,
            };
            match a.get(1) {
                Some(d) if on => {
                    let secs = parse_duration(d).ok_or_else(|| tf("\"{value}\" isn't a duration like 30m, 2h or 1h30m", &[("value", &d)]))?;
                    request(cx, NotificationCommand::DndUntil(Local::now().timestamp() + secs))
                }
                _ => request(cx, NotificationCommand::SetDnd(on)),
            }
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on / off / toggle")).choices(&["on", "off", "toggle"]))
        .arg(ArgSpec::optional("for", ArgKind::String, tk("how long, e.g. 45m or 2h")))
        .example("notifications dnd on 1h"),
    );
    r.command(
        ID,
        CommandSpec::new("notifications status", tk("Do not disturb state, server status and counts"), |_, cx| {
            let st = &hub(cx).read(cx).notifications;
            Ok(serde_json::json!({
                "serving": st.active,
                "dnd": st.dnd,
                "dnd_until": st.dnd_until,
                "auto_dnd": st.auto_dnd,
                "quiet": st.quiet(),
                "total": st.items.len(),
                "unread": st.unread(),
                "popups": st.items.iter().filter(|n| n.popup).count(),
            }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("notifications send", tk("Post a notification from the shell (--low / --critical for urgency)"), |a, cx| {
            let mut words: Vec<&str> = Vec::new();
            let mut urgency = Urgency::Normal;
            let mut i = 0;
            while let Some(w) = a.get(i) {
                match w {
                    "--low" => urgency = Urgency::Low,
                    "--critical" => urgency = Urgency::Critical,
                    other => words.push(other),
                }
                i += 1;
            }
            let summary = words.first().ok_or_else(|| t("A title is needed").to_string())?.to_string();
            let body = words[1..].join(" ");
            request(cx, NotificationCommand::PostUrgent { app: "Mandara".into(), icon: String::new(), summary, body, urgency })
        })
        .arg(ArgSpec::required("summary", ArgKind::String, tk("title")))
        .arg(ArgSpec::optional("body", ArgKind::String, tk("text")))
        .example("notifications send Hello \"from Mandara\"")
        .example("notifications send --critical Disk almost full"),
    );
    r.command(
        ID,
        CommandSpec::new_output("notifications reply", tk("Answer a notification that offers an inline reply"), |a, cx| {
            request(cx, NotificationCommand::Reply { id: notification_id(a.int(0)?)?, text: a.rest(1) })
        })
        .arg(ArgSpec::required("id", ArgKind::Integer, tk("notification id")))
        .arg(ArgSpec::required("text", ArgKind::String, tk("the reply"))),
    );
    r.command(
        ID,
        CommandSpec::new("notifications rules list", tk("Per-app notification rules, in order"), |_, cx| {
            Ok(json(&crate::state::config(cx).read(cx).get().notifications.rules))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("notifications rules add", tk("Add a rule for an app: mute, silent, no-history, allow-dnd, low, critical"), |a, cx| {
            let effect = a.str(0)?.to_string();
            let app = a.rest(1);
            if app.trim().is_empty() {
                return Err(t("Name the app the rule is for").to_string());
            }
            let rule = rule_for(&effect, app.trim()).ok_or_else(|| tf("Unknown effect \"{effect}\"", &[("effect", &effect)]))?;
            crate::state::update_config(cx, |c| c.notifications.rules.push(rule));
            Ok(serde_json::json!({ "rules": crate::state::config(cx).read(cx).get().notifications.rules.len() }))
        })
        .arg(ArgSpec::required("effect", ArgKind::Enum, tk("what the rule does")).choices(RULE_EFFECTS))
        .arg(ArgSpec::required("app", ArgKind::String, tk("app name (a regular expression)")))
        .example("notifications rules add silent Spotify"),
    );
    r.command(
        ID,
        CommandSpec::new("notifications rules remove", tk("Remove a rule by its position (from 1)"), |a, cx| {
            let n = a.int(0)?;
            let len = crate::state::config(cx).read(cx).get().notifications.rules.len();
            let i = usize::try_from(n).ok().filter(|i| (1..=len).contains(i)).ok_or_else(|| tf("There is no rule {n}", &[("n", &n)]))? - 1;
            crate::state::update_config(cx, |c| {
                c.notifications.rules.remove(i);
            });
            Ok(serde_json::json!({ "rules": len - 1 }))
        })
        .arg(ArgSpec::required("position", ArgKind::Integer, tk("rule number from `notifications rules list`"))),
    );
}

/// Effects `notifications rules add` understands.
const RULE_EFFECTS: &[&str] = &["mute", "silent", "no-history", "allow-dnd", "no-sound", "low", "critical"];

fn rule_for(effect: &str, app: &str) -> Option<mandara_config::alerts::NotificationRule> {
    use mandara_config::alerts::{NotificationRule, UrgencyLevel};
    let base = NotificationRule::for_app(app);
    Some(match effect {
        "mute" => NotificationRule { block: true, ..base },
        "silent" => NotificationRule { popup: false, sound: false, ..base },
        "no-history" => NotificationRule { history: false, ..base },
        "allow-dnd" => NotificationRule { bypass_dnd: true, ..base },
        "no-sound" => NotificationRule { sound: false, ..base },
        "low" => NotificationRule { set_urgency: Some(UrgencyLevel::Low), ..base },
        "critical" => NotificationRule { set_urgency: Some(UrgencyLevel::Critical), ..base },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_labels_tick_at_their_granularity() {
        assert_eq!(next_label_change(0), Some(60));
        assert_eq!(next_label_change(59), Some(1));
        assert_eq!(next_label_change(61), Some(59));
        assert_eq!(next_label_change(3600), Some(3600));
        assert_eq!(next_label_change(3601 + 1800), Some(1799));
        assert_eq!(next_label_change(90_000), None);
        assert_eq!(next_label_change(-5), Some(60), "clock skew reads as now");
    }

    #[test]
    fn durations_and_rule_effects_parse() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("90"), Some(5400));
        assert_eq!(parse_duration("2d"), Some(172_800));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration("0"), None);
        assert_eq!(parse_duration("5x"), None);
        for e in RULE_EFFECTS {
            assert!(rule_for(e, "app").is_some(), "{e}");
        }
        assert!(rule_for("mute", "x").unwrap().block);
    }

    #[test]
    fn ids_are_range_checked() {
        assert_eq!(notification_id(7), Ok(7));
        assert!(notification_id(-1).is_err() && notification_id(i64::from(u32::MAX) + 1).is_err());
    }
}
