//! The `MandaraPalette` semantic token set and its derivation rules.

use serde::{Deserialize, Serialize};

use crate::color::{Color, lerp};

/// Every semantic design token the shell renders with.
///
/// Surfaces form an elevation ladder; glass and rim tokens drive the
/// chromatic-glass material; accent and status roles are harmonized against
/// `primary` so the whole shell reads as one family.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MandaraPalette {
    pub name: String,
    pub is_dark: bool,

    // Surface hierarchy
    pub surface_dim: Color,
    pub surface_base: Color,
    pub surface_container_low: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_glass_tint: Color,
    pub specular_rim_light: Color,
    pub specular_rim_shadow: Color,

    // Content
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub outline: Color,

    // Accents & harmony
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub primary_glow: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub accent_gradient_start: Color,
    pub accent_gradient_end: Color,

    // Semantic status roles
    pub urgent_coral: Color,
    pub success_emerald: Color,
    pub warning_amber: Color,
}

/// Iterates `(token_name, color)` pairs; keeps the Studio swatch matrix,
/// template exporters and interpolation in lock-step with the struct.
macro_rules! palette_tokens {
    ($mac:ident) => {
        $mac!(
            surface_dim,
            surface_base,
            surface_container_low,
            surface_container,
            surface_container_high,
            surface_glass_tint,
            specular_rim_light,
            specular_rim_shadow,
            on_surface,
            on_surface_variant,
            outline,
            primary,
            on_primary,
            primary_container,
            primary_glow,
            secondary,
            tertiary,
            accent_gradient_start,
            accent_gradient_end,
            urgent_coral,
            success_emerald,
            warning_amber
        )
    };
}

/// User-facing tuning applied on top of any palette source (Studio sliders).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteAdjustments {
    /// Multiplies every accent's chroma (`1.0` = unchanged).
    pub chroma_scale: f32,
    /// Added to every token's lightness.
    pub lightness_shift: f32,
    /// Rotates every chromatic token's hue, in degrees.
    pub hue_shift: f32,
    /// Overrides the glass tint opacity, `0..=1`.
    pub glass_opacity: Option<f32>,
    /// Scales the specular rim alpha (`1.0` = unchanged).
    pub rim_intensity: f32,
    /// Accessibility: push surfaces and text apart until body text meets
    /// WCAG AAA (7:1) and outlines stay clearly visible.
    pub high_contrast: bool,
}

impl Default for PaletteAdjustments {
    fn default() -> Self {
        Self {
            chroma_scale: 1.0,
            lightness_shift: 0.0,
            hue_shift: 0.0,
            glass_opacity: None,
            rim_intensity: 1.0,
            high_contrast: false,
        }
    }
}

impl PaletteAdjustments {
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

/// High contrast: OKLCH lightness of surfaces scaled toward black (dark) or
/// lifted toward white (light), text / outline lightness targets, and the
/// contrast every text token must reach (WCAG AAA).
const HC_DARK_SURFACE_SCALE: f32 = 0.55;
const HC_LIGHT_SURFACE_LIFT: f32 = 0.6;
const HC_TEXT_DARK: f32 = 0.99;
const HC_TEXT_VARIANT_DARK: f32 = 0.92;
const HC_OUTLINE_DARK: f32 = 0.8;
const HC_TEXT_LIGHT: f32 = 0.08;
const HC_TEXT_VARIANT_LIGHT: f32 = 0.2;
const HC_OUTLINE_LIGHT: f32 = 0.35;
pub const HC_MIN_CONTRAST: f32 = 7.0;
/// Accents (icons, large text) reach WCAG AA on the base surface.
const HC_ACCENT_CONTRAST: f32 = 4.5;
/// Lightness step used when nudging a token toward the contrast target.
const HC_STEP: f32 = 0.02;

/// Hue rotation of the accent gradient's end stop, degrees (CLAUDE.md).
pub const ACCENT_HUE_ROTATION_DEG: f32 = 25.0;
/// Chroma boost of the accent gradient's end stop (×).
pub const ACCENT_CHROMA_BOOST: f32 = 1.25;
/// OKLCH lightness lift of the accent gradient's end stop.
pub const ACCENT_LIGHTNESS_LIFT: f32 = 0.02;
/// Glow: OKLCH lightness shift on dark / light themes, chroma boost and
/// opacity.
pub const GLOW_LIFT_DARK: f32 = 0.06;
pub const GLOW_LIFT_LIGHT: f32 = -0.04;
pub const GLOW_CHROMA_BOOST: f32 = 1.12;
pub const GLOW_ALPHA: f32 = 0.55;
/// `on_primary` candidates: near-white / near-black OKLCH lightness and
/// chroma, tinted with the primary's hue.
const ON_PRIMARY_LIGHT: (f32, f32) = (0.99, 0.01);
const ON_PRIMARY_DARK: (f32, f32) = (0.18, 0.03);

/// Accent gradient end: hue +25°, chroma boosted 25% (then gamut-mapped).
pub fn accent_gradient_end_for(primary: Color) -> Color {
    primary.rotate_hue(ACCENT_HUE_ROTATION_DEG).scale_chroma(ACCENT_CHROMA_BOOST).lighten(ACCENT_LIGHTNESS_LIFT)
}

/// A luminous halo version of `primary` for glows and focus rings.
pub fn glow_for(primary: Color, is_dark: bool) -> Color {
    let l = if is_dark { GLOW_LIFT_DARK } else { GLOW_LIFT_LIGHT };
    primary.lighten(l).scale_chroma(GLOW_CHROMA_BOOST).with_alpha(GLOW_ALPHA)
}

impl MandaraPalette {
    /// Recomputes the tokens that are defined relative to `primary`. Call after
    /// overriding `primary` by hand.
    pub fn rederive_accents(&mut self) {
        self.primary_glow = glow_for(self.primary, self.is_dark);
        self.accent_gradient_start = self.primary;
        self.accent_gradient_end = accent_gradient_end_for(self.primary);
        self.on_primary = self.primary.best_on(
            Color::oklch(ON_PRIMARY_LIGHT.0, ON_PRIMARY_LIGHT.1, self.primary.hue()),
            Color::oklch(ON_PRIMARY_DARK.0, ON_PRIMARY_DARK.1, self.primary.hue()),
        );
    }

    /// Interpolates every token in OKLCH. `t` is clamped to `0..=1`.
    pub fn lerp(&self, other: &MandaraPalette, t: f32) -> MandaraPalette {
        let t = t.clamp(0.0, 1.0);
        macro_rules! mix_all {
            ($($field:ident),*) => {
                MandaraPalette {
                    name: if t < 0.5 { self.name.clone() } else { other.name.clone() },
                    is_dark: if t < 0.5 { self.is_dark } else { other.is_dark },
                    $($field: self.$field.mix(other.$field, t),)*
                }
            };
        }
        palette_tokens!(mix_all)
    }

    /// Applies Studio tuning sliders.
    pub fn adjusted(&self, adj: &PaletteAdjustments) -> MandaraPalette {
        if adj.is_identity() {
            return self.clone();
        }
        let mut out = self.clone();
        let tweak = |c: Color, accent: bool| -> Color {
            let chroma = if accent { c.chroma() * adj.chroma_scale } else { c.chroma() };
            Color::oklcha(
                c.lightness() + adj.lightness_shift,
                chroma,
                c.hue() + adj.hue_shift,
                c.alpha(),
            )
            .gamut_mapped()
        };
        macro_rules! apply {
            ($($field:ident),*) => {
                $(
                    let accent = matches!(
                        stringify!($field),
                        "primary" | "primary_glow" | "secondary" | "tertiary"
                            | "accent_gradient_start" | "accent_gradient_end" | "primary_container"
                            | "urgent_coral" | "success_emerald" | "warning_amber"
                    );
                    out.$field = tweak(self.$field, accent);
                )*
            };
        }
        palette_tokens!(apply);
        if let Some(opacity) = adj.glass_opacity {
            out.surface_glass_tint = out.surface_glass_tint.with_alpha(opacity);
        }
        out.specular_rim_light = out
            .specular_rim_light
            .with_alpha((self.specular_rim_light.alpha() * adj.rim_intensity).clamp(0.0, 1.0));
        out.specular_rim_shadow = out
            .specular_rim_shadow
            .with_alpha((self.specular_rim_shadow.alpha() * adj.rim_intensity).clamp(0.0, 1.0));
        if adj.high_contrast {
            out.apply_high_contrast();
        }
        out
    }

    /// Separates surfaces and content until every text token reaches
    /// [`HC_MIN_CONTRAST`] on every surface of the ladder.
    fn apply_high_contrast(&mut self) {
        let dark = self.is_dark;
        let surface = |c: Color| {
            let l = if dark { c.lightness() * HC_DARK_SURFACE_SCALE } else { c.lightness() + (1.0 - c.lightness()) * HC_LIGHT_SURFACE_LIFT };
            Color::oklcha(l, c.chroma(), c.hue(), c.alpha()).gamut_mapped()
        };
        self.surface_dim = surface(self.surface_dim);
        self.surface_base = surface(self.surface_base);
        self.surface_container_low = surface(self.surface_container_low);
        self.surface_container = surface(self.surface_container);
        self.surface_container_high = surface(self.surface_container_high);
        let (text, variant, outline) = if dark { (HC_TEXT_DARK, HC_TEXT_VARIANT_DARK, HC_OUTLINE_DARK) } else { (HC_TEXT_LIGHT, HC_TEXT_VARIANT_LIGHT, HC_OUTLINE_LIGHT) };
        let set_l = |c: Color, l: f32| Color::oklcha(l, c.chroma().min(0.04), c.hue(), c.alpha()).gamut_mapped();
        self.on_surface = set_l(self.on_surface, text);
        self.on_surface_variant = set_l(self.on_surface_variant, variant);
        self.outline = set_l(self.outline, outline);
        let surfaces = [self.surface_dim, self.surface_base, self.surface_container_low, self.surface_container, self.surface_container_high];
        let worst = |c: Color| surfaces.iter().map(|s| c.contrast_ratio(*s)).fold(f32::INFINITY, f32::min);
        let step = if dark { HC_STEP } else { -HC_STEP };
        for token in [&mut self.on_surface, &mut self.on_surface_variant] {
            while worst(*token) < HC_MIN_CONTRAST && (0.0..=1.0).contains(&(token.lightness() + step)) {
                *token = Color::oklcha(token.lightness() + step, token.chroma(), token.hue(), token.alpha()).gamut_mapped();
            }
        }
        // Accents must stay readable as text / icons on the base surface.
        let mut primary = self.primary;
        while primary.contrast_ratio(self.surface_base) < HC_ACCENT_CONTRAST && (0.0..=1.0).contains(&(primary.lightness() + step)) {
            primary = Color::oklcha(primary.lightness() + step, primary.chroma(), primary.hue(), primary.alpha()).gamut_mapped();
        }
        self.primary = primary;
        self.rederive_accents();
    }

    /// `(token_name, color)` pairs in declaration order.
    pub fn tokens(&self) -> Vec<(&'static str, Color)> {
        macro_rules! list {
            ($($field:ident),*) => { vec![$((stringify!($field), self.$field)),*] };
        }
        palette_tokens!(list)
    }

    /// Looks a token up by name.
    pub fn token(&self, name: &str) -> Option<Color> {
        self.tokens().into_iter().find(|(n, _)| *n == name).map(|(_, c)| c)
    }

    /// Sets a token by name; returns `false` for unknown names.
    pub fn set_token(&mut self, name: &str, color: Color) -> bool {
        macro_rules! set {
            ($($field:ident),*) => {
                match name {
                    $(stringify!($field) => { self.$field = color; true })*
                    _ => false,
                }
            };
        }
        palette_tokens!(set)
    }

    /// WCAG contrast of `on_surface` against the opaque result of the glass
    /// tint over `surface_dim` — the worst realistic case for bar text.
    pub fn glass_text_contrast(&self) -> f32 {
        let backdrop = self.surface_glass_tint.over(self.surface_dim);
        self.on_surface.contrast_ratio(backdrop)
    }
}

/// Mixes two lightness values; exported for ladders built elsewhere.
pub fn mix_lightness(a: f32, b: f32, t: f32) -> f32 {
    lerp(a, b, t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog;

    #[test]
    fn surface_ladder_is_monotonic_for_dark_themes() {
        for t in catalog::builtin() {
            let p = t.palette(true);
            let ladder = [p.surface_base, p.surface_container_low, p.surface_container, p.surface_container_high];
            for pair in ladder.windows(2) {
                assert!(pair[0].lightness() <= pair[1].lightness() + 1e-3, "{}: dark ladder not ascending", p.name);
            }
        }
    }

    #[test]
    fn light_ladder_descends() {
        for t in catalog::builtin() {
            let p = t.palette(false);
            assert!(!p.is_dark);
            assert!(p.surface_container_high.lightness() <= p.surface_base.lightness() + 1e-3, "{}", p.name);
        }
    }

    #[test]
    fn accent_gradient_rotates_hue_by_25() {
        let p = catalog::find("mandara-corniche").unwrap().palette(true);
        let delta = crate::color::shortest_hue_delta(p.accent_gradient_start.hue(), p.accent_gradient_end.hue());
        assert!((delta - 25.0).abs() < 1.5, "delta = {delta}");
    }

    #[test]
    fn lerp_endpoints_and_token_access() {
        let a = catalog::find("mandara-corniche").unwrap().palette(true);
        let b = catalog::find("montaza-twilight").unwrap().palette(true);
        assert_eq!(a.lerp(&b, 0.0).primary.to_hex(), a.primary.to_hex());
        assert_eq!(a.lerp(&b, 1.0).primary.to_hex(), b.primary.to_hex());
        assert_eq!(a.tokens().len(), 22);
        let mut c = a.clone();
        assert!(c.set_token("primary", b.primary));
        assert_eq!(c.token("primary"), Some(b.primary));
        assert!(!c.set_token("nope", b.primary));
    }

    #[test]
    fn high_contrast_reaches_aaa_on_every_surface() {
        for t in catalog::builtin() {
            for dark in [true, false] {
                let p = t.palette(dark).adjusted(&PaletteAdjustments { high_contrast: true, ..Default::default() });
                for s in [p.surface_dim, p.surface_base, p.surface_container_low, p.surface_container, p.surface_container_high] {
                    assert!(p.on_surface.contrast_ratio(s) >= HC_MIN_CONTRAST, "{} dark={dark}: on_surface {}", p.name, p.on_surface.contrast_ratio(s));
                    assert!(p.on_surface_variant.contrast_ratio(s) >= HC_MIN_CONTRAST - 0.05, "{} dark={dark}: variant {}", p.name, p.on_surface_variant.contrast_ratio(s));
                }
            }
        }
    }

    #[test]
    fn adjustments_shift_hue_and_glass() {
        let p = catalog::find("mandara-corniche").unwrap().palette(true);
        let adj = PaletteAdjustments { hue_shift: 30.0, glass_opacity: Some(0.4), ..Default::default() };
        let q = p.adjusted(&adj);
        let d = crate::color::shortest_hue_delta(p.primary.hue(), q.primary.hue());
        assert!((d - 30.0).abs() < 2.0);
        assert!((q.surface_glass_tint.alpha() - 0.4).abs() < 1e-4);
        assert_eq!(p.adjusted(&PaletteAdjustments::default()), p);
    }
}
