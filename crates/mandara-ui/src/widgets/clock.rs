//! Bilingual clock and date formatting.

use chrono::{Datelike, Timelike};

const WEEKDAYS_AR: [&str; 7] = ["الاثنين", "الثلاثاء", "الأربعاء", "الخميس", "الجمعة", "السبت", "الأحد"];
const ARABIC_DIGITS: [char; 10] = ['٠', '١', '٢', '٣', '٤', '٥', '٦', '٧', '٨', '٩'];

pub fn hh_mm<T: Timelike>(t: &T) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

pub fn seconds<T: Timelike>(t: &T) -> String {
    format!("{:02}", t.second())
}

/// `datetime.time_locale` when it names a locale (`en_GB`, `ar_EG`…);
/// `None` follows the interface language.
static LOCALE_OVERRIDE: std::sync::RwLock<Option<chrono::Locale>> = std::sync::RwLock::new(None);

/// Sets the date-name locale override (`auto` / empty / unknown → none).
pub fn set_locale_override(name: &str) {
    let name = name.trim();
    let parsed = if name.is_empty() || name.eq_ignore_ascii_case("auto") { None } else { chrono::Locale::try_from(name).ok() };
    if let Ok(mut l) = LOCALE_OVERRIDE.write() {
        *l = parsed;
    }
}

/// No date language was chosen: names follow the interface language.
pub fn locale_is_default() -> bool {
    LOCALE_OVERRIDE.read().ok().is_none_or(|l| l.is_none())
}

/// The date locale (month/weekday names): the override, else the current
/// UI language's.
pub fn locale() -> chrono::Locale {
    if let Some(l) = LOCALE_OVERRIDE.read().ok().and_then(|l| *l) {
        return l;
    }
    chrono::Locale::try_from(mandara_i18n::language().posix()).unwrap_or(chrono::Locale::en_US)
}

/// Whether `pattern` is a valid strftime pattern (rendering an invalid one
/// would fail).
pub fn valid_pattern(pattern: &str) -> bool {
    !pattern.is_empty() && chrono::format::StrftimeItems::new(pattern).all(|i| !matches!(i, chrono::format::Item::Error))
}

/// Whether `pattern` shows seconds (so a clock using it ticks every second).
pub fn shows_seconds(pattern: &str) -> bool {
    ["%S", "%T", "%X", "%r", "%s", "%f", "%3f", "%6f", "%9f", "%.f"].iter().any(|t| pattern.contains(t))
}

/// The local wall-clock second (or minute) as a number: caches of clock
/// text key on it. Unlike the UTC timestamp it changes with the time zone,
/// so a new zone redraws the clock at once instead of keeping the old
/// local time until the next minute.
pub fn local_stamp<Tz: chrono::TimeZone>(now: &chrono::DateTime<Tz>, seconds: bool) -> i64 {
    const SECS_PER_MIN: i64 = 60;
    let local = now.naive_local().and_utc().timestamp();
    if seconds { local } else { local.div_euclid(SECS_PER_MIN) }
}

/// Formats `t` with a user pattern in the date locale, falling back to
/// `fallback` when the pattern is invalid.
pub fn format_with<Tz: chrono::TimeZone>(t: &chrono::DateTime<Tz>, pattern: &str, fallback: &str) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let pattern = if valid_pattern(pattern) { pattern } else { fallback };
    t.format_localized(pattern, locale()).to_string()
}

/// Formats `d` with a strftime pattern using the current language's names.
/// The pattern itself is translatable so word order can follow the language.
pub fn localized<T: Datelike>(d: &T, pattern: &str) -> String {
    let date = chrono::NaiveDate::from_ymd_opt(d.year(), d.month(), d.day()).unwrap_or_default();
    date.format_localized(pattern, locale()).to_string()
}

/// e.g. `Tue 27 Oct`.
pub fn short_date<T: Datelike>(d: &T) -> String {
    localized(d, mandara_i18n::t("%a %-d %b"))
}

/// e.g. `Tuesday, 27 October 2026`.
pub fn long_date<T: Datelike>(d: &T) -> String {
    localized(d, mandara_i18n::t("%A, %-d %B %Y"))
}

/// Arabic weekday name.
pub fn weekday_ar<T: Datelike>(d: &T) -> &'static str {
    WEEKDAYS_AR[d.weekday().num_days_from_monday() as usize]
}

/// Converts ASCII digits to Eastern Arabic numerals.
pub fn arabic_numerals(s: &str) -> String {
    s.chars()
        .map(|c| c.to_digit(10).map(|d| ARABIC_DIGITS[d as usize]).unwrap_or(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Riyadh's clock and Cairo's show different times for the same
    /// instant: a time-zone change must change the cache key.
    #[test]
    fn clock_caches_key_on_local_time() {
        use chrono::{FixedOffset, TimeZone as _};
        let instant = chrono::Utc.with_ymd_and_hms(2026, 10, 30, 14, 29, 30).unwrap();
        let riyadh = instant.with_timezone(&FixedOffset::east_opt(3 * 3600).unwrap());
        let cairo = instant.with_timezone(&FixedOffset::east_opt(2 * 3600).unwrap());
        assert_ne!(local_stamp(&riyadh, false), local_stamp(&cairo, false));
        assert_eq!(local_stamp(&riyadh, false) - local_stamp(&cairo, false), 60, "an hour apart, in minutes");
        assert_eq!(local_stamp(&riyadh, true) % 60, 30, "seconds keep their second");
    }
    use chrono::NaiveDate;

    #[test]
    fn formats() {
        let dt = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap().and_hms_opt(9, 5, 7).unwrap();
        assert_eq!(hh_mm(&dt), "09:05");
        assert_eq!(seconds(&dt), "07");
        assert_eq!(short_date(&dt), "Tue 27 Oct");
        assert_eq!(long_date(&dt), "Tuesday, 27 October 2026");
        assert_eq!(weekday_ar(&dt), "الثلاثاء");
        assert_eq!(arabic_numerals("12:30"), "١٢:٣٠");
    }

    #[test]
    fn user_patterns_are_validated() {
        use chrono::TimeZone as _;
        assert!(valid_pattern("%H:%M") && valid_pattern("%-I:%M %p"));
        assert!(!valid_pattern("%Q") && !valid_pattern(""));
        assert!(shows_seconds("%H:%M:%S") && shows_seconds("%T") && !shows_seconds("%H:%M"));
        let t = chrono::Utc.with_ymd_and_hms(2026, 10, 27, 21, 5, 0).unwrap();
        assert_eq!(format_with(&t, "%Q", "%H:%M"), "21:05", "invalid → fallback");
        assert_eq!(format_with(&t, "%-I.%M %p", "%H:%M"), "9.05 PM");
    }
}
