//! Perceptual color primitive used by every Mandara design token.
//!
//! [`Color`] is a thin newtype over [`palette::Oklcha`]. All tweening, hue
//! rotation and chroma shaping happens in OKLCH so transitions keep their
//! saturation instead of collapsing through muddy grays; sRGB is only produced
//! at the very edge (rendering, hex export) with CSS Color 4 style chroma
//! reduction for gamut mapping.

use std::fmt;
use std::str::FromStr;

use palette::convert::FromColorUnclamped;
use palette::{FromColor, IsWithinBounds, Oklab, Oklch, Oklcha, Srgb, Srgba};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Below this chroma a color is treated as achromatic: its hue is meaningless
/// and must not drive hue interpolation.
const ACHROMATIC_EPSILON: f32 = 0.0025;

/// A color expressed in OKLCH with straight (non-premultiplied) alpha.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Color(pub Oklcha<f32>);

/// Errors produced when parsing hex color strings.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ColorParseError {
    #[error("hex color must start with '#': {0:?}")]
    MissingHash(String),
    #[error("hex color must have 3, 4, 6 or 8 digits: {0:?}")]
    BadLength(String),
    #[error("invalid hex digit in {0:?}")]
    BadDigit(String),
}

impl Color {
    /// Fully transparent black.
    pub const TRANSPARENT: Color = Color(Oklcha::new_const(0.0, 0.0, palette::OklabHue::new(0.0), 0.0));

    /// Builds a color from OKLCH components. `hue` is in degrees.
    pub fn oklch(lightness: f32, chroma: f32, hue: f32) -> Self {
        Self::oklcha(lightness, chroma, hue, 1.0)
    }

    /// Builds a color from OKLCH components plus alpha. `hue` is in degrees.
    pub fn oklcha(lightness: f32, chroma: f32, hue: f32, alpha: f32) -> Self {
        Color(Oklcha::new(
            lightness.clamp(0.0, 1.0),
            chroma.max(0.0),
            normalize_hue(hue),
            alpha.clamp(0.0, 1.0),
        ))
    }

    /// Builds a color from 8-bit sRGB channels.
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: u8) -> Self {
        let srgb = Srgb::new(r, g, b).into_format::<f32>();
        let lch = Oklch::from_color(srgb.into_linear());
        Self::oklcha(lch.l, lch.chroma, lch.hue.into_positive_degrees(), a as f32 / 255.0)
    }

    /// Builds a color from a packed `0xAARRGGBB` value (Material's ARGB layout).
    pub fn from_argb(argb: u32) -> Self {
        let [a, r, g, b] = argb.to_be_bytes();
        Self::from_rgba8(r, g, b, a)
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
    pub fn from_hex(hex: &str) -> Result<Self, ColorParseError> {
        let digits = hex
            .strip_prefix('#')
            .ok_or_else(|| ColorParseError::MissingHash(hex.to_owned()))?;
        let nibble = |c: u8| -> Result<u8, ColorParseError> {
            (c as char)
                .to_digit(16)
                .map(|d| d as u8)
                .ok_or_else(|| ColorParseError::BadDigit(hex.to_owned()))
        };
        let bytes = digits.as_bytes();
        let channels: Vec<u8> = match bytes.len() {
            3 | 4 => bytes
                .iter()
                .map(|&c| nibble(c).map(|n| n * 17))
                .collect::<Result<_, _>>()?,
            6 | 8 => bytes
                .chunks(2)
                .map(|pair| Ok(nibble(pair[0])? * 16 + nibble(pair[1])?))
                .collect::<Result<_, _>>()?,
            _ => return Err(ColorParseError::BadLength(hex.to_owned())),
        };
        let alpha = channels.get(3).copied().unwrap_or(255);
        Ok(Self::from_rgba8(channels[0], channels[1], channels[2], alpha))
    }

    /// Like [`Color::from_hex`] but panics on malformed input. Intended for
    /// compile-time-known preset literals only.
    pub fn hex(hex: &str) -> Self {
        Self::from_hex(hex).unwrap_or_else(|e| panic!("invalid preset color: {e}"))
    }

    pub fn lightness(&self) -> f32 {
        self.0.l
    }

    pub fn chroma(&self) -> f32 {
        self.0.chroma
    }

    /// Hue in degrees, `0..360`.
    pub fn hue(&self) -> f32 {
        self.0.hue.into_positive_degrees()
    }

    pub fn alpha(&self) -> f32 {
        self.0.alpha
    }

    pub fn is_achromatic(&self) -> bool {
        self.0.chroma < ACHROMATIC_EPSILON
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self::oklcha(self.lightness(), self.chroma(), self.hue(), alpha)
    }

    pub fn with_lightness(self, lightness: f32) -> Self {
        Self::oklcha(lightness, self.chroma(), self.hue(), self.alpha())
    }

    pub fn with_chroma(self, chroma: f32) -> Self {
        Self::oklcha(self.lightness(), chroma, self.hue(), self.alpha())
    }

    pub fn with_hue(self, hue: f32) -> Self {
        Self::oklcha(self.lightness(), self.chroma(), hue, self.alpha())
    }

    /// Rotates the hue by `degrees` (positive = counter-clockwise on the wheel).
    pub fn rotate_hue(self, degrees: f32) -> Self {
        self.with_hue(self.hue() + degrees)
    }

    /// Adds `delta` to OKLCH lightness.
    pub fn lighten(self, delta: f32) -> Self {
        self.with_lightness(self.lightness() + delta)
    }

    /// Multiplies chroma by `factor`, then gamut-maps so the result stays
    /// displayable.
    pub fn scale_chroma(self, factor: f32) -> Self {
        self.with_chroma(self.chroma() * factor).gamut_mapped()
    }

    /// Multiplies alpha by `factor`.
    pub fn fade(self, factor: f32) -> Self {
        self.with_alpha(self.alpha() * factor)
    }

    /// Interpolates in OKLCH along the shortest hue arc.
    ///
    /// When one endpoint is achromatic its hue is borrowed from the other, so a
    /// fade from gray to teal never sweeps through unrelated hues.
    pub fn mix(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let (h0, h1) = match (self.is_achromatic(), other.is_achromatic()) {
            (true, false) => (other.hue(), other.hue()),
            (false, true) => (self.hue(), self.hue()),
            _ => (self.hue(), self.hue() + shortest_hue_delta(self.hue(), other.hue())),
        };
        Color::oklcha(
            lerp(self.lightness(), other.lightness(), t),
            lerp(self.chroma(), other.chroma(), t),
            lerp(h0, h1, t),
            lerp(self.alpha(), other.alpha(), t),
        )
    }

    /// Returns `true` when the color is inside the sRGB gamut.
    pub fn in_srgb_gamut(&self) -> bool {
        let rgb = Srgb::from_color_unclamped(Oklch::new(self.0.l, self.0.chroma, self.0.hue));
        // Allow for float noise right at the boundary.
        let eps = 1e-4;
        rgb.red > -eps
            && rgb.red < 1.0 + eps
            && rgb.green > -eps
            && rgb.green < 1.0 + eps
            && rgb.blue > -eps
            && rgb.blue < 1.0 + eps
    }

    /// Maps the color into the sRGB gamut by reducing chroma at constant
    /// lightness and hue (CSS Color 4 algorithm, binary search with a
    /// just-noticeable-difference clip shortcut).
    pub fn gamut_mapped(self) -> Color {
        if self.in_srgb_gamut() {
            return self;
        }
        let (l, h, a) = (self.lightness(), self.hue(), self.alpha());
        if l >= 1.0 {
            return Color::oklcha(1.0, 0.0, h, a);
        }
        if l <= 0.0 {
            return Color::oklcha(0.0, 0.0, h, a);
        }
        const JND: f32 = 0.02;
        let (mut lo, mut hi) = (0.0_f32, self.chroma());
        while hi - lo > 1e-4 {
            let mid = (lo + hi) * 0.5;
            let candidate = Color::oklcha(l, mid, h, a);
            if candidate.in_srgb_gamut() {
                lo = mid;
            } else {
                let clipped = candidate.clipped_srgb_oklch();
                if delta_e_ok(candidate, clipped) < JND {
                    return clipped.with_alpha(a);
                }
                hi = mid;
            }
        }
        Color::oklcha(l, lo, h, a)
    }

    /// Naive per-channel clip into sRGB, expressed back in OKLCH.
    fn clipped_srgb_oklch(self) -> Color {
        let rgb = Srgb::from_color_unclamped(Oklch::new(self.0.l, self.0.chroma, self.0.hue));
        let clipped = Srgb::new(
            rgb.red.clamp(0.0, 1.0),
            rgb.green.clamp(0.0, 1.0),
            rgb.blue.clamp(0.0, 1.0),
        );
        let lch = Oklch::from_color(clipped);
        Color::oklcha(lch.l, lch.chroma, lch.hue.into_positive_degrees(), self.alpha())
    }

    /// Gamut-mapped sRGB with float channels in `0..=1`.
    pub fn to_srgba(self) -> Srgba<f32> {
        let mapped = self.gamut_mapped();
        let rgb = Srgb::from_color_unclamped(Oklch::new(mapped.0.l, mapped.0.chroma, mapped.0.hue));
        let rgb = if rgb.is_within_bounds() {
            rgb
        } else {
            Srgb::new(
                rgb.red.clamp(0.0, 1.0),
                rgb.green.clamp(0.0, 1.0),
                rgb.blue.clamp(0.0, 1.0),
            )
        };
        Srgba::new(rgb.red, rgb.green, rgb.blue, mapped.alpha())
    }

    /// Gamut-mapped 8-bit sRGB channels `[r, g, b, a]`.
    pub fn to_rgba8(self) -> [u8; 4] {
        let c = self.to_srgba();
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(c.red), q(c.green), q(c.blue), q(c.alpha)]
    }

    /// Packed `0xRRGGBBAA`.
    pub fn to_rgba_u32(self) -> u32 {
        u32::from_be_bytes(self.to_rgba8())
    }

    /// Packed `0xAARRGGBB` (Material / Hyprland layout).
    pub fn to_argb_u32(self) -> u32 {
        let [r, g, b, a] = self.to_rgba8();
        u32::from_be_bytes([a, r, g, b])
    }

    /// `#rrggbb` when opaque, `#rrggbbaa` otherwise.
    pub fn to_hex(self) -> String {
        let [r, g, b, a] = self.to_rgba8();
        if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }

    /// `rrggbb` without the hash and without alpha (for configs like Hyprland's
    /// `rgb(...)` or Ghostty palettes).
    pub fn to_hex_rgb_bare(self) -> String {
        let [r, g, b, _] = self.to_rgba8();
        format!("{r:02x}{g:02x}{b:02x}")
    }

    /// WCAG 2.x relative luminance of the opaque color.
    pub fn relative_luminance(self) -> f32 {
        let linear = Srgb::new(self.to_srgba().red, self.to_srgba().green, self.to_srgba().blue)
            .into_linear::<f32>();
        0.2126 * linear.red + 0.7152 * linear.green + 0.0722 * linear.blue
    }

    /// WCAG 2.x contrast ratio between two opaque colors, `1.0..=21.0`.
    pub fn contrast_ratio(self, other: Color) -> f32 {
        let (a, b) = (self.relative_luminance(), other.relative_luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Composites `self` over an opaque `backdrop` in linear sRGB, returning
    /// the opaque result. Used to measure contrast of glass surfaces.
    pub fn over(self, backdrop: Color) -> Color {
        let top = self.to_srgba();
        let bottom = backdrop.to_srgba();
        let a = top.alpha;
        let tl = Srgb::new(top.red, top.green, top.blue).into_linear::<f32>();
        let bl = Srgb::new(bottom.red, bottom.green, bottom.blue).into_linear::<f32>();
        let mixed = palette::LinSrgb::new(
            tl.red * a + bl.red * (1.0 - a),
            tl.green * a + bl.green * (1.0 - a),
            tl.blue * a + bl.blue * (1.0 - a),
        );
        let lch = Oklch::from_color(mixed);
        Color::oklch(lch.l, lch.chroma, lch.hue.into_positive_degrees())
    }

    /// Picks whichever of `light` / `dark` has better contrast on `self`.
    pub fn best_on(self, light: Color, dark: Color) -> Color {
        if self.contrast_ratio(light) >= self.contrast_ratio(dark) {
            light
        } else {
            dark
        }
    }

    /// Material-style harmonization: rotates this color's hue toward
    /// `source` by half the angular distance, capped at 15°.
    pub fn harmonize_toward(self, source: Color) -> Color {
        if self.is_achromatic() || source.is_achromatic() {
            return self;
        }
        let delta = shortest_hue_delta(self.hue(), source.hue());
        let rotation = (delta * 0.5).clamp(-15.0, 15.0);
        self.rotate_hue(rotation)
    }

    /// Linear-light Oklab representation (useful for gradient math).
    pub fn to_oklab(self) -> Oklab<f32> {
        Oklab::from_color(Oklch::new(self.0.l, self.0.chroma, self.0.hue))
    }
}

impl Default for Color {
    fn default() -> Self {
        Color::TRANSPARENT
    }
}

impl From<Oklch<f32>> for Color {
    fn from(value: Oklch<f32>) -> Self {
        Color::oklch(value.l, value.chroma, value.hue.into_positive_degrees())
    }
}

impl From<Oklcha<f32>> for Color {
    fn from(value: Oklcha<f32>) -> Self {
        Color(value)
    }
}

impl From<Color> for Oklch<f32> {
    fn from(value: Color) -> Self {
        Oklch::new(value.0.l, value.0.chroma, value.0.hue)
    }
}

impl From<Color> for Oklcha<f32> {
    fn from(value: Color) -> Self {
        value.0
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for Color {
    type Err = ColorParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Color::from_hex(s.trim())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Color::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Signed shortest angular distance from `from` to `to`, in `-180..=180`.
pub fn shortest_hue_delta(from: f32, to: f32) -> f32 {
    let mut d = (to - from) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

/// Wraps degrees into `0..360`.
pub fn normalize_hue(hue: f32) -> f32 {
    let h = hue % 360.0;
    if h < 0.0 { h + 360.0 } else { h }
}

/// Euclidean distance in Oklab (ΔE_OK).
pub fn delta_e_ok(a: Color, b: Color) -> f32 {
    let (a, b) = (a.to_oklab(), b.to_oklab());
    ((a.l - b.l).powi(2) + (a.a - b.a).powi(2) + (a.b - b.b).powi(2)).sqrt()
}

#[inline]
pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;

    #[test]
    fn hex_round_trip() {
        for hex in ["#000000", "#ffffff", "#1e1e2e", "#cba6f7", "#31748f", "#ff9e6480"] {
            assert_eq!(Color::hex(hex).to_hex(), hex);
        }
        assert_eq!(Color::hex("#fff").to_hex(), "#ffffff");
        assert!(Color::from_hex("123456").is_err());
        assert!(Color::from_hex("#12345").is_err());
        assert!(Color::from_hex("#zzzzzz").is_err());
    }

    #[test]
    fn known_oklch_values() {
        // Pure white is L=1, C=0 in OKLCH.
        let white = Color::hex("#ffffff");
        assert_abs_diff_eq!(white.lightness(), 1.0, epsilon = 1e-3);
        assert!(white.is_achromatic());
        // sRGB red ≈ oklch(0.628 0.258 29.2).
        let red = Color::hex("#ff0000");
        assert_abs_diff_eq!(red.lightness(), 0.628, epsilon = 2e-3);
        assert_abs_diff_eq!(red.chroma(), 0.2577, epsilon = 2e-3);
        assert_abs_diff_eq!(red.hue(), 29.23, epsilon = 0.5);
    }

    #[test]
    fn shortest_hue_path() {
        assert_abs_diff_eq!(shortest_hue_delta(350.0, 10.0), 20.0);
        assert_abs_diff_eq!(shortest_hue_delta(10.0, 350.0), -20.0);
        let a = Color::oklch(0.7, 0.12, 350.0);
        let b = Color::oklch(0.7, 0.12, 10.0);
        let mid = a.mix(b, 0.5);
        assert!(mid.hue() < 1.0 || mid.hue() > 359.0, "hue went the long way: {}", mid.hue());
        // Chroma must be preserved mid-flight (no muddy gray).
        assert_abs_diff_eq!(mid.chroma(), 0.12, epsilon = 1e-4);
    }

    #[test]
    fn achromatic_mix_borrows_hue() {
        let gray = Color::oklch(0.5, 0.0, 0.0);
        let teal = Color::oklch(0.7, 0.1, 190.0);
        assert_abs_diff_eq!(gray.mix(teal, 0.3).hue(), 190.0, epsilon = 1e-3);
    }

    #[test]
    fn gamut_mapping_preserves_hue_and_lightness() {
        let wild = Color::oklch(0.7, 0.4, 150.0);
        assert!(!wild.in_srgb_gamut());
        let mapped = wild.gamut_mapped();
        assert!(mapped.chroma() < 0.4);
        assert_abs_diff_eq!(mapped.lightness(), 0.7, epsilon = 0.02);
        assert_abs_diff_eq!(mapped.hue(), 150.0, epsilon = 2.0);
    }

    #[test]
    fn wcag_contrast() {
        let ratio = Color::hex("#000000").contrast_ratio(Color::hex("#ffffff"));
        assert_abs_diff_eq!(ratio, 21.0, epsilon = 0.05);
        let same = Color::hex("#777777").contrast_ratio(Color::hex("#777777"));
        assert_abs_diff_eq!(same, 1.0, epsilon = 1e-4);
    }

    #[test]
    fn serde_as_hex() {
        #[derive(Serialize, Deserialize)]
        struct W {
            c: Color,
        }
        let w: W = serde_json::from_str(r##"{"c":"#cba6f7"}"##).unwrap();
        assert_eq!(w.c.to_hex(), "#cba6f7");
        assert_eq!(serde_json::to_string(&w).unwrap(), r##"{"c":"#cba6f7"}"##);
    }
}
