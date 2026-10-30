//! Soundscapes plugin: an ambient mixer synthesized live in pure Rust —
//! Corniche waves, rain, wind, café murmur, fireplace and white / pink /
//! brown noise — with per-layer and master volume, presets, a sleep timer
//! that fades out, and play / pause. Audio plays as "Mandara Soundscapes"
//! in every mixer; the audio thread exists only while sound is playing.
//!
//! The bar chip breathes gently while playing. Levels persist in
//! `~/.local/state/mandara/soundscapes.toml`. Settings
//! (`[plugins.settings.soundscapes]`): `bar_timer`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, FontWeight, SharedString, InteractiveElement, IntoElement, ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::soundscapes::{self as ss, LAYERS, Layer, PRESETS, Soundscapes};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{Flash, Segment, caption, segmented, toast};
use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, Cached, PanelShape, SECTION_H_PX, WASH_TRACK, alpha, big_slider, glass_card, glow, header, partner, percent_text,
    readable, redraw_in, section,
};
use crate::fonts::tabular_nums;
use crate::icons::{icon, ring};
use crate::kit::{reveal, slider, text};

pub const ID: &str = "soundscapes";

/// Panel height changes smaller than this don't resize the surface.
const RESIZE_EPSILON_PX: f32 = 0.5;

const PANEL_W: f32 = 404.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const SECTION_H: f32 = SECTION_H_PX;
/// Gap between a section label and its content.
const SECTION_GAP: f32 = 6.0;
const PRESET_H: f32 = 30.0;
const MASTER_H: f32 = 40.0;
const LAYER_H: f32 = 64.0;
const LAYER_GAP: f32 = 8.0;
const TIMER_H: f32 = 34.0;
const TIMER_INFO_H: f32 = 22.0;
const FOOTER_H: f32 = 16.0;
const PLAY_D: f32 = 38.0;
/// Bar chip animation cadence while playing (gentle, not per-frame).
const CHIP_FRAME: Duration = Duration::from_millis(40);
/// Panel animation cadence while playing (the play button glows with the
/// live output level).
const PANEL_FRAME: Duration = Duration::from_millis(33);
/// Panel cadence while only the sleep countdown changes (whole seconds).
const COUNTDOWN_FRAME: Duration = Duration::from_secs(1);
/// Bar chip "breathing" bars.
const CHIP_BARS: usize = 3;
const CHIP_BAR_MAX: f32 = 12.0;
const CHIP_BAR_MIN: f32 = 3.0;
const CHIP_BAR_W_PX: f32 = 3.0;
const CHIP_BAR_GAP_PX: f32 = 2.0;
const CHIP_GAP_PX: f32 = 6.0;
const CHIP_TIMER_TEXT_PX: f32 = 10.5;
const BAR_ICON_PX: f32 = 15.0;
/// Chip bar height follows the output RMS × this gain, kept in this band.
const CHIP_LEVEL_GAIN: f32 = 4.0;
const CHIP_LEVEL_MIN: f32 = 0.15;
/// Chip bars shrink to this share while the mix fades out.
const CHIP_FADING_SCALE: f32 = 0.4;
/// Breathing: base rate (rad/s), per-bar rate spread and phase offset.
const BREATH_RATE: f32 = 0.9;
const BREATH_RATE_STEP: f32 = 0.37;
const BREATH_PHASE_STEP: f32 = 1.7;
/// Breathing amplitude: resting share plus the sine's share.
const BREATH_BASE: f32 = 0.55;
const BREATH_SWING: f32 = 0.45;

// Play button glow: base alpha, level gain, level cap; blur base and gain.
const PLAY_GLOW_ALPHA: f32 = 0.25;
const PLAY_GLOW_LEVEL_CAP: f32 = 0.25;
const PLAY_GLOW_ALPHA_GAIN: f32 = 1.6;
const PLAY_GLOW_BLUR_PX: f32 = 10.0;
const PLAY_GLOW_BLUR_GAIN_PX: f32 = 30.0;
const PLAY_ICON_PX: f32 = 17.0;

// Presets.
const PRESET_GAP_PX: f32 = 6.0;
const PRESET_PAD_X_PX: f32 = 11.0;
const PRESET_TEXT_PX: f32 = text::LABEL + 0.5;
/// Active preset fill `(dark, light)` alphas.
const PRESET_ACTIVE: (f32, f32) = (0.2, 0.14);
/// Idle preset fill alpha.
const PRESET_IDLE_ALPHA: f32 = 0.05;

// Layer cards.
/// Layer cards are this much tighter than the panel's concentric radius.
const LAYER_RADIUS_INSET_PX: f32 = 4.0;
const LAYER_PAD_X_PX: f32 = 12.0;
const LAYER_PAD_Y_PX: f32 = 9.0;
const LAYER_HEAD_GAP_PX: f32 = 7.0;
const LAYER_GLYPH_PX: f32 = 22.0;
const LAYER_GLYPH_RADIUS_PX: f32 = 7.0;
const LAYER_GLYPH_FILL_ALPHA: f32 = 0.16;
const LAYER_ICON_PX: f32 = 15.0;
const SWATCH_PX: f32 = 11.0;
const SWATCH_EDGE_ALPHA: f32 = 0.2;
/// Active card tint mix `(dark, light)`.
const LAYER_ON_MIX: (f32, f32) = (0.12, 0.10);

// Sleep timer.
const TIMER_GAP_PX: f32 = 8.0;
const TIMER_RING_PX: f32 = 14.0;
const TIMER_RING_STROKE_PX: f32 = 2.0;
const TIMER_TRACK_ALPHA: f32 = 0.12;
const ERROR_GAP_PX: f32 = 6.0;
const ERROR_ICON_PX: f32 = 13.0;

const SECS_PER_HOUR: u64 = 3600;
/// Output RMS is reported rounded to this many steps per unit.
const RMS_STEPS: f64 = 1000.0;

// Layer swatch / tint colours as OKLCH `(lightness, chroma, hue°)`.
const WHITE_NOISE: (f32, f32, f32) = (0.95, 0.0, 0.0);
const PINK_NOISE: (f32, f32, f32) = (0.78, 0.11, 0.0);
const BROWN_NOISE: (f32, f32, f32) = (0.55, 0.08, 55.0);
const SEA_TINT: (f32, f32, f32) = (0.72, 0.11, 215.0);
const RAIN_TINT: (f32, f32, f32) = (0.72, 0.08, 250.0);
const WIND_TINT: (f32, f32, f32) = (0.78, 0.06, 170.0);
const CAFE_TINT: (f32, f32, f32) = (0.72, 0.09, 60.0);
const FIRE_TINT: (f32, f32, f32) = (0.72, 0.15, 40.0);

fn oklch((l, c, h): (f32, f32, f32)) -> Color {
    Color::oklch(l, c, h)
}
/// Sleep timer choices (minutes; 0 = off).
const SLEEP_CHOICES: [u64; 5] = [0, 15, 30, 60, 90];
const SECS_PER_MIN: u64 = 60;
const PERCENT: f32 = 100.0;

thread_local! {
    static MIXER: RefCell<Option<Soundscapes>> = const { RefCell::new(None) };
    static STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn mixer() -> Soundscapes {
    MIXER.with(|m| m.borrow_mut().get_or_insert_with(Soundscapes::new).clone())
}

/// Silences the mixer because the plugin was turned off (no-op when it
/// never played).
pub fn stop() {
    if let Some(m) = MIXER.with(|m| m.borrow().clone())
        && m.params().is_playing()
    {
        m.pause();
        STARTED.with(|s| s.set(None));
    }
}

fn layer_icon(layer: Layer) -> Option<&'static str> {
    match layer {
        Layer::Sea => Some("waves"),
        Layer::Rain => Some("rain"),
        Layer::Wind => Some("wind"),
        Layer::Cafe => Some("coffee"),
        Layer::Fire => Some("flame"),
        Layer::White | Layer::Pink | Layer::Brown => None,
    }
}

/// Swatch colours for the three noise colours.
fn noise_color(layer: Layer) -> Color {
    match layer {
        Layer::White => oklch(WHITE_NOISE),
        Layer::Pink => oklch(PINK_NOISE),
        _ => oklch(BROWN_NOISE),
    }
}

fn layer_tint(layer: Layer, palette: &MandaraPalette) -> Color {
    let c = match layer {
        Layer::Sea => oklch(SEA_TINT),
        Layer::Rain => oklch(RAIN_TINT),
        Layer::Wind => oklch(WIND_TINT),
        Layer::Cafe => oklch(CAFE_TINT),
        Layer::Fire => oklch(FIRE_TINT),
        other => noise_color(other),
    };
    readable(c, palette)
}

fn toggle(flash: Option<&Flash>, cx: &mut App) {
    match mixer().toggle() {
        Ok(playing) => {
            if playing {
                STARTED.with(|s| s.set(Some(Instant::now())));
            }
        }
        Err(e) => {
            if let Some(f) = flash {
                f.show(false, e);
            }
        }
    }
    cx.refresh_windows();
}

fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    if s >= SECS_PER_HOUR {
        format!("{}:{:02}:{:02}", s / SECS_PER_HOUR, (s % SECS_PER_HOUR) / SECS_PER_MIN, s % SECS_PER_MIN)
    } else {
        format!("{}:{:02}", s / SECS_PER_MIN, s % SECS_PER_MIN)
    }
}

thread_local! {
    /// The chip's "Nm" sleep label, formatted only when the minute changes.
    static CHIP_SLEEP: RefCell<Cached<u64>> = RefCell::new(Cached::default());
}

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let m = mixer();
    let p = m.params();
    let playing = p.is_playing() || p.is_running();
    let accent = readable(palette.primary, palette);
    if !playing {
        // Idle: a static glyph, no timer.
        return icon("waves", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface).into_any_element();
    }
    redraw_in(CHIP_FRAME, window, cx);
    let t0 = STARTED.with(|s| s.get()).unwrap_or_else(Instant::now);
    let t = t0.elapsed().as_secs_f32();
    let level = (p.output_level() * CHIP_LEVEL_GAIN).clamp(CHIP_LEVEL_MIN, 1.0) * if p.is_playing() { 1.0 } else { CHIP_FADING_SCALE };
    let bars = (0..CHIP_BARS).map(|i| {
        // Slow, out-of-phase breathing scaled by the real output level.
        let k = i as f32;
        let phase = t * (BREATH_RATE + k * BREATH_RATE_STEP) + k * BREATH_PHASE_STEP;
        let h = CHIP_BAR_MIN + (CHIP_BAR_MAX - CHIP_BAR_MIN) * level * (BREATH_BASE + BREATH_SWING * phase.sin());
        div().w(px(CHIP_BAR_W_PX)).h(px(h)).rounded_full().bg(accent.mix(partner(accent), k / CHIP_BARS as f32).hsla())
    });
    let show_timer = crate::state::config(cx).read(cx).get().plugins.setting(ID, "bar_timer", true);
    let sleep = p
        .sleep_remaining()
        .filter(|_| show_timer)
        .map(|(left, _)| CHIP_SLEEP.with(|c| c.borrow_mut().get(left.as_secs().div_ceil(SECS_PER_MIN), |m| format!("{m}m"))));
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(div().h(px(CHIP_BAR_MAX)).flex().items_center().gap(px(CHIP_BAR_GAP_PX)).children(bars))
        .when_some(sleep, |el, s| el.child(div().font_features(tabular_nums()).text_size(px(CHIP_TIMER_TEXT_PX)).text_color(palette.on_surface_variant.hsla()).child(s)))
        .into_any_element()
}

fn play_button(playing: bool, level: f32, flash: &Flash, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let flash = flash.clone();
    div()
        .id("ss-play")
        .size(px(PLAY_D))
        .flex_none()
        .rounded_full()
        .cursor_pointer()
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness())))
        .when(playing, |el| {
            el.shadow(vec![glow(
                accent.with_alpha(PLAY_GLOW_ALPHA + level.min(PLAY_GLOW_LEVEL_CAP) * PLAY_GLOW_ALPHA_GAIN),
                PLAY_GLOW_BLUR_PX + level * PLAY_GLOW_BLUR_GAIN_PX,
                0.0,
                1.0,
            )])
        })
        .flex()
        .items_center()
        .justify_center()
        .child(icon(if playing { "pause" } else { "play" }, PLAY_ICON_PX, palette.on_primary))
        .on_click(move |_, _, cx| toggle(Some(&flash), cx))
        .into_any_element()
}

fn presets(current: Option<&'static str>, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = palette.on_surface.with_alpha(WASH_TRACK.0);
    div()
        .flex()
        .gap(px(PRESET_GAP_PX))
        .children(PRESETS.iter().enumerate().map(|(i, p)| {
            let active = current == Some(p.key);
            let key = p.key;
            div()
                .id(("ss-preset", i))
                .h(px(PRESET_H))
                .px(px(PRESET_PAD_X_PX))
                .flex_1()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(PRESET_TEXT_PX))
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .when(active, |el| el.bg(accent.with_alpha(alpha(PRESET_ACTIVE, palette)).hsla()).text_color(accent.hsla()))
                .when(!active, |el| el.bg(palette.on_surface.with_alpha(PRESET_IDLE_ALPHA).hsla()).text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
                .child(crate::bidi::paragraph(tr(p.label)))
                .on_click(move |_, window, _| {
                    if let Some(p) = ss::preset(key) {
                        mixer().apply_preset(p);
                    }
                    window.refresh();
                })
        }))
        .into_any_element()
}

fn layer_card(index: usize, layer: Layer, level: f32, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let tint = layer_tint(layer, palette);
    let on = level > 0.0;
    let glyph: AnyElement = match layer_icon(layer) {
        Some(name) => icon(name, LAYER_ICON_PX, tint).into_any_element(),
        None => div()
            .size(px(SWATCH_PX))
            .rounded_full()
            .bg(noise_color(layer).hsla())
            .border_1()
            .border_color(palette.on_surface.with_alpha(SWATCH_EDGE_ALPHA).hsla())
            .into_any_element(),
    };
    glass_card(palette)
        .rounded(px(crate::style::concentric(radius, LAYER_RADIUS_INSET_PX)))
        .flex_1()
        .min_w(px(0.0))
        .h(px(LAYER_H))
        .px(px(LAYER_PAD_X_PX))
        .py(px(LAYER_PAD_Y_PX))
        .flex()
        .flex_col()
        .justify_between()
        .when(on, |el| el.bg(palette.surface_container.mix(tint, alpha(LAYER_ON_MIX, palette)).hsla()))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(LAYER_HEAD_GAP_PX))
                .child(
                    div()
                        .size(px(LAYER_GLYPH_PX))
                        .flex_none()
                        .rounded(px(crate::style::radius(LAYER_GLYPH_RADIUS_PX)))
                        .bg(tint.with_alpha(LAYER_GLYPH_FILL_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph),
                )
                .child(div().flex_1().min_w(px(0.0)).text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).truncate().child(tr(layer.label())))
                .child(
                    div()
                        .font_features(tabular_nums())
                        .text_size(px(text::LABEL))
                        .text_color(if on { tint.hsla() } else { palette.on_surface_variant.hsla() })
                        .child(percent_text(level)),
                ),
        )
        .child(slider(("ss-layer", index), level, palette, move |v, window, _| {
            mixer().set_level(layer, v);
            window.refresh();
        }))
        .into_any_element()
}

thread_local! {
    /// "15 m", "30 m"… segment labels (built once; "Off" is translated live).
    static SLEEP_LABELS: Vec<SharedString> = SLEEP_CHOICES.iter().map(|m| SharedString::from(format!("{m} m"))).collect();
}

fn timer(flash: &Flash, countdown: &mut Cached<u64>, palette: &MandaraPalette) -> AnyElement {
    let m = mixer();
    let remaining = m.params().sleep_remaining();
    let selected = remaining.map(|(_, total)| total.as_secs() / SECS_PER_MIN);
    let segments = SLEEP_CHOICES
        .iter()
        .enumerate()
        .map(|(i, &mins)| {
            let flash = flash.clone();
            Segment {
                label: if mins == 0 { t("Off").into() } else { SLEEP_LABELS.with(|l| l[i].clone()) },
                icon: None,
                active: if mins == 0 { remaining.is_none() } else { selected == Some(mins) },
                on_click: Rc::new(move |window, _| {
                    let m = mixer();
                    if mins == 0 {
                        m.cancel_sleep();
                    } else {
                        m.set_sleep(Duration::from_secs(mins * SECS_PER_MIN));
                        if !m.params().is_playing() {
                            if let Err(e) = m.play() {
                                flash.show(false, e);
                            } else {
                                STARTED.with(|s| s.set(Some(Instant::now())));
                            }
                        }
                    }
                    window.refresh();
                }),
            }
        })
        .collect();
    let info: AnyElement = match remaining {
        Some((left, total)) => {
            let accent = readable(palette.primary, palette);
            let stops = countdown.get(left.as_secs(), |s| tf("Stops in {time}", &[("time", &mmss(Duration::from_secs(*s)))]));
            div()
                .h(px(TIMER_INFO_H))
                .flex()
                .items_center()
                .gap(px(TIMER_GAP_PX))
                .child(ring(left.as_secs_f32() / total.as_secs_f32().max(1.0), TIMER_RING_PX, TIMER_RING_STROKE_PX, accent, palette.on_surface.with_alpha(TIMER_TRACK_ALPHA)))
                .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(stops))
                .child(caption(tf("fades out over the last {n} s", &[("n", &ss::SLEEP_FADE.as_secs())]), palette))
                .into_any_element()
        }
        None => div().h(px(TIMER_INFO_H)).flex().items_center().child(caption(t("Plays until you pause it"), palette)).into_any_element(),
    };
    div().flex().flex_col().gap(px(SECTION_GAP)).child(segmented("ss-sleep", segments, palette)).child(info).into_any_element()
}

fn panel_height(error: bool) -> f32 {
    let rows = LAYERS.div_ceil(2) as f32;
    PAD * 2.0
        + super::panel_fx::HEADER_H
        + GAP
        + PRESET_H
        + GAP
        + MASTER_H
        + GAP
        + SECTION_H
        + SECTION_GAP
        + rows * LAYER_H
        + (rows - 1.0) * LAYER_GAP
        + GAP
        + SECTION_H
        + SECTION_GAP
        + TIMER_H
        + SECTION_GAP
        + TIMER_INFO_H
        + GAP
        + FOOTER_H
        + if error { GAP + TIMER_INFO_H } else { 0.0 }
}

struct View {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<Pixels>>>,
    flash: Flash,
    /// Header subtitle keyed by `(preset, status)`.
    subtitle: RefCell<Cached<(Option<&'static str>, u8)>>,
    /// "Stops in …" keyed by whole seconds left.
    countdown: RefCell<Cached<u64>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = mixer();
        let params = m.params();
        let playing = params.is_playing();
        let visible = self.p.progress() > 0.0;
        if visible && (playing || params.is_running()) {
            redraw_in(PANEL_FRAME, window, cx);
        } else if visible && params.sleep_remaining().is_some() {
            redraw_in(COUNTDOWN_FRAME, window, cx);
        }
        let error = m.error();
        let h = panel_height(error.is_some());
        if (f32::from(self.size_cell.get().height) - h).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }
        let radius = PanelShape::of(cx).inner(PAD);
        let accent = readable(palette.primary, palette);
        let preset = m.current_preset();
        /// Status keys for the cached subtitle.
        const PLAYING: u8 = 0;
        const FADING: u8 = 1;
        const PAUSED: u8 = 2;
        let status = if playing { PLAYING } else if params.is_running() { FADING } else { PAUSED };
        let subtitle = self.subtitle.borrow_mut().get((preset.map(|p| p.label), status), |(label, status)| {
            let mix_name = label.map(tr).unwrap_or_else(|| t("Custom mix").to_string());
            let status = match *status {
                PLAYING => t("Playing"),
                FADING => t("Fading out…"),
                _ => t("Paused"),
            };
            format!("{mix_name} · {status}")
        });
        let head = header("waves", accent, t("Soundscapes"), subtitle, palette, Some(play_button(playing, params.output_level(), &self.flash, palette)));
        let master = big_slider("ss-master", params.master(), "volume-high", false, palette, |v, window, _| {
            mixer().set_master(v);
            window.refresh();
        });
        let rows = Layer::ALL.chunks(2).enumerate().map(|(row, pair)| {
            div().flex().gap(px(LAYER_GAP)).children(pair.iter().enumerate().map(|(col, l)| layer_card(row * 2 + col, *l, params.level_of(*l), palette, radius)))
        });
        let layers = div().flex().flex_col().gap(px(LAYER_GAP)).children(rows);
        let p = &self.p;
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(p, 1, presets(preset.map(|p| p.key), palette)))
            .child(div().h(px(GAP)))
            .child(reveal(p, 2, master))
            .child(div().h(px(GAP)))
            .child(reveal(p, 3, div().flex().flex_col().gap(px(SECTION_GAP)).child(section(t("LAYERS"), palette, None)).child(layers)))
            .child(div().h(px(GAP)))
            .child(reveal(
                p,
                4,
                div().flex().flex_col().gap(px(SECTION_GAP)).child(section(t("SLEEP TIMER"), palette, None)).child(timer(&self.flash, &mut self.countdown.borrow_mut(), palette)),
            ))
            .when_some(error, |el, e| {
                let coral = readable(palette.urgent_coral, palette);
                el.child(div().h(px(GAP))).child(div().h(px(TIMER_INFO_H)).flex().items_center().gap(px(ERROR_GAP_PX)).child(icon("alert", ERROR_ICON_PX, coral)).child(div().text_size(px(text::BODY_S)).text_color(coral.hsla()).truncate().child(e)))
            })
            .child(div().h(px(GAP)))
            .child(reveal(p, 5, div().h(px(FOOTER_H)).flex().justify_center().child(caption(t("Synthesized live — no audio files · listed as Mandara Soundscapes"), palette).truncate())))
            .when_some(self.flash.current(window, cx), |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }
}

fn status_json() -> serde_json::Value {
    let m = mixer();
    let p = m.params();
    json!({
        "playing": p.is_playing(),
        "streaming": p.is_running(),
        "output_rms": (f64::from(p.output_level()) * RMS_STEPS).round() / RMS_STEPS,
        "master": (p.master() * PERCENT).round(),
        "layers": Layer::ALL.iter().map(|l| (l.key(), (p.level_of(*l) * PERCENT).round())).collect::<std::collections::BTreeMap<_, _>>(),
        "preset": m.current_preset().map(|p| p.key),
        "sleep_remaining_s": p.sleep_remaining().map(|(left, _)| left.as_secs()),
        "error": m.error(),
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Soundscapes"), tk("Ambient sea, rain, wind, café, fire and noise — synthesized live"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(None, bar_view, window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height(false)))));
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone(), flash: Flash::default(), subtitle: RefCell::default(), countdown: RefCell::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), sidebar: None, on_click: None, on_secondary_click: Some(Rc::new(|_, cx| toggle(None, cx))) }
    });
    r.command(ID, CommandSpec::new("soundscapes status", tk("Playback state, levels, preset and sleep timer"), |_, _| Ok(status_json())));
    for (path, summary, action) in [("soundscapes play", tk("Start the ambient mix"), 0u8), ("soundscapes pause", tk("Fade the mix out and stop"), 1), ("soundscapes toggle", tk("Play or pause"), 2)] {
        r.command(ID, CommandSpec::new(path, summary, move |_, cx| {
            let m = mixer();
            match action {
                0 => {
                    m.play()?;
                    STARTED.with(|s| s.set(Some(Instant::now())));
                }
                1 => m.pause(),
                _ => {
                    if m.toggle()? {
                        STARTED.with(|s| s.set(Some(Instant::now())));
                    }
                }
            }
            cx.refresh_windows();
            Ok(status_json())
        }));
    }
    r.command(
        ID,
        CommandSpec::new("soundscapes volume", tk("Set a layer's volume or the master volume (0–100)"), |a, cx| {
            let target = a.str(0)?;
            let v = (a.number(1)? as f32 / PERCENT).clamp(0.0, 1.0);
            let m = mixer();
            if target == "master" {
                m.set_master(v);
            } else {
                m.set_level(Layer::parse(target).ok_or_else(|| tf("Unknown layer {name}", &[("name", &target)]))?, v);
            }
            cx.refresh_windows();
            Ok(status_json())
        })
        .arg(ArgSpec::required("layer", ArgKind::String, tk("master or a layer")).choices(&["master", "sea", "rain", "wind", "cafe", "fire", "white", "pink", "brown"]))
        .arg(ArgSpec::required("percent", ArgKind::Number, tk("0–100")))
        .example("soundscapes volume rain 60"),
    );
    r.command(
        ID,
        CommandSpec::new("soundscapes preset", tk("Load a preset mix"), |a, cx| {
            let p = ss::preset(a.str(0)?).ok_or_else(|| t("Unknown preset").to_string())?;
            mixer().apply_preset(p);
            cx.refresh_windows();
            Ok(status_json())
        })
        .arg(ArgSpec::required("name", ArgKind::String, tk("preset")).choices(&["corniche-night", "rainy-study", "deep-focus", "fireside"]))
        .example("soundscapes preset corniche-night"),
    );
    r.command(
        ID,
        CommandSpec::new("soundscapes sleep", tk("Stop after N minutes (fading out), or `off`"), |a, cx| {
            let m = mixer();
            match a.str(0)? {
                "off" | "0" => m.cancel_sleep(),
                mins => {
                    let mins: u64 = mins.parse().map_err(|_| t("Expected minutes or off").to_string())?;
                    m.set_sleep(Duration::from_secs(mins.max(1) * SECS_PER_MIN));
                }
            }
            cx.refresh_windows();
            Ok(status_json())
        })
        .arg(ArgSpec::required("minutes", ArgKind::String, tk("minutes, or off")))
        .example("soundscapes sleep 30"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_text() {
        assert_eq!(mmss(Duration::from_secs(61)), "1:01");
        assert_eq!(mmss(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn every_layer_has_a_glyph_or_swatch() {
        for l in Layer::ALL {
            assert!(layer_icon(l).is_some() || matches!(l, Layer::White | Layer::Pink | Layer::Brown));
        }
        assert!(panel_height(true) > panel_height(false));
    }
}
