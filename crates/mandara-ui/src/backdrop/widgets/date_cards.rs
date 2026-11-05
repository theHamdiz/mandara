//! Date tickets, perpetual cards and a month of calculated moon phases.

use chrono::{Datelike as _, NaiveDate};
use mandara_config::widget_kinds::{DateLayout, DateWidget};

use super::layout::{InkRole, Mark, MarkKind, Measure, face, style};
use super::lingo::{HijriDate, Lingo};

/// Reference geometry at 1080 px. All lengths scale with the placement.
const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 190.0;
const PAD: f32 = 20.0;
const DAY: f32 = 102.0;
const LABEL: f32 = 19.0;
const SMALL: f32 = 14.0;
const RULE: f32 = 1.0;
const GAP: f32 = 12.0;
const PERF_PITCH: f32 = 12.0;
const PERF_LENGTH: f32 = 5.0;
const CELL: f32 = 54.0;
const MOON: f32 = 14.0;
const COLUMNS: usize = 7;
const CARD_RADIUS: f32 = 12.0;
const MUTED: f32 = 0.4;

fn label(out: &mut Vec<Mark>, value: String, w: &DateWidget, size: f32, center: f32, y: f32, ink: InkRole, m: &mut dyn Measure) -> f32 {
    let font = style(w.look.font, size, face::MEDIUM);
    let (tw, th) = m.measure(&value, &font);
    out.push(Mark::new(MarkKind::Text { text: value, style: font, x: center - tw / 2.0, y, w: tw, h: th }, ink));
    th
}

/// Days are derived from the civil month, including leap years.
fn month_days(today: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    (1..=31).filter_map(move |day| today.with_day(day))
}

#[allow(clippy::too_many_arguments)]
/// Build date-card geometry from the supplied date, calendar and language.
pub fn layout(w: &DateWidget, today: NaiveDate, l: &Lingo, hijri: Option<HijriDate>, week_start: u32, k: f32, m: &mut dyn Measure) -> Vec<Mark> {
    let mut out = Vec::new();
    if w.layout == DateLayout::MoonCalendar {
        let width = CELL * COLUMNS as f32 * k;
        let head_h = label(&mut out, l.date(&today, "%B %Y"), w, LABEL * k, width / 2.0, 0.0, InkRole::Fg, m);
        let first = today.with_day(1).unwrap_or(today);
        let offset = (first.weekday().num_days_from_monday() + 7 - week_start % 7) % 7;
        for (i, date) in month_days(today).enumerate() {
            let slot = offset as usize + i;
            let col = slot % COLUMNS;
            let x = (if l.rtl { COLUMNS - 1 - col } else { col }) as f32 * CELL * k + CELL * k / 2.0;
            let y = head_h + GAP * k + (slot / COLUMNS) as f32 * (CELL + GAP) * k;
            let center = (x, y + MOON * k);
            if date == today {
                out.push(Mark::new(MarkKind::Disc { c: center, r: (MOON + GAP / 2.0) * k }, InkRole::Accent).dim(MUTED));
            }
            let noon = date.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp();
            out.extend(super::faces::moon(center, MOON * k, super::sky::moon_phase(noon), InkRole::Fg));
            label(&mut out, l.digits(date.day().to_string()), w, SMALL * k, x, y + (MOON * 2.0 + GAP / 2.0) * k, if date == today { InkRole::Accent } else { InkRole::Fg }, m);
        }
        return out;
    }
    let hijri_only = l.calendar == mandara_config::desktop_widgets::Calendar::Hijri;
    let (day, month, year) = match hijri.filter(|_| hijri_only) {
        Some((year, month, day)) => (l.hijri_digits(day), l.hijri_month(month), l.hijri_digits(year)),
        None => (l.digits(today.day().to_string()), l.date(&today, "%b"), l.digits(today.year().to_string())),
    };
    if w.layout == DateLayout::Ticket {
        let split = WIDTH * 0.65 * k;
        let steps = (HEIGHT / PERF_PITCH) as usize;
        for n in 0..steps {
            let y = (n as f32 * PERF_PITCH + PAD / 2.0) * k;
            out.push(Mark::new(MarkKind::Line { a: (split, y), b: (split, y + PERF_LENGTH * k), width: RULE * k }, InkRole::Fg).dim(MUTED).flat());
        }
        let main = split / 2.0;
        label(&mut out, l.date(&today, "%A"), w, LABEL * k, main, PAD * k, InkRole::Accent, m);
        label(&mut out, day.clone(), w, DAY * k, main, (PAD + LABEL + GAP) * k, InkRole::Fg, m);
        let stub = split + (WIDTH * k - split) / 2.0;
        let month_width = m.measure(&month, &style(w.look.font, LABEL * k, face::MEDIUM)).0;
        let month_size = LABEL * k * ((WIDTH * k - split - PAD * k) / month_width.max(1.0)).min(1.0);
        let mh = label(&mut out, month.clone(), w, month_size, stub, PAD * k, InkRole::Accent, m);
        label(&mut out, year, w, LABEL * k, stub, PAD * k + mh + GAP * k, InkRole::Fg, m);
        label(&mut out, l.date(&today, "%j"), w, SMALL * k, stub, (HEIGHT - PAD - SMALL) * k, InkRole::Fg, m);
    } else {
        let gap = GAP * k;
        let side = (WIDTH * k - gap * 2.0) / 3.0;
        for (i, value) in [month, day, l.date(&today, "%a")].into_iter().enumerate() {
            let x = i as f32 * (side + gap);
            out.push(Mark::new(MarkKind::Rect { x, y: 0.0, w: side, h: HEIGHT * k, radius: CARD_RADIUS * k, width: 0.0 }, InkRole::Plate).flat());
            let base_size = if i == 1 { DAY * k } else { LABEL * k };
            let measured = m.measure(&value, &style(w.look.font, base_size, face::MEDIUM)).0;
            let font_size = base_size * ((side - PAD * k) / measured.max(1.0)).min(1.0);
            let font = style(w.look.font, font_size, face::MEDIUM);
            let (_, h) = m.measure(&value, &font);
            label(&mut out, value, w, font_size, x + side / 2.0, (HEIGHT * k - h) / 2.0, if i == 1 { InkRole::Accent } else { InkRole::OnPlate }, m);
            out.push(Mark::new(MarkKind::Line { a: (x + PAD * k, PAD * k), b: (x + side - PAD * k, PAD * k), width: RULE * k }, InkRole::Fg).dim(MUTED).flat());
        }
    }
    if !hijri_only && l.calendar.hijri() && let Some(h) = hijri {
            let value = l.hijri_full(h);
            let measured = m.measure(&value, &style(w.look.font, SMALL * k, face::MEDIUM)).0;
            let size = SMALL * k * (WIDTH * k / measured.max(1.0)).min(1.0);
            label(&mut out, value, w, size, WIDTH * k / 2.0, (HEIGHT + GAP) * k, InkRole::Fg, m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layout::tests::Fixed;

    #[test]
    fn moon_calendar_has_the_real_number_of_dates() {
        let l = Lingo::of(&Default::default(), false, None);
        for (year, month, count) in [(2024, 2, 29), (2025, 2, 28), (2026, 4, 30), (2026, 12, 31)] {
            let today = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
            assert_eq!(month_days(today).count(), count);
            let marks = layout(&DateWidget { layout: DateLayout::MoonCalendar, ..Default::default() }, today, &l, None, 0, 1.0, &mut Fixed);
            assert_eq!(marks.iter().filter(|m| matches!(m.kind, MarkKind::Text { .. })).count(), count + 1);
            assert!(marks.iter().all(|m| { let b = m.bounds(); [b.0, b.1, b.2, b.3].into_iter().all(f32::is_finite) }));
        }
    }

    #[test]
    fn perpetual_cards_keep_both_digits_of_the_day() {
        let l = Lingo::of(&mandara_config::desktop_widgets::TextOptions { language: "en".into(), ..Default::default() }, false, None);
        let marks = layout(&DateWidget { layout: DateLayout::Perpetual, ..Default::default() }, NaiveDate::from_ymd_opt(2026, 11, 29).unwrap(), &l, None, 0, 1.0, &mut Fixed);
        assert!(marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == "29")));
    }
    #[test]
    fn date_cards_follow_the_selected_calendar() {
        use mandara_config::desktop_widgets::{Calendar, TextOptions};
        let today = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
        let hijri = super::super::lingo::hijri_of(today);
        for layout_kind in [DateLayout::Ticket, DateLayout::Perpetual] {
            let w = DateWidget { layout: layout_kind, ..Default::default() };
            for calendar in [Calendar::Hijri, Calendar::Both] {
                let l = Lingo::of(&TextOptions { calendar, language: "en".into(), hijri_in_arabic: false, ..Default::default() }, false, None);
                let marks = layout(&w, today, &l, Some(hijri), 0, 1.0, &mut Fixed);
                let expected = if calendar == Calendar::Hijri { l.hijri_month(hijri.1) } else { l.hijri_full(hijri) };
                assert!(marks.iter().any(|m| matches!(&m.kind, MarkKind::Text { text, .. } if text == &expected)));
            }
        }
    }

}
