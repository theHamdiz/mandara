//! The calendar desktop card: the month grid with today highlighted,
//! previous / next month and "Today", and optionally the next calendar
//! event (the clock panel's `[calendar]` ICS store).
//!
//! Cost: it redraws at midnight, when the calendars reload and when you
//! page months — never on a timer otherwise.

use chrono::{Datelike, Local, NaiveDate};
use gpui::{
    App, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use mandara_config::desktop_cards::{self as dc, CalendarOptions};
use mandara_core::ToGpui;
use mandara_theme::MandaraPalette;

use super::now_playing::{card_input, glass};
use super::SHADOW_PAD_PX;
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::plugins::clock::{month_grid, week_start_days};
use crate::widgets::clock as fmt;

/// Card geometry (logical px at scale 1).
const PAD_PX: f32 = 14.0;
const HEADER_PX: f32 = 30.0;
const WEEKDAYS_PX: f32 = 22.0;
const ROWS: usize = 6;
const DAYS: usize = 7;
const CELL_GAP_PX: f32 = 2.0;
/// Day cells are a little flatter than wide.
const CELL_ASPECT: f32 = 0.78;
const AGENDA_PX: f32 = 34.0;
const TITLE_PX: f32 = 14.0;
const DAY_PX: f32 = 12.0;
const LABEL_PX: f32 = 10.5;
const BUTTON_PX: f32 = 24.0;
const BUTTON_ICON_PX: f32 = 13.0;
const TODAY_PILL_PAD_PX: f32 = 8.0;
const STRIPE_W_PX: f32 = 3.0;
const MUTED_ALPHA: f32 = 0.7;
const HOVER_ALPHA: f32 = 0.08;
const MONTHS_PER_YEAR: i32 = 12;
/// Re-check for the next event at least this often (a meeting starting).
const AGENDA_TICK: std::time::Duration = std::time::Duration::from_secs(60);

fn cell_px(w: f32) -> f32 {
    (w - PAD_PX * 2.0 - CELL_GAP_PX * (DAYS as f32 - 1.0)) / DAYS as f32
}

/// Card size `(w, h)` for `opts` at scale `k` (without the shadow pad).
pub fn card_size(opts: &CalendarOptions, k: f32) -> (f32, f32) {
    let w = opts.width();
    let cell_h = cell_px(w) * CELL_ASPECT;
    let grid = ROWS as f32 * cell_h + (ROWS as f32 - 1.0) * CELL_GAP_PX;
    let agenda = if opts.agenda { AGENDA_PX } else { 0.0 };
    (w * k, (PAD_PX * 2.0 + HEADER_PX + WEEKDAYS_PX + grid + agenda) * k)
}

/// `(year, month)` shifted by `delta` months.
pub fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let i = year * MONTHS_PER_YEAR + month as i32 - 1 + delta;
    (i.div_euclid(MONTHS_PER_YEAR), (i.rem_euclid(MONTHS_PER_YEAR) + 1) as u32)
}

pub struct CalendarView {
    widget: String,
    /// Months away from the current one.
    offset: i32,
    input: Option<(f32, f32)>,
}

impl CalendarView {
    pub fn new(widget: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&crate::state::config(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        if let Some(store) = crate::calendar::store(cx) {
            cx.observe(&store, |_, _, cx| cx.notify()).detach();
        }
        Self { widget, offset: 0, input: None }
    }

    fn options(&self, cx: &App) -> (CalendarOptions, f32) {
        let cfg = crate::state::config(cx).read(cx).get();
        cfg.desktop.widgets.get(&self.widget).map(|w| (dc::as_card(w).map(dc::options).unwrap_or_default(), w.place.scale())).unwrap_or((CalendarOptions::default(), 1.0))
    }
}

/// The next event today or later: `(when, title, colour)`.
fn next_event(cx: &App) -> Option<(SharedString, SharedString, Option<mandara_theme::Color>)> {
    let cfg = crate::state::config(cx).read(cx).get();
    if !cfg.calendar.enabled {
        return None;
    }
    let store = crate::calendar::store(cx)?;
    let now = Local::now();
    let horizon = now + chrono::Duration::days(i64::from(cfg.calendar.agenda_days.max(1)));
    let o = store.read(cx).between(now, horizon).into_iter().find(|o| o.end > now)?;
    let colors: Vec<Option<mandara_theme::Color>> = cfg.calendar.sources.iter().filter(|s| s.enabled).map(|s| mandara_theme::Color::from_hex(s.color.trim()).ok()).collect();
    let day = if o.start.date_naive() <= now.date_naive() { mandara_i18n::t("Today").to_string() } else { fmt::localized(&o.start, "%a %-d") };
    let pattern = cfg.datetime.automatic_time(cfg.general.clock_24h, false);
    let when = if o.all_day { day } else { format!("{day} {}", fmt::format_with(&o.start, &pattern, "%H:%M")) };
    Some((when.into(), o.summary.clone().into(), colors.get(o.source).copied().flatten()))
}

fn nav_button(id: &'static str, glyph: &'static str, k: f32, palette: &MandaraPalette, on: impl Fn(&mut CalendarView, &mut Context<CalendarView>) + 'static, cx: &mut Context<CalendarView>) -> impl IntoElement {
    let hover = palette.on_surface.with_alpha(HOVER_ALPHA).hsla();
    div()
        .id(id)
        .size(px(BUTTON_PX * k))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon(glyph, BUTTON_ICON_PX * k, palette.on_surface_variant))
        .on_click(cx.listener(move |this, _, _, cx| {
            on(this, cx);
            cx.notify();
        }))
}

impl Render for CalendarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let (opts, k) = self.options(cx);
        let (w, h) = card_size(&opts, k);
        card_input(&mut self.input, w, h, window);
        let s = |v: f32| v * k;
        let now = Local::now();
        let today = now.date_naive();
        // Redraw at midnight (and every minute for the next-event line).
        let midnight = today.succ_opt().and_then(|d| d.and_hms_opt(0, 0, 0)).and_then(|t| t.and_local_timezone(Local).earliest());
        let until_midnight = midnight.and_then(|m| (m - now).to_std().ok()).unwrap_or(AGENDA_TICK);
        crate::plugins::panel_fx::redraw_in(if opts.agenda { until_midnight.min(AGENDA_TICK) } else { until_midnight }, window, cx);
        let (year, month) = shift_month(today.year(), today.month(), self.offset);
        let week_start = week_start_days(cx);
        let grid = month_grid(year, month, week_start);
        let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(today);
        let title: SharedString = fmt::localized(&first, mandara_i18n::t("%B %Y")).into();
        let english = mandara_i18n::language() == mandara_i18n::Lang::En;
        let weekdays: Vec<SharedString> = (0..DAYS as u32)
            .filter_map(|i| NaiveDate::from_ymd_opt(2024, 1, 1 + (i + week_start) % DAYS as u32))
            .map(|d| {
                let abbr = fmt::localized(&d, "%a");
                if english { abbr.chars().take(2).collect::<String>().into() } else { abbr.into() }
            })
            .collect();
        let cell_w = s(cell_px(opts.width()));
        let cell_h = cell_w * CELL_ASPECT;
        let muted = palette.on_surface_variant.with_alpha(MUTED_ALPHA).hsla();
        let rtl = crate::bidi::rtl();
        let current = self.offset == 0;
        let header = div()
            .h(px(s(HEADER_PX)))
            .flex()
            .items_center()
            .gap(px(s(CELL_GAP_PX)))
            .child(div().flex_1().text_size(px(s(TITLE_PX))).font_weight(FontWeight::SEMIBOLD).child(title))
            .when(!current, |el| {
                el.child(
                    div()
                        .id("cal-today")
                        .h(px(s(BUTTON_PX)))
                        .px(px(s(TODAY_PILL_PAD_PX)))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(s(LABEL_PX)))
                        .bg(palette.primary.with_alpha(HOVER_ALPHA * 2.0).hsla())
                        .text_color(palette.primary.hsla())
                        .child(mandara_i18n::t("Today"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.offset = 0;
                            cx.notify();
                        })),
                )
            })
            .child(nav_button("cal-prev", if rtl { "chevron-right" } else { "chevron-left" }, k, &palette, |v, _| v.offset -= 1, cx))
            .child(nav_button("cal-next", if rtl { "chevron-left" } else { "chevron-right" }, k, &palette, |v, _| v.offset += 1, cx));
        let row = || div().flex().when(rtl, |el| el.flex_row_reverse()).gap(px(s(CELL_GAP_PX)));
        let weekday_row = row().h(px(s(WEEKDAYS_PX))).items_center().children(
            weekdays.into_iter().map(|d| div().w(px(cell_w)).flex().justify_center().text_size(px(s(LABEL_PX))).text_color(muted).child(d)),
        );
        let cells = (0..ROWS).map(|r| {
            let week = grid.get(r).copied().unwrap_or([None; DAYS]);
            row().children(week.into_iter().map(|d| {
                let is_today = current && d == Some(today.day());
                div()
                    .w(px(cell_w))
                    .h(px(cell_h))
                    .rounded(px(cell_h / 2.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(s(DAY_PX)))
                    .font_features(tabular_nums())
                    .when(is_today, |el| el.bg(palette.primary.hsla()).text_color(palette.on_primary.hsla()).font_weight(FontWeight::SEMIBOLD))
                    .child(d.map(|d| SharedString::from(d.to_string())).unwrap_or_default())
            }))
        });
        let agenda = opts.agenda.then(|| {
            let line = next_event(cx);
            div()
                .h(px(s(AGENDA_PX)))
                .flex()
                .items_center()
                .gap(px(s(TODAY_PILL_PAD_PX)))
                .text_size(px(s(LABEL_PX + 1.0)))
                .child(div().w(px(s(STRIPE_W_PX))).h(px(s(AGENDA_PX) * 0.55)).rounded_full().bg(line.as_ref().and_then(|l| l.2).unwrap_or(palette.primary).hsla()))
                .child(match line {
                    Some((when, title, _)) => div()
                        .flex()
                        .gap(px(s(TODAY_PILL_PAD_PX)))
                        .min_w(px(0.0))
                        .child(div().flex_none().font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(when))
                        .child(div().truncate().child(title)),
                    None => div().text_color(muted).child(mandara_i18n::t("No upcoming events")),
                })
        });
        let fonts = crate::fonts::fonts(cx);
        div()
            .size_full()
            .p(px(SHADOW_PAD_PX))
            .font_family(fonts.ui.clone())
            .text_color(palette.on_surface.hsla())
            .child(
                glass(&palette, crate::style::card_radius() * k, 1.0)
                    .w(px(w))
                    .h(px(h))
                    .p(px(s(PAD_PX)))
                    .flex()
                    .flex_col()
                    .gap(px(s(CELL_GAP_PX)))
                    .child(header)
                    .child(weekday_row)
                    .children(cells)
                    .children(agenda),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_wrap_across_years() {
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 10, 0), (2026, 10));
        assert_eq!(shift_month(2026, 3, -27), (2023, 12));
    }

    #[test]
    fn size_fits_six_rows_and_the_agenda() {
        let o = CalendarOptions::default();
        let (w, h) = card_size(&o, 1.0);
        assert_eq!(w, o.width());
        let (_, h_no) = card_size(&CalendarOptions { agenda: false, ..o }, 1.0);
        assert!((h - h_no - AGENDA_PX).abs() < 0.01);
    }
}
