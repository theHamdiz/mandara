//! The shape controls every painted desktop widget shares: the plate's
//! shape, an outline, a hard or soft directional shadow, an optional header
//! line and a chrome (a window title bar, washi tape or a polaroid border).
//! They live in [`crate::desktop_widgets::Look`] beside the plate's opacity,
//! radius and padding; rotation and flip are in
//! [`crate::desktop_widgets::Placement`].
//!
//! Every default draws nothing new, so widgets written before these
//! settings existed look exactly as they did.

use serde::{Deserialize, Serialize};

use crate::surfaces::Span;

/// Bounds and defaults (reference px at a 1080 px tall display unless
/// noted).
pub mod ranges {
    use super::Span;

    /// Lobes of a cookie, petals of a flower.
    pub const LOBES: Span = Span::new(4.0, 20.0, 1.0);
    pub const LOBES_DEFAULT: u32 = 12;
    /// Depth of the lobes or the paper tear, fraction of the radius.
    pub const DEPTH: Span = Span::new(0.02, 0.4, 0.01);
    pub const DEPTH_DEFAULT: f32 = 0.08;
    /// Lean of a skewed plate, degrees.
    pub const SKEW_DEG: Span = Span::new(-30.0, 30.0, 1.0);
    pub const SKEW_DEFAULT_DEG: f32 = -12.0;
    /// Corner cut of a chamfered plate.
    pub const CHAMFER_PX: Span = Span::new(2.0, 64.0, 1.0);
    pub const CHAMFER_DEFAULT_PX: f32 = 14.0;
    pub const OUTLINE_PX: Span = Span::new(0.0, 6.0, 0.5);
    /// Shadow offset, blur and opacity.
    pub const SHADOW_OFFSET_PX: Span = Span::new(-24.0, 24.0, 1.0);
    pub const SHADOW_X_DEFAULT_PX: f32 = 4.0;
    pub const SHADOW_Y_DEFAULT_PX: f32 = 6.0;
    pub const SHADOW_BLUR_PX: Span = Span::new(1.0, 40.0, 1.0);
    pub const SHADOW_BLUR_DEFAULT_PX: f32 = 14.0;
    pub const SHADOW_OPACITY: Span = Span::new(0.05, 1.0, 0.05);
    pub const SHADOW_OPACITY_DEFAULT: f32 = 0.35;
    /// Header text size.
    pub const HEADER_SIZE_PX: Span = Span::new(9.0, 40.0, 1.0);
    pub const HEADER_SIZE_DEFAULT_PX: f32 = 15.0;
    /// Angle of the tape strips, degrees from horizontal.
    pub const TAPE_ANGLE_DEG: Span = Span::new(0.0, 60.0, 1.0);
    pub const TAPE_ANGLE_DEFAULT_DEG: f32 = 35.0;
    /// Rotation of a whole widget, degrees.
    pub const ROTATION_DEG: Span = Span::new(-180.0, 180.0, 1.0);
}

/// The plate's outline.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlateShape {
    /// A rectangle with the plate's corner radius.
    #[default]
    Rect,
    /// A circle (an ellipse on a box that isn't square).
    Circle,
    /// Flat sides with soft, continuous corners.
    Squircle,
    /// A scalloped "cookie" with `lobes` round bumps.
    Cookie,
    /// A flower with `lobes` petals.
    Flower,
    /// A pentagon, point up, with the plate's corner radius.
    Pentagon,
    /// Corners cut at 45°.
    Chamfer,
    /// A slanted parallelogram.
    Skew,
    /// Torn paper edges.
    Deckle,
}

impl PlateShape {
    pub const ALL: [PlateShape; 9] = [Self::Rect, Self::Circle, Self::Squircle, Self::Cookie, Self::Flower, Self::Pentagon, Self::Chamfer, Self::Skew, Self::Deckle];
}

/// A shadow cast by the whole widget (plate and marks).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShadowKind {
    #[default]
    None,
    /// Blurred.
    Soft,
    /// A crisp offset copy (neo-brutalist, sticker, HUD).
    Hard,
}

/// A frame drawn around the widget.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Chrome {
    #[default]
    None,
    /// A classic window: a pinstriped title bar with a close box (the
    /// header text is its title).
    Window,
    /// Two strips of washi tape over the top corners.
    Tape,
    /// A white print border, deeper at the bottom (the header is written
    /// there).
    Polaroid,
}

impl Chrome {
    pub const ALL: [Chrome; 4] = [Self::None, Self::Window, Self::Tape, Self::Polaroid];
}

/// Rounds a stored lobe count into its range.
pub fn lobes(v: u32) -> u32 {
    ranges::LOBES.clamp(v as f32, ranges::LOBES_DEFAULT as f32).round() as u32
}
