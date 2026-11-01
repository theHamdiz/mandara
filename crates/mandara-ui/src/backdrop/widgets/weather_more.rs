//! More weather layouts: a friendly sentence about the sky, a round disc
//! with the temperature, and the coming hours as a curve.

use std::f32::consts::TAU;

use mandara_config::desktop_widgets::WeatherWidget;
use mandara_config::widget_kinds::{count, ranges, within};
use mandara_i18n::tk;

use super::layout::{InkRole, Mark, MarkKind, Measure, Run, WeatherStrings, caps, face, line, run, stack, style, tracked, wrap};
use super::lingo::Lingo;
use super::Forecast;

/// Reference sizes.
const ICON: f32 = 56.0;
const CONDITION: f32 = 22.0;
const SENTENCE: f32 = 34.0;
const SENTENCE_W: f32 = 520.0;
const SENTENCE_LINES: usize = 4;
const GAP: f32 = 10.0;
const DISC_ICON: f32 = 0.24;
const DISC_TEMP: f32 = 0.26;
const DISC_SMALL: f32 = 0.065;
const RING_DOTS: usize = 60;
const RING_DOT: f32 = 0.009;
const RING_INSET: f32 = 0.06;
const RING_ALPHA: f32 = 0.35;
const HOUR_W: f32 = 54.0;
const CURVE_H: f32 = 90.0;
const CURVE_W: f32 = 3.0;
const HOUR_ICON: f32 = 26.0;
const HOUR_TEXT: f32 = 15.0;
const POINT: f32 = 4.0;
const FILL_ALPHA: f32 = 0.18;
/// Points per hour of the smoothed curve.
const SMOOTH_STEPS: usize = 10;
const TRACKING: f32 = 0.12;
/// Hours on a 12-hour clock.
const HALF_DAY: u32 = 12;

/// A friendly sentence per condition (clear by day, clear by night,
/// partly cloudy, cloudy, fog, drizzle, rain, snow, thunder).
pub const SENTENCES: [&str; 9] = [
    tk("A clear sky. A good day to be outside."),
    tk("A clear night. Look up, the stars are out."),
    tk("Some clouds, some sun. Easy weather."),
    tk("Grey skies today. Keep something warm close."),
    tk("Fog out there. Take it slow."),
    tk("A light drizzle. A hood will do."),
    tk("Rain today. Don't forget an umbrella."),
    tk("Snow is falling. Wrap up warm."),
    tk("Thunderstorms around. Better stay in."),
];

/// The sentence for the forecast: your own for the condition, else the
/// built-in one.
pub fn sentence(w: &WeatherWidget, f: &Forecast, l: &Lingo) -> String {
    if let Some(own) = w.sentences.get(f.kind).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return own.to_string();
    }
    // Clear splits into day and night; the rest shift by one.
    let i = match (f.kind, f.is_day) {
        (0, true) => 0,
        (0, false) => 1,
        (k, _) => (k + 1).min(SENTENCES.len() - 1),
    };
    l.word(SENTENCES[i])
}

/// The icon and condition over a sentence.
pub fn sentence_layout(w: &WeatherWidget, s: &WeatherStrings, text: &str, k: f32, align: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let role = w.look.font;
    let cs = tracked(style(role, px(CONDITION), face::SEMIBOLD), TRACKING);
    let head = format!("{} · {}", s.temp, caps(&s.condition));
    let (mut top, (tw, th)) = line(vec![run(head, cs, InkRole::Accent)], px(ICON) + px(GAP), 0.0, s.rtl, m);
    let icon = px(ICON);
    let ix = if s.rtl { tw + px(GAP) } else { 0.0 };
    if s.rtl {
        for mk in &mut top {
            mk.shift(-(px(ICON) + px(GAP)), 0.0);
        }
    }
    top.push(Mark::new(MarkKind::Icon { name: s.icon, x: ix, y: (th - icon) / 2.0, size: icon }, InkRole::Fg));
    let mut lines = vec![(top, (tw + icon + px(GAP), th.max(icon)))];
    let ss = style(role, px(SENTENCE), face::LIGHT);
    for t in wrap(text, &ss, px(SENTENCE_W), SENTENCE_LINES, m) {
        lines.push(line(vec![run(t, ss.clone(), InkRole::Fg)], 0.0, 0.0, s.rtl, m));
    }
    if !s.place.is_empty() {
        lines.push(line(vec![Run { opacity: face::DIM, ..run(&s.place, style(role, px(CONDITION) * 0.8, face::MEDIUM), InkRole::Fg) }], 0.0, 0.0, s.rtl, m));
    }
    stack(lines, px(GAP), align)
}

/// A round disc: the icon, a big temperature, today's range and an
/// optional ring of dots.
pub fn disc_layout(w: &WeatherWidget, s: &WeatherStrings, f: Option<&Forecast>, l: &Lingo, units: mandara_config::Units, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let d = within(w.disc, ranges::DISC_PX, ranges::DISC_DEFAULT_PX) * k;
    let c = (d / 2.0, d / 2.0);
    let mut marks = vec![Mark::new(MarkKind::Disc { c, r: d / 2.0 }, InkRole::Plate).dim(w.look.plate_opacity().max(face::DIM))];
    if w.dotted_ring {
        let rr = d / 2.0 - d * RING_INSET;
        for i in 0..RING_DOTS {
            let a = i as f32 / RING_DOTS as f32 * TAU;
            marks.push(Mark::new(MarkKind::Disc { c: (c.0 + rr * a.cos(), c.1 + rr * a.sin()), r: d * RING_DOT }, InkRole::OnPlate).dim(RING_ALPHA));
        }
    }
    let icon = d * DISC_ICON;
    marks.push(Mark::new(MarkKind::Icon { name: s.icon, x: c.0 - icon / 2.0, y: d * 0.16, size: icon }, InkRole::OnPlate));
    let ts = style(w.look.font, d * DISC_TEMP, face::SEMIBOLD);
    let (tw, th) = m.measure(&s.temp, &ts);
    marks.push(Mark::new(MarkKind::Text { text: s.temp.clone(), style: ts, x: c.0 - tw / 2.0, y: d * 0.42, w: tw, h: th }, InkRole::OnPlate));
    let small = caps(&match f.and_then(|f| f.today) {
        Some((lo, hi)) => format!("{} · {}/{}", s.condition, l.digits(units.format_degrees(lo)), l.digits(units.format_degrees(hi))),
        None => s.condition.clone(),
    });
    let ss = tracked(style(w.look.font, d * DISC_SMALL, face::SEMIBOLD), TRACKING);
    let (sw, sh) = m.measure(&small, &ss);
    let fits = sw <= d * 0.78;
    let text = if fits { small } else { caps(&s.condition) };
    let (sw, _) = if fits { (sw, sh) } else { m.measure(&text, &ss) };
    marks.push(Mark::new(MarkKind::Text { text, style: ss, x: c.0 - sw / 2.0, y: d * 0.42 + th, w: sw, h: sh }, InkRole::OnPlate).dim(face::DIM));
    marks
}

/// The coming hours as a smooth temperature curve, an icon and a
/// temperature over each, the hour under it.
pub fn hourly_layout(w: &WeatherWidget, f: Option<&Forecast>, l: &Lingo, units: mandara_config::Units, h24: bool, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let Some(f) = f.filter(|f| f.hours.len() >= 2) else {
        let text = l.word(tk("Weather unavailable"));
        let st = style(w.look.font, px(HOUR_TEXT), face::MEDIUM);
        let (tw, th) = m.measure(&text, &st);
        return vec![Mark::new(MarkKind::Text { text, style: st, x: 0.0, y: 0.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM)];
    };
    let hours: Vec<&(u32, &'static str, f32)> = f.hours.iter().take(count(w.hours, ranges::HOURS) as usize).collect();
    let n = hours.len();
    let cw = px(HOUR_W);
    let icon = px(HOUR_ICON);
    let ts = style(w.look.font, px(HOUR_TEXT), face::SEMIBOLD);
    let hs = style(w.look.font, px(HOUR_TEXT) * 0.9, face::MEDIUM);
    let text_h = m.measure("0", &ts).1;
    let top = icon + text_h + px(GAP);
    let (lo, hi) = hours.iter().fold((f32::MAX, f32::MIN), |(a, b), (_, _, t)| (a.min(*t), b.max(*t)));
    let span = (hi - lo).max(1.0);
    // A chart: time runs left to right in every language.
    let pt = |i: usize, t: f32| (cw * (i as f32 + 0.5), top + px(CURVE_H) * (1.0 - (t - lo) / span));
    let pts: Vec<(f32, f32)> = hours.iter().enumerate().map(|(i, (_, _, t))| pt(i, *t)).collect();
    // A Catmull-Rom spline through the hours.
    let mut curve = Vec::with_capacity(n * SMOOTH_STEPS);
    let ordered = &pts;
    for i in 0..ordered.len() - 1 {
        let p0 = ordered[i.saturating_sub(1)];
        let (p1, p2) = (ordered[i], ordered[i + 1]);
        let p3 = ordered[(i + 2).min(ordered.len() - 1)];
        for s in 0..SMOOTH_STEPS {
            let t = s as f32 / SMOOTH_STEPS as f32;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            curve.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    curve.push(*ordered.last().unwrap_or(&(0.0, 0.0)));
    let bottom = top + px(CURVE_H);
    let mut marks = Vec::new();
    if w.fill {
        let mut area = curve.clone();
        area.push((curve.last().map_or(0.0, |p| p.0), bottom));
        area.push((curve.first().map_or(0.0, |p| p.0), bottom));
        marks.push(Mark::new(MarkKind::Poly { pts: area }, InkRole::Accent).dim(FILL_ALPHA));
    }
    marks.push(Mark::new(MarkKind::Polyline { pts: curve, width: px(CURVE_W) }, InkRole::Accent));
    for (i, (hour, ic, t)) in hours.iter().enumerate() {
        let (x, y) = pts[i];
        marks.push(Mark::new(MarkKind::Disc { c: (x, y), r: px(POINT) }, InkRole::Accent));
        let temp = l.digits(units.format_degrees(*t));
        let (tw, th) = m.measure(&temp, &ts);
        marks.push(Mark::new(MarkKind::Icon { name: ic, x: x - icon / 2.0, y: 0.0, size: icon }, InkRole::Fg));
        marks.push(Mark::new(MarkKind::Text { text: temp, style: ts.clone(), x: x - tw / 2.0, y: icon, w: tw, h: th }, InkRole::Fg));
        let label = l.digits(if h24 { format!("{hour:02}") } else { format!("{}", if hour % HALF_DAY == 0 { HALF_DAY } else { hour % HALF_DAY }) });
        let (lw, lh) = m.measure(&label, &hs);
        marks.push(Mark::new(MarkKind::Text { text: label, style: hs.clone(), x: x - lw / 2.0, y: bottom + px(GAP), w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
    }
    marks
}
