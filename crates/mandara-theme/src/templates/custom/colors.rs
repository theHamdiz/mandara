//! Palette values stay colors through filters and format only at output.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use anyhow::Result;
use material_colors::{
    color::Rgb,
    dynamic_color::{DynamicScheme, Variant},
    palette::TonalPalette,
    scheme::Scheme,
};
use minijinja::{
    Environment, Error, ErrorKind, Value,
    value::{Enumerator, Object, ObjectRepr},
};
use palette::{FromColor, Hsl, Srgb};

use super::CustomColor;
use crate::{Color, MandaraPalette};

/// Byte, percentage and hue domains of app theme formats.
const BYTE: f32 = 255.0;
const PERCENT: f32 = 100.0;
const TURN: f32 = 360.0;
/// Standard luma weights used by template grayscale filters.
const LUMA: [f32; 3] = [0.299, 0.587, 0.114];
/// Standard Material tone steps used when iterating a tonal palette.
const TONES: [i32; 18] = [
    0, 5, 10, 15, 20, 25, 30, 35, 40, 50, 60, 70, 80, 90, 95, 98, 99, 100,
];
const ANSI_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

#[derive(Clone, Copy, Debug)]
enum Format {
    Hex,
    Bare,
    Rgb,
    Csv,
    Rgba,
    Hsl,
    Hsla,
    Red,
    Green,
    Blue,
    Alpha,
    Hue,
    Saturation,
    Lightness,
}
impl Format {
    fn of(s: &str) -> Option<Self> {
        Some(match s {
            "hex" => Self::Hex,
            "hex_stripped" => Self::Bare,
            "rgb" => Self::Rgb,
            "rgb_csv" => Self::Csv,
            "rgba" => Self::Rgba,
            "hsl" => Self::Hsl,
            "hsla" => Self::Hsla,
            "red" => Self::Red,
            "green" => Self::Green,
            "blue" => Self::Blue,
            "alpha" => Self::Alpha,
            "hue" => Self::Hue,
            "saturation" => Self::Saturation,
            "lightness" => Self::Lightness,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct Ink {
    color: Color,
    format: Format,
}
impl Ink {
    fn value(color: Color) -> Value {
        Value::from_object(Self {
            color,
            format: Format::Hex,
        })
    }
}
impl Object for Ink {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
    fn get_value_by_str(self: &Arc<Self>, key: &str) -> Option<Value> {
        Some(Value::from_object(Self {
            format: Format::of(key)?,
            ..**self
        }))
    }
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.get_value_by_str(key.as_str()?)
    }
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, _] = self.color.to_rgba8();
        let a = self.color.alpha();
        let hsl = Hsl::from_color(Srgb::new(
            f32::from(r) / BYTE,
            f32::from(g) / BYTE,
            f32::from(b) / BYTE,
        ));
        let (h, s, l) = (
            hsl.hue.into_positive_degrees().trunc(),
            (hsl.saturation * PERCENT).trunc(),
            (hsl.lightness * PERCENT).trunc(),
        );
        match self.format {
            Format::Hex => write!(f, "#{r:02x}{g:02x}{b:02x}"),
            Format::Bare => write!(f, "{r:02x}{g:02x}{b:02x}"),
            Format::Rgb => write!(f, "rgb({r}, {g}, {b})"),
            Format::Csv => write!(f, "{r},{g},{b}"),
            Format::Rgba => write!(f, "rgba({r}, {g}, {b}, {a})"),
            Format::Hsl => write!(f, "hsl({h}, {s}%, {l}%)"),
            Format::Hsla => write!(f, "hsla({h}, {s}%, {l}%, {a})"),
            Format::Red => write!(f, "{r}"),
            Format::Green => write!(f, "{g}"),
            Format::Blue => write!(f, "{b}"),
            Format::Alpha => write!(f, "{a}"),
            Format::Hue => write!(f, "{h}"),
            Format::Saturation => write!(f, "{s}"),
            Format::Lightness => write!(f, "{l}"),
        }
    }
}

fn bad(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, message.into())
}
fn ink(value: Value) -> std::result::Result<Ink, Error> {
    if let Some(ink) = value.downcast_object_ref::<Ink>() {
        return Ok(*ink);
    }
    let text = value.to_string();
    Color::from_hex(&text)
        .map(|color| Ink {
            color,
            format: Format::Hex,
        })
        .map_err(|_| bad(format!("expected a color, got {text:?}")))
}
fn changed(value: Value, f: impl FnOnce(Color) -> Color) -> std::result::Result<Value, Error> {
    let mut ink = ink(value)?;
    ink.color = f(ink.color).gamut_mapped();
    Ok(Value::from_object(ink))
}
fn number(
    value: Value,
    number: f32,
    change: impl FnOnce(Color, f32) -> Color,
) -> std::result::Result<Value, Error> {
    if !number.is_finite() {
        return Err(bad("color filter needs a finite number"));
    }
    changed(value, |color| change(color, number))
}
fn in_hsl(color: Color, change: impl FnOnce(&mut Hsl)) -> Color {
    let c = color.to_srgba();
    let mut hsl = Hsl::from_color(c.color);
    change(&mut hsl);
    hsl.saturation = hsl.saturation.clamp(0.0, 1.0);
    hsl.lightness = hsl.lightness.clamp(0.0, 1.0);
    let rgb = Srgb::from_color(hsl);
    Color::from_rgba8(
        (rgb.red * BYTE).round() as u8,
        (rgb.green * BYTE).round() as u8,
        (rgb.blue * BYTE).round() as u8,
        (color.alpha() * BYTE).round() as u8,
    )
}

fn words(text: &str) -> Vec<String> {
    let mut separated = String::with_capacity(text.len());
    let mut lower = false;
    for c in text.chars() {
        if c.is_uppercase() && lower {
            separated.push(' ');
        }
        if c.is_alphanumeric() {
            separated.extend(c.to_lowercase());
        } else {
            separated.push(' ');
        }
        lower = c.is_lowercase() || c.is_numeric();
    }
    separated.split_whitespace().map(str::to_string).collect()
}

fn title(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub(super) fn filters(env: &mut Environment<'static>) {
    env.add_function("truthy", |v: Value| {
        v.as_str().map_or_else(
            || v.is_true(),
            |s| {
                let s = s.trim();
                !s.is_empty()
                    && !["false", "0", "none"]
                        .iter()
                        .any(|f| s.eq_ignore_ascii_case(f))
            },
        )
    });
    env.add_filter("to_color", |v: Value| ink(v).map(Value::from_object));
    env.add_filter("grayscale", |v| {
        changed(v, |c| {
            let [r, g, b, a] = c.to_rgba8();
            let gray = (f32::from(r) * LUMA[0] + f32::from(g) * LUMA[1] + f32::from(b) * LUMA[2])
                .round() as u8;
            Color::from_rgba8(gray, gray, gray, a)
        })
    });
    env.add_filter("invert", |v| {
        changed(v, |c| {
            let [r, g, b, a] = c.to_rgba8();
            Color::from_rgba8(u8::MAX - r, u8::MAX - g, u8::MAX - b, a)
        })
    });
    env.add_filter("set_alpha", |v, a: f32| {
        number(v, a, |c, a| c.with_alpha(a.clamp(0.0, 1.0)))
    });
    env.add_filter("set_hue", |v, h: f32| {
        number(v, h, |c, h| {
            in_hsl(c, |v| v.hue = h.rem_euclid(TURN).into())
        })
    });
    env.add_filter("rotate_hue", |v, h: f32| {
        number(v, h, |c, h| {
            in_hsl(c, |value| {
                value.hue = (value.hue.into_positive_degrees() + h)
                    .rem_euclid(TURN)
                    .into()
            })
        })
    });
    env.add_filter("set_lightness", |v, n: f32| {
        number(v, n, |c, n| in_hsl(c, |v| v.lightness = n / PERCENT))
    });
    env.add_filter("set_saturation", |v, n: f32| {
        number(v, n, |c, n| in_hsl(c, |v| v.saturation = n / PERCENT))
    });
    env.add_filter("lighten", |v, n: f32| {
        number(v, n, |c, n| in_hsl(c, |v| v.lightness += n / PERCENT))
    });
    env.add_filter("darken", |v, n: f32| {
        number(v, n, |c, n| in_hsl(c, |v| v.lightness -= n / PERCENT))
    });
    env.add_filter("saturate", |v, n: f32| {
        number(v, n, |c, n| in_hsl(c, |v| v.saturation += n / PERCENT))
    });
    env.add_filter("desaturate", |v, n: f32| {
        number(v, n, |c, n| in_hsl(c, |v| v.saturation -= n / PERCENT))
    });
    env.add_filter("auto_lightness", |v, n: f32| {
        number(v, n, |c, n| {
            in_hsl(c, |v| {
                v.lightness += if v.lightness < 0.5 {
                    n / PERCENT
                } else {
                    -n / PERCENT
                }
            })
        })
    });
    for (name, index) in [("set_red", 0), ("set_green", 1), ("set_blue", 2)] {
        env.add_filter(name, move |v, n: f32| {
            number(v, n, |c, n| {
                let mut rgba = c.to_rgba8();
                rgba[index] = n.clamp(0.0, BYTE).round() as u8;
                Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
            })
        });
    }
    env.add_filter("blend", |v: Value, other: Value, amount: Option<f32>| {
        let other = ink(other)?.color;
        number(v, amount.unwrap_or(0.0), |color, amount| {
            color.mix(other, amount.clamp(0.0, 1.0))
        })
    });
    env.add_filter("harmonize", |v: Value, other: Value| {
        let other = ink(other)?.color;
        changed(v, |color| color.harmonize_toward(other))
    });
    for name in [
        "hex",
        "hex_stripped",
        "rgb",
        "rgb_csv",
        "rgba",
        "hsl",
        "hsla",
        "red",
        "green",
        "blue",
        "alpha",
        "hue",
        "saturation",
        "lightness",
    ] {
        env.add_filter(name, move |v: Value| {
            let mut ink = ink(v)?;
            ink.format = Format::of(name).unwrap();
            Ok::<_, Error>(Value::from_object(ink))
        });
    }
    env.add_filter("lower_case", |s: String| s.to_lowercase());
    env.add_filter("snake_case", |s: String| words(&s).join("_"));
    env.add_filter("kebab_case", |s: String| words(&s).join("-"));
    env.add_filter("pascal_case", |s: String| {
        words(&s).iter().map(|s| title(s)).collect::<String>()
    });
    env.add_filter("camel_case", |s: String| {
        let mut w = words(&s).into_iter();
        w.next().unwrap_or_default() + &w.map(|s| title(&s)).collect::<String>()
    });
}

fn rgb(c: Color) -> Rgb {
    let [r, g, b, _] = c.to_rgba8();
    Rgb::new(r, g, b)
}
fn color(c: Rgb) -> Color {
    Color::from_rgba8(c.red, c.green, c.blue, u8::MAX)
}
fn dynamic(p: &MandaraPalette) -> DynamicScheme {
    DynamicScheme::by_variant(rgb(p.primary), &Variant::TonalSpot, p.is_dark, None)
}

fn tokens(p: &MandaraPalette) -> BTreeMap<String, Color> {
    let mut tokens: BTreeMap<_, _> = Scheme::from(dynamic(p))
        .into_iter()
        .map(|(k, v)| (k.to_string(), color(v)))
        .collect();
    for (name, c) in p.tokens() {
        tokens.insert(name.into(), c);
    }
    tokens.insert("source_color".into(), p.primary);
    for (name, c) in [
        ("surface", p.surface_base),
        ("background", p.surface_base),
        ("on_background", p.on_surface),
        ("hover", p.tertiary),
        (
            "on_hover",
            p.tertiary
                .best_on(Color::hex("#ffffff"), Color::hex("#000000")),
        ),
        ("error", p.urgent_coral),
    ] {
        tokens.insert(name.into(), c);
    }
    for (name, c) in [
        ("foreground", p.on_surface),
        ("background", p.surface_base),
        ("cursor", p.primary),
        ("cursor_text", p.on_primary),
        ("selection_fg", p.on_surface),
        ("selection_bg", p.primary_container),
    ] {
        tokens.insert(format!("terminal_{name}"), c);
    }
    let ansi = super::super::ansi(p);
    for (i, name) in ANSI_NAMES.iter().enumerate() {
        tokens.insert(format!("terminal_normal_{name}"), ansi[i]);
        tokens.insert(
            format!("terminal_bright_{name}"),
            ansi[i + ANSI_NAMES.len()],
        );
    }
    tokens
}

fn modes(dark: Color, light: Color, is_dark: bool) -> Value {
    Value::from_object(BTreeMap::from([
        ("dark", Ink::value(dark)),
        ("light", Ink::value(light)),
        ("default", Ink::value(if is_dark { dark } else { light })),
    ]))
}

#[derive(Debug)]
struct Tones {
    palette: TonalPalette,
    is_dark: bool,
}
impl Object for Tones {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Iterable
    }
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let text = key.to_string();
        let tone = text
            .trim_start_matches('_')
            .parse::<i32>()
            .ok()
            .filter(|n| (0..=100).contains(n))?;
        Some(Ink::value(color(self.palette.tone(tone))))
    }
    fn get_value_by_str(self: &Arc<Self>, key: &str) -> Option<Value> {
        self.get_value(&Value::from(key))
    }
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        let palette = self.palette;
        let dark = self.is_dark;
        Enumerator::Iter(Box::new(TONES.into_iter().map(move |n| {
            let c = color(palette.tone(n));
            modes(c, c, dark)
        })))
    }
}

pub(super) fn context(
    dark: &MandaraPalette,
    light: &MandaraPalette,
    is_dark: bool,
    custom: &BTreeMap<String, CustomColor>,
) -> Result<(Value, Value)> {
    let (mut d, mut l) = (tokens(dark), tokens(light));
    for (name, spec) in custom {
        for (p, map) in [(dark, &mut d), (light, &mut l)] {
            let (source, blend) = spec.resolve(p.is_dark)?;
            let seed = if blend {
                source.harmonize_toward(p.primary)
            } else {
                source
            };
            let scheme = Scheme::from(DynamicScheme::by_variant(
                rgb(seed),
                &Variant::TonalSpot,
                p.is_dark,
                None,
            ));
            for (suffix, c) in [
                ("", color(scheme.primary)),
                ("_container", color(scheme.primary_container)),
                ("_source", source),
                ("_value", source),
            ] {
                map.insert(format!("{name}{suffix}"), c);
            }
            map.insert(format!("on_{name}"), color(scheme.on_primary));
            map.insert(
                format!("on_{name}_container"),
                color(scheme.on_primary_container),
            );
        }
    }
    let values = Value::from_object(
        d.iter()
            .filter_map(|(name, dark)| {
                l.get(name)
                    .map(|light| (name.clone(), modes(*dark, *light, is_dark)))
            })
            .collect::<BTreeMap<_, _>>(),
    );
    let scheme = dynamic(if is_dark { dark } else { light });
    let palettes = Value::from_object(
        [
            ("primary", scheme.primary_palette),
            ("secondary", scheme.secondary_palette),
            ("tertiary", scheme.tertiary_palette),
            ("error", scheme.error_palette),
            ("neutral", scheme.neutral_palette),
            ("neutral_variant", scheme.neutral_variant_palette),
        ]
        .into_iter()
        .map(|(name, palette)| (name, Value::from_object(Tones { palette, is_dark })))
        .collect::<BTreeMap<_, _>>(),
    );
    Ok((values, palettes))
}
