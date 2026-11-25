//! The Hadith library's ornament, in Jannah's geometry
//! ([`crate::plugins::jannah::art`]): a manuscript headpiece, two hairline
//! rules running into an eight-pointed star with a lozenge at each end,
//! served as a monochrome SVG mask (`art/hadith-headpiece.svg`) and tinted
//! gold like an icon. Only geometry.

use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::plugins::jannah::art::star;

/// The headpiece's canvas (its aspect is the element's).
pub const HEADPIECE_VIEW: (f32, f32) = (400.0, 28.0);
/// The star's outer radius, the gap around it and the lozenges' half size.
const STAR_R: f32 = 11.0;
const STAR_GAP: f32 = 9.0;
const LOZENGE: f32 = 4.0;
/// Stroke of the rules and the star; the inner star's share and alpha.
const RULE_W: f32 = 1.2;
const STAR_W: f32 = 1.4;
const INNER_SHARE: f32 = 0.45;
const INNER_ALPHA: f32 = 0.7;
/// The rules fade out toward the edges.
const RULE_EDGE_ALPHA: f32 = 0.0;

/// Asset path for `gpui::svg().path(…)`.
pub const HEADPIECE_PATH: &str = "art/hadith-headpiece.svg";

fn poly(pts: &[(f32, f32)]) -> String {
    let mut d = String::from("M");
    for (k, p) in pts.iter().enumerate() {
        let _ = write!(d, "{}{:.1} {:.1}", if k == 0 { "" } else { "L" }, p.0, p.1);
    }
    d.push('Z');
    d
}

fn headpiece_svg() -> String {
    let (w, h) = HEADPIECE_VIEW;
    let c = (w / 2.0, h / 2.0);
    let left_end = c.0 - STAR_R - STAR_GAP;
    let right_start = c.0 + STAR_R + STAR_GAP;
    let lozenge = |x: f32| poly(&[(x, c.1 - LOZENGE), (x + LOZENGE, c.1), (x, c.1 + LOZENGE), (x - LOZENGE, c.1)]);
    let outer = poly(&star(c, STAR_R));
    let inner = poly(&star(c, STAR_R * INNER_SHARE));
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" fill="none" stroke="black" stroke-linejoin="round" stroke-linecap="round"><defs><linearGradient id="l" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="{le:.1}" y2="0"><stop offset="0" stop-color="black" stop-opacity="{RULE_EDGE_ALPHA}"/><stop offset="1" stop-color="black"/></linearGradient><linearGradient id="r" gradientUnits="userSpaceOnUse" x1="{rs:.1}" y1="0" x2="{w}" y2="0"><stop offset="0" stop-color="black"/><stop offset="1" stop-color="black" stop-opacity="{RULE_EDGE_ALPHA}"/></linearGradient></defs><path d="M0 {cy:.1}H{le:.1}" stroke="url(#l)" stroke-width="{RULE_W}"/><path d="M{rs:.1} {cy:.1}H{w}" stroke="url(#r)" stroke-width="{RULE_W}"/><path d="{l1}" fill="black" stroke="none"/><path d="{l2}" fill="black" stroke="none"/><path d="{outer}" stroke-width="{STAR_W}"/><path d="{inner}" fill="black" stroke="none" fill-opacity="{INNER_ALPHA}"/></svg>"#,
        cy = c.1,
        le = left_end - LOZENGE * 2.0,
        rs = right_start + LOZENGE * 2.0,
        l1 = lozenge(left_end - LOZENGE),
        l2 = lozenge(right_start + LOZENGE),
    )
}

/// The SVG of `art/<name>.svg` (`hadith-headpiece`), built once.
pub fn asset(name: &str) -> Option<String> {
    static HEADPIECE: OnceLock<String> = OnceLock::new();
    (name == "hadith-headpiece").then(|| HEADPIECE.get_or_init(headpiece_svg).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headpiece_is_svg() {
        let svg = asset("hadith-headpiece").unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok());
        assert!(asset("jannah-crown").is_none());
        assert_eq!(HEADPIECE_PATH, "art/hadith-headpiece.svg");
    }
}
