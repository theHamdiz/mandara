//! The scalloped avatar shape of the Material You login and lock designs,
//! after Material 3 Expressive's twelve-sided "cookie", and its morph into
//! a circle.
//!
//! The shape is [`Shape::Cookie`] with [`LOBES`] round bumps; the morph
//! only flattens them ([`depth`]): at `0` it is the cookie, at `1` a plain
//! circle, so every step between keeps the same centre, size and number of
//! points and the outline never jumps.
//!
//! - The lock screen paints in software, so it fills the exact outline for
//!   any morph ([`skia_path`]).
//! - Mandara Login draws with GPUI, whose SVG sprites are rasterized once
//!   per shape and size: the morph uses [`STEPS`] fixed outlines
//!   ([`Step::svg`]), and a value between two steps cross-fades the pair
//!   ([`between`]). The outlines are built once per process.

use std::sync::OnceLock;

use crate::shape::Shape;

/// Bumps around the edge (Material's "cookie 12").
pub const LOBES: u32 = 12;
/// How deep the bumps are at rest, as a share of the radius.
pub const DEPTH: f32 = 0.11;
/// Fixed outlines from the cookie (step 0) to the circle (the last).
pub const STEPS: usize = 7;
/// The SVG view box (the outline's unit box scaled up).
const SVG_BOX: f32 = 100.0;

/// Bump depth `morph` of the way from the cookie to the circle.
pub fn depth(morph: f32) -> f32 {
    DEPTH * (1.0 - morph.clamp(0.0, 1.0))
}

/// The shape `morph` of the way from the cookie to the circle.
pub fn shape(morph: f32) -> Shape {
    Shape::Cookie { lobes: LOBES, depth: depth(morph) }
}

/// The exact outline at `morph`, filling the `side × side` square at
/// `(x, y)`, for software canvases.
pub fn skia_path(morph: f32, x: f32, y: f32, side: f32) -> Option<tiny_skia::Path> {
    shape(morph).skia_path(x, y, side, side)
}

/// One of the fixed outlines GPUI draws.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Step(usize);

impl Step {
    /// Step `i` (clamped to the last).
    pub fn new(i: usize) -> Self {
        Step(i.min(STEPS - 1))
    }

    /// Where this step sits between the cookie (0) and the circle (1).
    pub fn morph(self) -> f32 {
        self.0 as f32 / (STEPS - 1) as f32
    }

    /// The asset path the shell's asset source serves this step under.
    pub fn asset_path(self) -> &'static str {
        const PATHS: [&str; STEPS] = [
            "login-shapes/cookie-0.svg",
            "login-shapes/cookie-1.svg",
            "login-shapes/cookie-2.svg",
            "login-shapes/cookie-3.svg",
            "login-shapes/cookie-4.svg",
            "login-shapes/cookie-5.svg",
            "login-shapes/cookie-6.svg",
        ];
        PATHS[self.0]
    }

    /// The step an asset path names, if it is one.
    pub fn from_asset_path(path: &str) -> Option<Self> {
        (0..STEPS).map(Step).find(|s| s.asset_path() == path)
    }

    /// A single-colour SVG of this step (black, tinted when drawn).
    pub fn svg(self) -> &'static str {
        static SVGS: OnceLock<[String; STEPS]> = OnceLock::new();
        &SVGS.get_or_init(|| std::array::from_fn(|i| svg_text(Step(i).morph())))[self.0]
    }
}

/// The two fixed outlines around `morph` and how far the second shows
/// over the first (`0` = only the first).
pub fn between(morph: f32) -> (Step, Step, f32) {
    let at = morph.clamp(0.0, 1.0) * (STEPS - 1) as f32;
    let lower = (at.floor() as usize).min(STEPS - 1);
    let upper = (lower + 1).min(STEPS - 1);
    (Step(lower), Step(upper), if upper == lower { 0.0 } else { at - lower as f32 })
}

fn svg_text(morph: f32) -> String {
    use std::fmt::Write as _;
    let pts = shape(morph).outline(SVG_BOX, SVG_BOX);
    let mut d = String::with_capacity(pts.len() * 14 + 120);
    let _ = write!(d, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {SVG_BOX} {SVG_BOX}"><path fill="black" d=""#);
    for (i, (x, y)) in pts.iter().enumerate() {
        let _ = write!(d, "{}{x:.2} {y:.2}", if i == 0 { "M" } else { "L" });
    }
    d.push_str(r#"Z"/></svg>"#);
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_morph_ends_at_a_circle_and_stays_inside_its_box() {
        assert_eq!(depth(1.0), 0.0);
        assert_eq!(depth(0.0), DEPTH);
        assert_eq!(depth(-3.0), DEPTH, "clamped");
        for m in [0.0, 0.3, 0.5, 1.0] {
            let pts = shape(m).outline(100.0, 100.0);
            assert!(pts.iter().all(|&(x, y)| (-0.01..=100.01).contains(&x) && (-0.01..=100.01).contains(&y)), "{m}");
            let r_min = pts.iter().map(|&(x, y)| ((x - 50.0).powi(2) + (y - 50.0).powi(2)).sqrt()).fold(f32::MAX, f32::min);
            assert!((r_min - 50.0 * (1.0 - depth(m))).abs() < 0.6, "{m}: valleys at {r_min}");
        }
    }

    #[test]
    fn steps_are_served_and_cross_fade_in_order() {
        for i in 0..STEPS {
            let s = Step::new(i);
            assert_eq!(Step::from_asset_path(s.asset_path()), Some(s));
            assert!(s.svg().starts_with("<svg") && s.svg().ends_with("</svg>"));
        }
        assert_eq!(Step::from_asset_path("icons/user.svg"), None);
        assert_eq!(Step::new(99), Step::new(STEPS - 1));
        assert_eq!(between(0.0), (Step::new(0), Step::new(1), 0.0));
        assert_eq!(between(1.0), (Step::new(STEPS - 1), Step::new(STEPS - 1), 0.0));
        let (a, b, t) = between(0.5);
        assert!(a.morph() <= 0.5 && b.morph() >= 0.5 && (0.0..=1.0).contains(&t));
        // The same outline is drawn for the same morph (no per-call rebuild).
        assert!(std::ptr::eq(Step::new(2).svg(), Step::new(2).svg()));
    }

    #[test]
    fn the_exact_path_exists_for_any_morph() {
        for m in [0.0, 0.25, 0.77, 1.0] {
            let p = skia_path(m, 10.0, 20.0, 96.0).expect("a path");
            let b = p.bounds();
            assert!(b.left() >= 9.9 && b.top() >= 19.9 && b.right() <= 106.1 && b.bottom() <= 116.1);
        }
    }
}
