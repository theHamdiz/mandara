//! Surface grain (`Grain`): a fine procedural texture any surface can wear
//! (the dock, the taskbar, and later cards, bars, panels and Studio).
//!
//! One settings type for every surface, so they all offer the same styles
//! and bounds. [`Grain::default`] is off: tables written before a surface
//! had grain load without it, and a surface only shows grain once someone
//! picks a style. Every number is clamped to a named [`Span`] (`ranges`)
//! when read, so a hand-edited value can never become loud or costly.
//! Re-exported from [`crate::desktop`].

use serde::{Deserialize, Serialize};

use crate::surfaces::Span;

/// The grain's look (`style`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrainStyle {
    /// No grain (the default).
    #[default]
    None,
    /// An even, single-pixel sand: light and dark specks in equal measure.
    Fine,
    /// Soft clumps of grain, like fast photographic film.
    Film,
    /// Short horizontal fibres over a faint tooth, like laid paper.
    Paper,
    /// Sparse dark flecks on a clean surface, like speckled stone.
    Speckle,
}

impl GrainStyle {
    pub const ALL: [Self; 5] = [Self::None, Self::Fine, Self::Film, Self::Paper, Self::Speckle];
}

/// The colour of the specks (`tint`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GrainTint {
    /// Pure white and black specks.
    #[default]
    Neutral,
    /// Light and dark specks in the hue of the surface's palette.
    Surface,
}

impl GrainTint {
    pub const ALL: [Self; 2] = [Self::Neutral, Self::Surface];
}

/// Default values.
pub mod defaults {
    /// Strongest speck alpha: subtle, visible on close look.
    pub const AMOUNT: f32 = 0.05;
    /// Speck size, logical px.
    pub const SCALE: f32 = 1.0;
    pub const CONTRAST: f32 = 1.0;
}

/// Named bounds of the grain's numbers.
pub mod ranges {
    use super::Span;

    /// Strongest speck alpha: never more than a texture, never a pattern.
    pub const AMOUNT: Span = Span::new(0.0, 0.12, 0.005);
    /// Speck size, logical px.
    pub const SCALE: Span = Span::new(1.0, 3.0, 0.25);
    /// How far specks spread from faint to strong.
    pub const CONTRAST: Span = Span::new(0.5, 1.5, 0.05);
}

/// Grain over a surface. Read it through the accessors (or [`Self::sanitized`]):
/// they clamp every number to [`ranges`].
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grain {
    pub style: GrainStyle,
    /// Strongest speck alpha, 0–0.12.
    pub amount: f32,
    /// Speck size, 1–3 logical px.
    pub scale: f32,
    /// Spread from faint to strong specks, 0.5–1.5.
    pub contrast: f32,
    pub tint: GrainTint,
    /// A different random field (two surfaces side by side need not match).
    pub seed: u32,
}

impl Default for Grain {
    /// Off: no style, the other values at their defaults.
    fn default() -> Self {
        Self { style: GrainStyle::None, amount: defaults::AMOUNT, scale: defaults::SCALE, contrast: defaults::CONTRAST, tint: GrainTint::Neutral, seed: 0 }
    }
}

impl Grain {
    /// Grain in `style` at the default strength.
    pub fn with_style(style: GrainStyle) -> Self {
        Self { style, ..Self::default() }
    }

    pub fn amount(&self) -> f32 {
        ranges::AMOUNT.clamp(self.amount, defaults::AMOUNT)
    }

    pub fn scale(&self) -> f32 {
        ranges::SCALE.clamp(self.scale, defaults::SCALE)
    }

    pub fn contrast(&self) -> f32 {
        ranges::CONTRAST.clamp(self.contrast, defaults::CONTRAST)
    }

    /// Whether anything is drawn.
    pub fn is_on(&self) -> bool {
        self.style != GrainStyle::None && self.amount() > 0.0
    }

    /// Every number inside its range.
    pub fn sanitized(&self) -> Self {
        Self { amount: self.amount(), scale: self.scale(), contrast: self.contrast(), ..*self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_off_and_inside_its_ranges() {
        let g = Grain::default();
        assert!(!g.is_on());
        assert_eq!(g.sanitized(), g);
        assert!(Grain::with_style(GrainStyle::Film).is_on());
        assert!(!Grain { amount: 0.0, ..Grain::with_style(GrainStyle::Fine) }.is_on(), "zero amount draws nothing");
        // A table without the keys loads the default.
        let empty: Grain = toml::from_str("").unwrap();
        assert_eq!(empty, g);
        let partial: Grain = toml::from_str("style = \"paper\"\n").unwrap();
        assert_eq!(partial, Grain::with_style(GrainStyle::Paper));
    }

    #[test]
    fn every_number_is_clamped() {
        let wild = Grain { style: GrainStyle::Speckle, amount: 5.0, scale: 40.0, contrast: -3.0, tint: GrainTint::Surface, seed: 9 };
        let s = wild.sanitized();
        assert_eq!((s.amount, s.scale, s.contrast), (ranges::AMOUNT.max, ranges::SCALE.max, ranges::CONTRAST.min));
        assert_eq!((s.style, s.tint, s.seed), (GrainStyle::Speckle, GrainTint::Surface, 9), "choices are kept");
        let nan = Grain { amount: f32::NAN, scale: f32::INFINITY, contrast: f32::NAN, ..Grain::default() };
        assert_eq!(nan.sanitized(), Grain::default(), "non-finite values fall back to the defaults");
    }

    #[test]
    fn ids_are_kebab_case_and_complete() {
        let ids = |v: Vec<serde_json::Value>| v.into_iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(ids(GrainStyle::ALL.iter().map(|s| serde_json::to_value(s).unwrap()).collect()), ["none", "fine", "film", "paper", "speckle"]);
        assert_eq!(ids(GrainTint::ALL.iter().map(|s| serde_json::to_value(s).unwrap()).collect()), ["neutral", "surface"]);
    }
}
