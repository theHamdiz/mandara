//! The working color: OKLab. Every blend, gradient and glow is mixed in
//! OKLab (perceptually even, so a blue-to-gold ramp passes through clean
//! teals and sands instead of grey), then mapped to sRGB once per pixel.
//!
//! Palette stops are parsed and converted with `palette`; the per-pixel
//! path below uses [`crate::fmath`] so output is bit-identical everywhere.

use std::sync::OnceLock;

use palette::{IntoColor, Oklab, Srgb};

use crate::fmath::{atan2, cbrt, cos, pow, sin};

/// Stop colors are rounded to this grid after `palette`'s conversion so a
/// last-bit libm difference can never change a rendered pixel.
const STOP_QUANTUM: f32 = 1e-5;
/// Entries of the sRGB transfer table (linear light → encoded).
const SRGB_LUT: usize = 4096;
/// Linear light above this is compressed smoothly toward white instead of
/// clipping (bright glows roll off like film, not like a cut).
const HIGHLIGHT_KNEE: f32 = 0.9;
/// Below this chroma a color counts as grey (no hue to keep).
const CHROMA_EPS: f32 = 1e-4;
/// [`Lab::with_chroma`] never scales chroma up by more than this.
const MAX_CHROMA_BOOST: f32 = 2.5;

/// An OKLab color: lightness `l` (0 black … 1 white) and opponent axes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

impl Lab {
    pub const fn new(l: f32, a: f32, b: f32) -> Self {
        Lab { l, a, b }
    }

    /// Mix toward `o` by `t` (0 = self, 1 = `o`).
    #[inline]
    pub fn mix(self, o: Lab, t: f32) -> Lab {
        Lab { l: self.l + (o.l - self.l) * t, a: self.a + (o.a - self.a) * t, b: self.b + (o.b - self.b) * t }
    }

    #[inline]
    pub fn scale(self, k: f32) -> Lab {
        Lab { l: self.l * k, a: self.a * k, b: self.b * k }
    }

    #[inline]
    pub fn add(self, o: Lab) -> Lab {
        Lab { l: self.l + o.l, a: self.a + o.a, b: self.b + o.b }
    }

    /// Lightness changed by `dl`, chroma kept.
    #[inline]
    pub fn lighten(self, dl: f32) -> Lab {
        Lab { l: self.l + dl, ..self }
    }

    /// Mix that keeps chroma: the hue follows the OKLab blend, but chroma is
    /// interpolated between the endpoints instead of collapsing toward grey
    /// where opposing hues meet (teal → gold stays vivid, never olive).
    #[inline]
    pub fn mix_vivid(self, o: Lab, t: f32) -> Lab {
        let m = self.mix(o, t);
        m.with_chroma(self.chroma() + (o.chroma() - self.chroma()) * t)
    }

    /// Same hue and lightness at chroma `c` (unchanged when achromatic).
    #[inline]
    pub fn with_chroma(self, c: f32) -> Lab {
        let now = self.chroma();
        if now < CHROMA_EPS {
            return self;
        }
        // Never boost more than this, so a near-grey blend of opposites
        // doesn't snap to an arbitrary saturated hue.
        let k = (c.max(0.0) / now).min(MAX_CHROMA_BOOST);
        Lab { l: self.l, a: self.a * k, b: self.b * k }
    }

    /// Light added in linear light (physically: glows, stars, lamps):
    /// `self + light × amount`, rolled off by the highlight shoulder later.
    #[inline]
    pub fn add_light(self, light: Lab, amount: f32) -> Lab {
        if amount <= 0.0 {
            return self;
        }
        let (c, l) = (self.to_linear(), light.to_linear());
        Lab::from_linear([c[0].max(0.0) + l[0].max(0.0) * amount, c[1].max(0.0) + l[1].max(0.0) * amount, c[2].max(0.0) + l[2].max(0.0) * amount])
    }

    /// Chroma multiplied by `k`.
    #[inline]
    pub fn saturate(self, k: f32) -> Lab {
        Lab { a: self.a * k, b: self.b * k, ..self }
    }

    /// OKLCh chroma.
    pub fn chroma(self) -> f32 {
        self.a.hypot(self.b)
    }

    /// OKLCh hue in radians.
    pub fn hue(self) -> f32 {
        atan2(self.b, self.a)
    }

    /// From OKLCh (hue in radians).
    pub fn from_lch(l: f32, c: f32, h: f32) -> Lab {
        Lab { l, a: c * cos(h), b: c * sin(h) }
    }

    /// Hue rotated by `rad`.
    #[inline]
    pub fn rotate(self, cos_h: f32, sin_h: f32) -> Lab {
        Lab { l: self.l, a: self.a * cos_h - self.b * sin_h, b: self.a * sin_h + self.b * cos_h }
    }

    /// Parses `#rrggbb` / `rrggbb` (and `#rgb`) with `palette`.
    pub fn from_hex(s: &str) -> Option<Lab> {
        let s = s.trim();
        let body = s.strip_prefix('#').unwrap_or(s);
        if !matches!(body.len(), 3 | 6) || !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let rgb: Srgb<u8> = body.parse().ok()?;
        let lab: Oklab = rgb.into_format::<f32>().into_linear().into_color();
        let q = |v: f32| (v / STOP_QUANTUM).round() * STOP_QUANTUM;
        Some(Lab { l: q(lab.l), a: q(lab.a), b: q(lab.b) })
    }

    /// Linear-light sRGB (unclamped).
    #[inline]
    pub fn to_linear(self) -> [f32; 3] {
        let l_ = self.l + 0.396_337_78 * self.a + 0.215_803_76 * self.b;
        let m_ = self.l - 0.105_561_346 * self.a - 0.063_854_17 * self.b;
        let s_ = self.l - 0.089_484_18 * self.a - 1.291_485_5 * self.b;
        let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
        [
            4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
            -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
            -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
        ]
    }

    /// From linear-light sRGB.
    #[inline]
    pub fn from_linear(c: [f32; 3]) -> Lab {
        let l = 0.412_221_46 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2];
        let m = 0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2];
        let s = 0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2];
        let (l_, m_, s_) = (cbrt(l), cbrt(m), cbrt(s));
        Lab {
            l: 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
            a: 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
            b: 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
        }
    }

    /// Encoded sRGB in [0, 1] per channel: soft highlight roll-off, then
    /// gamut clipping and the sRGB transfer curve.
    #[inline]
    pub fn to_srgb(self) -> [f32; 3] {
        let lin = self.to_linear();
        let lut = srgb_lut();
        lin.map(|v| encode(lut, soft_clip(v)))
    }
}

/// Film-like shoulder above [`HIGHLIGHT_KNEE`]; identity below.
#[inline]
fn soft_clip(v: f32) -> f32 {
    if v <= HIGHLIGHT_KNEE {
        return v.max(0.0);
    }
    let room = 1.0 - HIGHLIGHT_KNEE;
    HIGHLIGHT_KNEE + room * crate::fmath::tanh((v - HIGHLIGHT_KNEE) / room)
}

fn srgb_lut() -> &'static [f32; SRGB_LUT + 1] {
    static LUT: OnceLock<Box<[f32; SRGB_LUT + 1]>> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = Box::new([0.0; SRGB_LUT + 1]);
        for (i, v) in t.iter_mut().enumerate() {
            let x = i as f32 / SRGB_LUT as f32;
            *v = if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * pow(x, 1.0 / 2.4) - 0.055 };
        }
        t
    })
}

/// The sRGB transfer curve by table: linear interpolation between 4096
/// entries, except the steep linear toe, which is exact.
#[inline]
fn encode(lut: &[f32; SRGB_LUT + 1], x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x <= 0.003_130_8 {
        return 12.92 * x;
    }
    let f = x * SRGB_LUT as f32;
    let i = (f as usize).min(SRGB_LUT - 1);
    let t = f - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_through_oklab() {
        for hex in ["#0a1520", "#5fe0d4", "#e9c98b", "#ffffff", "#000000", "#c01010"] {
            let lab = Lab::from_hex(hex).unwrap();
            let rgb = lab.to_srgb();
            let back = format!("#{:02x}{:02x}{:02x}", (rgb[0] * 255.0).round() as u8, (rgb[1] * 255.0).round() as u8, (rgb[2] * 255.0).round() as u8);
            if hex == "#ffffff" {
                // The highlight shoulder keeps full channels a hair under 1.
                assert!(rgb.iter().all(|v| *v > 0.97), "{rgb:?}");
            } else {
                assert_eq!(back, hex, "{lab:?}");
            }
        }
        assert!(Lab::from_hex("#12").is_none());
        assert!(Lab::from_hex("zzzzzz").is_none());
        assert_eq!(Lab::from_hex("#fff"), Lab::from_hex("#ffffff"));
    }

    #[test]
    fn linear_round_trip() {
        let c = [0.2, 0.5, 0.8];
        let back = Lab::from_linear(c).to_linear();
        for i in 0..3 {
            assert!((c[i] - back[i]).abs() < 1e-4);
        }
    }
}
