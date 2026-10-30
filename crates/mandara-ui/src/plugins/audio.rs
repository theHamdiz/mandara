//! Audio plugin: volume icon + level in the bar (scroll to adjust, right
//! click mutes the output, middle click mutes all sound); a sound panel with
//! a global "Mute all" switch, thick output / microphone sliders, mute pills,
//! device names and a live output spectrum while it is open, and an Apps
//! card with each app's own volume, mute and output ([`apps`]); a compact
//! sidebar card.
//!
//! Global mute (`audio mute-all`) mutes every sink — hardware and virtual,
//! e.g. EasyEffects' sink, whose own mute never reaches the speakers — and
//! every app stream, then restores exactly what was muted before
//! (see `mandara_services::audio::mute`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui::InteractiveElement as _;
use gpui::{AnyElement, App, FontWeight, MouseButton, MouseDownEvent, SharedString, IntoElement, ParentElement, ScrollDelta, ScrollEvent, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_i18n::{t, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::audio::{AudioCommand, AudioState, Channel, MAX_VOLUME};
use mandara_theme::MandaraPalette;

mod apps;

use apps::{AppsTap, Presence, RowCx};

use super::panel_fx::{HEADER_H, PanelShape, SpectrumTap, TINT_FILL, alpha, big_slider, glass_card, header, icon_chip, partner, pill_button, readable, redraw_in, spectrum_bars};
use super::{fn_view, json};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{card, reveal, text, tokens};

pub const ID: &str = "audio";

const PANEL_W: f32 = 380.0;
const PAD: f32 = 14.0;
const GAP: f32 = 10.0;
/// Gap under the panel header.
const HEADER_GAP: f32 = 12.0;
const CARD_PAD: f32 = 12.0;
const LABEL_H: f32 = 34.0;
const SLIDER_H: f32 = 40.0;
const VIZ_H: f32 = 24.0;
/// Gap between the rows inside a channel card.
const ROW_GAP: f32 = 10.0;
/// Gap between spectrum bars.
const VIZ_BAR_GAP_PX: f32 = 2.0;
const OUT_CARD_H: f32 = CARD_PAD * 2.0 + LABEL_H + ROW_GAP + SLIDER_H + ROW_GAP + VIZ_H;
const IN_CARD_H: f32 = CARD_PAD * 2.0 + LABEL_H + ROW_GAP + SLIDER_H;
const PANEL_H: f32 = PAD * 2.0 + HEADER_H + HEADER_GAP + OUT_CARD_H + GAP + IN_CARD_H;

// "All sound muted" banner (shown under the header while global mute is on).
const BANNER_H: f32 = 58.0;
const BANNER_PAD_X: f32 = 12.0;
const BANNER_CHIP_PX: f32 = 34.0;
const BANNER_GAP_PX: f32 = 12.0;
/// Banner fill: a coral wash a step stronger than a pill's.
const BANNER_FILL_BOOST: f32 = 0.6;
/// Banner coral edge alpha (dark / light).
const BANNER_EDGE: (f32, f32) = (0.32, 0.28);
/// Panel height while the banner shows.
const PANEL_H_MUTED: f32 = PANEL_H + BANNER_H + GAP;

// Apps card (per-app volume): its label, the gap under it, the gap between
// rows, the empty line, and the tallest the panel grows before the list
// scrolls.
const APPS_LABEL_H: f32 = 14.0;
const APPS_LABEL_GAP: f32 = 10.0;
const APP_ROW_GAP: f32 = 12.0;
const APPS_EMPTY_H: f32 = 20.0;
const APPS_CARD_CHROME_H: f32 = CARD_PAD * 2.0 + APPS_LABEL_H + APPS_LABEL_GAP;
const MAX_PANEL_H: f32 = 720.0;

// Channel card head.
const KIND_TEXT_PX: f32 = 10.5;
/// Device name: half a step above the small body size.
const DEVICE_TEXT_PX: f32 = text::BODY_S + 0.5;
const VOLUME_TEXT_PX: f32 = 22.0;
const HEAD_GAP_PX: f32 = 10.0;

// Bar item and sidebar card.
const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 6.0;
const SIDEBAR_ROW_GAP_PX: f32 = 6.0;
/// Apps listed in the sidebar card (playing ones; the panel lists all).
const SIDEBAR_MAX_APPS: usize = 3;
/// A sidebar app row: its name line, the gap, then the slider.
const SIDEBAR_LINE_H: f32 = 18.0;
const SIDEBAR_APP_ICON_PX: f32 = 16.0;
const SIDEBAR_APP_ROW_H: f32 = SIDEBAR_LINE_H + SIDEBAR_ROW_GAP_PX + apps::ROW_H - apps::ROW_HEAD_H - apps::ROW_SLIDER_GAP;
/// Sidebar card opacity while the audio server is unavailable.
const UNAVAILABLE_OPACITY: f32 = 0.6;

/// Spectrum redraw cadence while bands are live (the FFT runs at ≈ 60 Hz,
/// so drawing faster would repeat frames).
const SPECTRUM_FRAME: Duration = Duration::from_millis(16);
/// While the panel is open but silent, check this often for sound.
const SILENCE_POLL: Duration = Duration::from_millis(400);

/// Element ids (and drag-state keys) of the device sliders: the panel's
/// output and microphone, and the sidebar card's.
const OUT_SLIDER: &str = "audio-out";
const IN_SLIDER: &str = "audio-in";
const SIDE_OUT_SLIDER: &str = "side-out";
const SIDE_IN_SLIDER: &str = "side-in";

/// Volumes at or below this read as silent (mute glyph).
const SILENT_VOLUME: f32 = 0.001;
/// Volumes below this use the low-volume glyph.
const LOW_VOLUME: f32 = 0.5;
/// Volumes are exposed in percent.
const PERCENT: f32 = 100.0;
/// Scroll on the bar item: pixels per notch, dead zone, step per notch.
const SCROLL_PX_PER_LINE: f32 = 20.0;
const SCROLL_DEAD_ZONE: f32 = 0.01;

/// The bar / header glyph: global mute has its own struck-through speaker.
fn audio_icon(s: &AudioState) -> &'static str {
    if s.all_muted { "volume-off" } else { volume_icon(s.output.volume, s.output.muted) }
}

fn volume_icon(volume: f32, muted: bool) -> &'static str {
    if muted || volume <= SILENT_VOLUME {
        "volume-mute"
    } else if volume < LOW_VOLUME {
        "volume-low"
    } else {
        "volume-high"
    }
}

/// `0%..=150%` labels, formatted once per thread instead of every frame
/// (volume may exceed 100 %, so [`super::panel_fx::percent_text`] doesn't fit).
fn volume_text(volume: f32) -> SharedString {
    // `as` on the rounded, clamped value cannot truncate.
    let max = (MAX_VOLUME * PERCENT).round() as usize;
    thread_local! {
        static LABELS: Vec<SharedString> = (0..=(MAX_VOLUME * PERCENT).round() as usize).map(|p| SharedString::from(format!("{p}%"))).collect();
    }
    let i = (volume.clamp(0.0, MAX_VOLUME) * PERCENT).round() as usize;
    LABELS.with(|l| l[i.min(max)].clone())
}

/// A channel's device name (your label from `[audio] device_labels` when
/// set), or `fallback` when the server reports none.
fn device_name(ch: &Channel, fallback: &'static str, cx: &App) -> SharedString {
    let cfg = crate::state::config(cx).read(cx).get();
    let name = cfg.audio.device_label(&ch.device, &ch.name);
    if name.is_empty() { fallback.into() } else { name.to_string().into() }
}

/// The volume ceiling (`[audio] overamplify`): 100 % or 150 %.
fn limit(cx: &App) -> f32 {
    crate::state::config(cx).read(cx).get().audio.volume_limit()
}

/// The configured volume step, as a fraction.
fn step(cx: &App) -> f32 {
    crate::state::config(cx).read(cx).get().audio.volume_step.max(1) as f32 / PERCENT
}

/// The device after the default one, skipping hidden ones (wrapping).
fn next_device<'a>(list: &'a [mandara_services::audio::Device], cfg: &mandara_config::alerts::AudioConfig) -> Option<&'a mandara_services::audio::Device> {
    let visible: Vec<&mandara_services::audio::Device> = list.iter().filter(|d| !cfg.is_hidden(&d.name)).collect();
    let current = visible.iter().position(|d| d.is_default);
    let next = match current {
        Some(i) => visible.get((i + 1) % visible.len().max(1))?,
        None => visible.first()?,
    };
    (!next.is_default).then_some(*next)
}

/// A non-negative whole stream index from a command argument.
fn stream_index(n: f64) -> Result<u32, String> {
    if n >= 0.0 && n.fract() == 0.0 && n <= f64::from(u32::MAX) { Ok(n as u32) } else { Err(format!("invalid stream index {n}")) }
}

fn request(cx: &mut App, command: AudioCommand) -> Result<Output, String> {
    let service = hub(cx).read(cx).audio_service.clone().ok_or("audio service unavailable")?;
    Ok(Output::Pending(service.request(command)))
}

fn send(cx: &mut App, command: AudioCommand) {
    if let Some(s) = hub(cx).read(cx).audio_service.clone() {
        s.send(command);
    }
}

/// A slider's write: the command as a request, so the slider sends the
/// next value only once the server has applied this one
/// ([`crate::slider_model`]).
fn write(cx: &mut App, command: AudioCommand) -> crate::slider_model::Write {
    hub(cx).read(cx).audio_service.clone().map(|s| s.request(command))
}

/// A device channel's volume as drawn: the value you are dragging on the
/// slider `key` (a fraction of `lim`), else the server's.
fn shown_volume(key: &'static str, ch: &Channel, lim: f32) -> f32 {
    crate::slider_model::held(&key.into()).map_or(ch.volume, |frac| frac * lim)
}

/// Label row of a channel card: kind, device name, big percentage, mute.
#[allow(clippy::too_many_arguments)]
fn channel_head(
    id: &'static str,
    kind: &'static str,
    ch: &Channel,
    name: SharedString,
    (on_icon, off_icon): (&'static str, &'static str),
    mute: AudioCommand,
    palette: &MandaraPalette,
    (display, volume): (gpui::SharedString, f32),
) -> impl IntoElement {
    let accent = readable(palette.primary, palette);
    div()
        .h(px(LABEL_H))
        .flex()
        .items_center()
        .gap(px(HEAD_GAP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(KIND_TEXT_PX)).font_weight(FontWeight::BOLD).text_color(accent.hsla()).child(kind))
                .child(div().text_size(px(DEVICE_TEXT_PX)).font_weight(FontWeight::MEDIUM).truncate().child(name)),
        )
        .child(
            div()
                .font_family(display)
                .font_features(tabular_nums())
                .text_size(px(VOLUME_TEXT_PX))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if ch.muted { palette.on_surface_variant.hsla() } else { palette.on_surface.hsla() })
                .child(volume_text(volume)),
        )
        .child(pill_button(
            id,
            Some(if ch.muted { off_icon } else { on_icon }),
            if ch.muted { t("Muted") } else { t("Mute") },
            if ch.muted { palette.urgent_coral } else { palette.on_surface_variant },
            palette,
            move |_, cx| send(cx, mute.clone()),
        ))
}

fn output_card(s: &AudioState, bands: Option<&[f32]>, palette: &MandaraPalette, radius: f32, display: gpui::SharedString, (name, lim): (SharedString, f32)) -> impl IntoElement {
    let o = &s.output;
    let volume = shown_volume(OUT_SLIDER, o, lim);
    let from = readable(palette.primary, palette);
    glass_card(palette)
        .rounded(px(radius))
        .p(px(CARD_PAD))
        .flex()
        .flex_col()
        .child(channel_head("audio-out-mute", t("OUTPUT"), o, name, ("volume-high", "volume-mute"), AudioCommand::ToggleMute, palette, (display, volume)))
        .child(div().h(px(ROW_GAP)))
        .child(big_slider(OUT_SLIDER, o.volume / lim, volume_icon(volume, o.muted), o.muted, palette, move |v, _, cx| write(cx, AudioCommand::SetVolume(v * lim))))
        .child(div().h(px(ROW_GAP)))
        .child(spectrum_bars(bands.unwrap_or(&[]), from, readable(partner(palette.primary), palette), palette, VIZ_H, VIZ_BAR_GAP_PX))
}

fn input_card(s: &AudioState, palette: &MandaraPalette, radius: f32, display: gpui::SharedString, (name, lim): (SharedString, f32)) -> impl IntoElement {
    let i = &s.input;
    let volume = shown_volume(IN_SLIDER, i, lim);
    glass_card(palette)
        .rounded(px(radius))
        .p(px(CARD_PAD))
        .flex()
        .flex_col()
        .child(channel_head("audio-in-mute", t("MICROPHONE"), i, name, ("mic", "mic-off"), AudioCommand::ToggleMicMute, palette, (display, volume)))
        .child(div().h(px(ROW_GAP)))
        .child(big_slider(IN_SLIDER, i.volume / lim, if i.muted { "mic-off" } else { "mic" }, i.muted, palette, move |v, _, cx| write(cx, AudioCommand::SetMicVolume(v * lim))))
}

/// The header's global mute switch: "Mute all" / coral "Unmute all".
fn mute_all_button(on: bool, palette: &MandaraPalette) -> AnyElement {
    pill_button(
        "audio-mute-all",
        Some(if on { "volume-high" } else { "volume-off" }),
        if on { t("Unmute all") } else { t("Mute all") },
        if on { palette.urgent_coral } else { palette.on_surface_variant },
        palette,
        |_, cx| send(cx, AudioCommand::ToggleMuteAll),
    )
    .into_any_element()
}

/// The coral "All sound muted" banner.
fn muted_banner(palette: &MandaraPalette, radius: f32) -> impl IntoElement {
    let coral = readable(palette.urgent_coral, palette);
    let fill = palette.urgent_coral.with_alpha(alpha(TINT_FILL, palette) * BANNER_FILL_BOOST);
    div()
        .h(px(BANNER_H))
        .px(px(BANNER_PAD_X))
        .rounded(px(radius))
        .bg(fill.hsla())
        .border_1()
        .border_color(palette.urgent_coral.with_alpha(alpha(BANNER_EDGE, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(BANNER_GAP_PX))
        .child(icon_chip("volume-off", coral, BANNER_CHIP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(coral.hsla()).truncate().child(t("All sound muted")))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(t("Every output and app is silent; your own mutes are kept"))),
        )
}

/// Outputs an app can be sent to: every output you haven't hidden.
fn visible_outputs<'a>(list: &'a [mandara_services::audio::Device], cx: &App) -> Vec<&'a mandara_services::audio::Device> {
    let cfg = crate::state::config(cx).read(cx).get();
    list.iter().filter(|d| !cfg.audio.is_hidden(&d.name)).collect()
}

/// Height of the app rows at their current presence (rows fold in / out).
fn apps_content_h(rows: &[(apps::AppGroup, f32)], row_h: f32, gap: f32) -> f32 {
    rows.iter().enumerate().map(|(i, (_, a))| a * (row_h + if i == 0 { 0.0 } else { gap })).sum()
}

/// The Apps card: one row per app playing sound, or a muted empty line;
/// scrolls once it would push the panel past [`MAX_PANEL_H`]. Returns the
/// card and its height.
fn apps_card(rows: &[(apps::AppGroup, f32)], rc: &RowCx<'_>, radius: f32, max_list_h: f32, cx: &App) -> (AnyElement, f32) {
    let palette = rc.palette;
    let content_h = if rows.is_empty() { APPS_EMPTY_H } else { apps_content_h(rows, apps::ROW_H, APP_ROW_GAP) };
    let list_h = content_h.min(max_list_h.max(0.0));
    let list = if rows.is_empty() {
        div().h(px(APPS_EMPTY_H)).flex().items_center().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(t("No apps are playing sound")).into_any_element()
    } else {
        let items = rows.iter().enumerate().map(|(i, (g, a))| {
            let gap = if i == 0 { 0.0 } else { APP_ROW_GAP };
            div().flex_none().h(px(a * (apps::ROW_H + gap))).overflow_hidden().opacity(*a).child(div().pt(px(gap)).child(apps::panel_row(g, rc, cx)))
        });
        let list = div().id("audio-apps-list").h(px(list_h)).flex().flex_col().children(items);
        if content_h > list_h { gpui::StatefulInteractiveElement::overflow_y_scroll(list).into_any_element() } else { list.overflow_hidden().into_any_element() }
    };
    let accent = readable(palette.primary, palette);
    let card = glass_card(palette)
        .rounded(px(radius))
        .p(px(CARD_PAD))
        .flex()
        .flex_col()
        .child(div().h(px(APPS_LABEL_H)).row().items_center().text_size(px(KIND_TEXT_PX)).font_weight(FontWeight::BOLD).text_color(accent.hsla()).child(t("APPS")))
        .child(div().h(px(APPS_LABEL_GAP)))
        .child(list)
        .into_any_element();
    (card, APPS_CARD_CHROME_H + list_h)
}

fn panel_view(p: PanelCx, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>, apps_tap: Rc<RefCell<AppsTap>>) -> impl Fn(&MandaraPalette, &mut gpui::Window, &mut App) -> AnyElement {
    let tap = Rc::new(RefCell::new(SpectrumTap::default()));
    let presence = RefCell::new(Presence::default());
    move |palette, window, cx| {
        let visible = p.progress() > 0.0;
        let muted = hub(cx).read(cx).audio.output.muted;
        // The capture tap lives only while the panel is on screen.
        let bands = SpectrumTap::bands(&tap, visible && !muted, cx);
        if visible {
            redraw_in(if bands.is_some() { SPECTRUM_FRAME } else { SILENCE_POLL }, window, cx);
        }
        let r = PanelShape::of(cx).inner(PAD);
        let accent = readable(palette.primary, palette);
        let display = fonts(cx).display;
        // Borrow this frame's hub state instead of cloning it.
        let hub = hub(cx);
        let lim = limit(cx);
        let (out_name, in_name) = {
            let s = &hub.read(cx).audio;
            (device_name(&s.output, t("Speakers"), cx), device_name(&s.input, t("Microphone"), cx))
        };
        // Apps follow the server only while the panel is on screen.
        let snapshot = if visible { AppsTap::snapshot(&apps_tap, window, cx) } else { Rc::default() };
        let rows = presence.borrow_mut().frame(&snapshot.groups, window);
        let outputs = visible_outputs(&snapshot.outputs, cx);
        let s = &hub.read(cx).audio;
        let all = s.all_muted;
        let base_h = if all { PANEL_H_MUTED } else { PANEL_H };
        let default_sink = snapshot.outputs.iter().find(|d| d.is_default).map(|d| d.index);
        let rc = RowCx { tap: &apps_tap, limit: lim, all_muted: all, outputs: &outputs, default_sink, palette, id_prefix: "audio-app" };
        let (apps, apps_h) = apps_card(&rows, &rc, r, MAX_PANEL_H - base_h - GAP - APPS_CARD_CHROME_H, cx);
        size_cell.set(size(px(PANEL_W), px(base_h + GAP + apps_h)));
        let subtitle = if !s.available { t("Audio server unavailable").into() } else { out_name.clone() };
        let chip_tint = if all { readable(palette.urgent_coral, palette) } else { accent };
        let trailing = s.available.then(|| mute_all_button(all, palette));
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&p, 0, header(audio_icon(s), chip_tint, t("Sound"), subtitle, palette, trailing)))
            .child(div().h(px(HEADER_GAP)))
            .when(all, |el| el.child(reveal(&p, 1, muted_banner(palette, r))).child(div().h(px(GAP))))
            .child(reveal(&p, 2, output_card(s, bands.as_ref().map(|b| &b[..]), palette, r, display.clone(), (out_name, lim))))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 3, input_card(s, palette, r, display, (in_name, lim))))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 4, apps))
            .into_any_element()
    }
}

/// The sidebar's compact app rows: icon, name and percentage over a slider.
fn sidebar_apps(rows: &[(apps::AppGroup, f32)], rc: &RowCx<'_>) -> AnyElement {
    let palette = rc.palette;
    let items = rows.iter().enumerate().map(|(i, (g, a))| {
        let gap = if i == 0 { 0.0 } else { tokens::MD };
        let (volume, muted) = (apps::shown_volume(g, rc), rc.tap.borrow().muted(g));
        let line = div()
            .h(px(SIDEBAR_LINE_H))
            .row()
            .items_center()
            .gap(px(tokens::SM))
            .child(crate::plugins::workspaces::app_icon(&g.icon_key, SIDEBAR_APP_ICON_PX, palette.on_surface, false, palette))
            .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(g.name.clone()))
            .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).when(muted || rc.all_muted, |el| el.opacity(UNAVAILABLE_OPACITY)).child(volume_text(volume)));
        div()
            .flex_none()
            .h(px(a * (SIDEBAR_APP_ROW_H + gap)))
            .overflow_hidden()
            .opacity(*a)
            .child(div().pt(px(gap)).flex().flex_col().gap(px(SIDEBAR_ROW_GAP_PX)).child(line).child(apps::app_slider(g, rc)))
    });
    div().flex().flex_col().children(items).into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Audio"), tk("Volume and microphone"), Some(Zone::Right)), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let s = &hub(cx).read(cx).audio;
                if !s.available {
                    return div().into_any_element();
                }
                div()
                    .id("audio-bar")
                    .flex()
                    // A side bar: the icon over the value.
                    .when(crate::bar::vertical(cx), |el| el.flex_col().text_size(px(text::LABEL)))
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .font_features(tabular_nums())
                    .child(icon(audio_icon(s), crate::style::bar_icon(BAR_ICON_PX), if s.all_muted { readable(palette.urgent_coral, palette) } else { palette.on_surface }))
                    .child(div().text_color(palette.on_surface_variant.hsla()).when(s.all_muted, |el| el.opacity(UNAVAILABLE_OPACITY)).child(volume_text(s.output.volume)))
                    .on_mouse_down(MouseButton::Middle, |_: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        send(cx, AudioCommand::ToggleMuteAll);
                    })
                    .on_scroll(|e: &ScrollEvent, _, cx| {
                        let dy = match e.delta {
                            ScrollDelta::Lines(l) => l.y,
                            ScrollDelta::Pixels(p) => f32::from(p.y) / SCROLL_PX_PER_LINE,
                        };
                        if dy.abs() > SCROLL_DEAD_ZONE {
                            let step = step(cx);
                            send(cx, AudioCommand::AdjustVolume(dy.signum() * step));
                        }
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H + GAP + APPS_CARD_CHROME_H + APPS_EMPTY_H))));
        let apps_tap = AppsTap::shared();
        let panel = fn_view(None, panel_view(panel_cx.clone(), size_cell.clone(), apps_tap.clone()), window, cx);
        let side_presence = RefCell::new(Presence::default());
        let sidebar = fn_view(
            None,
            move |palette, window, cx| {
                let snapshot = AppsTap::snapshot(&apps_tap, window, cx);
                super::panel_fx::redraw_in(apps::KEEPALIVE, window, cx);
                let playing: Vec<apps::AppGroup> = snapshot.groups.iter().filter(|g| !g.corked).take(SIDEBAR_MAX_APPS).cloned().collect();
                let app_rows = side_presence.borrow_mut().frame(&playing, window);
                let lim = limit(cx);
                let names = {
                    let s = &hub(cx).read(cx).audio;
                    (device_name(&s.output, t("Output"), cx), device_name(&s.input, t("Microphone"), cx))
                };
                let s = &hub(cx).read(cx).audio;
                let row = |id: &'static str, ch: &Channel, name: SharedString, set: fn(f32) -> AudioCommand, mic: bool| {
                    let volume = shown_volume(id, ch, lim);
                    let icon_name = match (mic, ch.muted) {
                        (true, true) => "mic-off",
                        (true, false) => "mic",
                        (false, muted) => volume_icon(volume, muted),
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SIDEBAR_ROW_GAP_PX))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(tokens::SM))
                                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).child(name))
                                .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(volume_text(volume))),
                        )
                        .child(big_slider(id, ch.volume / lim, icon_name, ch.muted, palette, move |v, _, cx| write(cx, set(v * lim))))
                };
                let rc = RowCx { tap: &apps_tap, limit: lim, all_muted: s.all_muted, outputs: &[], default_sink: None, palette, id_prefix: "side-app" };
                let app_list = (!app_rows.is_empty()).then(|| sidebar_apps(&app_rows, &rc));
                card(palette)
                    .flex()
                    .flex_col()
                    .gap(px(tokens::MD))
                    .child(row(SIDE_OUT_SLIDER, &s.output, names.0.clone(), AudioCommand::SetVolume, false))
                    .child(row(SIDE_IN_SLIDER, &s.input, names.1.clone(), AudioCommand::SetMicVolume, true))
                    .when_some(app_list, |el, list| el.child(list))
                    .when(!s.available, |el| el.opacity(UNAVAILABLE_OPACITY))
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            sidebar: Some(sidebar),
            on_click: None,
            on_secondary_click: Some(Rc::new(|_, cx| send(cx, AudioCommand::ToggleMute))),
        }
    });

    r.command(ID, CommandSpec::new("audio status", tk("Output / input volume and mute"), |_, cx| Ok(json(&hub(cx).read(cx).audio))));
    // Per-app mixer and device pickers (same calls as Studio → Audio & media).
    r.command(ID, CommandSpec::new("audio streams", tk("Apps playing sound, with their own volume and mute"), |_, _| Ok(json(mandara_services::audio::mixer()?.streams))));
    r.command(ID, CommandSpec::new("audio devices", tk("Output and input devices with your names; the default is marked, hidden ones too"), |_, cx| {
        let m = mandara_services::audio::mixer()?;
        let cfg = crate::state::config(cx).read(cx).get().audio.clone();
        let annotate = |list: &[mandara_services::audio::Device]| -> Vec<serde_json::Value> {
            list.iter()
                .map(|d| {
                    let mut v = serde_json::to_value(d).unwrap_or_default();
                    if let Some(o) = v.as_object_mut() {
                        o.insert("label".into(), cfg.device_label(&d.name, &d.description).into());
                        o.insert("hidden".into(), cfg.is_hidden(&d.name).into());
                    }
                    v
                })
                .collect()
        };
        Ok(serde_json::json!({ "outputs": annotate(&m.outputs), "inputs": annotate(&m.inputs) }))
    }));
    for (path, summary, output) in [("audio output next", tk("Switch to the next output device (skips hidden ones)"), true), ("audio input next", tk("Switch to the next input device (skips hidden ones)"), false)] {
        r.command(
            ID,
            CommandSpec::new(path, summary, move |_, cx| {
                let m = mandara_services::audio::mixer()?;
                let cfg = crate::state::config(cx).read(cx).get().audio.clone();
                let list = if output { &m.outputs } else { &m.inputs };
                let next = next_device(list, &cfg).ok_or_else(|| t("No other device to switch to").to_string())?;
                if output { mandara_services::audio::set_default_output(&next.name)? } else { mandara_services::audio::set_default_input(&next.name)? }
                Ok(serde_json::json!({ "default": next.name, "label": cfg.device_label(&next.name, &next.description) }))
            }),
        );
    }
    r.command(
        ID,
        CommandSpec::new("audio stream volume", tk("Set one app stream's volume in percent (0-150)"), |a, _| {
            let index = stream_index(a.number(0)?)?;
            mandara_services::audio::set_stream_volume(index, a.number(1)? as f32 / PERCENT)?;
            Ok(serde_json::json!({ "stream": index, "volume": a.number(1)? }))
        })
        .arg(ArgSpec::required("stream", ArgKind::Number, tk("index from `audio streams`")))
        .arg(ArgSpec::required("percent", ArgKind::Number, "0-150"))
        .example("audio stream volume 42 60"),
    );
    r.command(
        ID,
        CommandSpec::new_output("audio stream mute", tk("Mute or unmute one app stream"), |a, cx| {
            let index = stream_index(a.number(0)?)?;
            // Your own mute: while everything is muted, kept for its release.
            let muted = mandara_services::audio::mixer()?.streams.into_iter().find(|s| s.index == index).ok_or("no such stream")?.own_muted;
            Ok(Output::Pending(apps::request_then_refresh(AudioCommand::SetStreamMute(vec![index], !muted), cx)?))
        })
        .arg(ArgSpec::required("stream", ArgKind::Number, tk("index from `audio streams`"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("audio stream output", tk("Send one app stream to another output"), |a, cx| {
            let index = stream_index(a.number(0)?)?;
            request(cx, AudioCommand::MoveStreams(vec![index], a.str(1)?.to_string()))
        })
        .arg(ArgSpec::required("stream", ArgKind::Number, tk("index from `audio streams`")))
        .arg(ArgSpec::required("device", ArgKind::String, tk("device name from `audio devices`"))),
    );
    for (path, summary, output) in [("audio output set", tk("Make a device the default output"), true), ("audio input set", tk("Make a device the default input"), false)] {
        r.command(
            ID,
            CommandSpec::new(path, summary, move |a, _| {
                let name = a.str(0)?.to_string();
                if output { mandara_services::audio::set_default_output(&name)? } else { mandara_services::audio::set_default_input(&name)? }
                Ok(serde_json::json!({ "default": name }))
            })
            .arg(ArgSpec::required("device", ArgKind::String, tk("device name from `audio devices`"))),
        );
    }
    r.command(
        ID,
        CommandSpec::new_output("audio volume set", tk("Set output volume in percent (0-150)"), |a, cx| request(cx, AudioCommand::SetVolume(a.number(0)? as f32 / PERCENT)))
            .arg(ArgSpec::required("percent", ArgKind::Number, "0-150"))
            .example("audio volume set 40"),
    );
    for (path, summary, sign) in [("audio volume up", tk("Raise volume"), 1.0), ("audio volume down", tk("Lower volume"), -1.0)] {
        r.command(
            ID,
            CommandSpec::new_output(path, summary, move |a, cx| {
                let step = a.get(0).and_then(|s| s.parse::<f32>().ok()).map(|p| p / PERCENT).unwrap_or_else(|| step(cx));
                request(cx, AudioCommand::AdjustVolume(sign * step))
            })
            .arg(ArgSpec::optional("step", ArgKind::Number, tk("percent, default: Studio → Audio & media → Volume step"))),
        );
    }
    r.command(ID, CommandSpec::new_output("audio mute", tk("Toggle output mute"), |_, cx| request(cx, AudioCommand::ToggleMute)));
    r.command(
        ID,
        CommandSpec::new_output("audio mute-all", tk("Mute all sound: every output and app at once; off restores your own mutes"), |a, cx| match a.str(0)? {
            "on" => request(cx, AudioCommand::SetMuteAll(true)),
            "off" => request(cx, AudioCommand::SetMuteAll(false)),
            "toggle" => request(cx, AudioCommand::ToggleMuteAll),
            _ => {
                let saved = mandara_services::audio::mute::MuteState::load(&mandara_services::audio::mute::state_path());
                Ok(Output::Value(serde_json::json!({
                    "all_muted": hub(cx).read(cx).audio.all_muted,
                    "outputs": saved.sinks.len(),
                    "streams": saved.streams.len(),
                    "pending": saved.pending.len(),
                })))
            }
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on / off / toggle / status")).choices(&["on", "off", "toggle", "status"]))
        .example("audio mute-all toggle"),
    );
    r.command(ID, CommandSpec::new_output("audio mic mute", tk("Toggle microphone mute"), |_, cx| request(cx, AudioCommand::ToggleMicMute)));
    r.command(
        ID,
        CommandSpec::new_output("audio mic volume set", tk("Set microphone volume in percent"), |a, cx| request(cx, AudioCommand::SetMicVolume(a.number(0)? as f32 / PERCENT)))
            .arg(ArgSpec::required("percent", ArgKind::Number, "0-150")),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_device_skips_hidden_and_wraps() {
        use mandara_services::audio::Device;
        let d = |name: &str, default: bool| Device { name: name.into(), description: name.into(), is_default: default, ..Default::default() };
        let mut cfg = mandara_config::alerts::AudioConfig::default();
        let list = [d("a", false), d("b", true), d("c", false)];
        assert_eq!(next_device(&list, &cfg).map(|d| d.name.as_str()), Some("c"));
        cfg.hidden_devices.push("c".into());
        assert_eq!(next_device(&list, &cfg).map(|d| d.name.as_str()), Some("a"));
        cfg.hidden_devices.push("a".into());
        assert!(next_device(&list, &cfg).is_none(), "nothing else to switch to");
    }

    #[test]
    fn volume_labels_cover_boost_range() {
        assert_eq!(volume_text(0.42).as_ref(), "42%");
        assert_eq!(volume_text(1.37).as_ref(), "137%");
        assert_eq!(volume_text(9.0).as_ref(), format!("{:.0}%", MAX_VOLUME * PERCENT));
        assert_eq!(volume_icon(0.0, false), "volume-mute");
        assert_eq!(volume_icon(0.3, false), "volume-low");
        assert_eq!(volume_icon(0.9, true), "volume-mute");
        let all = AudioState { all_muted: true, ..Default::default() };
        assert_eq!(audio_icon(&all), "volume-off", "global mute has its own glyph");
        assert!(crate::icons::ICON_NAMES.contains(&audio_icon(&all)));
        assert!(PANEL_H_MUTED > PANEL_H);
    }

    #[test]
    fn stream_indices_are_validated() {
        assert_eq!(stream_index(42.0), Ok(42));
        assert!(stream_index(-1.0).is_err() && stream_index(1.5).is_err());
    }
}
