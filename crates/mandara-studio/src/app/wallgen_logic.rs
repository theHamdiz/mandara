//! The generator panel's pure logic (Studio → Wallpaper → Generator):
//! slider ↔ parameter mapping and value labels, the parameter groups, the
//! preview queue (debounced while sliders move, one render in flight,
//! the main preview first) and file size estimates. No GUI types.

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use mandara_config::wallgen::WallgenFormat;
use mandara_i18n::tk;
use mandara_wallgen::{COMMON_PARAMS, ParamSpec, Recipe};

/// The large preview waits for this much quiet after the last change…
pub const PREVIEW_QUIET: Duration = Duration::from_millis(40);
/// …but never longer than this after the first unrendered change, so it
/// keeps up while a slider is dragged.
pub const PREVIEW_MAX_WAIT: Duration = Duration::from_millis(140);

/// The common parameters by group: tone, light, texture, layout. A key
/// missing here (a newer engine) lands in [`MORE_GROUP`].
pub const COMMON_GROUPS: &[(&str, &[&str])] = &[
    (tk("Tone"), &["exposure", "contrast", "saturation", "hue_shift", "temperature"]),
    (tk("Light"), &["light_angle", "vignette"]),
    (tk("Texture"), &["grain", "dither", "softness"]),
    (tk("Layout"), &["zoom", "pan_x", "pan_y", "motion"]),
];
pub const MORE_GROUP: &str = tk("More");

/// The common parameters of a group (and the ungrouped ones for
/// [`MORE_GROUP`]), in engine order.
pub fn common_group(title: &str) -> Vec<&'static ParamSpec> {
    if title == MORE_GROUP {
        return COMMON_PARAMS.iter().filter(|s| !COMMON_GROUPS.iter().any(|(_, keys)| keys.contains(&s.key))).collect();
    }
    let keys = COMMON_GROUPS.iter().find(|(t, _)| *t == title).map_or(&[][..], |(_, k)| *k);
    keys.iter().filter_map(|k| COMMON_PARAMS.iter().find(|s| s.key == *k)).collect()
}

/// A slider position (0–1) for `value`.
pub fn to_fraction(spec: &ParamSpec, value: f32) -> f32 {
    let span = spec.max - spec.min;
    if span <= 0.0 { 0.0 } else { ((spec.clamp(value) - spec.min) / span).clamp(0.0, 1.0) }
}

/// The value at slider position `fraction`, snapped to the step.
pub fn from_fraction(spec: &ParamSpec, fraction: f32) -> f32 {
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let raw = spec.min + (spec.max - spec.min) * f;
    let snapped = if spec.step > 0.0 { spec.min + ((raw - spec.min) / spec.step).round() * spec.step } else { raw };
    spec.clamp(snapped)
}

/// Decimal places a step needs (1 → 0, 0.05 → 2).
fn decimals(step: f32) -> usize {
    const MAX_DECIMALS: usize = 3;
    (0..MAX_DECIMALS).find(|d| (step * 10f32.powi(*d as i32)).fract().abs() < 1e-3).unwrap_or(MAX_DECIMALS)
}

/// A value for its label: `35%`, `135°`, `+0.25 EV`, `5`, `-12`.
pub fn value_label(spec: &ParamSpec, value: f32) -> String {
    let d = decimals(spec.step);
    // Signed for ranges around zero (offsets, exposure, temperature).
    let signed = spec.min < 0.0 && value > 0.0;
    let number = format!("{}{value:.d$}", if signed { "+" } else { "" });
    match spec.unit {
        "" => number,
        "%" | "°" => format!("{number}{}", spec.unit),
        unit => format!("{number} {unit}"),
    }
}

/// Whether every parameter of `specs` sits at its default in `recipe`.
pub fn at_defaults(recipe: &Recipe, specs: &[&ParamSpec]) -> bool {
    specs.iter().all(|s| (recipe.param(s.key) - s.default).abs() < f32::EPSILON)
}

/// `recipe` with `specs` back at their defaults.
pub fn reset(recipe: &Recipe, specs: &[&ParamSpec]) -> Recipe {
    let mut r = recipe.clone();
    for s in specs {
        r.params.remove(s.key);
    }
    r
}

/// A stable key for a recipe rendered at a size.
pub fn key(recipe: &Recipe, w: u32, h: u32) -> u64 {
    let mut h64 = std::collections::hash_map::DefaultHasher::new();
    recipe.to_toml().hash(&mut h64);
    (w, h).hash(&mut h64);
    h64.finish()
}

/// The large preview's schedule: renders are coalesced to the newest
/// recipe, one at a time, after a short quiet (bounded while dragging).
#[derive(Clone, Debug, Default)]
pub struct PreviewQueue {
    /// The newest recipe asked for, and when it first / last changed.
    wanted: Option<u64>,
    first_change: Option<Instant>,
    last_change: Option<Instant>,
    in_flight: Option<u64>,
    shown: Option<u64>,
}

/// What the preview queue wants now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Nothing to do (up to date, or a render is running).
    Idle,
    /// Check again after this long.
    Wait(Duration),
    /// Render the wanted key now.
    Render(u64),
}

impl PreviewQueue {
    /// The recipe on screen changed to `key` (no-op when it didn't).
    pub fn want(&mut self, key: u64, now: Instant) {
        if self.wanted == Some(key) {
            return;
        }
        // The first change since the screen (or the running render) was
        // current starts the max-wait clock.
        let was_current = self.wanted == self.shown || self.wanted == self.in_flight;
        self.wanted = Some(key);
        self.last_change = Some(now);
        if was_current || self.first_change.is_none() {
            self.first_change = Some(now);
        }
    }

    pub fn poll(&mut self, now: Instant) -> Next {
        let Some(wanted) = self.wanted else { return Next::Idle };
        if self.in_flight.is_some() || self.shown == Some(wanted) {
            return Next::Idle;
        }
        // Nothing on screen yet: render at once.
        if self.shown.is_none() {
            self.in_flight = Some(wanted);
            return Next::Render(wanted);
        }
        let quiet = self.last_change.map_or(Duration::MAX, |t| now.duration_since(t));
        let waited = self.first_change.map_or(Duration::MAX, |t| now.duration_since(t));
        if quiet >= PREVIEW_QUIET || waited >= PREVIEW_MAX_WAIT {
            self.in_flight = Some(wanted);
            return Next::Render(wanted);
        }
        Next::Wait((PREVIEW_QUIET - quiet).min(PREVIEW_MAX_WAIT - waited))
    }

    /// The render of `key` finished (shown at once; a newer one queues).
    pub fn done(&mut self, key: u64, now: Instant) {
        self.in_flight = None;
        self.shown = Some(key);
        self.first_change = (self.wanted != Some(key)).then_some(now);
    }

    pub fn shown(&self) -> Option<u64> {
        self.shown
    }

    /// Whether the newest recipe still waits for its render.
    pub fn stale(&self) -> bool {
        self.wanted.is_some() && self.wanted != self.shown
    }
}

/// How bytes grow with pixel count: `bytes ∝ pixels^exponent`, the mean
/// measured over the twelve styles (random recipes) between 640 × 360 and
/// 3840 × 2160: smooth gradients compress better the more pixels share them.
const WEBP_EXPONENT: f64 = 0.87;
const PNG_EXPONENT: f64 = 0.83;
const JPEG_EXPONENT: f64 = 0.77;

/// The expected file size at `target_px` pixels, from `sample_bytes` for
/// the same picture encoded at `sample_px`.
pub fn estimate_bytes(format: WallgenFormat, sample_bytes: u64, sample_px: u64, target_px: u64) -> u64 {
    if sample_px == 0 {
        return 0;
    }
    let exponent = match format {
        // A sample of the smaller exact file grows like the slower codec.
        WallgenFormat::Lossless => WEBP_EXPONENT.min(PNG_EXPONENT),
        WallgenFormat::Webp => WEBP_EXPONENT,
        WallgenFormat::Png => PNG_EXPONENT,
        WallgenFormat::Jpeg => JPEG_EXPONENT,
    };
    let scale = (target_px as f64 / sample_px as f64).powf(exponent);
    (sample_bytes as f64 * scale).round() as u64
}

/// The expected size of a [`WallgenFormat::Lossless`] file: the smaller
/// of the WebP and PNG estimates (which codec wins can change with size).
pub fn estimate_smallest(webp_bytes: u64, png_bytes: u64, sample_px: u64, target_px: u64) -> u64 {
    estimate_bytes(WallgenFormat::Webp, webp_bytes, sample_px, target_px).min(estimate_bytes(WallgenFormat::Png, png_bytes, sample_px, target_px))
}

/// Common schedule intervals offered as chips, minutes.
pub const EVERY_CHOICES: [u32; 7] = [30, 60, 180, 360, 720, 1440, 10080];
/// Video loop lengths (seconds) and frame rates offered as chips.
pub const SECONDS_CHOICES: [f32; 4] = [5.0, 10.0, 20.0, 30.0];
pub const FPS_CHOICES: [u32; 3] = [24, 30, 60];
/// "Keep the newest N" choices (0 keeps all).
pub const KEEP_CHOICES: [u32; 5] = [10, 30, 100, 500, 0];

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_wallgen::Style;

    fn spec(key: &str) -> &'static ParamSpec {
        Style::Mesh.spec(key).unwrap()
    }

    #[test]
    fn every_common_param_is_in_exactly_one_group() {
        let mut seen = std::collections::BTreeSet::new();
        for (title, _) in COMMON_GROUPS {
            for s in common_group(title) {
                assert!(seen.insert(s.key), "{} twice", s.key);
            }
        }
        assert!(common_group(MORE_GROUP).is_empty(), "ungrouped: {:?}", common_group(MORE_GROUP).iter().map(|s| s.key).collect::<Vec<_>>());
        assert_eq!(seen.len(), COMMON_PARAMS.len());
    }

    #[test]
    fn sliders_map_and_snap() {
        let vignette = spec("vignette");
        assert_eq!(from_fraction(vignette, 0.0), 0.0);
        assert_eq!(from_fraction(vignette, 1.0), 100.0);
        assert_eq!(from_fraction(vignette, 0.354), 35.0, "snapped to whole percents");
        assert_eq!(to_fraction(vignette, 25.0), 0.25);
        assert_eq!(to_fraction(vignette, 500.0), 1.0, "clamped");
        let exposure = spec("exposure");
        assert!((from_fraction(exposure, 0.5)).abs() < 1e-4);
        assert!((from_fraction(exposure, 0.53) - 0.10).abs() < 1e-4, "{}", from_fraction(exposure, 0.53));
        assert_eq!(from_fraction(exposure, f32::NAN), exposure.min);
        for s in Style::ALL.iter().flat_map(|s| s.params()).chain(COMMON_PARAMS) {
            for f in [0.0, 0.17, 0.5, 0.91, 1.0] {
                let v = from_fraction(s, f);
                assert!(v >= s.min && v <= s.max, "{} {v}", s.key);
                // Round trip within one step.
                assert!((from_fraction(s, to_fraction(s, v)) - v).abs() <= s.step + 1e-4, "{}", s.key);
            }
        }
    }

    #[test]
    fn labels_carry_units() {
        assert_eq!(value_label(spec("vignette"), 35.0), "35%");
        assert_eq!(value_label(spec("light_angle"), 135.0), "135°");
        assert_eq!(value_label(spec("exposure"), 0.25), "+0.25 EV");
        assert_eq!(value_label(spec("exposure"), -0.5), "-0.50 EV");
        assert_eq!(value_label(spec("temperature"), -12.0), "-12");
        assert_eq!(value_label(spec("pan_x"), 8.0), "+8%");
        assert_eq!(value_label(spec("points"), 5.0), "5");
    }

    #[test]
    fn group_reset() {
        let mut r = Recipe::new(Style::Mesh, 1);
        let tone = common_group("Tone");
        assert!(at_defaults(&r, &tone));
        r.set_param("contrast", 120.0);
        assert!(!at_defaults(&r, &tone));
        let back = reset(&r, &tone);
        assert!(at_defaults(&back, &tone));
        assert_eq!(back.seed, r.seed);
    }

    #[test]
    fn preview_queue_debounces_and_coalesces() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut q = PreviewQueue::default();
        assert_eq!(q.poll(t0), Next::Idle);
        // The first picture renders at once.
        q.want(1, t0);
        assert_eq!(q.poll(t0), Next::Render(1));
        assert_eq!(q.poll(ms(5)), Next::Idle, "one render at a time");
        q.done(1, ms(30));
        assert!(!q.stale());
        // A slider moves: wait for a short quiet…
        q.want(2, ms(100));
        assert_eq!(q.poll(ms(110)), Next::Wait(PREVIEW_QUIET - Duration::from_millis(10)));
        assert_eq!(q.poll(ms(140)), Next::Render(2));
        q.done(2, ms(170));
        // …but a continuous drag still renders every PREVIEW_MAX_WAIT.
        let mut renders = 0;
        let mut t = 200;
        while t < 1000 {
            q.want(t, ms(t));
            if let Next::Render(k) = q.poll(ms(t)) {
                renders += 1;
                q.done(k, ms(t));
            }
            t += 10;
        }
        assert!(renders >= 4, "only {renders} renders while dragging");
        assert!(renders <= 8, "{renders} renders: not debounced");
        // A change during a render is coalesced to the newest.
        let mut q = PreviewQueue::default();
        q.want(1, t0);
        assert_eq!(q.poll(t0), Next::Render(1));
        q.want(2, ms(5));
        q.want(3, ms(6));
        q.done(1, ms(20));
        assert!(q.stale());
        assert_eq!(q.poll(ms(200)), Next::Render(3));
        // Same key again: nothing to do.
        q.done(3, ms(220));
        q.want(3, ms(300));
        assert_eq!(q.poll(ms(400)), Next::Idle);
    }

    #[test]
    fn size_estimates_scale_sublinearly() {
        let sample = 640 * 360;
        let fourk = 3840 * 2160;
        let webp = estimate_bytes(WallgenFormat::Webp, 100_000, sample, fourk);
        assert!(webp > 100_000 * 20 && webp < 100_000 * 36, "{webp}");
        let jpeg = estimate_bytes(WallgenFormat::Jpeg, 100_000, sample, fourk);
        assert!(jpeg < webp, "JPEG grows slowest");
        assert_eq!(estimate_bytes(WallgenFormat::Png, 5000, sample, sample), 5000);
        assert_eq!(estimate_bytes(WallgenFormat::Png, 5000, 0, sample), 0);
        let png = estimate_bytes(WallgenFormat::Png, 90_000, sample, fourk);
        assert_eq!(estimate_smallest(100_000, 90_000, sample, fourk), webp.min(png), "lossless: the smaller exact file");
    }

    #[test]
    fn keys_follow_the_recipe_and_size() {
        let r = Recipe::new(Style::Waves, 4);
        assert_eq!(key(&r, 10, 10), key(&r.clone(), 10, 10));
        assert_ne!(key(&r, 10, 10), key(&r, 20, 10));
        let mut other = r.clone();
        other.set_param("vignette", 1.0);
        assert_ne!(key(&r, 10, 10), key(&other, 10, 10));
    }
}
