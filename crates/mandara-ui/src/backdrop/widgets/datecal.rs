//! Painted calendars: the month grid with today marked by any shape, this
//! week on one row, what's coming up with date chips, and a big date tile.
//! Events come from Studio → Calendar's sources; a click opens the clock
//! panel's calendar. They redraw with the minute (a new day, an event
//! passing).

use chrono::{DateTime, Datelike as _, Local, NaiveDate, Weekday};
use mandara_config::desktop_widgets::{DateLayout, DateWidget};
use mandara_config::widget_kinds::{count, ranges};
use mandara_core::shape::Shape;
use mandara_i18n::tk;

use super::actions::Action;
use super::frame::shape_of;
use super::layout::{Composition, Hit, InkRole, Mark, MarkKind, Measure, caps, face, fit, style, tracked};
use super::lingo::{HijriDate, Lingo};
use super::EventItem;

/// Reference sizes.
const CELL: f32 = 44.0;
const DAY: f32 = 19.0;
const HEAD: f32 = 24.0;
const HEAD_GAP: f32 = 14.0;
const WEEKDAY: f32 = 13.0;
const WEEKNUM: f32 = 12.0;
const EVENT_DOT: f32 = 2.6;
const OTHER_MONTH_ALPHA: f32 = 0.32;
const WEEKEND_ALPHA: f32 = 0.7;
const WEEK_CELL: f32 = 58.0;
const WEEK_H: f32 = 78.0;
const AGENDA_W: f32 = 380.0;
const CHIP: f32 = 54.0;
const CHIP_GAP: f32 = 14.0;
const ROW_GAP: f32 = 12.0;
const TITLE: f32 = 19.0;
const WHEN: f32 = 14.0;
const TILE_DAY: f32 = 150.0;
const TILE_SMALL: f32 = 20.0;
const TILE_GAP: f32 = 4.0;
const TRACKING: f32 = 0.16;
const DAYS: usize = 7;
const ROWS: usize = 6;
/// Markers on small cells: their corner radius as a part of the cell.
const MARKER_RADIUS: f32 = 0.3;

/// The marker shape for a cell of `side`.
fn marker(w: &DateWidget, k: f32, side: f32) -> Shape {
    // Small marks keep their corners in proportion to themselves.
    match shape_of(w.marker, &w.look, k) {
        Shape::Rect { .. } => Shape::Rect { radius: side * MARKER_RADIUS },
        Shape::Polygon { sides, .. } => Shape::Polygon { sides, radius: side * MARKER_RADIUS / 3.0 },
        Shape::Skew { degrees, .. } => Shape::Skew { degrees, radius: side * MARKER_RADIUS / 3.0 },
        s => s,
    }
}

/// The first day of the week holding `d`, `week_start` days after Monday.
fn week_first(d: NaiveDate, week_start: u32) -> NaiveDate {
    let back = (d.weekday().num_days_from_monday() + 7 - week_start % 7) % 7;
    d - chrono::Duration::days(i64::from(back))
}

fn weekend(d: NaiveDate) -> bool {
    matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

/// A painted calendar at `now`.
#[allow(clippy::too_many_arguments)]
pub fn date_widget(w: &DateWidget, events: &[EventItem], now: &DateTime<Local>, l: &Lingo, hijri: Option<HijriDate>, week_start: u32, h24: bool, k: f32, align: f32, m: &mut dyn Measure) -> Composition {
    let today = now.date_naive();
    let marks = match w.layout {
        DateLayout::Month => month(w, events, today, l, hijri, week_start, k, m),
        DateLayout::Week => week(w, events, today, l, week_start, k, m),
        DateLayout::Agenda => agenda(w, events, now, l, h24, k, m),
        DateLayout::Tile => tile(w, today, l, hijri, k, align, m),
        DateLayout::Ticket | DateLayout::Perpetual | DateLayout::MoonCalendar => super::date_cards::layout(w, today, l, hijri, week_start, k, m),
    };
    let mut c = Composition::of(marks);
    c.hits.push(Hit { rect: (0.0, 0.0, c.size.0, c.size.1), action: Action::Panel(crate::plugins::clock::ID.into()) });
    c
}

/// Whether any event touches `d`.
fn busy(events: &[EventItem], d: NaiveDate) -> bool {
    events.iter().any(|e| e.day <= d && d <= e.last_day)
}

/// The month header: `November 2026` (and the Hijri month when asked).
fn header(l: &Lingo, today: NaiveDate, hijri: Option<HijriDate>) -> String {
    let greg = l.date(&today, tk("%B %Y"));
    match hijri.filter(|_| l.calendar.hijri()) {
        Some(h) if l.calendar.gregorian() => format!("{greg}{}{} {}", face::SEPARATOR, l.hijri_month(h.1), l.hijri_digits(h.0)),
        Some(h) => format!("{} {}", l.hijri_month(h.1), l.hijri_digits(h.0)),
        None => greg,
    }
}

#[allow(clippy::too_many_arguments)]
fn month(w: &DateWidget, events: &[EventItem], today: NaiveDate, l: &Lingo, hijri: Option<HijriDate>, week_start: u32, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let cell = px(CELL);
    let num_w = if w.week_numbers { cell * 0.8 } else { 0.0 };
    let width = num_w + cell * DAYS as f32;
    let mut marks = Vec::new();
    let hs = style(w.look.font, px(HEAD), face::SEMIBOLD);
    let head = fit(&header(l, today, hijri), &hs, width, m);
    let (hw, hh) = m.measure(&head, &hs);
    marks.push(Mark::new(MarkKind::Text { text: head, style: hs, x: if l.rtl { width - hw } else { 0.0 }, y: 0.0, w: hw, h: hh }, InkRole::Fg));
    let first = today.with_day(1).unwrap_or(today);
    let start = week_first(first, week_start);
    let ws = tracked(style(w.look.font, px(WEEKDAY), face::BOLD), TRACKING / 2.0);
    let y0 = hh + px(HEAD_GAP);
    let col_x = |i: usize| {
        let i = if l.rtl { DAYS - 1 - i } else { i };
        (if l.rtl { 0.0 } else { num_w }) + i as f32 * cell
    };
    for i in 0..DAYS {
        let d = start + chrono::Duration::days(i as i64);
        let name = caps(&l.date(&d, "%a"));
        let name: String = name.chars().take(if l.arabic() { 1 } else { 2 }).collect();
        let (nw, nh) = m.measure(&name, &ws);
        marks.push(Mark::new(MarkKind::Text { text: name, style: ws.clone(), x: col_x(i) + (cell - nw) / 2.0, y: y0, w: nw, h: nh }, InkRole::Accent).dim(if weekend(d) { WEEKEND_ALPHA } else { 1.0 }));
    }
    let grid_y = y0 + m.measure("M", &ws).1 + px(HEAD_GAP) / 2.0;
    let ds = style(w.look.font, px(DAY), face::MEDIUM);
    let today_s = style(w.look.font, px(DAY), face::BOLD);
    let ns = style(w.look.font, px(WEEKNUM), face::MEDIUM);
    let side = cell * 0.82;
    for row in 0..ROWS {
        let row_start = start + chrono::Duration::days((row * DAYS) as i64);
        if row > 0 && row_start.month() != first.month() && row_start > first {
            break;
        }
        let y = grid_y + row as f32 * cell;
        if w.week_numbers {
            let n = l.digits(row_start.iso_week().week().to_string());
            let (nw, nh) = m.measure(&n, &ns);
            let x = if l.rtl { width - num_w + (num_w - nw) / 2.0 } else { (num_w - nw) / 2.0 };
            marks.push(Mark::new(MarkKind::Text { text: n, style: ns.clone(), x, y: y + (cell - nh) / 2.0, w: nw, h: nh }, InkRole::Fg).dim(OTHER_MONTH_ALPHA));
        }
        for i in 0..DAYS {
            let d = row_start + chrono::Duration::days(i as i64);
            let x = col_x(i);
            let c = (x + cell / 2.0, y + cell / 2.0);
            let is_today = d == today;
            if is_today {
                marks.push(Mark::new(MarkKind::Shaped { x: c.0 - side / 2.0, y: c.1 - side / 2.0, w: side, h: side, shape: marker(w, k, side), width: 0.0 }, InkRole::Accent));
            }
            let n = l.digits(d.day().to_string());
            let (nw, nh) = m.measure(&n, if is_today { &today_s } else { &ds });
            let alpha = if d.month() != first.month() { OTHER_MONTH_ALPHA } else if weekend(d) { WEEKEND_ALPHA } else { 1.0 };
            marks.push(Mark::new(MarkKind::Text { text: n, style: if is_today { today_s.clone() } else { ds.clone() }, x: c.0 - nw / 2.0, y: c.1 - nh / 2.0, w: nw, h: nh }, if is_today { InkRole::OnAccent } else { InkRole::Fg }).dim(if is_today { 1.0 } else { alpha }));
            if w.event_dots && busy(events, d) {
                marks.push(Mark::new(MarkKind::Disc { c: (c.0, c.1 + side / 2.0 - px(EVENT_DOT) * 1.6), r: px(EVENT_DOT) }, if is_today { InkRole::OnAccent } else { InkRole::Accent }));
            }
        }
    }
    marks
}

fn week(w: &DateWidget, events: &[EventItem], today: NaiveDate, l: &Lingo, week_start: u32, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let (cw, ch) = (px(WEEK_CELL), px(WEEK_H));
    let start = week_first(today, week_start);
    let ws = tracked(style(w.look.font, px(WEEKDAY), face::BOLD), TRACKING / 2.0);
    let ds = style(w.look.font, px(DAY) * 1.25, face::SEMIBOLD);
    let mut marks = Vec::new();
    for i in 0..DAYS {
        let d = start + chrono::Duration::days(i as i64);
        let col = if l.rtl { DAYS - 1 - i } else { i };
        let x = col as f32 * cw;
        let is_today = d == today;
        if is_today {
            let pill = match marker(w, k, cw) {
                Shape::Circle => Shape::Rect { radius: cw * 0.42 },
                s => s,
            };
            marks.push(Mark::new(MarkKind::Shaped { x: x + cw * 0.08, y: 0.0, w: cw * 0.84, h: ch, shape: pill, width: 0.0 }, InkRole::Accent));
        }
        let ink = if is_today { InkRole::OnAccent } else { InkRole::Fg };
        let name = caps(&l.date(&d, "%a"));
        let (nw, nh) = m.measure(&name, &ws);
        marks.push(Mark::new(MarkKind::Text { text: name, style: ws.clone(), x: x + (cw - nw) / 2.0, y: ch * 0.18, w: nw, h: nh }, ink).dim(if is_today { 1.0 } else { face::DIM }));
        let n = l.digits(d.day().to_string());
        let (dw, dh) = m.measure(&n, &ds);
        marks.push(Mark::new(MarkKind::Text { text: n, style: ds.clone(), x: x + (cw - dw) / 2.0, y: ch * 0.18 + nh, w: dw, h: dh }, ink));
        if w.event_dots && busy(events, d) {
            marks.push(Mark::new(MarkKind::Disc { c: (x + cw / 2.0, ch * 0.88), r: px(EVENT_DOT) }, if is_today { InkRole::OnAccent } else { InkRole::Accent }));
        }
    }
    marks
}

fn agenda(w: &DateWidget, events: &[EventItem], now: &DateTime<Local>, l: &Lingo, h24: bool, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let width = px(AGENDA_W);
    let chip = px(CHIP);
    let ts = style(w.look.font, px(TITLE), face::SEMIBOLD);
    let es = style(w.look.font, px(WHEN), face::MEDIUM);
    let ms = tracked(style(w.look.font, px(WHEN) * 0.85, face::BOLD), TRACKING);
    let ds = style(w.look.font, px(TITLE) * 1.2, face::BOLD);
    let mut marks = vec![Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w: width, h: 1.0, radius: 0.0, width: 0.0 }, InkRole::Fg).dim(0.0)];
    let upcoming: Vec<&EventItem> = events.iter().filter(|e| e.end_ms > now.timestamp_millis()).take(count(w.rows, ranges::AGENDA_ROWS) as usize).collect();
    if upcoming.is_empty() {
        let text = l.word(tk("Nothing coming up"));
        let (tw, th) = m.measure(&text, &ts);
        marks.push(Mark::new(MarkKind::Text { text, style: ts, x: if l.rtl { width - tw } else { 0.0 }, y: 0.0, w: tw, h: th }, InkRole::Fg).dim(face::DIM));
        return marks;
    }
    let shape = marker(w, k, chip);
    let mut y = 0.0;
    for e in upcoming {
        let (chip_x, text_x, text_w) = if l.rtl { (width - chip, 0.0, width - chip - px(CHIP_GAP)) } else { (0.0, chip + px(CHIP_GAP), width - chip - px(CHIP_GAP)) };
        let ink = e.color.map_or(InkRole::Accent, InkRole::Rgb);
        marks.push(Mark::new(MarkKind::Shaped { x: chip_x, y, w: chip, h: chip, shape, width: 0.0 }, ink).dim(face::DIM));
        let mon = caps(&l.date(&e.day, "%b"));
        let (mw, mh) = m.measure(&mon, &ms);
        let dn = l.digits(e.day.day().to_string());
        let (dw, dh) = m.measure(&dn, &ds);
        let top = y + (chip - mh - dh) / 2.0;
        marks.push(Mark::new(MarkKind::Text { text: mon, style: ms.clone(), x: chip_x + (chip - mw) / 2.0, y: top, w: mw, h: mh }, InkRole::OnAccent));
        marks.push(Mark::new(MarkKind::Text { text: dn, style: ds.clone(), x: chip_x + (chip - dw) / 2.0, y: top + mh, w: dw, h: dh }, InkRole::OnAccent));
        let title = fit(&e.title, &ts, text_w, m);
        let (tw, th) = m.measure(&title, &ts);
        let when = if e.all_day {
            l.word(tk("All day"))
        } else {
            let start = DateTime::from_timestamp_millis(e.start_ms).map(|t| t.with_timezone(&Local)).unwrap_or(*now);
            l.digits(if h24 { start.format("%H:%M").to_string() } else { start.format("%-I:%M %p").to_string() })
        };
        let (ww, wh) = m.measure(&when, &es);
        let tx = |w0: f32| if l.rtl { text_x + text_w - w0 } else { text_x };
        let ty = y + (chip - th - wh) / 2.0;
        marks.push(Mark::new(MarkKind::Text { text: title, style: ts.clone(), x: tx(tw), y: ty, w: tw, h: th }, InkRole::Fg));
        marks.push(Mark::new(MarkKind::Text { text: when, style: es.clone(), x: tx(ww), y: ty + th, w: ww, h: wh }, InkRole::Fg).dim(face::DIM));
        y += chip + px(ROW_GAP);
    }
    marks
}

fn tile(w: &DateWidget, today: NaiveDate, l: &Lingo, hijri: Option<HijriDate>, k: f32, align: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let px = |v: f32| v * k;
    let ss = tracked(style(w.look.font, px(TILE_SMALL), face::BOLD), TRACKING);
    let ds = style(w.look.font, px(TILE_DAY), face::BLACK);
    let hijri_only = l.calendar.hijri() && !l.calendar.gregorian();
    let (day, month) = match hijri.filter(|_| hijri_only) {
        Some(h) => (l.hijri_digits(h.2), l.hijri_month(h.1)),
        None => (l.digits(today.day().to_string()), l.date(&today, "%B")),
    };
    let lines = [(caps(&l.date(&today, "%A")), &ss, InkRole::Accent, 1.0, false), (day, &ds, InkRole::Fg, 1.0, true), (caps(&month), &ss, InkRole::Fg, face::DIM, false)];
    let sizes: Vec<(f32, f32)> = lines.iter().map(|(t, s, ..)| m.measure(t, s)).collect();
    let width = sizes.iter().map(|s| s.0).fold(0.0, f32::max);
    let mut y = 0.0;
    let mut marks = Vec::new();
    for ((text, st, ink, alpha, big), (tw, th)) in lines.into_iter().zip(sizes) {
        // Big numerals carry a tall line box: tighten it.
        let h = if big { th * TIGHT } else { th };
        marks.push(Mark::new(MarkKind::Text { text, style: st.clone(), x: (width - tw) * align, y: y - (th - h) / 2.0, w: tw, h: th }, ink).dim(alpha));
        y += h + px(TILE_GAP);
    }
    marks
}

/// How much of a big numeral's line box the tile keeps.
const TIGHT: f32 = 0.84;

#[cfg(test)]
mod tests {
    use super::super::layout::tests::Fixed;
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn every_layout_lays_out() {
        let now = Local.with_ymd_and_hms(2026, 11, 1, 10, 0, 0).unwrap();
        let day = now.date_naive();
        let events = vec![EventItem { day, last_day: day, start_ms: now.timestamp_millis() + 3_600_000, end_ms: now.timestamp_millis() + 7_200_000, all_day: false, title: "Dentist".into(), color: None }];
        let l = Lingo::of(&Default::default(), false, None);
        for layout in [DateLayout::Month, DateLayout::Week, DateLayout::Agenda, DateLayout::Tile] {
            let w = DateWidget { layout, week_numbers: true, ..Default::default() };
            let c = date_widget(&w, &events, &now, &l, None, 0, true, 1.0, 0.0, &mut Fixed);
            assert!(c.size.0 > 50.0 && c.size.1 > 50.0, "{layout:?} {:?}", c.size);
            assert_eq!(c.hits.len(), 1);
        }
        assert!(busy(&events, day) && !busy(&events, day.succ_opt().unwrap()));
        assert_eq!(week_first(day, 6), day, "1 Nov 2026 is a Sunday");
    }
}
