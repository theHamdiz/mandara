//! Share to phone: puts one thing — the clipboard (text, an image or a
//! copied file), the latest screenshot, or a file given on the command
//! line — on the local network behind a QR code, so a phone can open it
//! with its camera. Nothing is served until you pick something; only that
//! one item is served (`mandara_services::share`).
//!
//! The panel shows what is shared (a thumbnail, the size or a text
//! preview), the QR code on a light tile that scans in dark themes too,
//! the link (click to copy), a countdown ring until it stops, the download
//! count and Stop. Idle, it offers Clipboard and Latest screenshot. The bar
//! chip appears only while sharing: a gently pulsing glyph. The server
//! thread reports downloads and its end through a channel; the countdown
//! and the pulse redraw only while their view is on screen. Settings
//! (`[plugins.settings.share]`): `minutes`, `once`. Commands:
//! `share clipboard|text|file|screenshot|stop|status`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, FontWeight, ImageSource, InteractiveElement, IntoElement, ObjectFit, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::share::{self as sh, Ended, Event, Item, Kind, Qr, Share};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::extras::{Flash, caption, toast};
use super::model_view as fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, SMALL_ICON_PX, ViewSet, WASH_FAINT, WASH_SOFT, alpha, dot_ring, empty_state, glass_card, header, icon_chip, partner, pill_button,
    readable, redraw_in, wash,
};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};
use crate::widgets::system::format_bytes;

pub const ID: &str = "share";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 420.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
/// Idle: the two big buttons and the command hint.
const BIG_H: f32 = 104.0;
const BIG_GAP_PX: f32 = 10.0;
const BIG_ICON_PX: f32 = 40.0;
const BIG_FILL: (f32, f32) = (0.16, 0.08);
const BIG_HOVER: (f32, f32) = (0.24, 0.13);
const HINT_H: f32 = 58.0;
const HINT_PAD_PX: f32 = 12.0;
const BUSY_H: f32 = 150.0;
/// The item card.
const ITEM_H: f32 = 72.0;
const ITEM_PAD_PX: f32 = 12.0;
const THUMB_PX: f32 = 48.0;
const THUMB_RADIUS_PX: f32 = 10.0;
/// Decoded thumbnails are this many times the drawn size (sharp on HiDPI).
const THUMB_DECODE_SCALE: f32 = 2.0;
const ITEM_FILL: (f32, f32) = (0.14, 0.05);
/// QR tile: total side, quiet zone in modules, colours (dark on light in
/// every theme so phones read it).
const QR_SIDE_PX: f32 = 196.0;
const QR_QUIET_MODULES: usize = 4;
const QR_RADIUS_PX: f32 = 14.0;
const QR_LIGHT: (u8, u8, u8) = (255, 255, 255);
const QR_DARK: (u8, u8, u8) = (17, 17, 17);
/// The column beside the QR code: countdown ring and its text.
const RING_D_PX: f32 = 96.0;
const RING_DOTS: usize = 30;
const RING_DOT_PX: f32 = 4.0;
const RING_TEXT_PX: f32 = 18.0;
const RING_TRACK: (f32, f32) = (0.12, 0.1);
/// The link row.
const URL_H: f32 = 34.0;
const URL_PAD_X_PX: f32 = 12.0;
const URL_RADIUS_PX: f32 = 12.0;
/// A Stop press arms the button; a second press within this confirms.
const ARM: Duration = Duration::from_secs(3);
/// Bar chip: glyph, pulse period, floor and frame cadence.
const CHIP_ICON_PX: f32 = 15.0;
const PULSE_PERIOD_S: f32 = 1.8;
const PULSE_MIN_ALPHA: f32 = 0.45;
const PULSE_FRAME: Duration = Duration::from_millis(80);
const SIZE_EPSILON_PX: f32 = 0.5;
/// Gradient partners fade to this share of the accent's alpha.
const PARTNER_FADE: f32 = 0.35;
/// Gap between stacked text lines, and the command hint's glyph.
const LINE_GAP_PX: f32 = 2.0;
const HINT_ICON_PX: f32 = 14.0;
/// The open panel's countdown cadence.
const TICK: Duration = Duration::from_secs(1);
const SECS_PER_MINUTE: u64 = 60;

fn ttl(cx: &App) -> Duration {
    let m: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "minutes", sh::DEFAULT_MINUTES);
    Duration::from_secs(m.clamp(sh::MINUTES_RANGE.0, sh::MINUTES_RANGE.1).unsigned_abs() * SECS_PER_MINUTE)
}

fn once(cx: &App) -> bool {
    crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "once", true)
}

fn screenshot_dir(cx: &App) -> PathBuf {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    mandara_services::screenshot::output_dir(&mandara_config::workflow::ScreenshotSettings::from_plugins(&cfg.plugins).directory)
}

// ── Model (one per shell process) ────────────────────────────────────────

/// What to share.
#[derive(Clone, Debug)]
enum Source {
    Clipboard,
    Text(String),
    File(PathBuf),
    Screenshot(PathBuf),
}

struct Active {
    share: Share,
    qr: Option<Qr>,
    thumb: Option<Arc<RenderImage>>,
    /// Size or a text preview, formatted once.
    detail: SharedString,
    downloads: u32,
}

#[derive(Default)]
struct Model {
    active: Option<Active>,
    starting: bool,
    armed: Option<Instant>,
    flash: Flash,
    views: ViewSet,
}

thread_local! {
    static MODEL: Rc<RefCell<Model>> = Rc::new(RefCell::new(Model::default()));
}

fn model() -> Rc<RefCell<Model>> {
    MODEL.with(Rc::clone)
}

fn notify(cx: &mut App) {
    let views = model().borrow().views.clone();
    views.notify(cx);
}

/// A GPUI thumbnail (BGRA) of an image, at most `side` px.
fn thumbnail(image: image::DynamicImage, side: u32) -> Arc<RenderImage> {
    let resized = image.thumbnail(side, side);
    drop(image);
    let mut rgba = resized.into_rgba8();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))
}

/// Builds the item and starts serving it (blocking: reads the clipboard
/// or a file, binds a port, decodes the thumbnail).
fn prepare(source: Source, ttl: Duration, once: bool) -> Result<(Share, Option<Qr>, Option<Arc<RenderImage>>), String> {
    let item = match source {
        Source::Clipboard => sh::clipboard_item()?,
        Source::Text(text) => Item::text(&text)?,
        Source::File(path) => Item::file(&path)?,
        Source::Screenshot(dir) => Item::file(&sh::latest_screenshot(&dir)?)?,
    };
    let side = (THUMB_PX * THUMB_DECODE_SCALE) as u32;
    let thumb = match (item.kind, item.image_data(), &item.path) {
        (Kind::Image, Some(bytes), _) => image::load_from_memory(&bytes).ok().map(|i| thumbnail(i, side)),
        (Kind::Image, None, Some(path)) => image::open(path).ok().map(|i| thumbnail(i, side)),
        _ => None,
    };
    let share = sh::start(item, ttl, once)?;
    let qr = Qr::new(&share.url);
    Ok((share, qr, thumb))
}

fn detail(item: &Item) -> SharedString {
    match &item.preview {
        Some(p) => p.split_whitespace().collect::<Vec<_>>().join(" ").into(),
        None => format_bytes(item.size).into(),
    }
}

fn status_json(a: Option<&Active>) -> Value {
    match a {
        Some(a) if a.share.is_running() => json!({
            "sharing": true, "url": a.share.url, "kind": a.share.item.kind, "name": a.share.item.name, "size": a.share.item.size,
            "expires_in_s": a.share.expires_in().as_secs(), "expires_at": a.share.expires_at, "downloads": a.downloads, "once": a.share.once,
        }),
        _ => json!({ "sharing": false }),
    }
}

/// Follows the server's news for the share at `url`.
fn watch(url: String, events: async_channel::Receiver<Event>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Ok(ev) = events.recv().await {
            let m = model();
            {
                let mut mb = m.borrow_mut();
                let ours = mb.active.as_ref().is_some_and(|a| a.share.url == url);
                if !ours {
                    break;
                }
                match ev {
                    Event::Downloaded(n) => {
                        if let Some(a) = mb.active.as_mut() {
                            a.downloads = n;
                        }
                    }
                    Event::Ended(why) => {
                        mb.active = None;
                        mb.armed = None;
                        let msg = match why {
                            Ended::Downloaded => t("Downloaded: sharing stopped"),
                            Ended::Expired => t("The share expired"),
                            Ended::Stopped => t("Sharing stopped"),
                        };
                        mb.flash.show(true, msg);
                    }
                }
            }
            let views = m.borrow().views.clone();
            views.notify_async(cx);
        }
    })
    .detach();
}

/// Starts sharing `source` (replacing any share); the reply is the status.
fn start(source: Source, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let m = model();
    if std::mem::replace(&mut m.borrow_mut().starting, true) {
        let _ = tx.try_send(Err(t("Already preparing a share").into()));
        return rx;
    }
    notify(cx);
    let (ttl, once) = (ttl(cx), once(cx));
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(move || prepare(source, ttl, once)).await;
        let reply = {
            let mut mb = m.borrow_mut();
            mb.starting = false;
            mb.armed = None;
            match result {
                Ok((share, qr, thumb)) => {
                    let url = share.url.clone();
                    let events = share.events.clone();
                    let detail = detail(&share.item);
                    // The previous share (if any) stops as it drops.
                    mb.active = Some(Active { share, qr, thumb, detail, downloads: 0 });
                    let reply = status_json(mb.active.as_ref());
                    drop(mb);
                    cx.update(|cx| watch(url, events, cx));
                    Ok(reply.to_string())
                }
                Err(e) => {
                    mb.flash.show(false, e.clone());
                    Err(e)
                }
            }
        };
        let _ = tx.try_send(reply);
        let views = m.borrow().views.clone();
        views.notify_async(cx);
    })
    .detach();
    rx
}

fn stop(cx: &mut App) -> bool {
    let m = model();
    let stopped = m.borrow_mut().active.take().map(|a| a.share.stop()).is_some();
    m.borrow_mut().armed = None;
    if stopped {
        m.borrow().flash.show(true, t("Sharing stopped"));
    }
    notify(cx);
    stopped
}

fn copy(text: &str, cx: &mut App) {
    let m = model();
    match mandara_services::clipboard::serve(text.as_bytes().to_vec(), None) {
        Ok(()) => m.borrow().flash.show(true, t("Link copied")),
        Err(e) => m.borrow().flash.show(false, e),
    }
    notify(cx);
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::from_rgba8(r, g, b, u8::MAX)
}

/// The QR code as crisp square modules: one bar per run of dark modules,
/// on a light tile with the quiet zone. Never mirrored.
fn qr_tile(qr: &Qr) -> AnyElement {
    let total = qr.size + QR_QUIET_MODULES * 2;
    let module = (QR_SIDE_PX / total as f32).floor().max(1.0);
    let dark = rgb(QR_DARK).hsla();
    let rows = (0..qr.size).map(|y| {
        div().ltr_row().h(px(module)).children(qr.runs(y).into_iter().map(|(_, len, on)| {
            let cell = div().w(px(len as f32 * module)).h_full();
            if on { cell.bg(dark) } else { cell }
        }))
    });
    let side = module * total as f32;
    div()
        .size(px(side))
        .flex_none()
        .p(px(module * QR_QUIET_MODULES as f32))
        .rounded(px(crate::style::radius(QR_RADIUS_PX)))
        .bg(rgb(QR_LIGHT).hsla())
        .flex()
        .flex_col()
        .children(rows)
        .into_any_element()
}

fn kind_glyph(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "note",
        Kind::Image => "image",
        Kind::File => "folder",
    }
}

fn item_card(a: &Active, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let item = &a.share.item;
    let lead: AnyElement = match &a.thumb {
        Some(image) => img(ImageSource::Render(image.clone())).size(px(THUMB_PX)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS_PX))).object_fit(ObjectFit::Cover).into_any_element(),
        None => icon_chip(kind_glyph(item.kind), accent, THUMB_PX).into_any_element(),
    };
    let text_item = item.kind == Kind::Text;
    glass_card(palette)
        .h(px(ITEM_H))
        .p(px(ITEM_PAD_PX))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(alpha(ITEM_FILL, palette)), partner(accent).with_alpha(alpha(ITEM_FILL, palette) * PARTNER_FADE)))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(div().truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(item.name.clone()))
                .child(
                    div()
                        .text_size(px(text::BODY_S))
                        .text_color(palette.on_surface_variant.hsla())
                        .when(text_item, |el| el.line_clamp(2))
                        .when(!text_item, |el| el.truncate().font_features(tabular_nums()))
                        .child(a.detail.clone()),
                ),
        )
        .into_any_element()
}

fn mm_ss(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / SECS_PER_MINUTE, s % SECS_PER_MINUTE)
}

fn side_column(a: &Active, armed: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let left = a.share.expires_in();
    let total = a.share.deadline.saturating_duration_since(a.share.started).as_secs_f32().max(1.0);
    let ring = dot_ring(
        left.as_secs_f32() / total,
        RING_D_PX,
        RING_DOTS,
        RING_DOT_PX,
        accent,
        partner(accent),
        palette.on_surface.with_alpha(alpha(RING_TRACK, palette)),
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(div().text_size(px(RING_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).child(mm_ss(left)))
            .child(caption(t("left"), palette))
            .into_any_element(),
    );
    let downloads = match a.downloads {
        0 => t("No downloads yet").to_string(),
        1 => t("Downloaded once").to_string(),
        n => tf("Downloaded {count} times", &[("count", &n)]),
    };
    let coral = readable(palette.urgent_coral, palette);
    let button = pill_button("share-stop", Some(if armed { "check" } else { "stop" }), if armed { t("Stop sharing?") } else { t("Stop") }, coral, palette, move |_, cx| {
        let m = model();
        let confirm = m.borrow().armed.is_some_and(|at| at.elapsed() < ARM);
        if confirm {
            stop(cx);
        } else {
            m.borrow_mut().armed = Some(Instant::now());
            notify(cx);
        }
    });
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .h(px(QR_SIDE_PX))
        .child(ring)
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(LINE_GAP_PX))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).font_features(tabular_nums()).child(downloads))
                .when(a.share.once, |el| el.child(caption(t("Stops after the first download"), palette))),
        )
        .child(button)
        .into_any_element()
}

fn url_row(url: &str, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let link = url.to_string();
    let hover = wash(WASH_SOFT, palette);
    div()
        .id("share-url")
        .h(px(URL_H))
        .px(px(URL_PAD_X_PX))
        .rounded(px(crate::style::radius(URL_RADIUS_PX)))
        .bg(wash(WASH_FAINT, palette).hsla())
        .hover(move |s| s.bg(hover.hsla()))
        .cursor_pointer()
        .ltr_row()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("globe", SMALL_ICON_PX, palette.on_surface_variant))
        .child(div().flex_1().min_w(px(0.0)).truncate().font_family(fonts(cx).mono).text_size(px(text::LABEL)).child(url.to_string()))
        .child(icon("copy", SMALL_ICON_PX, palette.on_surface_variant))
        .on_click(move |_, _, cx| copy(&link, cx))
        .into_any_element()
}

fn big_button(id: &'static str, glyph: &'static str, title: &'static str, hint: &'static str, palette: &MandaraPalette, on_click: impl Fn(&mut App) + 'static) -> AnyElement {
    let accent = readable(palette.primary, palette);
    use crate::components::buttons::{Ink, designed};
    let (fill, hover) = (accent.with_alpha(alpha(BIG_FILL, palette)), accent.with_alpha(alpha(BIG_HOVER, palette)));
    let corner = crate::style::radius(super::panel_fx::CARD_RADIUS_PX);
    let el = div().id(id).flex_1().h(px(BIG_H));
    // A tile: its designed shape is sized from its card corner, not its
    // height (a round tile this tall would be a stadium). Designed shapes are
    // filled with the gradient's start.
    designed(el, id, corner * 2.0, false, Ink { bg: Some(fill), hover, outline: None }, Some(palette), |el| {
        el.rounded(px(corner))
            .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, fill, partner(accent).with_alpha(alpha(BIG_FILL, palette) * PARTNER_FADE)))
            .hover(move |s| s.bg(hover.hsla()))
    })
        .cursor_pointer()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(tokens::XS))
        .child(icon_chip(glyph, accent, BIG_ICON_PX))
        .child(div().mt(px(tokens::XS)).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title))
        .child(caption(hint, palette))
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

fn idle_view(palette: &MandaraPalette, cx: &App) -> Vec<AnyElement> {
    let dir = screenshot_dir(cx);
    let buttons = div()
        .row()
        .gap(px(BIG_GAP_PX))
        .child(big_button("share-clip", "clipboard", t("Clipboard"), t("Text, an image or a file"), palette, |cx| drop(start(Source::Clipboard, cx))))
        .child(big_button("share-shot", "image", t("Latest screenshot"), t("The newest one saved"), palette, move |cx| drop(start(Source::Screenshot(dir.clone()), cx))))
        .into_any_element();
    let hint = glass_card(palette)
        .h(px(HINT_H))
        .p(px(HINT_PAD_PX))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(icon("terminal", HINT_ICON_PX, palette.on_surface_variant))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(caption(t("Share any file from a terminal:"), palette))
                .child(div().truncate().font_family(fonts(cx).mono).text_size(px(text::LABEL)).child("mandara msg share file <path>")),
        )
        .into_any_element();
    vec![buttons, hint]
}

// ── Views ────────────────────────────────────────────────────────────────

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let m = model();
    m.borrow().views.track(window);
    if m.borrow().active.is_none() {
        return div().into_any_element();
    }
    redraw_in(PULSE_FRAME, window, cx);
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f32() % PULSE_PERIOD_S).unwrap_or(0.0);
    let wave = 0.5 + 0.5 * (std::f32::consts::TAU * secs / PULSE_PERIOD_S).cos();
    let tint = readable(palette.primary, palette).with_alpha(PULSE_MIN_ALPHA + (1.0 - PULSE_MIN_ALPHA) * wave);
    div().flex().items_center().child(icon("external", crate::style::bar_icon(CHIP_ICON_PX), tint)).into_any_element()
}

struct View {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn set_height(&self, h: f32) {
        if (f32::from(self.size_cell.get().height) - h).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = model();
        m.borrow().views.track(window);
        let flash = m.borrow().flash.clone().current(window, cx);
        let accent = readable(palette.primary, palette);
        let mut h = PAD * 2.0 + HEADER_H;
        let mut blocks: Vec<AnyElement> = Vec::new();
        let mb = m.borrow();
        let armed = mb.armed.is_some_and(|at| at.elapsed() < ARM);
        if armed {
            redraw_in(ARM.saturating_sub(mb.armed.map(|a| a.elapsed()).unwrap_or_default()) + EXPIRY_MARGIN, window, cx);
        }
        let subtitle = match (&mb.active, mb.starting) {
            (_, true) => t("Getting it ready…"),
            (Some(_), _) => t("Scan the code with your phone's camera"),
            (None, _) => t("Open something on your phone, no cable or app needed"),
        };
        if mb.starting {
            h += GAP + BUSY_H;
            blocks.push(empty_state("external", t("Preparing the share…"), t("Starting a private link on your network"), BUSY_H, palette).into_any_element());
        } else if let Some(a) = &mb.active {
            if self.p.progress() > 0.0 {
                redraw_in(TICK, window, cx);
            }
            h += GAP + ITEM_H + GAP + QR_SIDE_PX + GAP + URL_H;
            blocks.push(item_card(a, palette));
            let qr = match &a.qr {
                Some(qr) => qr_tile(qr),
                None => div().size(px(QR_SIDE_PX)).flex_none().into_any_element(),
            };
            blocks.push(div().row().items_center().gap(px(GAP)).child(qr).child(side_column(a, armed, palette)).into_any_element());
            blocks.push(url_row(&a.share.url, palette, cx));
        } else {
            h += GAP + BIG_H + GAP + HINT_H;
            blocks.extend(idle_view(palette, cx));
        }
        drop(mb);
        self.set_height(h);
        let mut col = div().flex().flex_col().gap(px(GAP)).child(reveal(&self.p, 0, header("external", accent, t("Share to phone"), subtitle, palette, None)));
        for (i, b) in blocks.into_iter().enumerate() {
            col = col.child(reveal(&self.p, i + 1, b));
        }
        div().relative().size_full().p(px(PAD)).child(col).when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette))).into_any_element()
    }
}

// ── Commands ─────────────────────────────────────────────────────────────

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Share to phone"), tk("Send the clipboard, a screenshot or a file to your phone with a QR code"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(None, bar_view, window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + BIG_H + GAP + HINT_H))));
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new_output("share clipboard", tk("Share what the clipboard holds: text, an image or a copied file (JSON with the link)"), |_, cx| Ok(Output::Pending(start(Source::Clipboard, cx)))));
    r.command(
        ID,
        CommandSpec::new_output("share text", tk("Share a piece of text as a page with a copy button"), |a, cx| Ok(Output::Pending(start(Source::Text(a.rest(0)), cx))))
            .arg(ArgSpec::required("text", ArgKind::String, tk("the text to share")))
            .example("share text Wi-Fi password is on the fridge"),
    );
    r.command(
        ID,
        CommandSpec::new_output("share file", tk("Share one file (never a folder)"), |a, cx| {
            let path = mandara_config::paths::expand_tilde(&a.rest(0));
            if !path.is_absolute() {
                return Err(t("Give the file's full path").into());
            }
            Ok(Output::Pending(start(Source::File(path), cx)))
        })
        .arg(ArgSpec::required("path", ArgKind::String, tk("the file to share")))
        .example("share file ~/Documents/boarding-pass.pdf"),
    );
    r.command(ID, CommandSpec::new_output("share screenshot", tk("Share the newest screenshot"), |_, cx| Ok(Output::Pending(start(Source::Screenshot(screenshot_dir(cx)), cx)))));
    r.command(ID, CommandSpec::new("share stop", tk("Stop sharing now"), |_, cx| Ok(json!({ "stopped": stop(cx) }))));
    r.command(ID, CommandSpec::new("share status", tk("What is shared: link, kind, name, time left and downloads (JSON)"), |_, _| Ok(status_json(model().borrow().active.as_ref()))));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_reads_minutes_and_seconds() {
        assert_eq!(mm_ss(Duration::from_secs(600)), "10:00");
        assert_eq!(mm_ss(Duration::from_secs(61)), "1:01");
        assert_eq!(mm_ss(Duration::from_millis(900)), "0:00");
    }

    #[test]
    fn previews_collapse_whitespace() {
        let item = Item::text("line one\n\n   line two").unwrap();
        assert_eq!(detail(&item).as_ref(), "line one line two");
        assert_eq!(status_json(None), json!({ "sharing": false }));
    }

    /// Thumbnail ownership changes keep aspect, channel order and alpha exact.
    #[test]
    fn thumbnails_match_borrowed_conversion_for_rgb_and_rgba() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(12, 8, |x, y| image::Rgba([(x * 17) as u8, (y * 23) as u8, 73, ((x + y) * 11) as u8])));
        for image in [source.clone(), image::DynamicImage::ImageRgb8(source.to_rgb8())] {
            let expected = image.thumbnail(6, 6).to_rgba8();
            let expected = expected.pixels().flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]]).collect::<Vec<_>>();
            let actual = thumbnail(image, 6);
            assert_eq!(actual.as_bytes(0).unwrap(), expected);
            assert_eq!((actual.size(0).width.0, actual.size(0).height.0), (6, 4));
        }
    }

    #[test]
    fn kinds_have_bundled_glyphs() {
        for k in [Kind::Text, Kind::Image, Kind::File] {
            assert!(crate::icons::svg_source(kind_glyph(k)).is_some());
        }
    }
}
