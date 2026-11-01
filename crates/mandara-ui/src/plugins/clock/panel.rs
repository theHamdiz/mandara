//! The clock panel: a hero with the time and today's date, a month
//! calendar you can page through, and the agenda of the selected day.
//!
//! - **Calendar:** a fixed six-week grid (so the panel keeps one height),
//!   today filled with the accent, the selected day ringed, weekends and
//!   the neighbouring months' days set apart, event dots in each
//!   calendar's colour, optional ISO week numbers, Hijri day numbers and
//!   Eastern Arabic numerals. Months slide in on a spring.
//! - **Navigation:** the arrows and **Today**, the scroll wheel over the
//!   month, a click on any day (one of a neighbouring month turns the page),
//!   the keyboard (arrows by day and week, Page Up / Down by month, with
//!   Shift by year, Home or T for today) and `calendar goto`.
//! - **Agenda:** today shows what's coming up; any other day lists its own
//!   events, with the one in progress marked.
//!
//! Labels are cached against what they show (the minute, the month on
//! screen, the calendars' generation), so a frame only lays out cached
//! strings. The slide is the only animation and requests frames only while
//! it moves; the panel ticks once a second while it is open and not at all
//! when it is closed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeDelta, Weekday};
use gpui::{
    AnyElement, App, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement, Pixels, ScrollDelta,
    ScrollEvent, SharedString, Size, StatefulInteractiveElement as _, Styled, TouchPhase, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_config::ShellConfig;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui, oklab_gradient};
use mandara_i18n::{t, tf};
use mandara_plugins::PanelCx;
use mandara_services::calendar::Occurrence;
use mandara_theme::{Color, MandaraPalette};
use smallvec::SmallVec;

use super::days::{self, GRID_DAYS, GRID_WEEKS, WEEK_DAYS, YEAR_MONTHS};
use super::{AGENDA_ROWS_RANGE, DEFAULT_AGENDA_ROWS, until_next_tick, week_start_days};
use crate::bidi::Bidi as _;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};
use crate::plugins::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_RIGHT_DEG, HALF_TURN_DEG, PanelShape, SMALL_ICON_PX, TINT_FILL, TINT_HOVER, ViewSet, WASH_FAINT, WASH_SOFT,
    alpha, glass_card, glow, partner, pill_button, readable, redraw_in, rim, wash,
};
use crate::style::ClipRounded as _;
use crate::widgets::clock as fmt;

// ── Layout ───────────────────────────────────────────────────────────────

/// Panel width, padding and the gap between its blocks.
pub(super) const PANEL_W_PX: f32 = 372.0;
const PAD_PX: f32 = tokens::LG;
const GAP_PX: f32 = tokens::MD;
/// Inner padding of the month and agenda cards.
const CARD_PAD_PX: f32 = 14.0;

/// Hero: height, inner padding, gradient angle and its two tones (dark /
/// light theme: lightness, share of the accent's chroma, chroma cap for the
/// primary and the tertiary colour).
const HERO_H_PX: f32 = 112.0;
const HERO_PAD_PX: f32 = 18.0;
const HERO_ANGLE_DEG: f32 = 125.0;
const HERO_DARK: [(f32, f32, f32); 2] = [(0.30, 0.55, 0.075), (0.22, 0.5, 0.06)];
const HERO_LIGHT: [(f32, f32, f32); 2] = [(0.93, 0.35, 0.045), (0.96, 0.3, 0.035)];
/// The big clock, its line height, and where the seconds sit on its
/// baseline.
const BIG_TIME_PX: f32 = 40.0;
const BIG_TIME_LINE_PX: f32 = 44.0;
const SECONDS_BASELINE_PX: f32 = 7.0;
/// Seconds and secondary text alpha.
const MUTED_ALPHA: f32 = 0.7;
/// Gap between the hero's chips.
const CHIP_GAP_PX: f32 = 6.0;
/// Hero chips: height, side padding and gap to their glyph.
const HERO_CHIP_H_PX: f32 = 22.0;
const HERO_CHIP_PAD_PX: f32 = 8.0;
const HERO_CHIP_ICON_GAP_PX: f32 = 5.0;

/// The page-a-day tile: size, corner radius, the coloured month band, the
/// day number and the tile's drop shadow (alpha, blur, y offset).
const TILE_W_PX: f32 = 64.0;
const TILE_H_PX: f32 = 72.0;
const TILE_RADIUS_PX: f32 = 15.0;
const TILE_BAND_PX: f32 = 22.0;
const TILE_MONTH_PX: f32 = 10.5;
const TILE_DAY_PX: f32 = 30.0;
const TILE_SHADOW: (f32, f32, f32) = (0.32, 10.0, 3.0);

/// Month card: the title row, the Hijri month line under it, the
/// navigation buttons, the weekday header and the gap above it.
const TITLE_ROW_PX: f32 = 34.0;
const HIJRI_LINE_PX: f32 = 16.0;
const NAV_BTN_PX: f32 = 28.0;
const NAV_ICON_PX: f32 = 14.0;
const NAV_GAP_PX: f32 = 4.0;
const WEEKDAY_ROW_PX: f32 = 26.0;
const HEAD_GAP_PX: f32 = 2.0;
/// Day discs: width, height (taller with a Hijri number under the day),
/// the gap between weeks and the Hijri number's size.
const DISC_W_PX: f32 = 36.0;
const DISC_H_PX: f32 = 34.0;
const DISC_H_HIJRI_PX: f32 = 42.0;
const DISC_RADIUS_HIJRI_PX: f32 = 13.0;
const WEEK_GAP_PX: f32 = 3.0;
const HIJRI_DAY_PX: f32 = 9.0;
/// Days of the neighbouring months, Hijri numbers and week numbers are
/// drawn this faint.
const OTHER_MONTH_ALPHA: f32 = 0.34;
const HIJRI_ALPHA: f32 = 0.62;
const WEEK_NUMBER_ALPHA: f32 = 0.6;
/// The selected day's ring width and fill.
const RING_PX: f32 = 1.5;
/// Glow under today's disc: alpha, blur, y offset.
const TODAY_GLOW: (f32, f32, f32) = (0.38, 10.0, 2.0);
/// Event dots: size, gap, distance from the disc's bottom, and how many
/// calendars one day shows at most.
const DOT_PX: f32 = 4.0;
const DOT_GAP_PX: f32 = 2.0;
const DOT_BOTTOM_PX: f32 = 3.5;
const MAX_DOTS: usize = 3;
/// ISO week column width.
const WEEK_COL_PX: f32 = 28.0;
/// The month card without Hijri numbers.
const MONTH_H_PX: f32 = CARD_PAD_PX * 2.0 + TITLE_ROW_PX + HEAD_GAP_PX + WEEKDAY_ROW_PX + GRID_WEEKS as f32 * DISC_H_PX + (GRID_WEEKS - 1) as f32 * WEEK_GAP_PX;
/// The panel without an agenda: its size before the first frame.
pub(super) const PANEL_MIN_H_PX: f32 = PAD_PX * 2.0 + HERO_H_PX + GAP_PX + MONTH_H_PX;

/// Agenda: head row, event row, the gap between rows, the empty line, the
/// time column, the calendar stripe and the "now" fill.
const AGENDA_HEAD_PX: f32 = 28.0;
const AGENDA_ROW_PX: f32 = 44.0;
const AGENDA_ROW_GAP_PX: f32 = 2.0;
const AGENDA_EMPTY_PX: f32 = 34.0;
const AGENDA_TIME_W_PX: f32 = 62.0;
const AGENDA_ROW_PAD_PX: f32 = 8.0;
const STRIPE_W_PX: f32 = 3.0;
const STRIPE_H_PX: f32 = 28.0;
const NOW_FILL: (f32, f32) = (0.13, 0.10);
const BADGE_H_PX: f32 = 18.0;
const BADGE_PAD_PX: f32 = 7.0;
const BADGE_TEXT_PX: f32 = 10.5;

// ── Motion & input ───────────────────────────────────────────────────────

/// The month slide: a soft, barely-underdamped spring, the distance a month
/// travels, how much it fades on the way and when it rests.
const SLIDE: SpringConfig = SpringConfig::new(240.0, 28.0, 1.0);
const SLIDE_PX: f32 = 30.0;
const SLIDE_FADE: f32 = 0.9;
const SLIDE_REST: f32 = 0.002;
/// Scroll: pixels per wheel line, the travel that turns a page, and the
/// pause between pages for touchpads (which send a stream of small steps).
const SCROLL_PX_PER_LINE: f32 = 20.0;
const WHEEL_STEP_LINES: f32 = 1.0;
const TOUCHPAD_COOLDOWN: Duration = Duration::from_millis(220);
/// A panel that hasn't drawn for this long was closed: the next frame is a
/// new opening (it ticks every second while open).
const REOPEN_AFTER: Duration = Duration::from_secs(2);
/// A closing panel that shrinks below this morph progress is closed: the
/// next time it grows is a new opening.
const CLOSED_BELOW: f32 = 0.15;

/// Tells a new opening of the panel from the frames of one that is open.
#[derive(Default)]
struct OpenWatch {
    last_frame: Option<Instant>,
    progress: f32,
    /// This opening was already handled.
    seen: bool,
}

impl OpenWatch {
    /// Whether this frame (at `now`, morph `progress`) starts a new opening.
    fn frame(&mut self, now: Instant, progress: f32) -> bool {
        let previous = std::mem::replace(&mut self.progress, progress);
        let gap = self.last_frame.replace(now).is_none_or(|t| now.duration_since(t) > REOPEN_AFTER);
        if gap || (progress < previous && progress < CLOSED_BELOW) {
            self.seen = false;
        }
        if self.seen || progress < previous {
            return false;
        }
        self.seen = true;
        true
    }
}

// ── Navigation state ─────────────────────────────────────────────────────

/// What the calendar shows, shared by every display's panel and by the
/// `calendar goto` command.
pub(crate) struct Nav {
    /// First day of the month on screen.
    view: NaiveDate,
    /// The day whose agenda is shown.
    selected: NaiveDate,
    /// Set by `calendar goto` while the panel is closed: the next opening
    /// keeps that day instead of returning to today.
    hold: bool,
    /// Month slide in months (`±1` easing to `0`).
    slide: SpringValue<f32>,
    clock: FrameClock,
    /// Wheel travel not yet turned into a page, and when the last page
    /// turned.
    wheel: f32,
    wheel_at: Option<Instant>,
    views: ViewSet,
}

thread_local! {
    static NAV: Rc<RefCell<Nav>> = {
        let today = Local::now().date_naive();
        Rc::new(RefCell::new(Nav {
            view: days::month_start(today),
            selected: today,
            hold: false,
            slide: SpringValue::new(0.0, SLIDE).with_rest_epsilon(SLIDE_REST),
            clock: FrameClock::new(),
            wheel: 0.0,
            wheel_at: None,
            views: ViewSet::default(),
        }))
    };
}

fn nav() -> Rc<RefCell<Nav>> {
    NAV.with(Rc::clone)
}

/// The selected day (`calendar goto` resolves relative targets from it).
pub(crate) fn selected() -> NaiveDate {
    nav().borrow().selected
}

/// Shows `date`: selects it and turns to its month, sliding in from the
/// side the month lies on.
pub(crate) fn go_to(date: NaiveDate, cx: &mut App) {
    let n = nav();
    let views = {
        let mut s = n.borrow_mut();
        s.selected = date;
        let month = days::month_start(date);
        if month != s.view {
            let dir = days::months_between(s.view, month).signum() as f32;
            s.view = month;
            s.slide.snap_to(dir);
            s.slide.set_target(0.0);
            s.clock.idle();
        }
        s.views.clone()
    };
    views.notify(cx);
}

/// [`go_to`] for a closed panel: the next opening shows `date`.
pub(crate) fn hold(date: NaiveDate, cx: &mut App) {
    go_to(date, cx);
    nav().borrow_mut().hold = true;
}

/// Turns `months` pages from the selected day.
fn turn(months: i32, cx: &mut App) {
    go_to(days::add_months(selected(), months), cx);
}

/// Moves the selection by `n` days.
fn step_days(n: i64, cx: &mut App) {
    let to = selected() + TimeDelta::days(n);
    go_to(to, cx);
}

/// A new opening: back to today, unless `calendar goto` asked for a day.
fn opened() {
    let n = nav();
    let mut s = n.borrow_mut();
    if std::mem::take(&mut s.hold) {
        return;
    }
    let today = Local::now().date_naive();
    s.selected = today;
    s.view = days::month_start(today);
    s.slide.snap_to(0.0);
    s.clock.idle();
}

/// Scroll over the month: up for the previous month, down for the next.
fn wheel(e: &ScrollEvent, cx: &mut App) {
    let (lines, touchpad) = match e.delta {
        ScrollDelta::Lines(l) => (l.y, false),
        ScrollDelta::Pixels(p) => (f32::from(p.y) / SCROLL_PX_PER_LINE, true),
    };
    let months = {
        let n = nav();
        let mut s = n.borrow_mut();
        if e.touch_phase == TouchPhase::Started {
            s.wheel = 0.0;
        }
        s.wheel += lines;
        if s.wheel.abs() < WHEEL_STEP_LINES {
            return;
        }
        if touchpad && s.wheel_at.is_some_and(|at| at.elapsed() < TOUCHPAD_COOLDOWN) {
            s.wheel = s.wheel.clamp(-WHEEL_STEP_LINES, WHEEL_STEP_LINES);
            return;
        }
        let months = if s.wheel > 0.0 { -1 } else { 1 };
        s.wheel = 0.0;
        s.wheel_at = Some(Instant::now());
        months
    };
    turn(months, cx);
}

/// A keyboard move on the calendar.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Days(i64),
    Months(i32),
    Today,
}

/// What `key` does: arrows move by day and week (left and right follow the
/// reading direction), Page Up / Down by month (a year with Shift), Home
/// or T back to today.
fn key_step(key: &str, shift: bool, rtl: bool) -> Option<Step> {
    let forward = if rtl { -1 } else { 1 };
    let pages = if shift { YEAR_MONTHS } else { 1 };
    Some(match key {
        "left" => Step::Days(-forward),
        "right" => Step::Days(forward),
        "up" => Step::Days(-(WEEK_DAYS as i64)),
        "down" => Step::Days(WEEK_DAYS as i64),
        "pageup" => Step::Months(-pages),
        "pagedown" => Step::Months(pages),
        "home" | "t" => Step::Today,
        _ => return None,
    })
}

/// Keyboard navigation; `true` when the key was used.
fn key(e: &KeyDownEvent, cx: &mut App) -> bool {
    let Some(step) = key_step(e.keystroke.key.as_str(), e.keystroke.modifiers.shift, crate::bidi::rtl()) else { return false };
    match step {
        Step::Days(n) => step_days(n, cx),
        Step::Months(n) => turn(n, cx),
        Step::Today => go_to(Local::now().date_naive(), cx),
    }
    true
}

// ── Cached labels ────────────────────────────────────────────────────────

/// Display options read once per frame.
#[derive(Clone, Copy, PartialEq)]
struct Opts {
    week_start: u32,
    eastern: bool,
    hijri: bool,
    hijri_offset: i64,
    weekend: [bool; WEEK_DAYS],
    arabic_ui: bool,
    lang: mandara_i18n::Lang,
    locale: chrono::Locale,
}

impl Opts {
    fn of(cfg: &ShellConfig, cx: &App) -> Self {
        let week_start = week_start_days(cx);
        let arabic_ui = cfg.general.locale.is_arabic();
        Self {
            week_start,
            eastern: cfg.datetime.eastern_numerals,
            hijri: cfg.datetime.hijri_dates,
            hijri_offset: cfg.plugins.setting(crate::plugins::prayer::ID, "hijri_offset", 0_i64),
            weekend: cfg.datetime.weekend.days_from_monday(cfg.general.week_start == mandara_config::WeekStart::Saturday, arabic_ui),
            arabic_ui,
            lang: mandara_i18n::language(),
            locale: fmt::locale(),
        }
    }

    fn num(&self, s: &str) -> SharedString {
        days::digits(s, self.eastern).into()
    }

    /// A Hijri date as `day month year`.
    fn hijri_text(&self, (y, m, d): (i64, usize, i64)) -> String {
        format!("{} {} {}", days::digits(&d.to_string(), self.eastern), days::hijri_month_name(m, self.arabic_ui), days::digits(&y.to_string(), self.eastern))
    }
}

/// The hero's labels (re-formatted once a minute, the seconds once a
/// second).
#[derive(Default)]
struct HeroText {
    key: Option<(i64, Opts, &'static str)>,
    hh_mm: SharedString,
    weekday: SharedString,
    arabic_weekday: bool,
    tile_month: SharedString,
    tile_day: SharedString,
    week: SharedString,
    second_chip: SharedString,
    second_icon: &'static str,
    second_arabic: bool,
    seconds_key: Option<(i64, bool)>,
    seconds: SharedString,
}

impl HeroText {
    fn update(&mut self, now: &DateTime<Local>, o: Opts, big_clock: &'static str) {
        let key = (fmt::local_stamp(now, false), o, big_clock);
        if self.key != Some(key) {
            self.key = Some(key);
            self.hh_mm = o.num(&fmt::format_with(now, big_clock, "%H:%M"));
            // Arabic names are built in, unless a date language is chosen.
            self.arabic_weekday = o.arabic_ui && fmt::locale_is_default();
            self.weekday = if self.arabic_weekday { fmt::weekday_ar(now).into() } else { fmt::localized(now, "%A").into() };
            self.tile_month = fmt::localized(now, "%b").trim_end_matches('.').to_uppercase().into();
            self.tile_day = o.num(&now.day().to_string());
            self.week = o.num(&tf("Week {week}", &[("week", &now.iso_week().week())]));
            let today = now.date_naive();
            if o.hijri {
                self.second_chip = o.hijri_text(days::hijri_of(today, o.hijri_offset)).into();
                self.second_icon = "crescent";
                self.second_arabic = o.arabic_ui;
            } else {
                self.second_chip = o.num(&tf("Day {n}", &[("n", &now.ordinal())]));
                self.second_icon = "sun";
                self.second_arabic = false;
            }
        }
        let skey = (now.timestamp(), o.eastern);
        if self.seconds_key != Some(skey) {
            self.seconds_key = Some(skey);
            self.seconds = o.num(&fmt::seconds(now));
        }
    }
}

/// One day on the grid.
struct GridCell {
    date: NaiveDate,
    label: SharedString,
    hijri: SharedString,
    in_month: bool,
    weekend: bool,
}

/// The month on screen (rebuilt when it, the week start or a display
/// option changes).
#[derive(Default)]
struct Grid {
    key: Option<(NaiveDate, Opts)>,
    title: SharedString,
    hijri_title: Option<SharedString>,
    /// Weekday headers with their weekend flag, in grid order.
    weekdays: Vec<(SharedString, bool)>,
    cells: Vec<GridCell>,
    /// ISO week of each grid row.
    weeks: Vec<SharedString>,
}

impl Grid {
    fn update(&mut self, view: NaiveDate, o: Opts) {
        if self.key == Some((view, o)) {
            return;
        }
        self.key = Some((view, o));
        self.title = o.num(&fmt::localized(&view, t("%B %Y")));
        let dates = days::grid_days(view, o.week_start);
        // Weekday headers from the date language's abbreviations; English
        // keeps its single-letter style.
        let english = o.lang == mandara_i18n::Lang::En && fmt::locale_is_default();
        self.weekdays = dates[..WEEK_DAYS]
            .iter()
            .map(|d| {
                let abbr = fmt::localized(d, "%a");
                let label: SharedString = if english { abbr.chars().take(1).collect::<String>().into() } else { abbr.into() };
                (label, o.weekend[d.weekday().num_days_from_monday() as usize])
            })
            .collect();
        self.cells = dates
            .iter()
            .map(|d| GridCell {
                date: *d,
                label: o.num(&d.day().to_string()),
                hijri: if o.hijri { o.num(&days::hijri_of(*d, o.hijri_offset).2.to_string()) } else { SharedString::default() },
                in_month: d.month() == view.month(),
                weekend: o.weekend[d.weekday().num_days_from_monday() as usize],
            })
            .collect();
        self.weeks = dates
            .chunks(WEEK_DAYS)
            .map(|row| {
                let monday = row.iter().find(|d| d.weekday() == Weekday::Mon).unwrap_or(&row[0]);
                o.num(&monday.iso_week().week().to_string())
            })
            .collect();
        self.hijri_title = o.hijri.then(|| {
            let in_month: Vec<NaiveDate> = dates.iter().copied().filter(|d| d.month() == view.month()).collect();
            let (first, last) = (in_month.first().copied().unwrap_or(view), in_month.last().copied().unwrap_or(view));
            let (a, b) = (days::hijri_of(first, o.hijri_offset), days::hijri_of(last, o.hijri_offset));
            let name = |m: usize| days::hijri_month_name(m, o.arabic_ui);
            let year = |y: i64| days::digits(&y.to_string(), o.eastern);
            if a.0 != b.0 {
                format!("{} {} – {} {}", name(a.1), year(a.0), name(b.1), year(b.0))
            } else if a.1 != b.1 {
                format!("{} – {} {}", name(a.1), name(b.1), year(b.0))
            } else {
                format!("{} {}", name(a.1), year(a.0))
            }
            .into()
        });
    }
}

/// Event dots per grid cell, in each calendar's colour.
#[derive(Default)]
struct Dots {
    key: Option<(u64, NaiveDate, bool, Vec<String>)>,
    cells: Vec<SmallVec<[Option<Color>; MAX_DOTS]>>,
}

/// Midnight starting `day`, local time.
fn local_midnight(day: NaiveDate) -> Option<DateTime<Local>> {
    day.and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(Local).earliest())
}

/// The enabled calendars' colours, in load order (event `source` indexes).
fn source_colors(cfg: &ShellConfig) -> Vec<Option<Color>> {
    cfg.calendar.sources.iter().filter(|s| s.enabled).map(|s| Color::from_hex(s.color.trim()).ok()).collect()
}

impl Dots {
    fn update(&mut self, first: NaiveDate, cfg: &ShellConfig, cx: &App) {
        let generation = crate::calendar::store(cx).map_or(0, |s| s.read(cx).generation);
        let key = (generation, first, cfg.calendar.enabled, cfg.calendar.sources.iter().filter(|s| s.enabled).map(|s| s.color.clone()).collect());
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        self.cells = vec![SmallVec::new(); GRID_DAYS];
        let Some(store) = crate::calendar::store(cx).filter(|_| cfg.calendar.enabled) else { return };
        let end = first + TimeDelta::days(GRID_DAYS as i64);
        let (Some(from), Some(to)) = (local_midnight(first), local_midnight(end)) else { return };
        let colors = source_colors(cfg);
        for o in store.read(cx).between(from, to) {
            let color = colors.get(o.source).copied().flatten();
            // Every day the event touches (multi-day events too).
            let last = (o.end - TimeDelta::seconds(1)).date_naive().max(o.start.date_naive());
            let mut d = o.start.date_naive().max(first);
            while d <= last && d < end {
                let i = days::days_from(first, d) as usize;
                let dots = &mut self.cells[i];
                if dots.len() < MAX_DOTS && !dots.contains(&color) {
                    dots.push(color);
                }
                let Some(next) = d.succ_opt() else { break };
                d = next;
            }
        }
    }
}

/// One event in the agenda.
struct AgendaRow {
    time: SharedString,
    sub: SharedString,
    title: SharedString,
    detail: SharedString,
    color: Option<Color>,
    now: bool,
}

/// The agenda card's content.
#[derive(Default)]
struct Agenda {
    key: Option<(u64, i64, NaiveDate, Opts, (bool, bool, u32, i64))>,
    show: bool,
    title: SharedString,
    badge: Option<SharedString>,
    rows: Vec<AgendaRow>,
    empty: SharedString,
}

impl Agenda {
    fn update(&mut self, now: &DateTime<Local>, selected: NaiveDate, o: Opts, cfg: &ShellConfig, cx: &App) {
        let store = crate::calendar::store(cx);
        let generation = store.as_ref().map_or(0, |s| s.read(cx).generation);
        let limit = cfg.plugins.setting(crate::plugins::clock::ID, "agenda_rows", DEFAULT_AGENDA_ROWS).clamp(AGENDA_ROWS_RANGE.0, AGENDA_ROWS_RANGE.1);
        let opts = (cfg.calendar.enabled, cfg.calendar.hide_past, cfg.calendar.agenda_days, limit);
        let key = (generation, fmt::local_stamp(now, false), selected, o, opts);
        if self.key == Some(key) {
            return;
        }
        self.key = Some(key);
        let today = now.date_naive();
        let is_today = selected == today;
        let has_sources = cfg.calendar.enabled && store.as_ref().is_some_and(|s| s.read(cx).has_sources());
        // Today is "what's next"; any other day is that day.
        let (from, to) = if is_today { (today, today + TimeDelta::days(i64::from(cfg.calendar.agenda_days.max(1)))) } else { (selected, selected + TimeDelta::days(1)) };
        let events: Vec<Occurrence> = match (has_sources, store, local_midnight(from), local_midnight(to)) {
            (true, Some(store), Some(from), Some(to)) => {
                store.read(cx).between(from, to).into_iter().filter(|e| !(is_today && cfg.calendar.hide_past && e.end <= *now)).take(usize::try_from(limit).unwrap_or(0)).collect()
            }
            _ => Vec::new(),
        };
        let colors = source_colors(cfg);
        let pattern = cfg.datetime.automatic_time(cfg.general.clock_24h, false);
        let time = |at: &DateTime<Local>| o.num(&fmt::format_with(at, &pattern, "%H:%M"));
        self.rows = events
            .iter()
            .map(|e| {
                let day = e.start.date_naive().max(from);
                // Rows of a later day say which; the selected day's say until when.
                let sub: SharedString = if day != selected {
                    if days::days_from(today, day) == 1 { t("Tomorrow").into() } else { o.num(&fmt::localized(&day, t("%a %-d %b"))) }
                } else if e.all_day {
                    SharedString::default()
                } else {
                    time(&e.end)
                };
                AgendaRow {
                    time: if e.all_day { t("All day").into() } else { time(&e.start) },
                    sub,
                    title: e.summary.clone().into(),
                    detail: e.location.clone().into(),
                    color: colors.get(e.source).copied().flatten(),
                    now: !e.all_day && e.start <= *now && *now < e.end,
                }
            })
            .collect();
        self.show = has_sources || !is_today;
        if is_today {
            self.title = t("Upcoming").into();
            self.badge = None;
        } else {
            self.title = o.num(&fmt::localized(&selected, t("%A, %-d %B")));
            self.badge = Some(o.num(&days::relative_label(today, selected)));
        }
        self.empty = if !has_sources {
            t("No calendars yet: add one in Studio → Date, time & location").into()
        } else if is_today {
            t("Nothing coming up").into()
        } else {
            t("Nothing scheduled").into()
        };
    }

    /// The card's height.
    fn height(&self) -> f32 {
        let n = self.rows.len();
        let body = if n == 0 { AGENDA_EMPTY_PX } else { n as f32 * AGENDA_ROW_PX + (n - 1) as f32 * AGENDA_ROW_GAP_PX };
        CARD_PAD_PX * 2.0 + AGENDA_HEAD_PX + body
    }
}

/// Everything one panel caches between frames.
#[derive(Default)]
struct Cache {
    hero: HeroText,
    grid: Grid,
    dots: Dots,
    agenda: Agenda,
}

// ── View ─────────────────────────────────────────────────────────────────

/// Hero backdrop tone from a theme colour: `(lightness, chroma share, cap)`.
fn tone(c: Color, (l, share, cap): (f32, f32, f32)) -> Color {
    c.with_lightness(l).with_chroma((c.chroma() * share).min(cap))
}

/// The family numbers are drawn in: Eastern Arabic digits take the Arabic
/// face (Latin faces lack them or fall back to a mismatched one).
fn num_font(o: &Opts, cx: &App) -> Option<SharedString> {
    o.eastern.then(|| fonts(cx).arabic.clone())
}

/// The month card's height for these options.
fn month_height(o: &Opts) -> f32 {
    let title = TITLE_ROW_PX + if o.hijri { HIJRI_LINE_PX } else { 0.0 };
    let disc = if o.hijri { DISC_H_HIJRI_PX } else { DISC_H_PX };
    CARD_PAD_PX * 2.0 + title + HEAD_GAP_PX + WEEKDAY_ROW_PX + GRID_WEEKS as f32 * disc + (GRID_WEEKS - 1) as f32 * WEEK_GAP_PX
}

/// A small chip on the hero glass: glyph and a tabular label.
fn hero_chip(icon_name: &'static str, label: SharedString, accent: Color, arabic: Option<SharedString>, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(HERO_CHIP_H_PX))
        .px(px(HERO_CHIP_PAD_PX))
        .flex_none()
        .rounded_full()
        .bg(wash(WASH_SOFT, palette).hsla())
        .row()
        .items_center()
        .gap(px(HERO_CHIP_ICON_GAP_PX))
        .text_size(px(text::LABEL))
        .font_features(tabular_nums())
        .text_color(palette.on_surface_variant.hsla())
        .whitespace_nowrap()
        .when_some(arabic, |el, f| el.font_family(f))
        .child(icon(icon_name, SMALL_ICON_PX, accent))
        .child(label)
        .into_any_element()
}

fn hero(h: &HeroText, palette: &MandaraPalette, radius: f32, nf: Option<SharedString>, cx: &App) -> AnyElement {
    let f = fonts(cx);
    let display = nf.clone().unwrap_or_else(|| f.display.clone());
    let accent = readable(palette.primary, palette);
    let tones = if palette.is_dark { HERO_DARK } else { HERO_LIGHT };
    let muted = palette.on_surface_variant.with_alpha(MUTED_ALPHA).hsla();
    let shadow = palette.specular_rim_shadow.with_alpha(TILE_SHADOW.0);
    let tile_bg = if palette.is_dark { palette.surface_container_high } else { palette.surface_base };
    let band = oklab_gradient(if crate::bidi::rtl() { ANGLE_RIGHT_DEG + HALF_TURN_DEG } else { ANGLE_RIGHT_DEG }, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()));
    let tile = div()
        .w(px(TILE_W_PX))
        .h(px(TILE_H_PX))
        .flex_none()
        .rounded(px(crate::style::radius(TILE_RADIUS_PX)))
        .clip_rounded()
        .bg(tile_bg.hsla())
        .shadow({
            let mut s = rim(palette);
            s.push(glow(shadow, TILE_SHADOW.1, 0.0, TILE_SHADOW.2));
            s
        })
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(TILE_BAND_PX))
                .w_full()
                .bg(band)
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TILE_MONTH_PX))
                .font_weight(FontWeight::BOLD)
                .text_color(palette.on_primary.hsla())
                .child(h.tile_month.clone()),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .font_family(display.clone())
                .font_features(tabular_nums())
                .text_size(px(TILE_DAY_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.on_surface.hsla())
                .child(h.tile_day.clone()),
        );
    let clock = div()
        .ltr_row()
        .items_end()
        .gap(px(tokens::XS))
        .font_features(tabular_nums())
        .child(div().font_family(display).text_size(px(BIG_TIME_PX)).line_height(px(BIG_TIME_LINE_PX)).font_weight(FontWeight::LIGHT).child(h.hh_mm.clone()))
        .child(div().pb(px(SECONDS_BASELINE_PX)).when_some(nf.clone(), |el, f| el.font_family(f)).text_size(px(text::TITLE)).text_color(muted).child(h.seconds.clone()));
    div()
        .relative()
        .h(px(HERO_H_PX))
        .rounded(px(radius))
        .clip_rounded()
        .bg(oklab_gradient(HERO_ANGLE_DEG, tone(palette.primary, tones[0]), tone(palette.tertiary, tones[1])))
        .shadow(rim(palette))
        .px(px(HERO_PAD_PX))
        .row()
        .items_center()
        .gap(px(tokens::LG))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .cross_start()
                .child(
                    div()
                        .when(h.arabic_weekday, |el| el.font_family(f.arabic.clone()))
                        .text_size(px(text::TITLE))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(accent.hsla())
                        .child(h.weekday.clone()),
                )
                .child(clock)
                .child(
                    div()
                        .row()
                        .gap(px(CHIP_GAP_PX))
                        .child(hero_chip("calendar", h.week.clone(), accent, nf.clone(), palette))
                        .child(hero_chip(h.second_icon, h.second_chip.clone(), accent, h.second_arabic.then(|| f.arabic.clone()).or(nf), palette)),
                ),
        )
        .child(tile)
        .into_any_element()
}

/// A round navigation button.
fn nav_button(id: &'static str, glyph: &'static str, palette: &MandaraPalette, on_click: impl Fn(&mut App) + 'static) -> AnyElement {
    let hover = wash(WASH_SOFT, palette);
    div()
        .id(id)
        .size(px(NAV_BTN_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(wash(WASH_FAINT, palette).hsla())
        .hover(move |s| s.bg(hover.hsla()))
        .child(icon(crate::bidi::dir_icon(glyph), NAV_ICON_PX, palette.on_surface))
        .on_click(move |_, _, cx| on_click(cx))
        .into_any_element()
}

/// What the grid needs to draw a frame.
struct GridFrame<'a> {
    grid: &'a Grid,
    dots: &'a Dots,
    today: NaiveDate,
    selected: NaiveDate,
    view: NaiveDate,
    slide: f32,
    week_numbers: bool,
    opts: Opts,
}

fn month_card(g: GridFrame<'_>, palette: &MandaraPalette, radius: f32, cx: &App) -> AnyElement {
    let f = fonts(cx);
    let accent = readable(palette.primary, palette);
    let weekend_ink = readable(palette.tertiary, palette);
    let rtl = crate::bidi::rtl();
    let shift = px(g.slide * SLIDE_PX * if rtl { -1.0 } else { 1.0 });
    let fade = (1.0 - g.slide.abs() * SLIDE_FADE).clamp(0.0, 1.0);
    let o = g.opts;
    let disc_h = if o.hijri { DISC_H_HIJRI_PX } else { DISC_H_PX };
    let away = g.selected != g.today || g.view != days::month_start(g.today);
    let nf = num_font(&o, cx);

    let title = div()
        .h(px(TITLE_ROW_PX))
        .row()
        .items_center()
        .gap(px(NAV_GAP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .relative()
                .left(shift)
                .opacity(fade)
                .when_some(nf.clone(), |el, f| el.font_family(f))
                .text_size(px(text::TITLE))
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .truncate()
                .child(g.grid.title.clone()),
        )
        .when(away, |el| el.child(pill_button("cal-today", Some("undo"), t("Today"), accent, palette, |_, cx| go_to(Local::now().date_naive(), cx))))
        .child(nav_button("cal-prev", "chevron-left", palette, |cx| turn(-1, cx)))
        .child(nav_button("cal-next", "chevron-right", palette, |cx| turn(1, cx)));
    // The Hijri months the page spans, across the card's full width.
    let hijri_line = g.grid.hijri_title.clone().map(|h| {
        div()
            .h(px(HIJRI_LINE_PX))
            .relative()
            .left(shift)
            .opacity(fade)
            .flex()
            .items_center()
            .when(o.arabic_ui || o.eastern, |el| el.font_family(f.arabic.clone()))
            .text_size(px(text::LABEL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(accent.with_alpha(MUTED_ALPHA).hsla())
            .whitespace_nowrap()
            .truncate()
            .child(h)
    });

    let header = div()
        .h(px(WEEKDAY_ROW_PX))
        .mt(px(HEAD_GAP_PX))
        .row()
        .items_center()
        .when(g.week_numbers, |el| el.child(div().w(px(WEEK_COL_PX)).flex_none()))
        .children(g.grid.weekdays.iter().map(|(label, weekend)| {
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .justify_center()
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if *weekend { weekend_ink.with_alpha(MUTED_ALPHA) } else { palette.on_surface_variant.with_alpha(MUTED_ALPHA) }.hsla())
                .whitespace_nowrap()
                .child(label.clone())
        }));

    let today_fill = oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()));
    let selected_fill = accent.with_alpha(alpha(TINT_FILL, palette));
    let hover_fill = wash(WASH_SOFT, palette);
    let selected_hover = accent.with_alpha(alpha(TINT_HOVER, palette));
    let cells = g.grid.cells.iter().enumerate().map(|(i, c)| {
        let dots: &[Option<Color>] = g.dots.cells.get(i).map_or(&[], |d| d.as_slice());
        let (is_today, is_sel) = (c.date == g.today, c.date == g.selected);
        let ink = if is_today {
            palette.on_primary
        } else if c.weekend {
            weekend_ink
        } else {
            palette.on_surface
        };
        let ink = if c.in_month || is_today { ink } else { ink.with_alpha(OTHER_MONTH_ALPHA) };
        let dot_ink = if is_today { Some(palette.on_primary) } else { None };
        let date = c.date;
        let disc = div()
            .id(("cal-day", i))
            .relative()
            .w(px(DISC_W_PX))
            .h(px(disc_h))
            .when(o.hijri, |el| el.rounded(px(crate::style::radius(DISC_RADIUS_HIJRI_PX))))
            .when(!o.hijri, |el| el.rounded_full())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .font_features(tabular_nums())
            .when_some(nf.clone(), |el, f| el.font_family(f))
            .text_color(ink.hsla())
            .when(is_today, |el| el.bg(today_fill).font_weight(FontWeight::SEMIBOLD).shadow(vec![glow(palette.primary.with_alpha(TODAY_GLOW.0), TODAY_GLOW.1, 0.0, TODAY_GLOW.2)]))
            .when(is_sel && !is_today, |el| el.bg(selected_fill.hsla()).border(px(RING_PX)).border_color(accent.hsla()).font_weight(FontWeight::SEMIBOLD).hover(move |s| s.bg(selected_hover.hsla())))
            .when(!is_sel && !is_today, |el| el.hover(move |s| s.bg(hover_fill.hsla())))
            .child(div().text_size(px(text::BODY_S)).line_height(px(text::BODY_S + tokens::XS)).child(c.label.clone()))
            .when(o.hijri, |el| el.child(div().text_size(px(HIJRI_DAY_PX)).line_height(px(HIJRI_DAY_PX + metrics_nudge())).text_color(ink.with_alpha(ink.alpha() * HIJRI_ALPHA).hsla()).child(c.hijri.clone())))
            .when(!dots.is_empty(), |el| {
                el.child(div().absolute().bottom(px(DOT_BOTTOM_PX)).left_0().right_0().flex().justify_center().gap(px(DOT_GAP_PX)).children(dots.iter().map(|d| {
                    let col = dot_ink.unwrap_or_else(|| d.map_or(accent, |d| readable(d, palette)));
                    div().size(px(DOT_PX)).rounded_full().bg(col.with_alpha(col.alpha() * if c.in_month || is_today { 1.0 } else { OTHER_MONTH_ALPHA }).hsla())
                })))
            })
            .on_click(move |_, _, cx| go_to(date, cx));
        div().flex_1().min_w(px(0.0)).flex().justify_center().child(disc)
    });
    let mut cells = cells.collect::<Vec<_>>().into_iter();
    let rows = (0..GRID_WEEKS).map(|_| div().row().h(px(disc_h)).children(cells.by_ref().take(WEEK_DAYS)));
    let weeks = g.week_numbers.then(|| {
        div()
            .w(px(WEEK_COL_PX))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(WEEK_GAP_PX))
            .rounded(px(crate::style::radius(tokens::MD)))
            .bg(wash(WASH_FAINT, palette).hsla())
            .when_some(nf.clone(), |el, f| el.font_family(f))
            .children(g.grid.weeks.iter().map(|w| {
                div()
                    .h(px(disc_h))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(text::LABEL))
                    .font_features(tabular_nums())
                    .text_color(palette.on_surface_variant.with_alpha(WEEK_NUMBER_ALPHA).hsla())
                    .child(w.clone())
            }))
    });
    let body = div()
        .row()
        .child(div().when_some(weeks, |el, w| el.child(w)))
        .child(div().flex_1().min_w(px(0.0)).relative().left(shift).opacity(fade).flex().flex_col().gap(px(WEEK_GAP_PX)).children(rows));

    glass_card(palette)
        .id("cal-month")
        .rounded(px(radius))
        .p(px(CARD_PAD_PX))
        .flex()
        .flex_col()
        .on_scroll(|e, _, cx| wheel(e, cx))
        .child(title)
        .when_some(hijri_line, |el, h| el.child(h))
        .child(header)
        .child(body)
        .into_any_element()
}

/// The small extra line height under day numbers.
const fn metrics_nudge() -> f32 {
    crate::kit::metrics::NUDGE
}

fn agenda_card(a: &Agenda, palette: &MandaraPalette, radius: f32, nf: Option<SharedString>) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let muted = palette.on_surface_variant.hsla();
    let head = div()
        .h(px(AGENDA_HEAD_PX))
        .row()
        .items_center()
        .gap(px(tokens::SM))
        .when_some(nf.clone(), |el, f| el.font_family(f))
        .child(div().flex_1().min_w(px(0.0)).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().truncate().child(a.title.clone()))
        .when_some(a.badge.clone(), |el, b| el.child(badge(b, accent, palette)));
    let body = if a.rows.is_empty() {
        div()
            .h(px(AGENDA_EMPTY_PX))
            .row()
            .items_center()
            .gap(px(tokens::SM))
            .text_size(px(text::BODY_S))
            .text_color(muted)
            .child(icon("calendar", SMALL_ICON_PX, palette.on_surface_variant))
            .child(div().min_w(px(0.0)).truncate().child(a.empty.clone()))
    } else {
        div().flex().flex_col().gap(px(AGENDA_ROW_GAP_PX)).children(a.rows.iter().map(|r| agenda_row(r, accent, palette, nf.clone())))
    };
    glass_card(palette).rounded(px(radius)).p(px(CARD_PAD_PX)).flex().flex_col().child(head).child(body).into_any_element()
}

fn badge(label: SharedString, tint: Color, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .bg(tint.with_alpha(alpha(TINT_FILL, palette)).hsla())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .font_features(tabular_nums())
        .text_color(tint.hsla())
        .whitespace_nowrap()
        .child(label)
        .into_any_element()
}

fn agenda_row(r: &AgendaRow, accent: Color, palette: &MandaraPalette, nf: Option<SharedString>) -> AnyElement {
    let stripe = r.color.map_or(accent, |c| readable(c, palette));
    let muted = palette.on_surface_variant.hsla();
    div()
        .h(px(AGENDA_ROW_PX))
        .px(px(AGENDA_ROW_PAD_PX))
        .rounded(px(crate::style::radius(tokens::MD)))
        .when(r.now, |el| el.bg(stripe.with_alpha(alpha(NOW_FILL, palette)).hsla()))
        .row()
        .items_center()
        .gap(px(tokens::SM + crate::kit::metrics::NUDGE))
        .child(
            div()
                .w(px(AGENDA_TIME_W_PX))
                .flex_none()
                .flex()
                .flex_col()
                .cross_start()
                .font_features(tabular_nums())
                .when_some(nf, |el, f| el.font_family(f))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().truncate().child(r.time.clone()))
                .when(!r.sub.is_empty(), |el| el.child(div().text_size(px(text::LABEL)).text_color(muted).whitespace_nowrap().truncate().child(r.sub.clone()))),
        )
        .child(div().w(px(STRIPE_W_PX)).h(px(STRIPE_H_PX)).flex_none().rounded_full().bg(stripe.hsla()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).whitespace_nowrap().truncate().child(r.title.clone()))
                .when(!r.detail.is_empty(), |el| el.child(div().text_size(px(text::LABEL)).text_color(muted).whitespace_nowrap().truncate().child(r.detail.clone()))),
        )
        .when(r.now, |el| el.child(badge(t("Now").into(), stripe, palette)))
        .into_any_element()
}

/// Builds the panel's render function. `size` receives the height the
/// content needs (the host morphs to it).
pub(super) fn view(p: PanelCx, size_cell: Rc<Cell<Size<Pixels>>>, focus: FocusHandle) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement + 'static {
    let cache = RefCell::new(Cache::default());
    let watch = RefCell::new(OpenWatch::default());
    move |palette, window, cx| {
        let now = Local::now();
        redraw_in(until_next_tick(&now, true), window, cx);
        let frame = Instant::now();
        // A new opening: back to today, and take the keyboard.
        if watch.borrow_mut().frame(frame, p.progress()) {
            opened();
            focus.focus(window, cx);
        }
        let n = nav();
        let (view, selected, slide) = {
            let mut s = n.borrow_mut();
            s.views.track(window);
            if s.slide.is_animating() {
                let dt = s.clock.tick(frame);
                s.slide.step(dt);
                if s.slide.is_animating() {
                    window.request_animation_frame();
                } else {
                    s.clock.idle();
                }
            }
            (s.view, s.selected, s.slide.value())
        };

        let cfg = crate::state::config(cx).read(cx).snapshot();
        let o = Opts::of(&cfg, cx);
        let big_clock = cfg.datetime.big_clock_pattern(cfg.general.clock_24h);
        let mut c = cache.borrow_mut();
        let c = &mut *c;
        c.hero.update(&now, o, big_clock);
        c.grid.update(view, o);
        c.dots.update(days::grid_start(view, o.week_start), &cfg, cx);
        c.agenda.update(&now, selected, o, &cfg, cx);

        let week_numbers = cfg.datetime.week_numbers;
        let agenda_h = if c.agenda.show { GAP_PX + c.agenda.height() } else { 0.0 };
        let want = size(px(PANEL_W_PX + if week_numbers { WEEK_COL_PX } else { 0.0 }), px(PAD_PX * 2.0 + HERO_H_PX + GAP_PX + month_height(&o) + agenda_h));
        if size_cell.get() != want {
            size_cell.set(want);
        }
        let radius = PanelShape::of(cx).inner(PAD_PX);
        let today = now.date_naive();
        let frame = GridFrame { grid: &c.grid, dots: &c.dots, today, selected, view, slide, week_numbers, opts: o };
        div()
            .id("clock-panel")
            .size_full()
            .p(px(PAD_PX))
            .flex()
            .flex_col()
            .gap(px(GAP_PX))
            .track_focus(&focus)
            .on_key_down(|e: &KeyDownEvent, _, cx| {
                if key(e, cx) {
                    cx.stop_propagation();
                }
            })
            .child(reveal(&p, 0, hero(&c.hero, palette, radius, num_font(&o, cx), cx)))
            .child(reveal(&p, 1, month_card(frame, palette, radius, cx)))
            .when(c.agenda.show, |el| el.child(reveal(&p, 2, agenda_card(&c.agenda, palette, radius, num_font(&o, cx)))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_add_up() {
        let mut a = Agenda::default();
        assert_eq!(a.height(), CARD_PAD_PX * 2.0 + AGENDA_HEAD_PX + AGENDA_EMPTY_PX, "an empty day keeps one line");
        a.rows = (0..3).map(|_| AgendaRow { time: "".into(), sub: "".into(), title: "".into(), detail: "".into(), color: None, now: false }).collect();
        assert_eq!(a.height(), CARD_PAD_PX * 2.0 + AGENDA_HEAD_PX + AGENDA_ROW_PX * 3.0 + AGENDA_ROW_GAP_PX * 2.0);
    }

    #[test]
    fn an_opening_is_seen_once() {
        let mut w = OpenWatch::default();
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        assert!(w.frame(at(0), 0.0), "the first frame opens");
        assert!(!w.frame(at(10), 0.1) && !w.frame(at(20), 0.6) && !w.frame(at(30), 1.0), "the rest of the morph does not");
        assert!(!w.frame(at(1030), 1.0), "nor does the once-a-second tick");
        // Closing, then opening again straight away.
        assert!(!w.frame(at(1040), 0.5) && !w.frame(at(1050), 0.05));
        assert!(w.frame(at(1100), 0.1), "growing again is a new opening");
        assert!(!w.frame(at(1110), 0.3));
        // Closed for a while: the next frame opens even at full size.
        assert!(w.frame(at(9000), 1.0));
    }

    #[test]
    fn keys_follow_the_reading_direction() {
        assert_eq!(key_step("right", false, false), Some(Step::Days(1)));
        assert_eq!(key_step("right", false, true), Some(Step::Days(-1)), "right goes back in a right-to-left calendar");
        assert_eq!(key_step("down", false, false), Some(Step::Days(7)));
        assert_eq!(key_step("pageup", false, false), Some(Step::Months(-1)));
        assert_eq!(key_step("pagedown", true, false), Some(Step::Months(YEAR_MONTHS)));
        assert_eq!(key_step("home", false, false), Some(Step::Today));
        assert_eq!(key_step("escape", false, false), None, "Esc still closes the panel");
    }

    fn opts(eastern: bool, hijri: bool) -> Opts {
        Opts {
            week_start: 0,
            eastern,
            hijri,
            hijri_offset: 0,
            weekend: mandara_config::desktop::Weekend::Auto.days_from_monday(false, false),
            arabic_ui: false,
            lang: mandara_i18n::Lang::En,
            locale: chrono::Locale::en_US,
        }
    }

    #[test]
    fn grid_labels_weeks_and_weekends() {
        let mut g = Grid::default();
        let nov = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        g.update(nov, opts(false, false));
        assert_eq!(g.cells.len(), GRID_DAYS);
        assert_eq!(g.title.as_ref(), "November 2026");
        assert_eq!(g.weekdays.iter().map(|(l, _)| l.as_ref()).collect::<String>(), "MTWTFSS");
        assert_eq!(g.weekdays.iter().filter(|(_, w)| *w).count(), 2);
        // Mon 26 Oct opens the page; Sun 1 Nov closes its first week.
        assert_eq!((g.cells[0].label.as_ref(), g.cells[0].in_month), ("26", false));
        assert!(g.cells[6].in_month && g.cells[6].weekend);
        assert_eq!(g.weeks.first().map(|w| w.as_ref()), Some("44"));
        assert!(g.hijri_title.is_none() && g.cells[6].hijri.is_empty());
        g.update(nov, opts(true, true));
        assert_eq!(g.cells[6].label.as_ref(), "١", "Eastern Arabic numerals");
        assert_eq!(g.title.as_ref(), "November ٢٠٢٦");
        assert!(!g.cells[6].hijri.is_empty());
        assert!(g.hijri_title.as_ref().is_some_and(|t| t.contains("Jumada")), "{:?}", g.hijri_title);
    }

    #[test]
    fn navigation_turns_months_and_holds_for_goto() {
        let n = nav();
        let start = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        {
            let mut s = n.borrow_mut();
            s.selected = start;
            s.view = days::month_start(start);
        }
        // `go_to` without an App: mirror its state change.
        let next = days::add_months(start, 1);
        {
            let mut s = n.borrow_mut();
            let dir = days::months_between(s.view, days::month_start(next)).signum() as f32;
            s.selected = next;
            s.view = days::month_start(next);
            s.slide.snap_to(dir);
            s.slide.set_target(0.0);
            assert!(s.slide.is_animating(), "a new month slides in");
            s.hold = true;
        }
        opened();
        assert_eq!(selected(), next, "an opening after goto keeps the day");
        opened();
        assert_eq!(selected(), Local::now().date_naive(), "the next opening is back to today");
    }
}
