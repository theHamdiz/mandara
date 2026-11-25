//! Night-light maths, pure Rust and allocation-light: colour temperature →
//! RGB white point (CIE Planckian locus), gamma-ramp tables for
//! `wlr-gamma-control`, and the day/night schedule (fixed times or
//! sunrise / sunset from [`crate::world_clock::daylight`]).

use chrono::{DateTime, Local, NaiveDate, NaiveTime, TimeZone as _, Timelike as _};
use mandara_config::outputs::{NightLightConfig, NightSchedule, defaults};
use serde::Serialize;

use crate::world_clock::{Daylight, daylight};

/// Validity range of the Kim et al. Planckian-locus fit, kelvin.
const LOCUS_MIN_K: f64 = 1667.0;
const LOCUS_MAX_K: f64 = 25000.0;
/// Where the locus fit switches polynomial pieces, kelvin.
const LOCUS_SPLIT_X_K: f64 = 4000.0;
const LOCUS_SPLIT_Y_K: f64 = 2222.0;
/// Kim et al. (2002) cubic fits of the Planckian locus in CIE 1931 xy.
const X_LOW: [f64; 4] = [-0.266_123_9e9, -0.234_358_9e6, 0.877_695_6e3, 0.179_910];
const X_HIGH: [f64; 4] = [-3.025_846_9e9, 2.107_037_9e6, 0.222_634_7e3, 0.240_390];
const Y_LOW: [f64; 4] = [-1.106_381_4, -1.348_110_20, 2.185_558_32, -0.202_196_83];
const Y_MID: [f64; 4] = [-0.954_947_6, -1.374_185_93, 2.091_370_15, -0.167_488_67];
const Y_HIGH: [f64; 4] = [3.081_758_0, -5.873_386_70, 3.751_129_97, -0.370_014_83];
/// CIE XYZ → linear sRGB (D65).
const XYZ_TO_SRGB: [[f64; 3]; 3] = [[3.240_454_2, -1.537_138_5, -0.498_531_4], [-0.969_266_0, 1.876_010_8, 0.041_556_0], [0.055_643_4, -0.204_025_9, 1.057_225_2]];
/// Mireds per kelvin⁻¹ (fades interpolate in mireds, which is perceptually
/// even).
const MIRED: f64 = 1.0e6;
/// Minutes in a day.
const DAY_MINUTES: f64 = 1440.0;
/// Seconds per minute.
const SECONDS_PER_MINUTE: f64 = 60.0;
/// Display encoding gamma (sRGB ≈ 2.2) between linear light and ramp values.
const DISPLAY_GAMMA: f64 = 2.2;
/// Smallest gamma exponent accepted (guards a division by zero).
const MIN_GAMMA: f32 = 0.1;

fn cubic(c: &[f64; 4], v: f64) -> f64 {
    ((c[0] * v + c[1]) * v + c[2]) * v + c[3]
}

/// Chromaticity of a black body at `kelvin`.
fn locus_xy(kelvin: f64) -> (f64, f64) {
    let t = kelvin.clamp(LOCUS_MIN_K, LOCUS_MAX_K);
    let inv = 1.0 / t;
    // x fits are in powers of 1/T: c0/T³ + c1/T² + c2/T + c3.
    let x = cubic(if t <= LOCUS_SPLIT_X_K { &X_LOW } else { &X_HIGH }, inv);
    let y = cubic(
        if t <= LOCUS_SPLIT_Y_K {
            &Y_LOW
        } else if t <= LOCUS_SPLIT_X_K {
            &Y_MID
        } else {
            &Y_HIGH
        },
        x,
    );
    (x, y)
}

fn linear_rgb(kelvin: f64) -> [f64; 3] {
    let (x, y) = locus_xy(kelvin);
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    XYZ_TO_SRGB.map(|row| (row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2]).max(0.0))
}

/// The white point of `kelvin` as per-channel multipliers in `0..=1`
/// (brightest channel = 1). 6500 K is exactly white.
pub fn white_point(kelvin: u32) -> [f32; 3] {
    let k = (kelvin.clamp(defaults::MIN_TEMPERATURE_K, defaults::MAX_TEMPERATURE_K)) as f64;
    let rgb = linear_rgb(k);
    let neutral = linear_rgb(defaults::NEUTRAL_TEMPERATURE_K as f64);
    let rel = [rgb[0] / neutral[0], rgb[1] / neutral[1], rgb[2] / neutral[2]];
    let max = rel.iter().copied().fold(f64::MIN, f64::max).max(f64::EPSILON);
    // Below the locus fit (candle light), keep warming: green and blue fade
    // with the square of the remaining range.
    let below = (k / LOCUS_MIN_K).min(1.0).powi(2);
    let [r, g, b] = rel.map(|c| (c / max).clamp(0.0, 1.0));
    // Gamma ramps act on encoded values: encode the linear multipliers.
    [r, g * below, b * below].map(|c| c.powf(1.0 / DISPLAY_GAMMA) as f32)
}

/// What the gamma ramps should show.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub struct GammaTarget {
    pub temperature: u32,
    /// Exponent applied to the ramp (1 = unchanged).
    pub gamma: f32,
    /// Output multiplier (1 = unchanged).
    pub brightness: f32,
}

impl GammaTarget {
    pub const NEUTRAL: GammaTarget = GammaTarget { temperature: defaults::NEUTRAL_TEMPERATURE_K, gamma: defaults::NEUTRAL_GAMMA, brightness: defaults::NEUTRAL_BRIGHTNESS };

    /// Within rounding of an untouched ramp (Mandara then releases the
    /// gamma controls, which restores the compositor's own ramp).
    pub fn is_neutral(&self) -> bool {
        *self == Self::NEUTRAL
    }

    /// Mired-space interpolation towards `to` (`t` in `0..=1`).
    pub fn lerp(self, to: GammaTarget, t: f32) -> GammaTarget {
        let t = t.clamp(0.0, 1.0);
        let (a, b) = (MIRED / self.temperature.max(1) as f64, MIRED / to.temperature.max(1) as f64);
        let mired = a + (b - a) * t as f64;
        GammaTarget {
            temperature: (MIRED / mired).round() as u32,
            gamma: self.gamma + (to.gamma - self.gamma) * t,
            brightness: self.brightness + (to.brightness - self.brightness) * t,
        }
    }
}

/// Fills `out` (`3 × size` `u16`s: all red, then green, then blue — the
/// `wlr-gamma-control` layout) for `target`.
pub fn fill_ramp(size: usize, target: GammaTarget, out: &mut Vec<u16>) {
    out.clear();
    let wp = white_point(target.temperature);
    let inv_gamma = 1.0 / target.gamma.max(MIN_GAMMA);
    let last = size.saturating_sub(1).max(1) as f32;
    for channel in wp {
        let scale = channel * target.brightness.clamp(0.0, 1.0);
        out.extend((0..size).map(|i| {
            let v = (i as f32 / last).powf(inv_gamma) * scale;
            (v.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
        }));
    }
}

/// Classic smoothstep easing.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far into the night (`0` = day, `1` = night) `now` is, for a window
/// from `start` to `end` (minutes after local midnight, wrapping past
/// midnight) with fades of `transition` minutes starting at each bound.
pub fn night_factor(now: f64, start: f64, end: f64, transition: f64) -> f32 {
    let wrap = |m: f64| m.rem_euclid(DAY_MINUTES);
    let night_len = wrap(end - start);
    let day_len = DAY_MINUTES - night_len;
    if night_len <= 0.0 {
        return 0.0;
    }
    let fade = transition.max(0.0).min(night_len).min(day_len);
    let since_start = wrap(now - start);
    if since_start < night_len {
        return if fade <= 0.0 { 1.0 } else { ease((since_start / fade) as f32) };
    }
    let since_end = wrap(now - end);
    if fade > 0.0 && since_end < fade { 1.0 - ease((since_end / fade) as f32) } else { 0.0 }
}

/// Parses `HH:MM` to minutes after midnight.
pub fn parse_minutes(hm: &str) -> Option<f64> {
    let t = NaiveTime::parse_from_str(hm.trim(), "%H:%M").ok()?;
    Some(t.hour() as f64 * 60.0 + t.minute() as f64)
}

fn minutes_of(t: DateTime<Local>) -> f64 {
    t.hour() as f64 * 60.0 + t.minute() as f64 + t.second() as f64 / SECONDS_PER_MINUTE
}

/// The night window in local minutes for `date`: `None` = never night
/// (polar day), `Some((0, 0))`-style full-day windows are reported as
/// `PolarNight`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub enum NightWindow {
    /// Warm from `start` to `end` (local minutes after midnight).
    Between { start: f64, end: f64 },
    /// No night today (sun never sets, or the feature is off).
    Never,
    /// Night all day (sun never rises, or "always").
    Always,
}

/// The night window for `cfg` on the local day of `now`; `lat`/`lon` are
/// used by the sun schedule.
pub fn window(cfg: &NightLightConfig, now: DateTime<Local>, lat: f64, lon: f64) -> NightWindow {
    match cfg.schedule {
        NightSchedule::Always => NightWindow::Always,
        NightSchedule::Fixed => match (parse_minutes(&cfg.start), parse_minutes(&cfg.end)) {
            (Some(start), Some(end)) if start != end => NightWindow::Between { start, end },
            _ => NightWindow::Never,
        },
        NightSchedule::Sun => match sun_minutes(lat, lon, now.date_naive()) {
            Daylight::Normal { sunrise, sunset } => NightWindow::Between { start: minutes_of(sunset.with_timezone(&Local)), end: minutes_of(sunrise.with_timezone(&Local)) },
            Daylight::PolarDay => NightWindow::Never,
            Daylight::PolarNight => NightWindow::Always,
        },
    }
}

/// Sunrise / sunset for the local `date` (computed for that UTC day).
fn sun_minutes(lat: f64, lon: f64, date: NaiveDate) -> Daylight {
    daylight(lat, lon, date)
}

/// Where the schedule stands.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
pub struct Phase {
    /// `0` = day, `1` = night.
    pub factor: f32,
    /// The temperature the schedule asks for now.
    pub target: GammaTarget,
    pub window: NightWindow,
}

/// The scheduled target for `now` (ignores `enabled`; callers decide).
pub fn phase(cfg: &NightLightConfig, now: DateTime<Local>, lat: f64, lon: f64) -> Phase {
    let window = window(cfg, now, lat, lon);
    let factor = match window {
        NightWindow::Never => 0.0,
        NightWindow::Always => 1.0,
        NightWindow::Between { start, end } => night_factor(minutes_of(now), start, end, cfg.transition_minutes as f64),
    };
    let day = GammaTarget { temperature: cfg.day_temperature, ..GammaTarget::NEUTRAL };
    let night = GammaTarget { temperature: cfg.night_temperature, gamma: cfg.gamma, brightness: cfg.brightness };
    Phase { factor, target: day.lerp(night, factor), window }
}

/// The next time the schedule starts or ends a night after `now`, and
/// whether it starts one.
pub fn next_change(window: NightWindow, now: DateTime<Local>) -> Option<(DateTime<Local>, bool)> {
    let NightWindow::Between { start, end } = window else { return None };
    let now_min = minutes_of(now);
    let ahead = |m: f64| (m - now_min).rem_euclid(DAY_MINUTES);
    let (to_night, minutes) = if ahead(start) <= ahead(end) { (true, ahead(start)) } else { (false, ahead(end)) };
    let at = now + chrono::Duration::seconds((minutes * SECONDS_PER_MINUTE).round() as i64);
    Local.from_local_datetime(&at.naive_local()).single().map(|t| (t, to_night))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn neutral_is_white_and_warm_drops_blue() {
        let w = white_point(defaults::NEUTRAL_TEMPERATURE_K);
        assert!(w.iter().all(|c| close(*c, 1.0, 1e-4)), "{w:?}");
        let mut last_blue = 1.0;
        for k in [5500, 4500, 4000, 3400, 2700, 1900, 1200] {
            let [r, g, b] = white_point(k);
            assert!(close(r, 1.0, 1e-4), "red stays full at {k}: {r}");
            assert!(g > b && b <= last_blue, "{k}: {g} {b}");
            last_blue = b;
        }
        // In the same ballpark as other night lights (redshift's table).
        let [_, g, b] = white_point(4000);
        assert!(white_point(1000)[1] < white_point(1700)[1]);
        assert!((0.7..0.9).contains(&g) && (0.45..0.75).contains(&b), "{g} {b}");
        // Cooler than daylight shifts toward blue.
        let [r, _, b] = white_point(9000);
        assert!(r < 1.0 && close(b, 1.0, 1e-4));
    }

    #[test]
    fn ramps_have_the_wlr_layout() {
        let mut ramp = Vec::new();
        fill_ramp(256, GammaTarget::NEUTRAL, &mut ramp);
        assert_eq!(ramp.len(), 768);
        assert_eq!((ramp[0], ramp[255], ramp[511], ramp[767]), (0, u16::MAX, u16::MAX, u16::MAX));
        assert!(ramp[..256].windows(2).all(|w| w[0] <= w[1]));
        fill_ramp(256, GammaTarget { temperature: 3000, gamma: 1.0, brightness: 0.8 }, &mut ramp);
        assert!(ramp[255] > ramp[511] && ramp[511] > ramp[767]);
        assert!(ramp[255] <= (0.8 * u16::MAX as f32) as u16 + 1);
    }

    #[test]
    fn lerp_runs_in_mireds() {
        let a = GammaTarget::NEUTRAL;
        let b = GammaTarget { temperature: 3000, gamma: 1.0, brightness: 1.0 };
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0).temperature, 3000);
        let mid = a.lerp(b, 0.5).temperature;
        // Mired midpoint of 6500 K and 3000 K ≈ 4105 K (a plain average is 4750).
        assert!((4050..4160).contains(&mid), "{mid}");
        assert!(a.is_neutral() && !b.is_neutral());
    }

    #[test]
    fn schedule_factor_wraps_midnight_and_fades() {
        let (start, end, fade) = (20.0 * 60.0, 7.0 * 60.0, 30.0);
        assert_eq!(night_factor(12.0 * 60.0, start, end, fade), 0.0);
        assert_eq!(night_factor(23.0 * 60.0, start, end, fade), 1.0);
        assert_eq!(night_factor(3.0 * 60.0, start, end, fade), 1.0);
        let dusk = night_factor(20.0 * 60.0 + 15.0, start, end, fade);
        assert!(close(dusk, 0.5, 1e-3), "{dusk}");
        let dawn = night_factor(7.0 * 60.0 + 15.0, start, end, fade);
        assert!(close(dawn, 0.5, 1e-3), "{dawn}");
        assert_eq!(night_factor(7.0 * 60.0 + 31.0, start, end, fade), 0.0);
        // No fade: hard switch.
        assert_eq!(night_factor(20.0 * 60.0, start, end, 0.0), 1.0);
        // Same-day window.
        assert_eq!(night_factor(13.0 * 60.0, 12.0 * 60.0, 14.0 * 60.0, 0.0), 1.0);
        assert_eq!(night_factor(15.0 * 60.0, 12.0 * 60.0, 14.0 * 60.0, 0.0), 0.0);
    }

    #[test]
    fn phases_and_next_change() {
        let at = |h: u32, m: u32| Local.with_ymd_and_hms(2026, 10, 27, h, m, 0).single().unwrap();
        let fixed = NightLightConfig { enabled: true, schedule: NightSchedule::Fixed, transition_minutes: 0, ..Default::default() };
        let (lat, lon) = (31.284, 30.017);
        assert_eq!(phase(&fixed, at(22, 0), lat, lon).target.temperature, fixed.night_temperature);
        assert_eq!(phase(&fixed, at(12, 0), lat, lon).target, GammaTarget::NEUTRAL);
        let (next, to_night) = next_change(window(&fixed, at(12, 0), lat, lon), at(12, 0)).unwrap();
        assert!(to_night && next.hour() == 20 && next.minute() == 0, "{next}");
        let always = NightLightConfig { schedule: NightSchedule::Always, ..fixed.clone() };
        assert_eq!(phase(&always, at(12, 0), lat, lon).factor, 1.0);
        assert!(next_change(window(&always, at(12, 0), lat, lon), at(12, 0)).is_none());
        // Alexandria in late October: dark at midnight, light at noon (local
        // time of the test machine may differ, so only check the window).
        let sun = NightLightConfig { schedule: NightSchedule::Sun, ..fixed };
        assert!(matches!(window(&sun, at(12, 0), lat, lon), NightWindow::Between { .. }));
        // Svalbard in December: polar night.
        assert_eq!(window(&sun, Local.with_ymd_and_hms(2026, 12, 21, 12, 0, 0).single().unwrap(), 78.22, 15.65), NightWindow::Always);
        assert_eq!(parse_minutes("07:30"), Some(450.0));
        assert_eq!(parse_minutes("7pm"), None);
    }
}
