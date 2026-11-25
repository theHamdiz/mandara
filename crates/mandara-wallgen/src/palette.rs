//! Palettes: curated named sets (the Mandara signatures, community
//! favourites and a few moods), custom hex stops, or colors handed in from
//! the live shell theme. Every palette resolves to a [`Scheme`]: its stops
//! ordered from deepest to brightest, with a smooth OKLab ramp through them.

use mandara_i18n::tk;
use serde::{Deserialize, Serialize};

use crate::color::Lab;

/// Fewest and most stops a palette may have.
pub const MIN_STOPS: usize = 2;
pub const MAX_STOPS: usize = 8;
/// The palette used when a spec resolves to fewer than [`MIN_STOPS`] colors.
pub const DEFAULT_PALETTE: &str = "mandara-corniche";
/// Samples in a scheme's precomputed ramp.
const RAMP_SAMPLES: usize = 512;
/// Light mode: the deepest stop becomes a pale ground at this lightness…
const LIGHT_GROUND_L: f32 = 0.955;
/// …the brightest stop an accent at this lightness…
const LIGHT_ACCENT_L: f32 = 0.64;
/// …and chroma is scaled from this factor (ground) to the next (accent).
const LIGHT_GROUND_CHROMA: f32 = 0.38;
const LIGHT_ACCENT_CHROMA: f32 = 1.05;

/// A curated palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NamedPalette {
    /// Stable kebab-case id (used in recipes).
    pub id: &'static str,
    /// English label, marked for translation (`mandara_i18n::tr` it).
    pub label: &'static str,
    /// `#rrggbb` stops, deepest first.
    pub colors: &'static [&'static str],
}

const fn named(id: &'static str, label: &'static str, colors: &'static [&'static str]) -> NamedPalette {
    NamedPalette { id, label, colors }
}

/// Every curated palette: the Mandara signatures first.
pub const PALETTES: &[NamedPalette] = &[
    named("mandara-corniche", tk("Mandara Corniche"), &["#040d18", "#0a2238", "#134d66", "#2aa3a8", "#5fe0d4", "#e9c98b"]),
    named("montaza-twilight", tk("Montaza Twilight"), &["#0b0818", "#211845", "#4b2f7f", "#b9a0ff", "#ff9d7d", "#ffc56e"]),
    named("mandara-obsidian", tk("Mandara Obsidian"), &["#030303", "#100f0d", "#262016", "#1f7a58", "#3fdc9a", "#ebc56a"]),
    named("pharos-night", tk("Pharos Night"), &["#050817", "#101a32", "#2b4a8c", "#7eabff", "#ff92b4", "#ffd56e"]),
    named("qaitbay-citadel", tk("Qaitbay Citadel"), &["#15110d", "#2b2219", "#5e4a33", "#72c4db", "#e48f7b", "#f2b66c"]),
    named("stanley-sunset", tk("Stanley Sunset"), &["#160e1b", "#2a1c32", "#6a3b6e", "#c69eff", "#ff8c8f", "#ffba6e"]),
    named("catppuccin-mocha", tk("Catppuccin Mocha"), &["#11111b", "#1e1e2e", "#45475a", "#89b4fa", "#cba6f7", "#f5c2e7", "#fab387"]),
    named("rose-pine", tk("Rosé Pine"), &["#12101b", "#191724", "#26233a", "#31748f", "#c4a7e7", "#ebbcba", "#f6c177"]),
    named("tokyo-night", tk("Tokyo Night"), &["#101014", "#1a1b26", "#24283b", "#3d59a1", "#7aa2f7", "#bb9af7", "#7dcfff"]),
    named("kanagawa", tk("Kanagawa"), &["#16161d", "#1f1f28", "#2d4f67", "#7e9cd8", "#957fb8", "#ffa066", "#e6c384"]),
    named("nord", tk("Nord"), &["#1d222b", "#2e3440", "#4c566a", "#5e81ac", "#88c0d0", "#b48ead", "#ebcb8b"]),
    named("gruvbox", tk("Gruvbox"), &["#141617", "#1d2021", "#3c3836", "#458588", "#8ec07c", "#fabd2f", "#fe8019"]),
    named("dracula", tk("Dracula"), &["#14151b", "#21222c", "#44475a", "#6272a4", "#bd93f9", "#ff79c6", "#8be9fd"]),
    named("sunset", tk("Sunset"), &["#140b26", "#3a1752", "#8a2a6a", "#e0505e", "#ff9a5a", "#ffd88a"]),
    named("forest", tk("Forest"), &["#07130e", "#10291f", "#245840", "#5c9a63", "#b8cf86", "#f0e4b4"]),
    named("desert", tk("Desert"), &["#1e120c", "#4a2a19", "#94532e", "#d38f53", "#ecc387", "#f8e6c4"]),
    named("ocean", tk("Ocean"), &["#020b17", "#05264a", "#0a5486", "#1a9cc0", "#7ddcdc", "#e6fbff"]),
    named("rose-gold", tk("Rose Gold"), &["#160f11", "#3a2328", "#7d4a52", "#cc8d86", "#f0c2ad", "#fbe6d8"]),
    named("mono", tk("Monochrome"), &["#09090b", "#18181c", "#34343a", "#66666e", "#b4b4bc", "#f1f1f3"]),
];

/// Every curated palette (see [`PALETTES`]).
pub fn palettes() -> &'static [NamedPalette] {
    PALETTES
}

/// The curated palette with this id.
pub fn named_palette(id: &str) -> Option<&'static NamedPalette> {
    PALETTES.iter().find(|p| p.id == id)
}

/// Where a recipe's colors come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaletteSpec {
    /// A curated palette by id ([`palettes`]).
    Named(String),
    /// Hex stops (`#rrggbb`), 2–8.
    Custom(Vec<String>),
    /// Colors from the live shell theme, passed in by the caller.
    Theme(Vec<String>),
}

impl Default for PaletteSpec {
    fn default() -> Self {
        PaletteSpec::Named(DEFAULT_PALETTE.into())
    }
}

/// `#rrggbb` / `rrggbb` / `#rgb` → bytes.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    let b = s.strip_prefix('#').unwrap_or(s);
    if !b.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize, n: usize| u8::from_str_radix(&b[i..i + n], 16).ok();
    match b.len() {
        6 => Some([byte(0, 2)?, byte(2, 2)?, byte(4, 2)?]),
        3 => Some([byte(0, 1)? * 17, byte(1, 1)? * 17, byte(2, 1)? * 17]),
        _ => None,
    }
}

impl PaletteSpec {
    /// The hex stops this spec names (unvalidated).
    pub fn hex_stops(&self) -> Vec<String> {
        match self {
            PaletteSpec::Named(id) => named_palette(id).map(|p| p.colors.iter().map(|c| (*c).to_string()).collect()).unwrap_or_default(),
            PaletteSpec::Custom(c) | PaletteSpec::Theme(c) => c.clone(),
        }
    }

    /// Whether this spec resolves on its own (known id, 2–8 valid hex
    /// stops) rather than falling back to the default palette.
    pub fn is_valid(&self) -> bool {
        let stops = self.hex_stops();
        (MIN_STOPS..=MAX_STOPS).contains(&stops.len()) && stops.iter().all(|s| parse_hex(s).is_some())
    }

    /// The OKLab stops: valid ones only, at most [`MAX_STOPS`], falling back
    /// to [`DEFAULT_PALETTE`] when fewer than [`MIN_STOPS`] remain.
    pub fn stops(&self) -> Vec<Lab> {
        let stops: Vec<Lab> = self.hex_stops().iter().filter_map(|s| Lab::from_hex(s)).take(MAX_STOPS).collect();
        if stops.len() >= MIN_STOPS {
            return stops;
        }
        PaletteSpec::default().stops()
    }

    /// Display label: the curated name, or a generic one.
    pub fn label(&self) -> &'static str {
        match self {
            PaletteSpec::Named(id) => named_palette(id).map_or(tk("Custom"), |p| p.label),
            PaletteSpec::Custom(_) => tk("Custom"),
            PaletteSpec::Theme(_) => tk("Current theme"),
        }
    }
}

/// A resolved palette: stops ordered deepest → brightest (for light mode,
/// remapped to a pale ground → saturated accents), and a ramp through them.
#[derive(Clone, Debug)]
pub struct Scheme {
    stops: Vec<Lab>,
    accents: Vec<Lab>,
    ramp: Vec<Lab>,
    pub light: bool,
}

impl Scheme {
    pub fn new(spec: &PaletteSpec, light: bool) -> Scheme {
        let mut stops = spec.stops();
        stops.sort_by(|a, b| a.l.total_cmp(&b.l).then(a.chroma().total_cmp(&b.chroma())));
        if light {
            let n = stops.len().max(2) - 1;
            stops = stops
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let r = i as f32 / n as f32;
                    let l = LIGHT_GROUND_L + (LIGHT_ACCENT_L - LIGHT_GROUND_L) * r;
                    let k = LIGHT_GROUND_CHROMA + (LIGHT_ACCENT_CHROMA - LIGHT_GROUND_CHROMA) * r;
                    Lab { l, a: s.a * k, b: s.b * k }
                })
                .collect();
        }
        let ramp = (0..RAMP_SAMPLES).map(|i| interpolate(&stops, i as f32 / (RAMP_SAMPLES - 1) as f32)).collect();
        let mut accents: Vec<Lab> = stops[stops.len() / 2..].to_vec();
        accents.sort_by(|a, b| b.chroma().total_cmp(&a.chroma()));
        Scheme { stops, accents, ramp, light }
    }

    /// Color at `t ∈ [0, 1]` along the deepest → brightest ramp.
    #[inline]
    pub fn ramp(&self, t: f32) -> Lab {
        let f = t.clamp(0.0, 1.0) * (RAMP_SAMPLES - 1) as f32;
        let i = (f as usize).min(RAMP_SAMPLES - 2);
        self.ramp[i].mix(self.ramp[i + 1], f - i as f32)
    }

    pub fn len(&self) -> usize {
        self.stops.len()
    }

    /// Stop `i` (deepest first), wrapping.
    pub fn stop(&self, i: usize) -> Lab {
        self.stops[i % self.stops.len()]
    }

    /// The deepest stop (the ground of a dark wallpaper).
    pub fn deep(&self) -> Lab {
        self.stops[0]
    }

    /// The brightest stop.
    pub fn bright(&self) -> Lab {
        self.stops[self.stops.len() - 1]
    }

    /// Accent `i`: cycles through the upper half of the stops, most
    /// colorful first, so blobs and lights get the palette's characters.
    pub fn accent(&self, i: usize) -> Lab {
        self.accents[i % self.accents.len()]
    }

    /// The warmest bright stop, for suns, lamps and glints: the upper-half
    /// stop whose hue is nearest gold, lifted toward white.
    pub fn sun(&self) -> Lab {
        let n = self.stops.len();
        let gold = Lab::from_lch(0.85, 0.12, SUN_HUE);
        let best = self.stops[n / 2..].iter().copied().max_by(|a, b| warmth(*a, gold).total_cmp(&warmth(*b, gold))).unwrap_or(self.bright());
        let lift = if self.light { SUN_LIFT_LIGHT } else { SUN_LIFT };
        best.mix(Lab::new(1.0, best.a * 0.4, best.b * 0.4), lift)
    }
}

/// OKLCh hue (radians) of warm sunlight.
const SUN_HUE: f32 = 1.2;
/// How far a sun color is lifted toward white (dark and light mode).
const SUN_LIFT: f32 = 0.35;
const SUN_LIFT_LIGHT: f32 = 0.5;

/// Similarity to gold weighted by lightness: picks a palette's "sun".
fn warmth(c: Lab, gold: Lab) -> f32 {
    let dh = (c.a - gold.a).hypot(c.b - gold.b);
    c.l * 0.6 + c.chroma() * 1.5 - dh * 2.0
}

/// Catmull–Rom-smoothed position along evenly spaced stops, blended in
/// OKLab (smooth through each stop, no kinks where segments meet).
fn interpolate(stops: &[Lab], t: f32) -> Lab {
    let n = stops.len();
    if n == 1 {
        return stops[0];
    }
    let f = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (f as usize).min(n - 2);
    let u = f - i as f32;
    let p0 = stops[i.saturating_sub(1)];
    let (p1, p2) = (stops[i], stops[i + 1]);
    let p3 = stops[(i + 2).min(n - 1)];
    let cr = |a: f32, b: f32, c: f32, d: f32| {
        let (u2, u3) = (u * u, u * u * u);
        0.5 * (2.0 * b + (c - a) * u + (2.0 * a - 5.0 * b + 4.0 * c - d) * u2 + (3.0 * b - a - 3.0 * c + d) * u3)
    };
    // Catmull–Rom can overshoot lightness between stops; keep it within
    // the segment so a ramp never pokes darker than its darkest stop.
    let l = cr(p0.l, p1.l, p2.l, p3.l).clamp(p1.l.min(p2.l), p1.l.max(p2.l));
    // Chroma follows its own spline so hue turns never pass through grey.
    let chroma = cr(p0.chroma(), p1.chroma(), p2.chroma(), p3.chroma()).clamp(p1.chroma().min(p2.chroma()), p1.chroma().max(p2.chroma()));
    Lab { l, a: cr(p0.a, p1.a, p2.a, p3.a), b: cr(p0.b, p1.b, p2.b, p3.b) }.with_chroma(chroma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_palette_is_valid() {
        let mut ids = std::collections::BTreeSet::new();
        for p in PALETTES {
            assert!(ids.insert(p.id), "duplicate {}", p.id);
            let spec = PaletteSpec::Named(p.id.into());
            assert!(spec.is_valid(), "{}", p.id);
            assert_eq!(spec.stops().len(), p.colors.len());
        }
        assert!(PALETTES.len() >= 15);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#0a1520"), Some([10, 21, 32]));
        assert_eq!(parse_hex("FFF"), Some([255, 255, 255]));
        assert_eq!(parse_hex(" #abc "), Some([0xAA, 0xBB, 0xCC]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#ggg"), None);
    }

    #[test]
    fn invalid_specs_fall_back() {
        let one = PaletteSpec::Custom(vec!["#ff0000".into(), "nope".into()]);
        assert!(!one.is_valid());
        assert_eq!(one.stops(), PaletteSpec::default().stops());
        assert!(!PaletteSpec::Named("no-such".into()).is_valid());
        let too_many = PaletteSpec::Custom(vec!["#101010".into(); 12]);
        assert!(!too_many.is_valid());
        assert_eq!(too_many.stops().len(), MAX_STOPS);
        assert!(PaletteSpec::Theme(vec!["#101820".into(), "#5fe0d4".into()]).is_valid());
    }

    #[test]
    fn scheme_orders_and_ramps_smoothly() {
        let s = Scheme::new(&PaletteSpec::Custom(vec!["#ffd56e".into(), "#050817".into(), "#7eabff".into()]), false);
        assert!(s.deep().l < s.stop(1).l && s.stop(1).l < s.bright().l);
        let mut prev = s.ramp(0.0);
        for i in 1..=200 {
            let c = s.ramp(i as f32 / 200.0);
            let step = (c.l - prev.l).abs() + (c.a - prev.a).abs() + (c.b - prev.b).abs();
            assert!(step < 0.03, "ramp jumps at {i}");
            prev = c;
        }
        let light = Scheme::new(&PaletteSpec::default(), true);
        assert!(light.deep().l > 0.9 && light.bright().l < 0.7);
        assert!(s.sun().l > 0.7);
    }
}
