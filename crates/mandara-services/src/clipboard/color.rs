//! Colour values copied as text: CSS hex (`#rgb`, `#rgba`, `#rrggbb`,
//! `#rrggbbaa`), `rgb()` / `rgba()` (also Hyprland's `rgba(rrggbbaa)`),
//! `hsl()` / `hsla()`, `hwb()`, `lab()`, `lch()`, `oklab()` and `oklch()`,
//! in legacy comma or modern space syntax with an optional `/ alpha`.
//!
//! Only a whole value counts (`#ff8800` is a colour, `#ff8800 is orange`
//! is text), and short all-digit hex (`#123`, `#2048`) is read as an issue
//! or tag reference, not a colour. Named colours (`red`) stay text: a
//! single English word is far more often just a word.

use mandara_theme::Color;

/// What the swatch shows: the value as sRGB (gamut-mapped) plus how it was
/// written.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// `hex`, `rgb`, `hsl`, `hwb`, `lab`, `lch`, `oklab`, `oklch` or
    /// `hyprland` (`rgba(rrggbbaa)`).
    pub notation: &'static str,
    pub rgba: [u8; 4],
}

/// Longest colour value considered (anything longer is prose or code).
const MAX_LEN: usize = 96;
/// Full channel value of 8-bit sRGB, and percent scale.
const U8_MAX: f32 = 255.0;
const PERCENT: f32 = 100.0;
/// Degrees per turn, and the other CSS angle units in degrees.
const DEG_PER_TURN: f32 = 360.0;
const DEG_PER_GRAD: f32 = 0.9;
/// `lab()` / `lch()`: what 100 % of a / b and of chroma stand for.
const LAB_AB_PERCENT: f32 = 125.0;
const LCH_C_PERCENT: f32 = 150.0;
/// `oklab()` / `oklch()`: what 100 % of a / b and of chroma stand for.
const OKLAB_AB_PERCENT: f32 = 0.4;
const OKLCH_C_PERCENT: f32 = 0.4;
/// CIE Lab constants (CSS Color 4): κ, ε, and the D50 white point.
const LAB_KAPPA: f32 = 24389.0 / 27.0;
const LAB_EPSILON: f32 = 216.0 / 24389.0;
const D50_WHITE: [f32; 3] = [0.3457 / 0.3585, 1.0, (1.0 - 0.3457 - 0.3585) / 0.3585];
/// XYZ (D50) → linear sRGB (D65), Bradford-adapted (CSS Color 4).
const XYZ_D50_TO_LINEAR_SRGB: [[f32; 3]; 3] = [
    [3.134_136, -1.617_386_4, -0.490_662],
    [-0.978_795_5, 1.916_254_6, 0.033_442_73],
    [0.071_955_38, -0.228_976_83, 1.405_386_1],
];
/// sRGB transfer function: linear segment threshold and slope, gamma.
const SRGB_LINEAR_LIMIT: f32 = 0.003_130_8;
const SRGB_LINEAR_SLOPE: f32 = 12.92;
const SRGB_GAMMA: f32 = 2.4;
const SRGB_OFFSET: f32 = 0.055;

/// Parses a whole colour value (surrounding spaces and one trailing `;`
/// allowed).
pub fn parse(text: &str) -> Option<Parsed> {
    let s = text.trim();
    let s = s.strip_suffix(';').unwrap_or(s).trim_end();
    if s.is_empty() || s.len() > MAX_LEN || s.contains('\n') {
        return None;
    }
    if let Some(hex) = s.strip_prefix('#') {
        // `#123` / `#2048` read as references (issues, tags), not colours.
        if hex.len() <= 4 && hex.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return hex_rgba(hex).map(|rgba| Parsed { notation: "hex", rgba });
    }
    let open = s.find('(')?;
    let inner = s[open + 1..].strip_suffix(')')?.trim();
    let name = s[..open].trim().to_ascii_lowercase();
    if inner.contains(['(', ')']) {
        return None;
    }
    match name.as_str() {
        "rgb" | "rgba" if inner.len() >= 6 && !inner.contains([',', ' ', '%', '.']) => {
            // Hyprland: `rgb(rrggbb)` / `rgba(rrggbbaa)`.
            (inner.len() == 6 || inner.len() == 8).then(|| hex_rgba(inner)).flatten().map(|rgba| Parsed { notation: "hyprland", rgba })
        }
        "rgb" | "rgba" => rgb(&args(inner)?).map(|rgba| Parsed { notation: "rgb", rgba }),
        "hsl" | "hsla" => hsl(&args(inner)?).map(|rgba| Parsed { notation: "hsl", rgba }),
        "hwb" => hwb(&args(inner)?).map(|rgba| Parsed { notation: "hwb", rgba }),
        "lab" => lab(&args(inner)?).map(|rgba| Parsed { notation: "lab", rgba }),
        "lch" => lch(&args(inner)?).map(|rgba| Parsed { notation: "lch", rgba }),
        "oklab" => oklab(&args(inner)?).map(|rgba| Parsed { notation: "oklab", rgba }),
        "oklch" => oklch(&args(inner)?).map(|rgba| Parsed { notation: "oklch", rgba }),
        _ => None,
    }
}

/// `rgb`, `rgba`, `rrggbb` or `rrggbbaa` hex digits.
fn hex_rgba(hex: &str) -> Option<[u8; 4]> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let nibble = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok().map(|v| v * 0x11);
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some(match hex.len() {
        3 => [nibble(0)?, nibble(1)?, nibble(2)?, u8::MAX],
        4 => [nibble(0)?, nibble(1)?, nibble(2)?, nibble(3)?],
        6 => [byte(0)?, byte(2)?, byte(4)?, u8::MAX],
        8 => [byte(0)?, byte(2)?, byte(4)?, byte(6)?],
        _ => return None,
    })
}

/// One component: a number, a percentage, an angle, or `none` (zero).
#[derive(Copy, Clone, Debug, PartialEq)]
enum Arg {
    Num(f32),
    Pct(f32),
    /// Degrees.
    Angle(f32),
}

impl Arg {
    /// The value where 100 % stands for `full`.
    fn scaled(self, full: f32) -> f32 {
        match self {
            Arg::Num(v) | Arg::Angle(v) => v,
            Arg::Pct(p) => p / PERCENT * full,
        }
    }

    /// A hue in degrees (plain numbers are degrees).
    fn hue(self) -> Option<f32> {
        match self {
            Arg::Num(v) | Arg::Angle(v) => Some(v.rem_euclid(DEG_PER_TURN)),
            Arg::Pct(_) => None,
        }
    }

    /// A 0–1 fraction where 100 % (or `full` as a number) is 1.
    fn unit(self, full: f32) -> f32 {
        match self {
            Arg::Num(v) | Arg::Angle(v) => v / full,
            Arg::Pct(p) => p / PERCENT,
        }
    }
}

/// Splits `inner` into three components plus an optional alpha, in comma
/// (`1, 2, 3, 0.5`) or space (`1 2 3 / 50%`) syntax.
fn args(inner: &str) -> Option<([Arg; 3], Option<Arg>)> {
    let parts: Vec<&str> = if inner.contains(',') {
        if inner.contains('/') {
            return None;
        }
        inner.split(',').map(str::trim).collect()
    } else {
        let (main, alpha) = match inner.split_once('/') {
            Some((m, a)) => (m, Some(a.trim())),
            None => (inner, None),
        };
        let mut parts: Vec<&str> = main.split_whitespace().collect();
        if let Some(a) = alpha {
            if a.is_empty() || a.contains(char::is_whitespace) {
                return None;
            }
            parts.push(a);
        }
        parts
    };
    if !(3..=4).contains(&parts.len()) {
        return None;
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in &parts {
        out.push(component(p)?);
    }
    Some(([out[0], out[1], out[2]], out.get(3).copied()))
}

fn component(s: &str) -> Option<Arg> {
    if s.eq_ignore_ascii_case("none") {
        return Some(Arg::Num(0.0));
    }
    let number = |t: &str| -> Option<f32> {
        // Plain decimal numbers only (`1e3`, `inf`, `NaN` are not CSS colour components here).
        let ok = !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'+') && t.bytes().any(|b| b.is_ascii_digit());
        ok.then(|| t.parse::<f32>().ok()).flatten().filter(|v| v.is_finite())
    };
    let lower = s.to_ascii_lowercase();
    if let Some(p) = lower.strip_suffix('%') {
        return number(p).map(Arg::Pct);
    }
    for (unit, deg) in [("deg", 1.0), ("grad", DEG_PER_GRAD), ("rad", DEG_PER_TURN / std::f32::consts::TAU), ("turn", DEG_PER_TURN)] {
        if let Some(v) = lower.strip_suffix(unit) {
            return number(v).map(|v| Arg::Angle(v * deg));
        }
    }
    number(&lower).map(Arg::Num)
}

fn alpha(a: Option<Arg>) -> u8 {
    a.map_or(u8::MAX, |a| to_u8(a.unit(1.0)))
}

fn to_u8(unit: f32) -> u8 {
    (unit.clamp(0.0, 1.0) * U8_MAX).round() as u8
}

fn rgb(([r, g, b], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    if [r, g, b].iter().any(|c| matches!(c, Arg::Angle(_))) {
        return None;
    }
    Some([to_u8(r.unit(U8_MAX)), to_u8(g.unit(U8_MAX)), to_u8(b.unit(U8_MAX)), alpha(*a)])
}

/// HSL → sRGB (all 0–1, hue in degrees).
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let f = |n: f32| {
        let k = (n + h / 30.0) % 12.0;
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn hsl(([h, s, l], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    let (s, l) = (s.unit(PERCENT).clamp(0.0, 1.0), l.unit(PERCENT).clamp(0.0, 1.0));
    let [r, g, b] = hsl_to_rgb(h.hue()?, s, l);
    Some([to_u8(r), to_u8(g), to_u8(b), alpha(*a)])
}

fn hwb(([h, w, bl], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    let (w, bl) = (w.unit(PERCENT).clamp(0.0, 1.0), bl.unit(PERCENT).clamp(0.0, 1.0));
    let rgb = if w + bl >= 1.0 {
        let gray = w / (w + bl);
        [gray; 3]
    } else {
        hsl_to_rgb(h.hue()?, 1.0, 0.5).map(|c| c * (1.0 - w - bl) + w)
    };
    Some([to_u8(rgb[0]), to_u8(rgb[1]), to_u8(rgb[2]), alpha(*a)])
}

/// CIE Lab (D50) → gamma-encoded sRGB, clipped.
fn lab_to_srgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let f1 = (l + 16.0) / 116.0;
    let f0 = a / 500.0 + f1;
    let f2 = f1 - b / 200.0;
    let x = if f0.powi(3) > LAB_EPSILON { f0.powi(3) } else { (116.0 * f0 - 16.0) / LAB_KAPPA };
    let y = if l > LAB_KAPPA * LAB_EPSILON { f1.powi(3) } else { l / LAB_KAPPA };
    let z = if f2.powi(3) > LAB_EPSILON { f2.powi(3) } else { (116.0 * f2 - 16.0) / LAB_KAPPA };
    let xyz = [x * D50_WHITE[0], y * D50_WHITE[1], z * D50_WHITE[2]];
    XYZ_D50_TO_LINEAR_SRGB.map(|row| encode(row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2]))
}

/// Linear → gamma-encoded sRGB channel.
fn encode(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= SRGB_LINEAR_LIMIT { c * SRGB_LINEAR_SLOPE } else { (1.0 + SRGB_OFFSET) * c.powf(1.0 / SRGB_GAMMA) - SRGB_OFFSET }
}

fn lab(([l, a_, b_], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    let [r, g, b] = lab_to_srgb(l.scaled(PERCENT), a_.scaled(LAB_AB_PERCENT), b_.scaled(LAB_AB_PERCENT));
    Some([to_u8(r), to_u8(g), to_u8(b), alpha(*a)])
}

fn lch(([l, c, h], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    let (c, h) = (c.scaled(LCH_C_PERCENT).max(0.0), h.hue()?.to_radians());
    let [r, g, b] = lab_to_srgb(l.scaled(PERCENT), c * h.cos(), c * h.sin());
    Some([to_u8(r), to_u8(g), to_u8(b), alpha(*a)])
}

/// OKLab / OKLCH lightness: a number is 0–1, a percentage 0–100 %.
fn ok_lightness(l: Arg) -> f32 {
    l.scaled(1.0).clamp(0.0, 1.0)
}

fn oklch(([l, c, h], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    let alpha_unit = a.map_or(1.0, |a| a.unit(1.0).clamp(0.0, 1.0));
    Some(Color::oklcha(ok_lightness(*l), c.scaled(OKLCH_C_PERCENT).max(0.0), h.hue()?, alpha_unit).gamut_mapped().to_rgba8())
}

fn oklab(([l, a_, b_], a): &([Arg; 3], Option<Arg>)) -> Option<[u8; 4]> {
    let (oa, ob) = (a_.scaled(OKLAB_AB_PERCENT), b_.scaled(OKLAB_AB_PERCENT));
    let chroma = oa.hypot(ob);
    let hue = ob.atan2(oa).to_degrees().rem_euclid(DEG_PER_TURN);
    let alpha_unit = a.map_or(1.0, |a| a.unit(1.0).clamp(0.0, 1.0));
    Some(Color::oklcha(ok_lightness(*l), chroma, hue, alpha_unit).gamut_mapped().to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(s: &str) -> Option<[u8; 4]> {
        parse(s).map(|p| p.rgba)
    }

    /// Channels within ±2 of each other (rounding across colour spaces).
    fn near(a: [u8; 4], b: [u8; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
    }

    #[test]
    fn hex_forms() {
        assert_eq!(rgba("#ff8800"), Some([255, 136, 0, 255]));
        assert_eq!(rgba("#F80"), Some([255, 136, 0, 255]));
        assert_eq!(rgba("#ff880080"), Some([255, 136, 0, 128]));
        assert_eq!(rgba("#f808"), Some([255, 136, 0, 136]));
        assert_eq!(rgba("  #1e1e2e;  "), Some([30, 30, 46, 255]));
        assert_eq!(parse("#cba6f7").unwrap().notation, "hex");
        // All-digit six / eight digit hex is still a colour.
        assert_eq!(rgba("#123456"), Some([0x12, 0x34, 0x56, 255]));
    }

    #[test]
    fn hex_near_misses() {
        for s in ["#123", "#2048", "#12", "#1234567", "#ggg", "#hashtag", "#ff8800 is orange", "ff8800", "#", "##fff", "#ff 88 00"] {
            assert_eq!(parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn rgb_and_hyprland() {
        assert_eq!(rgba("rgb(255, 136, 0)"), Some([255, 136, 0, 255]));
        assert_eq!(rgba("rgba(255,136,0,0.5)"), Some([255, 136, 0, 128]));
        assert_eq!(rgba("rgb(255 136 0 / 50%)"), Some([255, 136, 0, 128]));
        assert_eq!(rgba("rgb(100% 0% 50%)"), Some([255, 0, 128, 255]));
        assert_eq!(rgba("RGB(0 0 0)"), Some([0, 0, 0, 255]));
        assert_eq!(rgba("rgb(none 0 0)"), Some([0, 0, 0, 255]));
        let h = parse("rgba(cba6f7ee)").unwrap();
        assert_eq!((h.notation, h.rgba), ("hyprland", [0xcb, 0xa6, 0xf7, 0xee]));
        assert_eq!(rgba("rgb(89b4fa)"), Some([0x89, 0xb4, 0xfa, 255]));
    }

    #[test]
    fn rgb_near_misses() {
        for s in ["rgb(1, 2)", "rgb(1 2 3 4 5)", "rgb(1, 2, 3 / 4)", "rgb(a, b, c)", "rgb(1, 2, 3", "rgb()", "rgb(10deg 0 0)", "rgbx(1 2 3)", "rgb(1 2 3) and more", "rgb(fffff)", "rgb(1e3 0 0)"] {
            assert_eq!(parse(s), None, "{s:?}");
        }
    }

    #[test]
    fn hsl_hwb() {
        assert_eq!(rgba("hsl(0, 100%, 50%)"), Some([255, 0, 0, 255]));
        assert_eq!(rgba("hsl(120deg 100% 25%)"), Some([0, 128, 0, 255]));
        assert_eq!(rgba("hsla(240, 100%, 50%, 0.2)"), Some([0, 0, 255, 51]));
        assert_eq!(rgba("hsl(0.5turn 100% 50%)"), Some([0, 255, 255, 255]));
        assert_eq!(rgba("hwb(0 0% 0%)"), Some([255, 0, 0, 255]));
        assert_eq!(rgba("hwb(90 60% 60%)"), Some([128, 128, 128, 255]));
        assert_eq!(parse("hsl(50%, 1, 1)"), None, "a hue can't be a percentage");
    }

    #[test]
    fn lab_lch_oklab_oklch() {
        // CSS Lab / LCH are D50: sRGB red is lab(54.29 80.8 69.89).
        assert!(near(rgba("lab(54.29 80.8 69.89)").unwrap(), [255, 0, 0, 255]), "{:?}", rgba("lab(54.29 80.8 69.89)"));
        assert!(near(rgba("lch(54.29 106.84 40.85)").unwrap(), [255, 0, 0, 255]), "{:?}", rgba("lch(54.29 106.84 40.85)"));
        assert_eq!(rgba("lab(100% 0 0)"), Some([255, 255, 255, 255]));
        assert!(near(rgba("oklch(62.8% 0.2577 29.23)").unwrap(), [255, 0, 0, 255]), "{:?}", rgba("oklch(62.8% 0.2577 29.23)"));
        assert!(near(rgba("oklch(0.628 0.2577 29.23 / 0.5)").unwrap(), [255, 0, 0, 128]));
        assert!(near(rgba("oklab(0.628 0.2249 0.1258)").unwrap(), [255, 0, 0, 255]), "{:?}", rgba("oklab(0.628 0.2249 0.1258)"));
        assert_eq!(rgba("oklch(0 0 0)"), Some([0, 0, 0, 255]));
        assert_eq!(parse("oklch(70% 0.1 200)").unwrap().notation, "oklch");
    }

    #[test]
    fn text_is_not_a_colour() {
        for s in ["red", "orange juice", "color: red", "f(x) = rgb", "lab(results)", "oklch()", "hsl(1, 2, 3)\nhsl(4, 5, 6)", "call(1, 2, 3)", "rgb(1, 2, (3))"] {
            assert_eq!(parse(s), None, "{s:?}");
        }
    }
}
