//! Mandara surface materials: OKLCH → GPUI color bridge, chromatic glass,
//! 1px directional specular rims and anti-banding micro-grain.

use std::sync::{Arc, OnceLock};

use gpui::{
    Background, BoxShadow, ColorSpace, Corners, Hsla, ImageSource, IntoElement, ObjectFit, Pixels, RenderImage, Rgba,
    Styled, StyledImage as _, img, linear_color_stop, linear_gradient, point, px,
};
use mandara_theme::{Color, MandaraPalette};

/// Converts OKLCH design tokens to GPUI colors (gamut-mapped sRGB).
pub trait ToGpui {
    fn rgba(self) -> Rgba;
    fn hsla(self) -> Hsla;
}

impl ToGpui for Color {
    fn rgba(self) -> Rgba {
        let c = self.to_srgba();
        Rgba { r: c.red, g: c.green, b: c.blue, a: c.alpha }
    }

    fn hsla(self) -> Hsla {
        self.rgba().into()
    }
}

/// A two-stop linear gradient interpolated in Oklab on the GPU.
/// `angle` follows CSS: `0` points up, increasing clockwise.
pub fn oklab_gradient(angle: f32, from: Color, to: Color) -> Background {
    linear_gradient(angle, linear_color_stop(from.hsla(), 0.0), linear_color_stop(to.hsla(), 1.0))
        .color_space(ColorSpace::Oklab)
}

/// Vertical offset of the 1px specular rim, in px.
const RIM_OFFSET_PX: f32 = 1.0;
/// Blur of the crisp top highlight / the soft bottom shade, in px.
const RIM_LIGHT_BLUR_PX: f32 = 0.5;
const RIM_SHADOW_BLUR_PX: f32 = 1.5;
/// OKLCH lightness shifts of the glass sheen (top brighter, bottom deeper).
const SHEEN_TOP_LIGHTEN: f32 = 0.025;
const SHEEN_BOTTOM_LIGHTEN: f32 = -0.01;
/// CSS angle of a top-to-bottom gradient, in degrees.
const TOP_TO_BOTTOM_DEG: f32 = 180.0;

/// Precision-milled glass edge as SDF inset shadows: a crisp specular
/// highlight along the inner top edge and a soft shade along the inner
/// bottom. Anti-aliased like any rounded rect — no tessellated rings.
pub fn glass_rim(light: Color, shadow: Color) -> Vec<BoxShadow> {
    glass_rim_width(light, shadow, RIM_OFFSET_PX)
}

/// [`glass_rim`] with a rim `width` px thick (0 draws none).
pub fn glass_rim_width(light: Color, shadow: Color, width: f32) -> Vec<BoxShadow> {
    if width <= 0.0 {
        return Vec::new();
    }
    vec![
        BoxShadow {
            color: light.hsla(),
            offset: point(px(0.0), px(width)),
            blur_radius: px(RIM_LIGHT_BLUR_PX),
            spread_radius: px(0.0),
            inset: true,
        },
        BoxShadow {
            color: shadow.hsla(),
            offset: point(px(0.0), px(-width)),
            blur_radius: px(RIM_SHADOW_BLUR_PX),
            spread_radius: px(0.0),
            inset: true,
        },
    ]
}

/// Corner radii helper.
pub fn corners(r: Pixels) -> Corners<Pixels> {
    Corners::all(r)
}

/// Glass fill for a floating surface: the tint layered over the compositor
/// blur, with a faint vertical sheen so the pill reads as curved glass.
pub fn glass_fill(palette: &MandaraPalette) -> Background {
    let top = palette.surface_glass_tint.lighten(SHEEN_TOP_LIGHTEN);
    let bottom = palette.surface_glass_tint.lighten(SHEEN_BOTTOM_LIGHTEN);
    oklab_gradient(TOP_TO_BOTTOM_DEG, top, bottom)
}

/// Side length of the procedural grain tile.
const GRAIN_TILE: u32 = 256;
/// Alpha of each grain pixel: 4/255 ≈ 1.5 % opacity.
const GRAIN_ALPHA: u8 = 4;
/// xorshift32 seed (golden-ratio constant) and shift triple (13, 17, 5).
const GRAIN_SEED: u32 = 0x9E37_79B9;
const XORSHIFT_A: u32 = 13;
const XORSHIFT_B: u32 = 17;
const XORSHIFT_C: u32 = 5;
/// Random bit that lowers a grain pixel's alpha by one step.
const GRAIN_ALPHA_BIT: u32 = 8;

/// A cached tile of per-pixel ±luminance noise used to dither dark gradients.
pub fn grain_image() -> Arc<RenderImage> {
    static GRAIN: OnceLock<Arc<RenderImage>> = OnceLock::new();
    GRAIN
        .get_or_init(|| {
            // xorshift32 — deterministic, dependency-free and plenty random
            // for visual dithering.
            let mut state: u32 = GRAIN_SEED;
            let mut next = || {
                state ^= state << XORSHIFT_A;
                state ^= state >> XORSHIFT_B;
                state ^= state << XORSHIFT_C;
                state
            };
            let buffer = image::RgbaImage::from_fn(GRAIN_TILE, GRAIN_TILE, |_, _| {
                let n = next();
                let v = if n & 1 == 0 { u8::MAX } else { 0 };
                // Vary alpha slightly so the grain has no visible lattice.
                let a = GRAIN_ALPHA.saturating_sub(u8::from((n >> GRAIN_ALPHA_BIT) & 1 == 1));
                // GPUI expects BGRA; the channels are symmetric here.
                image::Rgba([v, v, v, a])
            });
            Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)]))
        })
        .clone()
}

/// Side length of the ink texture tiles (drawn once, cached), their dot /
/// line pitch and their ink alpha.
const INK_TILE: u32 = 384;
const INK_PITCH: f32 = 9.0;
const INK_ALPHA: f32 = 0.22;
/// Where the shade starts along the tile's diagonal (0 = top-left corner).
const INK_SHADE_FROM: f32 = 0.45;

/// The ink texture tiles: comic halftone dots that grow toward the
/// bottom-right corner, or pen hatching that thickens there. Black with
/// alpha, one shared image each, drawn on first use.
pub fn ink_texture(hatch: bool) -> Arc<RenderImage> {
    static HALFTONE: OnceLock<Arc<RenderImage>> = OnceLock::new();
    static HATCH: OnceLock<Arc<RenderImage>> = OnceLock::new();
    let cell = if hatch { &HATCH } else { &HALFTONE };
    cell.get_or_init(|| {
        let n = INK_TILE as f32;
        let buffer = image::RgbaImage::from_fn(INK_TILE, INK_TILE, |x, y| {
            let (fx, fy) = (x as f32, y as f32);
            // 0 at the shade's start, 1 in the far corner.
            let shade = (((fx + fy) / (2.0 * n) - INK_SHADE_FROM) / (1.0 - INK_SHADE_FROM)).clamp(0.0, 1.0);
            let coverage = if hatch {
                // 45° lines whose width follows the shade.
                let d = ((fx - fy).rem_euclid(INK_PITCH) - INK_PITCH / 2.0).abs();
                let half = shade * INK_PITCH * 0.32;
                (half + 0.5 - d).clamp(0.0, 1.0)
            } else {
                // Dots on a 45° grid whose radius follows the shade.
                let (u, v) = ((fx + fy) / std::f32::consts::SQRT_2, (fx - fy) / std::f32::consts::SQRT_2);
                let (cu, cv) = ((u / INK_PITCH).round() * INK_PITCH, (v / INK_PITCH).round() * INK_PITCH);
                let d = ((u - cu).powi(2) + (v - cv).powi(2)).sqrt();
                let r = shade * INK_PITCH * 0.42;
                (r + 0.5 - d).clamp(0.0, 1.0)
            };
            image::Rgba([0, 0, 0, (coverage * INK_ALPHA * 255.0).round() as u8])
        });
        Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buffer)]))
    })
    .clone()
}

/// Full-bleed grain overlay element.
pub fn grain_overlay() -> impl IntoElement {
    img(ImageSource::Render(grain_image()))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .object_fit(ObjectFit::Fill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_bridge_round_trips_through_rgba() {
        let c = Color::hex("#2ec4b6");
        let rgba = c.rgba();
        assert!((rgba.r - 0x2e as f32 / 255.0).abs() < 0.003);
        assert!((rgba.g - 0xc4 as f32 / 255.0).abs() < 0.003);
        assert!((rgba.a - 1.0).abs() < 1e-6);
        let translucent = c.with_alpha(0.5).hsla();
        assert!((translucent.a - 0.5).abs() < 0.003);
    }

    #[test]
    fn ink_textures_are_cached_and_shade_one_corner() {
        for hatch in [false, true] {
            let a = ink_texture(hatch);
            assert!(Arc::ptr_eq(&a, &ink_texture(hatch)), "drawn once");
        }
        assert!(!Arc::ptr_eq(&ink_texture(false), &ink_texture(true)));
    }

    #[test]
    fn grain_tile_is_cached() {
        let a = grain_image();
        let b = grain_image();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
