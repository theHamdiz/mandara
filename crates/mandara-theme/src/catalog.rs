//! The theme catalog: token-based themes authored as a handful of seed
//! colors per appearance (dark / light pair) in TOML, expanded into the full
//! [`MandaraPalette`] with contrast guarantees.
//!
//! Built-in themes are embedded at compile time; user themes are read from
//! `~/.config/mandara/themes/*.toml` (same format; a user theme with a
//! built-in's id overrides it).
//!
//! ```toml
//! id = "my-theme"
//! name = "My Theme"
//! [dark]
//! surface = "#0a1520"
//! on_surface = "#e4f0f4"
//! primary = "#5fe0d4"
//! secondary = "#e9c98b"
//! tertiary = "#86b6ff"
//! [light]   # optional — derived from dark when absent
//! ```

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::palette::{MandaraPalette, accent_gradient_end_for, glow_for};

/// Seed colors for one appearance. Only `surface`, `on_surface` and
/// `primary` are required; everything else is derived when absent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Seeds {
    pub surface: Color,
    #[serde(default)]
    pub surface_dim: Option<Color>,
    #[serde(default)]
    pub surface_variant: Option<Color>,
    pub on_surface: Color,
    #[serde(default)]
    pub on_surface_variant: Option<Color>,
    #[serde(default)]
    pub outline: Option<Color>,
    pub primary: Color,
    #[serde(default)]
    pub on_primary: Option<Color>,
    #[serde(default)]
    pub secondary: Option<Color>,
    #[serde(default)]
    pub tertiary: Option<Color>,
    #[serde(default)]
    pub error: Option<Color>,
    #[serde(default)]
    pub success: Option<Color>,
    #[serde(default)]
    pub warning: Option<Color>,
    /// Glass tint opacity (`0..=1`).
    #[serde(default)]
    pub glass_alpha: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ThemeOrigin {
    Builtin,
    User { path: PathBuf },
}

/// One theme: metadata plus a dark/light seed pair.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// One of Mandara's Alexandrian signature themes.
    #[serde(default)]
    pub signature: bool,
    /// Pure-black surfaces in dark mode.
    #[serde(default)]
    pub oled: bool,
    #[serde(default)]
    pub dark: Option<Seeds>,
    #[serde(default)]
    pub light: Option<Seeds>,
    #[serde(skip, default = "builtin_origin")]
    pub origin: ThemeOrigin,
}

fn builtin_origin() -> ThemeOrigin {
    ThemeOrigin::Builtin
}

/// Default theme id.
pub const DEFAULT_THEME: &str = "mandara-corniche";

const BUILTIN_SOURCES: &[&str] = &[
    include_str!("../themes/mandara-corniche.toml"),
    include_str!("../themes/montaza-twilight.toml"),
    include_str!("../themes/mandara-obsidian.toml"),
    include_str!("../themes/qaitbay-citadel.toml"),
    include_str!("../themes/pharos-night.toml"),
    include_str!("../themes/stanley-sunset.toml"),
    include_str!("../themes/catppuccin.toml"),
    include_str!("../themes/rose-pine.toml"),
    include_str!("../themes/kanagawa.toml"),
    include_str!("../themes/tokyo-night.toml"),
    include_str!("../themes/nord.toml"),
    include_str!("../themes/gruvbox.toml"),
    include_str!("../themes/eldritch.toml"),
    include_str!("../themes/vesper.toml"),
];

/// Built-in themes in display order.
pub fn builtin() -> &'static [ThemeDef] {
    static THEMES: OnceLock<Vec<ThemeDef>> = OnceLock::new();
    THEMES.get_or_init(|| {
        BUILTIN_SOURCES
            .iter()
            .map(|src| toml::from_str::<ThemeDef>(src).unwrap_or_else(|e| panic!("built-in theme is invalid: {e}")))
            .collect()
    })
}

/// `~/.config/mandara/themes`.
pub fn user_theme_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("mandara/themes")
}

/// Built-in plus user themes (user ids override built-ins).
pub fn all() -> Vec<ThemeDef> {
    let mut themes: Vec<ThemeDef> = builtin().to_vec();
    if let Ok(entries) = std::fs::read_dir(user_theme_dir()) {
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "toml")).collect();
        paths.sort();
        for path in paths {
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| toml::from_str::<ThemeDef>(&t).map_err(|e| e.to_string())) {
                Ok(mut def) if def.dark.is_some() || def.light.is_some() => {
                    def.origin = ThemeOrigin::User { path: path.clone() };
                    themes.retain(|t| t.id != def.id);
                    themes.push(def);
                }
                Ok(_) => tracing::warn!("{}: theme has neither [dark] nor [light]", path.display()),
                Err(err) => tracing::warn!("{}: invalid theme: {err}", path.display()),
            }
        }
    }
    themes
}

/// Looks a theme up by id (case-insensitive; spaces/underscores ≈ dashes).
pub fn find(id: &str) -> Option<ThemeDef> {
    let needle = id.trim().to_ascii_lowercase().replace([' ', '_'], "-").replace('é', "e");
    all().into_iter().find(|t| t.id == needle || t.name.to_ascii_lowercase().replace(' ', "-").replace('é', "e") == needle)
}

/// The id after `id` in catalog order (wrapping).
pub fn next_id(id: &str) -> String {
    let themes = all();
    let i = themes.iter().position(|t| t.id == id).map(|i| i + 1).unwrap_or(0);
    themes[i % themes.len()].id.clone()
}

/// Pushes `fg` away from `bg` in lightness until it reaches `min` contrast.
pub fn ensure_contrast(fg: Color, bg: Color, min: f32) -> Color {
    if fg.contrast_ratio(bg) >= min {
        return fg;
    }
    let lighter = bg.relative_luminance() < 0.2;
    let mut c = fg;
    for _ in 0..60 {
        c = c.lighten(if lighter { 0.015 } else { -0.015 });
        if c.contrast_ratio(bg) >= min {
            break;
        }
    }
    c
}

/// Derives light seeds from dark ones (for dark-only themes).
fn derive_light(d: &Seeds) -> Seeds {
    let hue = d.surface.hue();
    let chroma = d.surface.chroma().min(0.02);
    let accent = |c: Color| c.with_lightness(0.5).scale_chroma(0.9);
    Seeds {
        surface: Color::oklch(0.975, chroma * 0.6, hue),
        surface_dim: Some(Color::oklch(0.91, chroma * 0.8, hue)),
        surface_variant: Some(Color::oklch(0.935, chroma, hue)),
        on_surface: Color::oklch(0.22, chroma.max(0.01), hue),
        on_surface_variant: Some(Color::oklch(0.42, chroma.max(0.015), hue)),
        outline: Some(Color::oklch(0.62, chroma.max(0.015), hue)),
        primary: accent(d.primary),
        on_primary: None,
        secondary: d.secondary.map(accent),
        tertiary: d.tertiary.map(accent),
        error: d.error.map(accent),
        success: d.success.map(accent),
        warning: d.warning.map(|c| c.with_lightness(0.55)),
        glass_alpha: None,
    }
}

impl ThemeDef {
    /// Whether the theme provides (rather than derives) this appearance.
    pub fn has_mode(&self, dark: bool) -> bool {
        if dark { self.dark.is_some() } else { self.light.is_some() }
    }

    /// The expanded palette for an appearance.
    pub fn palette(&self, dark: bool) -> MandaraPalette {
        let seeds = match (dark, &self.dark, &self.light) {
            (true, Some(d), _) => d.clone(),
            (false, _, Some(l)) => l.clone(),
            (false, Some(d), None) => derive_light(d),
            (true, None, Some(l)) => l.clone(),
            (_, None, None) => unreachable!("validated on load"),
        };
        expand(&seeds, dark && (self.dark.is_some()), &self.name, self.oled && dark)
    }
}

/// Expands seeds into every Mandara token, enforcing contrast.
pub fn expand(s: &Seeds, dark: bool, name: &str, oled: bool) -> MandaraPalette {
    let surface = s.surface;
    let step = if dark { 1.0 } else { -1.0 };
    let surface_dim = s.surface_dim.unwrap_or(if oled { surface } else { surface.lighten(-0.025 * step) });
    let variant = s.surface_variant.unwrap_or(surface.lighten(0.06 * step));
    let container_low = surface.mix(variant, 0.5);
    let container_high = variant.lighten(0.035 * step);
    let on_surface = ensure_contrast(s.on_surface, surface, 7.0);
    let on_surface_variant = ensure_contrast(s.on_surface_variant.unwrap_or(on_surface.mix(surface, 0.3)), variant, 4.5);
    let outline = ensure_contrast(s.outline.unwrap_or(on_surface.mix(surface, 0.6)), surface, 3.0);
    let primary = ensure_contrast(s.primary, surface, 3.0);
    let on_primary = ensure_contrast(
        s.on_primary.unwrap_or_else(|| primary.best_on(Color::oklch(0.99, 0.01, primary.hue()), Color::oklch(0.17, 0.03, primary.hue()))),
        primary,
        4.5,
    );
    let secondary = ensure_contrast(s.secondary.unwrap_or(primary.rotate_hue(-40.0)), surface, 3.0);
    let tertiary = ensure_contrast(s.tertiary.unwrap_or(primary.rotate_hue(60.0)), surface, 3.0);
    let harmonized = |c: Color| ensure_contrast(c.harmonize_toward(primary).gamut_mapped(), surface, 3.0);
    let status = |l: f32, c: f32, h: f32| harmonized(Color::oklch(if dark { l } else { l - 0.2 }, c, h));
    let glass_alpha = s.glass_alpha.unwrap_or(if dark { 0.66 } else { 0.74 });
    MandaraPalette {
        name: name.to_string(),
        is_dark: dark,
        surface_dim,
        surface_base: surface,
        surface_container_low: container_low,
        surface_container: variant,
        surface_container_high: container_high,
        surface_glass_tint: surface.with_alpha(glass_alpha),
        specular_rim_light: if dark {
            Color::oklcha(0.97, (primary.chroma() * 0.12).min(0.02), primary.hue(), 0.34)
        } else {
            Color::oklcha(1.0, 0.0, 0.0, 0.85)
        },
        specular_rim_shadow: if dark { Color::oklcha(0.04, surface.chroma() * 0.5, surface.hue(), 0.5) } else { outline.with_alpha(0.18) },
        on_surface,
        on_surface_variant,
        outline,
        primary,
        on_primary,
        primary_container: surface.mix(primary, if dark { 0.3 } else { 0.18 }),
        primary_glow: glow_for(primary, dark),
        secondary,
        tertiary,
        accent_gradient_start: primary,
        accent_gradient_end: accent_gradient_end_for(primary),
        urgent_coral: s.error.map(harmonized).unwrap_or_else(|| status(0.72, 0.16, 28.0)),
        success_emerald: s.success.map(harmonized).unwrap_or_else(|| status(0.76, 0.15, 158.0)),
        warning_amber: s.warning.map(harmonized).unwrap_or_else(|| status(0.83, 0.15, 78.0)),
    }
}

/// WCAG contrast report for a palette (DMS-style `wcag.json` equivalent).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContrastReport {
    pub body: f32,
    pub body_variant: f32,
    pub on_primary: f32,
    pub outline: f32,
    pub accents_min: f32,
}

impl ContrastReport {
    pub fn of(p: &MandaraPalette) -> Self {
        Self {
            body: p.on_surface.contrast_ratio(p.surface_base),
            body_variant: p.on_surface_variant.contrast_ratio(p.surface_container),
            on_primary: p.on_primary.contrast_ratio(p.primary),
            outline: p.outline.contrast_ratio(p.surface_base),
            accents_min: [p.primary, p.secondary, p.tertiary]
                .iter()
                .map(|c| c.contrast_ratio(p.surface_base))
                .fold(f32::INFINITY, f32::min),
        }
    }

    pub fn passes(&self) -> bool {
        self.body >= 7.0 && self.body_variant >= 4.5 && self.on_primary >= 4.5 && self.outline >= 3.0 && self.accents_min >= 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_parses_with_both_modes() {
        let themes = builtin();
        assert!(themes.len() >= 14);
        assert_eq!(themes[0].id, DEFAULT_THEME);
        for t in themes {
            assert!(t.dark.is_some(), "{} needs a dark variant", t.id);
            for dark in [true, false] {
                let p = t.palette(dark);
                assert_eq!(p.is_dark, dark, "{}", t.id);
            }
        }
    }

    #[test]
    fn every_builtin_meets_contrast_guarantees() {
        for t in builtin() {
            for dark in [true, false] {
                let r = ContrastReport::of(&t.palette(dark));
                assert!(r.passes(), "{} ({}) fails contrast: {r:?}", t.id, if dark { "dark" } else { "light" });
            }
        }
    }

    #[test]
    fn signature_themes_and_derived_light() {
        let signature: Vec<_> = builtin().iter().filter(|t| t.signature).map(|t| t.id.as_str()).collect();
        assert!(signature.contains(&"mandara-corniche") && signature.contains(&"pharos-night"));
        let obsidian = find("mandara-obsidian").unwrap();
        assert_eq!(obsidian.palette(true).surface_dim.to_hex(), "#000000", "OLED black");
        let vesper = find("Vesper").unwrap();
        assert!(!vesper.has_mode(false));
        let light = vesper.palette(false);
        assert!(!light.is_dark && light.surface_base.lightness() > 0.9);
    }

    #[test]
    fn lookup_and_cycling() {
        assert_eq!(find("Rosé Pine").unwrap().id, "rose-pine");
        assert_eq!(find("tokyo_night").unwrap().id, "tokyo-night");
        assert!(find("nope").is_none());
        assert_ne!(next_id(DEFAULT_THEME), DEFAULT_THEME);
    }

    #[test]
    fn contrast_enforcement_moves_away_from_background() {
        let bg = Color::hex("#101010");
        let fixed = ensure_contrast(Color::hex("#202020"), bg, 4.5);
        assert!(fixed.contrast_ratio(bg) >= 4.5);
        let light_bg = Color::hex("#f8f8f8");
        assert!(ensure_contrast(Color::hex("#e0e0e0"), light_bg, 4.5).contrast_ratio(light_bg) >= 4.5);
    }
}
