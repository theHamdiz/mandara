//! Analytical damped harmonic oscillator.
//!
//! Each component of a [`SpringValue`] is advanced with the closed-form
//! solution of `m·x'' + c·x' + k·x = 0`, so the result is exact for any frame
//! time (no integration drift at 60 Hz vs 144 Hz) and retargeting mid-flight
//! simply keeps the current position and velocity — interrupted animations
//! inherit their momentum seamlessly.

use gpui::{Bounds, Pixels, Point, Size, point, px, size};
use mandara_theme::Color;
use mandara_theme::color::shortest_hue_delta;
use palette::Oklch;
use serde::{Deserialize, Serialize};

/// Physical parameters of a spring.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpringConfig {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

/// Default spring stiffness `k` (N/m-equivalent; CLAUDE.md design token).
pub const DEFAULT_STIFFNESS: f32 = 280.0;
/// Default damping coefficient `c`.
pub const DEFAULT_DAMPING: f32 = 24.0;
/// Default mass `m`.
pub const DEFAULT_MASS: f32 = 1.0;
/// Lower bound for stiffness and mass so the solver never divides by zero.
const MIN_PHYSICAL: f32 = 1e-3;
/// |ζ − 1| below which the critically damped closed form is used.
const CRITICAL_TOLERANCE: f32 = 1e-4;
/// Longest single step the solver accepts, in seconds. The frame clock
/// clamps far tighter; this only guards direct callers.
const MAX_STEP_S: f32 = 0.1;
/// Bounds of the global animation speed multiplier.
pub const MIN_TIME_SCALE: f32 = 0.25;
pub const MAX_TIME_SCALE: f32 = 4.0;

/// Global animation speed (`motion.speed`), stored as `f32` bits: every
/// spring advances `dt × scale`, so 2.0 plays every animation twice as fast
/// while keeping its shape (same damping ratio, same overshoot).
static TIME_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

/// Sets the global animation speed multiplier (clamped to
/// [`MIN_TIME_SCALE`]..=[`MAX_TIME_SCALE`]).
pub fn set_time_scale(scale: f32) {
    let scale = if scale.is_finite() { scale.clamp(MIN_TIME_SCALE, MAX_TIME_SCALE) } else { 1.0 };
    TIME_SCALE.store(scale.to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// The current global animation speed multiplier.
pub fn time_scale() -> f32 {
    f32::from_bits(TIME_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

/// Bounds of the user's spring tuning (`motion.stiffness / damping / mass`).
pub const MIN_FEEL_STIFFNESS: f32 = 20.0;
pub const MAX_FEEL_STIFFNESS: f32 = 2000.0;
pub const MIN_FEEL_DAMPING: f32 = 1.0;
pub const MAX_FEEL_DAMPING: f32 = 120.0;
pub const MIN_FEEL_MASS: f32 = 0.1;
pub const MAX_FEEL_MASS: f32 = 10.0;

/// The shell-wide motion profile: the user's spring tuning and reduced
/// motion. Every [`SpringValue`] keeps its own character (a snappy hover
/// stays snappier than a gentle morph); the tuning scales it relative to the
/// default spring:
///
/// - stiffness and mass scale each spring's stiffness and mass by
///   `user / default` (higher stiffness is snappier, more mass is slower);
/// - the damping *ratio* is scaled by `user ζ / default ζ`, so lower
///   damping bounces more and higher damping settles without overshoot.
///   Springs that are critically damped or slower (exits) never gain a
///   bounce, so a closing surface can't flash back into view.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MotionProfile {
    pub feel: SpringConfig,
    /// Every spring jumps straight to its target.
    pub reduce: bool,
}

impl Default for MotionProfile {
    fn default() -> Self {
        Self { feel: SpringConfig::default(), reduce: false }
    }
}

impl MotionProfile {
    /// A profile from raw config values (non-finite values fall back to the
    /// defaults, the rest are clamped to the `*_FEEL_*` bounds).
    pub fn new(stiffness: f32, damping: f32, mass: f32, reduce: bool) -> Self {
        let clean = |v: f32, default: f32, min: f32, max: f32| if v.is_finite() { v.clamp(min, max) } else { default };
        Self {
            feel: SpringConfig::new(
                clean(stiffness, DEFAULT_STIFFNESS, MIN_FEEL_STIFFNESS, MAX_FEEL_STIFFNESS),
                clean(damping, DEFAULT_DAMPING, MIN_FEEL_DAMPING, MAX_FEEL_DAMPING),
                clean(mass, DEFAULT_MASS, MIN_FEEL_MASS, MAX_FEEL_MASS),
            ),
            reduce,
        }
    }

    /// `spring` as tuned by this profile (unchanged at the default feel).
    pub fn tune(&self, spring: SpringConfig) -> SpringConfig {
        let default = SpringConfig::default();
        if self.feel == default {
            return spring;
        }
        let base = spring.sanitized();
        let stiffness = base.stiffness * self.feel.stiffness / default.stiffness;
        let mass = base.mass * self.feel.mass / default.mass;
        let zeta = base.damping_ratio();
        let zeta = if zeta >= 1.0 { zeta } else { zeta * self.feel.damping_ratio() / default.damping_ratio() };
        SpringConfig::new(stiffness, 2.0 * zeta * (stiffness * mass).sqrt(), mass)
    }
}

/// The global profile, as `f32` bits: stiffness, damping, mass, reduce.
static FEEL: [std::sync::atomic::AtomicU32; 3] = [
    std::sync::atomic::AtomicU32::new(DEFAULT_STIFFNESS.to_bits()),
    std::sync::atomic::AtomicU32::new(DEFAULT_DAMPING.to_bits()),
    std::sync::atomic::AtomicU32::new(DEFAULT_MASS.to_bits()),
];
static REDUCE_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets the shell-wide motion profile (`[motion]`).
pub fn set_motion_profile(profile: MotionProfile) {
    use std::sync::atomic::Ordering::Relaxed;
    let p = MotionProfile::new(profile.feel.stiffness, profile.feel.damping, profile.feel.mass, profile.reduce);
    FEEL[0].store(p.feel.stiffness.to_bits(), Relaxed);
    FEEL[1].store(p.feel.damping.to_bits(), Relaxed);
    FEEL[2].store(p.feel.mass.to_bits(), Relaxed);
    REDUCE_MOTION.store(p.reduce, Relaxed);
}

/// The current shell-wide motion profile.
pub fn motion_profile() -> MotionProfile {
    use std::sync::atomic::Ordering::Relaxed;
    let f = |i: usize| f32::from_bits(FEEL[i].load(Relaxed));
    MotionProfile { feel: SpringConfig::new(f(0), f(1), f(2)), reduce: REDUCE_MOTION.load(Relaxed) }
}
/// OKLCH chroma below which a colour counts as achromatic (its hue is
/// meaningless and must not sweep the wheel).
const ACHROMATIC_CHROMA: f32 = 0.0025;
/// A full turn of hue, in degrees.
const HUE_TURN_DEG: f32 = 360.0;

impl Default for SpringConfig {
    fn default() -> Self {
        Self { stiffness: DEFAULT_STIFFNESS, damping: DEFAULT_DAMPING, mass: DEFAULT_MASS }
    }
}

impl SpringConfig {
    pub const fn new(stiffness: f32, damping: f32, mass: f32) -> Self {
        Self { stiffness, damping, mass }
    }

    /// A softer spring for large geometry morphs.
    pub const GENTLE: SpringConfig = SpringConfig::new(170.0, 22.0, DEFAULT_MASS);
    /// A snappy spring for hover feedback.
    pub const SNAPPY: SpringConfig = SpringConfig::new(520.0, 38.0, DEFAULT_MASS);

    /// Damping ratio ζ (`< 1` overshoots, `1` is critical, `> 1` is sluggish).
    /// Critically damped (ζ = 1) at the given stiffness: reaches its target
    /// without overshoot — for exits, where a bounce past zero would flash
    /// the closing surface back into view.
    pub fn critical(stiffness: f32) -> Self {
        Self::new(stiffness, 2.0 * stiffness.sqrt(), DEFAULT_MASS)
    }

    pub fn damping_ratio(&self) -> f32 {
        self.damping / (2.0 * (self.stiffness * self.mass).sqrt())
    }

    /// Undamped angular frequency ω₀.
    pub fn natural_frequency(&self) -> f32 {
        (self.stiffness / self.mass).sqrt()
    }

    fn sanitized(self) -> Self {
        Self {
            stiffness: self.stiffness.max(MIN_PHYSICAL),
            damping: self.damping.max(0.0),
            mass: self.mass.max(MIN_PHYSICAL),
        }
    }

    /// Advances one component: given displacement `x0` from the target and
    /// velocity `v0`, returns `(x, v)` after `t` seconds.
    pub fn solve(&self, x0: f32, v0: f32, t: f32) -> (f32, f32) {
        let cfg = self.sanitized();
        let w0 = cfg.natural_frequency();
        let zeta = cfg.damping_ratio();
        if (zeta - 1.0).abs() < CRITICAL_TOLERANCE {
            // Critically damped: x = (A + B t) e^{-ω t}
            let b = v0 + w0 * x0;
            let e = (-w0 * t).exp();
            let x = (x0 + b * t) * e;
            let v = (b - w0 * (x0 + b * t)) * e;
            (x, v)
        } else if zeta < 1.0 {
            // Underdamped: decaying sinusoid.
            let wd = w0 * (1.0 - zeta * zeta).sqrt();
            let a = x0;
            let b = (v0 + zeta * w0 * x0) / wd;
            let e = (-zeta * w0 * t).exp();
            let (s, c) = (wd * t).sin_cos();
            let x = e * (a * c + b * s);
            let v = e * ((b * wd - zeta * w0 * a) * c - (a * wd + zeta * w0 * b) * s);
            (x, v)
        } else {
            // Overdamped: sum of two decaying exponentials.
            let root = (zeta * zeta - 1.0).sqrt();
            let r1 = -w0 * (zeta - root);
            let r2 = -w0 * (zeta + root);
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
            (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
        }
    }
}

/// Values that can be animated by a spring: decomposed into up to four
/// independent scalar channels.
pub trait Springable: Copy {
    /// Number of meaningful channels (`1..=4`).
    const CHANNELS: usize;
    /// Displacement below which a channel counts as at rest.
    const REST_EPSILON: f32;

    /// `(chroma_channel, hue_channel)` for polar color types whose hue must
    /// travel along the shortest arc.
    const POLAR_HUE: Option<(usize, usize)> = None;

    fn to_channels(&self) -> [f32; 4];
    fn from_channels(channels: [f32; 4]) -> Self;
}

impl Springable for f32 {
    const CHANNELS: usize = 1;
    const REST_EPSILON: f32 = 1e-3;
    fn to_channels(&self) -> [f32; 4] {
        [*self, 0.0, 0.0, 0.0]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        c[0]
    }
}

impl Springable for Pixels {
    const CHANNELS: usize = 1;
    const REST_EPSILON: f32 = 0.05;
    fn to_channels(&self) -> [f32; 4] {
        [f32::from(*self), 0.0, 0.0, 0.0]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        px(c[0])
    }
}

impl Springable for Point<Pixels> {
    const CHANNELS: usize = 2;
    const REST_EPSILON: f32 = 0.05;
    fn to_channels(&self) -> [f32; 4] {
        [f32::from(self.x), f32::from(self.y), 0.0, 0.0]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        point(px(c[0]), px(c[1]))
    }
}

impl Springable for Size<Pixels> {
    const CHANNELS: usize = 2;
    const REST_EPSILON: f32 = 0.05;
    fn to_channels(&self) -> [f32; 4] {
        [f32::from(self.width), f32::from(self.height), 0.0, 0.0]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        size(px(c[0].max(0.0)), px(c[1].max(0.0)))
    }
}

impl Springable for Bounds<Pixels> {
    const CHANNELS: usize = 4;
    // Sub-quarter-pixel motion is invisible; settle there instead of
    // spending extra frames on an imperceptible tail.
    const REST_EPSILON: f32 = 0.25;
    fn to_channels(&self) -> [f32; 4] {
        [
            f32::from(self.origin.x),
            f32::from(self.origin.y),
            f32::from(self.size.width),
            f32::from(self.size.height),
        ]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        Bounds { origin: point(px(c[0]), px(c[1])), size: size(px(c[2].max(0.0)), px(c[3].max(0.0))) }
    }
}

/// Hue channels are stored unwrapped (may exceed 360°) so the spring can travel
/// continuously; the target is placed on the nearest equivalent angle. When
/// either end is achromatic its hue is borrowed from the other end so a fade
/// out of gray never sweeps the colour wheel.
fn prepare_hue(current: &[f32; 4], target: &mut [f32; 4], chroma: usize, hue: usize) {
    if target[chroma] < ACHROMATIC_CHROMA {
        target[hue] = current[hue];
    } else if current[chroma] < ACHROMATIC_CHROMA {
        // Current hue is meaningless; `set_target` snaps it onto the target.
        target[hue] = target[hue].rem_euclid(HUE_TURN_DEG);
    } else {
        target[hue] = current[hue] + shortest_hue_delta(current[hue].rem_euclid(HUE_TURN_DEG), target[hue]);
    }
}

impl Springable for Oklch<f32> {
    const CHANNELS: usize = 3;
    const REST_EPSILON: f32 = 5e-4;
    const POLAR_HUE: Option<(usize, usize)> = Some((1, 2));
    fn to_channels(&self) -> [f32; 4] {
        [self.l, self.chroma, self.hue.into_positive_degrees(), 0.0]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        Oklch::new(c[0].clamp(0.0, 1.0), c[1].max(0.0), c[2].rem_euclid(HUE_TURN_DEG))
    }
}

impl Springable for Color {
    const CHANNELS: usize = 4;
    const REST_EPSILON: f32 = 5e-4;
    const POLAR_HUE: Option<(usize, usize)> = Some((1, 2));
    fn to_channels(&self) -> [f32; 4] {
        [self.lightness(), self.chroma(), self.hue(), self.alpha()]
    }
    fn from_channels(c: [f32; 4]) -> Self {
        Color::oklcha(c[0], c[1], c[2], c[3])
    }
}

/// A value animated toward a target by a damped spring.
#[derive(Clone, Debug)]
pub struct SpringValue<T: Springable> {
    position: [f32; 4],
    velocity: [f32; 4],
    target: [f32; 4],
    config: SpringConfig,
    /// Displacement below which a channel counts as at rest (defaults to
    /// [`Springable::REST_EPSILON`]; see [`SpringValue::with_rest_epsilon`]).
    rest_epsilon: f32,
    /// When set, targets are applied instantly (reduced motion).
    instant: bool,
    _marker: std::marker::PhantomData<T>,
}

/// Velocity below which a channel counts as at rest, as a multiple of the
/// rest epsilon (units / second).
pub const REST_VELOCITY_FACTOR: f32 = 10.0;

impl<T: Springable> SpringValue<T> {
    pub fn new(value: T, config: SpringConfig) -> Self {
        let c = value.to_channels();
        Self { position: c, velocity: [0.0; 4], target: c, config, rest_epsilon: T::REST_EPSILON, instant: false, _marker: Default::default() }
    }

    /// Settles once every channel is within `epsilon` of its target (and
    /// its velocity below `epsilon ×` [`REST_VELOCITY_FACTOR`]). Use the
    /// value's *visible* resolution — e.g. a 0..1 fraction drawn across 40 px
    /// is invisible below `0.25 / 40` — so a spring stops requesting frames
    /// as soon as further motion can't be seen.
    pub fn with_rest_epsilon(mut self, epsilon: f32) -> Self {
        self.rest_epsilon = epsilon.max(f32::EPSILON);
        self
    }

    pub fn config(&self) -> SpringConfig {
        self.config
    }

    pub fn set_config(&mut self, config: SpringConfig) {
        self.config = config;
    }

    /// Makes future targets apply instantly (accessibility: reduced motion).
    pub fn set_instant(&mut self, instant: bool) {
        self.instant = instant;
        if instant {
            self.snap_to_target();
        }
    }

    /// Current animated value.
    pub fn value(&self) -> T {
        T::from_channels(self.position)
    }

    pub fn target(&self) -> T {
        T::from_channels(self.target)
    }

    pub fn velocity(&self) -> [f32; 4] {
        self.velocity
    }

    /// Retargets the spring, keeping position and velocity. Returns `true`
    /// if the target actually changed.
    pub fn set_target(&mut self, target: T) -> bool {
        let mut t = target.to_channels();
        if let Some((chroma, hue)) = T::POLAR_HUE {
            prepare_hue(&self.position, &mut t, chroma, hue);
            // Snap a meaningless (achromatic) current hue onto the target hue.
            if self.position[chroma] < ACHROMATIC_CHROMA && t[chroma] >= ACHROMATIC_CHROMA {
                self.position[hue] = t[hue];
            }
        }
        let changed = (0..T::CHANNELS).any(|i| (t[i] - self.target[i]).abs() > f32::EPSILON);
        self.target = t;
        if self.instant || motion_profile().reduce {
            self.snap_to_target();
        }
        changed
    }

    /// Jumps to `value` with zero velocity.
    pub fn snap_to(&mut self, value: T) {
        let c = value.to_channels();
        self.position = c;
        self.target = c;
        self.velocity = [0.0; 4];
    }

    pub fn snap_to_target(&mut self) {
        self.position = self.target;
        self.velocity = [0.0; 4];
    }

    /// Adds an impulse (units / second) to every channel — e.g. a flick.
    pub fn add_velocity(&mut self, impulse: [f32; 4]) {
        for (v, i) in self.velocity.iter_mut().zip(impulse) {
            *v += i;
        }
    }

    /// `true` while any channel is displaced or moving.
    pub fn is_animating(&self) -> bool {
        let rest_v = self.rest_epsilon * REST_VELOCITY_FACTOR;
        (0..T::CHANNELS).any(|i| {
            (self.position[i] - self.target[i]).abs() > self.rest_epsilon || self.velocity[i].abs() > rest_v
        })
    }

    /// Advances the spring by `dt` seconds. Returns `true` if still animating
    /// (the caller should request another frame).
    pub fn step(&mut self, dt: f32) -> bool {
        self.step_in(dt, &motion_profile())
    }

    /// [`SpringValue::step`] under an explicit motion profile.
    pub fn step_in(&mut self, dt: f32, profile: &MotionProfile) -> bool {
        if profile.reduce {
            if self.position != self.target || self.velocity != [0.0; 4] {
                self.snap_to_target();
            }
            return false;
        }
        if !self.is_animating() {
            // Already at rest: only touch memory when not exactly settled.
            if self.position != self.target || self.velocity != [0.0; 4] {
                self.snap_to_target();
            }
            return false;
        }
        let dt = (dt * time_scale()).clamp(0.0, MAX_STEP_S);
        let config = profile.tune(self.config);
        for i in 0..T::CHANNELS {
            let x0 = self.position[i] - self.target[i];
            let (x, v) = config.solve(x0, self.velocity[i], dt);
            self.position[i] = self.target[i] + x;
            self.velocity[i] = v;
        }
        if self.is_animating() {
            true
        } else {
            self.snap_to_target();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    fn run<T: Springable>(s: &mut SpringValue<T>, hz: f32, seconds: f32) -> usize {
        let dt = 1.0 / hz;
        let mut frames = 0;
        while s.step(dt) {
            frames += 1;
            assert!(frames as f32 * dt < seconds, "spring did not settle within {seconds}s");
        }
        frames
    }

    #[test]
    fn default_spring_is_slightly_underdamped() {
        let cfg = SpringConfig::default();
        assert!(cfg.damping_ratio() < 1.0 && cfg.damping_ratio() > 0.6);
    }

    #[test]
    fn analytic_solution_matches_fine_integration() {
        // Compare against a tiny-step semi-implicit Euler integration.
        for cfg in [SpringConfig::default(), SpringConfig::new(100.0, 20.0, 1.0), SpringConfig::new(100.0, 60.0, 1.0)] {
            let (mut x, mut v) = (1.0_f32, 0.0_f32);
            let h = 1e-5;
            for _ in 0..(0.2 / h) as usize {
                let a = (-cfg.stiffness * x - cfg.damping * v) / cfg.mass;
                v += a * h;
                x += v * h;
            }
            let (ax, av) = cfg.solve(1.0, 0.0, 0.2);
            assert_abs_diff_eq!(ax, x, epsilon = 2e-3);
            assert_abs_diff_eq!(av, v, epsilon = 2e-2);
        }
    }

    #[test]
    fn frame_rate_independent() {
        let mut a = SpringValue::new(0.0_f32, SpringConfig::default());
        let mut b = a.clone();
        a.set_target(100.0);
        b.set_target(100.0);
        for _ in 0..60 {
            a.step(1.0 / 60.0);
        }
        for _ in 0..144 {
            b.step(1.0 / 144.0);
        }
        assert_abs_diff_eq!(a.value(), b.value(), epsilon = 1e-2);
    }

    #[test]
    fn settles_on_target() {
        let mut s = SpringValue::new(Bounds::<Pixels>::default(), SpringConfig::default());
        let target = Bounds { origin: point(px(10.0), px(8.0)), size: size(px(420.0), px(520.0)) };
        s.set_target(target);
        run(&mut s, 144.0, 3.0);
        assert_eq!(s.value(), target);
        assert!(!s.is_animating());
    }

    #[test]
    fn retarget_preserves_velocity() {
        let mut s = SpringValue::new(0.0_f32, SpringConfig::default());
        s.set_target(100.0);
        for _ in 0..10 {
            s.step(1.0 / 144.0);
        }
        let (pos, vel) = (s.value(), s.velocity()[0]);
        assert!(vel > 0.0);
        s.set_target(-50.0);
        assert_eq!(s.value(), pos);
        assert_eq!(s.velocity()[0], vel, "interruption must keep momentum");
    }

    #[test]
    fn color_hue_takes_shortest_arc() {
        let mut s = SpringValue::new(Color::oklch(0.7, 0.12, 350.0), SpringConfig::default());
        s.set_target(Color::oklch(0.7, 0.12, 20.0));
        let mut max_detour: f32 = 0.0;
        while s.step(1.0 / 144.0) {
            let h = s.value().hue();
            // Valid path stays within [350, 360) ∪ [0, 20] plus overshoot.
            let dist = shortest_hue_delta(5.0, h).abs();
            max_detour = max_detour.max(dist);
        }
        assert!(max_detour < 30.0, "hue wandered {max_detour}° from the short arc");
        assert_abs_diff_eq!(s.value().hue(), 20.0, epsilon = 0.1);
    }

    #[test]
    fn coarse_rest_epsilon_settles_sooner() {
        let mut fine = SpringValue::new(0.0_f32, SpringConfig::default());
        let mut coarse = fine.clone().with_rest_epsilon(1e-2);
        fine.set_target(1.0);
        coarse.set_target(1.0);
        let (f, c) = (run(&mut fine, 144.0, 3.0), run(&mut coarse, 144.0, 3.0));
        assert!(c < f, "coarse {c} frames vs fine {f}");
        assert_eq!(coarse.value(), 1.0);
    }

    /// Frames until a 0 → 1 move settles under `profile`.
    fn settle_frames(spring: SpringConfig, profile: &MotionProfile) -> usize {
        let mut s = SpringValue::new(0.0_f32, spring);
        s.set_target(1.0);
        let mut frames = 0;
        while s.step_in(1.0 / 144.0, profile) {
            frames += 1;
            assert!(frames < 144 * 10, "never settled under {profile:?}");
        }
        assert_eq!(s.value(), 1.0);
        frames
    }

    /// Largest value reached on a 0 → 1 move (> 1 = overshoot).
    fn peak(spring: SpringConfig, profile: &MotionProfile) -> f32 {
        let mut s = SpringValue::new(0.0_f32, spring);
        s.set_target(1.0);
        let mut max: f32 = 0.0;
        while s.step_in(1.0 / 144.0, profile) {
            max = max.max(s.value());
        }
        max
    }

    #[test]
    fn default_profile_leaves_every_spring_untouched() {
        let p = MotionProfile::default();
        for spring in [SpringConfig::default(), SpringConfig::SNAPPY, SpringConfig::GENTLE, SpringConfig::critical(420.0)] {
            assert_eq!(p.tune(spring), spring);
        }
        // Config values equal to the defaults are the identity too.
        assert_eq!(MotionProfile::new(DEFAULT_STIFFNESS, DEFAULT_DAMPING, DEFAULT_MASS, false), p);
    }

    #[test]
    fn stiffness_is_snappier_and_mass_is_slower() {
        let base = MotionProfile::default();
        let stiff = MotionProfile::new(DEFAULT_STIFFNESS * 3.0, DEFAULT_DAMPING * 3.0_f32.sqrt(), DEFAULT_MASS, false);
        let heavy = MotionProfile::new(DEFAULT_STIFFNESS, DEFAULT_DAMPING * 2.0_f32.sqrt(), DEFAULT_MASS * 2.0, false);
        for spring in [SpringConfig::default(), SpringConfig::SNAPPY, SpringConfig::GENTLE] {
            let n = settle_frames(spring, &base);
            assert!(settle_frames(spring, &stiff) < n, "{spring:?}: stiffer must settle sooner");
            assert!(settle_frames(spring, &heavy) > n, "{spring:?}: heavier must settle later");
            // Same damping ratio: the shape (overshoot) is kept.
            assert!((stiff.tune(spring).damping_ratio() - spring.damping_ratio()).abs() < 1e-3);
            assert!((heavy.tune(spring).damping_ratio() - spring.damping_ratio()).abs() < 1e-3);
        }
    }

    #[test]
    fn damping_controls_bounce_but_never_makes_exits_bounce() {
        let bouncy = MotionProfile::new(DEFAULT_STIFFNESS, DEFAULT_DAMPING * 0.5, DEFAULT_MASS, false);
        let calm = MotionProfile::new(DEFAULT_STIFFNESS, DEFAULT_DAMPING * 1.6, DEFAULT_MASS, false);
        let base = MotionProfile::default();
        let spring = SpringConfig::default();
        assert!(peak(spring, &bouncy) > peak(spring, &base), "less damping overshoots more");
        assert!(peak(spring, &calm) <= 1.0 + 1e-3, "damping past critical does not overshoot");
        let exit = SpringConfig::critical(420.0);
        assert!(bouncy.tune(exit).damping_ratio() >= 1.0 - 1e-3, "an exit spring stays critically damped");
        assert!(peak(exit, &bouncy) <= 1.0 + 1e-3);
    }

    #[test]
    fn profile_values_are_sanitized() {
        let p = MotionProfile::new(f32::NAN, 1.0e6, -3.0, true);
        assert_eq!(p.feel.stiffness, DEFAULT_STIFFNESS);
        assert_eq!(p.feel.damping, MAX_FEEL_DAMPING);
        assert_eq!(p.feel.mass, MIN_FEEL_MASS);
        assert!(p.reduce);
    }

    #[test]
    fn reduced_motion_profile_jumps_to_the_target() {
        let reduced = MotionProfile { reduce: true, ..Default::default() };
        let mut s = SpringValue::new(Color::oklch(0.5, 0.1, 30.0), SpringConfig::GENTLE);
        let target = Color::oklch(0.8, 0.12, 200.0);
        s.set_target(target);
        assert!(!s.step_in(1.0 / 144.0, &reduced), "no further frames are requested");
        assert!(!s.is_animating());
        assert_eq!(s.value().to_hex(), target.to_hex());
    }

    #[test]
    fn instant_mode_snaps() {
        let mut s = SpringValue::new(px(0.0), SpringConfig::default());
        s.set_instant(true);
        s.set_target(px(42.0));
        assert_eq!(s.value(), px(42.0));
        assert!(!s.step(1.0 / 144.0));
    }
}
