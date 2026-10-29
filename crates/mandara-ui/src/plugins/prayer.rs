//! Prayer times plugin: next prayer + countdown in the bar; a dashboard
//! panel with a live countdown hero (dotted progress ring between the
//! previous and next prayer), a bilingual timeline of the day with a "now"
//! rail, Hijri + Gregorian dates, daylight length and Qibla bearing.
//! Location and method come from `[plugins.settings.prayer-times]`
//! (defaults: El-Mandara, Egyptian method). Opt-in: shipped but disabled
//! until enabled in Studio → Plugins.
//!
//! The three days of astronomical times around today are computed once per
//! day and settings change, and every label is formatted once per minute
//! (the countdown once per second), so neither the bar chip, the sidebar
//! card nor the panel's open animation recomputes the sun's position per
//! frame. The chip and the card tick on the minute they display; only the
//! open panel ticks every second.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, Offset as _, TimeDelta, Timelike};
use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::prayer::{self, AsrSchool, Location, Method, Prayer, PrayerTimes};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    Cached, HEADER_CHIP_PX, HEADER_H, PanelShape, SMALL_ICON_PX, arabic_digits, dot_ring, glass_card, glow, icon_chip, partner,
    percent_text, readable, redraw_in, rim,
};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{card, reveal, text, tokens};
use crate::state::config;

pub const ID: &str = "prayer-times";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 372.0;
const PAD: f32 = 14.0;
const HERO_H: f32 = 134.0;
const ROW_H: f32 = 38.0;
/// Timeline row height in the sidebar card.
const SIDEBAR_ROW_H: f32 = 32.0;
const STATS_H: f32 = 54.0;
const GAP: f32 = 12.0;
/// Extra space between the header and the hero.
const HEADER_EXTRA_GAP: f32 = 2.0;
/// Rows on the timeline (five prayers and sunrise).
const TIMELINE_ROWS: usize = 6;

/// Panel height: header, hero, six timeline rows and the stats strip.
const PANEL_H: f32 = PAD * 2.0 + HEADER_H + GAP + HEADER_EXTRA_GAP + HERO_H + GAP + ROW_H * TIMELINE_ROWS as f32 + GAP + STATS_H;

/// Header: Arabic date size and gap.
const HIJRI_AR_PX: f32 = 14.0;
const HIJRI_GAP_PX: f32 = 5.0;
/// Hero: gradient angle, background tones (dark / light: lightness, chroma
/// share, chroma cap for primary and tertiary), padding / gap, the ring
/// (diameter, dots, dot size), its track alpha and the text sizes.
const HERO_ANGLE_DEG: f32 = 125.0;
const HERO_DARK: [(f32, f32, f32); 2] = [(0.30, 0.55, 0.075), (0.22, 0.5, 0.06)];
const HERO_LIGHT: [(f32, f32, f32); 2] = [(0.93, 0.35, 0.045), (0.96, 0.3, 0.035)];
const HERO_PAD_PX: f32 = 14.0;
const HERO_GAP_PX: f32 = tokens::LG;
const RING_D_PX: f32 = 108.0;
const RING_DOTS: usize = 52;
const RING_DOT_PX: f32 = 4.0;
const RING_TRACK_ALPHA: (f32, f32) = (0.16, 0.12);
const HERO_NAME_AR_PX: f32 = 19.0;
const HERO_KICKER_PX: f32 = 10.5;
const HERO_NAME_PX: f32 = 21.0;
const HERO_CLOCK_PX: f32 = 28.0;
const HERO_CLOCK_LINE_PX: f32 = 32.0;
const HERO_LINE_GAP_PX: f32 = 1.0;
/// Timeline: rail width, node sizes (next / other / marker / inner dot),
/// glow, the name pill (inset, padding, radius, alphas), icon size, the
/// "in …" badge and the time column width (24 h / 12 h).
const RAIL_W_PX: f32 = 2.0;
const RAIL_COL_PX: f32 = 14.0;
const NODE_NEXT_PX: f32 = 12.0;
const NODE_PX: f32 = 8.0;
const NODE_MARKER_PX: f32 = 6.0;
const NODE_INNER_PX: f32 = 5.0;
const NODE_GLOW: (f32, f32) = (0.5, 8.0);
const NODE_FUTURE_ALPHA: f32 = 0.22;
const RAIL_OFF_ALPHA: (f32, f32) = (0.12, 0.10);
const ROW_GAP_PX: f32 = tokens::SM;
const ROW_PILL_INSET_PX: f32 = tokens::XS;
const ROW_PILL_PAD_PX: f32 = 10.0;
const ROW_PILL_RADIUS_PX: f32 = tokens::MD;
const ROW_PILL_ALPHA: (f32, f32) = (0.14, 0.11);
const ROW_INNER_GAP_PX: f32 = 9.0;
const ROW_ICON_PX: f32 = 15.0;
const ROW_ICON_ALPHA: (f32, f32) = (0.6, 0.9);
const ROW_NAME_DIM_ALPHA: f32 = 0.78;
const BADGE_H_PX: f32 = 18.0;
const BADGE_PAD_PX: f32 = 7.0;
const BADGE_ALPHA: f32 = 0.18;
const BADGE_TEXT_PX: f32 = 10.5;
const ROW_AR_PX: f32 = 13.5;
const ROW_AR_ALPHA: (f32, f32) = (0.6, 0.85);
const ROW_TIME_PAST_ALPHA: f32 = 0.7;
const TIME_W_24H_PX: f32 = 44.0;
const TIME_W_12H_PX: f32 = 66.0;
/// Stats: inner padding / gaps and the gap between cards.
const STAT_PAD_PX: f32 = 11.0;
const STAT_LINE_GAP_PX: f32 = 2.0;
const STAT_ICON_GAP_PX: f32 = 5.0;
const STATS_GAP_PX: f32 = tokens::SM;
/// Bar chip / sidebar glyph sizes and the chip's gap.
const BAR_ICON_PX: f32 = 14.0;
const BAR_GAP_PX: f32 = 6.0;
const SIDEBAR_ICON_PX: f32 = 15.0;

// ── Time & calendar ──────────────────────────────────────────────────────

/// The bar turns to an "in …" countdown this close to the next prayer.
const SOON_MIN: i64 = 15;
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 3600;
const MINUTES_PER_HOUR: i64 = 60;
const MS_PER_S: u32 = 1000;
/// Ticks land this far past the boundary they wait for.
const TICK_MARGIN: Duration = Duration::from_millis(5);
/// Minute-granular views wait at most this long between redraws.
const MINUTE: Duration = Duration::from_secs(60);
/// The last third of the night starts two thirds of the way through it.
const NIGHT_THIRDS: i32 = 3;
const LAST_THIRD_START: i32 = 2;
/// Kaaba coordinates (degrees).
const KAABA_LAT_DEG: f64 = 21.4225;
const KAABA_LON_DEG: f64 = 39.8262;
const FULL_TURN_DEG: f64 = 360.0;
/// Compass rose: eight points of 45° each.
const COMPASS_POINTS: usize = 8;
const COMPASS_SECTOR_DEG: f64 = FULL_TURN_DEG / COMPASS_POINTS as f64;
/// Months in the Hijri year.
const HIJRI_MONTH_COUNT: usize = 12;
/// Qibla bearing precision in `prayer next` (tenths of a degree).
const QIBLA_DECIMALS: f64 = 10.0;

// Tabular Islamic calendar (the arithmetic "Kuwaiti" algorithm on Julian
// day numbers); the constants are the published integer coefficients.
/// Julian day number of 0001-01-01 minus one (CE day 0).
const JD_CE_DAY_ZERO: i64 = 1_721_425;
/// Julian day of the Islamic epoch (1 Muharram 1 AH) minus one.
const ISLAMIC_EPOCH_JD: i64 = 1_948_440;
/// Days in a 30-year cycle (and one more, for the cycle shift).
const CYCLE_DAYS: i64 = 10_631;
const CYCLE_SHIFT: i64 = CYCLE_DAYS + 1;
/// Years per cycle, a year of 354 days, and the 29 days of a short month.
const CYCLE_YEARS: i64 = 30;
const YEAR_DAYS: i64 = 354;
const SHORT_MONTH_DAYS: i64 = 29;
/// Year-in-cycle split coefficients.
const J_A: i64 = 10_985;
const J_B: i64 = 5_316;
const J_C: i64 = 5_670;
const J_D: i64 = 16;
const J_E: i64 = 15;
/// Year-length ratios `50 / 17 719` and `43 / 15 238` (days per year).
const YEAR_A: (i64, i64) = (50, 17_719);
const YEAR_B: (i64, i64) = (43, 15_238);
/// 24 months take 709 days.
const MONTH_PAIR: (i64, i64) = (24, 709);

// ── Settings ─────────────────────────────────────────────────────────────

struct Settings {
    loc: Location,
    method: Method,
    school: AsrSchool,
    place: String,
    hijri_offset: i64,
    h24: bool,
    /// Hours east of UTC the times are given in (the system's local time).
    utc_offset: f64,
}

fn settings(cx: &App) -> Settings {
    let cfg = config(cx).read(cx).snapshot();
    let s = &cfg.plugins;
    let loc = if cfg.location.shared {
        Location { latitude: cfg.location.latitude, longitude: cfg.location.longitude }
    } else {
        Location {
            latitude: s.setting(ID, "latitude", Location::EL_MANDARA.latitude),
            longitude: s.setting(ID, "longitude", Location::EL_MANDARA.longitude),
        }
    };
    let default_place = if cfg.location.shared && !cfg.location.name.is_empty() {
        cfg.location.name.clone()
    } else if loc == Location::EL_MANDARA {
        t("El-Mandara, Alexandria").to_string()
    } else {
        s.setting(
            "weather",
            "location_name",
            format!(
                "{:.2}°{}, {:.2}°{}",
                loc.latitude.abs(),
                if loc.latitude >= 0.0 { "N" } else { "S" },
                loc.longitude.abs(),
                if loc.longitude >= 0.0 { "E" } else { "W" }
            ),
        )
    };
    Settings {
        loc,
        method: s.setting(ID, "method", Method::Egyptian),
        school: s.setting(ID, "asr", AsrSchool::Shafii),
        // An empty name (a cleared Studio field) falls back like a missing one.
        place: if cfg.location.shared { default_place } else { Some(s.setting(ID, "location_name", String::new())).filter(|n| !n.trim().is_empty()).unwrap_or(default_place) },
        hijri_offset: s.setting(ID, "hijri_offset", 0_i64),
        h24: cfg.general.clock_24h,
        utc_offset: utc_offset_hours(),
    }
}

fn utc_offset_hours() -> f64 {
    f64::from(Local::now().offset().fix().local_minus_utc()) / SECS_PER_HOUR as f64
}

fn times_for(date: NaiveDate, s: &Settings) -> PrayerTimes {
    prayer::compute(date, s.loc, s.utc_offset, s.method, s.school)
}

/// What a [`Schedule`] was computed for.
#[derive(Clone, PartialEq)]
struct ScheduleKey {
    date: NaiveDate,
    loc: Location,
    offset: u64,
    method: Method,
    school: AsrSchool,
}

/// Yesterday's, today's and tomorrow's times around a date.
type Schedule = Rc<[PrayerTimes; 3]>;

thread_local! {
    static SCHEDULE: RefCell<Option<(ScheduleKey, Schedule)>> = const { RefCell::new(None) };
}

/// The three days around `date`, computed once per date and settings.
fn schedule(date: NaiveDate, s: &Settings) -> Schedule {
    let key = ScheduleKey { date, loc: s.loc, offset: s.utc_offset.to_bits(), method: s.method, school: s.school };
    SCHEDULE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((k, days)) = c.as_ref()
            && *k == key
        {
            return days.clone();
        }
        let days: Schedule = Rc::new([times_for(date.pred_opt().unwrap_or(date), s), times_for(date, s), times_for(date.succ_opt().unwrap_or(date), s)]);
        *c = Some((key, days.clone()));
        days
    })
}

/// Today's times from the user's settings.
pub fn today(cx: &App) -> PrayerTimes {
    let s = settings(cx);
    schedule(Local::now().date_naive(), &s)[1].clone()
}

// ── Calendar & geometry helpers ──────────────────────────────────────────

/// `(Latin name — a translation key, Arabic name)` per Hijri month.
pub const HIJRI_MONTHS: [(&str, &str); HIJRI_MONTH_COUNT] = [
    (tk("Muharram"), "محرم"),
    (tk("Safar"), "صفر"),
    (tk("Rabi al-Awwal"), "ربيع الأول"),
    (tk("Rabi al-Thani"), "ربيع الآخر"),
    (tk("Jumada al-Ula"), "جمادى الأولى"),
    (tk("Jumada al-Akhirah"), "جمادى الآخرة"),
    (tk("Rajab"), "رجب"),
    (tk("Sha'ban"), "شعبان"),
    (tk("Ramadan"), "رمضان"),
    (tk("Shawwal"), "شوال"),
    (tk("Dhu al-Qadah"), "ذو القعدة"),
    (tk("Dhu al-Hijjah"), "ذو الحجة"),
];

/// The month's names for a `1..=12` Hijri month (clamped).
fn hijri_month(m: usize) -> (&'static str, &'static str) {
    HIJRI_MONTHS[m.clamp(1, HIJRI_MONTH_COUNT) - 1]
}

/// Tabular (arithmetic) Islamic calendar date `(year, month 1..=12, day)`
/// for a Gregorian date. Within a day of the sighted calendar; the
/// `hijri_offset` setting corrects for local moon sighting.
pub fn hijri(date: NaiveDate) -> (i64, usize, i64) {
    let jd = i64::from(date.num_days_from_ce()) + JD_CE_DAY_ZERO;
    let l = jd - ISLAMIC_EPOCH_JD + CYCLE_SHIFT;
    let n = (l - 1) / CYCLE_DAYS;
    let l = l - CYCLE_DAYS * n + YEAR_DAYS;
    let j = ((J_A - l) / J_B) * ((YEAR_A.0 * l) / YEAR_A.1) + (l / J_C) * ((YEAR_B.0 * l) / YEAR_B.1);
    let l = l - ((CYCLE_YEARS - j) / J_E) * ((YEAR_A.1 * j) / YEAR_A.0) - (j / J_D) * ((YEAR_B.1 * j) / YEAR_B.0) + SHORT_MONTH_DAYS;
    let m = (MONTH_PAIR.0 * l) / MONTH_PAIR.1;
    let d = l - (MONTH_PAIR.1 * m) / MONTH_PAIR.0;
    let y = CYCLE_YEARS * n + j - CYCLE_YEARS;
    (y, usize::try_from(m).unwrap_or(1), d)
}

/// Great-circle bearing to the Kaaba in degrees from true north.
pub fn qibla(loc: Location) -> f64 {
    let (lat1, lon1) = (loc.latitude.to_radians(), loc.longitude.to_radians());
    let (lat2, lon2) = (KAABA_LAT_DEG.to_radians(), KAABA_LON_DEG.to_radians());
    let dl = lon2 - lon1;
    let y = dl.sin();
    let x = lat1.cos() * lat2.tan() - lat1.sin() * dl.cos();
    y.atan2(x).to_degrees().rem_euclid(FULL_TURN_DEG)
}

/// The eight compass points, clockwise from north.
const COMPASS: [&str; COMPASS_POINTS] = [tk("N"), tk("NE"), tk("E"), tk("SE"), tk("S"), tk("SW"), tk("W"), tk("NW")];

fn compass(deg: f64) -> &'static str {
    let sector = ((deg + COMPASS_SECTOR_DEG / 2.0).rem_euclid(FULL_TURN_DEG) / COMPASS_SECTOR_DEG) as usize % COMPASS_POINTS;
    t(COMPASS[sector])
}

fn method_label(m: Method) -> &'static str {
    match m {
        Method::Egyptian => t("Egyptian"),
        Method::MuslimWorldLeague => t("MWL"),
        Method::UmmAlQura => t("Umm al-Qura"),
        Method::Isna => t("ISNA"),
        Method::Karachi => t("Karachi"),
    }
}

fn prayer_icon(p: Prayer) -> &'static str {
    match p {
        Prayer::Fajr => "cloud-moon",
        Prayer::Sunrise => "sunset",
        Prayer::Dhuhr => "sun",
        Prayer::Asr => "cloud-sun",
        Prayer::Maghrib => "sunset",
        Prayer::Isha => "moon",
    }
}

fn fmt_time(t: NaiveTime, h24: bool) -> String {
    if h24 { t.format("%H:%M").to_string() } else { t.format("%-I:%M %p").to_string() }
}

fn fmt_span(d: TimeDelta) -> String {
    let mins = d.num_minutes().max(0);
    if mins >= MINUTES_PER_HOUR {
        tf("{h}h {m}m", &[("h", &(mins / MINUTES_PER_HOUR)), ("m", &format!("{:02}", mins % MINUTES_PER_HOUR))])
    } else {
        tf("{m}m", &[("m", &mins)])
    }
}

/// Localised prayer name.
fn pname(p: Prayer) -> &'static str {
    t(p.name())
}

fn fmt_clock(d: TimeDelta) -> String {
    let s = d.num_seconds().max(0);
    format!("{}:{:02}:{:02}", s / SECS_PER_HOUR, (s / SECS_PER_MIN) % MINUTES_PER_HOUR, s % SECS_PER_MIN)
}

/// Delay until just after the next whole second.
fn until_next_second() -> Duration {
    let into = Local::now().timestamp_subsec_millis().min(MS_PER_S - 1);
    Duration::from_millis(u64::from(MS_PER_S - into)) + TICK_MARGIN
}

/// The day as seen from `now`: the previous and next actual prayers (which
/// may fall on yesterday / tomorrow) and the three days' schedules.
struct Day {
    now: NaiveDateTime,
    days: Schedule,
    prev: (Prayer, NaiveDateTime),
    next: (Prayer, NaiveDateTime),
}

impl Day {
    fn at(now: NaiveDateTime, s: &Settings) -> Day {
        let date = now.date();
        let days = schedule(date, s);
        let [yesterday, today, tomorrow] = &*days;
        let mut events: Vec<(Prayer, NaiveDateTime)> = Vec::with_capacity(TIMELINE_ROWS + 2);
        if let Some(d) = date.pred_opt() {
            events.push((Prayer::Isha, d.and_time(yesterday.get(Prayer::Isha))));
        }
        events.extend(today.times.iter().map(|(p, t)| (*p, date.and_time(*t))));
        if let Some(d) = date.succ_opt() {
            events.push((Prayer::Fajr, d.and_time(tomorrow.get(Prayer::Fajr))));
        }
        let next = events.iter().copied().find(|(p, t)| p.is_prayer() && *t > now).unwrap_or((Prayer::Fajr, now));
        let prev = events.iter().copied().rev().find(|(p, t)| p.is_prayer() && *t <= now).unwrap_or((Prayer::Isha, now));
        Day { now, days, prev, next }
    }

    fn now(s: &Settings) -> Day {
        Day::at(Local::now().naive_local(), s)
    }

    fn today(&self) -> &PrayerTimes {
        &self.days[1]
    }

    /// Fraction of the way from the previous prayer to the next one.
    fn elapsed(&self) -> f32 {
        let span = (self.next.1 - self.prev.1).num_seconds().max(1) as f32;
        ((self.now - self.prev.1).num_seconds() as f32 / span).clamp(0.0, 1.0)
    }

    fn remaining(&self) -> TimeDelta {
        self.next.1 - self.now
    }

    /// Islamic date; the Islamic day begins at Maghrib.
    fn hijri(&self, offset: i64) -> (i64, usize, i64) {
        let after_maghrib = self.now.time() >= self.today().get(Prayer::Maghrib);
        let date = self.now.date() + TimeDelta::days(offset + i64::from(after_maghrib));
        hijri(date)
    }

    /// When a minute-granular readout of this day next changes: the next
    /// minute boundary or the next prayer, whichever is first.
    fn next_change(&self) -> Duration {
        let into_minute = TimeDelta::seconds(i64::from(self.now.second())) + TimeDelta::nanoseconds(i64::from(self.now.nanosecond()));
        let to_minute = TimeDelta::minutes(1) - into_minute;
        let to_prayer = self.remaining();
        let wait = if to_prayer > TimeDelta::zero() { to_minute.min(to_prayer) } else { to_minute };
        wait.to_std().unwrap_or(MINUTE).min(MINUTE) + TICK_MARGIN
    }
}

/// Start of the last third of the night (Maghrib → next Fajr) that contains
/// or follows `now`.
fn last_third(day: &Day) -> NaiveDateTime {
    let date = day.now.date();
    let [yesterday, today, tomorrow] = &*day.days;
    // Before Fajr the relevant night started yesterday evening.
    let (evening, dusk, dawn) =
        if day.now.time() < today.get(Prayer::Fajr) { (date.pred_opt().unwrap_or(date), yesterday, today) } else { (date, today, tomorrow) };
    let maghrib = evening.and_time(dusk.get(Prayer::Maghrib));
    let morning = evening.succ_opt().unwrap_or(evening);
    let fajr = morning.and_time(dawn.get(Prayer::Fajr));
    maghrib + (fajr - maghrib) * LAST_THIRD_START / NIGHT_THIRDS
}

// ── Labels (formatted once per minute) ───────────────────────────────────

/// What [`Texts`] were formatted from.
#[derive(Clone, PartialEq)]
struct TextKey {
    minute: NaiveDateTime,
    next: (Prayer, NaiveDateTime),
    prev: (Prayer, NaiveDateTime),
    h24: bool,
    hijri: (i64, usize, i64),
    place: String,
    method: Method,
    loc: Location,
}

struct Texts {
    key: TextKey,
    subtitle: SharedString,
    hijri_ar: (SharedString, &'static str, SharedString),
    hijri_line: SharedString,
    at_line: SharedString,
    times: Vec<SharedString>,
    in_span: SharedString,
    bar_soon: bool,
    bar: SharedString,
    sidebar: SharedString,
    stats: [SharedString; 3],
}

thread_local! {
    static TEXTS: RefCell<Option<Rc<Texts>>> = const { RefCell::new(None) };
}

/// The day's labels, re-formatted only when the minute or settings change.
fn texts(day: &Day, s: &Settings) -> Rc<Texts> {
    let hijri_date = day.hijri(s.hijri_offset);
    let minute = day.now.with_second(0).and_then(|m| m.with_nanosecond(0)).unwrap_or(day.now);
    let key = TextKey { minute, next: day.next, prev: day.prev, h24: s.h24, hijri: hijri_date, place: s.place.clone(), method: s.method, loc: s.loc };
    TEXTS.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(texts) = c.as_ref()
            && texts.key == key
        {
            return texts.clone();
        }
        let texts = Rc::new(build_texts(day, s, key));
        *c = Some(texts.clone());
        texts
    })
}

fn build_texts(day: &Day, s: &Settings, key: TextKey) -> Texts {
    let (y, m, d) = key.hijri;
    let (latin_month, arabic_month) = hijri_month(m);
    let (next, at) = day.next;
    let span = fmt_span(day.remaining());
    let soon = day.remaining() <= TimeDelta::minutes(SOON_MIN);
    let in_prayer = tf("{prayer} in {span}", &[("prayer", &pname(next)), ("span", &span)]);
    let daylight = day.today().get(Prayer::Maghrib).signed_duration_since(day.today().get(Prayer::Sunrise));
    let q = qibla(s.loc);
    Texts {
        subtitle: format!("{} · {}", s.place, method_label(s.method)).into(),
        hijri_ar: (arabic_digits(&y.to_string()).into(), arabic_month, arabic_digits(&d.to_string()).into()),
        hijri_line: format!("{d} {} · {}", t(latin_month), day.now.format(t("%a %-d %b"))).into(),
        at_line: tf("at {time} · after {prayer} {since}", &[("time", &fmt_time(at.time(), s.h24)), ("prayer", &pname(day.prev.0)), ("since", &fmt_time(day.prev.1.time(), s.h24))]).into(),
        times: day.today().times.iter().map(|(_, t)| SharedString::from(fmt_time(*t, s.h24))).collect(),
        in_span: tf("in {span}", &[("span", &span)]).into(),
        bar_soon: soon,
        bar: if soon { in_prayer.clone().into() } else { format!("{} {}", pname(next), fmt_time(at.time(), s.h24)).into() },
        sidebar: in_prayer.into(),
        stats: [fmt_span(daylight).into(), fmt_time(last_third(day).time(), s.h24).into(), format!("{q:.0}° {}", compass(q)).into()],
        key,
    }
}

// ── Views ────────────────────────────────────────────────────────────────

fn header(txt: &Texts, palette: &MandaraPalette, arabic: SharedString) -> impl IntoElement {
    let accent = readable(palette.primary, palette);
    let (year, month, day) = txt.hijri_ar.clone();
    div()
        .h(px(HEADER_H))
        .flex()
        .items_center()
        .gap(px(tokens::MD))
        .child(icon_chip("crescent", accent, HEADER_CHIP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().child(t("Prayer times")))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(txt.subtitle.clone())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .flex_none()
                .child(
                    // Laid out right-to-left by hand (day · month · year) so
                    // mixed Arabic words and digits never depend on bidi.
                    div()
                        .flex()
                        .items_center()
                        .gap(px(HIJRI_GAP_PX))
                        .font_family(arabic)
                        .text_size(px(HIJRI_AR_PX))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(accent.hsla())
                        .child(year)
                        .child(month)
                        .child(day),
                )
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .font_features(tabular_nums())
                        .text_color(palette.on_surface_variant.hsla())
                        .whitespace_nowrap()
                        .child(txt.hijri_line.clone()),
                ),
        )
}

/// Hero backdrop tone from a theme colour: `(lightness, chroma share, cap)`.
fn hero_tone(c: Color, (l, share, cap): (f32, f32, f32)) -> Color {
    c.with_lightness(l).with_chroma((c.chroma() * share).min(cap))
}

fn hero(day: &Day, txt: &Texts, clock: SharedString, palette: &MandaraPalette, arabic: SharedString, display: SharedString, radius: f32) -> impl IntoElement {
    let accent = readable(palette.primary, palette);
    let accent2 = readable(partner(palette.primary), palette);
    let tones = if palette.is_dark { HERO_DARK } else { HERO_LIGHT };
    let (bg_a, bg_b) = (hero_tone(palette.primary, tones[0]), hero_tone(palette.tertiary, tones[1]));
    let next = day.next.0;
    let frac = day.elapsed();
    let track = palette.on_surface.with_alpha(super::panel_fx::alpha(RING_TRACK_ALPHA, palette));
    let inner = div()
        .flex()
        .flex_col()
        .items_center()
        .child(div().font_family(arabic).text_size(px(HERO_NAME_AR_PX)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface.hsla()).child(next.name_ar()))
        .child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(percent_text(frac)))
        .into_any_element();
    div()
        .relative()
        .h(px(HERO_H))
        .rounded(px(radius))
        .overflow_hidden()
        .bg(oklab_gradient(HERO_ANGLE_DEG, bg_a, bg_b))
        .shadow(rim(palette))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .px(px(HERO_PAD_PX))
                .flex()
                .items_center()
                .gap(px(HERO_GAP_PX))
                .child(dot_ring(frac, RING_D_PX, RING_DOTS, RING_DOT_PX, accent, accent2, track, inner))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(HERO_LINE_GAP_PX))
                        .child(div().text_size(px(HERO_KICKER_PX)).font_weight(FontWeight::BOLD).text_color(accent.hsla()).child(t("NEXT PRAYER")))
                        .child(div().text_size(px(HERO_NAME_PX)).font_weight(FontWeight::SEMIBOLD).child(pname(next)))
                        .child(
                            div()
                                .font_family(display)
                                .font_features(tabular_nums())
                                .text_size(px(HERO_CLOCK_PX))
                                .line_height(px(HERO_CLOCK_LINE_PX))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.on_surface.hsla())
                                .child(clock),
                        )
                        .child(div().text_size(px(text::BODY_S)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(txt.at_line.clone())),
                ),
        )
}

/// One half of the timeline rail, filled `fill` (0..1) from the top.
fn rail_half(fill: f32, visible: bool, on: Color, off: Color) -> impl IntoElement {
    div().flex_1().w(px(RAIL_W_PX)).when(visible, |el| {
        el.rounded_full().bg(off.hsla()).child(div().w_full().h(gpui::relative(fill.clamp(0.0, 1.0))).rounded_full().bg(on.hsla()))
    })
}

fn timeline(day: &Day, txt: &Texts, h24: bool, palette: &MandaraPalette, arabic: SharedString, row_h: f32, show_arabic: bool) -> impl IntoElement {
    let pick = |(dark, light): (f32, f32)| super::panel_fx::alpha((dark, light), palette);
    let accent = readable(palette.primary, palette);
    let accent2 = readable(partner(palette.primary), palette);
    let off = palette.on_surface.with_alpha(pick(RAIL_OFF_ALPHA));
    let date = day.now.date();
    let times = &day.today().times;
    let at: [NaiveDateTime; TIMELINE_ROWS] = std::array::from_fn(|i| date.and_time(times[i].1));
    // How far `now` has travelled between row i and row i + 1.
    let seg = |i: usize| -> f32 {
        match (at.get(i), at.get(i + 1)) {
            (Some(a), Some(b)) => ((day.now - *a).num_seconds() as f32 / (*b - *a).num_seconds().max(1) as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    };
    let next_today = (day.next.1.date() == date).then_some(day.next.0);
    let n = times.len();
    let frame = crate::shell::frame_color(palette);
    let time_w = if h24 { TIME_W_24H_PX } else { TIME_W_12H_PX };
    div().flex().flex_col().children(times.iter().enumerate().map(move |(i, (p, _))| {
        let p = *p;
        let past = at[i] <= day.now;
        let is_next = next_today == Some(p);
        let marker = !p.is_prayer();
        let line_color = accent.mix(accent2, i as f32 / n as f32);
        // The rail meets the node halfway down each row.
        let top_fill = if i == 0 { 0.0 } else { (seg(i - 1) * 2.0 - 1.0).clamp(0.0, 1.0) };
        let bottom_fill = (seg(i) * 2.0).clamp(0.0, 1.0);
        let node_px = if marker { NODE_MARKER_PX } else { NODE_PX };
        let node = if is_next {
            div()
                .size(px(NODE_NEXT_PX))
                .rounded_full()
                .bg(accent.hsla())
                .shadow(vec![glow(accent.with_alpha(NODE_GLOW.0), NODE_GLOW.1, 1.0, 0.0)])
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(NODE_INNER_PX)).rounded_full().bg(frame.hsla()))
        } else if past {
            div().size(px(node_px)).rounded_full().bg(line_color.hsla())
        } else {
            div().size(px(node_px)).rounded_full().bg(palette.on_surface.with_alpha(NODE_FUTURE_ALPHA).hsla())
        };
        let name_color = if is_next {
            accent
        } else if past || marker {
            palette.on_surface_variant.with_alpha(ROW_NAME_DIM_ALPHA)
        } else {
            palette.on_surface
        };
        let dim = |(past_a, future_a): (f32, f32)| if past { past_a } else { future_a };
        div()
            .h(px(row_h))
            .flex()
            .items_center()
            .gap(px(ROW_GAP_PX))
            .child(
                div()
                    .w(px(RAIL_COL_PX))
                    .h_full()
                    .flex_none()
                    .relative()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(rail_half(top_fill, i > 0, line_color, off))
                    .child(div().h(px(0.0)).relative().flex().items_center().justify_center().child(node))
                    .child(rail_half(bottom_fill, i + 1 < n, line_color, off)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(row_h - ROW_PILL_INSET_PX))
                    .px(px(ROW_PILL_PAD_PX))
                    .rounded(px(crate::style::radius(ROW_PILL_RADIUS_PX)))
                    .when(is_next, |el| el.bg(accent.with_alpha(pick(ROW_PILL_ALPHA)).hsla()))
                    .flex()
                    .items_center()
                    .gap(px(ROW_INNER_GAP_PX))
                    .child(icon(prayer_icon(p), ROW_ICON_PX, if is_next { accent } else { palette.on_surface_variant.with_alpha(dim(ROW_ICON_ALPHA)) }))
                    .child(
                        div()
                            .text_size(px(if marker { text::BODY_S } else { text::BODY }))
                            .font_weight(if is_next {
                                FontWeight::SEMIBOLD
                            } else if marker {
                                FontWeight::NORMAL
                            } else {
                                FontWeight::MEDIUM
                            })
                            .text_color(name_color.hsla())
                            .child(pname(p)),
                    )
                    .when(is_next, |el| {
                        el.child(
                            div()
                                .h(px(BADGE_H_PX))
                                .px(px(BADGE_PAD_PX))
                                .rounded_full()
                                .bg(accent.with_alpha(BADGE_ALPHA).hsla())
                                .flex()
                                .items_center()
                                .text_size(px(BADGE_TEXT_PX))
                                .font_weight(FontWeight::SEMIBOLD)
                                .font_features(tabular_nums())
                                .text_color(accent.hsla())
                                .child(txt.in_span.clone()),
                        )
                    })
                    .child(div().flex_1())
                    .when(show_arabic, |el| {
                        el.child(
                            div()
                                .font_family(arabic.clone())
                                .text_size(px(ROW_AR_PX))
                                .text_color(if is_next { accent.hsla() } else { palette.on_surface_variant.with_alpha(dim(ROW_AR_ALPHA)).hsla() })
                                .child(p.name_ar()),
                        )
                    })
                    .child(
                        div()
                            .w(px(time_w))
                            .flex()
                            .justify_end()
                            .font_features(tabular_nums())
                            .text_size(px(text::BODY))
                            .font_weight(if is_next { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                            .text_color(if past && !is_next { palette.on_surface_variant.with_alpha(ROW_TIME_PAST_ALPHA).hsla() } else { palette.on_surface.hsla() })
                            .child(txt.times.get(i).cloned().unwrap_or_default()),
                    ),
            )
    }))
}

fn stat(icon_name: &'static str, label: &'static str, value: SharedString, tint: Color, palette: &MandaraPalette, radius: f32) -> impl IntoElement {
    glass_card(palette)
        .flex_1()
        .min_w(px(0.0))
        .h(px(STATS_H))
        .rounded(px(radius))
        .px(px(STAT_PAD_PX))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(STAT_LINE_GAP_PX))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(STAT_ICON_GAP_PX))
                .child(icon(icon_name, SMALL_ICON_PX, tint))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(label)),
        )
        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).truncate().child(value))
}

fn stats(txt: &Texts, palette: &MandaraPalette, r: f32) -> impl IntoElement {
    let [daylight, third, qibla] = txt.stats.clone();
    div()
        .flex()
        .gap(px(STATS_GAP_PX))
        .child(stat("sun", t("Daylight"), daylight, palette.warning_amber, palette, r))
        .child(stat("moon", t("Last third"), third, readable(palette.tertiary, palette), palette, r))
        .child(stat("arrow", t("Qibla"), qibla, readable(palette.primary, palette), palette, r))
}

fn panel_view(p: PanelCx) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement {
    let clock: RefCell<Cached<i64>> = RefCell::default();
    move |palette, window, cx| {
        let s = settings(cx);
        let day = Day::now(&s);
        let txt = texts(&day, &s);
        let remaining = day.remaining();
        let clock = clock.borrow_mut().get(remaining.num_seconds(), |_| fmt_clock(remaining));
        let f = fonts(cx);
        // Cards concentric with the panel's rounded corners.
        let r = PanelShape::of(cx).inner(PAD);
        if p.progress() > 0.0 {
            // Tick the countdown on the next whole second.
            redraw_in(until_next_second(), window, cx);
        }
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&p, 0, header(&txt, palette, f.arabic.clone())))
            .child(div().h(px(GAP + HEADER_EXTRA_GAP)))
            .child(reveal(&p, 1, hero(&day, &txt, clock, palette, f.arabic.clone(), f.display.clone(), r)))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 2, timeline(&day, &txt, s.h24, palette, f.arabic.clone(), ROW_H, true)))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 3, stats(&txt, palette, r)))
            .into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Prayer times"), tk("Islamic daily prayer times and countdown (opt-in)"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, window, cx| {
                let s = settings(cx);
                let day = Day::now(&s);
                // The chip shows minutes: redraw when they (or the prayer) change.
                redraw_in(day.next_change(), window, cx);
                let txt = texts(&day, &s);
                let accent = if txt.bar_soon { readable(palette.primary, palette) } else { palette.tertiary };
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .font_features(tabular_nums())
                    .child(icon("crescent", crate::style::bar_icon(BAR_ICON_PX), accent))
                    .child(div().text_color(if txt.bar_soon { accent.hsla() } else { palette.on_surface_variant.hsla() }).child(txt.bar.clone()))
                    .into_any_element()
            },
            window,
            cx,
        );
        let panel = fn_view(None, panel_view(panel_cx.clone()), window, cx);
        let sidebar = fn_view(
            None,
            |palette, window, cx| {
                let s = settings(cx);
                let day = Day::now(&s);
                redraw_in(day.next_change(), window, cx);
                let txt = texts(&day, &s);
                let accent = readable(palette.primary, palette);
                card(palette)
                    .flex()
                    .flex_col()
                    .gap(px(tokens::SM))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::SM))
                            .child(icon("crescent", SIDEBAR_ICON_PX, accent))
                            .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t("Prayer times")))
                            .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).font_features(tabular_nums()).text_color(accent.hsla()).child(txt.sidebar.clone())),
                    )
                    .child(timeline(&day, &txt, s.h24, palette, fonts(cx).arabic, SIDEBAR_ROW_H, true))
                    .into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H)))) }),
            sidebar: Some(sidebar),
            on_click: None,
            on_secondary_click: Some(Rc::new(|_, cx| {
                let shell = crate::state::shell(cx);
                let open = !shell.read(cx).sidebar_visible();
                shell.update(cx, |s, cx| s.set_sidebar(open, cx));
            })),
        }
    });
    r.command(ID, CommandSpec::new("prayer today", tk("Today's prayer times"), |_, cx| {
        let times = today(cx);
        Ok(json!(Prayer::ALL.iter().map(|p| json!({ "prayer": p.name(), "arabic": p.name_ar(), "time": times.get(*p).format("%H:%M").to_string() })).collect::<Vec<_>>()))
    }));
    r.command(ID, CommandSpec::new("prayer next", tk("The next prayer and time remaining"), |_, cx| {
        let s = settings(cx);
        let day = Day::now(&s);
        let (y, m, d) = day.hijri(s.hijri_offset);
        Ok(json!({
            "prayer": day.next.0.name(),
            "arabic": day.next.0.name_ar(),
            "time": day.next.1.format("%H:%M").to_string(),
            "minutes": day.remaining().num_minutes(),
            "elapsed": day.elapsed(),
            "hijri": format!("{d} {} {y}", hijri_month(m).0),
            "qibla": (qibla(s.loc) * QIBLA_DECIMALS).round() / QIBLA_DECIMALS,
        }))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Alexandria's offset on the test dates (EEST), so results don't
    /// depend on the machine's time zone (CI runs in UTC).
    const ALEXANDRIA_UTC_OFFSET: f64 = 3.0;

    fn s() -> Settings {
        Settings { loc: Location::EL_MANDARA, method: Method::Egyptian, school: AsrSchool::Shafii, place: String::new(), hijri_offset: 0, h24: true, utc_offset: ALEXANDRIA_UTC_OFFSET }
    }

    #[test]
    fn hijri_conversion_matches_known_dates() {
        // 1 Muharram 1448 ≈ 16 June 2026; 1 Ramadan 1445 ≈ 11 March 2024.
        let (y, m, _) = hijri(NaiveDate::from_ymd_opt(2026, 10, 27).unwrap());
        assert_eq!((y, m), (1448, 5));
        let (y, m, d) = hijri(NaiveDate::from_ymd_opt(2024, 3, 11).unwrap());
        assert_eq!(y, 1445);
        assert!((m == 9 && d <= 2) || (m == 8 && d >= 29), "{m}/{d}");
        let (y, m, d) = hijri(NaiveDate::from_ymd_opt(2026, 6, 16).unwrap());
        assert!((y, m) == (1448, 1) && d <= 2 || (y, m) == (1447, 12) && d >= 29, "{y}/{m}/{d}");
    }

    #[test]
    fn qibla_from_alexandria_points_south_east() {
        let q = qibla(Location::EL_MANDARA);
        assert!((130.0..142.0).contains(&q), "{q}");
        assert_eq!(compass(q), "SE");
        assert_eq!(compass(359.0), "N");
        assert_eq!(compass(90.0), "E");
        assert_eq!(compass(300.0), "NW");
    }

    #[test]
    fn day_spans_midnight() {
        let s = s();
        let date = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        // 02:00 → previous is yesterday's Isha, next is today's Fajr.
        let early = Day::at(date.and_hms_opt(2, 0, 0).unwrap(), &s);
        assert_eq!((early.prev.0, early.next.0), (Prayer::Isha, Prayer::Fajr));
        assert_eq!(early.prev.1.date(), date.pred_opt().unwrap());
        assert_eq!(early.next.1.date(), date);
        // 23:00 → next is tomorrow's Fajr.
        let late = Day::at(date.and_hms_opt(23, 0, 0).unwrap(), &s);
        assert_eq!(late.next.1.date(), date.succ_opt().unwrap());
        assert!(late.elapsed() > 0.0 && late.elapsed() < 1.0);
        // The last third of tonight starts after midnight, before Fajr.
        let third = last_third(&late);
        assert_eq!(third.date(), date.succ_opt().unwrap());
        assert!(third.time() < late.next.1.time());
        assert_eq!(last_third(&early).date(), date);
        // Midday sits between Dhuhr-1 and Dhuhr.
        let noon = Day::at(date.and_hms_opt(10, 0, 0).unwrap(), &s);
        assert_eq!((noon.prev.0, noon.next.0), (Prayer::Fajr, Prayer::Dhuhr));
        assert_eq!(fmt_clock(TimeDelta::seconds(3725)), "1:02:05");
        assert_eq!(fmt_span(TimeDelta::minutes(105)), "1h 45m");
    }

    #[test]
    fn schedule_is_computed_once_per_day() {
        let s = s();
        let date = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        let a = schedule(date, &s);
        assert!(Rc::ptr_eq(&a, &schedule(date, &s)));
        assert_eq!(a[1], times_for(date, &s));
        assert_eq!(a[0].date, date.pred_opt().unwrap());
    }

    #[test]
    fn minute_views_wake_on_the_minute_or_the_prayer() {
        let s = s();
        let date = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        let day = Day::at(date.and_hms_opt(10, 0, 20).unwrap(), &s);
        let wait = day.next_change();
        assert!(wait <= Duration::from_secs(40) + TICK_MARGIN && wait > Duration::from_secs(39), "{wait:?}");
        let texts = texts(&day, &s);
        assert_eq!(texts.times.len(), TIMELINE_ROWS);
        assert!(Rc::ptr_eq(&texts, &super::texts(&Day::at(date.and_hms_opt(10, 0, 50).unwrap(), &s), &s)), "same minute reuses labels");
    }

    #[test]
    fn panel_height_is_unchanged() {
        assert_eq!(PANEL_H, 14.0 * 2.0 + 38.0 + 12.0 + 2.0 + 134.0 + 12.0 + 38.0 * 6.0 + 12.0 + 54.0);
    }
}
