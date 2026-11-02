//! Jannah's decoration, drawn from one geometry for both renderers: the
//! pointed (mihrab) arch's crown and a faint eight-pointed-star lattice.
//! The panel draws them as monochrome SVG masks (`art/jannah-crown.svg`,
//! `art/jannah-lattice.svg`, rasterized anti-aliased by resvg and tinted
//! like icons); the painted desktop widget samples the same curves into
//! smooth polylines. No living beings, only geometry.

use std::f32::consts::{FRAC_PI_4, FRAC_PI_8, TAU};
use std::fmt::Write as _;
use std::sync::OnceLock;

/// The crown's SVG canvas: width and height (its aspect is the element's).
pub const CROWN_VIEW: (f32, f32) = (400.0, 120.0);
/// The crown's feet sit this far in from each side of its canvas.
pub const CROWN_FOOT: f32 = 6.0;
/// Inner (second) arch inset, canvas units.
pub const CROWN_INNER: f32 = 12.0;
/// Stroke widths of the outer and inner arch, canvas units.
const CROWN_STROKE: (f32, f32) = (2.4, 1.3);
/// Coverage of the inner arch (the mask's alpha).
const CROWN_INNER_ALPHA: f32 = 0.5;
/// Bézier handles of one half of the arch, as fractions of its width and
/// height: the side rises straight, then bends into the point.
const SHOULDER: (f32, f32) = (0.0, 0.46);
const NECK: (f32, f32) = (0.30, 0.10);

/// The lattice canvas (square), the star spacing and size, its stroke.
pub const LATTICE_VIEW: f32 = 440.0;
const LATTICE_STEP: f32 = 55.0;
const LATTICE_STAR_R: f32 = 17.0;
const LATTICE_STROKE: f32 = 1.1;
/// The lattice fades from this coverage at the top to none at the bottom.
const LATTICE_TOP_ALPHA: f32 = 1.0;

/// Inner radius of an eight-pointed star (two squares) over its outer one.
pub fn star_inner_ratio() -> f32 {
    FRAC_PI_4.cos() / FRAC_PI_8.cos()
}

/// The outline of an eight-pointed star (the khatam: two squares turned
/// 45° apart) centred at `c` with outer radius `r`, point up.
pub fn star(c: (f32, f32), r: f32) -> Vec<(f32, f32)> {
    let inner = r * star_inner_ratio();
    (0..16)
        .map(|i| {
            let a = -TAU / 4.0 + i as f32 * TAU / 16.0;
            let rad = if i % 2 == 0 { r } else { inner };
            (c.0 + rad * a.cos(), c.1 + rad * a.sin())
        })
        .collect()
}

fn cubic(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0, a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1)
}

/// The control points of a pointed arch over the box `(x, y, w, h)`: the
/// left half (foot → apex) and the right half (apex → foot).
fn halves(x: f32, y: f32, w: f32, h: f32) -> [[(f32, f32); 4]; 2] {
    let foot_l = (x, y + h);
    let foot_r = (x + w, y + h);
    let apex = (x + w / 2.0, y);
    let l1 = (x + SHOULDER.0 * w, y + SHOULDER.1 * h);
    let l2 = (x + NECK.0 * w, y + NECK.1 * h);
    let r2 = (x + w - NECK.0 * w, y + NECK.1 * h);
    let r1 = (x + w - SHOULDER.0 * w, y + SHOULDER.1 * h);
    [[foot_l, l1, l2, apex], [apex, r2, r1, foot_r]]
}

/// A pointed arch's crown over `(x, y, w, h)` (feet at the bottom corners,
/// the point at the top centre), sampled into `steps` points per half.
pub fn crown(x: f32, y: f32, w: f32, h: f32, steps: usize) -> Vec<(f32, f32)> {
    let steps = steps.max(2);
    let [l, r] = halves(x, y, w, h);
    let mut pts: Vec<(f32, f32)> = (0..=steps).map(|i| cubic(l[0], l[1], l[2], l[3], i as f32 / steps as f32)).collect();
    pts.extend((1..=steps).map(|i| cubic(r[0], r[1], r[2], r[3], i as f32 / steps as f32)));
    pts
}

fn arch_path(x: f32, y: f32, w: f32, h: f32) -> String {
    let [l, r] = halves(x, y, w, h);
    format!(
        "M{:.1} {:.1}C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
        l[0].0, l[0].1, l[1].0, l[1].1, l[2].0, l[2].1, l[3].0, l[3].1, r[1].0, r[1].1, r[2].0, r[2].1, r[3].0, r[3].1
    )
}

fn crown_svg() -> String {
    let (w, h) = CROWN_VIEW;
    let (outer, inner) = CROWN_STROKE;
    let half = outer / 2.0;
    let o = arch_path(CROWN_FOOT, half, w - 2.0 * CROWN_FOOT, h - half);
    let i = arch_path(CROWN_FOOT + CROWN_INNER, CROWN_INNER * 1.6, w - 2.0 * (CROWN_FOOT + CROWN_INNER), h - CROWN_INNER * 1.6);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" fill="none" stroke="black" stroke-linecap="round"><path d="{o}" stroke-width="{outer}"/><path d="{i}" stroke-width="{inner}" stroke-opacity="{CROWN_INNER_ALPHA}"/></svg>"#
    )
}

fn lattice_svg() -> String {
    let v = LATTICE_VIEW;
    let mut body = String::new();
    let n = (v / LATTICE_STEP).ceil() as usize + 1;
    for row in 0..n {
        for col in 0..n {
            // Every other row shifted half a step: a woven grid.
            let shift = if row % 2 == 1 { LATTICE_STEP / 2.0 } else { 0.0 };
            let c = (col as f32 * LATTICE_STEP + shift, row as f32 * LATTICE_STEP);
            let pts = star(c, LATTICE_STAR_R);
            let _ = write!(body, "<path d=\"M");
            for (k, p) in pts.iter().enumerate() {
                let _ = write!(body, "{}{:.1} {:.1}", if k == 0 { "" } else { "L" }, p.0, p.1);
            }
            body.push_str("Z\"/>");
        }
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {v} {v}" fill="none"><defs><linearGradient id="f" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="black" stop-opacity="{LATTICE_TOP_ALPHA}"/><stop offset="1" stop-color="black" stop-opacity="0"/></linearGradient></defs><g stroke="url(#f)" stroke-width="{LATTICE_STROKE}" stroke-linejoin="round">{body}</g></svg>"#
    )
}

/// The SVG of `art/<name>.svg` (`jannah-crown`, `jannah-lattice`), built
/// once.
pub fn asset(name: &str) -> Option<String> {
    static CROWN: OnceLock<String> = OnceLock::new();
    static LATTICE: OnceLock<String> = OnceLock::new();
    match name {
        "jannah-crown" => Some(CROWN.get_or_init(crown_svg).clone()),
        "jannah-lattice" => Some(LATTICE.get_or_init(lattice_svg).clone()),
        _ => None,
    }
}

/// Asset paths for `gpui::svg().path(…)`.
pub const CROWN_PATH: &str = "art/jannah-crown.svg";
pub const LATTICE_PATH: &str = "art/jannah-lattice.svg";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arch_rises_to_a_centred_point() {
        let pts = crown(0.0, 0.0, 200.0, 100.0, 24);
        assert_eq!(pts.len(), 49);
        assert_eq!(pts[0], (0.0, 100.0));
        let apex = pts[24];
        assert!((apex.0 - 100.0).abs() < 1e-3 && apex.1.abs() < 1e-3);
        assert!((pts[48].0 - 200.0).abs() < 1e-3 && (pts[48].1 - 100.0).abs() < 1e-3);
        // Symmetric.
        for i in 0..24 {
            assert!((pts[i].0 + pts[48 - i].0 - 200.0).abs() < 1e-2 && (pts[i].1 - pts[48 - i].1).abs() < 1e-2);
        }
    }

    #[test]
    fn stars_have_eight_points() {
        let s = star((0.0, 0.0), 10.0);
        assert_eq!(s.len(), 16);
        assert!((s[0].1 + 10.0).abs() < 1e-4, "point up");
        let inner = (s[1].0 * s[1].0 + s[1].1 * s[1].1).sqrt();
        assert!((inner - 10.0 * star_inner_ratio()).abs() < 1e-3);
    }

    #[test]
    fn the_art_assets_are_svg() {
        for name in ["jannah-crown", "jannah-lattice"] {
            let svg = asset(name).unwrap();
            assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"), "{name}");
            assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok(), "{name} parses");
        }
        assert!(asset("nope").is_none());
    }
}
