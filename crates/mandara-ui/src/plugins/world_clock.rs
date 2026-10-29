//! World clock plugin: city codes and times in the bar; a panel listing
//! each city's local time, its offset from you, sunrise / sunset and a
//! 24-hour daylight strip, with a meeting planner that slides every clock
//! together (working hours highlighted per city) and a city search over the
//! curated list (English + Arabic names) and the whole tz database.
//!
//! Everything shown changes at most once a minute (or when the planner
//! moves), so the per-city rows — names, clocks, offsets, sunrise / sunset
//! and the 48-segment sky strip — are computed once per displayed minute and
//! reused by every frame of the panel's open animation. Resolved cities,
//! search results and the local zone id are cached the same way.
//!
//! Settings (`[plugins.settings.world-clock]`): `cities` (comma-separated
//! names or zone ids), `clock` (`shell` / `12` / `24`), `work_start`,
//! `work_end` (hours), `bar_cities` (how many show in the bar).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Offset as _, TimeZone as _, Timelike, Utc};
use gpui::{
    AnyElement, App, AppContext as _, Bounds, Entity, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Task, Window, canvas, div, prelude::FluentBuilder as _, px, size,
};
use mandara_config::ShellConfig;
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::world_clock::{self as wc, Daylight, Place, Sky};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{CONTROL_H, caption, search_box};
use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_RIGHT_DEG, HEADER_H, PanelShape, SECTION_H_PX, ViewSet, WASH_SOFT, chip, empty_state, glass_card, glow, header, partner,
    pill_button, readable, redraw_in, section, wash,
};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::{dot, icon};
use crate::kit::{icon_button, reveal, text, tokens};
use crate::state::update_config;
use crate::text_field::TextField;

pub const ID: &str = "world-clock";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 404.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const PLANNER_H: f32 = 92.0;
const ROW_H: f32 = 70.0;
const ROW_GAP: f32 = 6.0;
const RESULT_H: f32 = 40.0;
const EMPTY_H: f32 = 150.0;
/// Padding of the city card around its rows.
const CITIES_PAD_PX: f32 = 6.0;
/// Gap between a section label / the search box and what follows.
const BLOCK_GAP_PX: f32 = 6.0;
/// Small gaps inside rows (label · badge, glyph · text).
const INLINE_GAP_PX: f32 = 6.0;
/// Row padding and the gap between its glyph, text and clock.
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_PAD_Y_PX: f32 = tokens::SM;
const ROW_INNER_GAP_PX: f32 = 10.0;
/// A row's hover radius sits this far inside the card's.
const ROW_RADIUS_INSET_PX: f32 = tokens::XS;
const ROW_HOVER_ALPHA: f32 = 0.035;
/// Sky glyph tile: side, radius, glyph size and gradient alphas.
const SKY_TILE_PX: f32 = 34.0;
const SKY_TILE_RADIUS_PX: f32 = 11.0;
const SKY_GLYPH_PX: f32 = 17.0;
const SKY_TILE_FILL: (f32, f32) = (0.22, 0.10);
/// The city clock: size, line height, meridiem size / baseline nudge and the
/// gap between them.
const CLOCK_PX: f32 = 22.0;
const CLOCK_LINE_PX: f32 = 24.0;
const MERIDIEM_PX: f32 = 10.5;
const MERIDIEM_PAD_PX: f32 = 3.0;
const CLOCK_GAP_PX: f32 = 3.0;
/// "Working hours" dot.
const WORKING_DOT_PX: f32 = 5.0;
/// Remove button side (shown on hover).
const REMOVE_BTN_PX: f32 = 24.0;
/// Search results: padding, radius, glyph, hover / already-added alpha.
const RESULT_PAD_X_PX: f32 = 10.0;
const RESULT_RADIUS_PX: f32 = 10.0;
const RESULT_GAP_PX: f32 = 10.0;
const RESULT_ICON_PX: f32 = 14.0;
const RESULT_HOVER_ALPHA: f32 = 0.05;
const RESULT_ADDED_OPACITY: f32 = 0.55;
/// Bar chip: gap between cities and inside one, the code's size, the
/// fallback glyph.
const BAR_GAP_PX: f32 = 10.0;
const BAR_CITY_GAP_PX: f32 = 5.0;
const BAR_CODE_PX: f32 = 10.5;
const BAR_ICON_PX: f32 = 15.0;
/// Panel size changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;

/// Daylight strip: half-hour segments over 24 h, bar / underline heights,
/// end radius, segment gap, sky alphas and the "now" marker overhang.
const STRIP_SEGMENTS: usize = 48;
const MINUTES_PER_SEGMENT: i64 = 30;
const STRIP_H: f32 = 6.0;
const STRIP_UNDERLINE_PX: f32 = 2.0;
const STRIP_GAP_PX: f32 = 2.0;
const STRIP_RADIUS_PX: f32 = 3.0;
const STRIP_DAY_ALPHA: f32 = 0.75;
const STRIP_DUSK_ALPHA: f32 = 0.7;
const STRIP_WORK_ALPHA: f32 = 0.85;
const STRIP_NEUTRAL_ALPHA: (f32, f32) = (0.10, 0.09);
const MARKER_OVERHANG_PX: f32 = 3.0;
const MARKER_W_PX: f32 = 2.0;
/// Sky palette (OKLCH): day sun, dusk, night (dark / light theme).
const SKY_DAY: (f32, f32, f32) = (0.82, 0.13, 82.0);
const SKY_DUSK: (f32, f32, f32) = (0.68, 0.14, 38.0);
const SKY_NIGHT_DARK: (f32, f32, f32) = (0.36, 0.07, 272.0);
const SKY_NIGHT_LIGHT: (f32, f32, f32) = (0.55, 0.08, 272.0);
const SKY_NIGHT_LIGHT_ALPHA: f32 = 0.55;
/// Lightness of the moon glyph lifted out of the night colour.
const MOON_L: f32 = 0.75;

/// Meeting planner: track height, hour ticks (every 6th is major) with
/// their offsets / heights / alphas, the selection band inset and alpha,
/// the centre line, and the knob (side, inset, dot, glow).
const PLANNER_TRACK_H: f32 = 30.0;
const PLANNER_HOURS: i64 = 24;
const MAJOR_TICK_EVERY: i64 = 6;
const TICK_TOP_PX: (f32, f32) = (9.0, 12.0);
const TICK_H_PX: (f32, f32) = (12.0, 6.0);
const TICK_ALPHA: (f32, f32) = (0.22, 0.12);
const TICK_W_PX: f32 = 1.0;
const BAND_INSET_PX: f32 = tokens::XS;
const BAND_ALPHA: f32 = 0.35;
const CENTER_INSET_PX: f32 = 3.0;
const CENTER_ALPHA: f32 = 0.5;
const KNOB_PX: f32 = 20.0;
const KNOB_TOP_PX: f32 = 5.0;
const KNOB_DOT_PX: f32 = 8.0;
const KNOB_GLOW: (f32, f32) = (0.45, 10.0);
const PLANNER_ICON_PX: f32 = 13.0;

// ── Behaviour ────────────────────────────────────────────────────────────

/// Most cities on the clock.
pub const MAX_CITIES: usize = 10;
/// Search results shown under the field.
const RESULTS_SHOWN: usize = 6;
/// Minutes per hour.
const MINUTES_PER_HOUR: i64 = 60;
/// Seconds per minute.
const SECS_PER_MIN: i64 = 60;
/// Meeting planner range either side of now, and its snap step.
const PLAN_RANGE_MIN: i64 = PLANNER_HOURS / 2 * MINUTES_PER_HOUR;
const PLAN_STEP_MIN: i64 = 15;
/// The planner resets after the panel has been closed this long.
const PLAN_RESET_AFTER: Duration = Duration::from_secs(3);
/// How often the closed-panel watchdog checks whether to free the caches.
const CACHE_CHECK_EVERY: Duration = Duration::from_secs(1);
/// Default number of cities in the bar chip, and the most it shows.
const DEFAULT_BAR_CITIES: i64 = 2;
const MAX_BAR_CITIES: i64 = 4;
/// Last valid hour of the day for working hours.
const LAST_HOUR: i64 = 23;
/// One minute, and how far past the minute boundary a tick lands.
const MINUTE: Duration = Duration::from_secs(60);
const MINUTE_MARGIN: Duration = Duration::from_millis(30);
/// The Arabic Unicode block (secondary names in it get the Arabic font).
const ARABIC_BLOCK: std::ops::RangeInclusive<char> = '\u{0600}'..='\u{06FF}';

// ── Settings ─────────────────────────────────────────────────────────────

/// The configured city entries (comma-separated string or array).
pub fn city_entries(cfg: &ShellConfig) -> Vec<String> {
    let parse = |s: &str| s.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>();
    match cfg.plugins.settings.get(ID).and_then(|t| t.get("cities")) {
        Some(toml::Value::String(s)) => parse(s),
        Some(toml::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).map(String::from).collect(),
        _ => wc::DEFAULT_CITIES.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn places(cfg: &ShellConfig) -> Vec<Place> {
    let mut out: Vec<Place> = Vec::new();
    for entry in city_entries(cfg) {
        if let Some(p) = wc::resolve(&entry).filter(|p| !out.iter().any(|o| o.key == p.key)) {
            out.push(p);
        }
    }
    out.truncate(MAX_CITIES);
    out
}

thread_local! {
    /// The resolved cities and the entries they were resolved from.
    static PLACES: RefCell<Option<(Vec<String>, Rc<[Place]>)>> = const { RefCell::new(None) };
}

/// [`places`], resolved again only when the configured entries change.
fn cached_places(cfg: &ShellConfig) -> Rc<[Place]> {
    let entries = city_entries(cfg);
    PLACES.with(|c| {
        let mut c = c.borrow_mut();
        match c.as_ref() {
            Some((e, list)) if *e == entries => list.clone(),
            _ => {
                let list: Rc<[Place]> = places(cfg).into();
                *c = Some((entries, list.clone()));
                list
            }
        }
    })
}

fn save_entries(entries: &[String], cx: &mut App) {
    let joined = entries.join(", ");
    update_config(cx, |c| {
        c.plugins.settings.entry(ID.to_string()).or_default().insert("cities".into(), toml::Value::String(joined));
    });
}

/// Adds a city (by search key); errors when full or already listed.
pub fn add_city(key: &str, cx: &mut App) -> Result<Place, String> {
    let place = wc::resolve(key).ok_or_else(|| tf("Unknown city or time zone {name}", &[("name", &key)]))?;
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let mut entries = city_entries(&cfg);
    if places(&cfg).iter().any(|p| p.key == place.key) {
        return Err(tf("{city} is already on the clock", &[("city", &place.name)]));
    }
    if entries.len() >= MAX_CITIES {
        return Err(tf("The world clock holds up to {n} cities", &[("n", &MAX_CITIES)]));
    }
    entries.push(place.key.clone());
    save_entries(&entries, cx);
    Ok(place)
}

pub fn remove_city(key: &str, cx: &mut App) -> Result<(), String> {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let target = wc::resolve(key).map(|p| p.key).unwrap_or_else(|| key.to_string());
    let mut entries = city_entries(&cfg);
    let before = entries.len();
    entries.retain(|e| wc::resolve(e).is_none_or(|p| p.key != target) && !e.eq_ignore_ascii_case(key));
    if entries.len() == before {
        return Err(tf("{city} is not on the clock", &[("city", &key)]));
    }
    save_entries(&entries, cx);
    Ok(())
}

fn h24(cfg: &ShellConfig) -> bool {
    match cfg.plugins.setting(ID, "clock", String::from("shell")).as_str() {
        "12" => false,
        "24" => true,
        _ => cfg.general.clock_24h,
    }
}

fn work_hours(cfg: &ShellConfig) -> (u32, u32) {
    let clamp = |v: i64| u32::try_from(v.clamp(0, LAST_HOUR)).unwrap_or(0);
    (
        clamp(cfg.plugins.setting(ID, "work_start", i64::from(wc::WORK_START_HOUR))),
        clamp(cfg.plugins.setting(ID, "work_end", i64::from(wc::WORK_END_HOUR))),
    )
}

fn local_offset(at: DateTime<Utc>) -> i32 {
    Local.from_utc_datetime(&at.naive_utc()).offset().fix().local_minus_utc()
}

/// `+1 h`, `−5 h 30 m`, or `Same time`.
fn offset_label(seconds: i32) -> String {
    match wc::split_offset(seconds) {
        (0, _, _) => t("Same time").to_string(),
        (sign, h, m) => {
            let s = if sign > 0 { "+" } else { "−" };
            match (h, m) {
                (0, m) => format!("{s}{m} m"),
                (h, 0) => format!("{s}{h} h"),
                (h, m) => format!("{s}{h} h {m} m"),
            }
        }
    }
}

fn day_label(delta: i64) -> String {
    match delta {
        0 => t("Today").to_string(),
        1 => t("Tomorrow").to_string(),
        -1 => t("Yesterday").to_string(),
        d if d > 0 => tf("+{n} days", &[("n", &d)]),
        d => tf("{n} days", &[("n", &d)]),
    }
}

/// Snaps a planner fraction (`0..=1`) to a minute shift.
fn shift_for(fraction: f32) -> i64 {
    let raw = (f64::from(fraction.clamp(0.0, 1.0)) * 2.0 - 1.0) * PLAN_RANGE_MIN as f64;
    ((raw / PLAN_STEP_MIN as f64).round() as i64 * PLAN_STEP_MIN).clamp(-PLAN_RANGE_MIN, PLAN_RANGE_MIN)
}

fn fraction_for(shift: i64) -> f32 {
    ((shift as f32 / PLAN_RANGE_MIN as f32) + 1.0) / 2.0
}

fn shift_label(shift: i64) -> String {
    if shift == 0 {
        return t("Now").to_string();
    }
    offset_label(i32::try_from(shift * SECS_PER_MIN).unwrap_or(0))
}

/// `14:05` / `2:05 PM` → `(time, " PM")` joined for headers.
fn clock_with_meridiem(c: &wc::ClockText) -> String {
    match c.meridiem {
        Some(m) => format!("{} {m}", c.time),
        None => c.time.clone(),
    }
}

/// Delay until just after the next minute boundary.
fn until_next_minute(now: DateTime<Utc>) -> Duration {
    let into_minute = Duration::from_millis(u64::try_from(now.timestamp_millis().rem_euclid(MINUTE.as_millis() as i64)).unwrap_or(0));
    MINUTE.saturating_sub(into_minute) + MINUTE_MARGIN
}

// ── Per-minute row data ──────────────────────────────────────────────────

/// Everything a city row shows, computed once per displayed minute.
struct Row {
    key: SharedString,
    row_id: SharedString,
    remove_id: SharedString,
    primary: SharedString,
    secondary: Option<(SharedString, bool)>,
    detail: SharedString,
    time: SharedString,
    meridiem: Option<&'static str>,
    working: bool,
    sky_now: Option<Sky>,
    /// Sky (when coordinates are known) and working-hour flag per segment.
    strip: Vec<(Option<Sky>, bool)>,
}

/// What the rows were computed from: the displayed minute, the clock
/// style, working hours and the UI script (plus the city list, compared by
/// identity next to it).
#[derive(Clone, PartialEq)]
struct RowsKey {
    minute: i64,
    h24: bool,
    work: (u32, u32),
    arabic_ui: bool,
}

fn sun_text(place: &Place, at: DateTime<Utc>, h24: bool) -> Option<String> {
    let (lat, lon) = place.coords?;
    let local_date = place.time(at).date_naive();
    Some(match wc::daylight(lat, lon, local_date) {
        Daylight::Normal { sunrise, sunset } => {
            let fmt = |t: DateTime<Utc>| {
                let lt = place.time(t);
                let c = wc::format_time(lt.hour(), lt.minute(), h24);
                match c.meridiem {
                    Some(m) => format!("{}{}", c.time, m.to_lowercase().chars().next().unwrap_or('m')),
                    None => c.time,
                }
            };
            format!("↑{} ↓{}", fmt(sunrise), fmt(sunset))
        }
        Daylight::PolarDay => t("Midnight sun").to_string(),
        Daylight::PolarNight => t("Polar night").to_string(),
    })
}

fn sky_at(place: &Place, at: DateTime<Utc>) -> Option<Sky> {
    place.coords.map(|(lat, lon)| wc::sky(wc::solar_altitude(lat, lon, at)))
}

fn build_row(place: &Place, at: DateTime<Utc>, h24: bool, work: (u32, u32), arabic_ui: bool) -> Row {
    let local = at.with_timezone(&Local);
    let lt = place.time(at);
    let clock = wc::format_time(lt.hour(), lt.minute(), h24);
    let delta = wc::day_delta(lt.date_naive(), local.date_naive());
    let offset = place.utc_offset(at) - local_offset(at);
    let mut detail = vec![day_label(delta), offset_label(offset)];
    if let Some(sun) = sun_text(place, at, h24) {
        detail.push(sun);
    }
    let (primary, secondary) = match (arabic_ui, place.arabic) {
        (true, Some(ar)) => (SharedString::from(ar), Some(SharedString::from(place.name.clone()))),
        (false, Some(ar)) => (SharedString::from(place.name.clone()), Some(SharedString::from(ar))),
        _ => (SharedString::from(place.name.clone()), None),
    };
    let start = at - chrono::Duration::minutes(MINUTES_PER_SEGMENT * STRIP_SEGMENTS as i64 / 2);
    let strip = (0..STRIP_SEGMENTS)
        .map(|i| {
            let mid = start + chrono::Duration::minutes(MINUTES_PER_SEGMENT * i as i64 + MINUTES_PER_SEGMENT / 2);
            (sky_at(place, mid), wc::is_working_hour(place.time(mid).hour(), work.0, work.1))
        })
        .collect();
    Row {
        key: place.key.clone().into(),
        row_id: format!("wc-row-{}", place.key).into(),
        remove_id: format!("wc-remove-{}", place.key).into(),
        primary,
        secondary: secondary.map(|s| {
            let arabic = s.chars().any(|ch| ARABIC_BLOCK.contains(&ch));
            (s, arabic)
        }),
        detail: detail.join(" · ").into(),
        time: clock.time.into(),
        meridiem: clock.meridiem,
        working: wc::is_working_hour(lt.hour(), work.0, work.1),
        sky_now: sky_at(place, at),
        strip,
    }
}

// ── Views ────────────────────────────────────────────────────────────────

fn sky_colors(palette: &MandaraPalette) -> (Color, Color, Color) {
    let oklch = |(l, c, h): (f32, f32, f32)| Color::oklch(l, c, h);
    let day = readable(oklch(SKY_DAY), palette);
    let dusk = readable(oklch(SKY_DUSK), palette);
    let night = if palette.is_dark { oklch(SKY_NIGHT_DARK) } else { oklch(SKY_NIGHT_LIGHT).with_alpha(SKY_NIGHT_LIGHT_ALPHA) };
    (day, dusk, night)
}

/// The 24 h strip centred on the row's instant: sky colour per half hour
/// (when the city's coordinates are known) with working hours underlined.
fn day_strip(row: &Row, palette: &MandaraPalette) -> AnyElement {
    let (day, dusk, night) = sky_colors(palette);
    let neutral = palette.on_surface.with_alpha(super::panel_fx::alpha(STRIP_NEUTRAL_ALPHA, palette));
    let work = readable(palette.primary, palette).with_alpha(STRIP_WORK_ALPHA);
    let (day, dusk) = (day.with_alpha(STRIP_DAY_ALPHA), dusk.with_alpha(STRIP_DUSK_ALPHA));
    let last = row.strip.len().saturating_sub(1);
    let segments = row.strip.iter().enumerate().map(move |(i, (sky, working))| {
        let fill = match sky {
            Some(Sky::Day) => day,
            Some(Sky::Twilight) => dusk,
            Some(Sky::Night) => night,
            None => neutral,
        };
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(STRIP_GAP_PX))
            .child(div().h(px(STRIP_H)).bg(fill.hsla()).when(i == 0, |el| el.rounded_l(px(crate::style::radius(STRIP_RADIUS_PX)))).when(i == last, |el| el.rounded_r(px(crate::style::radius(STRIP_RADIUS_PX)))))
            .child(div().h(px(STRIP_UNDERLINE_PX)).when(*working, |el| el.bg(work.hsla())))
    });
    div()
        .relative()
        .w_full()
        .flex()
        .children(segments)
        // The shared "this instant" marker.
        .child(
            div()
                .absolute()
                .top(px(-MARKER_OVERHANG_PX))
                .left(gpui::relative(0.5))
                .ml(px(-MARKER_W_PX / 2.0))
                .w(px(MARKER_W_PX))
                .h(px(STRIP_H + MARKER_OVERHANG_PX * 2.0))
                .rounded_full()
                .bg(palette.on_surface.hsla()),
        )
        .into_any_element()
}

fn sky_glyph(sky: Option<Sky>, palette: &MandaraPalette) -> AnyElement {
    let (day, dusk, night) = sky_colors(palette);
    let (name, tint) = match sky {
        Some(Sky::Day) => ("sun", day),
        Some(Sky::Twilight) => ("sunset", dusk),
        Some(Sky::Night) => ("moon", readable(night.with_alpha(1.0).with_lightness(MOON_L), palette)),
        None => ("clock", readable(palette.secondary, palette)),
    };
    div()
        .size(px(SKY_TILE_PX))
        .flex_none()
        .rounded(px(crate::style::radius(SKY_TILE_RADIUS_PX)))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(SKY_TILE_FILL.0), partner(tint).with_alpha(SKY_TILE_FILL.1)))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(name, SKY_GLYPH_PX, tint))
        .into_any_element()
}

struct Ctx<'a> {
    arabic_font: SharedString,
    display: SharedString,
    palette: &'a MandaraPalette,
    radius: f32,
    views: &'a ViewSet,
}

fn city_row(row: &Row, c: &Ctx<'_>) -> AnyElement {
    let palette = c.palette;
    let emerald = readable(palette.success_emerald, palette);
    let key = row.key.clone();
    let views = c.views.clone();
    div()
        .id(row.row_id.clone())
        .group("wc-row")
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .py(px(ROW_PAD_Y_PX))
        .rounded(px(crate::style::concentric(c.radius, ROW_RADIUS_INSET_PX)))
        .hover(|s| s.bg(palette.on_surface.with_alpha(ROW_HOVER_ALPHA).hsla()))
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(ROW_INNER_GAP_PX))
                .child(sky_glyph(row.sky_now, palette))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(INLINE_GAP_PX))
                                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(row.primary.clone()))
                                .when_some(row.secondary.clone(), |el, (s, arabic)| {
                                    el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).when(arabic, |el| el.font_family(c.arabic_font.clone())).truncate().child(s))
                                })
                                .when(row.working, |el| el.child(dot(emerald, WORKING_DOT_PX))),
                        )
                        .child(caption(row.detail.clone(), palette).font_features(tabular_nums()).truncate()),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_end()
                        .gap(px(CLOCK_GAP_PX))
                        .font_family(c.display.clone())
                        .font_features(tabular_nums())
                        .child(div().text_size(px(CLOCK_PX)).line_height(px(CLOCK_LINE_PX)).font_weight(FontWeight::MEDIUM).child(row.time.clone()))
                        .when_some(row.meridiem, |el, m| {
                            el.child(div().pb(px(MERIDIEM_PAD_PX)).text_size(px(MERIDIEM_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(m))
                        }),
                )
                .child(div().w(px(REMOVE_BTN_PX)).flex_none().invisible().group_hover("wc-row", |s| s.visible()).child(icon_button(
                    row.remove_id.clone(),
                    "close",
                    REMOVE_BTN_PX,
                    palette.on_surface_variant,
                    None,
                    move |_, cx| {
                        let _ = remove_city(&key, cx);
                        views.notify(cx);
                    },
                ))),
        )
        .child(day_strip(row, palette))
        .into_any_element()
}

/// The meeting planner: a track with hour ticks and a knob that shifts all
/// clocks by up to ±12 h in 15-minute steps.
fn planner(shift: &Rc<Cell<i64>>, when: SharedString, views: &ViewSet, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let v = fraction_for(shift.get());
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
    let pick = {
        let bounds = bounds.clone();
        move |x: Pixels| {
            let b = bounds.get();
            ((f32::from(x) - f32::from(b.origin.x)) / f32::from(b.size.width).max(1.0)).clamp(0.0, 1.0)
        }
    };
    let (pick_down, pick_move) = (pick.clone(), pick);
    let (s_down, s_move) = (shift.clone(), shift.clone());
    let (v_down, v_move) = (views.clone(), views.clone());
    let (lo, hi) = (v.min(0.5), v.max(0.5));
    let ticks = (0..=PLANNER_HOURS).map(|h| {
        let major = h % MAJOR_TICK_EVERY == 0;
        let pick = |(a, b): (f32, f32)| if major { a } else { b };
        div()
            .absolute()
            .left(gpui::relative(h as f32 / PLANNER_HOURS as f32))
            .top(px(pick(TICK_TOP_PX)))
            .ml(px(-TICK_W_PX / 2.0))
            .w(px(TICK_W_PX))
            .h(px(pick(TICK_H_PX)))
            .bg(palette.on_surface.with_alpha(pick(TICK_ALPHA)).hsla())
    });
    let track = div()
        .id("wc-planner")
        .relative()
        .h(px(PLANNER_TRACK_H))
        .w_full()
        .rounded_full()
        .cursor_pointer()
        .bg(wash(WASH_SOFT, palette).hsla())
        .children(ticks)
        .child(
            div()
                .absolute()
                .top(px(BAND_INSET_PX))
                .bottom(px(BAND_INSET_PX))
                .left(gpui::relative(lo))
                .w(gpui::relative(hi - lo))
                .rounded_full()
                .bg(oklab_gradient(ANGLE_RIGHT_DEG, accent.with_alpha(BAND_ALPHA), partner(accent).with_alpha(BAND_ALPHA))),
        )
        .child(
            div()
                .absolute()
                .top(px(CENTER_INSET_PX))
                .left(gpui::relative(0.5))
                .ml(px(-MARKER_W_PX / 2.0))
                .w(px(MARKER_W_PX))
                .h(px(PLANNER_TRACK_H - CENTER_INSET_PX * 2.0))
                .rounded_full()
                .bg(palette.on_surface_variant.with_alpha(CENTER_ALPHA).hsla()),
        )
        .child(
            div()
                .absolute()
                .top(px(KNOB_TOP_PX))
                .left(gpui::relative(v))
                .ml(px(-KNOB_PX / 2.0))
                .size(px(KNOB_PX))
                .rounded_full()
                .bg(accent.hsla())
                .shadow(vec![glow(accent.with_alpha(KNOB_GLOW.0), KNOB_GLOW.1, 0.0, 1.0)])
                .child(div().m(px((KNOB_PX - KNOB_DOT_PX) / 2.0)).size(px(KNOB_DOT_PX)).rounded_full().bg(palette.on_primary.hsla())),
        )
        .child(canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| {
            s_down.set(shift_for(pick_down(e.position.x)));
            v_down.notify(cx);
        })
        .on_mouse_move(move |e: &MouseMoveEvent, _, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                let next = shift_for(pick_move(e.position.x));
                if next != s_move.get() {
                    s_move.set(next);
                    v_move.notify(cx);
                }
            }
        });
    let label = |s: SharedString| caption(s, palette).font_features(tabular_nums());
    glass_card(palette)
        .rounded(px(radius))
        .h(px(PLANNER_H))
        .p(px(tokens::MD))
        .flex()
        .flex_col()
        .gap(px(BLOCK_GAP_PX))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .child(icon("calendar", PLANNER_ICON_PX, accent))
                        .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).child(t("Meeting planner"))),
                )
                .child(
                    div()
                        .font_features(tabular_nums())
                        .text_size(px(text::BODY_S))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if shift.get() == 0 { palette.on_surface_variant.hsla() } else { accent.hsla() })
                        .child(when),
                ),
        )
        .child(track)
        .child(div().flex().justify_between().child(label("−12 h".into())).child(label(t("now").into())).child(label("+12 h".into())))
        .into_any_element()
}

/// A search hit with its zone's current time, formatted when searched.
struct Hit {
    id: SharedString,
    m: wc::Match,
    time: SharedString,
}

fn result_row(hit: &Hit, already: bool, field: &Entity<TextField>, views: &ViewSet, palette: &MandaraPalette, arabic_font: &SharedString) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let m = &hit.m;
    let key = m.key.clone();
    let (f, views) = (field.clone(), views.clone());
    div()
        .id(hit.id.clone())
        .h(px(RESULT_H))
        .px(px(RESULT_PAD_X_PX))
        .rounded(px(crate::style::radius(RESULT_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(RESULT_GAP_PX))
        .when(!already, |el| el.cursor_pointer().hover(|s| s.bg(palette.on_surface.with_alpha(RESULT_HOVER_ALPHA).hsla())))
        .when(already, |el| el.opacity(RESULT_ADDED_OPACITY))
        .child(icon(if already { "check" } else { "plus" }, RESULT_ICON_PX, accent))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(INLINE_GAP_PX))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).truncate().child(SharedString::from(m.name.clone())))
                .when_some(m.arabic, |el, ar| el.child(div().font_family(arabic_font.clone()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(ar)))
                .child(caption(m.zone.clone(), palette).truncate()),
        )
        .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(hit.time.clone()))
        .when(!already, |el| {
            el.on_click(move |_, _, cx| {
                if add_city(&key, cx).is_ok() {
                    f.update(cx, |f, _| f.set(""));
                }
                views.notify(cx);
            })
        })
        .into_any_element()
}

/// Panel height for `cities` rows and the search state.
fn panel_height(cities: usize, query: bool, results: usize) -> f32 {
    let rows_h = if cities == 0 { EMPTY_H } else { cities as f32 * ROW_H + (cities as f32 - 1.0) * ROW_GAP + CITIES_PAD_PX * 2.0 };
    let results_h = if query { BLOCK_GAP_PX + results.max(1) as f32 * RESULT_H } else { 0.0 };
    PAD * 2.0 + HEADER_H + GAP + PLANNER_H + GAP + SECTION_H_PX + BLOCK_GAP_PX + rows_h + GAP + CONTROL_H + results_h
}

/// Per-view caches: rows per displayed minute, search hits per query.
#[derive(Default)]
struct Caches {
    rows: Option<(RowsKey, Rc<[Place]>, Rc<[Row]>)>,
    search: Option<(String, i64, Rc<[Hit]>)>,
    /// `(minute, h24)` → header subtitle and planner label.
    header: Option<((i64, i64, bool), SharedString, SharedString, SharedString)>,
}

struct View {
    p: PanelCx,
    field: Entity<TextField>,
    shift: Rc<Cell<i64>>,
    last_render: Rc<Cell<Option<Instant>>>,
    size_cell: Rc<Cell<gpui::Size<Pixels>>>,
    views: ViewSet,
    caches: Rc<RefCell<Caches>>,
    /// Frees the caches once the panel has been closed a while (dropping
    /// the handle cancels it).
    release: Rc<RefCell<Option<Task<()>>>>,
}

impl View {
    /// Starts the watchdog that drops the per-city rows, search hits and
    /// header labels after the panel stops rendering (it was closed).
    fn release_when_closed(&self, cx: &mut App) {
        if self.release.borrow().is_some() {
            return;
        }
        let (last_render, caches, release) = (self.last_render.clone(), self.caches.clone(), self.release.clone());
        let task = cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(CACHE_CHECK_EVERY).await;
                if last_render.get().is_none_or(|t| t.elapsed() > PLAN_RESET_AFTER) {
                    *caches.borrow_mut() = Caches::default();
                    break;
                }
            }
            release.borrow_mut().take();
        });
        *self.release.borrow_mut() = Some(task);
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        self.views.track(window);
        if self.last_render.get().is_none_or(|t| t.elapsed() > PLAN_RESET_AFTER) {
            self.shift.set(0);
        }
        self.last_render.set(Some(Instant::now()));
        self.release_when_closed(cx);
        let cfg = crate::state::config(cx).read(cx).snapshot();
        let now = Utc::now();
        if self.p.progress() > 0.0 {
            redraw_in(until_next_minute(now), window, cx);
        }
        let shift = self.shift.get();
        let at = now + chrono::Duration::minutes(shift);
        let minute = at.timestamp().div_euclid(SECS_PER_MIN);
        let now_minute = now.timestamp().div_euclid(SECS_PER_MIN);
        let list = cached_places(&cfg);
        let query = self.field.read(cx).value.clone();
        let query_trimmed = query.trim();
        let (clock24, work, arabic_ui) = (h24(&cfg), work_hours(&cfg), cfg.general.locale.is_arabic());
        let mut caches = self.caches.borrow_mut();

        let rows_key = RowsKey { minute, h24: clock24, work, arabic_ui };
        let rows = match &caches.rows {
            Some((k, places, rows)) if *k == rows_key && Rc::ptr_eq(places, &list) => rows.clone(),
            _ => {
                let rows: Rc<[Row]> = list.iter().map(|pl| build_row(pl, at, clock24, work, arabic_ui)).collect();
                caches.rows = Some((rows_key, list.clone(), rows.clone()));
                rows
            }
        };
        let hits: Rc<[Hit]> = if query_trimmed.is_empty() {
            Rc::from([])
        } else {
            match &caches.search {
                Some((q, m, hits)) if *q == query && *m == now_minute => hits.clone(),
                _ => {
                    let hits: Rc<[Hit]> = wc::search(&query)
                        .into_iter()
                        .take(RESULTS_SHOWN)
                        .map(|m| {
                            let time = m.zone.parse::<wc::Tz>().ok().map(|tz| now.with_timezone(&tz).format("%H:%M").to_string()).unwrap_or_default().into();
                            Hit { id: format!("wc-add-{}", m.key).into(), m, time }
                        })
                        .collect();
                    caches.search = Some((query.clone(), now_minute, hits.clone()));
                    hits
                }
            }
        };
        let header_key = (now_minute, shift, clock24);
        if caches.header.as_ref().is_none_or(|(k, ..)| *k != header_key) {
            let local = now.with_timezone(&Local);
            let lc = wc::format_time(local.hour(), local.minute(), clock24);
            let zone_name = iana_local().map(|z| wc::zone_display_name(&z)).unwrap_or_else(|| t("Local time").to_string());
            let subtitle = format!("{zone_name} · {}", clock_with_meridiem(&lc));
            let shifted = at.with_timezone(&Local);
            let sc = wc::format_time(shifted.hour(), shifted.minute(), clock24);
            let when = format!("{} · {} {}", shift_label(shift), crate::widgets::clock::localized(&shifted, "%a"), clock_with_meridiem(&sc));
            caches.header = Some((header_key, subtitle.into(), when.into(), utc_label(local_offset(now)).into()));
        }
        let Some((_, subtitle, when, utc)) = caches.header.clone() else { return div().into_any_element() };
        drop(caches);

        let f = fonts(cx);
        let radius = PanelShape::of(cx).inner(PAD);
        let height = panel_height(list.len(), !query_trimmed.is_empty(), hits.len());
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }
        let c = Ctx { arabic_font: f.arabic.clone(), display: f.display.clone(), palette, radius, views: &self.views };

        let accent = readable(palette.primary, palette);
        let trailing = if shift != 0 {
            let (s, views) = (self.shift.clone(), self.views.clone());
            pill_button("wc-now", Some("refresh"), t("Now"), accent, palette, move |_, cx| {
                s.set(0);
                views.notify(cx);
            })
            .into_any_element()
        } else {
            chip("globe", utc, palette.on_surface_variant, palette).into_any_element()
        };
        let head = header("globe", accent, t("World clock"), subtitle, palette, Some(trailing));

        let cities: AnyElement = if rows.is_empty() {
            empty_state("globe", t("No cities yet"), t("Search below to add a city or time zone"), EMPTY_H, palette).into_any_element()
        } else {
            glass_card(palette).rounded(px(radius)).p(px(CITIES_PAD_PX)).flex().flex_col().gap(px(ROW_GAP)).children(rows.iter().map(|row| city_row(row, &c))).into_any_element()
        };
        let count = tf("{n} of {max}", &[("n", &list.len()), ("max", &MAX_CITIES)]);
        let search = div()
            .flex()
            .flex_col()
            .gap(px(BLOCK_GAP_PX))
            .child(div().flex().child(search_box("wc-clear", &self.field, &query, palette)))
            .when(!query_trimmed.is_empty(), |el| {
                if hits.is_empty() {
                    el.child(
                        div()
                            .h(px(RESULT_H))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(text::BODY_S))
                            .text_color(palette.on_surface_variant.hsla())
                            .child(tf("No city or zone matches \u{201c}{query}\u{201d}", &[("query", &query_trimmed)])),
                    )
                } else {
                    el.children(hits.iter().map(|h| {
                        let m = &h.m;
                        let already = list.iter().any(|p| p.key == m.key || (p.name == m.name && p.tz.name() == m.zone));
                        result_row(h, already, &self.field, &self.views, palette, &f.arabic)
                    }))
                }
            });
        let p = &self.p;
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(p, 1, planner(&self.shift, when, &self.views, palette, radius)))
            .child(div().h(px(GAP)))
            .child(reveal(
                p,
                2,
                div().flex().flex_col().gap(px(BLOCK_GAP_PX)).child(section(t("CITIES"), palette, Some(caption(count, palette).font_features(tabular_nums()).into_any_element()))).child(cities),
            ))
            .child(div().h(px(GAP)))
            .child(reveal(p, 3, search))
            .into_any_element()
    }
}

fn utc_label(seconds: i32) -> String {
    match wc::split_offset(seconds) {
        (0, _, _) => "UTC".into(),
        (sign, h, 0) => format!("UTC{}{h}", if sign > 0 { "+" } else { "−" }),
        (sign, h, m) => format!("UTC{}{h}:{m:02}", if sign > 0 { "+" } else { "−" }),
    }
}

/// The local IANA zone id (`$TZ` or `/etc/localtime`'s target), looked up
/// once per process (a symlink read is not something to do per frame).
fn iana_local() -> Option<String> {
    thread_local! {
        static ZONE: Option<String> = read_iana_local();
    }
    ZONE.with(Clone::clone)
}

fn read_iana_local() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':');
        if tz.parse::<wc::Tz>().is_ok() {
            return Some(tz.to_string());
        }
    }
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let s = target.to_string_lossy();
    let id = s.split("zoneinfo/").nth(1)?;
    id.parse::<wc::Tz>().ok().map(|_| id.to_string())
}

fn bar_view(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let now = Utc::now();
    redraw_in(until_next_minute(now), window, cx);
    let n = usize::try_from(cfg.plugins.setting(ID, "bar_cities", DEFAULT_BAR_CITIES).clamp(0, MAX_BAR_CITIES)).unwrap_or(0);
    let list = cached_places(&cfg);
    let h24 = h24(&cfg);
    if list.is_empty() || n == 0 {
        return icon("globe", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface).into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap(px(BAR_GAP_PX))
        .font_features(tabular_nums())
        .children(list.iter().take(n).map(|p| {
            let lt = p.time(now);
            let c = wc::format_time(lt.hour(), lt.minute(), h24);
            div()
                .flex()
                .items_center()
                .gap(px(BAR_CITY_GAP_PX))
                .child(div().text_size(px(BAR_CODE_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(SharedString::from(p.code.clone())))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(match c.meridiem {
                    Some(m) => format!("{}{}", c.time, m.to_lowercase()),
                    None => c.time,
                }))
        }))
        .into_any_element()
}

fn place_json(p: &Place, at: DateTime<Utc>) -> serde_json::Value {
    let lt = p.time(at);
    let sun = p.coords.map(|(lat, lon)| match wc::daylight(lat, lon, lt.date_naive()) {
        Daylight::Normal { sunrise, sunset } => json!({ "sunrise": p.time(sunrise).to_rfc3339(), "sunset": p.time(sunset).to_rfc3339() }),
        Daylight::PolarDay => json!("polar-day"),
        Daylight::PolarNight => json!("polar-night"),
    });
    let minutes = |secs: i32| secs / SECS_PER_MIN as i32;
    json!({
        "name": p.name,
        "arabic": p.arabic,
        "code": p.code,
        "zone": p.tz.name(),
        "time": lt.to_rfc3339(),
        "utc_offset_minutes": minutes(p.utc_offset(at)),
        "offset_vs_local_minutes": minutes(p.utc_offset(at) - local_offset(at)),
        "daylight": sky_at(p, at).map(|s| format!("{s:?}").to_lowercase()),
        "sun": sun,
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, tk("World clock"), tk("Cities around the world with daylight and a meeting planner"), Some(Zone::Right)),
        |window, cx, panel_cx| {
            let views = ViewSet::default();
            let field_views = views.clone();
            let field = cx.new(|cx| TextField::new(String::new(), t("Add a city or time zone…"), true, Rc::new(move |_, _, cx| field_views.notify(cx)), cx));
            let bar = fn_view(None, bar_view, window, cx);
            // Open at the final height: the city count is known from config.
            let cities = cached_places(&crate::state::config(cx).read(cx).snapshot()).len();
            let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height(cities, false, 0)))));
            let view = View {
                p: panel_cx.clone(),
                field,
                shift: Rc::new(Cell::new(0)),
                last_render: Rc::default(),
                size_cell: size_cell.clone(),
                views,
                caches: Rc::default(),
                release: Rc::default(),
            };
            let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
            PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
        },
    );
    r.command(
        ID,
        CommandSpec::new("world-clock list", tk("Every city on the world clock with its time, offsets and daylight"), |_, cx| {
            let cfg = crate::state::config(cx).read(cx).snapshot();
            let now = Utc::now();
            Ok(json!(cached_places(&cfg).iter().map(|p| place_json(p, now)).collect::<Vec<_>>()))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("world-clock search", tk("Search cities (English or Arabic) and IANA time zones"), |a, _| {
            Ok(json!(wc::search(&a.rest(0)).iter().map(|m| json!({ "key": m.key, "name": m.name, "arabic": m.arabic, "zone": m.zone })).collect::<Vec<_>>()))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("city name, Arabic name or zone id")))
        .example("world-clock search lisbon"),
    );
    r.command(
        ID,
        CommandSpec::new("world-clock add", tk("Add a city or time zone (best search match)"), |a, cx| {
            let query = a.rest(0);
            let key = wc::resolve(&query)
                .map(|p| p.key)
                .or_else(|| wc::search(&query).first().map(|m| m.key.clone()))
                .ok_or_else(|| tf("No city or zone matches {query}", &[("query", &query)]))?;
            let place = add_city(&key, cx)?;
            Ok(place_json(&place, Utc::now()))
        })
        .arg(ArgSpec::required("city", ArgKind::String, tk("city name, Arabic name or zone id")))
        .example("world-clock add Mecca")
        .example("world-clock add Europe/Lisbon"),
    );
    r.command(
        ID,
        CommandSpec::new("world-clock remove", tk("Remove a city from the world clock"), |a, cx| {
            remove_city(&a.rest(0), cx)?;
            Ok(json!("ok"))
        })
        .arg(ArgSpec::required("city", ArgKind::String, tk("city name or zone id")))
        .example("world-clock remove Tokyo"),
    );
    r.command(
        ID,
        CommandSpec::new("world-clock at", tk("Every city's time when it is HH:MM here today (meeting planner)"), |a, cx| {
            const HOURS_PER_DAY: u32 = 24;
            const MINUTES: u32 = 60;
            let (h, m) = a
                .str(0)?
                .split_once(':')
                .and_then(|(h, m)| Some((h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)))
                .filter(|(h, m)| *h < HOURS_PER_DAY && *m < MINUTES)
                .ok_or_else(|| t("Expected a local time as HH:MM").to_string())?;
            let local = Local::now()
                .date_naive()
                .and_hms_opt(h, m, 0)
                .and_then(|n| Local.from_local_datetime(&n).earliest())
                .ok_or_else(|| t("That local time does not exist today").to_string())?;
            let at = local.with_timezone(&Utc);
            let cfg = crate::state::config(cx).read(cx).snapshot();
            let work = work_hours(&cfg);
            Ok(json!(cached_places(&cfg)
                .iter()
                .map(|p| {
                    let mut v = place_json(p, at);
                    v["working_hours"] = json!(wc::is_working_hour(p.time(at).hour(), work.0, work.1));
                    v
                })
                .collect::<Vec<_>>()))
        })
        .arg(ArgSpec::required("time", ArgKind::String, tk("local time, HH:MM")))
        .example("world-clock at 15:30"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_snaps_to_quarter_hours() {
        assert_eq!(shift_for(0.5), 0);
        assert_eq!(shift_for(0.0), -PLAN_RANGE_MIN);
        assert_eq!(shift_for(1.0), PLAN_RANGE_MIN);
        assert_eq!(shift_for(0.5 + 1.0 / 96.0) % PLAN_STEP_MIN, 0);
        for s in [-720, -15, 0, 45, 720] {
            assert_eq!(shift_for(fraction_for(s)), s);
        }
    }

    #[test]
    fn offset_and_day_labels() {
        assert_eq!(offset_label(3600), "+1 h");
        assert_eq!(offset_label(-(5 * 3600 + 1800)), "−5 h 30 m");
        assert_eq!(offset_label(1800), "+30 m");
        assert_eq!(offset_label(0), "Same time");
        assert_eq!(day_label(1), "Tomorrow");
        assert_eq!(utc_label(3 * 3600), "UTC+3");
        assert_eq!(utc_label(5 * 3600 + 45 * 60), "UTC+5:45");
        assert_eq!(utc_label(0), "UTC");
    }

    #[test]
    fn city_entries_accept_strings_and_arrays() {
        let mut cfg = ShellConfig::default();
        assert_eq!(city_entries(&cfg).len(), wc::DEFAULT_CITIES.len());
        cfg.plugins.settings.entry(ID.into()).or_default().insert("cities".into(), toml::Value::String("Cairo, Europe/Lisbon, ,Cairo".into()));
        assert_eq!(city_entries(&cfg), ["Cairo", "Europe/Lisbon", "Cairo"]);
        let p = places(&cfg);
        assert_eq!(p.len(), 2, "duplicates collapse");
        assert_eq!(cached_places(&cfg).len(), 2);
        assert!(Rc::ptr_eq(&cached_places(&cfg), &cached_places(&cfg)), "resolved once per entry list");
        cfg.plugins.settings.entry(ID.into()).or_default().insert("cities".into(), toml::Value::Array(vec!["Tokyo".into()]));
        assert_eq!(places(&cfg)[0].name, "Tokyo");
        assert_eq!(cached_places(&cfg)[0].name, "Tokyo");
    }

    #[test]
    fn local_zone_is_detected() {
        // Either $TZ or /etc/localtime resolves on a normal system.
        if std::path::Path::new("/etc/localtime").exists() {
            assert!(iana_local().is_some());
        }
    }

    #[test]
    fn panel_height_matches_the_original_layout() {
        // Two cities, no search: 14·2 + 38 + 12 + 92 + 12 + 18 + 6 + (2·70 + 6 + 12) + 12 + 34.
        assert_eq!(panel_height(2, false, 0), 410.0);
        assert_eq!(panel_height(0, true, 0), 14.0 * 2.0 + 38.0 + 12.0 + 92.0 + 12.0 + 18.0 + 6.0 + 150.0 + 12.0 + 34.0 + 6.0 + 40.0);
    }

    #[test]
    fn rows_cover_the_day_strip() {
        let place = wc::resolve("Cairo").expect("Cairo resolves");
        let row = build_row(&place, Utc::now(), true, (9, 17), false);
        assert_eq!(row.strip.len(), STRIP_SEGMENTS);
        assert!(row.secondary.as_ref().is_some_and(|(_, arabic)| *arabic));
    }

    #[test]
    fn next_minute_delay_is_bounded() {
        let d = until_next_minute(Utc::now());
        assert!(d > MINUTE_MARGIN && d <= MINUTE + MINUTE_MARGIN);
    }
}
