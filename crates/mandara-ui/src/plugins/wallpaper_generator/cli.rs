//! Argument parsing for the `wallgen` commands: pure functions from words
//! to typed requests, so every flag is unit-tested without a shell.

use mandara_config::wallgen::{WallgenFormat, WallgenSize};
use mandara_i18n::tf;
use mandara_wallgen::{PaletteSpec, Recipe, Resolution, Style};

/// Where a generated recipe comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// The current recipe in the settings.
    Recipe,
    /// A random recipe, of one style when given.
    Random(Option<Style>),
    /// A variation of the current recipe.
    Variation,
    /// The current recipe with a new layout seed.
    Reseed,
}

/// An explicit output size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// A size class (`display`, `4k`, …), resolved like the setting.
    Class(WallgenSize),
    /// Exact pixels (`3000x2000`).
    Exact(Resolution),
}

/// `wallgen generate [source] [flags…]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Generate {
    pub source: Option<Source>,
    pub size: Option<Size>,
    pub format: Option<WallgenFormat>,
    pub palette: Option<PaletteChoice>,
    pub seed: Option<u64>,
    /// `--light` / `--dark` (else the settings' tone).
    pub light: Option<bool>,
    /// `--apply` / `--no-apply` (else the settings).
    pub apply: Option<bool>,
    pub video: bool,
    pub seconds: Option<f32>,
    pub fps: Option<u32>,
    /// Reply at once with the job id instead of waiting for the file.
    pub detach: bool,
}

/// A palette named on the command line.
#[derive(Clone, Debug, PartialEq)]
pub enum PaletteChoice {
    Spec(PaletteSpec),
    /// The live theme colors (resolved by the shell).
    Theme,
}

/// Parses a seed: decimal, or hex with `0x`.
pub fn parse_seed(s: &str) -> Option<u64> {
    let s = s.trim();
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// A size id: a class (`display`, `1080p`, `4k`, `uw1440`, …) or `WxH`.
pub fn parse_size(s: &str) -> Option<Size> {
    if let Some(class) = WallgenSize::from_id(s).filter(|c| *c != WallgenSize::Custom) {
        return Some(Size::Class(class));
    }
    let alias = match s.trim().to_ascii_lowercase().as_str() {
        "2k" | "qhd" => Some(WallgenSize::Qhd1440),
        "fhd" | "1080" => Some(WallgenSize::Fhd1080),
        "ultrawide" => Some(WallgenSize::Ultrawide1440),
        "portrait" => Some(WallgenSize::Portrait4k),
        _ => None,
    };
    alias.map(Size::Class).or_else(|| Resolution::from_id(s).map(Size::Exact))
}

/// A palette: a curated id, `theme`, or comma-separated hex stops.
pub fn parse_palette(s: &str) -> Result<PaletteChoice, String> {
    let s = s.trim();
    if s == "theme" {
        return Ok(PaletteChoice::Theme);
    }
    if mandara_wallgen::named_palette(s).is_some() {
        return Ok(PaletteChoice::Spec(PaletteSpec::Named(s.into())));
    }
    let stops: Vec<String> = s.split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
    let spec = PaletteSpec::Custom(stops);
    if spec.is_valid() {
        Ok(PaletteChoice::Spec(spec))
    } else {
        Err(tf("Unknown palette {palette}: use an id from `wallgen palettes`, theme, or 2–8 hex colors like #101820,#5fe0d4", &[("palette", &s)]))
    }
}

fn value<'a>(words: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    words.get(*i).map(String::as_str).ok_or_else(|| tf("{flag} needs a value", &[("flag", &flag)]))
}

/// Splits `--flag=value` into two words.
fn split_words(words: &[String]) -> Vec<String> {
    words
        .iter()
        .flat_map(|w| match w.strip_prefix("--").and_then(|rest| rest.split_once('=')) {
            Some((flag, v)) => vec![format!("--{flag}"), v.to_string()],
            None => vec![w.clone()],
        })
        .collect()
}

/// Parses `wallgen generate`'s words.
pub fn parse_generate(words: &[String]) -> Result<Generate, String> {
    let words = split_words(words);
    let mut g = Generate::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        match w {
            "--size" => {
                let v = value(&words, &mut i, w)?;
                g.size = Some(parse_size(v).ok_or_else(|| tf("Unknown size {size}: use display, 1080p, 1440p, 4k, 5k, 8k, uw1440, portrait4k or WIDTHxHEIGHT", &[("size", &v)]))?);
            }
            "--format" => {
                let v = value(&words, &mut i, w)?;
                g.format = Some(WallgenFormat::from_id(v).ok_or_else(|| tf("Unknown format {format}: use lossless, webp, png or jpeg", &[("format", &v)]))?);
            }
            "--palette" => g.palette = Some(parse_palette(value(&words, &mut i, w)?)?),
            "--seed" => {
                let v = value(&words, &mut i, w)?;
                g.seed = Some(parse_seed(v).ok_or_else(|| tf("{value} isn't a seed (a number, or hex like 0x1f)", &[("value", &v)]))?);
            }
            "--seconds" => {
                let v = value(&words, &mut i, w)?;
                g.seconds = Some(v.parse::<f32>().ok().filter(|s| s.is_finite() && *s > 0.0).ok_or_else(|| tf("{flag} needs a number, got {value}", &[("flag", &w), ("value", &v)]))?);
            }
            "--fps" => {
                let v = value(&words, &mut i, w)?;
                g.fps = Some(v.parse::<u32>().ok().filter(|f| *f > 0).ok_or_else(|| tf("{flag} needs a number, got {value}", &[("flag", &w), ("value", &v)]))?);
            }
            "--light" => g.light = Some(true),
            "--dark" => g.light = Some(false),
            "--apply" => g.apply = Some(true),
            "--no-apply" => g.apply = Some(false),
            "--video" => g.video = true,
            "--detach" => g.detach = true,
            flag if flag.starts_with("--") => return Err(tf("Unknown option {flag}", &[("flag", &flag)])),
            word if g.source.is_none() => {
                g.source = Some(match word {
                    "recipe" | "current" => Source::Recipe,
                    "random" => Source::Random(None),
                    "variation" => Source::Variation,
                    "reseed" => Source::Reseed,
                    style => Source::Random(Some(Style::from_id(style).ok_or_else(|| tf("Unknown style {style}; see `wallgen styles`", &[("style", &style)]))?)),
                });
            }
            extra => return Err(tf("Unexpected argument {value}", &[("value", &extra)])),
        }
        i += 1;
    }
    if !g.video && (g.seconds.is_some() || g.fps.is_some()) {
        return Err(mandara_i18n::t("--seconds and --fps go with --video").into());
    }
    Ok(g)
}

/// Applies `key=value` assignments to `recipe`: `style`, `seed`,
/// `palette` (id, or hex stops separated by commas), `light` and any
/// parameter key of the style. Changing the style keeps the parameters the
/// new style shares.
pub fn assign(recipe: &Recipe, words: &[String]) -> Result<Recipe, String> {
    let mut r = recipe.clone();
    for word in words.iter().flat_map(|w| w.split_whitespace()) {
        let (key, v) = word.split_once('=').ok_or_else(|| tf("Expected key=value, got {value}", &[("value", &word)]))?;
        let (key, v) = (key.trim(), v.trim().trim_matches('"'));
        match key {
            "style" => r.style = Style::from_id(v).ok_or_else(|| tf("Unknown style {style}; see `wallgen styles`", &[("style", &v)]))?,
            "seed" => r.seed = parse_seed(v).ok_or_else(|| tf("{value} isn't a seed (a number, or hex like 0x1f)", &[("value", &v)]))?,
            "light" => r.light = matches!(v, "true" | "on" | "1" | "yes"),
            "palette" => match parse_palette(v)? {
                PaletteChoice::Spec(spec) => r.palette = spec,
                PaletteChoice::Theme => return Err(mandara_i18n::t("Use `wallgen generate --palette theme` for the live theme colors").into()),
            },
            param => {
                let spec = r.style.spec(param).ok_or_else(|| tf("{style} has no parameter {key}; see `wallgen styles`", &[("style", &r.style.id()), ("key", &param)]))?;
                let n: f32 = v.parse().map_err(|_| tf("{flag} needs a number, got {value}", &[("flag", &param), ("value", &v)]))?;
                r.set_param(spec.key, n);
            }
        }
    }
    r.sanitize();
    Ok(r)
}

/// `wallgen recipe set`'s text: a whole TOML recipe, or assignments.
pub fn parse_recipe_set(current: &Recipe, words: &[String]) -> Result<Recipe, String> {
    let text = words.join(" ");
    if text.contains('\n') || text.trim_start().starts_with('[') || (text.contains("style =") && text.contains("seed =")) {
        return Recipe::from_toml(&text);
    }
    assign(current, words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn generate_flags() {
        let g = parse_generate(&words("aurora --size 8k --format png --palette nord --seed 0x2a --light --apply")).unwrap();
        assert_eq!(g.source, Some(Source::Random(Some(Style::Aurora))));
        assert_eq!(g.size, Some(Size::Class(WallgenSize::Uhd8k)));
        assert_eq!(g.format, Some(WallgenFormat::Png));
        assert_eq!(g.palette, Some(PaletteChoice::Spec(PaletteSpec::Named("nord".into()))));
        assert_eq!((g.seed, g.light, g.apply, g.video, g.detach), (Some(42), Some(true), Some(true), false, false));
        let g = parse_generate(&words("random --size=3000x2000 --format=jpeg --dark --no-apply --video --seconds 4 --fps 12 --detach")).unwrap();
        assert_eq!(g.source, Some(Source::Random(None)));
        assert_eq!(g.size, Some(Size::Exact(Resolution::Custom { width: 3000, height: 2000 })));
        assert_eq!(g.format, Some(WallgenFormat::Jpeg));
        assert_eq!(parse_generate(&words("--format lossless")).unwrap().format, Some(WallgenFormat::Lossless));
        assert_eq!(parse_generate(&words("--format webp")).unwrap().format, Some(WallgenFormat::Webp));
        assert_eq!((g.light, g.apply, g.video, g.seconds, g.fps, g.detach), (Some(false), Some(false), true, Some(4.0), Some(12), true));
        assert_eq!(parse_generate(&[]).unwrap(), Generate::default(), "no words: the current recipe with the settings");
        assert_eq!(parse_generate(&words("variation")).unwrap().source, Some(Source::Variation));
        assert_eq!(parse_generate(&words("reseed")).unwrap().source, Some(Source::Reseed));
        assert_eq!(parse_generate(&words("recipe --size 2k")).unwrap().size, Some(Size::Class(WallgenSize::Qhd1440)));
        assert_eq!(parse_generate(&words("--size 1920x1080")).unwrap().size, Some(Size::Exact(Resolution::Fhd1080)));
        assert_eq!(parse_generate(&words("--size display")).unwrap().size, Some(Size::Class(WallgenSize::Display)));
    }

    #[test]
    fn generate_rejects_bad_input() {
        for bad in ["nope", "--size 12k", "--format gif", "--seed x", "--palette nope", "--fps 0", "--seconds", "--video --seconds -1", "--seconds 5", "random extra", "--frobnicate"] {
            assert!(parse_generate(&words(bad)).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn palettes_parse() {
        assert_eq!(parse_palette("theme").unwrap(), PaletteChoice::Theme);
        assert_eq!(parse_palette("#101820,#5fe0d4,#e9c98b").unwrap(), PaletteChoice::Spec(PaletteSpec::Custom(vec!["#101820".into(), "#5fe0d4".into(), "#e9c98b".into()])));
        assert!(parse_palette("#101820").is_err(), "one stop is not a palette");
        assert!(parse_palette("#zzzzzz,#000000").is_err());
    }

    #[test]
    fn recipe_assignments() {
        let r = Recipe::new(Style::Mesh, 1);
        let next = assign(&r, &words("style=waves seed=0x10 light=true palette=nord vignette=80 bands=99")).unwrap();
        assert_eq!(next.style, Style::Waves);
        assert_eq!(next.seed, 16);
        assert!(next.light);
        assert_eq!(next.palette, PaletteSpec::Named("nord".into()));
        assert_eq!(next.param("vignette"), 80.0);
        assert_eq!(next.param("bands"), 12.0, "clamped to the range");
        assert!(assign(&r, &words("nonsense")).is_err());
        assert!(assign(&r, &words("bands=3")).is_err(), "mesh has no bands");
        assert!(assign(&r, &words("vignette=lots")).is_err());
        // A whole TOML recipe replaces the current one.
        let text = Recipe::random(Some(Style::Glass), 5).to_toml();
        let parsed = parse_recipe_set(&r, std::slice::from_ref(&text)).unwrap();
        assert_eq!(parsed, Recipe::from_toml(&text).unwrap());
        assert_eq!(parse_recipe_set(&r, &words("seed=7")).unwrap().seed, 7);
    }

    #[test]
    fn seeds_and_sizes() {
        assert_eq!(parse_seed("0xff"), Some(255));
        assert_eq!(parse_seed("12"), Some(12));
        assert_eq!(parse_seed("-1"), None);
        assert_eq!(parse_size("portrait"), Some(Size::Class(WallgenSize::Portrait4k)));
        assert_eq!(parse_size("custom"), None, "custom needs WxH");
        assert_eq!(parse_size("8x8"), None);
    }
}
