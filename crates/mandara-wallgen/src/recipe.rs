//! Recipes: everything that determines a wallpaper, small enough to save
//! next to it as TOML so it can be re-rendered at any size or remixed.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::noise::Rng;
use crate::palette::{PALETTES, PaletteSpec};
use crate::params::{COMMON_PARAMS, ParamSpec};
use crate::style::Style;

/// Recipe format version written to new files.
pub const RECIPE_VERSION: u32 = 1;
/// Share of [`Recipe::random`] draws that take one of a style's favourite
/// palettes rather than any curated one.
const FAVOURITE_SHARE: f32 = 0.6;
/// [`Recipe::mutate`] jitters a parameter by up to this many standard
/// deviations of its nice range at amount 1.
const MUTATE_SPREAD: f32 = 0.35;
/// At amount 1, [`Recipe::mutate`] swaps a named palette with this chance.
const MUTATE_PALETTE_CHANCE: f32 = 0.3;

/// A complete wallpaper description.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    #[serde(default = "version")]
    pub version: u32,
    pub style: Style,
    /// Layout seed (written as hex, so all 64 bits survive TOML).
    #[serde(serialize_with = "seed_out", deserialize_with = "seed_in")]
    pub seed: u64,
    /// Light wallpaper: a pale ground with saturated accents.
    #[serde(default)]
    pub light: bool,
    #[serde(default)]
    pub palette: PaletteSpec,
    /// Parameter values by key; missing keys use their defaults and
    /// out-of-range values are clamped when read.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

fn version() -> u32 {
    RECIPE_VERSION
}

fn seed_out<S: Serializer>(seed: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{seed:016x}"))
}

fn seed_in<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Seed {
        Int(i64),
        Text(String),
    }
    match Seed::deserialize(d)? {
        Seed::Int(i) => Ok(i as u64),
        Seed::Text(s) => {
            let s = s.trim();
            let hex = s.strip_prefix("0x").unwrap_or(s);
            u64::from_str_radix(hex, 16).or_else(|_| s.parse()).map_err(serde::de::Error::custom)
        }
    }
}

/// Palettes each style is designed around (ids), favoured by [`Recipe::random`].
fn favourites(style: Style) -> &'static [&'static str] {
    match style {
        Style::Mesh => &["mandara-corniche", "montaza-twilight", "stanley-sunset", "rose-pine", "catppuccin-mocha", "pharos-night"],
        Style::Landscape => &["montaza-twilight", "sunset", "stanley-sunset", "nord", "forest", "kanagawa"],
        Style::Corniche => &["mandara-corniche", "sunset", "montaza-twilight", "pharos-night", "ocean"],
        Style::Aurora => &["mandara-obsidian", "mandara-corniche", "tokyo-night", "nord", "pharos-night"],
        Style::Dunes => &["desert", "qaitbay-citadel", "sunset", "stanley-sunset", "montaza-twilight"],
        Style::Flow => &["mandara-obsidian", "rose-pine", "tokyo-night", "dracula", "mandara-corniche"],
        Style::Contours => &["mandara-corniche", "forest", "nord", "gruvbox", "kanagawa", "mono"],
        Style::Bokeh => &["montaza-twilight", "pharos-night", "rose-gold", "stanley-sunset", "catppuccin-mocha"],
        Style::LowPoly => &["mandara-corniche", "ocean", "sunset", "nord", "dracula"],
        Style::Liquid => &["montaza-twilight", "rose-pine", "catppuccin-mocha", "mandara-corniche", "rose-gold"],
        Style::Waves => &["mandara-corniche", "ocean", "montaza-twilight", "tokyo-night", "sunset"],
        Style::Glass => &["mandara-obsidian", "montaza-twilight", "pharos-night", "dracula", "rose-pine"],
    }
}

impl Recipe {
    /// Defaults for `style` with the default palette.
    pub fn new(style: Style, seed: u64) -> Recipe {
        Recipe { version: RECIPE_VERSION, style, seed, light: false, palette: PaletteSpec::default(), params: BTreeMap::new() }
    }

    /// Every parameter of this recipe's style: its own, then the common ones.
    pub fn specs(&self) -> impl Iterator<Item = &'static ParamSpec> {
        self.style.params().iter().chain(COMMON_PARAMS)
    }

    /// Value of parameter `key`: the stored value clamped to its range, or
    /// its default. Keys the style doesn't have read as 0.
    pub fn param(&self, key: &str) -> f32 {
        match self.style.spec(key) {
            Some(spec) => self.params.get(key).map_or(spec.default, |v| spec.clamp(*v)),
            None => 0.0,
        }
    }

    /// Sets parameter `key` (clamped); unknown keys are ignored.
    pub fn set_param(&mut self, key: &str, value: f32) {
        if let Some(spec) = self.style.spec(key) {
            self.params.insert(key.to_string(), spec.clamp(value));
        }
    }

    /// Clamps every known parameter, drops non-finite values (unknown keys
    /// are kept so a newer recipe survives an older engine).
    pub fn sanitize(&mut self) {
        let style = self.style;
        self.params.retain(|_, v| v.is_finite());
        for (k, v) in self.params.iter_mut() {
            if let Some(spec) = style.spec(k) {
                *v = spec.clamp(*v);
            }
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Parses and sanitizes a TOML recipe.
    pub fn from_toml(text: &str) -> Result<Recipe, String> {
        let mut r: Recipe = toml::from_str(text).map_err(|e| e.to_string())?;
        r.sanitize();
        Ok(r)
    }

    /// A tasteful random recipe: a style (or the given one), a palette
    /// (usually one the style is designed around) and every parameter drawn
    /// from its nice range. Dark by default; set `light` to follow the
    /// shell theme.
    pub fn random(style: Option<Style>, seed: u64) -> Recipe {
        let mut rng = Rng::new(seed ^ 0x5241_4E44);
        let style = style.unwrap_or_else(|| Style::ALL[rng.below(Style::ALL.len())]);
        let fav = favourites(style);
        let palette = if rng.f() < FAVOURITE_SHARE { fav[rng.below(fav.len())] } else { PALETTES[rng.below(PALETTES.len())].id };
        let mut r = Recipe::new(style, rng.next_u64());
        r.palette = PaletteSpec::Named(palette.into());
        for spec in style.params().iter().chain(COMMON_PARAMS) {
            // Average of two uniforms: centre-weighted inside the nice range.
            let u = (rng.f() + rng.f()) * 0.5;
            let v = spec.nice.0 + (spec.nice.1 - spec.nice.0) * u;
            r.params.insert(spec.key.to_string(), spec.clamp(v));
        }
        r
    }

    /// A variation: parameters jittered in proportion to `amount` (0–1),
    /// and with probability `amount` a new layout seed; named palettes are
    /// occasionally swapped at high amounts. Style and light mode are kept.
    pub fn mutate(&self, amount: f32, seed: u64) -> Recipe {
        let amount = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
        let mut rng = Rng::new(seed ^ self.seed.rotate_left(17) ^ 0x4D55_5441);
        let mut r = self.clone();
        for spec in self.style.params().iter().chain(COMMON_PARAMS) {
            // Fixed-at-default finishes (grain, dither) stay as the user set them.
            if spec.nice.0 == spec.nice.1 {
                continue;
            }
            let width = (spec.nice.1 - spec.nice.0).max(spec.step);
            let v = self.param(spec.key) + rng.normal() * width * MUTATE_SPREAD * amount;
            r.params.insert(spec.key.to_string(), spec.clamp(v));
        }
        if rng.f() < amount {
            r.seed = rng.next_u64();
        }
        if matches!(r.palette, PaletteSpec::Named(_)) && rng.f() < amount * MUTATE_PALETTE_CHANCE {
            let fav = favourites(self.style);
            r.palette = PaletteSpec::Named(fav[rng.below(fav.len())].into());
        }
        r
    }

    /// Short readable token of the seed for file names.
    pub fn short_seed(&self) -> String {
        format!("{:06x}", (self.seed ^ (self.seed >> 24) ^ (self.seed >> 48)) & 0xFF_FFFF)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_keeps_every_bit() {
        let mut r = Recipe::random(Some(Style::Aurora), 42);
        r.seed = u64::MAX - 3;
        r.light = true;
        r.palette = PaletteSpec::Custom(vec!["#101820".into(), "#5fe0d4".into()]);
        let text = r.to_toml();
        assert!(text.contains("style = \"aurora\""), "{text}");
        assert_eq!(Recipe::from_toml(&text).unwrap(), r);
    }

    #[test]
    fn missing_params_default_and_out_of_range_clamp() {
        let text = "style = \"waves\"\nseed = 7\n[params]\nbands = 99.0\namplitude = -5.0\nvignette = 1e9\nfuture_knob = 3.0\nrim = nan\n";
        let r = Recipe::from_toml(text).unwrap();
        assert_eq!(r.version, RECIPE_VERSION);
        assert_eq!(r.seed, 7);
        assert_eq!(r.palette, PaletteSpec::default());
        assert_eq!(r.param("bands"), 12.0);
        assert_eq!(r.param("amplitude"), 0.0);
        assert_eq!(r.param("vignette"), 100.0);
        assert_eq!(r.param("frequency"), 40.0, "default");
        assert_eq!(r.param("rim"), 60.0, "NaN dropped → default");
        assert_eq!(r.params.get("future_knob"), Some(&3.0), "unknown keys survive");
        assert_eq!(r.param("future_knob"), 0.0);
        // Hex seeds with a prefix parse too.
        assert_eq!(Recipe::from_toml("style = \"mesh\"\nseed = \"0x1f\"").unwrap().seed, 31);
        assert!(Recipe::from_toml("style = \"nope\"\nseed = 1").is_err());
    }

    #[test]
    fn random_and_mutate_stay_valid() {
        for i in 0..200u64 {
            let r = Recipe::random(None, i);
            assert!(r.palette.is_valid());
            for spec in r.specs() {
                let v = r.param(spec.key);
                assert!(v >= spec.min && v <= spec.max && v.is_finite());
                assert!(v >= spec.nice.0 - spec.step && v <= spec.nice.1 + spec.step, "{}.{} = {v}", r.style, spec.key);
            }
            let m = r.mutate(1.0, i * 31);
            assert_eq!(m.style, r.style);
            for spec in m.specs() {
                let v = m.param(spec.key);
                assert!(v >= spec.min && v <= spec.max);
            }
            assert_eq!(r.mutate(0.0, i).params, r.params, "amount 0 changes nothing");
        }
        assert_eq!(Recipe::random(None, 5), Recipe::random(None, 5));
        let styles: std::collections::BTreeSet<Style> = (0..100).map(|i| Recipe::random(None, i).style).collect();
        assert_eq!(styles.len(), Style::ALL.len(), "every style is reachable");
    }

    #[test]
    fn set_param_clamps() {
        let mut r = Recipe::new(Style::Mesh, 1);
        r.set_param("points", 100.0);
        assert_eq!(r.param("points"), 9.0);
        r.set_param("nope", 1.0);
        assert!(!r.params.contains_key("nope"));
    }
}
