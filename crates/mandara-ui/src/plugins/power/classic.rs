//! The Classic Power panel (`power.panel_style = "classic"`, the default):
//! the profiles as three cards with a line each, the apps holding a profile
//! and the "performance degraded" reason, then each battery as a hero card —
//! a dotted charge ring, state, rate and the time estimate, health / cycles /
//! capacity / temperature, a bar-style charge history and the charge limit
//! (switchable when UPower supports it, read-only from sysfs otherwise).
//!
//! The data, actions, header, peripherals, toast and scrolling are shared
//! with the Modern style in the parent module. Unlike the panel this layout
//! was restored from, it names no CPU scaling driver (`intel_pstate` …): no
//! header chip, no driver line on the cards.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{AnyElement, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf};
use mandara_services::power::{self, HistoryPoint, PowerDevice, Profiles};
use mandara_theme::{Color, MandaraPalette};

use super::super::panel_fx::{
    ANGLE_DIAGONAL_DEG, PARTNER_HUE_DEG, SMALL_ICON_PX, WASH_FAINT, alpha, chip, dot_ring, glass_card, glow, icon_chip, meter, percent_text, readable,
    section, switch,
};
use super::{
    HOLD_H, NOTICE_H, PERCENT_FULL, ROW_INSET_PX, SECS_PER_HOUR, SECTION_GAP, SECTION_H, Shared, WORN_HEALTH, degraded_text, estimate_text, hashed_id,
    hold_row, level_colors, level_fraction, notice, now_unix, profile_accent, profile_icon, profile_label, set_limit, set_profile, state_text,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::text;

const PROFILE_H: f32 = 108.0;
const PROFILE_GAP: f32 = 8.0;
const HERO_H: f32 = 112.0;
const STATS_H: f32 = 58.0;
const HISTORY_H: f32 = 74.0;
const LIMIT_H: f32 = 46.0;
const BATTERY_INNER_GAP: f32 = 10.0;
/// Charge ring geometry.
const RING_D: f32 = 88.0;
const RING_DOTS: usize = 40;
const RING_DOT: f32 = 3.2;
/// Sparkline columns across the history span.
const HISTORY_BUCKETS: usize = 48;

/// `ink_on`: backgrounds lighter than this get dark ink.
const INK_SWITCH_L: f32 = 0.64;
/// Dark ink `(lightness, chroma)` and light ink lightness (OKLCH).
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

// Profile cards.
const PROFILE_PAD_PX: f32 = 12.0;
const PROFILE_GLYPH_BOX_PX: f32 = 34.0;
const PROFILE_GLYPH_RADIUS_PX: f32 = 12.0;
const PROFILE_GLYPH_PX: f32 = 18.0;
const PROFILE_CHECK_PX: f32 = 20.0;
const PROFILE_MARK_PX: f32 = 14.0;
const PROFILE_PIN_PX: f32 = 13.0;
/// Subtitle ink alpha on an active card.
const ACTIVE_SUB_ALPHA: f32 = 0.78;
/// Glyph box / check fills on the active card.
const ACTIVE_GLYPH_FILL_ALPHA: f32 = 0.16;
const ACTIVE_CHECK_FILL_ALPHA: f32 = 0.2;
/// Active card glow: alpha `(dark, light)`, blur and dy.
const ACTIVE_GLOW: (f32, f32) = (0.32, 0.26);
const ACTIVE_GLOW_BLUR_PX: f32 = 14.0;
const ACTIVE_GLOW_DY_PX: f32 = 3.0;
/// Hover wash on inactive cards `(dark, light)`.
const CARD_HOVER: (f32, f32) = (0.05, 0.04);

// Stats.
const STAT_LINE_GAP_PX: f32 = 3.0;

// History sparkline.
const SPARK_PAD_PX: f32 = 3.0;
const SPARK_RADIUS_PX: f32 = 8.0;
const SPARK_GAP_PX: f32 = 2.0;
const SPARK_COL_MIN_PX: f32 = 2.0;
const SPARK_COL_RADIUS_PX: f32 = 1.5;
/// Oldest columns fade by this share.
const SPARK_AGE_FADE: f32 = 0.45;
/// History bar start colour alpha.
const HISTORY_FROM_ALPHA: f32 = 0.7;
/// Gap between the "Charge history" label row and the bars.
const HISTORY_LABEL_GAP_PX: f32 = 4.0;

// Battery card.
const BATTERY_PAD_PX: f32 = 14.0;
const RING_TRACK: (f32, f32) = (0.12, 0.1);
const RING_VALUE_PX: f32 = 22.0;
const RING_VALUE_LINE_PX: f32 = 24.0;
const HEADLINE_GAP_PX: f32 = 4.0;
/// Battery state line: two points above the title size.
const STATE_TEXT_PX: f32 = text::TITLE + 2.0;
const CHIPS_GAP_PX: f32 = 6.0;
const CHIPS_TOP_PX: f32 = 2.0;
const HERO_GAP_PX: f32 = 16.0;
const HEALTH_METER_W_PX: f32 = 64.0;
const HEALTH_METER_H_PX: f32 = 4.0;
const STATS_PAD_X_PX: f32 = 12.0;
/// Stats well radius: concentric inside the card, never below the minimum.
const STATS_RADIUS_INSET_PX: f32 = 6.0;
const STATS_MIN_RADIUS_PX: f32 = 10.0;
const STATS_FILL: (f32, f32) = (0.035, 0.03);
const STATS_GAP_PX: f32 = 10.0;
const LIMIT_ICON_PX: f32 = 15.0;
const LIMIT_BUSY_OPACITY: f32 = 0.6;

/// The header line: `74% · On battery · Balanced`.
pub(super) fn subtitle(battery: Option<&PowerDevice>, profile: Option<&str>, ready: bool) -> String {
    let profile = profile.map(profile_label);
    match (battery, &profile) {
        (Some(b), Some(p)) => tf("{pct}% · {state} · {profile}", &[("pct", &b.percentage.round()), ("state", &state_text(b)), ("profile", p)]),
        (Some(b), None) => tf("{pct}% · {state}", &[("pct", &b.percentage.round()), ("state", &state_text(b))]),
        (None, Some(p)) => tf("On mains power · {profile}", &[("profile", p)]),
        (None, None) if !ready => t("Reading power state…").into(),
        (None, None) => t("On mains power").into(),
    }
}

fn profile_hint(name: &str) -> String {
    match name {
        "performance" => t("Full speed, more heat").into(),
        "balanced" => t("Speed and efficiency").into(),
        "power-saver" => t("Cooler, quieter, longer").into(),
        _ => String::new(),
    }
}

/// Legible foreground on a solid `bg` (dark ink on light fills).
fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_SWITCH_L { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn profile_card(name: &str, profiles: &Profiles, switching: Option<&str>, m: &Shared, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let active = profiles.active == name;
    let pending = switching == Some(name);
    let (from, to) = profile_accent(name, palette);
    let accent = readable(from, palette);
    // Midpoint of the card gradient (halfway between both ends).
    let mid = from.mix(to, 0.5);
    let ink = if active { ink_on(mid) } else { palette.on_surface };
    let sub = if active { ink.with_alpha(ACTIVE_SUB_ALPHA) } else { palette.on_surface_variant };
    let held = profiles.holds.iter().any(|h| h.profile == name);
    let hover = palette.on_surface.with_alpha(alpha(CARD_HOVER, palette));
    let (m, target) = (m.clone(), name.to_string());
    let base = if active {
        div()
            .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, from, to))
            .shadow(vec![glow(mid.with_alpha(alpha(ACTIVE_GLOW, palette)), ACTIVE_GLOW_BLUR_PX, 0.0, ACTIVE_GLOW_DY_PX)])
    } else {
        glass_card(palette)
    };
    base.id(hashed_id("power-profile", name))
        .relative()
        .flex_1()
        .min_w(px(0.0))
        .h(px(PROFILE_H))
        .rounded(px(radius))
        .p(px(PROFILE_PAD_PX))
        .flex()
        .flex_col()
        .justify_between()
        .cursor_pointer()
        .when(!active, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .child(if active {
                    div()
                        .size(px(PROFILE_GLYPH_BOX_PX))
                        .rounded(px(crate::style::radius(PROFILE_GLYPH_RADIUS_PX)))
                        .bg(ink.with_alpha(ACTIVE_GLYPH_FILL_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(profile_icon(name), PROFILE_GLYPH_PX, ink))
                        .into_any_element()
                } else {
                    icon_chip(profile_icon(name), accent, PROFILE_GLYPH_BOX_PX).into_any_element()
                })
                .when(active, |el| {
                    el.child(
                        div()
                            .size(px(PROFILE_CHECK_PX))
                            .rounded_full()
                            .bg(ink.with_alpha(ACTIVE_CHECK_FILL_ALPHA).hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon("check", SMALL_ICON_PX, ink)),
                    )
                })
                .when(pending, |el| el.child(icon("timer", PROFILE_MARK_PX, accent)))
                .when(held && !active && !pending, |el| el.child(icon("pin", PROFILE_PIN_PX, palette.on_surface_variant))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(ink.hsla()).truncate().child(profile_label(name)))
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .text_color(sub.hsla())
                        .truncate()
                        .child(if pending { t("Switching…").to_string() } else { profile_hint(name) }),
                ),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            set_profile(&m, target.clone(), cx);
        })
        .into_any_element()
}

/// The profiles section: a card per profile, the degraded reason and the
/// apps holding a profile. Returns the block and its height.
pub(super) fn profiles_block(profiles: &Profiles, switching: Option<&str>, m: &Shared, radius: f32, palette: &MandaraPalette) -> (AnyElement, f32) {
    let mut col = div().flex().flex_col().gap(px(SECTION_GAP)).child(section(t("Power profile"), palette, None));
    let mut h = SECTION_H + SECTION_GAP + PROFILE_H;
    col = col.child(div().flex().gap(px(PROFILE_GAP)).children(profiles.available.iter().map(|p| profile_card(&p.name, profiles, switching, m, radius, palette))));
    if !profiles.degraded.is_empty() {
        col = col.child(notice("thermometer", degraded_text(&profiles.degraded), palette.warning_amber, palette));
        h += SECTION_GAP + NOTICE_H - ROW_INSET_PX;
    }
    for hold in &profiles.holds {
        col = col.child(hold_row(&hold.application_id, &hold.reason, &hold.profile, palette));
        h += SECTION_GAP + HOLD_H - ROW_INSET_PX;
    }
    (col.into_any_element(), h)
}

fn stat(label: &'static str, value: impl Into<SharedString>, extra: Option<AnyElement>, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(STAT_LINE_GAP_PX))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).truncate().child(label))
        .child(div().font_features(tabular_nums()).text_size(px(text::TITLE)).font_weight(FontWeight::MEDIUM).truncate().child(value.into()))
        .when_some(extra, |el, e| el.child(e))
        .into_any_element()
}

/// Columns for the bucketed history; empty slots stay empty (never fake data).
fn history_bars(buckets: &[Option<f32>], height: f32, (from, to): (Color, Color), palette: &MandaraPalette) -> AnyElement {
    let n = buckets.len().max(1);
    let well = palette.on_surface.with_alpha(alpha(WASH_FAINT, palette));
    let inner = height - SPARK_PAD_PX * 2.0;
    div()
        .h(px(height))
        .w_full()
        .p(px(SPARK_PAD_PX))
        .rounded(px(crate::style::radius(SPARK_RADIUS_PX)))
        .bg(well.hsla())
        .flex()
        .items_end()
        .gap(px(SPARK_GAP_PX))
        .children(buckets.iter().enumerate().map(move |(i, b)| {
            let col = div().flex_1().min_w(px(1.0)).h_full().flex().flex_col().justify_end();
            match b {
                Some(v) => {
                    let age = 1.0 - i as f32 / n as f32;
                    col.child(
                        div()
                            .w_full()
                            .h(px((v * inner).max(SPARK_COL_MIN_PX)))
                            .rounded(px(crate::style::radius(SPARK_COL_RADIUS_PX)))
                            .bg(from.mix(to, *v).with_alpha(1.0 - age * SPARK_AGE_FADE).hsla()),
                    )
                }
                None => col,
            }
        }))
        .into_any_element()
}

fn battery_card(d: &PowerDevice, history: Option<&(Instant, Duration, Vec<HistoryPoint>)>, span: Duration, m: &Shared, limit_busy: bool, radius: f32, low_percent: f64, palette: &MandaraPalette) -> (AnyElement, f32) {
    let (from, to) = level_colors(d, low_percent, palette);
    let track = palette.on_surface.with_alpha(alpha(RING_TRACK, palette));
    let frac = level_fraction(d.percentage);
    let ring_inner = div()
        .flex()
        .flex_col()
        .items_center()
        .child(div().font_features(tabular_nums()).text_size(px(RING_VALUE_PX)).line_height(px(RING_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(percent_text(frac)))
        .when(d.state.is_charging(), |el| el.child(icon("zap", SMALL_ICON_PX, from)))
        .into_any_element();
    let ring = dot_ring(frac, RING_D, RING_DOTS, RING_DOT, from, to, track, ring_inner);
    let rate = (d.energy_rate > 0.0).then(|| format!("{:.1} W", d.energy_rate));
    let headline = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(HEADLINE_GAP_PX))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).truncate().child(d.display_name()))
        .child(div().text_size(px(STATE_TEXT_PX)).font_weight(FontWeight::SEMIBOLD).child(state_text(d)))
        .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(estimate_text(d).unwrap_or_else(|| t("Estimating…").into())))
        .child(
            div()
                .flex()
                .gap(px(CHIPS_GAP_PX))
                .mt(px(CHIPS_TOP_PX))
                .when_some(rate, |el, r| el.child(chip(if d.state.is_charging() { "plug" } else { "zap" }, r, from, palette)))
                .when(!d.technology.is_empty(), |el| el.child(chip("battery", d.technology.clone(), palette.on_surface_variant, palette))),
        );
    let hero = div().h(px(HERO_H)).flex().items_center().gap(px(HERO_GAP_PX)).child(ring).child(headline);

    // Health, cycles, energy, temperature.
    let health = d.health();
    let worn = health.is_some_and(|h| h < WORN_HEALTH);
    let health_value: SharedString = health.map_or(SharedString::new_static("–"), |h| percent_text(h as f32));
    let health_tint = if worn { (palette.warning_amber, palette.urgent_coral) } else { (palette.success_emerald, palette.success_emerald.rotate_hue(PARTNER_HUE_DEG)) };
    let health_meter = health.map(|h| {
        div()
            .w(px(HEALTH_METER_W_PX))
            .child(meter(h as f32, HEALTH_METER_H_PX, readable(health_tint.0, palette), readable(health_tint.1, palette), palette))
            .into_any_element()
    });
    let energy: SharedString =
        if d.energy_full > 0.0 && d.energy_full_design > 0.0 { format!("{:.1} / {:.1} Wh", d.energy_full, d.energy_full_design).into() } else { SharedString::new_static("–") };
    let stats = div()
        .h(px(STATS_H))
        .px(px(STATS_PAD_X_PX))
        .rounded(px(crate::style::concentric(radius, STATS_RADIUS_INSET_PX).max(crate::style::radius(STATS_MIN_RADIUS_PX))))
        .bg(palette.on_surface.with_alpha(alpha(STATS_FILL, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(STATS_GAP_PX))
        .child(stat(t("Health"), health_value, health_meter, palette))
        .child(stat(t("Cycles"), d.charge_cycles.map_or(SharedString::new_static("–"), |c| c.to_string().into()), None, palette))
        .child(stat(t("Capacity"), energy, None, palette))
        .when_some(d.temperature, |el, c| el.child(stat(t("Temperature"), format!("{c:.0} °C"), None, palette)));

    // Charge history.
    let hours = span.as_secs() / SECS_PER_HOUR;
    let bars_h = HISTORY_H - SECTION_H - HISTORY_LABEL_GAP_PX;
    let placeholder = |msg: &'static str| {
        div().h(px(bars_h)).flex().items_center().justify_center().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(msg).into_any_element()
    };
    let history_el = match history {
        Some((_, _, points)) if !points.is_empty() => {
            let buckets = power::bucket_history(points, now_unix(), span, HISTORY_BUCKETS, PERCENT_FULL);
            history_bars(&buckets, bars_h, (from.with_alpha(HISTORY_FROM_ALPHA), to), palette)
        }
        Some(_) => placeholder(t("UPower has no history for this span yet")),
        None => placeholder(t("Reading history…")),
    };
    let history_block = div()
        .h(px(HISTORY_H))
        .flex()
        .flex_col()
        .gap(px(HISTORY_LABEL_GAP_PX))
        .child(
            div()
                .h(px(SECTION_H))
                .flex()
                .items_center()
                .justify_between()
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(t("Charge history")))
                .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(tf("last {hours} h", &[("hours", &hours)]))),
        )
        .child(history_el);

    // Charge limit.
    let limit = d.threshold.as_ref().map(|th| {
        let end = th.end.map(|e| tf("Stops charging at {end}%", &[("end", &e)])).unwrap_or_else(|| t("Stops charging early to slow wear").into());
        let detail = match (th.start, th.supported) {
            (Some(s), true) => tf("{limit} · resumes below {start}%", &[("limit", &end), ("start", &s)]),
            (_, true) => end,
            (_, false) => tf("{limit} · set by firmware or root (read-only)", &[("limit", &end)]),
        };
        let row = div()
            .id(hashed_id("power-limit", &d.path))
            .h(px(LIMIT_H))
            .flex()
            .items_center()
            .gap(px(STATS_GAP_PX))
            .child(icon("heart", LIMIT_ICON_PX, readable(palette.success_emerald, palette)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).child(t("Charge limit")))
                    .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(detail)),
            );
        if th.supported {
            let (m, path, on) = (m.clone(), d.path.clone(), th.enabled);
            row.cursor_pointer()
                .when(limit_busy, |el| el.opacity(LIMIT_BUSY_OPACITY))
                .child(switch(on, palette))
                .on_click(move |_, _, cx| {
                    if !m.borrow().limit_busy {
                        set_limit(&m, path.clone(), !on, cx);
                    }
                })
                .into_any_element()
        } else {
            row.child(chip("lock", if th.enabled { t("On") } else { t("Off") }, palette.on_surface_variant, palette)).into_any_element()
        }
    });
    let has_limit = limit.is_some();

    let mut height = BATTERY_PAD_PX * 2.0 + HERO_H + BATTERY_INNER_GAP + STATS_H + BATTERY_INNER_GAP + HISTORY_H;
    if has_limit {
        height += BATTERY_INNER_GAP / 2.0 + LIMIT_H;
    }
    let card = glass_card(palette)
        .rounded(px(radius))
        .p(px(BATTERY_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(BATTERY_INNER_GAP))
        .child(hero)
        .child(stats)
        .child(history_block)
        .when_some(limit, |el, l| el.child(div().mt(px(-BATTERY_INNER_GAP / 2.0)).child(l)))
        .into_any_element();
    (card, height)
}

/// The batteries section: a hero card per battery. Returns the block and
/// its height.
pub(super) fn batteries_block(
    batteries: &[&PowerDevice],
    history: &HashMap<String, (Instant, Duration, Vec<HistoryPoint>)>,
    span: Duration,
    m: &Shared,
    limit_busy: bool,
    radius: f32,
    low_percent: f64,
    palette: &MandaraPalette,
) -> (AnyElement, f32) {
    let mut col = div().flex().flex_col().gap(px(SECTION_GAP)).child(section(if batteries.len() > 1 { t("Batteries") } else { t("Battery") }, palette, None));
    let mut h = SECTION_H;
    for b in batteries {
        let (card, ch) = battery_card(b, history.get(&b.path), span, m, limit_busy, radius, low_percent, palette);
        col = col.child(card);
        h += SECTION_GAP + ch;
    }
    (col.into_any_element(), h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::power::ChargeState;

    #[test]
    fn ink_contrasts_with_fill() {
        assert!(ink_on(Color::oklch(0.8, 0.12, 30.0)).lightness() < 0.4);
        assert!(ink_on(Color::oklch(0.5, 0.15, 30.0)).lightness() > 0.9);
    }

    #[test]
    fn the_header_reads_charge_state_and_profile() {
        let b = PowerDevice { percentage: 74.4, state: ChargeState::Discharging, ..Default::default() };
        assert_eq!(subtitle(Some(&b), Some("balanced"), true), "74% · On battery · Balanced");
        assert_eq!(subtitle(None, Some("power-saver"), true), "On mains power · Power saver");
        assert_eq!(subtitle(None, None, false), "Reading power state…");
    }

    #[test]
    fn profile_lines_describe_the_profile_not_its_driver() {
        for p in ["performance", "balanced", "power-saver"] {
            let hint = profile_hint(p);
            assert!(!hint.is_empty() && !hint.contains("pstate"), "{p}: {hint}");
        }
    }
}
