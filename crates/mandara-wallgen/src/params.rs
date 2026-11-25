//! Parameter specs: every visual variable of every style is a [`ParamSpec`]
//! so a UI can build its sliders generically. Values are stored in display
//! units (percent as 0–100, angles in degrees).

use mandara_i18n::tk;

/// One tunable variable of a style.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    /// Stable key, used in recipes.
    pub key: &'static str,
    /// English label, marked for translation (`mandara_i18n::tr` it).
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    /// Slider step (1 for counts).
    pub step: f32,
    pub default: f32,
    /// Display unit (`%`, `°`, `EV` or empty).
    pub unit: &'static str,
    /// The range [`crate::Recipe::random`] draws from: the slice of the
    /// slider that reliably looks good. Always inside `min..=max`.
    pub nice: (f32, f32),
}

impl ParamSpec {
    /// `v` clamped to the range and snapped to the step (counts stay whole).
    pub fn clamp(&self, v: f32) -> f32 {
        if !v.is_finite() {
            return self.default;
        }
        let v = v.clamp(self.min, self.max);
        if self.step >= 1.0 { v.round().clamp(self.min, self.max) } else { v }
    }

    /// Whether the value is a whole-number count.
    pub fn is_count(&self) -> bool {
        self.step >= 1.0 && self.unit.is_empty()
    }
}

/// Builds a spec (const so the tables below are static).
#[allow(clippy::too_many_arguments)]
pub(crate) const fn spec(key: &'static str, label: &'static str, min: f32, max: f32, step: f32, default: f32, unit: &'static str, nice: (f32, f32)) -> ParamSpec {
    ParamSpec { key, label, min, max, step, default, unit, nice }
}

/// A 0–100 % spec.
pub(crate) const fn pct(key: &'static str, label: &'static str, default: f32, nice: (f32, f32)) -> ParamSpec {
    spec(key, label, 0.0, 100.0, 1.0, default, "%", nice)
}

/// A whole-number count.
pub(crate) const fn count(key: &'static str, label: &'static str, min: f32, max: f32, default: f32, nice: (f32, f32)) -> ParamSpec {
    spec(key, label, min, max, 1.0, default, "", nice)
}

pub const EXPOSURE: &str = "exposure";
pub const CONTRAST: &str = "contrast";
pub const SATURATION: &str = "saturation";
pub const HUE_SHIFT: &str = "hue_shift";
pub const TEMPERATURE: &str = "temperature";
pub const VIGNETTE: &str = "vignette";
pub const GRAIN: &str = "grain";
pub const DITHER: &str = "dither";
pub const SOFTNESS: &str = "softness";
pub const LIGHT_ANGLE: &str = "light_angle";
pub const ZOOM: &str = "zoom";
pub const PAN_X: &str = "pan_x";
pub const PAN_Y: &str = "pan_y";
pub const MOTION: &str = "motion";

/// Variables every style shares: tone, color, finish, framing and motion.
pub const COMMON_PARAMS: &[ParamSpec] = &[
    spec(EXPOSURE, tk("Exposure"), -2.0, 2.0, 0.05, 0.0, "EV", (-0.25, 0.25)),
    spec(CONTRAST, tk("Contrast"), 50.0, 150.0, 1.0, 100.0, "%", (92.0, 112.0)),
    spec(SATURATION, tk("Saturation"), 0.0, 200.0, 1.0, 100.0, "%", (90.0, 120.0)),
    spec(HUE_SHIFT, tk("Hue shift"), -180.0, 180.0, 1.0, 0.0, "°", (-10.0, 10.0)),
    spec(TEMPERATURE, tk("Temperature"), -100.0, 100.0, 1.0, 0.0, "", (-12.0, 12.0)),
    pct(VIGNETTE, tk("Vignette"), 35.0, (20.0, 50.0)),
    pct(GRAIN, tk("Film grain"), 0.0, (0.0, 0.0)),
    pct(DITHER, tk("Anti-banding dither"), 100.0, (100.0, 100.0)),
    pct(SOFTNESS, tk("Softness"), 30.0, (15.0, 50.0)),
    spec(LIGHT_ANGLE, tk("Light angle"), 0.0, 360.0, 1.0, 135.0, "°", (100.0, 160.0)),
    spec(ZOOM, tk("Scale"), 50.0, 200.0, 1.0, 100.0, "%", (90.0, 120.0)),
    spec(PAN_X, tk("Horizontal offset"), -50.0, 50.0, 1.0, 0.0, "%", (-8.0, 8.0)),
    spec(PAN_Y, tk("Vertical offset"), -50.0, 50.0, 1.0, 0.0, "%", (-4.0, 4.0)),
    spec(MOTION, tk("Motion"), 0.0, 200.0, 1.0, 100.0, "%", (80.0, 120.0)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_snaps_counts_and_rejects_nan() {
        let c = count("n", "N", 2.0, 8.0, 5.0, (3.0, 6.0));
        assert_eq!(c.clamp(9.7), 8.0);
        assert_eq!(c.clamp(3.4), 3.0);
        assert_eq!(c.clamp(f32::NAN), 5.0);
        assert!(c.is_count());
        let p = pct("p", "P", 50.0, (20.0, 80.0));
        assert_eq!(p.clamp(-3.0), 0.0);
        assert!(!p.is_count());
    }
}
