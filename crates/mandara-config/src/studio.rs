//! Studio's own appearance, independent of desktop Looks.

mod presets;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::desktop::StyleConfig;
use crate::surfaces::Span;
use crate::{MotionConfig, ShellConfig, ThemeMode, TypographyConfig};
use mandara_theme::{Color, PaletteAdjustments, SchemeVariant, ThemeSource};

pub use presets::{StudioLook, StudioLookDef};

/// An absent snapshot preserves the existing shell-following appearance.
/// A selected Look stores its values, so later desktop changes cannot alter it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioAppearance {
    pub look: StudioLook,
    pub theme: StudioPalette,
    pub style: StyleConfig,
    pub typography: TypographyConfig,
    pub motion: MotionConfig,
    pub frame: StudioFrame,
}

impl Default for StudioAppearance {
    fn default() -> Self {
        StudioLook::default().appearance()
    }
}

/// A fixed palette stays independent of wallpaper changes and desktop schedules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioPalette {
    pub mode: ThemeMode,
    pub color: Color,
    pub variant: SchemeVariant,
    pub adjustments: PaletteAdjustments,
    pub transition_ms: u64,
}

impl Default for StudioPalette {
    fn default() -> Self {
        let [r, g, b, a] = Color::oklch(0.68, 0.12, 220.0).to_rgba8();
        Self {
            mode: ThemeMode::Dark,
            color: Color::from_rgba8(r, g, b, a),
            variant: SchemeVariant::TonalSpot,
            adjustments: Default::default(),
            transition_ms: crate::defaults::THEME_TRANSITION_MS,
        }
    }
}

impl StudioPalette {
    pub fn source(&self) -> ThemeSource {
        ThemeSource::Seed {
            color: self.color,
            variant: self.variant,
        }
    }
}

/// The window's structure; control designs remain shared with the shell.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FrameDesign {
    #[default]
    Classic,
    Glass,
    Header,
    Chassis,
    Comic,
    Editorial,
    Terminal,
}

/// Window and navigation measurements in logical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StudioFrame {
    pub design: FrameDesign,
    pub controls_at_start: bool,
    pub inset: f32,
    pub radius: f32,
    pub border: f32,
    pub title_height: f32,
    pub sidebar_width: f32,
    pub sidebar_inset: f32,
    pub nav_height: f32,
    pub nav_radius: f32,
    pub card_radius: f32,
    pub card_padding: f32,
}

impl Default for StudioFrame {
    fn default() -> Self {
        Self {
            design: FrameDesign::Classic,
            controls_at_start: false,
            inset: 0.0,
            radius: 18.0,
            border: 1.0,
            title_height: 46.0,
            sidebar_width: 252.0,
            sidebar_inset: 0.0,
            nav_height: 38.0,
            nav_radius: 9.0,
            card_radius: 18.0,
            card_padding: 18.0,
        }
    }
}

/// Bounds keep hand-edited frame settings usable at the minimum window size.
pub mod ranges {
    use super::Span;
    pub const INSET: Span = Span::new(0.0, 28.0, 1.0);
    pub const RADIUS: Span = Span::new(0.0, 48.0, 1.0);
    pub const BORDER: Span = Span::new(0.0, 6.0, 0.5);
    pub const TITLE_HEIGHT: Span = Span::new(32.0, 80.0, 1.0);
    pub const SIDEBAR_WIDTH: Span = Span::new(180.0, 340.0, 1.0);
    pub const NAV_HEIGHT: Span = Span::new(28.0, 56.0, 1.0);
}

impl StudioFrame {
    pub fn bounded(&self) -> Self {
        let d = Self::default();
        Self {
            inset: ranges::INSET.clamp(self.inset, d.inset),
            radius: ranges::RADIUS.clamp(self.radius, d.radius),
            border: ranges::BORDER.clamp(self.border, d.border),
            title_height: ranges::TITLE_HEIGHT.clamp(self.title_height, d.title_height),
            sidebar_width: ranges::SIDEBAR_WIDTH.clamp(self.sidebar_width, d.sidebar_width),
            sidebar_inset: ranges::INSET.clamp(self.sidebar_inset, d.sidebar_inset),
            nav_height: ranges::NAV_HEIGHT.clamp(self.nav_height, d.nav_height),
            nav_radius: ranges::RADIUS.clamp(self.nav_radius, d.nav_radius),
            card_radius: crate::surfaces::ranges::CARD_RADIUS
                .clamp(self.card_radius, d.card_radius),
            card_padding: crate::surfaces::ranges::CARD_PADDING
                .clamp(self.card_padding, d.card_padding),
            ..self.clone()
        }
    }
}

impl ShellConfig {
    /// Studio resolves its own fonts only after the user chooses a Studio Look.
    pub fn studio_typography(&self) -> &TypographyConfig {
        self.studio
            .as_deref()
            .map_or(&self.typography, |studio| &studio.typography)
    }
}
