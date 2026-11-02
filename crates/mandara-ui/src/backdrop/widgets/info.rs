//! Information on the desktop: a machine card (like a terminal fetch),
//! screen time, the next prayer, a search field and an "at a glance" line.

use std::f32::consts::{FRAC_PI_2, TAU};

use chrono::{DateTime, Local, NaiveTime, Timelike as _};
use mandara_config::desktop_widgets::{FetchLayout, FetchWidget, GlanceWidget, PrayerLayout, PrayerWidget, ScreenTimeWidget, SearchWidget};
use mandara_config::widget_kinds::{count, ranges};
use mandara_core::shape::Shape;
use mandara_i18n::tk;

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, caps, face, fit, style, tracked};
use super::lingo::Lingo;
use super::{FetchInfo, GlanceInfo, PrayerInfo, ScreenInfo};

/// Reference sizes.
const TEXT: f32 = 17.0;
const SMALL: f32 = 13.0;
const BIG: f32 = 44.0;
const GAP: f32 = 12.0;
const LINE_GAP: f32 = 5.0;
const LOGO: f32 = 110.0;
const ICON: f32 = 20.0;
const SWATCH: f32 = 22.0;
const TILE_W: f32 = 150.0;
/// Widest a machine card's value runs before it is cut with an ellipsis
/// (a long processor name), reference px.
const VALUE_W: f32 = 300.0;
const TILE_H: f32 = 74.0;
const BAR_W: f32 = 24.0;
const BAR_H: f32 = 90.0;
const BAR_GAP: f32 = 10.0;
const RING_R: f32 = 92.0;
const RING_W: f32 = 8.0;
const LIST_W: f32 = 260.0;
const SEARCH_H: f32 = 52.0;
const TRACK_ALPHA: f32 = 0.18;
/// The prayer widget's Gregorian date, and the separator between dates.
const PRAYER_DATE: &str = "%-d %B";
const DATE_SEP: &str = " · ";
const TRACKING: f32 = 0.1;
const SEPARATOR: &str = "·";
/// Swatches in the fetch card's colour row (the palette's roles).
const SWATCHES: usize = 8;
const MINUTES_PER_DAY: f32 = 1440.0;
const SECS_PER_MINUTE: i64 = 60;
const MINUTES_PER_HOUR: i64 = 60;

/// `9 h 2 min` / `14 min` for a span of minutes.
fn span(minutes: i64, l: &Lingo) -> String {
    let (h, m) = (minutes / MINUTES_PER_HOUR, minutes % MINUTES_PER_HOUR);
    let text = if h > 0 { l.word(tk("{hours} h {minutes} min")).replace("{hours}", &h.to_string()) } else { l.word(tk("{minutes} min")) };
    l.digits(text.replace("{minutes}", &m.to_string()))
}

/// The machine card.
pub fn fetch(w: &FetchWidget, f: &FetchInfo, palette: &[(String, u32)], l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let up_min = (f.uptime_s / SECS_PER_MINUTE.unsigned_abs()) as i64;
    let mem = if f.memory.1 > 0 { format!("{} / {}", crate::widgets::system::format_bytes(f.memory.0), crate::widgets::system::format_bytes(f.memory.1)) } else { String::new() };
    let rows: Vec<(&'static str, &'static str, String)> = [
        ("monitor", tk("OS"), f.os.clone()),
        ("terminal", tk("Kernel"), f.kernel.clone()),
        ("window", tk("Compositor"), f.desktop.clone()),
        ("cpu", tk("CPU"), f.cpu.clone()),
        ("gauge", tk("GPU"), f.gpu.clone()),
        ("memory", tk("Memory"), mem),
        ("timer", tk("Uptime"), span(up_min, l)),
        ("terminal", tk("Shell"), f.shell.clone()),
        ("maximize", tk("Screen"), f.screen.clone()),
    ]
    .into_iter()
    .filter(|(_, _, v)| !v.trim().is_empty())
    .collect();
    let mut marks = Vec::new();
    let mono = w.layout == FetchLayout::Terminal;
    let font = if mono { mandara_config::desktop_widgets::FontRole::Mono } else { w.look.font };
    let ts = style(font, px(TEXT), face::MEDIUM);
    let ks = style(font, px(TEXT), face::BOLD);
    let logo = px(LOGO);
    let text_x = if w.logo { logo + px(GAP) * 2.0 } else { 0.0 };
    let mut y = 0.0;
    match w.layout {
        FetchLayout::Terminal => {
            let head = format!("{}@{}", f.user, f.host);
            let (hw, hh) = m.measure(&head, &ks);
            marks.push(Mark::new(MarkKind::Text { text: head.clone(), style: ks.clone(), x: text_x, y, w: hw, h: hh }, InkRole::Accent));
            y += hh;
            let rule = "─".repeat(head.chars().count());
            let (rw, rh) = m.measure(&rule, &ts);
            marks.push(Mark::new(MarkKind::Text { text: rule, style: ts.clone(), x: text_x, y, w: rw, h: rh }, InkRole::Fg).dim(face::DIM));
            y += rh;
            // In right-to-left text the key leads from the right and the
            // value follows to its left.
            let sizes: Vec<((String, f32, f32), (String, f32, f32))> = rows
                .iter()
                .map(|(_, key, value)| {
                    let key = if l.rtl { format!(" :{}", l.word(key)) } else { format!("{}: ", l.word(key)) };
                    let (kw, kh) = m.measure(&key, &ks);
                    let value = fit(value, &ts, px(VALUE_W), m);
                    let (vw, vh) = m.measure(&value, &ts);
                    ((key, kw, kh), (value, vw, vh))
                })
                .collect();
            let right = text_x + sizes.iter().map(|((_, kw, _), (_, vw, _))| kw + vw).fold(hw, f32::max);
            for ((key, kw, kh), (value, vw, vh)) in sizes {
                let (kx, vx) = if l.rtl { (right - kw, right - kw - vw) } else { (text_x, text_x + kw) };
                marks.push(Mark::new(MarkKind::Text { text: key, style: ks.clone(), x: kx, y, w: kw, h: kh }, InkRole::Accent));
                marks.push(Mark::new(MarkKind::Text { text: value, style: ts.clone(), x: vx, y, w: vw, h: vh }, InkRole::Fg));
                y += kh.max(vh);
            }
        }
        FetchLayout::Rows => {
            let label_w = rows.iter().map(|(_, k0, _)| m.measure(&l.word(k0), &ts).0).fold(0.0, f32::max);
            for (icon, key, value) in &rows {
                let (_, rh) = m.measure(value, &ts);
                marks.push(Mark::new(MarkKind::Icon { name: icon, x: text_x, y: y + (rh - px(ICON)) / 2.0, size: px(ICON) }, InkRole::Accent));
                let key = l.word(key);
                let (kw, kh) = m.measure(&key, &ts);
                marks.push(Mark::new(MarkKind::Text { text: key, style: ts.clone(), x: text_x + px(ICON) + px(GAP), y, w: kw, h: kh }, InkRole::Fg).dim(face::DIM));
                let value = fit(value, &ks, px(VALUE_W), m);
                let (vw, vh) = m.measure(&value, &ks);
                marks.push(Mark::new(MarkKind::Text { text: value, style: ks.clone(), x: text_x + px(ICON) + px(GAP) * 2.0 + label_w, y, w: vw, h: vh }, InkRole::Fg));
                y += rh.max(vh) + px(LINE_GAP);
            }
        }
        FetchLayout::Grid => {
            let (tw, th) = (px(TILE_W), px(TILE_H));
            let ls = tracked(style(font, px(SMALL), face::BOLD), TRACKING);
            for (i, (icon, key, value)) in rows.iter().enumerate() {
                let (c, r) = (i % 3, i / 3);
                let c = if l.rtl { 2 - c } else { c };
                let (x, ty) = (text_x + c as f32 * (tw + px(GAP)), r as f32 * (th + px(GAP)));
                marks.push(Mark::new(MarkKind::Rect { x, y: ty, w: tw, h: th, radius: px(GAP), width: 0.0 }, InkRole::Pastel((i * 40) as u16)));
                marks.push(Mark::new(MarkKind::Icon { name: icon, x: x + px(GAP), y: ty + px(GAP), size: px(ICON) }, InkRole::OnPaper));
                let key = caps(&l.word(key));
                let (kw, kh) = m.measure(&key, &ls);
                marks.push(Mark::new(MarkKind::Text { text: key, style: ls.clone(), x: x + px(ICON) + px(GAP) * 1.5, y: ty + px(GAP) + (px(ICON) - kh) / 2.0, w: kw, h: kh }, InkRole::OnPaper).dim(face::DIM));
                let value = fit(value, &ks, tw - 2.0 * px(GAP), m);
                let (vw, vh) = m.measure(&value, &ks);
                marks.push(Mark::new(MarkKind::Text { text: value, style: ks.clone(), x: x + px(GAP), y: ty + th - px(GAP) - vh, w: vw, h: vh }, InkRole::OnPaper));
                y = y.max(ty + th);
            }
        }
    }
    if w.colors && !palette.is_empty() {
        y += px(GAP);
        for (i, (_, rgb)) in palette.iter().take(SWATCHES).enumerate() {
            marks.push(Mark::new(MarkKind::Rect { x: text_x + i as f32 * px(SWATCH), y, w: px(SWATCH), h: px(SWATCH) * 0.7, radius: 0.0, width: 0.0 }, InkRole::Rgb(*rgb)));
        }
    }
    if w.logo {
        match &f.logo {
            Some(p) => marks.push(Mark::new(MarkKind::Image { x: 0.0, y: 0.0, w: logo, h: logo, pic: p.clone(), shape: Shape::SQUARE, turn: 0.0 }, InkRole::Fg)),
            None => marks.push(Mark::new(MarkKind::Icon { name: "monitor", x: 0.0, y: 0.0, size: logo }, InkRole::Accent)),
        }
    }
    if l.rtl && w.layout != FetchLayout::Terminal {
        let width = marks.iter().map(|m| m.bounds().2).fold(0.0, f32::max);
        for mk in &mut marks {
            let (x0, _, x1, _) = mk.bounds();
            mk.shift(width - x1 - x0, 0.0);
        }
    }
    Composition::of(marks)
}

/// Screen time: today's total, a bar per day and the top apps.
pub fn screen_time(w: &ScreenTimeWidget, s: &ScreenInfo, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let hm = |secs: u64| span((secs / SECS_PER_MINUTE.unsigned_abs()) as i64, l);
    let mut marks = Vec::new();
    let today = s.days.last().map_or(0, |d| d.1);
    let ls = tracked(style(w.look.font, px(SMALL), face::BOLD), TRACKING);
    let label = caps(&l.word(tk("Screen time today")));
    let (lw, lh) = m.measure(&label, &ls);
    let bs = style(w.look.font, px(BIG), face::SEMIBOLD);
    let total = hm(today);
    let (tw, th) = m.measure(&total, &bs);
    let days: Vec<&(String, u64, bool)> = s.days.iter().rev().take(count(w.days, ranges::SCREEN_DAYS) as usize).collect::<Vec<_>>().into_iter().rev().collect();
    let bars_w = days.len() as f32 * (px(BAR_W) + px(BAR_GAP));
    let width = tw.max(lw).max(bars_w).max(px(LIST_W));
    let ax = |w0: f32| if l.rtl { width - w0 } else { 0.0 };
    marks.push(Mark::new(MarkKind::Text { text: label, style: ls.clone(), x: ax(lw), y: 0.0, w: lw, h: lh }, InkRole::Accent));
    marks.push(Mark::new(MarkKind::Text { text: total, style: bs, x: ax(tw), y: lh, w: tw, h: th }, InkRole::Fg));
    let mut y = lh + th + px(GAP);
    if days.len() > 1 {
        let most = days.iter().map(|d| d.1).max().unwrap_or(1).max(1) as f32;
        let ds = style(w.look.font, px(SMALL), face::MEDIUM);
        for (i, (name, secs, is_today)) in days.iter().enumerate() {
            // Days run left to right like any chart.
            let x = i as f32 * (px(BAR_W) + px(BAR_GAP));
            let h = (px(BAR_H) * *secs as f32 / most).max(px(BAR_W) / 2.0);
            marks.push(Mark::new(MarkKind::Rect { x, y, w: px(BAR_W), h: px(BAR_H), radius: px(BAR_W) / 3.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA));
            marks.push(Mark::new(MarkKind::Rect { x, y: y + px(BAR_H) - h, w: px(BAR_W), h, radius: px(BAR_W) / 3.0, width: 0.0 }, if *is_today { InkRole::Accent } else { InkRole::Fg }).dim(if *is_today { 1.0 } else { face::DIM }));
            let (nw, nh) = m.measure(name, &ds);
            marks.push(Mark::new(MarkKind::Text { text: name.clone(), style: ds.clone(), x: x + (px(BAR_W) - nw) / 2.0, y: y + px(BAR_H) + px(LINE_GAP), w: nw, h: nh }, InkRole::Fg).dim(face::DIM));
        }
        y += px(BAR_H) + px(LINE_GAP) + m.measure("M", &ds).1 + px(GAP);
    }
    let ts = style(w.look.font, px(TEXT), face::MEDIUM);
    for (name, secs) in s.top.iter().take(count(w.apps, ranges::SCREEN_APPS) as usize) {
        let value = hm(*secs);
        let (vw, vh) = m.measure(&value, &ts);
        let name = fit(name, &ts, width - vw - px(GAP), m);
        let (nw, nh) = m.measure(&name, &ts);
        let (nx, vx) = if l.rtl { (width - nw, 0.0) } else { (0.0, width - vw) };
        marks.push(Mark::new(MarkKind::Text { text: name, style: ts.clone(), x: nx, y, w: nw, h: nh }, InkRole::Fg));
        marks.push(Mark::new(MarkKind::Text { text: value, style: ts.clone(), x: vx, y, w: vw, h: vh }, InkRole::Fg).dim(face::DIM));
        let f = *secs as f32 / today.max(1) as f32;
        let bar_y = y + nh.max(vh) + px(LINE_GAP) / 2.0;
        marks.push(Mark::new(MarkKind::Rect { x: 0.0, y: bar_y, w: width, h: px(LINE_GAP) / 2.0, radius: px(LINE_GAP), width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA));
        let fw = (width * f).max(px(LINE_GAP));
        marks.push(Mark::new(MarkKind::Rect { x: if l.rtl { width - fw } else { 0.0 }, y: bar_y, w: fw, h: px(LINE_GAP) / 2.0, radius: px(LINE_GAP), width: 0.0 }, InkRole::Accent));
        y = bar_y + px(GAP);
    }
    let mut c = Composition::of(marks);
    c.hits.push(Hit { rect: (0.0, 0.0, c.size.0, c.size.1), action: Action::Panel(crate::plugins::screen_time::ID.into()) });
    c
}

/// The name of a prayer in the widget's words (or Arabic).
/// The prayer widget's date line in its own language, digits and calendar
/// (Hijri with the year, Gregorian, both, or none).
fn prayer_date(p: &PrayerInfo, now: &DateTime<Local>, l: &Lingo) -> String {
    let g = l.calendar.gregorian().then(|| l.date(&now.date_naive(), PRAYER_DATE));
    let h = l.calendar.hijri().then_some(p.hijri).flatten().map(|h| l.hijri_full(h));
    [g, h].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(DATE_SEP)
}

fn prayer_name(p: &PrayerInfo, i: usize, arabic: bool, l: &Lingo) -> String {
    if arabic { p.times[i].1.to_string() } else { l.word(p.times[i].0) }
}

/// The next prayer: its name and time and the time to it, in a ring of
/// the day (the prayers as ticks), or a list with the next one lit.
pub fn prayer(w: &PrayerWidget, p: &PrayerInfo, now: &DateTime<Local>, l: &Lingo, h24: bool, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let t = now.time();
    let fmt = |time: NaiveTime| l.digits(if h24 { time.format("%H:%M").to_string() } else { time.format("%-I:%M").to_string() });
    // The next prayer (Sunrise is a marker, not a prayer), else tomorrow's Fajr.
    let next = p.times.iter().position(|(key, _, time)| *key != tk("Sunrise") && *time > t);
    let (next_name, next_time, mins) = match next {
        Some(i) => (prayer_name(p, i, w.arabic_names, l), p.times[i].2, (p.times[i].2 - t).num_minutes()),
        None => (prayer_name(p, 0, w.arabic_names, l), p.tomorrow_fajr, (p.tomorrow_fajr - t).num_minutes() + MINUTES_PER_DAY as i64),
    };
    let mut marks = Vec::new();
    match w.layout {
        PrayerLayout::Ring => {
            let (r, rw) = (px(RING_R), px(RING_W));
            let c = (r + rw, r + rw);
            let at = |time: NaiveTime| -FRAC_PI_2 + TAU * time.num_seconds_from_midnight() as f32 / (MINUTES_PER_DAY * SECS_PER_MINUTE as f32);
            marks.push(Mark::new(MarkKind::Arc { c, r, width: rw, start: 0.0, sweep: TAU }, InkRole::Fg).dim(TRACK_ALPHA));
            // The day so far, from midnight.
            let gone = TAU * t.num_seconds_from_midnight() as f32 / (MINUTES_PER_DAY * SECS_PER_MINUTE as f32);
            marks.push(Mark::new(MarkKind::Arc { c, r, width: rw, start: -FRAC_PI_2, sweep: gone }, InkRole::Accent).dim(face::DIM));
            for (i, (key, _, time)) in p.times.iter().enumerate() {
                let a = at(*time);
                let is_next = next == Some(i);
                let d = if is_next { rw * 0.9 } else { rw * 0.45 };
                marks.push(Mark::new(MarkKind::Disc { c: (c.0 + r * a.cos(), c.1 + r * a.sin()), r: d }, if is_next { InkRole::Accent } else { InkRole::Fg }).dim(if *key == tk("Sunrise") { face::DIM } else { 1.0 }));
            }
            let ns = style(w.look.font, px(TEXT) * 1.3, face::SEMIBOLD);
            let ts = style(w.look.font, px(BIG) * 0.8, face::SEMIBOLD);
            let ss = style(w.look.font, px(SMALL), face::MEDIUM);
            let left = l.word(tk("in {time}")).replace("{time}", &span(mins.max(0), l));
            let lines = [(next_name, ns, InkRole::Accent, 1.0), (fmt(next_time), ts, InkRole::Fg, 1.0), (left, ss, InkRole::Fg, face::DIM)];
            let sizes: Vec<(f32, f32)> = lines.iter().map(|(text, st, ..)| m.measure(text, st)).collect();
            let total: f32 = sizes.iter().map(|s| s.1).sum();
            let mut y = c.1 - total / 2.0;
            for ((text, st, ink, alpha), (tw, th)) in lines.into_iter().zip(sizes) {
                marks.push(Mark::new(MarkKind::Text { text, style: st, x: c.0 - tw / 2.0, y, w: tw, h: th }, ink).dim(alpha));
                y += th;
            }
            let date = prayer_date(p, now, l);
            if !date.is_empty() {
                let (hw, hh) = m.measure(&date, &style(w.look.font, px(SMALL), face::MEDIUM));
                marks.push(Mark::new(MarkKind::Text { text: date, style: style(w.look.font, px(SMALL), face::MEDIUM), x: c.0 - hw / 2.0, y: 2.0 * (r + rw) + px(GAP), w: hw, h: hh }, InkRole::Fg).dim(face::DIM));
            }
        }
        PrayerLayout::List => {
            let width = px(LIST_W);
            let ts = style(w.look.font, px(TEXT), face::MEDIUM);
            let bs = style(w.look.font, px(TEXT), face::BOLD);
            let mut y = 0.0;
            let date = prayer_date(p, now, l);
            if !date.is_empty() {
                let hs = tracked(style(w.look.font, px(SMALL), face::BOLD), TRACKING);
                let (hw, hh) = m.measure(&date, &hs);
                marks.push(Mark::new(MarkKind::Text { text: date, style: hs, x: if l.rtl { width - hw } else { 0.0 }, y, w: hw, h: hh }, InkRole::Accent));
                y += hh + px(GAP);
            }
            for (i, (_, _, time)) in p.times.iter().enumerate() {
                let is_next = next == Some(i);
                let st = if is_next { &bs } else { &ts };
                let name = prayer_name(p, i, w.arabic_names, l);
                let (nw, nh) = m.measure(&name, st);
                let tt = fmt(*time);
                let (tw, th) = m.measure(&tt, st);
                let h = nh.max(th) + px(LINE_GAP) * 2.0;
                if is_next {
                    marks.push(Mark::new(MarkKind::Rect { x: -px(GAP) / 2.0, y, w: width + px(GAP), h, radius: h / 2.0, width: 0.0 }, InkRole::Accent));
                }
                let ink = if is_next { InkRole::OnAccent } else { InkRole::Fg };
                let (nx, tx) = if l.rtl { (width - nw, 0.0) } else { (0.0, width - tw) };
                marks.push(Mark::new(MarkKind::Text { text: name, style: st.clone(), x: nx, y: y + (h - nh) / 2.0, w: nw, h: nh }, ink));
                marks.push(Mark::new(MarkKind::Text { text: tt, style: st.clone(), x: tx, y: y + (h - th) / 2.0, w: tw, h: th }, ink));
                y += h;
            }
            let left = l.word(tk("{prayer} in {time}")).replace("{prayer}", &next_name).replace("{time}", &span(mins.max(0), l));
            let ss = style(w.look.font, px(SMALL), face::MEDIUM);
            let (lw, lh) = m.measure(&left, &ss);
            marks.push(Mark::new(MarkKind::Text { text: left, style: ss, x: if l.rtl { width - lw } else { 0.0 }, y: y + px(GAP), w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
        }
    }
    let mut c = Composition::of(marks);
    c.hits.push(Hit { rect: (0.0, 0.0, c.size.0, c.size.1), action: Action::Panel(crate::plugins::prayer::ID.into()) });
    c
}

/// A search field; a click opens the launcher.
pub fn search(w: &SearchWidget, l: &Lingo, k: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let (sw, sh) = (px(w.width.clamp(ranges::WIDTH_PX.0, ranges::WIDTH_PX.1)), px(SEARCH_H));
    let shape = match shape_of(w.shape, &w.look, k) {
        Shape::Rect { .. } => Shape::Rect { radius: sh / 2.0 },
        s => s,
    };
    let mut marks = vec![Mark::new(MarkKind::Shaped { x: 0.0, y: 0.0, w: sw, h: sh, shape, width: 0.0 }, InkRole::Plate)];
    marks.push(Mark::new(MarkKind::Shaped { x: 0.0, y: 0.0, w: sw, h: sh, shape, width: 1.0 }, InkRole::OnPlate).dim(TRACK_ALPHA));
    let icon = px(ICON) * 1.1;
    let pad = sh / 2.0 - icon / 2.0 + px(LINE_GAP);
    let text = if w.placeholder.trim().is_empty() { l.word(tk("Search apps, files and more")) } else { w.placeholder.trim().to_string() };
    let ts = style(w.look.font, px(TEXT), face::MEDIUM);
    let text = fit(&text, &ts, sw - icon - pad * 3.0, m);
    let (tw, th) = m.measure(&text, &ts);
    let (ix, tx) = if l.rtl { (sw - pad - icon, sw - pad * 2.0 - icon - tw) } else { (pad, pad * 2.0 + icon) };
    marks.push(Mark::new(MarkKind::Icon { name: "search", x: ix, y: (sh - icon) / 2.0, size: icon }, InkRole::OnPlate));
    marks.push(Mark::new(MarkKind::Text { text, style: ts, x: tx, y: (sh - th) / 2.0, w: tw, h: th }, InkRole::OnPlate).dim(face::DIM));
    Composition::with_hits(marks, vec![Hit { rect: (0.0, 0.0, sw, sh), action: Action::Search }])
}

/// At a glance: the date (and the weather) over the one thing worth
/// knowing now.
#[allow(clippy::too_many_arguments)]
pub fn glance(w: &GlanceWidget, g: &GlanceInfo, weather: Option<&(&'static str, String)>, now: &DateTime<Local>, l: &Lingo, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let ds = style(w.look.font, px(BIG) * 0.7, face::SEMIBOLD);
    let date = l.date(&now.date_naive(), tk("%A, %-d %B"));
    let mut head = date;
    if w.weather
        && let Some((_, temp)) = weather
    {
        head = format!("{head}  {SEPARATOR}  {}", l.digits(temp.clone()));
    }
    let (hw, hh) = m.measure(&head, &ds);
    let ns = style(w.look.font, px(TEXT) * 1.15, face::MEDIUM);
    let note = g.text.clone();
    let (nw, nh) = m.measure(&note, &ns);
    let icon = px(ICON);
    let width = hw.max(nw + icon + px(GAP));
    let mut marks = vec![Mark::new(MarkKind::Text { text: head, style: ds, x: (width - hw) * align, y: 0.0, w: hw, h: hh }, InkRole::Fg)];
    let row_w = nw + icon + px(GAP);
    let x0 = (width - row_w) * align;
    let (ix, tx) = if l.rtl { (x0 + nw + px(GAP), x0) } else { (x0, x0 + icon + px(GAP)) };
    let y = hh + px(LINE_GAP);
    marks.push(Mark::new(MarkKind::Icon { name: g.icon, x: ix, y: y + (nh - icon) / 2.0, size: icon }, InkRole::Accent));
    marks.push(Mark::new(MarkKind::Text { text: note, style: ns, x: tx, y, w: nw, h: nh }, InkRole::Fg).dim(face::DIM));
    let mut c = Composition::of(marks);
    if let Some(a) = g.action.clone() {
        c.hits.push(Hit { rect: (0.0, 0.0, c.size.0, c.size.1), action: a });
    }
    c
}


#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;
    use chrono::TimeZone as _;

    fn l() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    #[test]
    fn the_prayer_date_follows_the_widgets_language_and_calendar() {
        use mandara_config::desktop_widgets::{Calendar, TextOptions};
        let now = Local.with_ymd_and_hms(2026, 11, 1, 13, 0, 0).unwrap();
        let p = PrayerInfo { times: Vec::new(), tomorrow_fajr: NaiveTime::MIN, hijri: Some((1448, 5, 21)), lang: None };
        let arabic = TextOptions { language: "ar".into(), calendar: Calendar::Hijri, hijri_in_arabic: true, ..Default::default() };
        let text = prayer_date(&p, &now, &Lingo::of(&arabic, true, None));
        assert!(!text.contains("Jumada") && text.contains('٢') && text.chars().any(|c| ('\u{0621}'..='\u{064A}').contains(&c)), "{text}");
        let none = TextOptions { calendar: Calendar::None, ..Default::default() };
        assert_eq!(prayer_date(&p, &now, &Lingo::of(&none, false, None)), "", "no date when asked for none");
        let gregorian = TextOptions { calendar: Calendar::Gregorian, ..Default::default() };
        assert!(prayer_date(&p, &now, &Lingo::of(&gregorian, false, None)).contains("November"));
    }

    #[test]
    fn info_widgets_lay_out() {
        let now = Local.with_ymd_and_hms(2026, 3, 4, 13, 0, 0).unwrap();
        let f = FetchInfo { user: "demo".into(), host: "mandara-demo".into(), os: "Arch Linux".into(), kernel: "6.1".into(), desktop: "MangoWM".into(), cpu: "Ryzen".into(), gpu: "Radeon".into(), memory: (1 << 30, 1 << 34), uptime_s: 7200, shell: "fish".into(), screen: "1920×1080".into(), logo: None };
        for layout in [FetchLayout::Terminal, FetchLayout::Rows, FetchLayout::Grid] {
            let c = fetch(&FetchWidget { layout, ..Default::default() }, &f, &[("Primary".into(), 0x123456)], &l(), 1.0, &mut Fixed);
            assert!(c.size.0 > 100.0, "{layout:?}");
        }
        let s = ScreenInfo { days: vec![("Mon".into(), 3600, false), ("Tue".into(), 7200, true)], top: vec![("Browser".into(), 3000)] };
        assert!(screen_time(&ScreenTimeWidget::default(), &s, &l(), 1.0, &mut Fixed).hits.len() == 1);
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let p = PrayerInfo { times: vec![(tk("Fajr"), "الفجر", t(4, 50)), (tk("Sunrise"), "الشروق", t(6, 10)), (tk("Dhuhr"), "الظهر", t(11, 50)), (tk("Asr"), "العصر", t(15, 10)), (tk("Maghrib"), "المغرب", t(17, 40)), (tk("Isha"), "العشاء", t(19, 0))], tomorrow_fajr: t(4, 49), hijri: None, lang: None };
        for layout in [PrayerLayout::Ring, PrayerLayout::List] {
            let c = prayer(&PrayerWidget { layout, ..Default::default() }, &p, &now, &l(), true, 1.0, &mut Fixed);
            assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text.contains("Asr"))), "{layout:?}: Asr is next");
        }
        assert_eq!(search(&SearchWidget::default(), &l(), 1.0, &mut Fixed).hits[0].action, Action::Search);
        let g = GlanceInfo { icon: "calendar", text: "Dentist in 20 min".into(), action: None };
        assert!(glance(&GlanceWidget::default(), &g, Some(&("sun", "23°".into())), &now, &l(), 1.0, 0.0, &mut Fixed).size.0 > 50.0);
    }

    #[test]
    fn long_machine_values_stay_inside_the_card() {
        let long = "AMD Ryzen 9 9950X 16-Core Processor with a much longer name than fits".to_string();
        let f = FetchInfo { user: "demo".into(), host: "mandara-demo".into(), os: "CachyOS".into(), cpu: long.clone(), gpu: long, memory: (1 << 30, 1 << 34), ..Default::default() };
        for rtl in [false, true] {
            let lingo = Lingo::of(&Default::default(), rtl, None);
            for layout in [FetchLayout::Terminal, FetchLayout::Rows, FetchLayout::Grid] {
                let w = FetchWidget { layout, ..Default::default() };
                let c = fetch(&w, &f, &[], &lingo, 1.0, &mut Fixed);
                let most = LOGO + 2.0 * GAP + 3.0 * (TILE_W + GAP) + ICON + VALUE_W + 2.0 * GAP + 200.0;
                assert!(c.size.0 <= most, "{layout:?} rtl={rtl}: {} wide", c.size.0);
                assert!(c.marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text.ends_with('…'))), "{layout:?}: the long name is cut");
                for mk in &c.marks {
                    let (x0, _, x1, _) = mk.bounds();
                    assert!(x0 >= -0.01 && x1 <= c.size.0 + 0.01, "{layout:?}: a mark runs outside the card");
                }
            }
        }
    }
}
