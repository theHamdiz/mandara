//! The Qur'an plugin's ornament, from the same geometry as Jannah's art
//! (its eight-pointed star): the medallion that holds a surah's or a
//! juz's number, an outlined khatam with a ring inside, drawn as a
//! monochrome SVG mask (`art/quran-star.svg`) and tinted like an icon.

use std::fmt::Write as _;
use std::sync::OnceLock;

use crate::plugins::jannah::art::star;

/// The medallion's canvas (square), the star's radius and the ring's.
const VIEW: f32 = 40.0;
const STAR_R: f32 = 19.0;
const RING_R: f32 = 13.2;
const STROKE: f32 = 1.4;
/// The ring is fainter than the star.
const RING_ALPHA: f32 = 0.55;

/// Asset path for `gpui::svg().path(…)`.
pub const STAR_PATH: &str = "art/quran-star.svg";

fn star_svg() -> String {
    let c = VIEW / 2.0;
    let mut d = String::from("M");
    for (k, p) in star((c, c), STAR_R).iter().enumerate() {
        let _ = write!(d, "{}{:.2} {:.2}", if k == 0 { "" } else { "L" }, p.0, p.1);
    }
    d.push('Z');
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {VIEW} {VIEW}" fill="none" stroke="black" stroke-linejoin="round"><path d="{d}" stroke-width="{STROKE}"/><circle cx="{c}" cy="{c}" r="{RING_R}" stroke-width="{w}" stroke-opacity="{RING_ALPHA}"/></svg>"#,
        w = STROKE * 0.8
    )
}

/// The SVG of `art/<name>.svg` (`quran-star`), built once.
pub fn asset(name: &str) -> Option<String> {
    static STAR: OnceLock<String> = OnceLock::new();
    match name {
        "quran-star" => Some(STAR.get_or_init(star_svg).clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_medallion_is_svg() {
        let svg = asset("quran-star").unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(usvg::Tree::from_str(&svg, &usvg::Options::default()).is_ok());
        assert!(asset("jannah-crown").is_none());
        assert_eq!(STAR_PATH, "art/quran-star.svg");
    }
}
