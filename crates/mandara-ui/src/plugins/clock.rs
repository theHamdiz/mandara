//! Clock plugin: time · date in the bar; panel with a large bilingual clock
//! and a month calendar.
//!
//! Both views tick at the granularity they display: the bar on the next
//! minute boundary (or second, with `bar.show_seconds`), the panel every
//! second while it is open. Labels are formatted once per change.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, NaiveDate, Timelike};
use gpui::{FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_plugins::{CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{EXPIRY_MARGIN, redraw_in};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::dot;
use crate::kit::{card, reveal, text, tokens};
use crate::widgets::clock as fmt;

pub const ID: &str = "clock";

/// Days in a week (calendar columns).
const DAYS_PER_WEEK: u32 = 7;
/// Calendar columns as an array length.
const WEEK: usize = DAYS_PER_WEEK as usize;
/// Last month of the year.
const DECEMBER: u32 = 12;
/// Longest month, for the day-label table.
const MAX_MONTH_DAYS: usize = 31;
/// A known Monday (1 Jan 2024) used to name weekdays in the UI language.
const REF_MONDAY: (i32, u32, u32) = (2024, 1, 1);
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

// Panel.
const PANEL_W_PX: f32 = 340.0;
const PANEL_H_PX: f32 = 392.0;
const BIG_TIME_PX: f32 = 44.0;
const BIG_TIME_LINE_PX: f32 = 46.0;
/// Seconds sit on the big clock's baseline.
const SECONDS_BASELINE_PX: f32 = 6.0;
/// Muted text (seconds, out-of-month labels) alpha.
const MUTED_ALPHA: f32 = 0.7;
const CELL_PX: f32 = 36.0;
const CALENDAR_ROW_GAP_PX: f32 = 2.0;
const CALENDAR_TITLE_PAD_PX: f32 = 6.0;
/// Week-number column width, and the panel width it adds.
const WEEK_COL_PX: f32 = 26.0;
/// Event dot under a calendar day.
const EVENT_DOT_PX: f32 = 4.0;
const EVENT_DOT_BOTTOM_PX: f32 = 3.0;
/// Agenda rows shown (`agenda_rows`).
pub const DEFAULT_AGENDA_ROWS: i64 = 5;
pub const AGENDA_ROWS_RANGE: (i64, i64) = (1, 12);
/// Agenda: row height, the title row and the card's padding.
const AGENDA_ROW_PX: f32 = 40.0;
const AGENDA_TITLE_PX: f32 = 26.0;
const AGENDA_CARD_PAD_PX: f32 = 28.0;
const AGENDA_STRIPE_W_PX: f32 = 3.0;
const AGENDA_STRIPE_H_PX: f32 = 26.0;
const AGENDA_TIME_W_PX: f32 = 104.0;

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

/// Day-of-month labels `"" , "1" … "31"`, formatted once per thread.
fn day_label(day: Option<u32>) -> SharedString {
    thread_local! {
        static LABELS: Vec<SharedString> = (0..=MAX_MONTH_DAYS).map(|d| if d == 0 { SharedString::default() } else { d.to_string().into() }).collect();
    }
    let i = day.map_or(0, |d| d as usize).min(MAX_MONTH_DAYS);
    LABELS.with(|l| l[i].clone())
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

/// Panel label cache. Everything but the seconds changes once a minute (or
/// on a language / week-start change).
#[derive(Default)]
struct PanelText {
    /// (minute, language, date language, big clock pattern)
    minute: Option<(i64, mandara_i18n::Lang, chrono::Locale, &'static str)>,
    hh_mm: SharedString,
    weekday: SharedString,
    long_date: SharedString,
    month_title: SharedString,
    week_label: SharedString,
    second: Option<i64>,
    seconds: SharedString,
    header_key: Option<(u32, mandara_i18n::Lang, chrono::Locale)>,
    weekdays: Vec<SharedString>,
    grid_key: Option<(i32, u32, u32)>,
    grid: Vec<[Option<u32>; WEEK]>,
    /// ISO week number of each grid row.
    week_numbers: Vec<SharedString>,
    /// Calendar events: (store generation, minute) they were built for,
    /// days of this month with events, and the agenda rows.
    events_key: Option<(u64, i64, u32, bool)>,
    event_days: std::collections::HashSet<u32>,
    agenda: Vec<AgendaRow>,
}

/// One upcoming event in the clock panel.
#[derive(Clone)]
struct AgendaRow {
    when: SharedString,
    title: SharedString,
    detail: SharedString,
    color: Option<mandara_theme::Color>,
}

/// ISO week of a grid row: its Monday's, else its first day's.
fn row_week(year: i32, month: u32, row: &[Option<u32>; WEEK], week_start: u32) -> Option<u32> {
    let monday_col = ((DAYS_PER_WEEK - week_start % DAYS_PER_WEEK) % DAYS_PER_WEEK) as usize;
    let day = row[monday_col].or_else(|| row.iter().flatten().next().copied())?;
    NaiveDate::from_ymd_opt(year, month, day).map(|d| d.iso_week().week())
}

impl PanelText {
    /// Event dots and agenda rows from the calendar store (once a minute or
    /// when the calendars reload).
    fn update_events(&mut self, now: &DateTime<Local>, cfg: &mandara_config::ShellConfig, cx: &gpui::App) {
        let Some(store) = crate::calendar::store(cx) else { return };
        let store = store.read(cx);
        let minute = now.timestamp().div_euclid(MS_PER_MIN as i64 / MS_PER_S as i64);
        let key = (store.generation, minute, cfg.calendar.agenda_days, cfg.calendar.enabled && cfg.calendar.hide_past);
        if self.events_key == Some(key) {
            return;
        }
        self.events_key = Some(key);
        self.event_days.clear();
        self.agenda.clear();
        if !cfg.calendar.enabled {
            return;
        }
        let Some(first) = now.date_naive().with_day(1) else { return };
        let next_month = first.checked_add_months(chrono::Months::new(1)).unwrap_or(first);
        let local = |d: NaiveDate| d.and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(Local).earliest());
        if let (Some(from), Some(to)) = (local(first), local(next_month)) {
            for o in store.between(from, to) {
                // Every day the event touches (multi-day events too).
                let mut d = o.start.date_naive().max(first);
                let last = (o.end - chrono::Duration::seconds(1)).date_naive().max(o.start.date_naive());
                while d <= last && d < next_month {
                    self.event_days.insert(d.day());
                    let Some(n) = d.succ_opt() else { break };
                    d = n;
                }
            }
        }
        let Some(today) = local(now.date_naive()) else { return };
        let horizon = today + chrono::Duration::days(i64::from(cfg.calendar.agenda_days.max(1)));
        let colors: Vec<Option<mandara_theme::Color>> = cfg.calendar.sources.iter().filter(|s| s.enabled).map(|s| mandara_theme::Color::from_hex(s.color.trim()).ok()).collect();
        let time_pattern = cfg.datetime.automatic_time(cfg.general.clock_24h, false);
        for o in store.between(today, horizon).into_iter().filter(|o| !(cfg.calendar.hide_past && o.end <= *now)).take(cfg.plugins.setting(ID, "agenda_rows", DEFAULT_AGENDA_ROWS).clamp(AGENDA_ROWS_RANGE.0, AGENDA_ROWS_RANGE.1) as usize) {
            let day = if o.start.date_naive() <= now.date_naive() {
                mandara_i18n::t("Today").to_string()
            } else if Some(o.start.date_naive()) == now.date_naive().succ_opt() {
                mandara_i18n::t("Tomorrow").to_string()
            } else {
                fmt::localized(&o.start, "%a %-d")
            };
            let when = if o.all_day { day.clone() } else { format!("{day} {}", fmt::format_with(&o.start, &time_pattern, "%H:%M")) };
            let detail = if o.all_day { mandara_i18n::t("All day").to_string() } else { o.location.clone() };
            self.agenda.push(AgendaRow { when: when.into(), title: o.summary.clone().into(), detail: detail.into(), color: colors.get(o.source).copied().flatten() });
        }
    }
}

impl PanelText {
    /// `big_clock`: the hours-and-minutes pattern (24-hour / leading zero).
    fn update(&mut self, now: &DateTime<Local>, arabic: bool, week_start: u32, big_clock: &'static str) {
        let lang = mandara_i18n::language();
        // The date language (an override of the interface's) is part of
        // every key, so changing it re-renders without a restart.
        let locale = fmt::locale();
        let minute = (now.timestamp().div_euclid(MS_PER_MIN as i64 / MS_PER_S as i64), lang, locale, big_clock);
        if self.minute != Some(minute) {
            self.minute = Some(minute);
            self.hh_mm = fmt::format_with(now, big_clock, "%H:%M").into();
            // Arabic names are built in, unless a date language is chosen.
            let arabic_names = arabic && fmt::locale_is_default();
            self.weekday = if arabic_names { fmt::weekday_ar(now).into() } else { fmt::localized(now, "%A").into() };
            self.long_date = fmt::long_date(now).into();
            self.month_title = fmt::localized(now, mandara_i18n::t("%B %Y")).into();
            self.week_label = mandara_i18n::tf("Week {week}", &[("week", &now.iso_week().week())]).into();
        }
        if self.second != Some(now.timestamp()) {
            self.second = Some(now.timestamp());
            self.seconds = fmt::seconds(now).into();
        }
        if self.header_key != Some((week_start, lang, locale)) {
            self.header_key = Some((week_start, lang, locale));
            // Weekday headers from the UI language's abbreviations;
            // English keeps its single-letter style.
            let english = lang == mandara_i18n::Lang::En;
            let (y, m, d) = REF_MONDAY;
            self.weekdays = (0..DAYS_PER_WEEK)
                .filter_map(|i| NaiveDate::from_ymd_opt(y, m, d + (i + week_start) % DAYS_PER_WEEK))
                .map(|day| {
                    let abbr = fmt::localized(&day, "%a");
                    if english { abbr.chars().take(1).collect::<String>().into() } else { abbr.into() }
                })
                .collect();
        }
        let grid_key = (now.year(), now.month(), week_start);
        if self.grid_key != Some(grid_key) {
            self.grid_key = Some(grid_key);
            self.grid = month_grid(grid_key.0, grid_key.1, week_start);
            self.week_numbers = self.grid.iter().map(|row| row_week(grid_key.0, grid_key.1, row, week_start).map(|w| w.to_string()).unwrap_or_default().into()).collect();
        }
    }
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
                let (vertical, clock_24h) = (cfg.bar.position.is_vertical(), cfg.general.clock_24h);
                redraw_in(until_next_tick(&now, seconds), window, cx);
                let mut cache = bar_text.borrow_mut();
                let stamp = if seconds { now.timestamp() } else { now.timestamp().div_euclid(MS_PER_MIN as i64 / MS_PER_S as i64) };
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
        let p = panel_cx.clone();
        let panel_text = RefCell::new(PanelText::default());
        let panel_size = Rc::new(Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX))));
        let size_cell = panel_size.clone();
        let panel = fn_view(
            None,
            move |palette, window, cx| {
                let now = Local::now();
                redraw_in(until_next_tick(&now, true), window, cx);
                let f = fonts(cx);
                let (arabic, big_clock) = {
                    let c = crate::state::config(cx).read(cx).get();
                    (c.general.locale.is_arabic(), c.datetime.big_clock_pattern(c.general.clock_24h))
                };
                let week_start = week_start_days(cx);
                let mut s = panel_text.borrow_mut();
                s.update(&now, arabic, week_start, big_clock);
                let cfg = crate::state::config(cx).read(cx).snapshot();
                s.update_events(&now, &cfg, cx);
                let week_numbers = cfg.datetime.week_numbers;
                // The panel grows for the week column and the agenda.
                let agenda_h = if s.agenda.is_empty() { 0.0 } else { AGENDA_TITLE_PX + AGENDA_CARD_PAD_PX + s.agenda.len() as f32 * AGENDA_ROW_PX + tokens::MD };
                let want = size(px(PANEL_W_PX + if week_numbers { WEEK_COL_PX } else { 0.0 }), px(PANEL_H_PX + agenda_h));
                if size_cell.get() != want {
                    size_cell.set(want);
                }
                let today = now.day();
                let muted = palette.on_surface_variant.with_alpha(MUTED_ALPHA).hsla();
                let header = div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(tokens::XS))
                            .font_features(tabular_nums())
                            .child(div().font_family(f.display.clone()).text_size(px(BIG_TIME_PX)).line_height(px(BIG_TIME_LINE_PX)).font_weight(FontWeight::LIGHT).child(s.hh_mm.clone()))
                            .child(div().pb(px(SECONDS_BASELINE_PX)).text_size(px(text::TITLE)).text_color(muted).child(s.seconds.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .pb(px(tokens::XS))
                            .child(
                                div()
                                    .when(arabic, |el| el.font_family(f.arabic.clone()))
                                    .text_size(px(text::TITLE))
                                    .text_color(palette.primary.hsla())
                                    .child(s.weekday.clone()),
                            )
                            .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(s.long_date.clone())),
                    );
                let event_days = &s.event_days;
                let cell = |label: SharedString, is_today: bool, muted_label: bool| {
                    let has_event = !muted_label && label.parse::<u32>().is_ok_and(|d| event_days.contains(&d));
                    div()
                        .size(px(CELL_PX))
                        .relative()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(text::BODY_S))
                        .font_features(tabular_nums())
                        .when(is_today, |el| el.bg(palette.primary.hsla()).text_color(palette.on_primary.hsla()).font_weight(FontWeight::SEMIBOLD))
                        .when(!is_today && muted_label, |el| el.text_color(muted))
                        .child(label)
                        .when(has_event, |el| {
                            let ink = if is_today { palette.on_primary } else { palette.primary };
                            el.child(div().absolute().bottom(px(EVENT_DOT_BOTTOM_PX)).size(px(EVENT_DOT_PX)).rounded_full().bg(ink.hsla()))
                        })
                };
                let week_cell = |label: SharedString| {
                    div().w(px(WEEK_COL_PX)).h(px(CELL_PX)).flex().items_center().justify_center().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(muted).child(label)
                };
                let calendar = card(palette)
                    .flex()
                    .flex_col()
                    .gap(px(CALENDAR_ROW_GAP_PX))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .pb(px(CALENDAR_TITLE_PAD_PX))
                            .child(div().font_weight(FontWeight::MEDIUM).child(s.month_title.clone()))
                            .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(s.week_label.clone())),
                    )
                    .child(div().flex().justify_between().when(week_numbers, |el| el.child(week_cell(SharedString::default()))).children(s.weekdays.iter().map(|w| cell(w.clone(), false, true))))
                    .children(s.grid.iter().zip(s.week_numbers.iter()).map(|(row, wn)| {
                        div()
                            .flex()
                            .justify_between()
                            .when(week_numbers, |el| el.child(week_cell(wn.clone())))
                            .children(row.iter().map(|d| cell(day_label(*d), *d == Some(today), false)))
                    }));
                let agenda = (!s.agenda.is_empty()).then(|| {
                    card(palette)
                        .flex()
                        .flex_col()
                        .child(div().h(px(AGENDA_TITLE_PX)).font_weight(FontWeight::MEDIUM).child(mandara_i18n::t("Upcoming")))
                        .children(s.agenda.iter().map(|row| {
                            div()
                                .h(px(AGENDA_ROW_PX))
                                .flex()
                                .items_center()
                                .gap(px(tokens::SM))
                                .child(div().w(px(AGENDA_STRIPE_W_PX)).h(px(AGENDA_STRIPE_H_PX)).rounded_full().bg(row.color.unwrap_or(palette.primary).hsla()))
                                .child(div().w(px(AGENDA_TIME_W_PX)).flex_none().whitespace_nowrap().truncate().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(row.when.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w(px(0.0))
                                        .child(div().text_size(px(text::BODY_S)).truncate().child(row.title.clone()))
                                        .when(!row.detail.is_empty(), |el| el.child(div().text_size(px(text::LABEL)).truncate().text_color(muted).child(row.detail.clone()))),
                                )
                        }))
                });
                div()
                    .size_full()
                    .p(px(tokens::LG))
                    .flex()
                    .flex_col()
                    .gap(px(tokens::MD))
                    .child(reveal(&p, 0, header))
                    .child(reveal(&p, 1, calendar))
                    .when_some(agenda, |el, a| el.child(reveal(&p, 2, a)))
                    .into_any_element()
            },
            window,
            cx,
        );
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
    fn week_numbers_follow_iso_weeks() {
        // Oct 2026, Monday-first: the first row holds Thu 1 (ISO week 40).
        let grid = month_grid(2026, 10, 0);
        assert_eq!(row_week(2026, 10, &grid[0], 0), Some(40));
        // Sunday-first: the Monday sits in column 1.
        let sunday = month_grid(2026, 10, SUNDAY_FROM_MONDAY);
        assert_eq!(row_week(2026, 10, &sunday[1], SUNDAY_FROM_MONDAY), Some(41), "Sun 4 – Sat 10 → Monday 5 is week 41");
    }

    #[test]
    fn ticks_align_to_the_displayed_unit() {
        use chrono::TimeZone as _;
        let t = Local.with_ymd_and_hms(2026, 10, 27, 12, 30, 45).single().unwrap();
        assert_eq!(until_next_tick(&t, false), Duration::from_secs(15) + EXPIRY_MARGIN);
        assert_eq!(until_next_tick(&t, true), Duration::from_secs(1) + EXPIRY_MARGIN);
        assert_eq!(day_label(Some(7)).as_ref(), "7");
        assert_eq!(day_label(None).as_ref(), "");
    }
}
