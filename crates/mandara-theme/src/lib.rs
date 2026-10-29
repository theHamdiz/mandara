//! Mandara color engine.
//!
//! * [`Color`] — OKLCH newtype over `palette::Oklcha`, with gamut mapping and
//!   WCAG contrast.
//! * [`MandaraPalette`] — the semantic token set every surface renders with.
//! * [`catalog`] — token-based themes (TOML seed pairs, built-in + user),
//!   expanded with contrast guarantees.
//! * [`material`] — CAM16/HCT wallpaper quantization and Material 3 schemes.
//! * [`ThemeEngine`] — source resolution and 450 ms OKLCH crossfades.

pub mod color;
pub mod engine;
pub mod material;
pub mod catalog;
pub mod palette;
pub mod templates;

pub use color::{Color, ColorParseError};
pub use engine::{CubicBezier, ResolvedTheme, THEME_TRANSITION, ThemeEngine, ThemeSource};
pub use material::{MiniPalette, SchemeVariant, SeedColor};
pub use catalog::{DEFAULT_THEME, ThemeDef};
pub use palette::{MandaraPalette, PaletteAdjustments};
