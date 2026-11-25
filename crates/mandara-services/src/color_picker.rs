//! Color picker logic: the picked-color history (persisted in
//! `~/.local/state/mandara/colors.toml`), format conversions (HEX, RGB,
//! HSL via `palette`, OKLCH), OKLCH tonal ramps, WCAG contrast grading and
//! pixel sampling / loupe grids from a screencopy frame.

use std::path::PathBuf;

use mandara_theme::Color;
use palette::{FromColor, Hsl, Srgb};
use serde::{Deserialize, Serialize};

/// Most colors kept in the history.
pub const MAX_HISTORY: usize = 24;
/// Steps of the tonal ramp.
pub const RAMP_STEPS: usize = 9;
/// OKLCH lightness range of the ramp (lightest → darkest).
const RAMP_LIGHT: f32 = 0.96;
const RAMP_DARK: f32 = 0.22;
/// WCAG thresholds.
pub const WCAG_AAA: f32 = 7.0;
pub const WCAG_AA: f32 = 4.5;
pub const WCAG_AA_LARGE: f32 = 3.0;

/// One picked color.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picked {
    /// `#rrggbb`, lowercase.
    pub hex: String,
    /// Unix seconds.
    pub at: i64,
}

impl Picked {
    pub fn rgb(&self) -> Option<[u8; 3]> {
        parse_hex(&self.hex)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct HistoryFile {
    #[serde(default)]
    colors: Vec<Picked>,
}

/// `~/.local/state/mandara/colors.toml`.
pub fn history_path() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("colors.toml")
}

pub fn load_history_from(path: &std::path::Path) -> Vec<Picked> {
    let mut colors = std::fs::read_to_string(path).ok().and_then(|t| toml::from_str::<HistoryFile>(&t).ok()).map(|f| f.colors).unwrap_or_default();
    colors.retain(|c| parse_hex(&c.hex).is_some());
    colors.truncate(MAX_HISTORY);
    colors
}

pub fn load_history() -> Vec<Picked> {
    load_history_from(&history_path())
}

pub fn save_history_to(path: &std::path::Path, colors: &[Picked]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string(&HistoryFile { colors: colors.to_vec() }).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}

pub fn save_history(colors: &[Picked]) -> Result<(), String> {
    save_history_to(&history_path(), colors)
}

/// Adds a color newest-first, moving a repeat to the front and capping
/// the list at [`MAX_HISTORY`].
pub fn remember(history: &mut Vec<Picked>, rgb: [u8; 3], at: i64) {
    let hex = to_hex(rgb);
    history.retain(|c| c.hex != hex);
    history.insert(0, Picked { hex, at });
    history.truncate(MAX_HISTORY);
}

pub fn to_hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Parses `#rgb`, `#rrggbb` (with or without `#`).
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    let digit = |i: usize| u8::from_str_radix(h.get(i..i + 1)?, 16).ok();
    match h.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        6 => Some([u8::from_str_radix(&h[0..2], 16).ok()?, u8::from_str_radix(&h[2..4], 16).ok()?, u8::from_str_radix(&h[4..6], 16).ok()?]),
        _ => None,
    }
}

pub fn color_of(rgb: [u8; 3]) -> Color {
    Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255)
}

/// Every textual format of a color.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Formats {
    pub hex: String,
    pub rgb: String,
    pub hsl: String,
    pub oklch: String,
}

/// HSL `(hue°, saturation %, lightness %)` via `palette`.
pub fn hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let srgb = Srgb::new(rgb[0], rgb[1], rgb[2]).into_format::<f32>();
    let h = Hsl::from_color(srgb);
    (h.hue.into_positive_degrees(), h.saturation * 100.0, h.lightness * 100.0)
}

pub fn formats(rgb: [u8; 3]) -> Formats {
    let (h, s, l) = hsl(rgb);
    let c = color_of(rgb);
    // Achromatic colors have no meaningful hue: print 0 like CSS tools do.
    let hue = if c.chroma() < 1e-4 { 0.0 } else { c.hue() };
    Formats {
        hex: to_hex(rgb),
        rgb: format!("rgb({} {} {})", rgb[0], rgb[1], rgb[2]),
        hsl: format!("hsl({:.0} {:.0}% {:.0}%)", h, s, l),
        oklch: format!("oklch({:.1}% {:.3} {:.1})", c.lightness() * 100.0, c.chroma(), hue),
    }
}

/// A ramp of [`RAMP_STEPS`] tones of `rgb`'s hue and chroma from light to
/// dark, evenly spaced in OKLCH lightness and mapped into the sRGB gamut.
pub fn tonal_ramp(rgb: [u8; 3]) -> Vec<[u8; 3]> {
    let base = color_of(rgb);
    (0..RAMP_STEPS)
        .map(|i| {
            let l = RAMP_LIGHT + (RAMP_DARK - RAMP_LIGHT) * i as f32 / (RAMP_STEPS - 1) as f32;
            // Chroma tapers toward the ends where the gamut narrows.
            let taper = 1.0 - ((l - 0.6) / 0.4).abs().min(1.0) * 0.55;
            let [r, g, b, _] = Color::oklch(l, base.chroma() * taper, base.hue()).gamut_mapped().to_rgba8();
            [r, g, b]
        })
        .collect()
}

/// WCAG 2.x grade of a contrast ratio.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub enum Grade {
    Aaa,
    Aa,
    AaLarge,
    Fail,
}

impl Grade {
    pub fn of(ratio: f32) -> Self {
        if ratio >= WCAG_AAA {
            Grade::Aaa
        } else if ratio >= WCAG_AA {
            Grade::Aa
        } else if ratio >= WCAG_AA_LARGE {
            Grade::AaLarge
        } else {
            Grade::Fail
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Grade::Aaa => "AAA",
            Grade::Aa => "AA",
            Grade::AaLarge => "AA Large",
            Grade::Fail => "Fail",
        }
    }
}

/// The physical pixel at a logical position of a capture.
pub fn sample(width: u32, height: u32, rgba: &[u8], px: i64, py: i64) -> Option<[u8; 3]> {
    if px < 0 || py < 0 || px >= width as i64 || py >= height as i64 {
        return None;
    }
    let i = (py as usize * width as usize + px as usize) * 4;
    rgba.get(i..i + 3).map(|s| [s[0], s[1], s[2]])
}

/// `side × side` pixels centred on `(cx, cy)` (row-major; `None` outside
/// the frame) for the magnifier loupe.
pub fn loupe(width: u32, height: u32, rgba: &[u8], cx: i64, cy: i64, side: usize) -> Vec<Option<[u8; 3]>> {
    let half = (side / 2) as i64;
    let mut out = Vec::with_capacity(side * side);
    for dy in -half..=half {
        for dx in -half..=half {
            out.push(sample(width, height, rgba, cx + dx, cy + dy));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex("#0A7cFf"), Some([10, 124, 255]));
        assert_eq!(parse_hex("fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#zzzzzz"), None);
        assert_eq!(to_hex([10, 124, 255]), "#0a7cff");
    }

    #[test]
    fn formats_match_css() {
        let f = formats([255, 0, 0]);
        assert_eq!(f.hex, "#ff0000");
        assert_eq!(f.rgb, "rgb(255 0 0)");
        assert_eq!(f.hsl, "hsl(0 100% 50%)");
        // CSS Color 4 reference: oklch(62.8% 0.2577 29.23).
        assert!(f.oklch.starts_with("oklch(62.8% 0.25"), "{}", f.oklch);
        let gray = formats([128, 128, 128]);
        assert_eq!(gray.hsl, "hsl(0 0% 50%)");
        assert!(gray.oklch.ends_with(" 0.0)"), "{}", gray.oklch);
    }

    #[test]
    fn ramp_runs_light_to_dark_and_keeps_hue() {
        let ramp = tonal_ramp([0x1e, 0x88, 0xe5]);
        assert_eq!(ramp.len(), RAMP_STEPS);
        let l: Vec<f32> = ramp.iter().map(|c| color_of(*c).lightness()).collect();
        assert!(l.windows(2).all(|w| w[0] > w[1]), "{l:?}");
        let hue = color_of([0x1e, 0x88, 0xe5]).hue();
        let mid = color_of(ramp[RAMP_STEPS / 2]);
        assert!(mandara_theme::color::shortest_hue_delta(mid.hue(), hue).abs() < 6.0);
    }

    #[test]
    fn contrast_grades() {
        let white = color_of([255, 255, 255]);
        let black = color_of([0, 0, 0]);
        assert!((white.contrast_ratio(black) - 21.0).abs() < 0.1);
        assert_eq!(Grade::of(21.0), Grade::Aaa);
        assert_eq!(Grade::of(4.6), Grade::Aa);
        assert_eq!(Grade::of(3.2), Grade::AaLarge);
        assert_eq!(Grade::of(1.5).label(), "Fail");
    }

    #[test]
    fn history_dedupes_caps_and_persists() {
        let mut h = Vec::new();
        for i in 0..(MAX_HISTORY as u8 + 5) {
            remember(&mut h, [i, 0, 0], i as i64);
        }
        assert_eq!(h.len(), MAX_HISTORY);
        remember(&mut h, [10, 0, 0], 99);
        assert_eq!(h[0].hex, "#0a0000");
        assert_eq!(h.iter().filter(|c| c.hex == "#0a0000").count(), 1);
        let dir = std::env::temp_dir().join(format!("mandara-colors-{}", std::process::id()));
        let path = dir.join("colors.toml");
        save_history_to(&path, &h).unwrap();
        assert_eq!(load_history_from(&path), h);
        std::fs::write(&path, "colors = [{ hex = \"nope\", at = 1 }]").unwrap();
        assert!(load_history_from(&path).is_empty(), "invalid entries are dropped");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn samples_and_loupe_grid() {
        // 3×2 image, pixel (x, y) = [x*10, y*10, 7].
        let rgba: Vec<u8> = (0..2).flat_map(|y| (0..3).flat_map(move |x| [x * 10, y * 10, 7, 255])).collect();
        assert_eq!(sample(3, 2, &rgba, 2, 1), Some([20, 10, 7]));
        assert_eq!(sample(3, 2, &rgba, 3, 0), None);
        let grid = loupe(3, 2, &rgba, 0, 0, 3);
        assert_eq!(grid.len(), 9);
        assert_eq!(grid[4], Some([0, 0, 7]), "centre");
        assert_eq!(grid[0], None, "outside the frame");
    }
}
