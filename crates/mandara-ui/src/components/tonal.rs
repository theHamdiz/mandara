//! Material 3's tonal container roles, derived from a Mandara palette.
//!
//! The palette carries Material's surfaces, `primary_container` and the
//! accents; the Material You login and lock designs also need the
//! container / on-container pairs Material builds its tonal buttons, chips
//! and fields from. They are derived here in OKLCH from the palette's own
//! hues, so they follow every theme and every wallpaper-made palette:
//! containers are deep (dark themes) or pale (light themes) tones of their
//! accent, and their content is the opposite tone of the same hue.
//!
//! Pure and allocation-free; [`Tonal::of`] is cheap enough to call per
//! frame, and callers that keep a palette keep its roles alongside.

use mandara_theme::{Color, MandaraPalette};

/// Container lightness on dark and light themes (Material's tones 30 / 90).
const CONTAINER_L: (f32, f32) = (0.34, 0.92);
/// Content on a container (tones 90 / 10).
const ON_CONTAINER_L: (f32, f32) = (0.93, 0.22);
/// Share of the accent's chroma a container keeps, and its content.
const CONTAINER_CHROMA: f32 = 0.7;
const ON_CONTAINER_CHROMA: f32 = 0.45;
/// The highest surface: this much lighter (dark) or darker (light) than
/// the high one.
const HIGHEST_STEP: f32 = 0.035;

/// Lightness of a picture from which white text over it gets an extra
/// scrim, the scrim per unit of lightness above it, and its strongest.
const LIGHT_FROM: f32 = 0.4;
const SCRIM_PER_LUMA: f32 = 1.0;
const SCRIM_MAX: f32 = 0.4;

/// The extra dark scrim (alpha) that keeps light text readable over a
/// picture whose mean luma is `luma` (0–1): none over a dark one.
pub fn scrim_for_luma(luma: f32) -> f32 {
    ((luma - LIGHT_FROM) * SCRIM_PER_LUMA).clamp(0.0, SCRIM_MAX)
}

/// Material's tonal roles for one palette.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Tonal {
    pub primary_container: Color,
    pub on_primary_container: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,
    pub tertiary_container: Color,
    pub on_tertiary_container: Color,
    /// `surface_container_highest`: filled fields and chips.
    pub surface_highest: Color,
}

impl Tonal {
    /// The roles of `p`.
    pub fn of(p: &MandaraPalette) -> Tonal {
        let pick = |(dark, light): (f32, f32)| if p.is_dark { dark } else { light };
        let container = |c: Color| Color::oklch(pick(CONTAINER_L), c.chroma() * CONTAINER_CHROMA, c.hue()).gamut_mapped();
        let on = |c: Color| Color::oklch(pick(ON_CONTAINER_L), c.chroma() * ON_CONTAINER_CHROMA, c.hue()).gamut_mapped();
        let step = if p.is_dark { HIGHEST_STEP } else { -HIGHEST_STEP };
        // One step up the ladder, unless that costs the text its contrast
        // (a light theme whose text is only just readable on its high one).
        let high = p.surface_container_high.with_alpha(1.0);
        let stepped = high.lighten(step);
        let enough = 4.5_f32.min(p.on_surface.contrast_ratio(high));
        let surface_highest = if p.on_surface.contrast_ratio(stepped) >= enough { stepped } else { high };
        Tonal {
            primary_container: container(p.primary),
            on_primary_container: on(p.primary),
            secondary_container: container(p.secondary),
            on_secondary_container: on(p.secondary),
            tertiary_container: container(p.tertiary),
            on_tertiary_container: on(p.tertiary),
            surface_highest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every container keeps its content readable (WCAG AA for text) in
    /// every built-in theme, dark and light, and in wallpaper-made ones.
    #[test]
    fn containers_keep_their_content_readable() {
        let mut palettes: Vec<MandaraPalette> = Vec::new();
        for def in mandara_theme::catalog::all() {
            palettes.push(def.palette(true));
            palettes.push(def.palette(false));
        }
        for seed in ["#e5383b", "#2b9348", "#3a86ff", "#ffbe0b", "#8338ec", "#9e9e9e"] {
            for dark in [true, false] {
                palettes.push(mandara_theme::material::palette_from_seed(Color::hex(seed), mandara_theme::material::SchemeVariant::TonalSpot, dark));
            }
        }
        for p in &palettes {
            let t = Tonal::of(p);
            for (bg, fg, role) in [
                (t.primary_container, t.on_primary_container, "primary"),
                (t.secondary_container, t.on_secondary_container, "secondary"),
                (t.tertiary_container, t.on_tertiary_container, "tertiary"),
            ] {
                let ratio = fg.contrast_ratio(bg);
                assert!(ratio >= 4.5, "{} ({}): {role} container contrast {ratio:.2}", p.name, if p.is_dark { "dark" } else { "light" });
            }
            // Text stays readable (AA) on the highest surface, or at least as
            // readable as the theme makes it on its own high one.
            let (highest, high) = (p.on_surface.contrast_ratio(t.surface_highest), p.on_surface.contrast_ratio(p.surface_container_high));
            assert!(highest >= 4.5_f32.min(high) - 1e-3, "{}: text on the highest surface {highest:.2} (high {high:.2})", p.name);
        }
    }
}
