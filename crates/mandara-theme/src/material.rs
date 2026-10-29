//! CAM16/HCT Material 3 bridge: wallpaper / album-art quantization and
//! dynamic tonal schemes mapped onto Mandara tokens.

use std::path::Path;

use material_colors::color::Rgb;
use material_colors::dynamic_color::dynamic_scheme::DynamicScheme;
use material_colors::dynamic_color::variant::Variant;
use material_colors::hct::Hct;
use material_colors::quantize::{Quantizer, QuantizerCelebi};
use material_colors::scheme::Scheme;
use material_colors::score::Score;
use serde::{Deserialize, Serialize};

use crate::color::Color;
use crate::palette::{MandaraPalette, accent_gradient_end_for, glow_for};

/// Material 3 dynamic scheme flavours exposed in the Studio.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SchemeVariant {
    #[default]
    TonalSpot,
    Vibrant,
    Expressive,
    Fidelity,
    FruitSalad,
    Monochrome,
}

impl SchemeVariant {
    pub const ALL: [SchemeVariant; 6] = [
        SchemeVariant::TonalSpot,
        SchemeVariant::Vibrant,
        SchemeVariant::Expressive,
        SchemeVariant::Fidelity,
        SchemeVariant::FruitSalad,
        SchemeVariant::Monochrome,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            SchemeVariant::TonalSpot => "Tonal Spot",
            SchemeVariant::Vibrant => "Vibrant",
            SchemeVariant::Expressive => "Expressive",
            SchemeVariant::Fidelity => "Fidelity",
            SchemeVariant::FruitSalad => "Fruit Salad",
            SchemeVariant::Monochrome => "Monochrome",
        }
    }

    fn to_material(self) -> Variant {
        match self {
            SchemeVariant::TonalSpot => Variant::TonalSpot,
            SchemeVariant::Vibrant => Variant::Vibrant,
            SchemeVariant::Expressive => Variant::Expressive,
            SchemeVariant::Fidelity => Variant::Fidelity,
            SchemeVariant::FruitSalad => Variant::FruitSalad,
            SchemeVariant::Monochrome => Variant::Monochrome,
        }
    }
}

/// A dominant color cluster extracted from an image.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeedColor {
    pub color: Color,
    /// Fraction of sampled pixels belonging to this cluster, `0..=1`.
    pub proportion: f32,
    /// CAM16 hue / chroma / tone.
    pub hue: f32,
    pub chroma: f32,
    pub tone: f32,
}

/// Errors from image-driven color extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("failed to decode image {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: image::ImageError,
    },
    #[error("failed to decode video {path}: {reason}")]
    Video { path: String, reason: String },
    #[error("image contains no opaque pixels")]
    Empty,
}

/// Longest edge (px) images are downsampled to before quantization. Keeps
/// extraction well under 20 ms even for 8K wallpapers.
const SAMPLE_EDGE: u32 = 128;
/// Celebi (Wu + WSMeans) cluster budget, as used by Android's wallpaper picker.
const QUANTIZE_CLUSTERS: usize = 128;

fn to_rgb(c: Color) -> Rgb {
    let [r, g, b, _] = c.to_rgba8();
    Rgb::new(r, g, b)
}

fn from_rgb(rgb: Rgb) -> Color {
    Color::from_rgba8(rgb.red, rgb.green, rgb.blue, 255)
}

/// Extracts up to `count` seed colors ranked by Material's chroma/proportion
/// score from already-decoded pixels.
pub fn extract_seeds_from_image(img: &image::DynamicImage, count: usize) -> Result<Vec<SeedColor>, ExtractError> {
    seeds_from_thumbnail(&img.thumbnail(SAMPLE_EDGE, SAMPLE_EDGE).to_rgba8(), count)
}

/// Quantizes an already downscaled image.
fn seeds_from_thumbnail(thumb: &image::RgbaImage, count: usize) -> Result<Vec<SeedColor>, ExtractError> {
    let pixels: Vec<Rgb> = thumb
        .pixels()
        .filter(|p| p.0[3] == 255)
        .map(|p| Rgb::new(p.0[0], p.0[1], p.0[2]))
        .collect();
    if pixels.is_empty() {
        return Err(ExtractError::Empty);
    }
    let total = pixels.len() as f32;
    let result = QuantizerCelebi::quantize(&pixels, QUANTIZE_CLUSTERS);
    let ranked = Score::score(&result.color_to_count, Some(count.max(1)), None, Some(true));

    let mut seeds: Vec<SeedColor> = ranked
        .into_iter()
        .map(|rgb| {
            let hct = Hct::new(rgb);
            // Score may return a fallback color that isn't a quantized cluster.
            let population = result.color_to_count.get(&rgb).copied().unwrap_or(0);
            SeedColor {
                color: from_rgb(rgb),
                proportion: population as f32 / total,
                hue: hct.get_hue() as f32,
                chroma: hct.get_chroma() as f32,
                tone: hct.get_tone() as f32,
            }
        })
        .collect();
    // Material's score weighs area heavily, so a large muted sky can outrank
    // the image's character colour. Lead with colourful seeds when there
    // are any (stable, so Material's order holds within each group).
    seeds.sort_by_key(|s| s.chroma < VIVID_CHROMA);
    seeds.truncate(count);
    Ok(seeds)
}

/// CAM16 chroma from which a seed counts as colourful enough to lead.
const VIVID_CHROMA: f32 = 18.0;

/// Decodes `path` and extracts its seed colors. The full-size pixels are
/// freed as soon as the thumbnail exists, before quantization. Video
/// wallpapers (`.webm`, `.mkv`, `.ivf`) use a representative frame.
pub fn extract_seeds(path: &Path, count: usize) -> Result<Vec<SeedColor>, ExtractError> {
    let thumb = if mandara_media::is_video(path) {
        let frame = mandara_media::representative_frame(path).map_err(|reason| ExtractError::Video { path: path.display().to_string(), reason })?;
        image::DynamicImage::ImageRgba8(frame).thumbnail(SAMPLE_EDGE, SAMPLE_EDGE).to_rgba8()
    } else {
        let img = image::open(path).map_err(|source| ExtractError::Decode { path: path.display().to_string(), source })?;
        img.thumbnail(SAMPLE_EDGE, SAMPLE_EDGE).to_rgba8()
    };
    seeds_from_thumbnail(&thumb, count)
}

/// Builds a Material 3 scheme from a seed and maps it onto Mandara tokens.
pub fn palette_from_seed(seed: Color, variant: SchemeVariant, is_dark: bool) -> MandaraPalette {
    let dynamic = DynamicScheme::by_variant(to_rgb(seed), &variant.to_material(), is_dark, None);
    let scheme = Scheme::from(dynamic);
    palette_from_scheme(&scheme, is_dark, format!("Dynamic · {}", variant.display_name()))
}

/// Specular rim light: OKLCH lightness and alpha on dark / light themes,
/// the share of the primary's chroma it keeps and its chroma ceiling.
const RIM_LIGHT_L: (f32, f32) = (0.97, 1.0);
const RIM_LIGHT_ALPHA: (f32, f32) = (0.34, 0.85);
const RIM_LIGHT_CHROMA_SHARE: f32 = 0.12;
const RIM_LIGHT_CHROMA_MAX: f32 = 0.02;
/// Specular rim shadow: lightness, alpha (dark / light) and surface chroma share.
const RIM_SHADOW_L: (f32, f32) = (0.04, 0.35);
const RIM_SHADOW_ALPHA: (f32, f32) = (0.5, 0.18);
const RIM_SHADOW_CHROMA_SHARE: f32 = 0.5;
/// Status colors are darkened by this much OKLCH lightness on light themes.
const STATUS_LIGHT_THEME_DARKEN: f32 = 0.18;
/// Glass tint opacity over the base surface.
const GLASS_TINT_ALPHA: f32 = 0.62;
/// Signature status hues before harmonizing toward the primary.
const STATUS_CORAL: &str = "#ff7a6b";
const STATUS_EMERALD: &str = "#3ddc97";
const STATUS_AMBER: &str = "#ffc145";
/// Share of the signature coral mixed into Material's error color.
const CORAL_MIX: f32 = 0.5;

/// Picks the dark- or light-theme value of a `(dark, light)` pair.
fn pick((dark, light): (f32, f32), is_dark: bool) -> f32 {
    if is_dark { dark } else { light }
}

/// Maps a resolved Material scheme onto the Mandara semantic tokens.
pub fn palette_from_scheme(scheme: &Scheme, is_dark: bool, name: String) -> MandaraPalette {
    let c = |rgb: Rgb| from_rgb(rgb);
    let primary = c(scheme.primary);
    let surface_base = c(scheme.surface);
    let rim_light = Color::oklcha(
        pick(RIM_LIGHT_L, is_dark),
        (primary.chroma() * RIM_LIGHT_CHROMA_SHARE).min(RIM_LIGHT_CHROMA_MAX),
        primary.hue(),
        pick(RIM_LIGHT_ALPHA, is_dark),
    );
    let rim_shadow = Color::oklcha(pick(RIM_SHADOW_L, is_dark), surface_base.chroma() * RIM_SHADOW_CHROMA_SHARE, surface_base.hue(), pick(RIM_SHADOW_ALPHA, is_dark));
    let status = |hex: &str| {
        let base = Color::hex(hex);
        let base = if is_dark { base } else { base.lighten(-STATUS_LIGHT_THEME_DARKEN) };
        base.harmonize_toward(primary).gamut_mapped()
    };
    MandaraPalette {
        name,
        is_dark,
        surface_dim: c(scheme.surface_dim),
        surface_base,
        surface_container_low: c(scheme.surface_container_low),
        surface_container: c(scheme.surface_container),
        surface_container_high: c(scheme.surface_container_high),
        surface_glass_tint: surface_base.with_alpha(GLASS_TINT_ALPHA),
        specular_rim_light: rim_light,
        specular_rim_shadow: rim_shadow,
        on_surface: c(scheme.on_surface),
        on_surface_variant: c(scheme.on_surface_variant),
        outline: c(scheme.outline),
        primary,
        on_primary: c(scheme.on_primary),
        primary_container: c(scheme.primary_container),
        primary_glow: glow_for(primary, is_dark),
        secondary: c(scheme.secondary),
        tertiary: c(scheme.tertiary),
        accent_gradient_start: primary,
        accent_gradient_end: accent_gradient_end_for(primary),
        urgent_coral: c(scheme.error).mix(status(STATUS_CORAL), CORAL_MIX),
        success_emerald: status(STATUS_EMERALD),
        warning_amber: status(STATUS_AMBER),
    }
}

/// Mini-palette used by the Media Island (ambient glow + FFT gradient).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MiniPalette {
    pub glow: Color,
    pub gradient_start: Color,
    pub gradient_end: Color,
    pub backdrop: Color,
    pub text: Color,
}

/// CAM16 chroma a seed needs to lead the mini-palette / to partner it.
const MINI_LEAD_CHROMA: f32 = 12.0;
const MINI_PARTNER_CHROMA: f32 = 8.0;
/// Smallest hue distance (degrees) between lead and partner.
const MINI_PARTNER_MIN_HUE_DELTA: f32 = 20.0;
/// Luminous band accents are lifted into (OKLCH lightness) and their chroma boost.
const MINI_LIFT_L: (f32, f32) = (0.68, 0.86);
const MINI_LIFT_CHROMA_BOOST: f32 = 1.1;
/// Backdrop lightness, share of the lead's chroma and chroma ceiling.
const MINI_BACKDROP_L: f32 = 0.2;
const MINI_BACKDROP_CHROMA_SHARE: f32 = 0.5;
const MINI_BACKDROP_CHROMA_MAX: f32 = 0.06;
/// Ambient glow opacity.
const MINI_GLOW_ALPHA: f32 = 0.45;
/// Text candidates: (lightness, chroma) of near-white and near-black.
const MINI_TEXT_LIGHT: (f32, f32) = (0.97, 0.01);
const MINI_TEXT_DARK: (f32, f32) = (0.15, 0.02);

impl MiniPalette {
    /// Derives a mini-palette from ranked seeds; falls back to the shell
    /// palette's accents when the art is achromatic.
    pub fn from_seeds(seeds: &[SeedColor], fallback: &MandaraPalette) -> MiniPalette {
        let lead = seeds
            .iter()
            .find(|s| s.chroma > MINI_LEAD_CHROMA)
            .map(|s| s.color)
            .unwrap_or(fallback.primary);
        let partner = seeds
            .iter()
            .filter(|s| s.chroma > MINI_PARTNER_CHROMA)
            .map(|s| s.color)
            .find(|c| crate::color::shortest_hue_delta(lead.hue(), c.hue()).abs() > MINI_PARTNER_MIN_HUE_DELTA)
            .unwrap_or_else(|| accent_gradient_end_for(lead));
        // Lift accents into a luminous band so they glow on dark glass.
        let lift = |c: Color| c.with_lightness(c.lightness().clamp(MINI_LIFT_L.0, MINI_LIFT_L.1)).scale_chroma(MINI_LIFT_CHROMA_BOOST);
        let backdrop = lead.with_lightness(MINI_BACKDROP_L).with_chroma((lead.chroma() * MINI_BACKDROP_CHROMA_SHARE).min(MINI_BACKDROP_CHROMA_MAX));
        MiniPalette {
            glow: lift(lead).with_alpha(MINI_GLOW_ALPHA),
            gradient_start: lift(lead),
            gradient_end: lift(partner),
            backdrop,
            text: backdrop.best_on(Color::oklch(MINI_TEXT_LIGHT.0, MINI_TEXT_LIGHT.1, lead.hue()), Color::oklch(MINI_TEXT_DARK.0, MINI_TEXT_DARK.1, lead.hue())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};

    fn split_image(left: [u8; 3], right: [u8; 3], left_share: f32) -> DynamicImage {
        let (w, h) = (200u32, 100u32);
        let split = (w as f32 * left_share) as u32;
        DynamicImage::ImageRgba8(RgbaImage::from_fn(w, h, |x, _| {
            let c = if x < split { left } else { right };
            Rgba([c[0], c[1], c[2], 255])
        }))
    }

    #[test]
    fn extracts_dominant_clusters() {
        let img = split_image([20, 120, 200], [230, 160, 40], 0.7);
        let seeds = extract_seeds_from_image(&img, 4).unwrap();
        assert!(!seeds.is_empty());
        // The blue half dominates.
        let top = seeds[0];
        assert!((200.0..280.0).contains(&top.color.hue()), "top hue {}", top.color.hue());
        assert!(top.proportion > 0.5);
    }

    #[test]
    fn transparent_image_is_rejected() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0])));
        assert!(matches!(extract_seeds_from_image(&img, 4), Err(ExtractError::Empty)));
    }

    #[test]
    fn every_variant_builds_accessible_palettes() {
        let seed = Color::hex("#1f8a9e");
        for variant in SchemeVariant::ALL {
            for dark in [true, false] {
                let p = palette_from_seed(seed, variant, dark);
                let ratio = p.on_surface.contrast_ratio(p.surface_base);
                assert!(ratio >= 4.5, "{variant:?} dark={dark}: contrast {ratio:.2}");
                assert_eq!(p.is_dark, dark);
            }
        }
    }

    #[test]
    fn mini_palette_falls_back_for_gray_art() {
        let fallback = crate::engine::default_palette(true);
        let gray = SeedColor { color: Color::hex("#808080"), proportion: 1.0, hue: 0.0, chroma: 0.0, tone: 50.0 };
        let mini = MiniPalette::from_seeds(&[gray], &fallback);
        assert!((mini.gradient_start.hue() - fallback.primary.hue()).abs() < 3.0);
    }
}
