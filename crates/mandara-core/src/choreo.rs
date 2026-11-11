//! Choreography: small motion primitives layered on a surface's own spring.
//!
//! Every function here is a pure function of a spring's current value (its
//! open progress) or velocity, so a choreography adds no timers, no state
//! and no allocations: it only moves where things are drawn while a spring
//! is already animating. At rest (progress 1, velocity 0) every primitive
//! returns the neutral value, so surfaces settle on exactly the geometry
//! they would have without it and their input regions are unchanged.
//! Reduced motion makes [`choreography`] return [`Choreography::NEUTRAL`].

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering::Relaxed};

/// Where entering content comes from.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Entrance {
    /// The surface's own path (no extra travel).
    #[default]
    Anchor = 0,
    /// From below.
    Rise = 1,
    /// From above, landing on the spring's overshoot.
    Drop = 2,
    /// From the side where reading starts.
    Sweep = 3,
    /// On a slant from the side where reading ends.
    Drift = 4,
}

impl Entrance {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Rise,
            2 => Self::Drop,
            3 => Self::Sweep,
            4 => Self::Drift,
            _ => Self::Anchor,
        }
    }
}

/// Share of a spring's progress the last item of a full cascade waits for.
const MAX_CASCADE_SPREAD: f32 = 0.6;
/// Items after this many share the last delay, so a long list still settles
/// with the spring.
const CASCADE_ITEMS: usize = 8;
/// Largest overshoot (as a share of the travel) an entrance passes its rest
/// by: a Drop's landing bounce, bounded however loose the spring.
pub const MAX_OVERSHOOT: f32 = 0.25;
/// Longest stretch, as a share of the shape's length along its travel.
pub const MAX_STRETCH: f32 = 0.35;
/// Speed (logical px / s) at which a full stretch reaches ~63 % of its
/// longest: a launcher highlight hopping one row moves at ~1–3 k px/s.
const STRETCH_SPEED_PX_S: f32 = 1800.0;
/// Slant of a Drift entrance: mostly sideways, a little from below.
const DRIFT: (f32, f32) = (0.866_025_4, 0.5);

/// The shell-wide choreography (`[motion.choreography]`, scaled by motion
/// intensity).
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct Choreography {
    pub entrance: Entrance,
    /// 0–1: how far a cascade spreads its items over the opening.
    pub stagger: f32,
    /// 0–1: how strongly moving shapes stretch along their travel.
    pub stretch: f32,
    /// 0–1: how far content trails its surface.
    pub parallax: f32,
}

fn amount(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

impl Choreography {
    /// No choreography (also what reduced motion uses).
    pub const NEUTRAL: Self = Self { entrance: Entrance::Anchor, stagger: 0.0, stretch: 0.0, parallax: 0.0 };

    /// Amounts bounded to 0–1 (invalid values count as off), each scaled by
    /// `intensity` (0–1): lower intensity is gentler, zero is none.
    pub fn new(entrance: Entrance, stagger: f32, stretch: f32, parallax: f32, intensity: f32) -> Self {
        let k = amount(intensity);
        Self { entrance, stagger: amount(stagger) * k, stretch: amount(stretch) * k, parallax: amount(parallax) * k }
    }

    /// Whether this adds nothing to any surface.
    pub fn is_neutral(&self) -> bool {
        self.entrance == Entrance::Anchor && self.stagger == 0.0 && self.stretch == 0.0 && self.parallax == 0.0
    }

    /// Unit direction from content's rest toward where it enters from.
    /// `natural` is the surface's own (Anchor) direction; `rtl` mirrors the
    /// sideways entrances.
    pub fn direction(&self, natural: (f32, f32), rtl: bool) -> (f32, f32) {
        let start = if rtl { 1.0 } else { -1.0 };
        match self.entrance {
            Entrance::Anchor => natural,
            Entrance::Rise => (0.0, 1.0),
            Entrance::Drop => (0.0, -1.0),
            Entrance::Sweep => (start, 0.0),
            Entrance::Drift => (-start * DRIFT.0, DRIFT.1),
        }
    }

    /// Item `index`'s own progress in a cascade over the opening `progress`
    /// (0 → 1, possibly overshooting). Monotonic in `progress`, never ahead
    /// of it, and exactly 1 when the opening reaches 1, so the last item
    /// lands with the surface. Overshoot past 1 passes through unchanged.
    pub fn cascade(&self, progress: f32, index: usize) -> f32 {
        if self.stagger <= 0.0 || progress >= 1.0 || !progress.is_finite() {
            return if progress.is_finite() { progress } else { 1.0 };
        }
        let delay = self.stagger * MAX_CASCADE_SPREAD * index.min(CASCADE_ITEMS) as f32 / CASCADE_ITEMS as f32;
        ((progress - delay) / (1.0 - delay)).max(0.0)
    }

    /// Extra offset (px) of entering content at `progress`: `travel` px
    /// along [`Self::direction`] at 0, none at 1. Anchor adds nothing (the
    /// surface's own motion is the entrance). Overshoot carries content
    /// past its rest by at most [`MAX_OVERSHOOT`] of the travel.
    pub fn entrance_offset(&self, progress: f32, natural: (f32, f32), rtl: bool, travel: f32) -> (f32, f32) {
        if self.entrance == Entrance::Anchor {
            return (0.0, 0.0);
        }
        self.travel_offset(progress, natural, rtl, travel)
    }

    /// [`Self::entrance_offset`] that also travels for Anchor, along the
    /// surface's own direction (items inside a surface that has no path of
    /// their own).
    pub fn travel_offset(&self, progress: f32, natural: (f32, f32), rtl: bool, travel: f32) -> (f32, f32) {
        let left = remaining(progress);
        let (dx, dy) = self.direction(natural, rtl);
        (dx * left * travel, dy * left * travel)
    }

    /// `(dx, dy, opacity)` of item `index` of a list cascading in while its
    /// surface opens over `progress`, travelling `travel` px; `None` when
    /// the choreography leaves lists alone (no stagger, Anchor entrance).
    pub fn item(&self, progress: f32, index: usize, natural: (f32, f32), rtl: bool, travel: f32) -> Option<(f32, f32, f32)> {
        if self.stagger <= 0.0 && self.entrance == Entrance::Anchor {
            return None;
        }
        let local = self.cascade(progress, index);
        let (dx, dy) = self.travel_offset(local, natural, rtl, travel);
        Some((dx, dy, cascade_opacity(local)))
    }

    /// How far content trails its surface (px) at `progress`, along
    /// [`Self::direction`]: up to `depth` px behind at 0, none at rest.
    pub fn parallax_offset(&self, progress: f32, natural: (f32, f32), rtl: bool, depth: f32) -> (f32, f32) {
        if self.parallax <= 0.0 {
            return (0.0, 0.0);
        }
        // Eased so the lag shows mid-flight and melts away at the end.
        let left = remaining(progress);
        let lag = left * left.abs() * self.parallax * depth;
        let (dx, dy) = self.direction(natural, rtl);
        (dx * lag, dy * lag)
    }

    /// Stretch (0 ≤ s ≤ [`MAX_STRETCH`]) of a shape moving at `speed`
    /// logical px / s: lengthen it by `len × s` along its travel. Zero at
    /// rest, rising smoothly with speed and saturating, so a fling never
    /// tears a shape apart.
    pub fn stretch_at(&self, speed: f32) -> f32 {
        if self.stretch <= 0.0 || !speed.is_finite() {
            return 0.0;
        }
        self.stretch * MAX_STRETCH * (1.0 - (-speed.abs() / STRETCH_SPEED_PX_S).exp())
    }

    /// A span `start..start + len` on one axis, moving at `velocity` px/s,
    /// with its liquid tail: it grows behind its direction of travel (the
    /// leading edge stays where the spring is). Returns `(start, len)`.
    pub fn stretch_span(&self, start: f32, len: f32, velocity: f32) -> (f32, f32) {
        let extra = len * self.stretch_at(velocity);
        if velocity > 0.0 { (start - extra, len + extra) } else { (start, len + extra) }
    }

    /// The width a stretched shape keeps across its travel (roughly the
    /// same area: it thins as it lengthens).
    pub fn thinned(&self, across: f32, speed: f32) -> f32 {
        across / (1.0 + self.stretch_at(speed) * 0.5)
    }
}

/// Distance still to go, bounded to the overshoot allowance.
fn remaining(progress: f32) -> f32 {
    if progress.is_finite() { (1.0 - progress).clamp(-MAX_OVERSHOOT, 1.0) } else { 0.0 }
}

/// An item's opacity for its cascade progress (fully visible from 0.7 on).
pub fn cascade_opacity(local: f32) -> f32 {
    (local / 0.7).clamp(0.0, 1.0)
}

static ENTRANCE: AtomicU8 = AtomicU8::new(0);
static AMOUNTS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

/// Sets the shell-wide choreography (from `[motion]`).
pub fn set_choreography(c: Choreography) {
    ENTRANCE.store(c.entrance as u8, Relaxed);
    for (slot, v) in AMOUNTS.iter().zip([c.stagger, c.stretch, c.parallax]) {
        slot.store(amount(v).to_bits(), Relaxed);
    }
}

/// The shell-wide choreography: neutral while motion is reduced.
pub fn choreography() -> Choreography {
    stored(crate::spring::motion_profile().reduce)
}

fn stored(reduce: bool) -> Choreography {
    if reduce {
        return Choreography::NEUTRAL;
    }
    let f = |i: usize| f32::from_bits(AMOUNTS[i].load(Relaxed));
    Choreography { entrance: Entrance::from_u8(ENTRANCE.load(Relaxed)), stagger: f(0), stretch: f(1), parallax: f(2) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spring::{SpringConfig, SpringValue};

    fn full() -> Choreography {
        Choreography::new(Entrance::Drop, 1.0, 1.0, 1.0, 1.0)
    }

    #[test]
    fn neutral_changes_nothing() {
        let n = Choreography::NEUTRAL;
        assert!(n.is_neutral());
        for p in [0.0, 0.3, 1.0, 1.2] {
            assert_eq!(n.cascade(p, 5), p);
            assert_eq!(n.entrance_offset(p, (0.0, -1.0), false, 40.0), (0.0, 0.0));
            assert_eq!(n.parallax_offset(p, (0.0, -1.0), false, 40.0), (0.0, 0.0));
        }
        assert_eq!(n.stretch_at(5000.0), 0.0);
        assert_eq!(n.stretch_span(10.0, 40.0, 3000.0), (10.0, 40.0));
        assert_eq!(n.thinned(20.0, 3000.0), 20.0);
    }

    #[test]
    fn amounts_are_bounded_and_scaled_by_intensity() {
        let c = Choreography::new(Entrance::Rise, 3.0, f32::NAN, -1.0, 1.0);
        assert_eq!((c.stagger, c.stretch, c.parallax), (1.0, 0.0, 0.0));
        let half = Choreography::new(Entrance::Rise, 0.8, 0.6, 0.4, 0.5);
        assert!((half.stagger - 0.4).abs() < 1e-6 && (half.stretch - 0.3).abs() < 1e-6 && (half.parallax - 0.2).abs() < 1e-6);
        let none = Choreography::new(Entrance::Rise, 0.8, 0.6, 0.4, 0.0);
        assert_eq!((none.stagger, none.stretch, none.parallax), (0.0, 0.0, 0.0));
    }

    #[test]
    fn cascade_is_monotonic_bounded_and_lands_with_the_spring() {
        let c = full();
        for i in 0..20 {
            let mut last = 0.0;
            for step in 0..=100 {
                let p = step as f32 / 100.0;
                let l = c.cascade(p, i);
                assert!(l >= last - 1e-6, "item {i} went back at {p}");
                assert!(l <= p + 1e-6 && l >= 0.0);
                last = l;
            }
            assert_eq!(c.cascade(1.0, i), 1.0);
            assert_eq!(c.cascade(1.1, i), 1.1, "overshoot passes through");
        }
        // Later items wait longer, up to the cap.
        assert!(c.cascade(0.3, 0) > c.cascade(0.3, 1));
        assert!(c.cascade(0.3, 1) > c.cascade(0.3, 4));
        assert_eq!(c.cascade(0.5, 8), c.cascade(0.5, 200));
        assert_eq!(c.cascade(f32::NAN, 3), 1.0);
        assert_eq!(cascade_opacity(0.0), 0.0);
        assert_eq!(cascade_opacity(0.7), 1.0);
        assert_eq!(cascade_opacity(1.3), 1.0);
    }

    #[test]
    fn entrances_start_away_settle_at_rest_and_bound_overshoot() {
        let travel = 20.0;
        for e in [Entrance::Rise, Entrance::Drop, Entrance::Sweep, Entrance::Drift] {
            let c = Choreography::new(e, 0.0, 0.0, 0.0, 1.0);
            let (dx, dy) = c.entrance_offset(0.0, (0.0, -1.0), false, travel);
            assert!(((dx * dx + dy * dy).sqrt() - travel).abs() < 0.05, "{e:?} starts {travel} px away");
            assert_eq!(c.entrance_offset(1.0, (0.0, -1.0), false, travel), (0.0, 0.0));
            let (ox, oy) = c.entrance_offset(5.0, (0.0, -1.0), false, travel);
            assert!((ox * ox + oy * oy).sqrt() <= MAX_OVERSHOOT * travel + 1e-4);
            assert_eq!(c.entrance_offset(f32::NAN, (0.0, -1.0), false, travel), (0.0, 0.0));
        }
        let rise = Choreography::new(Entrance::Rise, 0.0, 0.0, 0.0, 1.0);
        assert!(rise.entrance_offset(0.0, (0.0, -1.0), false, 10.0).1 > 0.0, "rises from below");
        let drop = Choreography::new(Entrance::Drop, 0.0, 0.0, 0.0, 1.0);
        assert!(drop.entrance_offset(0.0, (0.0, 1.0), false, 10.0).1 < 0.0, "drops from above");
        // An overshooting drop lands past its rest (below it), then settles.
        assert!(drop.entrance_offset(1.1, (0.0, 1.0), false, 10.0).1 > 0.0);
        let sweep = Choreography::new(Entrance::Sweep, 0.0, 0.0, 0.0, 1.0);
        assert!(sweep.entrance_offset(0.0, (0.0, 0.0), false, 10.0).0 < 0.0, "from the left in LTR");
        assert!(sweep.entrance_offset(0.0, (0.0, 0.0), true, 10.0).0 > 0.0, "from the right in RTL");
        let drift = Choreography::new(Entrance::Drift, 0.0, 0.0, 0.0, 1.0);
        assert!(drift.entrance_offset(0.0, (0.0, 0.0), false, 10.0).0 > 0.0, "from the reading end");
        // Anchor follows the surface's own path, adding no travel.
        let anchor = Choreography::new(Entrance::Anchor, 0.0, 0.0, 1.0, 1.0);
        assert_eq!(anchor.entrance_offset(0.0, (0.0, -1.0), false, 10.0), (0.0, 0.0));
        assert!(anchor.parallax_offset(0.5, (0.0, -1.0), false, 10.0).1 < 0.0, "parallax trails along the natural path");
    }

    #[test]
    fn list_items_cascade_in_and_rest_in_place() {
        assert_eq!(Choreography::NEUTRAL.item(0.2, 3, (0.0, -1.0), false, 12.0), None);
        assert_eq!(Choreography::new(Entrance::Anchor, 0.0, 1.0, 1.0, 1.0).item(0.2, 3, (0.0, -1.0), false, 12.0), None, "stretch and parallax alone leave lists be");
        let c = Choreography::new(Entrance::Anchor, 1.0, 0.0, 0.0, 1.0);
        let (_, dy, a) = c.item(0.3, 4, (0.0, -1.0), false, 12.0).unwrap();
        assert!(dy < 0.0 && a < 1.0, "Anchor items travel along the surface's own path");
        for i in 0..12 {
            assert_eq!(c.item(1.0, i, (0.0, -1.0), false, 12.0), Some((0.0, 0.0, 1.0)));
        }
        let first = c.item(0.4, 0, (0.0, -1.0), false, 12.0).unwrap();
        let later = c.item(0.4, 6, (0.0, -1.0), false, 12.0).unwrap();
        assert!(first.2 >= later.2 && first.1.abs() <= later.1.abs(), "later items are further behind");
    }

    #[test]
    fn parallax_is_bounded_and_settles() {
        let c = full();
        let depth = 16.0;
        let mut last = f32::INFINITY;
        for step in 0..=100 {
            let p = step as f32 / 100.0;
            let (x, y) = c.parallax_offset(p, (0.0, -1.0), false, depth);
            let d = (x * x + y * y).sqrt();
            assert!(d <= depth + 1e-4);
            assert!(d <= last + 1e-5, "the lag only shrinks while opening");
            last = d;
        }
        assert_eq!(c.parallax_offset(1.0, (0.0, -1.0), false, depth), (0.0, 0.0));
        let (_, over) = c.parallax_offset(1.3, (0.0, 1.0), false, depth);
        assert!(over.abs() <= MAX_OVERSHOOT * MAX_OVERSHOOT * depth + 1e-4);
    }

    #[test]
    fn stretch_rises_with_speed_saturates_and_vanishes_at_rest() {
        let c = full();
        assert_eq!(c.stretch_at(0.0), 0.0);
        let mut last = 0.0;
        for v in [100.0, 500.0, 1500.0, 4000.0] {
            let s = c.stretch_at(v);
            assert!(s > last && s < MAX_STRETCH);
            assert_eq!(c.stretch_at(-v), s, "direction does not matter");
            last = s;
        }
        assert!(c.stretch_at(1.0e6) <= MAX_STRETCH, "a fling saturates");
        assert_eq!(c.stretch_at(f32::INFINITY), 0.0);
        // The tail grows behind the travel; the leading edge stays put.
        let (start, len) = c.stretch_span(100.0, 40.0, 2000.0);
        assert!(len > 40.0 && (start + len - 140.0).abs() < 1e-3, "moving down: the top trails");
        let (start, len) = c.stretch_span(100.0, 40.0, -2000.0);
        assert!(len > 40.0 && start == 100.0, "moving up: the bottom trails");
        assert!(c.thinned(20.0, 2000.0) < 20.0 && c.thinned(20.0, 2000.0) > 20.0 / (1.0 + MAX_STRETCH));
    }

    /// Driving the primitives from a real spring: everything settles on the
    /// neutral values as the spring comes to rest, at 60 and 144 Hz.
    #[test]
    fn primitives_settle_with_their_spring() {
        let c = full();
        for hz in [60.0_f32, 144.0] {
            let mut s = SpringValue::new(0.0_f32, SpringConfig::new(340.0, 15.0, 1.0));
            s.set_target(1.0);
            let mut frames = 0;
            while s.step(1.0 / hz) {
                let p = s.value();
                let (x, y) = c.entrance_offset(p, (0.0, -1.0), false, 24.0);
                assert!(x.abs() <= 24.0 && y.abs() <= 24.0);
                assert!(c.stretch_at(s.velocity()[0] * 400.0) < MAX_STRETCH);
                frames += 1;
                assert!(frames < (hz * 3.0) as usize, "settles within 3 s");
            }
            assert_eq!(s.value(), 1.0);
            assert_eq!(c.entrance_offset(s.value(), (0.0, -1.0), false, 24.0), (0.0, 0.0));
            assert_eq!(c.cascade(s.value(), 7), 1.0);
            assert_eq!(c.stretch_at(s.velocity()[0]), 0.0);
        }
    }

    #[test]
    fn the_global_choreography_round_trips_and_is_neutral_when_reduced() {
        let c = Choreography::new(Entrance::Drift, 0.2, 0.4, 0.6, 1.0);
        set_choreography(c);
        assert_eq!(stored(false), c);
        assert_eq!(stored(true), Choreography::NEUTRAL, "reduced motion: none");
        set_choreography(Choreography::NEUTRAL);
        assert_eq!(choreography(), Choreography::NEUTRAL);
    }
}
