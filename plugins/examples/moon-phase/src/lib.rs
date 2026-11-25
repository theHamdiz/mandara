//! Example Mandara plugin (Rust → WebAssembly component): a bar item, a
//! rich panel (a look-ahead slider, a view switch, an illumination chart, a
//! virtual list of coming full and new moons, a date field), a desktop
//! widget, a moon calendar tab in Studio → Desktop, launcher results
//! (`moon `), `remind` and `show` commands, a reminder notification, private
//! storage and a `phase` operation other plugins can call.
//!
//! Build with `cargo build --release --target wasm32-wasip2`, then copy the
//! `.wasm` next to `plugin.toml` as `plugin.wasm` (with `locales/` and
//! `assets/`) in `~/.local/share/mandara/plugins-dev/moon-phase/`.
//!
//! Building your own from this? Meet PLUGINS.md's "Design and quality
//! standard" (mandatory for AI agents): a designed hierarchy, polish passes
//! on real screenshots, bounded memory and no Rust anti-patterns.

mod calendar;
mod moon;
mod tree;

use std::cell::RefCell;

wit_bindgen::generate!({ world: "plugin", path: "../../../crates/mandara-plugins/wit" });

use mandara::plugin::broker;
use mandara::plugin::storage;
use mandara::plugin::host;
use mandara::plugin::types::Tone;
use mandara::plugin::ui::{self, Kind as K, Surface, Value};

use calendar::{Date, DAY_MS};
use moon::{Hemisphere, Moon, SYNODIC};
use tree::Tree;

/// The features this plugin uses (`negotiate`); an older Mandara without
/// one of them refuses to load it with a clear message.
const FEATURES: &[&str] = &["ui.layout", "ui.input", "ui.slider", "ui.choice", "ui.grid", "ui.list", "ui.image", "ui.canvas", "ui.keys", "launcher", "studio-tabs", "broker"];

// The plugin's own design, on Mandara's 4 px rhythm. Plugins can't read
// Mandara's settings, so these are fixed by design; the person's theme,
// light or dark mode, button design and language still apply through the
// host's rendering.

/// Sizes (logical px): the header's moon, a row's moon or icon, the
/// title, a caption, a calendar day's moon and the plugin's mark in Studio.
const HERO_MOON_PX: f32 = 72.0;
const ROW_MOON_PX: f32 = 16.0;
const TITLE_PX: f32 = 20.0;
const CAPTION_PX: f32 = 11.0;
const DAY_MOON_PX: f32 = 22.0;
const MARK_PX: f32 = 40.0;
const MARK_RADIUS_PX: f32 = 10.0;
/// Spacing (logical px) on a 4 px rhythm, and the header's hover radius.
const SECTION_GAP: f32 = 16.0;
const HERO_GAP: f32 = 16.0;
const TIGHT_GAP: f32 = 4.0;
const ROW_GAP: f32 = 8.0;
const CELL_PAD_PX: f32 = 4.0;
const HERO_RADIUS_PX: f32 = 12.0;
/// The bar item's icon.
const BAR_ICON_PX: f32 = 15.0;
/// How much of the desktop widget's height its moon takes.
const DESKTOP_MOON_SHARE: f32 = 0.42;
/// How far the look-ahead slider reaches, in days either way.
const LOOK_DAYS: f32 = 14.0;
/// The illumination chart: its height, one bar per day of the cycle, the
/// gap between bars (a share of a bar's slot), the other days' opacity,
/// the shortest bar and the bars' corner radius.
const CHART_H_PX: f32 = 72.0;
const CHART_BARS: usize = 30;
const CHART_BAR_GAP: f32 = 0.35;
const CHART_DIM: f32 = 0.18;
const CHART_FLOOR: f32 = 0.04;
const CHART_RADIUS_PX: f32 = 2.0;
/// The coming moons list: how many it holds (about eight years), its
/// height, a row's height and how many rows the plugin sends at a time.
const UPCOMING: u32 = 200;
const LIST_H_PX: f32 = 168.0;
const LIST_ROW_PX: f32 = 36.0;
const LIST_WINDOW: u32 = 24;
/// The storage key the panel's view is remembered under.
const MODE_KEY: &str = "mode";
/// Launcher results when nothing more is typed.
const LAUNCHER_DEFAULTS: usize = 3;

/// What the panel shows below tonight's moon.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Mode {
    Overview,
    Cycle,
    Upcoming,
}

impl Mode {
    const ALL: [(Mode, &'static str, &'static str); 3] = [(Mode::Overview, "overview", "Overview"), (Mode::Cycle, "cycle", "Cycle"), (Mode::Upcoming, "upcoming", "Coming moons")];

    /// The mode a stored or chosen key names (Overview for anything else).
    fn parse(key: &str) -> Self {
        Self::ALL.iter().find(|(_, k, _)| *k == key).map(|(m, _, _)| *m).unwrap_or(Mode::Overview)
    }
}

/// Everything the views remember between calls. Kept small: numbers and
/// two short strings, nothing that grows.
struct State {
    /// Days from today the panel looks at (the slider).
    ahead: i32,
    mode: Mode,
    /// The first coming moon the list holds.
    list_start: u32,
    /// The date field's text and the day it found.
    typed: String,
    found: Option<i64>,
    /// Months from this one the Studio calendar shows.
    month: i32,
    /// The reminder request waiting for its answer, and what to say after.
    reminder: Option<u64>,
    status: String,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { ahead: 0, mode: Mode::Overview, list_start: 0, typed: String::new(), found: None, month: 0, reminder: None, status: String::new() });
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// The `hemisphere` setting (plugin.toml `[[settings]]`); values arrive as JSON.
fn hemisphere() -> Hemisphere {
    match host::setting("hemisphere").as_deref() {
        Some("\"south\"") => Hemisphere::South,
        _ => Hemisphere::North,
    }
}

/// The `show_percent` setting (on unless set to `false`).
fn show_percent() -> bool {
    host::setting("show_percent").as_deref() != Some("false")
}

/// `{name}` placeholders filled in a translated string.
fn fill(key: &str, values: &[(&str, &str)]) -> String {
    let mut s = host::translate(key);
    for (name, value) in values {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}

/// "Tonight", "Tomorrow", "In {days} days", "Yesterday" or "{days} days ago".
fn when(days: f64) -> String {
    match days.round() as i64 {
        0 => host::translate("Tonight"),
        1 => host::translate("Tomorrow"),
        -1 => host::translate("Yesterday"),
        n if n > 1 => fill("In {days} days", &[("days", &n.to_string())]),
        n => fill("{days} days ago", &[("days", &(-n).to_string())]),
    }
}

fn now_ms() -> f64 {
    host::now_unix_ms() as f64
}

/// The day `days` from now (the same rounding everywhere: panel, calendar,
/// launcher).
fn day_in(days: f64) -> Date {
    Date::from_ms(now_ms() + days * DAY_MS)
}

/// The moon the panel looks at: today plus the slider's days.
fn looked_at() -> Moon {
    let ahead = with(|s| s.ahead);
    Moon::at(now_ms() + f64::from(ahead) * DAY_MS)
}

// ── Views ────────────────────────────────────────────────────────────────

/// Moon images rendered per phase (`assets/moon-<n|s>-NN.png`, made by
/// `art/`): one every 1.2 days, each hemisphere's view.
const ART_PHASES: f64 = 24.0;

thread_local! {
    /// Image handles of the moon pictures, asked for once each.
    static ART: RefCell<std::collections::HashMap<String, u64>> = RefCell::new(std::collections::HashMap::new());
}

/// The handle of `m`'s picture as seen from `hemi` (0 when missing).
fn moon_image(m: &Moon, hemi: Hemisphere) -> u64 {
    let frame = ((m.age / SYNODIC * ART_PHASES).round() as usize) % ART_PHASES as usize;
    let name = format!("moon-{}-{frame:02}.png", if hemi == Hemisphere::North { 'n' } else { 's' });
    ART.with(|art| *art.borrow_mut().entry(name).or_insert_with_key(|name| broker::asset_image(name).unwrap_or(0)))
}

/// The moon's picture, `size` logical px across.
fn moon_art(t: &mut Tree, m: &Moon, hemi: Hemisphere, size: f32) -> u32 {
    let art = t.add(K::Image, "");
    let n = t.node(art);
    n.image = moon_image(m, hemi);
    n.style.width = size;
    n.style.height = size;
    n.hint = host::translate(m.name());
    art
}

/// The looked-at day's moon, large, beside a caption, its name and how
/// lit it is. A click goes back to tonight; ← and → step days.
fn header(t: &mut Tree, m: &Moon, hemi: Hemisphere) -> u32 {
    let ahead = with(|s| s.ahead);
    let art = moon_art(t, m, hemi, HERO_MOON_PX);
    let date = Date::from_ms(now_ms() + f64::from(ahead) * DAY_MS);
    let caption = t.text(&format!("{} · {}", when(f64::from(ahead)), date.short()).to_uppercase(), CAPTION_PX);
    t.tone(caption, Tone::Muted);
    t.bold(caption);
    let title = t.text(&host::translate(m.name()), TITLE_PX);
    t.bold(title);
    let lit = fill("{percent}% illuminated", &[("percent", &format!("{:.0}", m.illumination() * 100.0))]);
    let subtitle = t.text(&format!("{lit} · {}", host::translate(if m.waxing() { "Waxing" } else { "Waning" }).to_lowercase()), 0.0);
    t.tone(subtitle, Tone::Muted);
    let words = t.column(TIGHT_GAP, &[caption, title, subtitle]);
    t.grow(words);
    let row = t.row(HERO_GAP, &[art, words]);
    let n = t.node(row);
    n.id = "hero".into();
    n.action = "hero".into();
    n.hint = host::translate("Back to tonight");
    n.style.radius = HERO_RADIUS_PX;
    row
}

/// A settings-list row: a label, then its value at the trailing edge.
fn line(t: &mut Tree, lead: Option<u32>, label: &str, value: &str, detail: &str) -> u32 {
    let mut kids: Vec<u32> = lead.into_iter().collect();
    kids.push(t.text(label, 0.0));
    kids.push(t.add(K::Spacer, ""));
    kids.push(t.text(value, 0.0));
    if !detail.is_empty() {
        let d = t.text(detail, 0.0);
        t.tone(d, Tone::Muted);
        kids.push(d);
    }
    let row = t.row(ROW_GAP, &kids);
    t.grow(row);
    row
}

/// The look-ahead slider, in its group with the day it shows.
fn look_ahead(t: &mut Tree) -> u32 {
    let ahead = with(|s| s.ahead);
    let value = if ahead == 0 { host::translate("Tonight") } else { when(f64::from(ahead)) };
    let head = line(t, None, &host::translate("Look ahead"), &value, "");
    let slider = t.add(K::Slider, "");
    let n = t.node(slider);
    n.id = "ahead".into();
    n.action = "ahead".into();
    n.min = -LOOK_DAYS;
    n.max = LOOK_DAYS;
    n.step = 1.0;
    n.value = ahead as f32;
    n.hint = host::translate("Look ahead");
    let body = t.column(ROW_GAP, &[head, slider]);
    t.group(&[body])
}

fn mode_switch(t: &mut Tree) -> u32 {
    let mode = with(|s| s.mode);
    let choice = t.add(K::Choice, "");
    let n = t.node(choice);
    n.id = "mode".into();
    n.action = "mode".into();
    n.options = Mode::ALL.iter().map(|(_, key, label)| ui::OptionItem { value: (*key).into(), label: (*label).into() }).collect();
    n.index = Mode::ALL.iter().position(|(m, _, _)| *m == mode).unwrap_or(0) as u32;
    choice
}

/// The next full and new moon, the moon's age and its lit side.
fn overview(t: &mut Tree, m: &Moon, hemi: Hemisphere) -> u32 {
    let base = now_ms() + f64::from(with(|s| s.ahead)) * DAY_MS;
    let full_art = moon_art(t, &Moon::full_moon(), hemi, ROW_MOON_PX);
    let full = line(t, Some(full_art), &host::translate("Full moon"), &Date::from_ms(base + m.days_to_full() * DAY_MS).day_month(), &when(m.days_to_full()));
    let new_art = moon_art(t, &Moon::new_moon(), hemi, ROW_MOON_PX);
    let new = line(t, Some(new_art), &host::translate("New moon"), &Date::from_ms(base + m.days_to_new() * DAY_MS).day_month(), &when(m.days_to_new()));
    let age_icon = t.add(K::Icon, "clock");
    t.tone(age_icon, Tone::Muted);
    t.node(age_icon).style.size = ROW_MOON_PX;
    let age = line(t, Some(age_icon), &host::translate("Moon age"), &fill("{days} days", &[("days", &format!("{:.1}", m.age))]), "");
    let side_icon = t.add(K::Icon, "sun");
    t.tone(side_icon, Tone::Muted);
    t.node(side_icon).style.size = ROW_MOON_PX;
    let side = line(t, Some(side_icon), &host::translate("Lit side"), &host::translate(m.lit_edge(hemi)), "");
    t.group(&[full, new, age, side])
}

/// How lit the moon is on each day of this cycle, as soft bars; the
/// looked-at day in the accent colour.
fn cycle(t: &mut Tree, m: &Moon) -> u32 {
    let start = now_ms() + f64::from(with(|s| s.ahead)) * DAY_MS - m.age * DAY_MS;
    let today = (m.age.floor() as usize).min(CHART_BARS - 1);
    let width = 1.0 / CHART_BARS as f32;
    let marks: Vec<ui::Mark> = (0..CHART_BARS)
        .map(|day| {
            let lit = Moon::at(start + day as f64 * DAY_MS).illumination() as f32;
            let h = lit.max(CHART_FLOOR);
            let here = day == today;
            ui::Mark { shape: ui::Shape::Rect, x: day as f32 * width + width * CHART_BAR_GAP / 2.0, y: 1.0 - h, width: width * (1.0 - CHART_BAR_GAP), height: h, tone: if here { Tone::Accent } else { Tone::Normal }, opacity: if here { 1.0 } else { CHART_DIM }, radius: CHART_RADIUS_PX }
        })
        .collect();
    let chart = t.add(K::Canvas, "");
    let n = t.node(chart);
    n.marks = marks;
    n.style.height = CHART_H_PX;
    n.hint = host::translate("Illumination over this cycle");
    let mut legend = Vec::new();
    for (at, label) in ["New moon", "Full moon", "New moon"].into_iter().enumerate() {
        if at > 0 {
            legend.push(t.add(K::Spacer, ""));
        }
        let l = t.text(&host::translate(label), CAPTION_PX);
        t.tone(l, Tone::Muted);
        legend.push(l);
    }
    let legend = t.row(0.0, &legend);
    t.grow(legend);
    let caption = line(t, None, &host::translate("Cycle"), &fill("Day {day} of {month}", &[("day", &format!("{:.1}", m.age)), ("month", &format!("{SYNODIC:.1}"))]), "");
    let body = t.column(ROW_GAP, &[chart, legend]);
    t.group(&[caption, body])
}

/// The coming full and new moons, as a virtual list: Mandara draws the rows
/// on screen and asks for others as you scroll.
fn upcoming(t: &mut Tree, hemi: Hemisphere) -> u32 {
    let start = with(|s| s.list_start).min(UPCOMING);
    let first = Moon::at(now_ms());
    let rows: Vec<u32> = (start..(start + LIST_WINDOW).min(UPCOMING))
        .map(|i| {
            let (full, in_days) = first.event(i as usize);
            let art = moon_art(t, &if full { Moon::full_moon() } else { Moon::new_moon() }, hemi, ROW_MOON_PX);
            line(t, Some(art), &host::translate(if full { "Full moon" } else { "New moon" }), &Date::from_ms(now_ms() + in_days * DAY_MS).medium(), "")
        })
        .collect();
    let list = t.add(K::List, "");
    let n = t.node(list);
    n.id = "upcoming".into();
    n.action = "upcoming".into();
    n.index = UPCOMING;
    n.value = start as f32;
    n.style.height = LIST_H_PX;
    n.style.min_height = LIST_ROW_PX;
    t.children(list, &rows);
    t.group(&[list])
}

/// The date field (drawn in the theme's field design) and, below it in a
/// group, what it found.
fn finder(t: &mut Tree, hemi: Hemisphere) -> u32 {
    let (typed, found) = with(|s| (s.typed.clone(), s.found));
    let field = t.add(K::Input, &typed);
    let n = t.node(field);
    n.id = "date".into();
    n.action = "date".into();
    n.hint = host::translate("Find a date (YYYY-MM-DD)");
    let mut rows = vec![field];
    match found {
        Some(day) => {
            let m = Moon::at((day as f64 + 0.5) * DAY_MS);
            let art = moon_art(t, &m, hemi, ROW_MOON_PX);
            let answer = line(t, Some(art), &host::translate(m.name()), &Date::from_days(day).medium(), "");
            rows.push(t.group(&[answer]));
        }
        None if !typed.trim().is_empty() => {
            let hint = t.text(&host::translate("Type a date like 2026-12-24, then press Enter"), CAPTION_PX);
            t.tone(hint, Tone::Muted);
            rows.push(hint);
        }
        None => {}
    }
    t.column(ROW_GAP, &rows)
}

/// What happened last, and "Remind me" as a quiet text button.
fn footer(t: &mut Tree) -> u32 {
    let status = with(|s| s.status.clone());
    let note = t.text(&status, CAPTION_PX);
    t.tone(note, Tone::Muted);
    t.grow(note);
    let bell = t.add(K::Icon, "bell");
    t.tone(bell, Tone::Accent);
    let remind = t.add(K::Button, "Remind me");
    let n = t.node(remind);
    n.id = "remind".into();
    n.action = "remind".into();
    n.style.tone = Tone::Muted;
    t.children(remind, &[bell]);
    let row = t.row(ROW_GAP, &[note, remind]);
    t.grow(row);
    row
}

fn panel(compact: bool) -> ui::View {
    let hemi = hemisphere();
    let m = looked_at();
    let mut t = Tree::default();
    let mut rows = vec![header(&mut t, &m, hemi)];
    if compact {
        rows.push(overview(&mut t, &m, hemi));
    } else {
        rows.push(look_ahead(&mut t));
        rows.push(mode_switch(&mut t));
        rows.push(match with(|s| s.mode) {
            Mode::Overview => overview(&mut t, &m, hemi),
            Mode::Cycle => cycle(&mut t, &m),
            Mode::Upcoming => upcoming(&mut t, hemi),
        });
        rows.push(finder(&mut t, hemi));
        rows.push(footer(&mut t));
    }
    let root = t.column(SECTION_GAP, &rows);
    t.view(root, "")
}

/// The bar item: a moon icon and, unless switched off, the lit percentage.
fn bar() -> ui::View {
    let m = Moon::at(now_ms());
    let mut t = Tree::default();
    let icon = t.add(K::Icon, "moon");
    t.tone(icon, Tone::Accent);
    t.node(icon).style.size = BAR_ICON_PX;
    let mut kids = vec![icon];
    if show_percent() {
        kids.push(t.text(&format!("{:.0}%", m.illumination() * 100.0), 0.0));
    }
    let root = t.row(TIGHT_GAP, &kids);
    t.view(root, "")
}

/// The desktop widget: tonight's moon, drawn large, its name and how lit,
/// in the digits the person chose for it.
fn desktop(context: &ui::Context) -> ui::View {
    let height = context.height;
    let language = if context.language.is_empty() { host::locale() } else { context.language.clone() };
    let eastern = context.numerals == "eastern-arabic" || (context.numerals == "auto" && language == "ar");
    let hemi = hemisphere();
    let m = Moon::at(now_ms());
    let mut t = Tree::default();
    let size = (height * DESKTOP_MOON_SHARE).clamp(ROW_MOON_PX, HERO_MOON_PX * 2.0).round();
    let art = moon_art(&mut t, &m, hemi, size);
    let title = t.text(&host::translate(m.name()), TITLE_PX);
    t.bold(title);
    let lit = t.text(&fill("{percent}% illuminated", &[("percent", &tree::digits(&format!("{:.0}", m.illumination() * 100.0), eastern))]), 0.0);
    t.tone(lit, Tone::Muted);
    let words = t.column(TIGHT_GAP, &[title, lit]);
    let root = t.column(ROW_GAP, &[art, words]);
    t.view(root, "")
}

/// A round, quiet button showing a bundled icon; `hint` names it.
fn icon_button(t: &mut Tree, icon: &str, action: &str, hint: &str) -> u32 {
    let glyph = t.add(K::Icon, icon);
    let button = t.add(K::Button, "");
    let n = t.node(button);
    n.action = action.into();
    n.hint = host::translate(hint);
    t.children(button, &[glyph])
}

/// Studio → Desktop → Moon: a month of moons.
fn studio_tab(width: f32) -> ui::View {
    let hemi = hemisphere();
    let month = with(|s| s.month);
    let today = Date::from_ms(now_ms());
    let shown = today.add_months(month);
    let mut t = Tree::default();

    let mark = t.add(K::Image, "");
    let n = t.node(mark);
    n.image = broker::asset_image("moon.png").unwrap_or(0);
    n.style.width = MARK_PX;
    n.style.height = MARK_PX;
    n.style.radius = MARK_RADIUS_PX;
    n.hint = host::translate("Moon phase");
    let title = t.text(&host::translate("Moon calendar"), TITLE_PX);
    t.bold(title);
    let subtitle = t.text(&shown.month_year(), 0.0);
    t.tone(subtitle, Tone::Muted);
    let titles = t.column(TIGHT_GAP, &[title, subtitle]);
    t.grow(titles);
    let prev = icon_button(&mut t, "chevron-left", "prev-month", "Previous month");
    let this = t.add(K::Button, "Today");
    t.node(this).action = "this-month".into();
    t.node(this).disabled = month == 0;
    let next = icon_button(&mut t, "chevron-right", "next-month", "Next month");
    let nav = t.row(TIGHT_GAP, &[prev, this, next]);
    let head = t.row(HERO_GAP, &[mark, titles, nav]);

    // Seven columns, Monday first, the weekday names on top.
    let mut cells = Vec::new();
    for name in calendar::WEEKDAYS {
        let n = t.text(&host::translate(name).to_uppercase(), CAPTION_PX);
        t.tone(n, Tone::Muted);
        t.bold(n);
        let cell = t.column(0.0, &[n]);
        t.align(cell, ui::Align::Center);
        cells.push(cell);
    }
    let first = shown.first_of_month();
    for _ in 0..first.weekday() {
        cells.push(t.add(K::Spacer, ""));
    }
    for day in 0..first.days_in_month() {
        let date = Date::from_days(first.days + i64::from(day));
        let m = Moon::at((date.days as f64 + 0.5) * DAY_MS);
        let art = moon_art(&mut t, &m, hemi, DAY_MOON_PX);
        let number = t.text(&(day + 1).to_string(), CAPTION_PX);
        if date.days == today.days {
            t.tone(number, Tone::Accent);
            t.bold(number);
        } else {
            t.tone(number, Tone::Muted);
        }
        let cell = t.column(TIGHT_GAP, &[art, number]);
        t.align(cell, ui::Align::Center);
        t.node(cell).style.padding = CELL_PAD_PX;
        t.node(cell).hint = format!("{} · {}", date.long(), host::translate(m.name()));
        cells.push(cell);
    }
    let grid = t.add(K::Grid, "");
    t.node(grid).index = 7;
    t.node(grid).style.gap = TIGHT_GAP;
    t.children(grid, &cells);
    let calendar = t.group(&[grid]);

    let tonight = Moon::at(now_ms());
    let full_art = moon_art(&mut t, &Moon::full_moon(), hemi, ROW_MOON_PX);
    let full = line(&mut t, Some(full_art), &host::translate("Next full moon"), &day_in(tonight.days_to_full()).medium(), &when(tonight.days_to_full()));
    let new_art = moon_art(&mut t, &Moon::new_moon(), hemi, ROW_MOON_PX);
    let new = line(&mut t, Some(new_art), &host::translate("Next new moon"), &day_in(tonight.days_to_new()).medium(), &when(tonight.days_to_new()));
    let coming = t.group(&[full, new]);
    let tip = t.text(&host::translate("Type “moon ” in the launcher for the next full and new moons, or a date."), CAPTION_PX);
    t.tone(tip, Tone::Muted);
    let _ = width;
    let root = t.column(SECTION_GAP, &[head, calendar, coming, tip]);
    t.view(root, "")
}

// ── Events ───────────────────────────────────────────────────────────────

fn remind() {
    let tonight = Moon::at(now_ms());
    let title = fill("Full moon {when}", &[("when", &when(tonight.days_to_full()).to_lowercase())]);
    let body = day_in(tonight.days_to_full()).long();
    let payload = format!("{{\"title\": {}, \"body\": {}}}", json_string(&title), json_string(&body));
    let request = broker::Request { capability: "notifications.send".into(), operation: "send".into(), payload };
    match broker::submit(&request) {
        Ok(id) => with(|s| {
            s.reminder = Some(id);
            s.status = host::translate("Sending…");
        }),
        Err(why) => with(|s| s.status = denied(&why)),
    }
}

/// What to say when a request was refused.
fn denied(why: &str) -> String {
    if why.contains("granted") { host::translate("Allow notifications for this plugin in Studio → Plugins") } else { why.to_string() }
}

/// The integer field `key` of a flat JSON object (any JSON whitespace
/// around the colon), without a JSON library in the plugin.
fn json_int(json: &str, key: &str) -> Option<i64> {
    let rest = json.split(&format!("\"{key}\"")).nth(1)?;
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace()).strip_prefix(':')?.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// `s` as a JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn on_extension_event(event: ui::Event) {
    match (event.action.as_str(), event.value) {
        (_, Value::Replies(replies)) => {
            for r in replies {
                if with(|s| s.reminder) == Some(r.id) {
                    let status = match r.outcome {
                        Ok(_) => host::translate("Reminder sent"),
                        Err(why) => denied(&why),
                    };
                    with(|s| {
                        s.reminder = None;
                        s.status = status;
                    });
                }
            }
        }
        (_, Value::Launch(id)) => launch(&id),
        ("ahead", Value::Number(v)) => with(|s| s.ahead = v.round().clamp(-LOOK_DAYS, LOOK_DAYS) as i32),
        ("hero", Value::Activate) => with(|s| s.ahead = 0),
        ("hero", Value::Key(k)) => with(|s| match k.key.as_str() {
            "left" => s.ahead = (s.ahead - 1).max(-LOOK_DAYS as i32),
            "right" => s.ahead = (s.ahead + 1).min(LOOK_DAYS as i32),
            "home" => s.ahead = 0,
            _ => {}
        }),
        ("mode", Value::Choice(key)) => {
            with(|s| s.mode = Mode::parse(&key));
            // Remembered across restarts in the plugin's private storage.
            let _ = storage::set(MODE_KEY, &key);
        }
        (_, Value::Command(words)) => match words.first().map(String::as_str) {
            Some("remind") => remind(),
            Some("show") => match words.get(1).and_then(|d| Date::parse(d)) {
                Some(date) => launch(&format!("day:{}", date.days)),
                None => launch("tonight"),
            },
            _ => {}
        },
        ("upcoming", Value::Range((first, _))) => with(|s| s.list_start = first.min(UPCOMING)),
        ("date", Value::Text(text)) => with(|s| {
            s.typed = text.chars().take(calendar::MAX_DATE_CHARS).collect();
            s.found = None;
        }),
        ("date", Value::Submit(text)) => with(|s| {
            s.typed = text.chars().take(calendar::MAX_DATE_CHARS).collect();
            s.found = Date::parse(&s.typed).map(|d| d.days);
        }),
        ("remind", Value::Activate) => remind(),
        ("prev-month", Value::Activate) => with(|s| s.month -= 1),
        ("next-month", Value::Activate) => with(|s| s.month += 1),
        ("this-month", Value::Activate) => with(|s| s.month = 0),
        _ => {}
    }
}

/// A launcher result was chosen: show that day in the panel and open it.
fn launch(id: &str) {
    let today = Date::from_ms(now_ms()).days;
    let tonight = Moon::at(now_ms());
    let day = match id {
        "full" => Some(day_in(tonight.days_to_full()).days),
        "new" => Some(day_in(tonight.days_to_new()).days),
        "tonight" => Some(today),
        other => other.strip_prefix("day:").and_then(|d| d.parse::<i64>().ok()),
    };
    if let Some(day) = day {
        with(|s| {
            let ahead = day - today;
            if ahead.unsigned_abs() <= LOOK_DAYS as u64 {
                s.ahead = ahead as i32;
            } else {
                s.found = Some(day);
                s.typed = Date::from_days(day).iso();
            }
        });
        let _ = host::command("panel open moon-phase");
    }
}

fn launcher_results(query: &str, limit: u32) -> Vec<ui::Hit> {
    let tonight = Moon::at(now_ms());
    let hemi = hemisphere();
    // Each result leads with that day's moon.
    let hit = |id: String, title: String, detail: String, m: &Moon, keywords: &str| ui::Hit { id, title, detail, icon: "moon".into(), image: moon_image(m, hemi), keywords: keywords.into() };
    let q = query.trim().to_lowercase();
    if limit == 0 {
        return Vec::new();
    }
    if let Some(date) = Date::parse(&q) {
        let m = Moon::at((date.days as f64 + 0.5) * DAY_MS);
        return vec![hit(format!("day:{}", date.days), fill("{phase} on {date}", &[("phase", &host::translate(m.name())), ("date", &date.long())]), fill("{percent}% illuminated", &[("percent", &format!("{:.0}", m.illumination() * 100.0))]), &m, "date")];
    }
    let mut hits = vec![
        hit("tonight".into(), fill("Tonight: {phase}", &[("phase", &host::translate(tonight.name()))]), fill("{percent}% illuminated", &[("percent", &format!("{:.0}", tonight.illumination() * 100.0))]), &tonight, "tonight now today"),
        hit("full".into(), fill("Full moon {when}", &[("when", &when(tonight.days_to_full()).to_lowercase())]), day_in(tonight.days_to_full()).long(), &Moon::full_moon(), "full"),
        hit("new".into(), fill("New moon {when}", &[("when", &when(tonight.days_to_new()).to_lowercase())]), day_in(tonight.days_to_new()).long(), &Moon::new_moon(), "new dark"),
    ];
    if !q.is_empty() {
        hits.retain(|h| h.keywords.split(' ').any(|k| k.starts_with(q.as_str())) || h.title.to_lowercase().contains(q.as_str()));
    }
    hits.truncate((limit as usize).min(LAUNCHER_DEFAULTS));
    hits
}

struct Plugin;

impl Guest for Plugin {
    fn negotiate(_host: mandara::plugin::platform::HostInfo) -> Vec<String> {
        FEATURES.iter().map(|f| (*f).to_string()).collect()
    }

    fn init() -> Result<(), String> {
        if let Some(saved) = storage::get(MODE_KEY) {
            with(|s| s.mode = Mode::parse(&saved));
        }
        Ok(())
    }

    fn refresh_ms() -> u32 {
        60_000
    }

    fn render(_theme: Palette, context: ui::Context) -> Option<ui::View> {
        match context.surface {
            Surface::Bar => Some(bar()),
            Surface::Panel => Some(panel(false)),
            Surface::Sidebar => Some(panel(true)),
            Surface::Studio if context.outlet == "moon" => Some(studio_tab(context.width)),
            Surface::Desktop if context.outlet == "moon" => Some(desktop(&context)),
            _ => None,
        }
    }

    fn on_event(event: ui::Event) {
        on_extension_event(event);
    }

    fn launcher_query(provider: String, query: String, limit: u32) -> Vec<ui::Hit> {
        if provider == "moon" { launcher_results(&query, limit) } else { Vec::new() }
    }

    /// `phase` (plugin.toml `serves`): tonight's moon for other plugins,
    /// or a day's with `{"day": <days since 1970-01-01>}`.
    fn on_call(_caller: String, operation: String, payload: String) -> Result<String, String> {
        if operation != "phase" {
            return Err(format!("no operation {operation}"));
        }
        let day = json_int(&payload, "day");
        let m = match day {
            Some(day) => Moon::at((day as f64 + 0.5) * DAY_MS),
            None => Moon::at(now_ms()),
        };
        Ok(format!("{{\"name\": {}, \"illumination\": {:.3}, \"age\": {:.2}, \"waxing\": {}}}", json_string(m.name()), m.illumination(), m.age, m.waxing()))
    }
}

export!(Plugin);
