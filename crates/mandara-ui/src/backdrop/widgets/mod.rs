//! Desktop widgets on the software wallpaper canvas (`[desktop.widgets]`,
//! [`mandara_config::desktop_widgets`]): hero clocks, the system monitor
//! column, static decorations and the plane visualizer.
//!
//! Each widget owns two planes of its output's canvas, sized to what it
//! draws: a *static* plane (repainted when its content key changes — the
//! minute for clocks, never for decorations) and a *live* plane for what
//! changes in between (seconds, readings, the visualizer). Widgets set
//! `behind` sit between the wallpaper and the depth subject, the others
//! above it.
//!
//! Nothing here runs without enabled widgets: an empty list adds no plane,
//! no wake-up and no sampler. Live work also waits for the desktop to be
//! visible on that output (no fullscreen / tiled window over it, not
//! locked): the system monitor samples only then, the visualizer listens
//! only while music plays and stops after its silence timeout.

pub mod draw;
pub mod layout;
pub mod sysmon;
pub mod visualizer;

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike as _, Local, Timelike as _};
use image::RgbImage;
use mandara_config::Units;
use mandara_config::desktop_widgets::{ClockFace, ClockWidget, Depth, DesktopWidget, Insets, MonitorRow, MonitorWidget, WidgetKind, ranges};
use mandara_core::shm_canvas::{Place, Planes, Rect, snap};
use mandara_i18n::t;
use mandara_theme::Color;
use tiny_skia::Pixmap;

use self::draw::{Inks, Measurer, Style};
use self::layout::{ClockStrings, Composition, MonitorLine};
use crate::backdrop::paint::FontNames;
use crate::lock::paint::TextPainter;
use crate::widgets::clock::{arabic_numerals, localized};
use crate::widgets::system::format_bytes;

/// Plane index of widget `i`'s static plane (its live plane is the next).
/// The depth clock uses the planes below.
pub const PLANE_BASE: usize = 8;
/// Slack after a second / minute boundary before redrawing, so the new
/// value is current.
const TICK_SLACK: Duration = Duration::from_millis(15);
const MS_PER_SECOND: i64 = 1000;
const MS_PER_MINUTE: i64 = 60_000;
/// Share of a widget the subject may cover before the widget is drawn in
/// front of it instead of behind.
const HIDDEN_BEHIND_COVERAGE: f32 = 0.6;
/// Poll period while a visualizer waits for sound (music playing).
const VIS_POLL: Duration = Duration::from_millis(250);
/// Retry period of the spectrum subscription.
const SPECTRUM_RETRY: Duration = Duration::from_secs(2);
/// Region around a widget sampled for its automatic ink, as a fraction of
/// the box (centre ± half).
const TONE_HALF: f32 = 0.5;
/// Hours where the greeting changes.
const MORNING_FROM: u32 = 5;
const AFTERNOON_FROM: u32 = 12;
const EVENING_FROM: u32 = 17;
const NIGHT_FROM: u32 = 21;
const SECONDS_PER_DAY: f32 = 86_400.0;
const SECONDS_PER_HOUR: f32 = 3_600.0;
/// Hours on a 12-hour clock.
const HALF_DAY: u32 = 12;

pub fn planes(i: usize) -> (usize, usize) {
    (PLANE_BASE + 2 * i, PLANE_BASE + 2 * i + 1)
}

pub(crate) fn key(parts: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

/// What the renderer needs from the shell for the widgets.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetScene {
    /// Enabled widgets the canvas paints, in order.
    pub items: Vec<DesktopWidget>,
    pub on_surface: Color,
    /// `(icon, temperature)` of the current weather, when known.
    pub weather: Option<(&'static str, String)>,
    /// Outputs where the desktop is visible (live widgets run only there);
    /// filled only while a live widget exists.
    pub visible: Vec<String>,
    /// A player is playing (filled only while a visualizer exists).
    pub music: bool,
    pub units: Units,
    pub h24: bool,
    pub rtl: bool,
}

impl Default for WidgetScene {
    fn default() -> Self {
        Self { items: Vec::new(), on_surface: Color::oklch(0.95, 0.0, 0.0), weather: None, visible: Vec::new(), music: false, units: Units::default(), h24: true, rtl: false }
    }
}

impl WidgetScene {
    /// Any widget below the depth subject (the depth mask is needed).
    pub fn any_behind(&self) -> bool {
        self.items.iter().any(|w| w.place.depth == Depth::Behind)
    }

    /// Any widget whose content changes between minutes.
    pub fn any_live(&self) -> bool {
        self.items.iter().any(|w| is_live(&w.kind))
    }

    pub fn any_visualizer(&self) -> bool {
        self.items.iter().any(|w| matches!(w.kind, WidgetKind::Visualizer(_)))
    }
}

fn is_live(k: &WidgetKind) -> bool {
    match k {
        WidgetKind::Clock(c) => c.seconds && c.face == ClockFace::Superscript,
        WidgetKind::Monitor(_) | WidgetKind::Visualizer(_) => true,
        _ => false,
    }
}

/// One widget's planes on one output.
#[derive(Default)]
struct State {
    id: String,
    cfg: u64,
    static_key: Option<u64>,
    live_key: Option<u64>,
    rect: Rect,
    live_rect: Rect,
    /// Where the plane content may reach (grows; the depth cut covers it).
    envelope: Rect,
    behind: bool,
    /// The box [`mandara_config::desktop_widgets::Placement::origin_in`]
    /// placed (physical `x, y, w, h`): edit mode moves it.
    outer: (f32, f32, f32, f32),
    vis: Option<(visualizer::Geometry, visualizer::Painter)>,
    frame_at: Option<Instant>,
    inks: Option<Inks>,
}

/// The widgets of one output.
#[derive(Default)]
pub struct OutWidgets {
    states: Vec<State>,
}

impl OutWidgets {
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Each shown widget's id, painted bounds and placed box (physical
    /// px), for edit mode.
    pub fn geometry(&self) -> Vec<(String, Rect, (f32, f32, f32, f32))> {
        self.states
            .iter()
            .filter(|st| !st.id.is_empty())
            .map(|st| (st.id.clone(), st.rect.union(&st.live_rect), st.outer))
            .filter(|(_, r, _)| !r.is_empty())
            .collect()
    }

    /// Removes every widget plane of output `id` (buffers and surfaces)
    /// and forgets the widgets.
    pub fn remove_all(&mut self, host: &mut impl Planes, id: u32) {
        remove_unused(host, id, &Layers::default());
        self.states.clear();
    }
}

/// Removes every widget plane of output `id` that `layers` doesn't stack:
/// the plane set then matches the shown widgets exactly, whatever changed
/// (a widget removed, disabled, moved to another display or index, or
/// turned into a kind that uses fewer planes), and removed planes keep no
/// buffer or surface.
fn remove_unused(host: &mut impl Planes, id: u32, layers: &Layers) {
    for i in 0..ranges::MAX_WIDGETS {
        let (a, b) = planes(i);
        for p in [a, b] {
            if !layers.behind.contains(&p) && !layers.front.contains(&p) && host.has_plane(id, p) {
                host.remove_plane(id, p);
            }
        }
    }
}

/// Renderer-wide widget services.
#[derive(Default)]
pub struct Shared {
    pub sampler: Option<sysmon::Sampler>,
    tap: Option<mandara_services::spectrum::Spectrum>,
    tap_tried: Option<Instant>,
    silent_since: Option<Instant>,
    /// Stopped listening after the silence timeout (until music restarts).
    silenced: bool,
    music_was: bool,
}

impl Shared {
    pub fn listening(&self) -> bool {
        self.tap.is_some()
    }
}

/// Planes the widgets of an output contribute to its stacking order and
/// the area the depth subject must cover.
#[derive(Default)]
pub struct Layers {
    pub behind: Vec<usize>,
    pub front: Vec<usize>,
    pub reach: Rect,
}

/// An output as the widgets see it.
pub struct OutputCx<'a> {
    pub id: u32,
    pub names: &'a [&'a str],
    pub logical: (f32, f32),
    pub physical: (u32, u32),
    pub scale120: u32,
    /// Small copy of the wallpaper for automatic ink.
    pub tone: Option<&'a RgbImage>,
    pub primary: Color,
    pub fonts: &'a FontNames,
    /// Depth effect available (a subject cut-out is drawn).
    pub depth: bool,
    /// The shown subject's coverage map (output fractions), once cut out.
    pub subject: Option<&'a image::GrayImage>,
    /// Edges widgets keep clear of (the bar, the screen frame), physical px.
    pub safe: Insets,
}

impl OutputCx<'_> {
    fn output_rect(&self) -> Rect {
        Rect::new(0, 0, self.physical.0, self.physical.1)
    }

    /// Physical px per reference px at `scale`.
    fn k(&self, scale: f32) -> f32 {
        self.physical.1 as f32 / ranges::REFERENCE_HEIGHT_PX * scale
    }
}

/// Gap kept between desktop widgets and the bar / screen edges, logical px.
const SAFE_MARGIN_PX: f32 = 8.0;

/// What the desktop clock and widgets keep clear of on an output, logical
/// px: the bar's exclusive zone and the screen frame (where the bar shows),
/// plus a small margin from every edge.
pub fn safe_insets(cfg: &mandara_config::ShellConfig, bar_here: bool) -> Insets {
    use mandara_config::BarPosition;
    let mut i = Insets::default();
    if bar_here {
        let m = crate::bar::layout::Metrics::of(cfg, 1.0);
        let frame = m.screen_frame.max(0.0);
        i = Insets { top: frame, right: frame, bottom: frame, left: frame };
        let zone = crate::bar::layout::exclusive_zone(&m);
        match cfg.bar.position {
            BarPosition::Top => i.top = zone,
            BarPosition::Bottom => i.bottom = zone,
            BarPosition::Left => i.left = zone,
            BarPosition::Right => i.right = zone,
        }
    }
    Insets { top: i.top + SAFE_MARGIN_PX, right: i.right + SAFE_MARGIN_PX, bottom: i.bottom + SAFE_MARGIN_PX, left: i.left + SAFE_MARGIN_PX }
}

fn shows(w: &DesktopWidget, o: &OutputCx<'_>) -> bool {
    w.place.shows_on(o.names)
}

// ── Strings ──────────────────────────────────────────────────────────────

/// The greeting for `hour` (localized).
pub fn greeting(hour: u32) -> &'static str {
    match hour {
        h if (MORNING_FROM..AFTERNOON_FROM).contains(&h) => t("Good morning"),
        h if (AFTERNOON_FROM..EVENING_FROM).contains(&h) => t("Good afternoon"),
        h if (EVENING_FROM..NIGHT_FROM).contains(&h) => t("Good evening"),
        _ => t("Good night"),
    }
}

/// Every string of a clock at `now`.
pub fn clock_strings(now: &DateTime<Local>, c: &ClockWidget, h24: bool, weather: Option<&(&'static str, String)>) -> ClockStrings {
    let (h, m, s) = (now.hour(), now.minute(), now.second());
    let h12 = match h % HALF_DAY {
        0 => HALF_DAY,
        v => v,
    };
    // Stacked hours balance better as two digits.
    let hours = if h24 { format!("{h:02}") } else if c.face == ClockFace::Superscript { format!("{h12:02}") } else { h12.to_string() };
    let minutes = format!("{m:02}");
    let num = |v: String| if c.eastern_numerals { arabic_numerals(&v) } else { v };
    let date = now.date_naive();
    let secs = now.num_seconds_from_midnight() as f32;
    ClockStrings {
        time: num(format!("{hours}:{minutes}")),
        hours: num(hours),
        minutes: num(minutes),
        seconds: num(format!("{s:02}")),
        meridiem: (!h24).then(|| if h < HALF_DAY { t("AM") } else { t("PM") }.to_string()),
        weekday: localized(&date, "%A"),
        weekday_short: localized(&date, "%a"),
        day: num(date.day().to_string()),
        month: localized(&date, "%B"),
        numeric_date: num(localized(&date, t("%d/%m"))),
        day_month: num(localized(&date, t("%-d %B"))),
        greeting: greeting(h).to_string(),
        weather: if c.weather { weather.map(|(i, temp)| (*i, num(temp.clone()))) } else { None },
        hour_fraction: (m as f32 * 60.0 + s as f32) / SECONDS_PER_HOUR,
        day_fraction: secs / SECONDS_PER_DAY,
    }
}

/// `6.1 / 31.2 GB`: both in the total's unit.
fn pair_bytes(used: u64, total: u64) -> String {
    let t = format_bytes(total);
    match t.split_once(' ') {
        Some((num, unit)) => {
            let scale = total as f64 / num.parse::<f64>().unwrap_or(1.0).max(f64::EPSILON);
            format!("{:.1} / {num} {unit}", used as f64 / scale)
        }
        None => format!("{} / {t}", format_bytes(used)),
    }
}

fn percent(v: f32) -> String {
    format!("{:.0}%", (v * 100.0).clamp(0.0, 999.0))
}

/// The monitor's rows for the current reading (rows without a reading are
/// left out once the sampler has had a full sample).
pub fn monitor_lines(w: &MonitorWidget, s: Option<&sysmon::Sampler>, units: Units) -> Vec<MonitorLine> {
    let r = s.map(|s| &s.reading);
    let settled = s.is_some_and(|s| s.samples >= 2);
    let dash = || "—".to_string();
    let mut out = Vec::new();
    for row in &w.rows {
        let row = *row;
        let line = |label: &str, value: Option<String>| -> Option<MonitorLine> {
            if value.is_none() && settled {
                return None;
            }
            Some(MonitorLine { row: Some(row), label: label.to_string(), value: value.unwrap_or_else(dash), ..Default::default() })
        };
        let rate = |v: Option<f64>| v.map(|b| format!("{}{}", format_bytes(b as u64), t("/s")));
        let made = match row {
            MonitorRow::Cpu => line(t("CPU"), r.and_then(|r| r.cpu).map(percent)),
            MonitorRow::Gpu => line(t("GPU"), r.and_then(|r| r.gpu).map(percent)),
            MonitorRow::Memory => line(t("Memory"), r.and_then(|r| r.memory).map(|(u, tot)| pair_bytes(u, tot))),
            MonitorRow::GpuPower => line(t("GPU power"), r.and_then(|r| r.gpu_power_w).map(|w| format!("{w:.0} W"))),
            MonitorRow::CpuTemp => line(t("CPU temp"), r.and_then(|r| r.cpu_temp).map(|c| units.format_degrees(c))),
            MonitorRow::GpuTemp => line(t("GPU temp"), r.and_then(|r| r.gpu_temp).map(|c| units.format_degrees(c))),
            MonitorRow::Fan => line(t("Fan"), r.and_then(|r| r.fan_rpm).map(|v| format!("{v} {}", t("RPM")))),
            MonitorRow::Battery => line(t("Battery"), r.and_then(|r| r.battery).map(percent)),
            MonitorRow::Disk => line(t("Disk"), r.and_then(|r| r.disk).map(|(u, tot)| pair_bytes(u, tot))).map(|mut l| {
                l.bar = Some(r.and_then(|r| r.disk).map(|(u, tot)| u as f32 / tot.max(1) as f32).unwrap_or(0.0));
                l
            }),
            MonitorRow::Download => line(t("Download"), rate(r.and_then(|r| r.down))),
            MonitorRow::Upload => line(t("Upload"), rate(r.and_then(|r| r.up))),
            MonitorRow::CpuGraph => Some(MonitorLine { row: Some(row), graph: Some((s.map(|s| s.cpu_history.normalized(1.0)).unwrap_or_default(), None)), ..Default::default() }),
            MonitorRow::NetGraph => {
                let scale = s.map(|s| s.down_history.max().max(s.up_history.max())).unwrap_or(1.0).max(1.0);
                Some(MonitorLine { row: Some(row), graph: Some((s.map(|s| s.down_history.normalized(scale)).unwrap_or_default(), s.map(|s| s.up_history.normalized(scale)))), ..Default::default() })
            }
        };
        out.extend(made);
    }
    out
}

// ── Layout of one widget ─────────────────────────────────────────────────

/// A widget laid out on an output: its composition, where its box goes
/// (physical, absolute) and its rotation.
pub struct Laid {
    pub comp: Composition,
    pub origin: (f32, f32),
    pub k: f32,
    pub rotation: f32,
    /// The placed (possibly rotated) outer box, `x, y, w, h`.
    pub outer: (f32, f32, f32, f32),
}

/// Lays out `w` at `now` (`None` for visualizers, which have no marks).
pub fn lay_out(w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, sampler: Option<&sysmon::Sampler>, now: &DateTime<Local>, text: &mut TextPainter) -> Option<Laid> {
    let k = o.k(w.place.scale());
    let align = w.place.anchor.fraction().0;
    let mut m = Measurer { text, fonts: o.fonts };
    let (comp, rotation) = match &w.kind {
        WidgetKind::Clock(c) => (layout::clock(c, &clock_strings(now, c, scene.h24, scene.weather.as_ref()), k, align, scene.rtl, &mut m), 0.0),
        WidgetKind::Monitor(mw) => (layout::monitor(mw, &monitor_lines(mw, sampler, scene.units), k, scene.rtl, &mut m), 0.0),
        WidgetKind::Decoration(d) => (layout::decoration(d, k, d.sideways.clockwise(&w.place), &mut m), d.rotation.clamp(ranges::ANGLE_DEG.0, ranges::ANGLE_DEG.1)),
        WidgetKind::Visualizer(_) | WidgetKind::Card(_) => return None,
    };
    let margin = draw::effect_margin(look_of(&w.kind).map(|l| l.effect).unwrap_or_default(), k);
    let boxed = (comp.size.0 + 2.0 * margin, comp.size.1 + 2.0 * margin);
    let outer = if rotation != 0.0 { draw::rotated_size(boxed, rotation) } else { boxed };
    let (x, y) = w.place.origin_in((o.physical.0 as f32, o.physical.1 as f32), outer, o.safe);
    // The composition's origin inside the (possibly rotated) outer box.
    let origin = (x + (outer.0 - comp.size.0) / 2.0, y + (outer.1 - comp.size.1) / 2.0);
    Some(Laid { comp, origin, k, rotation, outer: (x, y, outer.0, outer.1) })
}

fn look_of(k: &WidgetKind) -> Option<&mandara_config::desktop_widgets::Look> {
    match k {
        WidgetKind::Clock(c) => Some(&c.look),
        WidgetKind::Monitor(m) => Some(&m.look),
        WidgetKind::Decoration(d) => Some(&d.look),
        WidgetKind::Visualizer(v) => Some(&v.look),
        WidgetKind::Card(_) => None,
    }
}

impl Laid {
    /// The plane rectangle holding the marks with `live == live`.
    pub fn rect(&self, live: bool, effect: mandara_config::desktop_widgets::Effect, scale120: u32, output: Rect) -> Rect {
        let margin = draw::effect_margin(effect, self.k);
        if self.rotation != 0.0 {
            // A rotated decoration is one static plane over its outer box.
            if live {
                return Rect::default();
            }
            let (w, h) = draw::rotated_size((self.comp.size.0 + 2.0 * margin, self.comp.size.1 + 2.0 * margin), self.rotation);
            let (cx, cy) = (self.origin.0 + self.comp.size.0 / 2.0, self.origin.1 + self.comp.size.1 / 2.0);
            return snap(Rect::from_edges(cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0), scale120).intersect(&output);
        }
        match self.comp.bounds(live) {
            Some((x0, y0, x1, y1)) => snap(Rect::from_edges(self.origin.0 + x0 - margin, self.origin.1 + y0 - margin, self.origin.0 + x1 + margin, self.origin.1 + y1 + margin), scale120).intersect(&output),
            None => Rect::default(),
        }
    }

    /// Paints into a plane at `rect`.
    pub fn paint(&self, pm: &mut tiny_skia::PixmapMut<'_>, text: &mut TextPainter, live: bool, rect: Rect, st: &Style<'_>) {
        if self.rotation == 0.0 {
            draw::paint(pm, text, &self.comp, live, (rect.x as f32 - self.origin.0, rect.y as f32 - self.origin.1), st);
            return;
        }
        let margin = draw::effect_margin(st.effect, self.k);
        let (w, h) = ((self.comp.size.0 + 2.0 * margin).ceil() as u32, (self.comp.size.1 + 2.0 * margin).ceil() as u32);
        let Some(mut tmp) = Pixmap::new(w.max(1), h.max(1)) else { return };
        draw::paint(&mut tmp.as_mut(), text, &self.comp, false, (-margin, -margin), st);
        let c = (self.origin.0 + self.comp.size.0 / 2.0 - rect.x as f32, self.origin.1 + self.comp.size.1 / 2.0 - rect.y as f32);
        draw::draw_rotated(pm, &tmp, self.rotation, c, 1.0);
    }
}

/// The widget's colours over the wallpaper under `rect`.
fn inks_for(w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, rect: Rect) -> Inks {
    let (pw, ph) = (o.physical.0.max(1) as f32, o.physical.1.max(1) as f32);
    let tone = o.tone.map(|t| {
        let c = ((rect.x as f32 + rect.w as f32 / 2.0) / pw, (rect.y as f32 + rect.h as f32 / 2.0) / ph);
        crate::desktop::region_tone(t, c, (rect.w as f32 / pw * TONE_HALF, rect.h as f32 / ph * TONE_HALF))
    });
    let auto = crate::desktop::ink_for(tone, o.primary);
    let default = mandara_config::desktop_widgets::Look::default();
    draw::inks(look_of(&w.kind).unwrap_or(&default), &auto, scene.on_surface)
}

// ── Static pass (from the clock layer, every wake) ───────────────────────

/// Paints every widget's static plane whose content changed (and, after a
/// layout change, its live plane), and returns the stacking / depth
/// information. Cheap when nothing changed: one hash per widget.
#[allow(clippy::too_many_arguments)]
pub fn step_static(host: &mut impl Planes, ws: &mut OutWidgets, o: &OutputCx<'_>, scene: &WidgetScene, shared: &Shared, text: &mut TextPainter, now: DateTime<Local>, wallpaper_key: u64) -> anyhow::Result<Layers> {
    let mut layers = Layers::default();
    let shown: Vec<(usize, &DesktopWidget)> = scene.items.iter().enumerate().take(ranges::MAX_WIDGETS).filter(|(_, w)| shows(w, o)).collect();
    // Forget widgets that went away or moved index (their planes are
    // removed at the end, with every other plane no shown widget stacks).
    let used: Vec<usize> = shown.iter().map(|(i, _)| *i).collect();
    for (i, st) in ws.states.iter_mut().enumerate() {
        if !used.contains(&i) {
            *st = State::default();
        }
    }
    ws.states.resize_with(scene.items.len().min(ranges::MAX_WIDGETS), State::default);
    let output = o.output_rect();
    let minute = now.timestamp().div_euclid(MS_PER_MINUTE / MS_PER_SECOND);
    let avail = shared.sampler.as_ref().map(|s| availability(&s.reading, s.samples));
    for (i, w) in shown {
        let (p_static, p_live) = planes(i);
        let st = &mut ws.states[i];
        let cfg = key(format!("{w:?}"));
        if st.id != w.id || st.cfg != cfg {
            *st = State { id: w.id.clone(), cfg, ..State::default() };
        }
        let opacity = w.place.opacity();
        if let WidgetKind::Visualizer(v) = &w.kind {
            let k = o.k(w.place.scale());
            let g = visualizer::Geometry::new(v, k);
            let (x, y) = w.place.origin_in((o.physical.0 as f32, o.physical.1 as f32), g.size, o.safe);
            let rect = snap(Rect::from_edges(x, y, x + g.size.0, y + g.size.1), o.scale120).intersect(&output);
            st.outer = (x, y, g.size.0, g.size.1);
            if st.live_rect != rect || st.vis.is_none() {
                st.live_rect = rect;
                st.vis = Some((g, visualizer::Painter::default()));
                st.live_key = None;
                st.inks = Some(inks_for(w, o, scene, rect));
            }
            st.envelope = st.envelope.union(&rect);
            st.behind = behind_subject(w, o, &st.envelope);
            push_layers(&mut layers, st, None, Some(p_live));
            continue;
        }
        let tick = match &w.kind {
            WidgetKind::Clock(_) => minute,
            _ => 0,
        };
        let skey = key(((cfg, tick, o.physical, o.scale120, wallpaper_key), (scene.on_surface.to_rgba8(), o.primary.to_rgba8(), o.fonts, &scene.weather), (scene.h24, scene.rtl, mandara_i18n::language(), if matches!(w.kind, WidgetKind::Monitor(_)) { avail } else { None })));
        if st.static_key != Some(skey) {
            let Some(laid) = lay_out(w, o, scene, shared.sampler.as_ref(), &now, text) else { continue };
            let effect = look_of(&w.kind).map(|l| l.effect).unwrap_or_default();
            let rect = laid.rect(false, effect, o.scale120, output);
            let live_rect = laid.rect(true, effect, o.scale120, output);
            st.outer = laid.outer;
            let inks = inks_for(w, o, scene, rect.union(&live_rect));
            let style = Style { fonts: o.fonts, inks, opacity, effect, k: laid.k };
            if rect.is_empty() {
                host.hide(o.id, p_static);
                st.static_key = Some(skey);
            } else if host.paint(o.id, p_static, Place::Rect(rect), None, false, true, |pm| laid.paint(pm, text, false, rect, &style))? {
                st.static_key = Some(skey);
            }
            st.rect = rect;
            if st.live_rect != live_rect || st.inks != Some(inks) {
                st.live_key = None;
            }
            st.live_rect = live_rect;
            st.inks = Some(inks);
            if live_rect.is_empty() {
                host.hide(o.id, p_live);
            } else if st.live_key.is_none() {
                // Paint the live part now (the live pass keeps it current).
                if host.paint(o.id, p_live, Place::Rect(live_rect), None, true, true, |pm| laid.paint(pm, text, true, live_rect, &style))? {
                    st.live_key = Some(live_tick_key(w, shared, &now));
                }
            }
            st.envelope = st.envelope.union(&rect).union(&live_rect);
        }
        st.behind = behind_subject(w, o, &st.envelope);
        let live = (!st.live_rect.is_empty()).then_some(p_live);
        push_layers(&mut layers, st, (!st.rect.is_empty()).then_some(p_static), live);
    }
    remove_unused(host, o.id, &layers);
    Ok(layers)
}

/// Whether `w` is drawn under the wallpaper's subject: set to `behind`,
/// with a subject to go behind — unless the subject would hide most of it
/// (then it goes in front, as the depth clock fades a subject that covers
/// its digits): a widget behind a landscape that fills the lower screen
/// was invisible, a visualizer there never seen to move.
fn behind_subject(w: &DesktopWidget, o: &OutputCx<'_>, area: &Rect) -> bool {
    if w.place.depth != Depth::Behind || !o.depth {
        return false;
    }
    let Some(map) = o.subject else { return true };
    if area.w == 0 || area.h == 0 {
        return true;
    }
    let (pw, ph) = (o.physical.0.max(1) as f32, o.physical.1.max(1) as f32);
    let fraction = (area.x as f32 / pw, area.y as f32 / ph, (area.x as f32 + area.w as f32) / pw, (area.y as f32 + area.h as f32) / ph);
    crate::depth::coverage(map, fraction) < HIDDEN_BEHIND_COVERAGE
}

fn push_layers(layers: &mut Layers, st: &State, a: Option<usize>, b: Option<usize>) {
    let list = if st.behind { &mut layers.behind } else { &mut layers.front };
    list.extend(a);
    list.extend(b);
    if st.behind {
        layers.reach = layers.reach.union(&st.envelope);
    }
}

/// Which monitor readings exist (keys the label plane).
fn availability(r: &sysmon::Reading, samples: u64) -> u64 {
    let bits = [r.cpu.is_some(), r.gpu.is_some(), r.memory.is_some(), r.disk.is_some(), r.gpu_power_w.is_some(), r.cpu_temp.is_some(), r.gpu_temp.is_some(), r.fan_rpm.is_some(), r.battery.is_some(), r.down.is_some(), samples >= 2];
    bits.iter().enumerate().fold(0, |acc, (i, b)| acc | ((*b as u64) << i))
}

/// The live content's tick: the second (seconds clocks) or the sample.
fn live_tick_key(w: &DesktopWidget, shared: &Shared, now: &DateTime<Local>) -> u64 {
    match &w.kind {
        WidgetKind::Monitor(_) => key(("sample", shared.sampler.as_ref().map(|s| s.generation))),
        _ => key(("second", now.timestamp())),
    }
}

// ── Live pass (seconds, readings, visualizer frames) ─────────────────────

/// What the sampler should read and how often, or `None` to drop it: only
/// while a monitor widget shows on an output whose desktop is visible.
pub fn sampler_wanted(scene: &WidgetScene, outputs: &[Vec<&str>]) -> Option<(sysmon::Wants, Duration)> {
    let mut wants = sysmon::Wants::default();
    let mut every: Option<u32> = None;
    for w in &scene.items {
        let WidgetKind::Monitor(m) = &w.kind else { continue };
        let on_visible = outputs.iter().any(|names| w.place.shows_on(names) && names.iter().any(|n| scene.visible.iter().any(|v| v == n)));
        if on_visible {
            wants.add(&m.rows);
            every = Some(every.map_or(m.interval_s(), |e| e.min(m.interval_s())));
        }
    }
    every.map(|s| (wants, Duration::from_secs(u64::from(s))))
}

/// Whether the visualizers should listen now. `silent_for` is how long the
/// spectrum has been quiet while listening.
pub fn should_listen(scene: &WidgetScene, outputs: &[Vec<&str>], silenced: bool) -> bool {
    scene.music
        && !silenced
        && scene.items.iter().any(|w| matches!(w.kind, WidgetKind::Visualizer(_)) && outputs.iter().any(|names| w.place.shows_on(names) && names.iter().any(|n| scene.visible.iter().any(|v| v == n))))
}

/// Shortest silence timeout of the visualizers.
fn silence_limit(scene: &WidgetScene) -> Duration {
    let s = scene.items.iter().filter_map(|w| match &w.kind {
        WidgetKind::Visualizer(v) => Some(v.silence_s.clamp(ranges::VISUALIZER_SILENCE_S.0, ranges::VISUALIZER_SILENCE_S.1)),
        _ => None,
    });
    Duration::from_secs(u64::from(s.min().unwrap_or(ranges::VISUALIZER_SILENCE_DEFAULT_S)))
}

/// Starts / stops the sampler and the spectrum subscription; samples when
/// due. `outputs` lists each output's names.
pub fn step_services(shared: &mut Shared, scene: &WidgetScene, outputs: &[Vec<&str>], now: Instant) {
    match sampler_wanted(scene, outputs) {
        Some((wants, every)) => {
            let s = shared.sampler.get_or_insert_with(sysmon::Sampler::new);
            if s.sampled_at.is_none_or(|t| now.saturating_duration_since(t) + TICK_SLACK >= every) {
                s.sample(wants, now);
            }
        }
        None => shared.sampler = None,
    }
    // Music starting again lifts a silence stop.
    if scene.music && !shared.music_was {
        shared.silenced = false;
    }
    shared.music_was = scene.music;
    if should_listen(scene, outputs, shared.silenced) {
        if shared.tap.is_none() && shared.tap_tried.is_none_or(|t| now.saturating_duration_since(t) >= SPECTRUM_RETRY) {
            shared.tap_tried = Some(now);
            shared.tap = mandara_services::spectrum::subscribe();
            shared.silent_since = None;
        }
        if let Some(tap) = &shared.tap {
            if tap.is_active() {
                shared.silent_since = None;
            } else if now.saturating_duration_since(*shared.silent_since.get_or_insert(now)) >= silence_limit(scene) {
                // Freeze: the planes keep their last frame; capture stops.
                shared.tap = None;
                shared.silenced = true;
                shared.silent_since = None;
            }
        }
    } else {
        shared.tap = None;
        shared.silent_since = None;
    }
}

/// Repaints the live planes that are due on output `o`. Returns whether
/// anything was painted (the caller commits).
#[allow(clippy::too_many_arguments)]
pub fn step_live(host: &mut impl Planes, ws: &mut OutWidgets, o: &OutputCx<'_>, scene: &WidgetScene, shared: &Shared, text: &mut TextPainter, now: DateTime<Local>, at: Instant) -> anyhow::Result<bool> {
    if !o.names.iter().any(|n| scene.visible.iter().any(|v| v == n)) {
        return Ok(false);
    }
    let mut painted = false;
    let bands = shared.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands());
    for (i, w) in scene.items.iter().enumerate().take(ranges::MAX_WIDGETS) {
        let Some(st) = ws.states.get_mut(i) else { break };
        if st.id != w.id || st.live_rect.is_empty() || !shows(w, o) {
            continue;
        }
        let (_, p_live) = planes(i);
        let rect = st.live_rect;
        let opacity = w.place.opacity();
        match &w.kind {
            WidgetKind::Visualizer(v) => {
                let Some(bands) = bands.as_ref() else { continue };
                let period = Duration::from_secs_f32(1.0 / v.fps() as f32);
                if st.frame_at.is_some_and(|t| at.saturating_duration_since(t) + TICK_SLACK < period) || !host.frame_ready(o.id) {
                    continue;
                }
                let accent = st.inks.map(|i| i.accent).unwrap_or(o.primary);
                let Some((g, painter)) = st.vis.as_mut() else { continue };
                let g = *g;
                if host.paint(o.id, p_live, Place::Rect(rect), None, true, true, |pm| painter.frame(pm, &g, bands, accent, opacity))? {
                    st.frame_at = Some(at);
                    painted = true;
                }
            }
            WidgetKind::Clock(_) | WidgetKind::Monitor(_) => {
                let k = live_tick_key(w, shared, &now);
                if st.live_key == Some(k) {
                    continue;
                }
                let Some(laid) = lay_out(w, o, scene, shared.sampler.as_ref(), &now, text) else { continue };
                let effect = look_of(&w.kind).map(|l| l.effect).unwrap_or_default();
                let Some(inks) = st.inks else { continue };
                let style = Style { fonts: o.fonts, inks, opacity, effect, k: laid.k };
                if host.paint(o.id, p_live, Place::Rect(rect), None, true, true, |pm| laid.paint(pm, text, true, rect, &style))? {
                    st.live_key = Some(k);
                    painted = true;
                }
            }
            _ => {}
        }
    }
    Ok(painted)
}

/// When the widgets of an output next need the renderer.
pub fn next_wake(ws: &OutWidgets, o_names: &[&str], scene: &WidgetScene, shared: &Shared, now: DateTime<Local>, at: Instant) -> Option<Duration> {
    let mut wake: Option<Duration> = None;
    let mut soon = |d: Duration| wake = Some(wake.map_or(d, |w| w.min(d)));
    let ms = now.timestamp_millis();
    let until = |period: i64| Duration::from_millis((period - ms.rem_euclid(period)).unsigned_abs()) + TICK_SLACK;
    let visible = o_names.iter().any(|n| scene.visible.iter().any(|v| v == n));
    for (i, w) in scene.items.iter().enumerate().take(ranges::MAX_WIDGETS) {
        if !w.place.shows_on(o_names) {
            continue;
        }
        match &w.kind {
            WidgetKind::Clock(c) => {
                soon(until(MS_PER_MINUTE));
                if visible && c.seconds && c.face == ClockFace::Superscript {
                    soon(until(MS_PER_SECOND));
                }
            }
            WidgetKind::Monitor(m) if visible => {
                let every = Duration::from_secs(u64::from(m.interval_s()));
                soon(shared.sampler.as_ref().and_then(|s| s.sampled_at).map_or(Duration::ZERO, |t| every.saturating_sub(at.saturating_duration_since(t))));
            }
            WidgetKind::Visualizer(v) if visible && scene.music && !shared.silenced => {
                if shared.tap.as_ref().is_some_and(|t| t.is_active()) {
                    let period = Duration::from_secs_f32(1.0 / v.fps() as f32);
                    let last = ws.states.get(i).and_then(|s| s.frame_at);
                    soon(last.map_or(Duration::ZERO, |t| period.saturating_sub(at.saturating_duration_since(t))));
                } else {
                    // Waiting for sound (or the silence timeout).
                    soon(VIS_POLL);
                }
            }
            _ => {}
        }
    }
    wake
}

// ── Offscreen (preview, Studio thumbnails) ───────────────────────────────

/// Paints `w` (static and live parts) over `canvas` (a full output at
/// `o.physical`), as the live canvas would show it.
#[allow(clippy::too_many_arguments)]
pub fn compose(canvas: &mut tiny_skia::PixmapMut<'_>, w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, sampler: Option<&sysmon::Sampler>, text: &mut TextPainter, now: &DateTime<Local>, bands: &[f32]) {
    let output = o.output_rect();
    let opacity = w.place.opacity();
    if let WidgetKind::Visualizer(v) = &w.kind {
        let g = visualizer::Geometry::new(v, o.k(w.place.scale()));
        let (x, y) = w.place.origin_in((o.physical.0 as f32, o.physical.1 as f32), g.size, o.safe);
        let rect = snap(Rect::from_edges(x, y, x + g.size.0, y + g.size.1), o.scale120).intersect(&output);
        let Some(mut pm) = Pixmap::new(rect.w.max(1), rect.h.max(1)) else { return };
        let inks = inks_for(w, o, scene, rect);
        visualizer::Painter::default().frame(&mut pm.as_mut(), &g, bands, inks.accent, opacity);
        crate::backdrop::paint::compose(canvas, &pm, rect, 1.0);
        return;
    }
    let Some(laid) = lay_out(w, o, scene, sampler, now, text) else { return };
    let effect = look_of(&w.kind).map(|l| l.effect).unwrap_or_default();
    let (rect, live) = (laid.rect(false, effect, o.scale120, output), laid.rect(true, effect, o.scale120, output));
    let inks = inks_for(w, o, scene, rect.union(&live));
    let style = Style { fonts: o.fonts, inks, opacity, effect, k: laid.k };
    for (r, is_live) in [(rect, false), (live, true)] {
        if r.is_empty() {
            continue;
        }
        let Some(mut pm) = Pixmap::new(r.w, r.h) else { continue };
        laid.paint(&mut pm.as_mut(), text, is_live, r, &style);
        crate::backdrop::paint::compose(canvas, &pm, r, 1.0);
    }
}

/// Paints every widget shown on `o` on one side of the depth subject
/// (`behind` it or in front), as the live canvas stacks them, over
/// `canvas`; returns the area the ones behind reach (the subject covers
/// it). A wallpaper crossfade bakes the incoming desktop with this.
#[allow(clippy::too_many_arguments)]
pub fn compose_side(canvas: &mut tiny_skia::PixmapMut<'_>, o: &OutputCx<'_>, scene: &WidgetScene, shared: &Shared, text: &mut TextPainter, now: &DateTime<Local>, behind: bool) -> Rect {
    let bands = shared.tap.as_ref().filter(|t| t.is_active()).map(|t| t.bands().to_vec());
    let mut reach = Rect::default();
    for w in scene.items.iter().take(ranges::MAX_WIDGETS).filter(|w| shows(w, o)) {
        let Some(area) = widget_rect(w, o, scene, text, now) else { continue };
        if behind_subject(w, o, &area) != behind {
            continue;
        }
        // A silent visualizer paints no frame on the live canvas either.
        match (&w.kind, &bands) {
            (WidgetKind::Visualizer(_), None) => {}
            (_, b) => compose(canvas, w, o, scene, shared.sampler.as_ref(), text, now, b.as_deref().unwrap_or_default()),
        }
        if behind {
            reach = reach.union(&area);
        }
    }
    reach
}

/// Where `w` draws on output `o` (physical px), for Studio's drag boxes.
pub fn widget_rect(w: &DesktopWidget, o: &OutputCx<'_>, scene: &WidgetScene, text: &mut TextPainter, now: &DateTime<Local>) -> Option<Rect> {
    let output = o.output_rect();
    if let WidgetKind::Visualizer(v) = &w.kind {
        let g = visualizer::Geometry::new(v, o.k(w.place.scale()));
        let (x, y) = w.place.origin_in((o.physical.0 as f32, o.physical.1 as f32), g.size, o.safe);
        return Some(Rect::from_edges(x, y, x + g.size.0, y + g.size.1).intersect(&output));
    }
    let laid = lay_out(w, o, scene, None, now, text)?;
    let effect = look_of(&w.kind).map(|l| l.effect).unwrap_or_default();
    Some(laid.rect(false, effect, o.scale120, output).union(&laid.rect(true, effect, o.scale120, output)))
}

/// A widget's id and box as fractions of a preview frame `[x, y, w, h]`.
pub type WidgetBox = (String, [f32; 4]);

/// A widget preview (all of `items` on a transparent `size` frame, as
/// straight RGBA) and each widget's box as fractions of the frame.
pub fn preview(items: &[DesktopWidget], size: (u32, u32), fonts: &FontNames, primary: Color, text: &mut TextPainter) -> Option<(image::RgbaImage, Vec<WidgetBox>)> {
    let mut canvas = Pixmap::new(size.0.max(1), size.1.max(1))?;
    let names: [&str; 0] = [];
    let o = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary, fonts, depth: false, subject: None, safe: Insets::default() };
    let scene = WidgetScene { items: items.to_vec(), weather: Some(("sun", "23°".into())), ..WidgetScene::default() };
    let now = Local::now();
    let bands = preview_bands();
    let mut boxes = Vec::new();
    let (fw, fh) = (size.0.max(1) as f32, size.1.max(1) as f32);
    for w in items {
        compose(&mut canvas.as_mut(), w, &o, &scene, None, text, &now, &bands);
        if let Some(r) = widget_rect(w, &o, &scene, text, &now) {
            boxes.push((w.id.clone(), [r.x as f32 / fw, r.y as f32 / fh, r.w as f32 / fw, r.h as f32 / fh]));
        }
    }
    Some((straight(canvas)?, boxes))
}

/// Straight RGBA → a GPUI image (BGRA), for Studio.
pub fn render_image(mut rgba: image::RgbaImage) -> std::sync::Arc<gpui::RenderImage> {
    for px in rgba.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    std::sync::Arc::new(gpui::RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))
}

fn straight(canvas: Pixmap) -> Option<image::RgbaImage> {
    let (w, h) = (canvas.width(), canvas.height());
    let mut img = image::RgbaImage::from_raw(w, h, canvas.take())?;
    for p in img.pixels_mut() {
        let a = p.0[3] as u32;
        if a > 0 && a < 255 {
            for c in 0..3 {
                p.0[c] = ((p.0[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some(img)
}

/// Example spectrum for previews (a gentle musical slope).
pub fn preview_bands() -> [f32; mandara_services::spectrum::BANDS] {
    let mut b = [0.0; mandara_services::spectrum::BANDS];
    for (i, v) in b.iter_mut().enumerate() {
        let f = i as f32 / mandara_services::spectrum::BANDS as f32;
        *v = (0.85 - 0.55 * f + 0.18 * (f * 23.0).sin()).clamp(0.05, 1.0);
    }
    b
}

/// A thumbnail of `w` alone on a transparent `w × h` frame (Studio's
/// gallery), as straight RGBA.
pub fn thumbnail(w: &DesktopWidget, size: (u32, u32), fonts: &FontNames, primary: Color, text: &mut TextPainter) -> Option<image::RgbaImage> {
    let mut canvas = Pixmap::new(size.0.max(1), size.1.max(1))?;
    let mut sample = sysmon::Sampler::new();
    if let WidgetKind::Monitor(m) = &w.kind {
        let mut wants = sysmon::Wants::default();
        wants.add(&m.rows);
        sample.sample(wants, Instant::now());
    }
    let names: [&str; 0] = [];
    let o = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary, fonts, depth: false, subject: None, safe: Insets::default() };
    let scene = WidgetScene { weather: Some(("sun", "23°".into())), ..WidgetScene::default() };
    compose(&mut canvas.as_mut(), w, &o, &scene, Some(&sample), text, &Local::now(), &preview_bands());
    straight(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop_widgets::{MonitorWidget, Placement, PlaneVisualizer};

    fn scene(items: Vec<DesktopWidget>) -> WidgetScene {
        WidgetScene { items, visible: vec!["DP-1".into()], music: true, ..Default::default() }
    }

    fn monitor_on(displays: &[&str]) -> DesktopWidget {
        DesktopWidget { id: "monitor-1".into(), place: Placement { displays: displays.iter().map(|d| d.to_string()).collect(), ..Default::default() }, kind: WidgetKind::Monitor(MonitorWidget { interval_s: 3, ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn sampler_runs_only_with_a_visible_monitor() {
        let outs = vec![vec!["DP-1"], vec!["HDMI-A-1"]];
        assert!(sampler_wanted(&scene(vec![]), &outs).is_none(), "no widget, no sampler");
        let (wants, every) = sampler_wanted(&scene(vec![monitor_on(&[])]), &outs).expect("visible monitor");
        assert!(wants.cpu && wants.net && every == Duration::from_secs(3));
        assert!(sampler_wanted(&scene(vec![monitor_on(&["HDMI-A-1"])]), &outs).is_none(), "its display is covered");
        let mut off = scene(vec![monitor_on(&[])]);
        off.visible.clear();
        assert!(sampler_wanted(&off, &outs).is_none(), "fullscreen / locked: paused");
        // The renderer drops the sampler as soon as it is not wanted.
        let mut shared = Shared::default();
        step_services(&mut shared, &scene(vec![monitor_on(&[])]), &outs, Instant::now());
        assert!(shared.sampler.as_ref().is_some_and(|s| s.samples == 1));
        step_services(&mut shared, &scene(vec![monitor_on(&[])]), &outs, Instant::now());
        assert_eq!(shared.sampler.as_ref().map(|s| s.samples), Some(1), "not due yet");
        step_services(&mut shared, &off, &outs, Instant::now());
        assert!(shared.sampler.is_none());
    }

    #[test]
    fn visualizer_listens_only_while_music_plays_and_stops_when_silent() {
        let outs = vec![vec!["DP-1"]];
        let vis = DesktopWidget { kind: WidgetKind::Visualizer(PlaneVisualizer { silence_s: 2, ..Default::default() }), ..Default::default() };
        let s = scene(vec![vis.clone()]);
        assert!(should_listen(&s, &outs, false));
        assert!(!should_listen(&s, &outs, true), "silenced");
        assert!(!should_listen(&WidgetScene { music: false, ..s.clone() }, &outs, false), "paused player");
        assert!(!should_listen(&WidgetScene { visible: vec![], ..s.clone() }, &outs, false), "desktop covered");
        assert!(!should_listen(&scene(vec![]), &outs, false));
        assert_eq!(silence_limit(&s), Duration::from_secs(2));
        // Silence: with no sound server the tap never starts, and nothing
        // keeps listening once the music stops.
        let mut shared = Shared::default();
        let t0 = Instant::now();
        step_services(&mut shared, &WidgetScene { music: false, ..s.clone() }, &outs, t0);
        assert!(!shared.listening());
        // A silence stop holds until the music restarts.
        shared.silenced = true;
        shared.music_was = true;
        step_services(&mut shared, &s, &outs, t0);
        assert!(shared.silenced && !shared.listening());
        step_services(&mut shared, &WidgetScene { music: false, ..s.clone() }, &outs, t0);
        step_services(&mut shared, &s, &outs, t0);
        assert!(!shared.silenced, "music restarting lifts the stop");
    }

    #[test]
    fn greetings_follow_the_hour() {
        assert_eq!(greeting(6), "Good morning");
        assert_eq!(greeting(13), "Good afternoon");
        assert_eq!(greeting(18), "Good evening");
        assert_eq!(greeting(23), "Good night");
        assert_eq!(greeting(2), "Good night");
    }

    #[test]
    fn clock_strings_respect_hours_and_numerals() {
        use chrono::TimeZone as _;
        let now = Local.with_ymd_and_hms(2026, 8, 22, 0, 9, 5).unwrap();
        let c = ClockWidget { face: ClockFace::Superscript, ..Default::default() };
        let s = clock_strings(&now, &c, false, None);
        assert_eq!((s.hours.as_str(), s.minutes.as_str(), s.time.as_str()), ("12", "09", "12:09"));
        assert_eq!(s.meridiem.as_deref(), Some("AM"));
        let h24 = clock_strings(&now, &ClockWidget { eastern_numerals: true, ..c.clone() }, true, Some(&("sun", "23°".into())));
        assert_eq!(h24.time, "٠٠:٠٩");
        assert!(h24.meridiem.is_none());
        assert_eq!(h24.weather.as_ref().map(|w| w.1.as_str()), Some("٢٣°"));
        assert!((s.day_fraction - (9.0 * 60.0 + 5.0) / SECONDS_PER_DAY).abs() < 1e-6);
    }

    #[test]
    fn byte_pairs_share_a_unit() {
        assert_eq!(pair_bytes(6_100_000_000, 31_200_000_000), "6.1 / 31.2 GB");
        let lines = monitor_lines(&MonitorWidget::default(), None, Units::Metric);
        assert!(lines.iter().any(|l| l.label == "CPU" && l.value == "—"), "unknown until sampled");
    }

    /// A compositor stand-in: which planes exist, which are mapped, the
    /// bytes their buffers hold and the rect each was last painted at.
    #[derive(Default)]
    struct Recorder {
        exists: std::collections::BTreeSet<usize>,
        bytes: std::collections::BTreeMap<usize, u64>,
        rects: std::collections::BTreeMap<usize, Rect>,
    }

    impl Planes for Recorder {
        fn paint(&mut self, _id: u32, index: usize, place: Place, _damage: Option<Rect>, _keep: bool, _clear: bool, paint: impl FnOnce(&mut tiny_skia::PixmapMut<'_>)) -> anyhow::Result<bool> {
            let Place::Rect(r) = place else { panic!("widgets paint plain rects") };
            let mut pm = Pixmap::new(r.w.max(1), r.h.max(1)).expect("plane pixmap");
            paint(&mut pm.as_mut());
            self.exists.insert(index);
            self.bytes.insert(index, u64::from(r.w.max(1)) * u64::from(r.h.max(1)) * 4);
            self.rects.insert(index, r);
            Ok(true)
        }

        fn hide(&mut self, _id: u32, index: usize) {
            self.bytes.remove(&index);
            self.rects.remove(&index);
        }

        fn remove_plane(&mut self, id: u32, index: usize) {
            self.hide(id, index);
            self.exists.remove(&index);
        }

        fn has_plane(&self, _id: u32, index: usize) -> bool {
            self.exists.contains(&index)
        }

        fn frame_ready(&self, _id: u32) -> bool {
            true
        }
    }

    impl Recorder {
        fn mapped(&self) -> Vec<usize> {
            self.bytes.keys().copied().collect()
        }
    }

    fn shape(id: &str, shape: mandara_config::desktop_widgets::DecorShape, anchor: mandara_config::desktop_widgets::Anchor) -> DesktopWidget {
        use mandara_config::desktop_widgets::Decoration;
        DesktopWidget { id: id.into(), place: Placement { anchor, ..Default::default() }, kind: WidgetKind::Decoration(Decoration { shape, ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn plane_set_matches_the_enabled_widgets_after_every_change() {
        use mandara_config::desktop_widgets::{Anchor, DecorShape, DesktopWidgetsConfig};
        let fonts = FontNames::default();
        let names = ["DP-1"];
        let o = OutputCx { id: 1, names: &names, logical: (1920.0, 1080.0), physical: (1920, 1080), scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: None, primary: Color::oklch(0.6, 0.15, 200.0), fonts: &fonts, depth: false, subject: None, safe: Insets::default() };
        let mut text = TextPainter::new();
        let mut host = Recorder::default();
        let mut ws = OutWidgets::default();
        let shared = Shared::default();
        let now = Local::now();
        let mut cfg = DesktopWidgetsConfig::default();
        // What the renderer is given: the enabled, painted widgets.
        let mut step = |cfg: &DesktopWidgetsConfig, host: &mut Recorder, ws: &mut OutWidgets| {
            let items: Vec<DesktopWidget> = cfg.painted().cloned().collect();
            step_static(host, ws, &o, &scene(items), &shared, &mut text, now, 0).expect("step");
        };
        let disc = shape("decoration-1", DecorShape::Disc, Anchor::TopLeft);
        let stripes = shape("decoration-2", DecorShape::Stripes, Anchor::BottomRight);
        let vis = DesktopWidget { id: "visualizer-1".into(), kind: WidgetKind::Visualizer(PlaneVisualizer::default()), ..Default::default() };
        let (d0, _) = planes(0);
        let (d1, l1) = planes(1);
        let (_, l2) = planes(2);

        // Add: each widget gets exactly the planes it draws.
        cfg.items = vec![disc.clone(), stripes.clone(), vis.clone()];
        step(&cfg, &mut host, &mut ws);
        // The visualizer's plane fills with its first frame (live pass).
        assert_eq!(host.mapped(), [d0, d1], "static planes painted");
        assert!(host.exists.iter().all(|p| [d0, d1].contains(p)));
        let disc_rect = host.rects[&d0];
        let stripes_rect = host.rects[&d1];

        // Remove: the stripes go, the visualizer moves up one index.
        cfg.items = vec![disc.clone(), vis.clone()];
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0], "the removed decoration's plane is gone");
        assert!(!host.has_plane(1, d1) && !host.bytes.contains_key(&d1), "and holds no buffer");
        assert!(!host.has_plane(1, l2), "nothing left at the visualizer's old index");

        // Disable: switched off widgets leave the canvas too.
        cfg.items = vec![disc.clone(), stripes.clone()];
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0, d1]);
        cfg.items[0].enabled = false;
        step(&cfg, &mut host, &mut ws);
        // The renderer is given the enabled widgets only: the stripes take
        // the first planes, the second pair goes.
        assert_eq!(host.mapped(), [d0], "the disabled disc's plane is gone");
        assert_eq!(host.rects[&d0], stripes_rect);
        assert!(!host.has_plane(1, d1));

        // Change kind: a decoration turned visualizer keeps only a live plane.
        cfg.items[0].enabled = true;
        cfg.items[1].kind = vis.kind.clone();
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0], "the decoration's static plane is gone");
        assert!(host.exists.iter().all(|p| *p == d0 || *p == l1));

        // Reorder: each plane shows the widget now at its index.
        cfg.items = vec![stripes.clone(), disc.clone()];
        step(&cfg, &mut host, &mut ws);
        assert_eq!(host.mapped(), [d0, d1]);
        assert_eq!((host.rects[&d0], host.rects[&d1]), (stripes_rect, disc_rect), "planes repainted for their new owners");
        assert!(!host.has_plane(1, l1));

        // Remove everything (a look with only the depth clock): no plane,
        // no buffer left.
        cfg.items.clear();
        step(&cfg, &mut host, &mut ws);
        assert!(host.exists.is_empty() && host.bytes.values().sum::<u64>() == 0, "{:?}", host.exists);
        assert!(ws.states.is_empty());
    }
}
