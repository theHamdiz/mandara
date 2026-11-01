//! The sky by arithmetic: the moon's phase and the sun's path for a place,
//! from the time alone (no network). Accurate to a few minutes, which is
//! what a desktop widget needs.
//!
//! Formulas: the mean synodic month from a reference new moon, and NOAA's
//! low-precision solar position (declination, equation of time, hour
//! angle at the standard −0.833° sunrise altitude).

use std::f64::consts::TAU;

use chrono::{DateTime, Datelike as _, NaiveDate, TimeZone, Utc};

/// Mean length of a lunar month (new moon to new moon), days.
pub const SYNODIC_MONTH_DAYS: f64 = 29.530_588_853;
/// A new moon: 2000-01-06 18:14 UTC, as Unix seconds.
const REFERENCE_NEW_MOON_UNIX: f64 = 947_182_440.0;
const SECONDS_PER_DAY: f64 = 86_400.0;
/// Altitude of the sun's centre at sunrise and sunset (refraction and the
/// disc's radius), degrees.
const SUNRISE_ALTITUDE_DEG: f64 = -0.833;
/// Julian day of the Unix epoch, and of J2000.0.
const JD_UNIX_EPOCH: f64 = 2_440_587.5;
const JD_J2000: f64 = 2_451_545.0;
const DAYS_PER_CENTURY: f64 = 36_525.0;
const MINUTES_PER_DAY: f64 = 1_440.0;
/// Minutes of time per degree of longitude.
const MINUTES_PER_DEG: f64 = 4.0;

/// The moon's age as a fraction of its cycle at `unix` seconds: 0 new,
/// ¼ first quarter, ½ full, ¾ last quarter.
pub fn moon_phase(unix: i64) -> f32 {
    let days = (unix as f64 - REFERENCE_NEW_MOON_UNIX) / SECONDS_PER_DAY;
    (days / SYNODIC_MONTH_DAYS).rem_euclid(1.0) as f32
}

/// The lit fraction of the moon's disc at `phase`.
pub fn moon_lit(phase: f32) -> f32 {
    (1.0 - (phase * std::f32::consts::TAU).cos()) / 2.0
}

/// The name of a moon phase (an English catalog key).
pub fn moon_name(phase: f32) -> &'static str {
    use mandara_i18n::tk;
    // Eight phases, each centred on its exact moment.
    match ((phase * 8.0 + 0.5).floor() as i32).rem_euclid(8) {
        0 => tk("New moon"),
        1 => tk("Waxing crescent"),
        2 => tk("First quarter"),
        3 => tk("Waxing gibbous"),
        4 => tk("Full moon"),
        5 => tk("Waning gibbous"),
        6 => tk("Last quarter"),
        _ => tk("Waning crescent"),
    }
}

/// The sun on one day at one place.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SunDay {
    /// Sunrise and sunset (UTC); `None` in polar day or night.
    pub rise: Option<DateTime<Utc>>,
    pub set: Option<DateTime<Utc>>,
    /// Solar noon (UTC).
    pub noon: DateTime<Utc>,
}

fn julian_century(unix: f64) -> f64 {
    (unix / SECONDS_PER_DAY + JD_UNIX_EPOCH - JD_J2000) / DAYS_PER_CENTURY
}

/// Declination (radians) and equation of time (minutes) at `unix`.
fn solar(unix: f64) -> (f64, f64) {
    let t = julian_century(unix);
    let l0 = (280.466_46 + t * (36_000.769_83 + t * 0.000_303_2)).rem_euclid(360.0);
    let m = 357.529_11 + t * (35_999.050_29 - 0.000_153_7 * t);
    let e = 0.016_708_634 - t * (0.000_042_037 + 0.000_000_126_7 * t);
    let mr = m.to_radians();
    let c = mr.sin() * (1.914_602 - t * (0.004_817 + 0.000_014 * t)) + (2.0 * mr).sin() * (0.019_993 - 0.000_101 * t) + (3.0 * mr).sin() * 0.000_289;
    let true_long = l0 + c;
    let omega = 125.04 - 1_934.136 * t;
    let lambda = (true_long - 0.005_69 - 0.004_78 * omega.to_radians().sin()).to_radians();
    let eps0 = 23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.000_59 - t * 0.001_813))) / 60.0) / 60.0;
    let eps = (eps0 + 0.002_56 * omega.to_radians().cos()).to_radians();
    let decl = (eps.sin() * lambda.sin()).asin();
    let y = (eps / 2.0).tan().powi(2);
    let l0r = l0.to_radians();
    let eq = 4.0 * (y * (2.0 * l0r).sin() - 2.0 * e * mr.sin() + 4.0 * e * y * mr.sin() * (2.0 * l0r).cos() - 0.5 * y * y * (4.0 * l0r).sin() - 1.25 * e * e * (2.0 * mr).sin()).to_degrees();
    (decl, eq)
}

/// Sunrise, solar noon and sunset on `date` (the local calendar day) at
/// `lat`, `lon` (degrees, east positive).
pub fn sun_day(date: NaiveDate, lat: f64, lon: f64) -> SunDay {
    let midnight = Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap_or_default()).timestamp() as f64;
    // Noon UTC of that day, refined once.
    let mut noon_min = MINUTES_PER_DAY / 2.0 - MINUTES_PER_DEG * lon;
    let (_, eq) = solar(midnight + noon_min * 60.0);
    noon_min -= eq;
    let (decl, eq) = solar(midnight + noon_min * 60.0);
    noon_min = MINUTES_PER_DAY / 2.0 - MINUTES_PER_DEG * lon - eq;
    let at = |minutes: f64| Utc.timestamp_opt((midnight + minutes * 60.0).round() as i64, 0).single();
    let latr = lat.to_radians();
    let cos_h = (SUNRISE_ALTITUDE_DEG.to_radians().sin() - latr.sin() * decl.sin()) / (latr.cos() * decl.cos());
    let (rise, set) = if (-1.0..=1.0).contains(&cos_h) {
        let h = cos_h.acos().to_degrees();
        (at(noon_min - MINUTES_PER_DEG * h), at(noon_min + MINUTES_PER_DEG * h))
    } else {
        (None, None)
    };
    SunDay { rise, set, noon: at(noon_min).unwrap_or_else(Utc::now) }
}

/// The sun's altitude (degrees above the horizon) at `unix` seconds at
/// `lat`, `lon`.
pub fn sun_altitude(unix: i64, lat: f64, lon: f64) -> f64 {
    let (decl, eq) = solar(unix as f64);
    let utc_min = (unix as f64).rem_euclid(SECONDS_PER_DAY) / 60.0;
    let solar_time = (utc_min + eq + MINUTES_PER_DEG * lon).rem_euclid(MINUTES_PER_DAY);
    let hour_angle = (solar_time / MINUTES_PER_DEG - 180.0).to_radians();
    let latr = lat.to_radians();
    (latr.sin() * decl.sin() + latr.cos() * decl.cos() * hour_angle.cos()).asin().to_degrees()
}

/// Where the sun stands overhead at `unix` (the subsolar point): latitude
/// and longitude, degrees.
pub fn subsolar(unix: i64) -> (f64, f64) {
    let (decl, eq) = solar(unix as f64);
    let utc_min = (unix as f64).rem_euclid(SECONDS_PER_DAY) / 60.0;
    let lon = -(utc_min + eq - MINUTES_PER_DAY / 2.0) / MINUTES_PER_DEG;
    (decl.to_degrees(), (lon + 180.0).rem_euclid(360.0) - 180.0)
}

/// Where the moon stands overhead, roughly: its longitude trails the sun's
/// by its age, its latitude swings within the orbit's ±5° tilt plus the
/// sun's declination envelope. Good to a few degrees — enough to place it
/// on a small globe.
pub fn sublunar(unix: i64) -> (f64, f64) {
    let (sun_lat, sun_lon) = subsolar(unix);
    let phase = f64::from(moon_phase(unix));
    let lon = (sun_lon - phase * 360.0 + 180.0).rem_euclid(360.0) - 180.0;
    // Opposite the sun's declination at full moon, like it at new moon.
    let lat = sun_lat * (phase * TAU).cos();
    (lat, lon)
}

/// Day of the year (1-based) and the days in that year, for progress.
pub fn day_of_year(date: NaiveDate) -> (u32, u32) {
    let days = if NaiveDate::from_ymd_opt(date.year(), 12, 31).map_or(365, |d| d.ordinal()) == 366 { 366 } else { 365 };
    (date.ordinal(), days)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_moons() {
        // Full moon: 2024-01-25 17:54 UTC; new moon: 2024-02-09 22:59 UTC.
        let full = Utc.with_ymd_and_hms(2024, 1, 25, 17, 54, 0).unwrap().timestamp();
        assert!((moon_phase(full) - 0.5).abs() < 0.02, "{}", moon_phase(full));
        assert!(moon_lit(moon_phase(full)) > 0.98);
        let new = Utc.with_ymd_and_hms(2024, 2, 9, 22, 59, 0).unwrap().timestamp();
        let p = moon_phase(new);
        assert!(!(0.02..=0.98).contains(&p), "{p}");
        assert_eq!(moon_name(0.5), "Full moon");
        assert_eq!(moon_name(0.99), "New moon");
    }

    #[test]
    fn alexandria_sunrise_and_sunset() {
        // Alexandria (31.2°N, 29.9°E), 2026-06-21: sunrise ≈ 02:56 UTC,
        // sunset ≈ 17:03 UTC (05:56 / 20:03 EEST).
        let d = sun_day(NaiveDate::from_ymd_opt(2026, 6, 21).unwrap(), 31.2, 29.9);
        let rise = d.rise.unwrap();
        let set = d.set.unwrap();
        let mins = |t: DateTime<Utc>| t.timestamp().rem_euclid(86_400) / 60;
        assert!((mins(rise) - (2 * 60 + 56)).abs() <= 5, "{rise}");
        assert!((mins(set) - (17 * 60 + 3)).abs() <= 5, "{set}");
        // At solar noon the sun is high (90 − 31.2 + 23.4 ≈ 82°).
        assert!((sun_altitude(d.noon.timestamp(), 31.2, 29.9) - 82.2).abs() < 1.0);
        // Polar night: no sunrise at 80°N in December.
        let polar = sun_day(NaiveDate::from_ymd_opt(2026, 12, 21).unwrap(), 80.0, 0.0);
        assert!(polar.rise.is_none() && polar.set.is_none());
    }

    #[test]
    fn subsolar_point_follows_the_clock() {
        // Around 12:00 UTC the sun is over the prime meridian (± the
        // equation of time, under 17 minutes ≈ 4.2°).
        let noon = Utc.with_ymd_and_hms(2026, 3, 20, 12, 0, 0).unwrap().timestamp();
        let (lat, lon) = subsolar(noon);
        assert!(lat.abs() < 1.0 && lon.abs() < 4.5, "{lat} {lon}");
        assert_eq!(day_of_year(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap()), (366, 366));
    }
}
