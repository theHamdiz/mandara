//! Tailscale: a bar glyph tinted by the connection state (with a dot while
//! an exit node is in use) and a panel with this device (name, address,
//! Connect / Disconnect), an exit-node picker and every peer with its
//! online state, OS, address, direct or relayed path and when it was last
//! seen. Data comes from `tailscale status --json`
//! (`mandara_services::tailscale`), read off the UI thread.
//!
//! No timer runs: a render of the chip asks for fresh data when it is older
//! than 60 s and schedules its own redraw for then, the open panel does the
//! same every 5 s, so nothing is read while neither is on screen. No
//! settings. Commands: `tailscale status|up|down|exit-node`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::tailscale::{self as ts, Backend, Error, Node, Status};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::extras::{Flash, caption, toast};
use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, SECTION_H_PX, SMALL_ICON_PX, ViewSet, WASH_SOFT, alpha, duration_short, empty_state, glass_card, glow, header, icon_chip, partner,
    pill_button, readable, redraw_in, section, wash,
};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "tailscale";
/// Official distribution-specific setup instructions, opened only on request.
const DOWNLOAD_URL: &str = "https://tailscale.com/download/linux";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 400.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const EMPTY_H: f32 = 170.0;
/// Hero card: height, padding, OS chip side, name size.
const HERO_H: f32 = 96.0;
const HERO_PAD_PX: f32 = 16.0;
const HERO_CHIP_PX: f32 = 44.0;
const HERO_NAME_PX: f32 = 17.0;
const HERO_LINE_GAP_PX: f32 = 3.0;
const HERO_FILL: (f32, f32) = (0.2, 0.06);
/// Problem note under the hero: height without and with the operator hint.
const NOTE_H: f32 = 52.0;
const NOTE_HINT_H: f32 = 84.0;
const NOTE_PAD_PX: f32 = 12.0;
const NOTE_RADIUS_PX: f32 = 14.0;
const NOTE_FILL_ALPHA: f32 = 0.12;
const HINT_H_PX: f32 = 26.0;
const HINT_PAD_X_PX: f32 = 10.0;
/// Exit-node chips: height, padding, gap, rows assumed per this many chips.
const EXIT_CHIP_H: f32 = 28.0;
const EXIT_CHIP_PAD_X_PX: f32 = 11.0;
const EXIT_GAP_PX: f32 = 6.0;
const EXIT_PER_ROW: usize = 4;
const SECTION_GAP_PX: f32 = tokens::XS + 2.0;
/// Peer rows.
const ROW_H: f32 = 48.0;
const ROW_GAP_PX: f32 = 4.0;
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_INNER_GAP_PX: f32 = 10.0;
const ROW_GLYPH_PX: f32 = 32.0;
const ROW_ICON_PX: f32 = 15.0;
const ROW_FILL: (f32, f32) = (0.16, 0.08);
const OFFLINE_OPACITY: f32 = 0.75;
/// Online dot on a peer glyph.
const DOT_PX: f32 = 9.0;
const DOT_OFFSET_PX: f32 = -1.0;
const DOT_GLOW: (f32, f32) = (0.55, 5.0);
/// Path badge.
const BADGE_H_PX: f32 = 20.0;
const BADGE_PAD_X_PX: f32 = 8.0;
const BADGE_TEXT_PX: f32 = 10.5;
const BADGE_FILL_ALPHA: f32 = 0.14;
/// Peer rows shown before the list scrolls.
const VISIBLE_ROWS: f32 = 6.5;
/// Bar chip.
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_DOT_PX: f32 = 6.0;
const CHIP_DOT_OFFSET_PX: f32 = -2.0;
const CHIP_IDLE_ALPHA: f32 = 0.6;
/// Panel height changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;
/// Gradient partners fade to this share of the accent's alpha.
const PARTNER_FADE: f32 = 0.4;
/// Small glyphs: the copy mark beside addresses, the problem note's alert.
const COPY_ICON_PX: f32 = 10.0;
const ALERT_ICON_PX: f32 = 14.0;
/// Exit-node chip: dot-to-label gap and label size.
const EXIT_INNER_GAP_PX: f32 = 5.0;
const EXIT_TEXT_PX: f32 = 11.5;

// ── Timing ───────────────────────────────────────────────────────────────

/// How often the chip refreshes (while it is on screen).
const CHIP_EVERY: Duration = Duration::from_secs(60);
/// How often the open panel refreshes.
const PANEL_EVERY: Duration = Duration::from_secs(5);

// ── Model (one per shell process) ────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum Busy {
    Up,
    Down,
    ExitNode(String),
}

/// A failed action: the CLI's message, and whether the operator hint fits.
#[derive(Clone)]
struct Problem {
    message: SharedString,
    operator: bool,
}

#[derive(Default)]
struct Model {
    status: Option<Rc<Result<Status, Error>>>,
    fetched: Option<Instant>,
    fetching: bool,
    busy: Option<Busy>,
    problem: Option<Problem>,
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

/// Reads `tailscale status` off the UI thread (once at a time).
fn refresh(cx: &mut App) {
    let m = model();
    if std::mem::replace(&mut m.borrow_mut().fetching, true) {
        return;
    }
    cx.spawn(async move |cx| {
        let status = mandara_services::runtime::unblock(ts::status).await;
        let views = {
            let mut mb = m.borrow_mut();
            mb.status = Some(Rc::new(status));
            mb.fetched = Some(Instant::now());
            mb.fetching = false;
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

/// Marks the rendering view as on screen and refreshes data older than
/// `every`, scheduling the next redraw for when it goes stale.
fn keep_fresh(every: Duration, window: &mut Window, cx: &mut App) {
    let stale = {
        let m = model();
        let mb = m.borrow();
        mb.views.track(window);
        mb.fetched.is_none_or(|at| at.elapsed() >= every)
    };
    if stale {
        refresh(cx);
    }
    redraw_in(every + EXPIRY_MARGIN, window, cx);
}

/// Runs an action off the UI thread, then refreshes; the reply carries the
/// action's JSON (for commands) and failures land on the panel.
fn act(busy: Busy, cx: &mut App) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let m = model();
    if m.borrow().busy.is_some() {
        let _ = tx.try_send(Err(t("Tailscale is busy with another change").into()));
        return rx;
    }
    {
        let mut mb = m.borrow_mut();
        mb.busy = Some(busy.clone());
        mb.problem = None;
    }
    notify(cx);
    cx.spawn(async move |cx| {
        let work = busy.clone();
        let result: Result<Value, Error> = mandara_services::runtime::unblock(move || match work {
            Busy::Up => ts::up().map(|url| match url {
                Some(url) => {
                    let _ = mandara_services::session::open_uri(&url);
                    json!({ "connected": false, "login_url": url })
                }
                None => json!({ "connected": true }),
            }),
            Busy::Down => ts::down().map(|_| json!({ "connected": false })),
            Busy::ExitNode(target) => {
                let ip = if target.is_empty() || target == "none" {
                    None
                } else {
                    let status = ts::status()?;
                    let node = ts::resolve_exit_node(&status, &target).map_err(Error::Failed)?;
                    Some(node.ip().unwrap_or_default().to_string())
                };
                ts::set_exit_node(ip.as_deref()).map(|_| json!({ "exit_node": ip }))
            }
        })
        .await;
        {
            let mut mb = m.borrow_mut();
            mb.busy = None;
            match &result {
                Ok(v) if v.get("login_url").is_some() => mb.flash.show(true, t("Finish logging in in your browser")),
                Ok(_) => {}
                Err(e) => mb.problem = Some(Problem { message: e.message().into(), operator: matches!(e, Error::NotOperator(_)) }),
            }
            mb.fetched = None;
        }
        let _ = tx.try_send(result.map(|v| v.to_string()).map_err(|e| e.message()));
        cx.update(|cx| {
            refresh(cx);
            notify(cx);
        });
    })
    .detach();
    rx
}

fn copy(text: &str, cx: &mut App) {
    let ok = mandara_services::clipboard::serve(text.as_bytes().to_vec(), None);
    let m = model();
    match ok {
        Ok(()) => m.borrow().flash.show(true, tf("Copied {text}", &[("text", &text)])),
        Err(e) => m.borrow().flash.show(false, e),
    }
    notify(cx);
}

// ── Pieces ───────────────────────────────────────────────────────────────

/// A bundled glyph for an OS name as Tailscale reports it.
fn os_glyph(os: &str) -> &'static str {
    match os.to_ascii_lowercase().as_str() {
        "windows" => "window",
        "macos" | "darwin" => "monitor",
        "ios" | "android" | "ipados" => "pointer",
        "linux" | "freebsd" | "openbsd" => "terminal",
        "" => "globe",
        _ => "server",
    }
}

fn ago(at: i64) -> String {
    tf("seen {span} ago", &[("span", &duration_short(chrono::Utc::now().timestamp() - at))])
}

/// A copyable address: tabular, monospace, with a copy glyph on hover.
fn address(id: impl Into<gpui::ElementId>, ip: &str, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let ip_owned = ip.to_string();
    let hover = wash(WASH_SOFT, palette);
    div()
        .id(id)
        .flex_none()
        .px(px(tokens::XS))
        .rounded(px(crate::style::radius(tokens::SM)))
        .row()
        .items_center()
        .gap(px(tokens::XS))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .font_family(fonts(cx).mono)
        .font_features(tabular_nums())
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(ip.to_string())
        .child(icon("copy", COPY_ICON_PX, palette.on_surface_variant))
        .on_click(move |_, _, cx| copy(&ip_owned, cx))
        .into_any_element()
}

fn hero(s: &Status, busy: Option<&Busy>, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let connected = s.connected();
    let accent = readable(if connected { palette.primary } else { palette.on_surface_variant }, palette);
    let me = s.this.as_ref();
    let name: SharedString = me.map(|n| n.name.clone()).unwrap_or_else(|| t("This device").into()).into();
    let state = match (busy, &s.backend) {
        (Some(Busy::Up), _) => t("Connecting…"),
        (Some(Busy::Down), _) => t("Disconnecting…"),
        (_, b) => b.label(),
    };
    let (button_label, button_icon, go) = if connected { (t("Disconnect"), "power", Busy::Down) } else if s.backend == Backend::NeedsLogin { (t("Log in"), "user", Busy::Up) } else { (t("Connect"), "power", Busy::Up) };
    let button = if busy.is_some() {
        div().h(px(super::panel_fx::PILL_H_PX)).px(px(EXIT_CHIP_PAD_X_PX)).flex().items_center().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Working…")).into_any_element()
    } else {
        pill_button("ts-toggle", Some(button_icon), button_label, readable(if connected { palette.urgent_coral } else { palette.primary }, palette), palette, move |_, cx| drop(act(go.clone(), cx))).into_any_element()
    };
    glass_card(palette)
        .h(px(HERO_H))
        .p(px(HERO_PAD_PX))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(alpha(HERO_FILL, palette)), partner(accent).with_alpha(alpha(HERO_FILL, palette) * PARTNER_FADE)))
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(icon_chip(os_glyph(me.map(|n| n.os.as_str()).unwrap_or_default()), accent, HERO_CHIP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(HERO_LINE_GAP_PX))
                .child(div().text_size(px(HERO_NAME_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(name))
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(tokens::SM))
                        .child(div().size(px(CHIP_DOT_PX)).rounded_full().bg(accent.hsla()).when(connected, |el| el.shadow(vec![glow(accent.with_alpha(DOT_GLOW.0), DOT_GLOW.1, 0.0, 0.0)])))
                        .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(accent.hsla()).child(state))
                        .when_some(me.and_then(Node::ip), |el, ip| el.child(address("ts-self-ip", ip, palette, cx))),
                ),
        )
        .child(button)
        .into_any_element()
}

fn problem_note(p: &Problem, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let coral = readable(palette.urgent_coral, palette);
    let mono = fonts(cx).mono;
    div()
        .p(px(NOTE_PAD_PX))
        .rounded(px(crate::style::radius(NOTE_RADIUS_PX)))
        .bg(coral.with_alpha(NOTE_FILL_ALPHA).hsla())
        .flex()
        .flex_col()
        .gap(px(tokens::SM))
        .child(div().row().items_start().gap(px(tokens::SM)).child(icon("alert", ALERT_ICON_PX, coral)).child(div().flex_1().min_w(px(0.0)).text_size(px(text::BODY_S)).line_clamp(2).child(p.message.clone())))
        .when(p.operator, |el| {
            el.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(tokens::SM))
                    .child(caption(t("Allow your user once:"), palette))
                    .child(
                        div()
                            .id("ts-operator-hint")
                            .h(px(HINT_H_PX))
                            .px(px(HINT_PAD_X_PX))
                            .rounded_full()
                            .bg(palette.surface_container_high.hsla())
                            .row()
                            .items_center()
                            .gap(px(tokens::XS))
                            .cursor_pointer()
                            .font_family(mono)
                            .text_size(px(text::LABEL))
                            .child(ts::OPERATOR_HINT)
                            .child(icon("copy", COPY_ICON_PX, palette.on_surface_variant))
                            .on_click(|_, _, cx| copy(ts::OPERATOR_HINT, cx)),
                    ),
            )
        })
        .into_any_element()
}

fn exit_chip(id: usize, label: SharedString, online: bool, active: bool, target: String, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    div()
        .id(("ts-exit", id))
        .h(px(EXIT_CHIP_H))
        .px(px(EXIT_CHIP_PAD_X_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(EXIT_INNER_GAP_PX))
        .cursor_pointer()
        .text_size(px(EXIT_TEXT_PX))
        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .when(active, |el| el.bg(accent.with_alpha(alpha(ROW_FILL, palette)).hsla()).text_color(accent.hsla()))
        .when(!active, |el| el.bg(wash(super::panel_fx::WASH_FAINT, palette).hsla()).text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
        .when(id > 0, |el| el.child(div().size(px(CHIP_DOT_PX)).rounded_full().bg(if online { readable(palette.success_emerald, palette) } else { palette.on_surface_variant.with_alpha(CHIP_IDLE_ALPHA) }.hsla())))
        .child(label)
        .when(!active, |el| el.on_click(move |_, _, cx| drop(act(Busy::ExitNode(target.clone()), cx))))
        .into_any_element()
}

fn exit_picker(s: &Status, busy: Option<&Busy>, palette: &MandaraPalette) -> AnyElement {
    let current = s.exit_node().map(|n| n.id.clone());
    let pending = match busy {
        Some(Busy::ExitNode(t)) => Some(t.clone()),
        _ => None,
    };
    let mut chips = vec![exit_chip(0, t("None").into(), true, (current.is_none() && pending.is_none()) || pending.as_deref() == Some(""), String::new(), palette)];
    for (i, n) in s.exit_options().enumerate() {
        let target = n.ip().unwrap_or_default().to_string();
        let active = pending.as_deref().map_or(current.as_deref() == Some(n.id.as_str()), |p| p == target);
        chips.push(exit_chip(i + 1, n.name.clone().into(), n.online, active, target, palette));
    }
    div().flex().flex_wrap().gap(px(EXIT_GAP_PX)).children(chips).into_any_element()
}

fn peer_row(i: usize, n: &Node, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let emerald = readable(palette.success_emerald, palette);
    let tint = if n.online { readable(palette.primary, palette) } else { palette.on_surface_variant };
    let dot = if n.online { emerald } else { palette.on_surface_variant.with_alpha(CHIP_IDLE_ALPHA) };
    let glyph = div()
        .relative()
        .size(px(ROW_GLYPH_PX))
        .flex_none()
        .rounded_full()
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(alpha(ROW_FILL, palette)), partner(tint).with_alpha(alpha(ROW_FILL, palette) * PARTNER_FADE)))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(os_glyph(&n.os), ROW_ICON_PX, tint))
        .child(
            div()
                .absolute()
                .right(px(DOT_OFFSET_PX))
                .bottom(px(DOT_OFFSET_PX))
                .size(px(DOT_PX))
                .rounded_full()
                .border_2()
                .border_color(palette.surface_container.hsla())
                .bg(dot.hsla())
                .when(n.online, |el| el.shadow(vec![glow(emerald.with_alpha(DOT_GLOW.0), DOT_GLOW.1, 0.0, 0.0)])),
        );
    let badge: Option<(SharedString, Color)> = if n.online {
        Some(if n.direct() { (t("direct").into(), emerald) } else { (tf("relay · {region}", &[("region", &n.relay)]).into(), readable(palette.warning_amber, palette)) })
    } else {
        n.last_seen.map(|at| (ago(at).into(), palette.on_surface_variant))
    };
    div()
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .bg(wash(super::panel_fx::WASH_FAINT, palette).hsla())
        .row()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .when(!n.online, |el| el.opacity(OFFLINE_OPACITY))
        .child(glyph)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SECTION_GAP_PX))
                        .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(n.name.clone()))
                        .when(n.exit_node, |el| el.child(icon("external", SMALL_ICON_PX, readable(palette.primary, palette)))),
                )
                .when_some(n.ip(), |el, ip| el.child(div().row().child(address(("ts-peer-ip", i), ip, palette, cx)))),
        )
        .when_some(badge, |el, (label, tint)| {
            el.child(
                div()
                    .h(px(BADGE_H_PX))
                    .px(px(BADGE_PAD_X_PX))
                    .flex_none()
                    .rounded_full()
                    .bg(tint.with_alpha(BADGE_FILL_ALPHA).hsla())
                    .flex()
                    .items_center()
                    .text_size(px(BADGE_TEXT_PX))
                    .font_weight(FontWeight::MEDIUM)
                    .font_features(tabular_nums())
                    .text_color(tint.hsla())
                    .child(label),
            )
        })
        .into_any_element()
}

// ── Views ────────────────────────────────────────────────────────────────

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    keep_fresh(CHIP_EVERY, window, cx);
    let m = model();
    let mb = m.borrow();
    let (connected, exit) = match mb.status.as_deref() {
        Some(Ok(s)) => (s.connected(), s.connected() && s.exit_node().is_some()),
        _ => (false, false),
    };
    let tint = if connected { readable(palette.primary, palette) } else { palette.on_surface_variant.with_alpha(CHIP_IDLE_ALPHA) };
    div()
        .relative()
        .flex()
        .items_center()
        .child(icon("globe", crate::style::bar_icon(CHIP_ICON_PX), tint))
        .when(exit, |el| {
            let dot = readable(palette.tertiary, palette);
            el.child(div().absolute().right(px(CHIP_DOT_OFFSET_PX)).bottom(px(CHIP_DOT_OFFSET_PX)).size(px(CHIP_DOT_PX)).rounded_full().bg(dot.hsla()))
        })
        .into_any_element()
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
        if self.p.progress() > 0.0 {
            keep_fresh(PANEL_EVERY, window, cx);
        } else {
            model().borrow().views.track(window);
        }
        let m = model();
        let (status, busy, problem, flash) = {
            let mb = m.borrow();
            (mb.status.clone(), mb.busy.clone(), mb.problem.clone(), mb.flash.clone())
        };
        let flash = flash.current(window, cx);
        let p = &self.p;
        let accent = readable(palette.primary, palette);
        let mut h = PAD * 2.0 + HEADER_H;
        let (subtitle, body): (String, Vec<AnyElement>) = match status.as_deref() {
            None => {
                h += GAP + EMPTY_H;
                (t("Checking…").into(), vec![empty_state("globe", t("Checking Tailscale…"), t("Reading tailscale status"), EMPTY_H, palette).into_any_element()])
            }
            Some(Err(e)) => {
                h += GAP + EMPTY_H;
                let (glyph, title, hint): (&'static str, String, String) = match e {
                    Error::NotInstalled => ("download", t("Tailscale isn't installed").into(), t("Install the tailscale package, then start tailscaled").into()),
                    Error::Daemon(_) => ("alert", t("Tailscale isn't running").into(), t("Start it with: sudo systemctl enable --now tailscaled").into()),
                    Error::NotOperator(msg) | Error::Failed(msg) => ("alert", t("Couldn't read Tailscale").into(), msg.clone()),
                };
                let mut blocks = vec![empty_state(glyph, title.clone(), hint, EMPTY_H, palette).into_any_element()];
                if matches!(e, Error::NotInstalled) {
                    h += GAP + super::panel_fx::PILL_H_PX;
                    blocks.push(pill_button("ts-download", Some("download"), t("Download"), accent, palette, |_, cx| {
                        cx.spawn(async move |cx| {
                            let result = mandara_services::runtime::unblock(|| mandara_services::session::open_uri(DOWNLOAD_URL)).await;
                            if let Err(error) = result {
                                model().borrow_mut().flash.show(false, error.to_string());
                                cx.update(notify);
                            }
                        }).detach();
                    }).into_any_element());
                }
                (title, blocks)
            }
            Some(Ok(s)) => {
                let mut blocks = vec![hero(s, busy.as_ref(), palette, cx)];
                h += GAP + HERO_H;
                if let Some(pr) = &problem {
                    h += GAP + if pr.operator { NOTE_HINT_H } else { NOTE_H };
                    blocks.push(problem_note(pr, palette, cx));
                }
                let subtitle = match &s.tailnet {
                    Some(net) => net.clone(),
                    None => s.backend.label().to_string(),
                };
                if s.backend == Backend::NeedsLogin {
                    h += GAP + EMPTY_H;
                    blocks.push(empty_state("user", t("You're logged out"), t("Log in to join your tailnet; the page opens in your browser"), EMPTY_H, palette).into_any_element());
                } else {
                    let options = s.exit_options().count();
                    if options > 0 {
                        let rows = (options + 1).div_ceil(EXIT_PER_ROW) as f32;
                        h += GAP + SECTION_H_PX + SECTION_GAP_PX + rows * EXIT_CHIP_H + (rows - 1.0) * EXIT_GAP_PX;
                        blocks.push(div().flex().flex_col().gap(px(SECTION_GAP_PX)).child(section(t("Exit node"), palette, None)).child(exit_picker(s, busy.as_ref(), palette)).into_any_element());
                    }
                    let n = s.peers.len();
                    if n == 0 {
                        h += GAP + EMPTY_H;
                        blocks.push(empty_state("globe", t("No other devices yet"), t("Devices you add to your tailnet appear here"), EMPTY_H, palette).into_any_element());
                    } else {
                        let list_h = (n as f32).min(VISIBLE_ROWS) * (ROW_H + ROW_GAP_PX) - ROW_GAP_PX;
                        h += GAP + SECTION_H_PX + SECTION_GAP_PX + list_h;
                        let label = tf("Devices · {online} of {total} online", &[("online", &s.online_count()), ("total", &n)]);
                        let rows: Vec<AnyElement> = s.peers.iter().enumerate().map(|(i, peer)| peer_row(i, peer, palette, cx)).collect();
                        blocks.push(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(SECTION_GAP_PX))
                                .child(section_owned(label, palette))
                                .child(div().h(px(list_h)).child(div().id("ts-peers").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP_PX)).children(rows)))
                                .into_any_element(),
                        );
                    }
                }
                (subtitle, blocks)
            }
        };
        self.set_height(h);
        let mut col = div().flex().flex_col().gap(px(GAP)).child(reveal(p, 0, header("globe", accent, t("Tailscale"), subtitle, palette, None)));
        for (i, b) in body.into_iter().enumerate() {
            col = col.child(reveal(p, i + 1, b));
        }
        div().relative().size_full().p(px(PAD)).child(col).when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette))).into_any_element()
    }
}

/// [`section`] for a formatted label.
fn section_owned(label: String, palette: &MandaraPalette) -> gpui::Div {
    div()
        .h(px(SECTION_H_PX))
        .row()
        .items_center()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.on_surface_variant.hsla())
        .font_features(tabular_nums())
        .child(label)
}

// ── Commands ─────────────────────────────────────────────────────────────

fn node_json(n: &Node) -> Value {
    json!({
        "name": n.name, "dns_name": n.dns_name, "ips": n.ips, "os": n.os, "online": n.online,
        "exit_node": n.exit_node, "exit_node_option": n.exit_node_option, "last_seen": n.last_seen,
        "path": if n.direct() { "direct" } else { "relay" }, "relay": n.relay, "cur_addr": n.cur_addr,
        "rx_bytes": n.rx_bytes, "tx_bytes": n.tx_bytes,
    })
}

fn status_json(s: &Status) -> Value {
    json!({
        "backend": s.backend, "connected": s.connected(), "version": s.version, "tailnet": s.tailnet,
        "self": s.this.as_ref().map(node_json), "exit_node": s.exit_node().map(|n| n.name.clone()),
        "online": s.online_count(), "peers": s.peers.iter().map(node_json).collect::<Vec<_>>(),
        "login_url": s.auth_url, "health": s.health,
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Tailscale"), tk("Your tailnet: this device, exit node and peers with their paths"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(None, bar_view, window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("tailscale status", tk("This device, the exit node in use and every peer (JSON)"), |_, _| {
            Ok(crate::powerctl::pending(|| ts::status().map(|s| status_json(&s)).map_err(|e| e.message())))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("tailscale up", tk("Connect to the tailnet (answers with a login link when the device must log in)"), |_, cx| Ok(Output::Pending(act(Busy::Up, cx)))),
    );
    r.command(ID, CommandSpec::new_output("tailscale down", tk("Disconnect from the tailnet"), |_, cx| Ok(Output::Pending(act(Busy::Down, cx)))));
    r.command(
        ID,
        CommandSpec::new_output("tailscale exit-node", tk("Route traffic through a peer, or stop with none"), |a, cx| {
            let target = a.rest(0);
            let target = if target.eq_ignore_ascii_case("none") { String::new() } else { target };
            Ok(Output::Pending(act(Busy::ExitNode(target), cx)))
        })
        .arg(ArgSpec::required("peer", ArgKind::String, tk("peer name, MagicDNS name or IP, or none")))
        .example("tailscale exit-node nas")
        .example("tailscale exit-node none"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_names_map_to_bundled_glyphs() {
        assert_eq!(os_glyph("linux"), "terminal");
        assert_eq!(os_glyph("macOS"), "monitor");
        assert_eq!(os_glyph("android"), "pointer");
        assert_eq!(os_glyph("windows"), "window");
        assert_eq!(os_glyph("plan9"), "server");
        for g in ["terminal", "monitor", "pointer", "window", "server", "globe"] {
            assert!(crate::icons::svg_source(g).is_some(), "{g} is bundled");
        }
    }
}
