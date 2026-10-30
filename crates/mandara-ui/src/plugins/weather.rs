//! Weather plugin: an animated sky scene for the current conditions (sun
//! rays, drifting clouds, rain, snow, lightning, fog, twinkling stars), an
//! hourly strip and a 5-day forecast. Data from Open-Meteo.
//!
//! Settings (`[plugins.settings.weather]`): `latitude`, `longitude`,
//! `location_name`, `animate` (bool). Location falls back to the
//! prayer-times location, then El-Mandara. Temperatures and wind follow the
//! global `[general] units` (the old per-plugin `fahrenheit = true` still
//! reads as Imperial, see [`ShellConfig::units`]).
//!
//! The scene repaints at [`SCENE_FRAME`] only while it is painted, and
//! only the view that painted it is notified. Because each scene frame
//! re-renders that view, every label is formatted once per weather update
//! (or units / language / config change) and the scene's colours are
//! parsed once.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::f32::consts::TAU;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ContentMask, EntityId, FontWeight, Global, IntoElement, ParentElement, PathBuilder, Pixels, SharedString, Styled, Window,
    canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::FluentBuilder as _, px, size,
};
use mandara_config::{ShellConfig, Units};
use mandara_core::ToGpui;
use mandara_i18n::Lang;
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::weather::{Day, Hour, Weather};
use mandara_services::{Condition, WeatherCommand, WeatherQuery, WeatherState};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, ANGLE_RIGHT_DEG, CARD_RADIUS_PX, SMALL_ICON_PX};
use crate::fonts::{fonts, tabular_nums};
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{card, reveal, text, tokens};
use crate::state::config;

pub const ID: &str = "weather";

/// Scene frame interval: drifting weather reads smoothly at 30 fps and
/// costs a fraction of a full-rate redraw.
const SCENE_FRAME: Duration = Duration::from_millis(33);

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W_PX: f32 = 420.0;
const PANEL_H_PX: f32 = 628.0;
/// Panel hero height and its temperature size; the sidebar card's.
const PANEL_HERO_H_PX: f32 = 188.0;
const PANEL_HERO_TEMP_PX: f32 = 58.0;
const SIDEBAR_HERO_H_PX: f32 = 150.0;
const SIDEBAR_HERO_TEMP_PX: f32 = 46.0;
/// Sidebar placeholder height.
const SIDEBAR_PLACEHOLDER_H_PX: f32 = 120.0;
/// Hours in the panel strip and in the sidebar strip.
const PANEL_HOURS: usize = 8;
const SIDEBAR_HOURS: usize = 6;
/// Card paddings: hourly strip in the panel / sidebar, daily rows.
const HOURLY_PAD_Y_PX: f32 = 12.0;
const SIDEBAR_HOURLY_PAD_Y_PX: f32 = 10.0;
const DAILY_PAD_Y_PX: f32 = 6.0;
/// Hero height at which the scene art is drawn 1:1.
const ART_BASE_H_PX: f32 = 180.0;
/// Hero text inset (left, top / bottom) and its opacity levels.
const HERO_INSET_X_PX: f32 = 18.0;
const HERO_INSET_Y_PX: f32 = 14.0;
const HERO_PLACE_OPACITY: f32 = 0.9;
const HERO_RANGE_OPACITY: f32 = 0.85;
/// Temperature line height relative to its size.
const HERO_TEMP_LINE: f32 = 1.05;
/// Stat tiles: padding, radius, gaps.
const STAT_PAD_X_PX: f32 = 10.0;
const STAT_PAD_Y_PX: f32 = 8.0;
const STAT_RADIUS_PX: f32 = 12.0;
const STAT_LINE_GAP_PX: f32 = 2.0;
const STAT_ICON_GAP_PX: f32 = 5.0;
const STATS_GAP_PX: f32 = 6.0;
/// Hourly strip: column width, gap, icon size and precipitation line.
const HOUR_W_PX: f32 = 40.0;
const HOUR_GAP_PX: f32 = 4.0;
const HOUR_ICON_PX: f32 = 18.0;
const PRECIP_LINE_H_PX: f32 = 12.0;
const PRECIP_TEXT_PX: f32 = 10.0;
/// Precipitation chances below this percentage aren't shown.
const PRECIP_SHOWN_PCT: u8 = 20;
/// Daily rows: height, gaps, columns, the range track and its minimum fill.
const DAY_H_PX: f32 = 32.0;
const DAY_ROW_GAP_PX: f32 = 2.0;
const DAY_COL_GAP_PX: f32 = 10.0;
const DAY_NAME_W_PX: f32 = 48.0;
const DAY_ICON_PX: f32 = 17.0;
const DAY_PRECIP_W_PX: f32 = 34.0;
const DAY_PRECIP_TEXT_PX: f32 = 10.5;
const DAY_TEMP_W_PX: f32 = 30.0;
const RANGE_TRACK_W_PX: f32 = 110.0;
const RANGE_TRACK_H_PX: f32 = 5.0;
const RANGE_MIN_FILL_PX: f32 = 6.0;
/// Range track ink (same on dark and light themes).
const RANGE_TRACK_ALPHA: f32 = 0.08;
/// Week-scale gradient ends and how far they lean toward the theme accent.
const RANGE_COOL: &str = "#5fb6ff";
const RANGE_WARM: &str = "#ffb35c";
const RANGE_COOL_TINT: f32 = 0.2;
const RANGE_WARM_TINT: f32 = 0.15;
/// Placeholder: gap, icon size / alpha and text inset.
const PLACEHOLDER_GAP_PX: f32 = 8.0;
const PLACEHOLDER_ICON_PX: f32 = 26.0;
const PLACEHOLDER_ICON_ALPHA: f32 = 0.7;
const PLACEHOLDER_PAD_X_PX: f32 = 16.0;
/// Bar chip: icon and gap.
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_GAP_PX: f32 = 5.0;

/// Panel reveal order.
const REVEAL_HERO: usize = 0;
const REVEAL_STATS: usize = 1;
const REVEAL_HOURLY: usize = 2;
const REVEAL_DAILY: usize = 3;

// ── Scene art (px at a 180 px hero; × art scale) ─────────────────────────

/// Scene time used when animation is off (a pleasant still frame), s.
const STILL_TIME_S: f32 = 2.0;
/// Celestial body anchor as fractions of the scene (upper-right).
const BODY_X_FRAC: f32 = 0.78;
const BODY_Y_FRAC: f32 = 0.36;
/// Cloud bobbing: rate (rad/s) and amplitude (px).
const BOB_RATE: f32 = 0.35;
const BOB_PX: f32 = 3.0;
/// Drifting clouds: start spacing (fraction of width), wrap margin.
const DRIFT_SPACING: f32 = 2.6;
const DRIFT_MARGIN_PX: f32 = 60.0;
/// Clear sky: sun / moon radius, star count.
const CLEAR_SUN_R_PX: f32 = 26.0;
const CLEAR_MOON_R_PX: f32 = 20.0;
const CLEAR_STARS: u32 = 34;
/// Partly cloudy: body offset / radii, star count, main and drifting cloud.
const PARTLY_BODY_DX_PX: f32 = 10.0;
const PARTLY_BODY_DY_PX: f32 = -8.0;
const PARTLY_SUN_R_PX: f32 = 22.0;
const PARTLY_MOON_R_PX: f32 = 17.0;
const PARTLY_STARS: u32 = 22;
const PARTLY_CLOUD: (f32, f32, f32) = (-12.0, 18.0, 34.0);
const PARTLY_CLOUD_ALPHA: f32 = 0.96;
const PARTLY_DRIFT: (f32, f32, f32, f32) = (1.0, 7.0, 0.2, 16.0);
const PARTLY_DRIFT_ALPHA: f32 = 0.5;
/// Overcast: drifting cloud (phase, speed, y fraction, size), two banks.
const OVERCAST_DRIFT: (f32, f32, f32, f32) = (0.0, 5.0, 0.22, 20.0);
const OVERCAST_DRIFT_ALPHA: f32 = 0.7;
const OVERCAST_BACK: (f32, f32, f32, f32) = (1.0, -30.0, -4.0, 30.0);
const OVERCAST_FRONT: (f32, f32, f32, f32) = (2.0, 8.0, 14.0, 36.0);
const OVERCAST_ALPHA: f32 = 0.95;
/// Rain / thunder: drop counts, drifting cloud, two banks, tones.
const RAIN_DROPS_HEAVY: u32 = 46;
const RAIN_DROPS_LIGHT: u32 = 22;
const RAIN_DRIFT: (f32, f32, f32, f32) = (0.5, 6.0, 0.2, 18.0);
const RAIN_DRIFT_ALPHA: f32 = 0.75;
const RAIN_BACK: (f32, f32, f32, f32) = (1.0, -24.0, -2.0, 32.0);
const RAIN_FRONT: (f32, f32, f32, f32) = (2.5, 14.0, 12.0, 30.0);
const RAIN_CLOUD_ALPHA: f32 = 0.97;
const RAIN_FRONT_LIFT: f32 = 0.06;
/// Snow: flake count and its cloud.
const SNOW_FLAKES: u32 = 38;
const SNOW_CLOUD: (f32, f32, f32) = (-14.0, 0.0, 34.0);
const SNOW_CLOUD_ALPHA: f32 = 0.92;
/// Cloud lobes `(dx, dy, radius)` and base `(x, y, w, h, radius)` as
/// fractions of the cloud size; shadow drop and alpha.
const CLOUD_BASE: (f32, f32, f32, f32, f32) = (-1.1, -0.05, 2.2, 0.7, 0.35);
const CLOUD_LOBES: [(f32, f32, f32); 3] = [(-0.5, 0.0, 0.46), (0.08, -0.28, 0.62), (0.62, 0.06, 0.4)];
const CLOUD_SHADOW_DY: f32 = 0.1;
const CLOUD_SHADOW_ALPHA: f32 = 0.16;
/// Sun: halo radii / alphas (with a slow pulse), ray count, spin, reach.
const SUN_HALO_OUTER: f32 = 2.3;
const SUN_HALO_INNER: f32 = 1.65;
const SUN_HALO_ALPHA: f32 = 0.08;
const SUN_HALO_PULSE: (f32, f32) = (0.02, 1.3);
const SUN_HALO_INNER_ALPHA: f32 = 0.16;
const SUN_RAYS: u32 = 10;
const SUN_RAY_SPIN: f32 = 0.18;
const SUN_RAY_START: f32 = 1.22;
const SUN_RAY_REACH: f32 = 1.55;
const SUN_RAY_FLICKER: (f32, f32) = (0.12, 2.0);
const SUN_RAY_WIDTH: f32 = 0.11;
const SUN_RAY_ALPHA: f32 = 0.85;
/// Moon: halo radii / alphas (with a pulse) and the crescent bite.
const MOON_HALO_OUTER: f32 = 2.0;
const MOON_HALO_INNER: f32 = 1.45;
const MOON_HALO_ALPHA: f32 = 0.06;
const MOON_HALO_PULSE: (f32, f32) = (0.015, 0.9);
const MOON_HALO_INNER_ALPHA: f32 = 0.1;
const MOON_BITE: (f32, f32, f32) = (0.42, -0.28, 0.86);
/// Stars: band height, twinkle rate range, radius range, alpha range.
const STAR_BAND: f32 = 0.62;
const STAR_TWINKLE: (f32, f32) = (0.8, 2.2);
const STAR_R_PX: (f32, f32) = (0.7, 0.9);
const STAR_ALPHA: (f32, f32) = (0.25, 0.7);
/// Rain drops: fall speed (heavy / light) plus random extra, length
/// (heavy / light) plus random extra, slant, side margin, stroke, alpha.
const DROP_SPEED_PX_S: (f32, f32, f32) = (260.0, 150.0, 90.0);
const DROP_LEN_PX: (f32, f32, f32) = (11.0, 7.0, 6.0);
const DROP_SLANT: f32 = 0.25;
const DROP_DRIFT: f32 = 0.18;
const DROP_MARGIN_PX: f32 = 10.0;
const DROP_STROKE_PX: (f32, f32) = (1.6, 1.2);
const DROP_ALPHA: (f32, f32) = (0.35, 0.35);
/// Snow: fall speed range, sway rate / amplitude, radius and alpha ranges.
const FLAKE_SPEED_PX_S: (f32, f32) = (16.0, 26.0);
const FLAKE_SWAY_RATE: f32 = 0.6;
const FLAKE_SWAY_PX: f32 = 9.0;
const FLAKE_R_PX: (f32, f32) = (1.3, 1.8);
const FLAKE_ALPHA: (f32, f32) = (0.55, 0.4);
/// Fog: bank count, first bank / spacing (fractions of height), sway,
/// height range and alpha range.
const FOG_BANKS: u32 = 5;
const FOG_TOP: f32 = 0.28;
const FOG_SPACING: f32 = 0.15;
const FOG_LEFT: f32 = 0.25;
const FOG_WIDTH: f32 = 1.5;
const FOG_SWAY_RATE: (f32, f32) = (0.18, 0.04);
const FOG_SWAY_PX: f32 = 26.0;
const FOG_H_PX: (f32, f32) = (9.0, 8.0);
const FOG_ALPHA: (f32, f32) = (0.13, 0.12);
/// Lightning: period, flash length, the dim gap inside it, flash alpha,
/// bolt start, segments, jitter, stroke and alpha.
const BOLT_PERIOD_S: f32 = 4.6;
const BOLT_FLASH_S: f32 = 0.32;
const BOLT_GAP_S: (f32, f32) = (0.1, 0.16);
const BOLT_GAP_DIM: f32 = 0.4;
const BOLT_SKY_ALPHA: f32 = 0.22;
const BOLT_X: (f32, f32) = (0.45, 0.3);
const BOLT_TOP: f32 = 0.32;
const BOLT_SEGMENTS: u32 = 5;
const BOLT_SEGMENT_H: f32 = 0.11;
const BOLT_JITTER_PX: f32 = 18.0;
const BOLT_STROKE_PX: f32 = 2.6;
const BOLT_ALPHA: f32 = 0.95;
/// Sky gradient tint toward the theme accent (top, bottom).
const SKY_TINT: (f32, f32) = (0.1, 0.08);

/// Hash multipliers / resolution of the deterministic particle noise.
const HASH_A: u32 = 0x9E37_79B9;
const HASH_B: u32 = 0x85EB_CA6B;
const HASH_C: u32 = 0x2C1B_3C6D;
const HASH_SHIFT_A: u32 = 15;
const HASH_SHIFT_B: u32 = 12;
const HASH_RESOLUTION: u32 = 10_000;

// ── Settings ─────────────────────────────────────────────────────────────

/// A plugin's own coordinates: only when it has them and "Use one location
/// everywhere" is off. Everything else follows the shared location (Studio
/// → Date, time & location).
pub fn own_location(cfg: &ShellConfig, plugin: &str) -> Option<(f64, f64)> {
    if cfg.location.shared {
        return None;
    }
    let own = cfg.plugins.settings.get(plugin)?;
    (own.contains_key("latitude") && own.contains_key("longitude")).then(|| (cfg.plugins.setting(plugin, "latitude", cfg.location.latitude), cfg.plugins.setting(plugin, "longitude", cfg.location.longitude)))
}

pub fn query(cfg: &ShellConfig) -> WeatherQuery {
    // The weather's own place, else the prayer times', else the shared one.
    let (latitude, longitude) = own_location(cfg, ID).or_else(|| own_location(cfg, super::prayer::ID)).unwrap_or((cfg.location.latitude, cfg.location.longitude));
    WeatherQuery { latitude, longitude, refresh_minutes: cfg.weather.refresh_minutes }
}

pub fn location_name(cfg: &ShellConfig) -> String {
    if own_location(cfg, ID).is_none() {
        return cfg.location.name.clone();
    }
    // An empty name (a cleared Studio field) falls back like a missing one.
    Some(cfg.plugins.setting(ID, "location_name", String::new())).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| mandara_config::desktop::defaults::PLACE.to_string())
}

#[derive(Default)]
struct LastQuery(Option<WeatherQuery>);
impl Global for LastQuery {}

pub fn last_query(cx: &App) -> Option<WeatherQuery> {
    cx.try_global::<LastQuery>().and_then(|q| q.0)
}

pub fn remember_query(q: WeatherQuery, cx: &mut App) {
    cx.set_global(LastQuery(Some(q)));
}

/// Forgets the query (the service was stopped).
pub fn forget_query(cx: &mut App) {
    cx.set_global(LastQuery(None));
}

// ── Scene ────────────────────────────────────────────────────────────────

fn clock() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

thread_local! {
    /// Views with a scene frame pending.
    static PENDING: RefCell<HashSet<EntityId>> = RefCell::new(HashSet::new());
}

/// Re-renders the view painting a scene one scene frame from now
/// (coalesced per view). Only called while a scene is being painted, so
/// hidden scenes cost nothing, and only that view redraws.
fn schedule_frame(window: &mut Window, cx: &mut App) {
    let view = window.current_view();
    if !PENDING.with(|p| p.borrow_mut().insert(view)) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SCENE_FRAME).await;
        PENDING.with(|p| p.borrow_mut().remove(&view));
        cx.update(|cx| cx.notify(view));
    })
    .detach();
}

/// Deterministic pseudo-random in `0..1` for particle `i`, channel `k`.
fn rand(i: u32, k: u32) -> f32 {
    let mut x = i.wrapping_mul(HASH_A) ^ k.wrapping_mul(HASH_B);
    x ^= x >> HASH_SHIFT_A;
    x = x.wrapping_mul(HASH_C);
    x ^= x >> HASH_SHIFT_B;
    (x % HASH_RESOLUTION) as f32 / HASH_RESOLUTION as f32
}

/// The scene's fixed inks, parsed from hex once instead of per particle
/// per frame.
struct Ink {
    white: Color,
    cloud_day: Color,
    cloud_night: Color,
    dark_day: Color,
    dark_night: Color,
    thunder: Color,
    shadow: Color,
    sun_warm: Color,
    sun_core: (Color, Color),
    moon: Color,
    drop: Color,
    flash: Color,
    bolt: Color,
}

fn ink() -> &'static Ink {
    static INK: OnceLock<Ink> = OnceLock::new();
    INK.get_or_init(|| Ink {
        white: Color::hex("#ffffff"),
        cloud_day: Color::hex("#ffffff"),
        cloud_night: Color::hex("#c9cfdf"),
        dark_day: Color::hex("#c9d1dc"),
        dark_night: Color::hex("#5a6075"),
        thunder: Color::hex("#6c6787"),
        shadow: Color::hex("#0b1020"),
        sun_warm: Color::hex("#ffd36b"),
        sun_core: (Color::hex("#fff1b8"), Color::hex("#ffb547")),
        moon: Color::hex("#f2ecd9"),
        drop: Color::hex("#d8e8ff"),
        flash: Color::hex("#e9e6ff"),
        bolt: Color::hex("#fff6c9"),
    })
}

/// `(top, bottom)` sky colors, tinted toward the theme's primary.
fn sky(condition: Condition, day: bool, palette: &MandaraPalette) -> (Color, Color) {
    let (a, b) = match (condition, day) {
        (Condition::Clear, true) => ("#2f86d6", "#8fcaf0"),
        (Condition::Clear, false) => ("#0b1230", "#2b2662"),
        (Condition::PartlyCloudy, true) => ("#4a8bc6", "#a8cae6"),
        (Condition::PartlyCloudy, false) => ("#121937", "#363c66"),
        (Condition::Cloudy, true) => ("#66788d", "#a6b2c0"),
        (Condition::Cloudy, false) => ("#1a2030", "#3a4151"),
        (Condition::Fog, true) => ("#878f9c", "#c6cad1"),
        (Condition::Fog, false) => ("#252933", "#4b505b"),
        (Condition::Drizzle | Condition::Rain, true) => ("#465a71", "#7e90a4"),
        (Condition::Drizzle | Condition::Rain, false) => ("#121926", "#2e3848"),
        (Condition::Thunder, _) => ("#24213a", "#4a4365"),
        (Condition::Snow, true) => ("#8aa6c3", "#dce7f1"),
        (Condition::Snow, false) => ("#1c293c", "#45576f"),
    };
    (Color::hex(a).mix(palette.primary, SKY_TINT.0), Color::hex(b).mix(palette.primary, SKY_TINT.1))
}

fn circle(window: &mut Window, x: f32, y: f32, r: f32, color: Color) {
    window.paint_quad(fill(Bounds::new(point(px(x - r), px(y - r)), size(px(r * 2.0), px(r * 2.0))), color.hsla()).corner_radii(px(r)));
}

fn line(window: &mut Window, from: (f32, f32), to: (f32, f32), width: f32, color: Color) {
    let mut b = PathBuilder::stroke(px(width));
    b.move_to(point(px(from.0), px(from.1)));
    b.line_to(point(px(to.0), px(to.1)));
    if let Ok(path) = b.build() {
        window.paint_path(path, color.hsla());
    }
}

/// A soft cloud of overlapping lobes on a rounded base, with a faint
/// shadow underneath for depth.
fn cloud(window: &mut Window, x: f32, y: f32, s: f32, color: Color) {
    let shape = |window: &mut Window, dy: f32, c: Color| {
        let (bx, by, bw, bh, br) = CLOUD_BASE;
        window.paint_quad(fill(Bounds::new(point(px(x + bx * s), px(y + by * s + dy)), size(px(bw * s), px(bh * s))), c.hsla()).corner_radii(px(br * s)));
        for (lx, ly, lr) in CLOUD_LOBES {
            circle(window, x + lx * s, y + ly * s + dy, lr * s, c);
        }
    };
    shape(window, CLOUD_SHADOW_DY * s, ink().shadow.with_alpha(color.alpha() * CLOUD_SHADOW_ALPHA));
    shape(window, 0.0, color);
}

fn sun(window: &mut Window, x: f32, y: f32, r: f32, t: f32) {
    let ink = ink();
    let warm = ink.sun_warm;
    circle(window, x, y, r * SUN_HALO_OUTER, warm.with_alpha(SUN_HALO_ALPHA + SUN_HALO_PULSE.0 * (t * SUN_HALO_PULSE.1).sin()));
    circle(window, x, y, r * SUN_HALO_INNER, warm.with_alpha(SUN_HALO_INNER_ALPHA));
    let ray = warm.with_alpha(SUN_RAY_ALPHA);
    for i in 0..SUN_RAYS {
        let a = t * SUN_RAY_SPIN + i as f32 * TAU / SUN_RAYS as f32;
        let reach = r * (SUN_RAY_REACH + SUN_RAY_FLICKER.0 * (t * SUN_RAY_FLICKER.1 + i as f32).sin());
        line(window, (x + a.cos() * r * SUN_RAY_START, y + a.sin() * r * SUN_RAY_START), (x + a.cos() * reach, y + a.sin() * reach), r * SUN_RAY_WIDTH, ray);
    }
    window.paint_quad(
        fill(
            Bounds::new(point(px(x - r), px(y - r)), size(px(r * 2.0), px(r * 2.0))),
            linear_gradient(ANGLE_DIAGONAL_DEG, linear_color_stop(ink.sun_core.0.hsla(), 0.0), linear_color_stop(ink.sun_core.1.hsla(), 1.0)),
        )
        .corner_radii(px(r)),
    );
}

fn moon(window: &mut Window, x: f32, y: f32, r: f32, sky_top: Color, t: f32) {
    let pale = ink().moon;
    circle(window, x, y, r * MOON_HALO_OUTER, pale.with_alpha(MOON_HALO_ALPHA + MOON_HALO_PULSE.0 * (t * MOON_HALO_PULSE.1).sin()));
    circle(window, x, y, r * MOON_HALO_INNER, pale.with_alpha(MOON_HALO_INNER_ALPHA));
    circle(window, x, y, r, pale);
    // Crescent: the sky's colour bites out the upper-right.
    let (bx, by, br) = MOON_BITE;
    circle(window, x + r * bx, y + r * by, r * br, sky_top);
}

fn stars(window: &mut Window, b: (f32, f32, f32, f32), t: f32, count: u32) {
    let (ox, oy, w, h) = b;
    let white = ink().white;
    for i in 0..count {
        let x = ox + rand(i, 1) * w;
        let y = oy + rand(i, 2) * h * STAR_BAND;
        let tw = 0.5 + 0.5 * (t * (STAR_TWINKLE.0 + rand(i, 3) * STAR_TWINKLE.1) + rand(i, 4) * TAU).sin();
        circle(window, x, y, STAR_R_PX.0 + rand(i, 5) * STAR_R_PX.1, white.with_alpha(STAR_ALPHA.0 + STAR_ALPHA.1 * tw));
    }
}

fn precipitation(window: &mut Window, b: (f32, f32, f32, f32), t: f32, count: u32, heavy: bool) {
    let (ox, oy, w, h) = b;
    let drop = ink().drop;
    for i in 0..count {
        let speed = if heavy { DROP_SPEED_PX_S.0 } else { DROP_SPEED_PX_S.1 } + rand(i, 1) * DROP_SPEED_PX_S.2;
        let len = if heavy { DROP_LEN_PX.0 } else { DROP_LEN_PX.1 } + rand(i, 2) * DROP_LEN_PX.2;
        let span = h + len;
        let y = oy + (rand(i, 3) * span + t * speed) % span - len;
        let x = ox + rand(i, 4) * (w + DROP_MARGIN_PX * 2.0) - DROP_MARGIN_PX - (y - oy) * DROP_DRIFT;
        let stroke = if heavy { DROP_STROKE_PX.0 } else { DROP_STROKE_PX.1 };
        line(window, (x, y), (x - len * DROP_SLANT, y + len), stroke, drop.with_alpha(DROP_ALPHA.0 + rand(i, 5) * DROP_ALPHA.1));
    }
}

fn snow(window: &mut Window, b: (f32, f32, f32, f32), t: f32, count: u32) {
    let (ox, oy, w, h) = b;
    let white = ink().white;
    for i in 0..count {
        let speed = FLAKE_SPEED_PX_S.0 + rand(i, 1) * FLAKE_SPEED_PX_S.1;
        let y = oy + (rand(i, 2) * h + t * speed) % h;
        let x = ox + rand(i, 3) * w + (t * (FLAKE_SWAY_RATE + rand(i, 4)) + rand(i, 5) * TAU).sin() * FLAKE_SWAY_PX;
        circle(window, x, y, FLAKE_R_PX.0 + rand(i, 6) * FLAKE_R_PX.1, white.with_alpha(FLAKE_ALPHA.0 + rand(i, 7) * FLAKE_ALPHA.1));
    }
}

fn fog(window: &mut Window, b: (f32, f32, f32, f32), t: f32) {
    let (ox, oy, w, h) = b;
    let white = ink().white;
    for i in 0..FOG_BANKS {
        let y = oy + h * (FOG_TOP + i as f32 * FOG_SPACING);
        let x = ox - w * FOG_LEFT + (t * (FOG_SWAY_RATE.0 + i as f32 * FOG_SWAY_RATE.1) + i as f32).sin() * FOG_SWAY_PX;
        let bh = FOG_H_PX.0 + rand(i, 1) * FOG_H_PX.1;
        window.paint_quad(
            fill(Bounds::new(point(px(x), px(y)), size(px(w * FOG_WIDTH), px(bh))), white.with_alpha(FOG_ALPHA.0 + rand(i, 2) * FOG_ALPHA.1).hsla()).corner_radii(px(bh / 2.0)),
        );
    }
}

/// Lightning: a flash every few seconds with a jagged bolt.
fn lightning(window: &mut Window, b: (f32, f32, f32, f32), t: f32) {
    let (ox, oy, w, h) = b;
    let phase = t % BOLT_PERIOD_S;
    // Whole periods elapsed: small and non-negative, so the cast is exact.
    let strike = (t / BOLT_PERIOD_S).floor().max(0.0) as u32;
    if phase > BOLT_FLASH_S {
        return;
    }
    let ink = ink();
    let flash = (1.0 - phase / BOLT_FLASH_S).powi(2) * if phase > BOLT_GAP_S.0 && phase < BOLT_GAP_S.1 { BOLT_GAP_DIM } else { 1.0 };
    window.paint_quad(fill(Bounds::new(point(px(ox), px(oy)), size(px(w), px(h))), ink.flash.with_alpha(BOLT_SKY_ALPHA * flash).hsla()));
    let mut x = ox + w * (BOLT_X.0 + rand(strike, 1) * BOLT_X.1);
    let mut y = oy + h * BOLT_TOP;
    let mut b = PathBuilder::stroke(px(BOLT_STROKE_PX));
    b.move_to(point(px(x), px(y)));
    // Channels past the particle channels pick the bolt's segments.
    const SEGMENT_CHANNEL: u32 = 10;
    for k in 0..BOLT_SEGMENTS {
        x += (rand(strike, SEGMENT_CHANNEL + k) - 0.5) * BOLT_JITTER_PX;
        y += h * BOLT_SEGMENT_H;
        b.line_to(point(px(x), px(y)));
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, ink.bolt.with_alpha(BOLT_ALPHA * flash).hsla());
    }
}

/// A cloud placed relative to the celestial anchor: `(bob phase, dx, dy,
/// size)` in art px.
fn anchored_cloud(window: &mut Window, anchor: (f32, f32), (phase, dx, dy, side): (f32, f32, f32, f32), s: f32, bob: impl Fn(f32) -> f32, color: Color) {
    cloud(window, anchor.0 + dx * s + bob(phase), anchor.1 + dy * s, side * s, color);
}

/// The animated sky for a condition, filling its parent.
fn scene(condition: Condition, day: bool, palette: &MandaraPalette, animate: bool, art_scale: f32) -> impl IntoElement {
    let (top, bottom) = sky(condition, day, palette);
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, cx| {
            let t = if animate { clock() } else { STILL_TIME_S };
            let (ox, oy) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let area = (ox, oy, w, h);
            let s = art_scale;
            let ink = ink();
            window.with_content_mask(Some(ContentMask::rect(bounds)), |window| {
                let clouds = if day { ink.cloud_day } else { ink.cloud_night };
                let dark = if day { ink.dark_day } else { ink.dark_night };
                let bob = |i: f32| (t * BOB_RATE + i).sin() * BOB_PX;
                let drift = |i: f32, speed: f32| ox + (t * speed + i * w / DRIFT_SPACING) % (w + DRIFT_MARGIN_PX * 2.0 * s) - DRIFT_MARGIN_PX * s;
                let drifting = |window: &mut Window, (phase, speed, y, side): (f32, f32, f32, f32), color: Color| {
                    cloud(window, drift(phase, speed), oy + h * y, side * s, color);
                };
                // Celestial body sits upper-right, clear of the temperature text.
                let anchor = (ox + w * BODY_X_FRAC, oy + h * BODY_Y_FRAC);
                match condition {
                    Condition::Clear => {
                        if day {
                            sun(window, anchor.0, anchor.1, CLEAR_SUN_R_PX * s, t);
                        } else {
                            stars(window, area, t, CLEAR_STARS);
                            moon(window, anchor.0, anchor.1, CLEAR_MOON_R_PX * s, top, t);
                        }
                    }
                    Condition::PartlyCloudy => {
                        let (bx, by) = (anchor.0 + PARTLY_BODY_DX_PX * s, anchor.1 + PARTLY_BODY_DY_PX * s);
                        if day {
                            sun(window, bx, by, PARTLY_SUN_R_PX * s, t);
                        } else {
                            stars(window, area, t, PARTLY_STARS);
                            moon(window, bx, by, PARTLY_MOON_R_PX * s, top, t);
                        }
                        let (dx, dy, side) = PARTLY_CLOUD;
                        anchored_cloud(window, anchor, (0.0, dx, dy, side), s, bob, clouds.with_alpha(PARTLY_CLOUD_ALPHA));
                        drifting(window, PARTLY_DRIFT, clouds.with_alpha(PARTLY_DRIFT_ALPHA));
                    }
                    Condition::Cloudy | Condition::Fog => {
                        drifting(window, OVERCAST_DRIFT, dark.with_alpha(OVERCAST_DRIFT_ALPHA));
                        anchored_cloud(window, anchor, OVERCAST_BACK, s, bob, dark.with_alpha(OVERCAST_ALPHA));
                        anchored_cloud(window, anchor, OVERCAST_FRONT, s, bob, clouds.with_alpha(OVERCAST_ALPHA));
                        if condition == Condition::Fog {
                            fog(window, area, t);
                        }
                    }
                    Condition::Drizzle | Condition::Rain | Condition::Thunder => {
                        let heavy = condition != Condition::Drizzle;
                        precipitation(window, area, t, if heavy { RAIN_DROPS_HEAVY } else { RAIN_DROPS_LIGHT }, heavy);
                        let tone = if condition == Condition::Thunder { ink.thunder } else { dark };
                        drifting(window, RAIN_DRIFT, tone.with_alpha(RAIN_DRIFT_ALPHA));
                        anchored_cloud(window, anchor, RAIN_BACK, s, bob, tone.with_alpha(RAIN_CLOUD_ALPHA));
                        anchored_cloud(window, anchor, RAIN_FRONT, s, bob, tone.lighten(RAIN_FRONT_LIFT).with_alpha(RAIN_CLOUD_ALPHA));
                        if condition == Condition::Thunder {
                            lightning(window, area, t);
                        }
                    }
                    Condition::Snow => {
                        snow(window, area, t, SNOW_FLAKES);
                        let (dx, dy, side) = SNOW_CLOUD;
                        anchored_cloud(window, anchor, (0.0, dx, dy, side), s, bob, clouds.with_alpha(SNOW_CLOUD_ALPHA));
                    }
                }
            });
            if animate {
                schedule_frame(window, cx);
            }
        },
    )
    .absolute()
    .inset_0()
    .rounded(px(crate::style::radius(CARD_RADIUS_PX)))
    .bg(linear_gradient(ANGLE_DOWN_DEG, linear_color_stop(top.hsla(), 0.0), linear_color_stop(bottom.hsla(), 1.0)))
}

// ── Pieces ───────────────────────────────────────────────────────────────

pub fn condition_icon(condition: Condition, day: bool) -> &'static str {
    match condition {
        Condition::Clear if day => "sun",
        Condition::Clear => "moon",
        Condition::PartlyCloudy if day => "cloud-sun",
        Condition::PartlyCloudy => "cloud-moon",
        Condition::Cloudy => "cloud",
        Condition::Fog => "fog",
        Condition::Drizzle | Condition::Rain => "rain",
        Condition::Snow => "snow",
        Condition::Thunder => "bolt",
    }
}

fn icon_color(condition: Condition, day: bool, palette: &MandaraPalette) -> Color {
    match condition {
        Condition::Clear | Condition::PartlyCloudy if day => palette.warning_amber,
        Condition::Thunder => palette.warning_amber,
        Condition::Drizzle | Condition::Rain => palette.primary,
        _ => palette.on_surface_variant,
    }
}

/// A Celsius reading as a compact `23°` in the user's units.
fn deg(units: Units, celsius: f32) -> SharedString {
    units.format_degrees(celsius).into()
}

/// A km/h wind speed in the user's units.
fn wind(units: Units, kmh: f32, unit: mandara_config::desktop::WindUnit) -> String {
    if let Some((v, u)) = unit.convert(kmh) {
        return format!("{v:.0} {u}");
    }
    let (v, _) = units.speed(kmh);
    let v = format!("{v:.0}");
    match units {
        Units::Metric => mandara_i18n::tf("{speed} km/h", &[("speed", &v)]),
        Units::Imperial => mandara_i18n::tf("{speed} mph", &[("speed", &v)]),
    }
}

fn weekday(date: &str, index: usize) -> String {
    if index == 0 {
        return mandara_i18n::t("Today").into();
    }
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map(|d| crate::widgets::clock::localized(&d, "%a")).unwrap_or_else(|_| date.to_string())
}

fn precip_text(pct: u8) -> Option<SharedString> {
    (pct >= PRECIP_SHOWN_PCT).then(|| format!("{pct}%").into())
}

struct HourLabels {
    time: SharedString,
    temp: SharedString,
    precip: Option<SharedString>,
}

struct DayLabels {
    weekday: SharedString,
    precip: Option<SharedString>,
    min: SharedString,
    max: SharedString,
}

/// Everything the weather views print, formatted once per weather update,
/// units / language / config change — the scene re-renders its view at
/// 30 fps, which must not re-format (or re-parse dates) every frame.
struct Labels {
    key: (i64, *const ShellConfig, Lang),
    /// Keeps the keyed config alive so its address can't be reused.
    _config: Arc<ShellConfig>,
    animate: bool,
    place: SharedString,
    temp: SharedString,
    hi_lo: Option<SharedString>,
    feels: SharedString,
    humidity: SharedString,
    wind: SharedString,
    sun_time: SharedString,
    hours: Vec<HourLabels>,
    days: Vec<DayLabels>,
}

type SharedLabels = Rc<RefCell<Option<Labels>>>;

/// The labels for `w` under `cfg`, rebuilt only when either changed.
fn labels<'a>(cache: &'a mut Option<Labels>, w: &Weather, cfg: &Arc<ShellConfig>) -> &'a Labels {
    let key = (w.fetched_at, Arc::as_ptr(cfg), mandara_i18n::language());
    if cache.as_ref().is_none_or(|l| l.key != key) {
        *cache = Some(build_labels(key, w, cfg));
    }
    cache.get_or_insert_with(|| build_labels(key, w, cfg))
}

fn build_labels(key: (i64, *const ShellConfig, Lang), w: &Weather, cfg: &Arc<ShellConfig>) -> Labels {
    let units = cfg.units();
    let hours = w
        .hours
        .iter()
        .take(PANEL_HOURS.max(SIDEBAR_HOURS))
        .enumerate()
        .map(|(i, h)| HourLabels {
            time: if i == 0 { mandara_i18n::t("Now").into() } else { format!("{:02}", h.hour).into() },
            temp: deg(units, h.temp),
            precip: precip_text(h.precip),
        })
        .collect();
    let days = w
        .days
        .iter()
        .enumerate()
        .map(|(i, d)| DayLabels { weekday: weekday(&d.date, i).into(), precip: precip_text(d.precip), min: deg(units, d.min), max: deg(units, d.max) })
        .collect();
    Labels {
        key,
        _config: cfg.clone(),
        animate: !cfg.motion.reduce_motion && cfg.plugins.setting(ID, "animate", true),
        place: location_name(cfg).into(),
        temp: deg(units, w.temp),
        hi_lo: w.days.first().map(|d| mandara_i18n::tf("H {high}  L {low}", &[("high", &deg(units, d.max)), ("low", &deg(units, d.min))]).into()),
        feels: deg(units, w.feels_like),
        humidity: format!("{}%", w.humidity).into(),
        wind: wind(units, w.wind, cfg.weather.wind_unit).into(),
        sun_time: if w.is_day { w.sunset.clone() } else { w.sunrise.clone() }.into(),
        hours,
        days,
    }
}

/// The hero: animated sky with temperature, condition and place.
fn hero(w: &Weather, l: &Labels, palette: &MandaraPalette, height: f32, big: f32, display: SharedString) -> AnyElement {
    div()
        .relative()
        .w_full()
        .h(px(height))
        .rounded(px(crate::style::radius(CARD_RADIUS_PX)))
        .overflow_hidden()
        .child(scene(w.condition, w.is_day, palette, l.animate, height / ART_BASE_H_PX))
        .child(
            div()
                .absolute()
                .left(px(HERO_INSET_X_PX))
                .top(px(HERO_INSET_Y_PX))
                .bottom(px(HERO_INSET_Y_PX))
                .flex()
                .flex_col()
                .justify_between()
                .text_color(gpui::white())
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).opacity(HERO_PLACE_OPACITY).child(l.place.clone()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_family(display)
                                .text_size(px(big))
                                .line_height(px(big * HERO_TEMP_LINE))
                                .font_weight(FontWeight::MEDIUM)
                                .font_features(tabular_nums())
                                .child(l.temp.clone()),
                        )
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(w.condition.label()))
                        .when_some(l.hi_lo.clone(), |el, range| {
                            el.child(div().text_size(px(text::BODY_S)).opacity(HERO_RANGE_OPACITY).font_features(tabular_nums()).child(range))
                        }),
                ),
        )
        .into_any_element()
}

fn stat(icon_name: &'static str, label: &'static str, value: SharedString, palette: &MandaraPalette) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(0.0))
        .px(px(STAT_PAD_X_PX))
        .py(px(STAT_PAD_Y_PX))
        .rounded(px(crate::style::radius(STAT_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .flex_col()
        .gap(px(STAT_LINE_GAP_PX))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(STAT_ICON_GAP_PX))
                .child(icon(icon_name, SMALL_ICON_PX, palette.on_surface_variant))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(label)),
        )
        .child(div().font_weight(FontWeight::MEDIUM).font_features(tabular_nums()).child(value))
}

fn stats(w: &Weather, l: &Labels, palette: &MandaraPalette) -> impl IntoElement {
    div()
        .flex()
        .gap(px(STATS_GAP_PX))
        .child(stat("thermometer", mandara_i18n::t("Feels"), l.feels.clone(), palette))
        .child(stat("droplet", mandara_i18n::t("Humidity"), l.humidity.clone(), palette))
        .child(stat("wind", mandara_i18n::t("Wind"), l.wind.clone(), palette))
        .child(stat("sunset", if w.is_day { mandara_i18n::t("Sunset") } else { mandara_i18n::t("Sunrise") }, l.sun_time.clone(), palette))
}

fn hourly(hours: &[Hour], labels: &[HourLabels], count: usize, palette: &MandaraPalette) -> impl IntoElement {
    // Reserve the precipitation line only when some hour needs it.
    let wet = labels.iter().take(count).any(|l| l.precip.is_some());
    div().flex().justify_between().children(hours.iter().zip(labels).take(count).map(move |(h, l)| {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(HOUR_GAP_PX))
            .w(px(HOUR_W_PX))
            .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).font_features(tabular_nums()).child(l.time.clone()))
            .child(icon(condition_icon(h.condition, h.is_day), HOUR_ICON_PX, icon_color(h.condition, h.is_day, palette)))
            .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).font_features(tabular_nums()).child(l.temp.clone()))
            .when(wet, |el| el.child(div().h(px(PRECIP_LINE_H_PX)).text_size(px(PRECIP_TEXT_PX)).text_color(palette.primary.hsla()).children(l.precip.clone())))
    }))
}

/// 5-day rows with a temperature-range bar on a shared week scale, the
/// fill blended cool → warm in OKLCH.
fn daily(days: &[Day], labels: &[DayLabels], palette: &MandaraPalette) -> impl IntoElement {
    let lo = days.iter().map(|d| d.min).fold(f32::INFINITY, f32::min);
    let hi = days.iter().map(|d| d.max).fold(f32::NEG_INFINITY, f32::max);
    let span = (hi - lo).max(1.0);
    let cool = Color::hex(RANGE_COOL).mix(palette.primary, RANGE_COOL_TINT);
    let warm = Color::hex(RANGE_WARM).mix(palette.primary, RANGE_WARM_TINT);
    let track = palette.on_surface.with_alpha(RANGE_TRACK_ALPHA);
    div().flex().flex_col().gap(px(DAY_ROW_GAP_PX)).children(days.iter().zip(labels).enumerate().map(move |(i, (d, l))| {
        let (a, b) = ((d.min - lo) / span, (d.max - lo) / span);
        div()
            .h(px(DAY_H_PX))
            .flex()
            .items_center()
            .gap(px(DAY_COL_GAP_PX))
            .child(div().w(px(DAY_NAME_W_PX)).font_weight(if i == 0 { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(l.weekday.clone()))
            .child(icon(condition_icon(d.condition, true), DAY_ICON_PX, icon_color(d.condition, true, palette)))
            .child(div().w(px(DAY_PRECIP_W_PX)).text_size(px(DAY_PRECIP_TEXT_PX)).text_color(palette.primary.hsla()).children(l.precip.clone()))
            .child(div().w(px(DAY_TEMP_W_PX)).text_right().text_color(palette.on_surface_variant.hsla()).font_features(tabular_nums()).child(l.min.clone()))
            .child(
                div().relative().w(px(RANGE_TRACK_W_PX)).h(px(RANGE_TRACK_H_PX)).rounded_full().bg(track.hsla()).child(
                    div()
                        .absolute()
                        .left(px(RANGE_TRACK_W_PX * a))
                        .w(px((RANGE_TRACK_W_PX * (b - a)).max(RANGE_MIN_FILL_PX)))
                        .h_full()
                        .rounded_full()
                        .bg(linear_gradient(ANGLE_RIGHT_DEG, linear_color_stop(cool.mix(warm, a).hsla(), 0.0), linear_color_stop(cool.mix(warm, b).hsla(), 1.0))),
                ),
            )
            .child(div().w(px(DAY_TEMP_W_PX)).font_weight(FontWeight::MEDIUM).font_features(tabular_nums()).child(l.max.clone()))
    }))
}

fn placeholder(state: &WeatherState, palette: &MandaraPalette, height: f32) -> AnyElement {
    let msg: SharedString = if state.loading {
        mandara_i18n::t("Fetching the weather…").into()
    } else {
        state.error.as_deref().map_or_else(|| mandara_i18n::t("Weather unavailable").into(), |e| mandara_i18n::tr(e).into())
    };
    div()
        .h(px(height))
        .rounded(px(crate::style::radius(CARD_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(PLACEHOLDER_GAP_PX))
        .child(icon("cloud", PLACEHOLDER_ICON_PX, palette.on_surface_variant.with_alpha(PLACEHOLDER_ICON_ALPHA)))
        .child(div().px(px(PLACEHOLDER_PAD_X_PX)).text_center().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(msg))
        .into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, mandara_i18n::tk("Weather"), mandara_i18n::tk("Animated current conditions, hourly and 5-day forecast"), Some(Zone::Right)),
        |window, cx, panel_cx| {
            let cache: SharedLabels = Rc::default();
            let bar_cache = cache.clone();
            let bar = fn_view(
                None,
                move |palette, _, cx| {
                    let cx: &App = cx;
                    let Some(w) = hub(cx).read(cx).weather.weather.as_ref() else {
                        return div().flex().items_center().child(icon("cloud", crate::style::bar_icon(CHIP_ICON_PX), palette.on_surface_variant)).into_any_element();
                    };
                    let cfg = config(cx).read(cx).snapshot();
                    let mut cache = bar_cache.borrow_mut();
                    let l = labels(&mut cache, w, &cfg);
                    div()
                        .flex()
                        .items_center()
                        .gap(px(CHIP_GAP_PX))
                        .child(icon(condition_icon(w.condition, w.is_day), crate::style::bar_icon(CHIP_ICON_PX), icon_color(w.condition, w.is_day, palette)))
                        .child(div().font_features(tabular_nums()).child(l.temp.clone()))
                        .when_some(l.hi_lo.clone().filter(|_| cfg.weather.show_high_low_in_bar), |el, hl| {
                            el.child(div().font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(hl))
                        })
                        .when(cfg.weather.show_location_in_bar, |el| el.child(div().text_color(palette.on_surface_variant.hsla()).child(l.place.clone())))
                        .into_any_element()
                },
                window,
                cx,
            );
            let pcx: PanelCx = panel_cx.clone();
            let panel_cache = cache.clone();
            let panel = fn_view(
                None,
                move |palette, _, cx| {
                    let cx: &App = cx;
                    let state = &hub(cx).read(cx).weather;
                    let mut col = div().size_full().p(px(tokens::LG)).flex().flex_col().gap(px(tokens::MD));
                    match &state.weather {
                        Some(w) => {
                            let cfg = config(cx).read(cx).snapshot();
                            let mut cache = panel_cache.borrow_mut();
                            let l = labels(&mut cache, w, &cfg);
                            let display = fonts(cx).display.clone();
                            col = col
                                .child(reveal(&pcx, REVEAL_HERO, hero(w, l, palette, PANEL_HERO_H_PX, PANEL_HERO_TEMP_PX, display)))
                                .child(reveal(&pcx, REVEAL_STATS, stats(w, l, palette)))
                                .child(reveal(&pcx, REVEAL_HOURLY, card(palette).py(px(HOURLY_PAD_Y_PX)).child(hourly(&w.hours, &l.hours, PANEL_HOURS, palette))))
                                .child(reveal(&pcx, REVEAL_DAILY, card(palette).py(px(DAILY_PAD_Y_PX)).child(daily(&w.days, &l.days, palette))));
                        }
                        None => col = col.child(placeholder(state, palette, PANEL_HERO_H_PX)),
                    }
                    col.into_any_element()
                },
                window,
                cx,
            );
            let side_cache = cache;
            let sidebar = fn_view(
                None,
                move |palette, _, cx| {
                    let cx: &App = cx;
                    let state = &hub(cx).read(cx).weather;
                    match &state.weather {
                        Some(w) => {
                            let cfg = config(cx).read(cx).snapshot();
                            let mut cache = side_cache.borrow_mut();
                            let l = labels(&mut cache, w, &cfg);
                            let display = fonts(cx).display.clone();
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(tokens::SM))
                                .child(hero(w, l, palette, SIDEBAR_HERO_H_PX, SIDEBAR_HERO_TEMP_PX, display))
                                .child(card(palette).py(px(SIDEBAR_HOURLY_PAD_Y_PX)).child(hourly(&w.hours, &l.hours, SIDEBAR_HOURS, palette)))
                                .into_any_element()
                        }
                        None => placeholder(state, palette, SIDEBAR_PLACEHOLDER_H_PX),
                    }
                },
                window,
                cx,
            );
            PluginInstance {
                bar: Some(bar),
                panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX)))) }),
                sidebar: Some(sidebar),
                ..PluginInstance::empty()
            }
        },
    );
    r.command(
        ID,
        CommandSpec::new("weather", mandara_i18n::tk("Current conditions and forecast as JSON"), |_, cx| {
            let hub = hub(cx).read(cx);
            if hub.weather_service.is_none() {
                return Err("the weather plugin isn't placed in the bar or sidebar".into());
            }
            // Values stay metric (°C, km/h); `units` is the display preference.
            let cfg = config(cx).read(cx).get();
            Ok(json!({ "location": location_name(cfg), "units": cfg.units(), "temperature_unit": "celsius", "wind_unit": "km/h", "state": hub.weather }))
        })
        .example("weather"),
    );
    r.command(
        ID,
        CommandSpec::new_output("weather refresh", mandara_i18n::tk("Fetch fresh weather now"), |_, cx| {
            let service = hub(cx).read(cx).weather_service.clone().ok_or("the weather plugin isn't placed in the bar or sidebar")?;
            Ok(mandara_plugins::Output::Pending(service.request(WeatherCommand::Refresh)))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_location_is_the_default_and_a_plugins_own_place_overrides_it() {
        let mut cfg = ShellConfig::default();
        cfg.location.latitude = 52.52;
        cfg.location.longitude = 13.40;
        cfg.location.name = "Berlin".into();
        // No place of its own: the weather follows the shared location,
        // even with "Use one location everywhere" off.
        assert!(!cfg.location.shared);
        assert_eq!((query(&cfg).latitude, query(&cfg).longitude), (52.52, 13.40));
        assert_eq!(location_name(&cfg), "Berlin");
        // Its own place wins…
        let own = toml::toml! { latitude = 31.2
        longitude = 29.92
        location_name = "Alexandria" };
        cfg.plugins.settings.insert(ID.into(), own);
        assert_eq!((query(&cfg).latitude, query(&cfg).longitude), (31.2, 29.92));
        assert_eq!(location_name(&cfg), "Alexandria");
        // …unless one location is forced everywhere.
        cfg.location.shared = true;
        assert_eq!((query(&cfg).latitude, query(&cfg).longitude), (52.52, 13.40));
        assert_eq!(location_name(&cfg), "Berlin");
        // Half a place (only a latitude) isn't a place.
        cfg.location.shared = false;
        cfg.plugins.settings.insert(ID.into(), toml::toml! { latitude = 1.0 });
        assert_eq!(own_location(&cfg, ID), None);
    }

    #[test]
    fn particle_noise_is_deterministic_and_in_range() {
        for i in 0..64 {
            for k in 0..8 {
                let v = rand(i, k);
                assert!((0.0..1.0).contains(&v));
                assert_eq!(v, rand(i, k));
            }
        }
    }

    #[test]
    fn precipitation_shows_from_threshold() {
        assert!(precip_text(PRECIP_SHOWN_PCT - 1).is_none());
        assert_eq!(precip_text(PRECIP_SHOWN_PCT).as_deref(), Some("20%"));
    }
}
