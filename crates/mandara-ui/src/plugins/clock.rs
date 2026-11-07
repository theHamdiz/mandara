//! Clock plugin: time · date in the bar; a panel with a large clock, a
//! month calendar you can page through and the selected day's agenda
//! ([`panel`]).
//!
//! Both views tick at the granularity they display: the bar on the next
//! minute boundary (or second, with `bar.show_seconds`), the panel every
//! second while it is open. Labels are formatted once per change.

mod days;
mod panel;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeDelta, Timelike};
use gpui::{FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{EXPIRY_MARGIN, redraw_in};
use crate::fonts::tabular_nums;
use crate::icons::dot;
use crate::widgets::clock as fmt;

pub const ID: &str = "clock";

/// Days in a week (calendar columns).
const DAYS_PER_WEEK: u32 = 7;
/// Calendar columns as an array length.
const WEEK: usize = DAYS_PER_WEEK as usize;
/// Last month of the year.
const DECEMBER: u32 = 12;
/// `week_start` offsets in days from Monday.
const SATURDAY_FROM_MONDAY: u32 = 5;
const SUNDAY_FROM_MONDAY: u32 = 6;

/// Milliseconds per second / per minute (tick alignment).
const MS_PER_S: u64 = 1000;
const MS_PER_MIN: u64 = 60 * MS_PER_S;

// Bar chip.
const BAR_GAP_PX: f32 = 8.0;
const BAR_DOT_PX: f32 = 3.0;
const BAR_DOT_ALPHA: f32 = 0.45;

/// Agenda rows shown (`agenda_rows`).
pub const DEFAULT_AGENDA_ROWS: i64 = 5;
pub const AGENDA_ROWS_RANGE: (i64, i64) = (1, 12);

/// Weeks of `month` as rows of 7 optional days, starting on `first_weekday`
/// (days from Monday: 0 = Monday, 6 = Sunday, 5 = Saturday). An invalid
/// month yields no rows.
pub fn month_grid(year: i32, month: u32, first_weekday: u32) -> Vec<[Option<u32>; WEEK]> {
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else { return Vec::new() };
    let next = if month == DECEMBER { NaiveDate::from_ymd_opt(year + 1, 1, 1) } else { NaiveDate::from_ymd_opt(year, month + 1, 1) };
    let Some(next) = next else { return Vec::new() };
    let days = u32::try_from(next.signed_duration_since(first).num_days()).unwrap_or(0);
    let offset = ((first.weekday().num_days_from_monday() + DAYS_PER_WEEK - first_weekday % DAYS_PER_WEEK) % DAYS_PER_WEEK) as usize;
    let mut rows = Vec::with_capacity(WEEK - 1);
    let mut row = [None; WEEK];
    let mut col = offset;
    for day in 1..=days {
        row[col] = Some(day);
        col += 1;
        if col == WEEK {
            rows.push(row);
            row = [None; WEEK];
            col = 0;
        }
    }
    if col > 0 {
        rows.push(row);
    }
    rows
}

/// Delay until just past the next whole second (or minute) of `now`.
fn until_next_tick(now: &DateTime<Local>, seconds: bool) -> Duration {
    let into_second = u64::from(now.timestamp_subsec_millis()).min(MS_PER_S - 1);
    let (period, into) = if seconds { (MS_PER_S, into_second) } else { (MS_PER_MIN, u64::from(now.second()) * MS_PER_S + into_second) };
    Duration::from_millis(period.saturating_sub(into)) + EXPIRY_MARGIN
}

pub(crate) fn week_start_days(cx: &gpui::App) -> u32 {
    match crate::state::config(cx).read(cx).get().general.week_start {
        mandara_config::WeekStart::Monday => 0,
        mandara_config::WeekStart::Saturday => SATURDAY_FROM_MONDAY,
        mandara_config::WeekStart::Sunday => SUNDAY_FROM_MONDAY,
    }
}

/// Bar label cache: key `(minute or second stamp, 24 h, seconds)`.
#[derive(Default)]
struct BarText {
    /// (tick stamp, language, date language, time pattern, date pattern, show date)
    key: Option<(i64, mandara_i18n::Lang, chrono::Locale, String, String, bool)>,
    time: SharedString,
    date: SharedString,
}

/// `calendar day`: one day as JSON (week, Hijri date, events).
fn day_json(day: NaiveDate, cx: &gpui::App) -> serde_json::Value {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let offset: i64 = cfg.plugins.setting(super::prayer::ID, "hijri_offset", 0_i64);
    let (hy, hm, hd) = days::hijri_of(day, offset);
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(Local).earliest());
    let events: Vec<serde_json::Value> = match (crate::calendar::store(cx), midnight(day), midnight(day + TimeDelta::days(1))) {
        (Some(store), Some(from), Some(to)) if cfg.calendar.enabled => store.read(cx).between(from, to).iter().map(mandara_services::calendar::Occurrence::to_json).collect(),
        _ => Vec::new(),
    };
    let today = Local::now().date_naive();
    json!({
        "date": day.to_string(),
        "weekday": day.format("%A").to_string(),
        "iso_week": day.iso_week().week(),
        "day_of_year": day.ordinal(),
        "days_from_today": days::days_from(today, day),
        "hijri": { "year": hy, "month": hm, "day": hd, "month_name": days::hijri_month_name(hm, false) },
        "events": events,
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Clock"), mandara_i18n::tk("Time, date and calendar"), Some(Zone::Center)), |window, cx, panel_cx| {
        let bar_text = RefCell::new(BarText::default());
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                let now = Local::now();
                let cfg = crate::state::config(cx).read(cx).get();
                let dt = &cfg.datetime;
                let time_pattern = dt.bar_time_pattern(cfg.general.clock_24h, cfg.bar.show_seconds);
                let fallback = dt.automatic_time(cfg.general.clock_24h, cfg.bar.show_seconds);
                let seconds = fmt::shows_seconds(&time_pattern) || (!fmt::valid_pattern(&time_pattern) && fmt::shows_seconds(&fallback));
                let show_date = dt.bar_show_date;
                let date_pattern = dt.bar_date_format.trim().to_string();
                let clock_24h = cfg.general.clock_24h;
                let vertical = crate::bar::vertical(cx);
                redraw_in(until_next_tick(&now, seconds), window, cx);
                let mut cache = bar_text.borrow_mut();
                let stamp = crate::widgets::clock::local_stamp(&now, seconds);
                let key = (stamp, mandara_i18n::language(), fmt::locale(), time_pattern, date_pattern, show_date);
                if cache.key.as_ref() != Some(&key) {
                    cache.time = fmt::format_with(&now, &key.3, &fallback).into();
                    cache.date = if key.4.is_empty() { fmt::short_date(&now) } else { fmt::format_with(&now, &key.4, mandara_i18n::t("%a %-d %b")) }.into();
                    cache.key = Some(key);
                }
                if vertical {
                    // A side bar: hours over minutes.
                    let hours = now.format(if clock_24h { "%H" } else { "%I" }).to_string();
                    return div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .font_features(tabular_nums())
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(hours))
                        .child(div().text_color(palette.on_surface_variant.hsla()).child(now.format("%M").to_string()))
                        .into_any_element();
                }
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .font_features(tabular_nums())
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(cache.time.clone()))
                    .when(show_date, |el| {
                        el.child(dot(palette.on_surface_variant.with_alpha(BAR_DOT_ALPHA), BAR_DOT_PX))
                            .child(div().text_color(palette.on_surface_variant.hsla()).child(cache.date.clone()))
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        // The panel reports the height it needs on its first frame.
        let panel_size = Rc::new(Cell::new(size(px(panel::PANEL_W_PX), px(panel::PANEL_MIN_H_PX))));
        let panel = fn_view(None, panel::view(panel_cx.clone(), panel_size.clone(), cx.focus_handle()), window, cx);
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: panel_size }),
            on_secondary_click: Some(Rc::new(|_, cx| {
                let shell = crate::state::shell(cx);
                let open = !shell.read(cx).sidebar_visible();
                shell.update(cx, |s, cx| s.set_sidebar(open, cx));
            })),
            ..PluginInstance::empty()
        }
    });
    r.command(ID, CommandSpec::new("clock now", mandara_i18n::tk("Current local date and time"), |_, _| {
        let now = Local::now();
        Ok(json!({ "iso": now.to_rfc3339(), "time": fmt::hh_mm(&now), "date": fmt::long_date(&now), "weekday": now.format("%A").to_string() }))
    }));
    r.command(ID, CommandSpec::new("calendar open", mandara_i18n::tk("Open the calendar"), |_, cx| {
        crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
        Ok(json!({"panel": ID}))
    }).example("calendar open"));
    r.command(
        ID,
        CommandSpec::new("calendar goto", mandara_i18n::tk("Open the clock panel's calendar on a day or month"), |a, cx| {
            let arg = a.rest(0);
            let today = Local::now().date_naive();
            let day = days::parse_target(if arg.trim().is_empty() { "today" } else { &arg }, panel::selected(), today)
                .ok_or_else(|| mandara_i18n::t("Use today, next, prev, +N / -N days, YYYY-MM or YYYY-MM-DD").to_string())?;
            let shell = crate::state::shell(cx);
            if shell.read(cx).panel.as_deref() == Some(ID) {
                panel::go_to(day, cx);
            } else {
                panel::hold(day, cx);
                shell.update(cx, |s, cx| s.set_panel(Some(ID.to_string()), cx));
            }
            Ok(json!({ "month": day.format("%Y-%m").to_string(), "selected": day.to_string() }))
        })
        .arg(ArgSpec::optional("date", ArgKind::String, mandara_i18n::tk("today, next, prev, +N / -N days, YYYY-MM or YYYY-MM-DD")))
        .example("calendar goto 2026-12")
        .example("calendar goto next")
        .example("calendar goto +7"),
    );
    r.command(
        ID,
        CommandSpec::new("calendar day", mandara_i18n::tk("A day's events, ISO week and Hijri date as JSON"), |a, cx| {
            let arg = a.rest(0);
            let today = Local::now().date_naive();
            let day = days::parse_target(if arg.trim().is_empty() { "today" } else { &arg }, today, today)
                .ok_or_else(|| mandara_i18n::t("Use today, next, prev, +N / -N days, YYYY-MM or YYYY-MM-DD").to_string())?;
            Ok(day_json(day, cx))
        })
        .arg(ArgSpec::optional("date", ArgKind::String, mandara_i18n::tk("today, +N / -N days or YYYY-MM-DD")))
        .example("calendar day")
        .example("calendar day 2026-12-25"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn october_2026_grid() {
        // 1 Oct 2026 is a Thursday.
        let grid = month_grid(2026, 10, 0);
        assert_eq!(grid[0], [None, None, None, Some(1), Some(2), Some(3), Some(4)]);
        assert_eq!(grid.last().unwrap()[5], Some(31));
        // Sunday-first: Thursday the 1st sits in column 4.
        assert_eq!(month_grid(2026, 10, 6)[0][4], Some(1));
        // Saturday-first (common in Egypt): column 5.
        assert_eq!(month_grid(2026, 10, 5)[0][5], Some(1));
        let feb = month_grid(2028, 2, 0);
        assert_eq!(feb.iter().flatten().flatten().count(), 29, "leap year");
        assert!(month_grid(2026, 13, 0).is_empty(), "invalid month");
    }

    #[test]
    fn ticks_align_to_the_displayed_unit() {
        use chrono::TimeZone as _;
        let t = Local.with_ymd_and_hms(2026, 10, 27, 12, 30, 45).single().unwrap();
        assert_eq!(until_next_tick(&t, false), Duration::from_secs(15) + EXPIRY_MARGIN);
        assert_eq!(until_next_tick(&t, true), Duration::from_secs(1) + EXPIRY_MARGIN);
    }
}
