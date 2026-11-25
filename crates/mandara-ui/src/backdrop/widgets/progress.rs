//! The progress widget: how much of the hour, day, week, month or year has
//! passed, as a ring, a wavy ring, a bar or a grid of dots, with
//! `62 % of today · 9 h 2 min left` under it. It redraws with the minute.

use std::f32::consts::{FRAC_PI_2, TAU};

use chrono::{DateTime, Datelike as _, Duration, Local, NaiveDate, TimeZone as _, Timelike as _};
use mandara_config::desktop_widgets::{Period, ProgressStyle, ProgressWidget};
use mandara_i18n::tk;

use super::layout::{Composition, InkRole, Mark, MarkKind, Measure, Run, face, line, run, stack, style, tracked};
use super::lingo::Lingo;

/// Reference sizes.
const RING_R: f32 = 110.0;
const PERCENT: f32 = 56.0;
const LABEL: f32 = 20.0;
const LABEL_GAP: f32 = 14.0;
const BAR_W: f32 = 420.0;
const TRACK_ALPHA: f32 = 0.2;
const DOT: f32 = 11.0;
const DOT_GAP: f32 = 5.0;
const TODAY_RING_W: f32 = 2.0;
/// The wavy ring: waves round the ring and their height (of the line
/// width), and points drawn per wave.
const WAVES: f32 = 14.0;
const WAVE_AMP: f32 = 0.45;
const WAVE_POINTS: f32 = 16.0;
const SECONDS_PER_MINUTE: i64 = 60;
const MINUTES_PER_HOUR: i64 = 60;
const HOURS_PER_DAY: i64 = 24;
const DAYS_PER_WEEK: i64 = 7;
const PERCENT_SCALE: f32 = 100.0;
/// Dots per row of the grid: minutes of an hour, hours of a day, and a
/// week's days for the rest (a year runs in week columns).
const MINUTE_COLUMNS: usize = 10;
const HOUR_COLUMNS: usize = 6;

/// When the current `period` started and ends.
pub fn span(period: Period, now: &DateTime<Local>, week_start: u32) -> (DateTime<Local>, DateTime<Local>) {
    let local = |d: NaiveDate| Local.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap_or_default()).earliest().unwrap_or(*now);
    let today = now.date_naive();
    match period {
        Period::Hour => {
            let start = now.with_minute(0).and_then(|t| t.with_second(0)).and_then(|t| t.with_nanosecond(0)).unwrap_or(*now);
            (start, start + Duration::hours(1))
        }
        Period::Day => (local(today), local(today + Duration::days(1))),
        Period::Week => {
            let back = (today.weekday().num_days_from_monday() + 7 - week_start % 7) % 7;
            let first = today - Duration::days(i64::from(back));
            (local(first), local(first + Duration::days(DAYS_PER_WEEK)))
        }
        Period::Month => {
            let first = today.with_day(1).unwrap_or(today);
            let next = if first.month() == 12 { NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1) };
            (local(first), local(next.unwrap_or(today)))
        }
        Period::Year => (local(NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today)), local(NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap_or(today))),
    }
}

/// "of today" and friends (English catalog keys).
fn period_label(p: Period) -> &'static str {
    match p {
        Period::Hour => tk("of this hour"),
        Period::Day => tk("of today"),
        Period::Week => tk("of this week"),
        Period::Month => tk("of this month"),
        Period::Year => tk("of this year"),
    }
}

/// `9 h 2 min left` for `secs` left, in the widget's digits.
pub fn left_text(secs: i64, l: &Lingo) -> String {
    let mins = (secs / SECONDS_PER_MINUTE).max(0);
    let (d, h, m) = (mins / (MINUTES_PER_HOUR * HOURS_PER_DAY), mins / MINUTES_PER_HOUR % HOURS_PER_DAY, mins % MINUTES_PER_HOUR);
    let text = if d > 0 {
        l.word(tk("{days} d {hours} h left"))
    } else if h > 0 {
        l.word(tk("{hours} h {minutes} min left"))
    } else {
        l.word(tk("{minutes} min left"))
    };
    l.digits(text.replace("{days}", &d.to_string()).replace("{hours}", &h.to_string()).replace("{minutes}", &m.to_string()))
}

/// `62 %` in the widget's digits.
pub fn percent(f: f32, l: &Lingo) -> String {
    let n = (f * PERCENT_SCALE).floor() as i64;
    let sign = if l.eastern { "٪" } else { "%" };
    l.digits(format!("{n}{sign}"))
}

/// The progress widget at `now`.
pub fn progress(w: &ProgressWidget, now: &DateTime<Local>, l: &Lingo, week_start: u32, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let px = |v: f32| v * k;
    let role = w.look.font;
    let (start, end) = span(w.period, now, week_start);
    let total = (end - start).num_seconds().max(1) as f32;
    let gone = (*now - start).num_seconds().max(0) as f32;
    let f = (gone / total).clamp(0.0, 1.0);
    let pct = percent(f, l);
    let label = format!("{} {}{}{}", pct, l.word(period_label(w.period)), face::SEPARATOR, left_text((end - *now).num_seconds(), l));
    let thick = px(w.thickness());
    let mut marks = Vec::new();
    match w.style {
        ProgressStyle::Ring | ProgressStyle::WavyRing => {
            let r = px(RING_R);
            let c = (r + thick, r + thick);
            if w.style == ProgressStyle::Ring {
                marks.push(Mark::new(MarkKind::Arc { c, r, width: thick, start: -FRAC_PI_2, sweep: TAU }, InkRole::Fg).dim(TRACK_ALPHA));
                if f > 0.0 {
                    marks.push(Mark::new(MarkKind::Arc { c, r, width: thick, start: -FRAC_PI_2, sweep: TAU * f }, InkRole::Accent));
                }
            } else {
                // Track as a plain ring, progress as a wave riding it.
                marks.push(Mark::new(MarkKind::Arc { c, r, width: thick / 2.0, start: -FRAC_PI_2, sweep: TAU }, InkRole::Fg).dim(TRACK_ALPHA));
                let n = ((WAVES * WAVE_POINTS * f).ceil() as usize).max(2);
                let pts = (0..=n)
                    .map(|i| {
                        let t = f * i as f32 / n as f32;
                        let a = -FRAC_PI_2 + TAU * t;
                        let rr = r + thick * WAVE_AMP * (WAVES * TAU * t).sin();
                        (c.0 + rr * a.cos(), c.1 + rr * a.sin())
                    })
                    .collect();
                if f > 0.0 {
                    marks.push(Mark::new(MarkKind::Polyline { pts, width: thick * (1.0 - WAVE_AMP) }, InkRole::Accent));
                }
                // The plane covers the whole ring.
                marks.push(Mark::new(MarkKind::Arc { c, r: r + thick * WAVE_AMP, width: thick, start: 0.0, sweep: TAU }, InkRole::Fg).dim(0.0));
            }
            let ps = style(role, px(PERCENT), face::SEMIBOLD);
            let (pw, ph) = m.measure(&pct, &ps);
            marks.push(Mark::new(MarkKind::Text { text: pct.clone(), style: ps, x: c.0 - pw / 2.0, y: c.1 - ph / 2.0, w: pw, h: ph }, InkRole::Fg));
            if w.label {
                let ls = style(role, px(LABEL), face::MEDIUM);
                let text = format!("{}{}{}", l.word(period_label(w.period)), face::SEPARATOR, left_text((end - *now).num_seconds(), l));
                let (lw, lh) = m.measure(&text, &ls);
                marks.push(Mark::new(MarkKind::Text { text, style: ls, x: c.0 - lw / 2.0, y: c.1 + r + thick + px(LABEL_GAP), w: lw, h: lh }, InkRole::Fg).dim(face::DIM));
            }
        }
        ProgressStyle::Bar => {
            let bw = px(BAR_W);
            let mut lines = Vec::new();
            if w.label {
                lines.push(line(vec![run(label.clone(), tracked(style(role, px(LABEL), face::MEDIUM), 0.02), InkRole::Fg)], 0.0, 0.0, l.rtl, m));
            }
            let mut bar = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: bw, h: thick, radius: thick / 2.0, width: 0.0 }, InkRole::Fg).dim(TRACK_ALPHA)];
            if f > 0.0 {
                let fw = (bw * f).max(thick);
                let x = if l.rtl { bw - fw } else { 0.0 };
                bar.push(Mark::new(MarkKind::Rect { x, y: 0.0, w: fw, h: thick, radius: thick / 2.0, width: 0.0 }, InkRole::Accent));
            }
            lines.push((bar, (bw, thick)));
            marks.extend(stack(lines, px(LABEL_GAP), align));
        }
        ProgressStyle::Dots => {
            let (count, done, cols) = dots(w.period, now, &start);
            let (d, g) = (px(DOT), px(DOT_GAP));
            let rows = count.div_ceil(cols);
            // A year runs in columns of weeks, the rest in rows.
            let year = w.period == Period::Year;
            let mut grid = Vec::new();
            for i in 0..count {
                let (col, row) = if year { (i / cols, i % cols) } else { (i % cols, i / cols) };
                let col = if l.rtl && !year { cols - 1 - col } else { col };
                let c = (col as f32 * (d + g) + d / 2.0, row as f32 * (d + g) + d / 2.0);
                if i == done {
                    grid.push(Mark::new(MarkKind::Arc { c, r: d / 2.0, width: px(TODAY_RING_W), start: 0.0, sweep: TAU }, InkRole::Accent));
                } else {
                    grid.push(Mark::new(MarkKind::Disc { c, r: d / 2.0 }, if i < done { InkRole::Accent } else { InkRole::Fg }).dim(if i < done { 1.0 } else { TRACK_ALPHA }));
                }
            }
            let (gw, gh) = if year { (rows as f32 * (d + g) - g, cols as f32 * (d + g) - g) } else { (cols as f32 * (d + g) - g, rows as f32 * (d + g) - g) };
            let mut lines = vec![(grid, (gw, gh))];
            if w.label {
                lines.push(line(vec![Run { opacity: face::DIM, ..run(label, style(role, px(LABEL), face::MEDIUM), InkRole::Fg) }], 0.0, 0.0, l.rtl, m));
            }
            marks.extend(stack(lines, px(LABEL_GAP), align));
        }
    }
    Composition::of(marks)
}

/// Dots of a period: how many, how many have passed, and per row (per
/// column for a year).
fn dots(period: Period, now: &DateTime<Local>, start: &DateTime<Local>) -> (usize, usize, usize) {
    match period {
        Period::Hour => (MINUTES_PER_HOUR as usize, now.minute() as usize, MINUTE_COLUMNS),
        Period::Day => (HOURS_PER_DAY as usize, now.hour() as usize, HOUR_COLUMNS),
        Period::Week => (DAYS_PER_WEEK as usize, (*now - *start).num_days() as usize, DAYS_PER_WEEK as usize),
        Period::Month => {
            let (y, mo) = (now.year(), now.month());
            let next = if mo == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, mo + 1, 1) };
            let days = next.and_then(|n| n.pred_opt()).map_or(30, |d| d.day()) as usize;
            (days, now.day0() as usize, DAYS_PER_WEEK as usize)
        }
        Period::Year => {
            let (day, days) = super::sky::day_of_year(now.date_naive());
            (days as usize, day as usize - 1, DAYS_PER_WEEK as usize)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;

    fn lingo() -> Lingo {
        Lingo::of(&Default::default(), false, None)
    }

    #[test]
    fn spans_and_texts() {
        let now = Local.with_ymd_and_hms(2026, 3, 4, 15, 0, 0).unwrap();
        let (s, e) = span(Period::Day, &now, 0);
        assert_eq!((e - s).num_hours(), 24);
        let (s, _) = span(Period::Week, &now, 0);
        assert_eq!(s.date_naive(), NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(), "Monday");
        let (s, _) = span(Period::Week, &now, 6);
        assert_eq!(s.date_naive(), NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(), "Sunday");
        assert_eq!(left_text(9 * 3600 + 2 * 60 + 30, &lingo()), "9 h 2 min left");
        assert_eq!(left_text(3 * 86_400 + 3600, &lingo()), "3 d 1 h left");
        assert_eq!(percent(0.625, &lingo()), "62%");
    }

    #[test]
    fn every_style_and_period_lays_out() {
        let now = Local.with_ymd_and_hms(2026, 3, 4, 15, 30, 0).unwrap();
        for style in [ProgressStyle::Ring, ProgressStyle::WavyRing, ProgressStyle::Bar, ProgressStyle::Dots] {
            for period in [Period::Hour, Period::Day, Period::Week, Period::Month, Period::Year] {
                let w = ProgressWidget { style, period, ..Default::default() };
                let c = progress(&w, &now, &lingo(), 0, 1.0, 0.0, &mut Fixed);
                assert!(c.size.0 > 20.0 && c.size.1 > 10.0, "{style:?} {period:?}");
            }
        }
        let (count, done, _) = dots(Period::Year, &now, &now);
        assert_eq!((count, done), (365, 62));
    }
}
