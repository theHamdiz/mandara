//! Component designs (Themes → Components, `[style]`): the shapes,
//! outlines, elevation and motion of the shared controls every surface
//! draws: buttons ([`crate::plugins::panel_fx::pill_button`],
//! [`crate::kit::icon_button`] and Studio's buttons), switches, check boxes
//! and radio buttons ([`crate::plugins::panel_fx::switch`] and friends),
//! text and search fields ([`crate::text_field::TextField`]), chips and
//! segmented choices, progress meters ([`crate::plugins::panel_fx::meter`])
//! and cards ([`crate::kit::card`], [`crate::plugins::panel_fx::glass_card`]).
//!
//! The controls stay where they are; this module holds what the non-classic
//! designs need: the geometry (pure functions over fixed arrays and
//! iterators, tested inside their bounds in both reading directions and for
//! zero allocations per frame), the painters, and the motion (springs that
//! follow the animation speed and reduced motion). Every *Classic* design is
//! drawn by the control's own original code, unchanged.
//!
//! Everything is drawn with SDF quads (rounded rectangles, the fractional
//! "subpixel" quads for smooth diagonals), never tessellated paths, and in
//! palette tokens only.

pub mod buttons;
pub mod chips;
pub mod cut;
pub mod inputs;
pub mod motion;
pub mod progress;
pub mod shapes;
pub mod surfaces;
pub mod toggles;

pub use mandara_config::components::{ButtonDesign, CardStyle, ChipDesign, ComponentPreset, InputDesign, ProgressDesign, SurfaceCorners, ToggleDesign};
