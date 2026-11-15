//! Civil dates from Unix time, without a date library (the component has
//! no time zone database: dates are UTC, which is what a moon phase that
//! is good to a day needs).

/// Milliseconds per day.
pub const DAY_MS: f64 = 86_400_000.0;
/// The longest text the date field keeps.
pub const MAX_DATE_CHARS: usize = 32;
/// Monday-first weekday and month names (translated by the plugin's
/// catalogs through `host::translate`).
pub const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTH_NAMES: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
/// Years a typed date may name.
const YEARS: std::ops::RangeInclusive<i64> = 1900..=2200;

/// A day: days since 1970-01-01 and its year, month (1–12) and day (1–31).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Date {
    pub days: i64,
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

impl Date {
    pub fn from_ms(ms: f64) -> Self {
        Self::from_days((ms / DAY_MS).floor() as i64)
    }

    /// From days since 1970-01-01 (Howard Hinnant's `civil_from_days`).
    pub fn from_days(days: i64) -> Self {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        Self { days, year, month, day }
    }

    /// Days since 1970-01-01 of a civil date (`days_from_civil`).
    pub fn civil(year: i64, month: u32, day: u32) -> Self {
        let y = year - i64::from(month <= 2);
        let era = y.div_euclid(400);
        let yoe = y.rem_euclid(400);
        let m = i64::from(month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Self::from_days(era * 146_097 + doe - 719_468)
    }

    /// `YYYY-MM-DD` (spaces around it are fine); `None` for anything else
    /// or a day that doesn't exist.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().splitn(3, '-');
        let year: i64 = parts.next()?.parse().ok()?;
        let month: u32 = parts.next()?.parse().ok()?;
        let day: u32 = parts.next()?.parse().ok()?;
        if !YEARS.contains(&year) || !(1..=12).contains(&month) || day == 0 {
            return None;
        }
        let date = Self::civil(year, month, day);
        (date.month == month && date.day == day).then_some(date)
    }

    /// 0 = Monday … 6 = Sunday (1970-01-01 was a Thursday).
    pub fn weekday(&self) -> u32 {
        (self.days + 3).rem_euclid(7) as u32
    }

    pub fn first_of_month(&self) -> Self {
        Self::civil(self.year, self.month, 1)
    }

    pub fn days_in_month(&self) -> u32 {
        let next = if self.month == 12 { Self::civil(self.year + 1, 1, 1) } else { Self::civil(self.year, self.month + 1, 1) };
        (next.days - self.first_of_month().days) as u32
    }

    /// The first of the month `n` months from this one.
    pub fn add_months(&self, n: i32) -> Self {
        let index = self.year * 12 + i64::from(self.month) - 1 + i64::from(n);
        Self::civil(index.div_euclid(12), (index.rem_euclid(12) + 1) as u32, 1)
    }

    pub fn iso(&self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// "Sat 21 Nov".
    pub fn short(&self) -> String {
        format!("{} {} {}", crate::host::translate(WEEKDAYS[self.weekday() as usize]), self.day, crate::host::translate(MONTHS[self.month as usize - 1]))
    }

    /// "21 Nov": compact enough for a row's trailing edge in any language.
    pub fn day_month(&self) -> String {
        format!("{} {}", self.day, crate::host::translate(MONTHS[self.month as usize - 1]))
    }

    /// "21 Nov 2026".
    pub fn medium(&self) -> String {
        format!("{} {}", self.day_month(), self.year)
    }

    /// "Sat 21 Nov 2026".
    pub fn long(&self) -> String {
        format!("{} {}", self.short(), self.year)
    }

    /// "November 2026".
    pub fn month_year(&self) -> String {
        format!("{} {}", crate::host::translate(MONTH_NAMES[self.month as usize - 1]), self.year)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(Date::from_days(0), Date { days: 0, year: 1970, month: 1, day: 1 });
        assert_eq!(Date::civil(2026, 11, 15).iso(), "2026-11-15");
        assert_eq!(Date::civil(2026, 11, 15).weekday(), 6, "a Sunday");
        assert_eq!(Date::civil(2024, 2, 1).days_in_month(), 29);
        assert_eq!(Date::civil(2026, 12, 1).add_months(1).iso(), "2027-01-01");
        assert_eq!(Date::civil(2026, 1, 1).add_months(-1).iso(), "2025-12-01");
        assert_eq!(Date::parse(" 2026-12-24 ").map(|d| d.iso()).as_deref(), Some("2026-12-24"));
        for bad in ["2026-02-30", "2026-13-01", "26-1-1x", "", "1800-01-01", "2026-00-10"] {
            assert!(Date::parse(bad).is_none(), "{bad}");
        }
    }
}
