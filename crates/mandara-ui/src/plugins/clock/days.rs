//! Date arithmetic behind the clock panel's calendar: the six-week grid,
//! month steps, relative day names, Hijri dates and the targets
//! `calendar goto` accepts. No GUI types, so all of it is unit-tested.

use chrono::{Datelike, Months, NaiveDate, TimeDelta};

/// Weeks on the grid: every month fits in six, and a fixed count keeps the
/// panel one height while you page through months.
pub const GRID_WEEKS: usize = 6;
/// Days in a week (grid columns).
pub const WEEK_DAYS: usize = 7;
/// Cells on the grid.
pub const GRID_DAYS: usize = GRID_WEEKS * WEEK_DAYS;
/// Months in a year (`Shift+PageUp` / `PageDown` jump).
pub const YEAR_MONTHS: i32 = 12;

/// The first day of `d`'s month.
pub fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

/// `d` moved by `months`, keeping the day where the month has it (31 March
/// minus a month is 28 or 29 February).
pub fn add_months(d: NaiveDate, months: i32) -> NaiveDate {
    let step = Months::new(months.unsigned_abs());
    let moved = if months >= 0 { d.checked_add_months(step) } else { d.checked_sub_months(step) };
    moved.unwrap_or(d)
}

/// Whole months from `a`'s month to `b`'s (negative when `b` is earlier).
pub fn months_between(a: NaiveDate, b: NaiveDate) -> i32 {
    (b.year() - a.year()) * YEAR_MONTHS + b.month() as i32 - a.month() as i32
}

/// The first cell of `month`'s grid: the week start (`first_weekday` days
/// from Monday) on or before the 1st.
pub fn grid_start(month: NaiveDate, first_weekday: u32) -> NaiveDate {
    let first = month_start(month);
    let week = WEEK_DAYS as u32;
    let back = (first.weekday().num_days_from_monday() + week - first_weekday % week) % week;
    first - TimeDelta::days(i64::from(back))
}

/// The 42 dates on `month`'s grid, the neighbouring months' days included.
pub fn grid_days(month: NaiveDate, first_weekday: u32) -> [NaiveDate; GRID_DAYS] {
    let start = grid_start(month, first_weekday);
    std::array::from_fn(|i| start + TimeDelta::days(i as i64))
}

/// Days from `today` to `day` (negative in the past).
pub fn days_from(today: NaiveDate, day: NaiveDate) -> i64 {
    day.signed_duration_since(today).num_days()
}

/// "Today", "Tomorrow", "In 3 days", "2 days ago" for `day` seen from
/// `today`.
pub fn relative_label(today: NaiveDate, day: NaiveDate) -> String {
    match days_from(today, day) {
        0 => mandara_i18n::t("Today").to_string(),
        1 => mandara_i18n::t("Tomorrow").to_string(),
        -1 => mandara_i18n::t("Yesterday").to_string(),
        n if n > 0 => mandara_i18n::tf("In {n} days", &[("n", &n)]),
        n => mandara_i18n::tf("{n} days ago", &[("n", &-n)]),
    }
}

/// The Hijri date of `day` with the user's moon-sighting `offset` (days),
/// as `(year, month 1..=12, day)`.
pub fn hijri_of(day: NaiveDate, offset: i64) -> (i64, usize, i64) {
    crate::plugins::prayer::hijri(day + TimeDelta::days(offset))
}

/// A Hijri month's name in the interface language: the Arabic name in
/// Arabic, the translated transliteration otherwise.
pub fn hijri_month_name(month: usize, arabic: bool) -> &'static str {
    let (latin, arabic_name) = crate::plugins::prayer::HIJRI_MONTHS[month.clamp(1, crate::plugins::prayer::HIJRI_MONTHS.len()) - 1];
    if arabic { arabic_name } else { mandara_i18n::t(latin) }
}

/// ASCII digits as Eastern Arabic numerals when `eastern`.
pub fn digits(s: &str, eastern: bool) -> String {
    if eastern { crate::widgets::clock::arabic_numerals(s) } else { s.to_string() }
}

/// What `calendar goto` was asked to show, from `selected` (the day on
/// screen) and `today`: `today`, `next` / `prev` (a month), `+N` / `-N`
/// (days), `YYYY-MM` (that month's 1st, or today in the current month) or
/// `YYYY-MM-DD`.
pub fn parse_target(arg: &str, selected: NaiveDate, today: NaiveDate) -> Option<NaiveDate> {
    let arg = arg.trim().to_ascii_lowercase();
    match arg.as_str() {
        "today" | "now" => return Some(today),
        "next" => return Some(add_months(selected, 1)),
        "prev" | "previous" => return Some(add_months(selected, -1)),
        _ => {}
    }
    if arg.starts_with(['+', '-']) {
        let days = arg.trim_start_matches('+').parse::<i64>().ok()?;
        return selected.checked_add_signed(TimeDelta::try_days(days)?);
    }
    if let Ok(day) = NaiveDate::parse_from_str(&arg, "%Y-%m-%d") {
        return Some(day);
    }
    let month = NaiveDate::parse_from_str(&format!("{arg}-01"), "%Y-%m-%d").ok()?;
    Some(if months_between(today, month) == 0 { today } else { month })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn grids_start_on_the_week_start_and_hold_six_weeks() {
        // 1 Nov 2026 is a Sunday.
        let monday = grid_days(d(2026, 11, 17), 0);
        assert_eq!(monday[0], d(2026, 10, 26), "Monday-first: the week before");
        assert_eq!(monday[6], d(2026, 11, 1));
        assert_eq!(monday[GRID_DAYS - 1], d(2026, 12, 6));
        assert_eq!(grid_start(d(2026, 11, 1), 6), d(2026, 11, 1), "Sunday-first: the 1st opens the grid");
        assert_eq!(grid_start(d(2026, 11, 1), 5), d(2026, 10, 31), "Saturday-first");
        // February 2027 starts on a Monday: it fills four rows, then two of March.
        let feb = grid_days(d(2027, 2, 1), 0);
        assert_eq!(feb[0], d(2027, 2, 1));
        assert!(feb.iter().filter(|x| x.month() == 2).count() == 28);
    }

    #[test]
    fn month_steps_keep_the_day_where_they_can() {
        assert_eq!(add_months(d(2026, 3, 31), -1), d(2026, 2, 28));
        assert_eq!(add_months(d(2028, 1, 31), 1), d(2028, 2, 29), "leap year");
        assert_eq!(add_months(d(2026, 12, 15), 1), d(2027, 1, 15));
        assert_eq!(add_months(d(2026, 5, 5), -YEAR_MONTHS), d(2025, 5, 5));
        assert_eq!(months_between(d(2026, 11, 30), d(2027, 1, 1)), 2);
        assert_eq!(months_between(d(2026, 11, 30), d(2026, 9, 1)), -2);
        assert_eq!(month_start(d(2026, 11, 30)), d(2026, 11, 1));
    }

    #[test]
    fn relative_days_read_naturally() {
        let today = d(2026, 11, 1);
        assert_eq!(relative_label(today, today), "Today");
        assert_eq!(relative_label(today, d(2026, 11, 2)), "Tomorrow");
        assert_eq!(relative_label(today, d(2026, 10, 31)), "Yesterday");
        assert_eq!(relative_label(today, d(2026, 11, 6)), "In 5 days");
        assert_eq!(relative_label(today, d(2026, 10, 29)), "3 days ago");
    }

    #[test]
    fn goto_targets() {
        let (sel, today) = (d(2026, 11, 20), d(2026, 11, 1));
        assert_eq!(parse_target("today", sel, today), Some(today));
        assert_eq!(parse_target("next", sel, today), Some(d(2026, 12, 20)));
        assert_eq!(parse_target("prev", sel, today), Some(d(2026, 10, 20)));
        assert_eq!(parse_target("+3", sel, today), Some(d(2026, 11, 23)));
        assert_eq!(parse_target("-21", sel, today), Some(d(2026, 10, 30)));
        assert_eq!(parse_target("2027-02-14", sel, today), Some(d(2027, 2, 14)));
        assert_eq!(parse_target("2027-02", sel, today), Some(d(2027, 2, 1)));
        assert_eq!(parse_target("2026-11", sel, today), Some(today), "this month opens on today");
        assert_eq!(parse_target("someday", sel, today), None);
        assert_eq!(parse_target("2026-13", sel, today), None);
    }

    #[test]
    fn hijri_dates_and_numerals() {
        // 1 Nov 2026 ≈ 21 Jumada al-Ula 1448 (tabular calendar).
        let (y, m, _) = hijri_of(d(2026, 11, 1), 0);
        assert_eq!((y, m), (1448, 5));
        let (_, _, day) = hijri_of(d(2026, 11, 1), 0);
        let (_, _, next) = hijri_of(d(2026, 11, 1), 1);
        assert_eq!((next - day).rem_euclid(30), 1, "the offset shifts by whole days");
        assert_eq!(hijri_month_name(5, true), "جمادى الأولى");
        assert_eq!(hijri_month_name(5, false), "Jumada al-Ula");
        assert_eq!(digits("1448", true), "١٤٤٨");
        assert_eq!(digits("1448", false), "1448");
    }
}
