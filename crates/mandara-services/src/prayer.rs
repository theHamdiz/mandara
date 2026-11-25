//! Astronomical prayer-time calculation (no network, no dependencies).
//!
//! Implements the classic solar-position method: sun declination and the
//! equation of time from the Julian date, then the hour angle at which the
//! sun reaches each method's depression angle. Defaults follow the Egyptian
//! General Authority of Survey (Fajr 19.5°, Isha 17.5°, Shafi'i Asr), with the
//! location defaulting to El-Mandara, Alexandria.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Prayer {
    Fajr,
    Sunrise,
    Dhuhr,
    Asr,
    Maghrib,
    Isha,
}

impl Prayer {
    pub const ALL: [Prayer; 6] = [Prayer::Fajr, Prayer::Sunrise, Prayer::Dhuhr, Prayer::Asr, Prayer::Maghrib, Prayer::Isha];

    /// English name (stable: used in JSON output). Marked for translation;
    /// display it with `mandara_i18n::tr(p.name())`.
    pub fn name(self) -> &'static str {
        match self {
            Prayer::Fajr => mandara_i18n::tk("Fajr"),
            Prayer::Sunrise => mandara_i18n::tk("Sunrise"),
            Prayer::Dhuhr => mandara_i18n::tk("Dhuhr"),
            Prayer::Asr => mandara_i18n::tk("Asr"),
            Prayer::Maghrib => mandara_i18n::tk("Maghrib"),
            Prayer::Isha => mandara_i18n::tk("Isha"),
        }
    }

    pub fn name_ar(self) -> &'static str {
        match self {
            Prayer::Fajr => "الفجر",
            Prayer::Sunrise => "الشروق",
            Prayer::Dhuhr => "الظهر",
            Prayer::Asr => "العصر",
            Prayer::Maghrib => "المغرب",
            Prayer::Isha => "العشاء",
        }
    }

    /// Sunrise is a time marker, not a prayer.
    pub fn is_prayer(self) -> bool {
        self != Prayer::Sunrise
    }
}

/// Calculation method (twilight depression angles).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Method {
    /// Egyptian General Authority of Survey: Fajr 19.5°, Isha 17.5°.
    #[default]
    Egyptian,
    /// Muslim World League: 18° / 17°.
    MuslimWorldLeague,
    /// Umm al-Qura: Fajr 18.5°, Isha 90 min after Maghrib.
    UmmAlQura,
    /// ISNA: 15° / 15°.
    Isna,
    /// University of Islamic Sciences, Karachi: 18° / 18°.
    Karachi,
}

impl Method {
    fn fajr_angle(self) -> f64 {
        match self {
            Method::Egyptian => 19.5,
            Method::MuslimWorldLeague | Method::Karachi => 18.0,
            Method::UmmAlQura => 18.5,
            Method::Isna => 15.0,
        }
    }

    /// Isha as a depression angle, or minutes after Maghrib.
    fn isha(self) -> IshaRule {
        match self {
            Method::Egyptian => IshaRule::Angle(17.5),
            Method::MuslimWorldLeague => IshaRule::Angle(17.0),
            Method::UmmAlQura => IshaRule::MinutesAfterMaghrib(90.0),
            Method::Isna => IshaRule::Angle(15.0),
            Method::Karachi => IshaRule::Angle(18.0),
        }
    }
}

enum IshaRule {
    Angle(f64),
    MinutesAfterMaghrib(f64),
}

/// Asr juristic school (shadow length factor).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AsrSchool {
    #[default]
    Shafii,
    Hanafi,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
}

impl Location {
    /// El-Mandara, Alexandria.
    pub const EL_MANDARA: Location = Location { latitude: 31.2840, longitude: 30.0170 };
}

/// One day's times in local wall-clock time.
#[derive(Clone, Debug, PartialEq)]
pub struct PrayerTimes {
    pub date: NaiveDate,
    pub times: [(Prayer, NaiveTime); 6],
}

impl PrayerTimes {
    pub fn get(&self, prayer: Prayer) -> NaiveTime {
        self.times.iter().find(|(p, _)| *p == prayer).map(|(_, t)| *t).unwrap_or_default()
    }

    /// The next actual prayer after `now` (today only; `None` after Isha).
    pub fn next_after(&self, now: NaiveTime) -> Option<(Prayer, NaiveTime)> {
        self.times.iter().copied().find(|(p, t)| p.is_prayer() && *t > now)
    }

    /// The prayer whose time window contains `now`.
    pub fn current(&self, now: NaiveTime) -> Option<Prayer> {
        self.times.iter().rev().find(|(p, t)| p.is_prayer() && *t <= now).map(|(p, _)| *p)
    }
}

fn deg(r: f64) -> f64 {
    r.to_degrees()
}
fn rad(d: f64) -> f64 {
    d.to_radians()
}
fn fix_angle(a: f64) -> f64 {
    a.rem_euclid(360.0)
}
fn fix_hour(h: f64) -> f64 {
    h.rem_euclid(24.0)
}

fn julian_date(date: NaiveDate) -> f64 {
    let (mut y, mut m) = (date.year() as f64, date.month() as f64);
    let d = date.day() as f64;
    if m <= 2.0 {
        y -= 1.0;
        m += 12.0;
    }
    let a = (y / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y + 4716.0)).floor() + (30.6001 * (m + 1.0)).floor() + d + b - 1524.5
}

/// Sun declination (degrees) and equation of time (hours) at Julian date `jd`.
fn sun_position(jd: f64) -> (f64, f64) {
    let d = jd - 2451545.0;
    let g = fix_angle(357.529 + 0.98560028 * d);
    let q = fix_angle(280.459 + 0.98564736 * d);
    let l = fix_angle(q + 1.915 * rad(g).sin() + 0.020 * rad(2.0 * g).sin());
    let e = 23.439 - 0.00000036 * d;
    let ra = deg((rad(e).cos() * rad(l).sin()).atan2(rad(l).cos())) / 15.0;
    let eqt = q / 15.0 - fix_hour(ra);
    let eqt = if eqt > 12.0 { eqt - 24.0 } else if eqt < -12.0 { eqt + 24.0 } else { eqt };
    let decl = deg((rad(e).sin() * rad(l).sin()).asin());
    (decl, eqt)
}

/// Computes the day's times. `utc_offset_hours` is the local offset (e.g.
/// `3.0` for EEST). Times are rounded to the nearest minute.
pub fn compute(date: NaiveDate, loc: Location, utc_offset_hours: f64, method: Method, school: AsrSchool) -> PrayerTimes {
    let jd = julian_date(date) - loc.longitude / (15.0 * 24.0);
    // Two refinement passes: evaluate the sun at each event's own time.
    let mid_day = |t: f64| {
        let (_, eqt) = sun_position(jd + t / 24.0);
        fix_hour(12.0 - eqt)
    };
    let angle_time = |angle: f64, t: f64, ccw: bool| -> f64 {
        let (decl, _) = sun_position(jd + t / 24.0);
        let noon = mid_day(t);
        let lat = rad(loc.latitude);
        let cos_h = (-rad(angle).sin() - lat.sin() * rad(decl).sin()) / (lat.cos() * rad(decl).cos());
        let h = deg(cos_h.clamp(-1.0, 1.0).acos()) / 15.0;
        if ccw { noon - h } else { noon + h }
    };
    let asr_angle = |t: f64| -> f64 {
        let (decl, _) = sun_position(jd + t / 24.0);
        let factor = match school {
            AsrSchool::Shafii => 1.0,
            AsrSchool::Hanafi => 2.0,
        };
        -deg((1.0 / (factor + rad(loc.latitude - decl).abs().tan())).atan())
    };

    // Initial guesses (hours), then refine.
    let mut fajr = 5.0;
    let mut sunrise = 6.0;
    let mut dhuhr = 12.0;
    let mut asr = 13.0;
    let mut sunset = 18.0;
    let mut isha = 18.0;
    for _ in 0..2 {
        fajr = angle_time(method.fajr_angle(), fajr, true);
        sunrise = angle_time(0.833, sunrise, true);
        dhuhr = mid_day(dhuhr);
        asr = angle_time(asr_angle(asr), asr, false);
        sunset = angle_time(0.833, sunset, false);
        isha = match method.isha() {
            IshaRule::Angle(a) => angle_time(a, isha, false),
            IshaRule::MinutesAfterMaghrib(m) => sunset + m / 60.0,
        };
    }
    let local = |t: f64| -> NaiveTime {
        let h = fix_hour(t + utc_offset_hours - loc.longitude / 15.0);
        let total_minutes = (h * 60.0).round() as u32 % (24 * 60);
        NaiveTime::from_hms_opt(total_minutes / 60, total_minutes % 60, 0).unwrap_or_default()
    };
    PrayerTimes {
        date,
        times: [
            (Prayer::Fajr, local(fajr)),
            (Prayer::Sunrise, local(sunrise)),
            (Prayer::Dhuhr, local(dhuhr)),
            (Prayer::Asr, local(asr)),
            (Prayer::Maghrib, local(sunset)),
            (Prayer::Isha, local(isha)),
        ],
    }
}

/// Minutes from `now` until `then` (wrapping past midnight).
pub fn minutes_until(now: NaiveTime, then: NaiveTime) -> i64 {
    let diff = then.num_seconds_from_midnight() as i64 - now.num_seconds_from_midnight() as i64;
    let diff = if diff < 0 { diff + 86_400 } else { diff };
    (diff + 59) / 60
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    fn within(actual: NaiveTime, expected: NaiveTime, minutes: i64) -> bool {
        (actual.signed_duration_since(expected).num_minutes()).abs() <= minutes
    }

    #[test]
    fn alexandria_late_october() {
        // Alexandria, 27 Oct 2026, EEST (UTC+3). Solar noon is ~12:44.
        let times = compute(
            NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
            Location::EL_MANDARA,
            3.0,
            Method::Egyptian,
            AsrSchool::Shafii,
        );
        assert!(within(times.get(Prayer::Dhuhr), t(12, 44), 2), "dhuhr {}", times.get(Prayer::Dhuhr));
        // Plausibility of the rest of the day for 31°N in late October.
        assert!(within(times.get(Prayer::Sunrise), t(7, 2), 10), "sunrise {}", times.get(Prayer::Sunrise));
        assert!(within(times.get(Prayer::Maghrib), t(18, 25), 10), "maghrib {}", times.get(Prayer::Maghrib));
        assert!(times.get(Prayer::Fajr) < times.get(Prayer::Sunrise));
        assert!(times.get(Prayer::Dhuhr) < times.get(Prayer::Asr));
        assert!(times.get(Prayer::Asr) < times.get(Prayer::Maghrib));
        assert!(times.get(Prayer::Maghrib) < times.get(Prayer::Isha));
        // Egyptian Fajr is ~80 minutes before sunrise at this latitude.
        let gap = times.get(Prayer::Sunrise).signed_duration_since(times.get(Prayer::Fajr)).num_minutes();
        assert!((70..=95).contains(&gap), "fajr→sunrise gap {gap}");
    }

    #[test]
    fn umm_al_qura_isha_is_90_minutes_after_maghrib() {
        let times = compute(
            NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            Location { latitude: 21.4225, longitude: 39.8262 },
            3.0,
            Method::UmmAlQura,
            AsrSchool::Shafii,
        );
        let gap = times.get(Prayer::Isha).signed_duration_since(times.get(Prayer::Maghrib)).num_minutes();
        assert!((89..=91).contains(&gap));
    }

    #[test]
    fn hanafi_asr_is_later() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 21).unwrap();
        let shafii = compute(date, Location::EL_MANDARA, 3.0, Method::Egyptian, AsrSchool::Shafii);
        let hanafi = compute(date, Location::EL_MANDARA, 3.0, Method::Egyptian, AsrSchool::Hanafi);
        assert!(hanafi.get(Prayer::Asr) > shafii.get(Prayer::Asr));
    }

    #[test]
    fn next_and_current() {
        let times = compute(NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(), Location::EL_MANDARA, 3.0, Method::Egyptian, AsrSchool::Shafii);
        let (next, _) = times.next_after(t(10, 0)).unwrap();
        assert_eq!(next, Prayer::Dhuhr);
        assert_eq!(times.current(t(10, 0)), Some(Prayer::Fajr));
        assert!(times.next_after(t(23, 59)).is_none());
        assert_eq!(minutes_until(t(23, 50), t(0, 10)), 20);
    }
}
