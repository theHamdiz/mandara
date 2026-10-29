//! Layout and look of the shell's surfaces beyond the bar's basics: the
//! bar's chips, rim, shadow and alignment (`[bar]` extras), the side panels
//! (`[sidebar.look]`, shared by the power dock) and the drop-down panels
//! (`[panels]`).
//!
//! Every numeric setting has a named [`Span`] in [`ranges`]: Studio's
//! sliders and the shell's clamps read the same bounds.

use serde::{Deserialize, Serialize};

/// Bounds and slider step of one numeric setting.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Span {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

impl Span {
    pub const fn new(min: f32, max: f32, step: f32) -> Self {
        Self { min, max, step }
    }

    /// `v` inside the span; `fallback` for NaN / infinities (a hand-edited
    /// config never breaks the layout).
    pub fn clamp(self, v: f32, fallback: f32) -> f32 {
        if v.is_finite() { v.clamp(self.min, self.max) } else { fallback }
    }
}

/// Named bounds of every surface setting (logical px unless noted).
pub mod ranges {
    use super::Span;

    pub const BAR_HEIGHT: Span = Span::new(24.0, 44.0, 1.0);
    pub const BAR_EDGE_GAP: Span = Span::new(0.0, 24.0, 1.0);
    pub const BAR_SIDE_GAP: Span = Span::new(0.0, 48.0, 1.0);
    pub const BAR_PILL_RADIUS: Span = Span::new(0.0, 22.0, 1.0);
    pub const BAR_FLARE_RADIUS: Span = Span::new(0.0, 40.0, 1.0);
    /// Horizontal padding inside a bar chip, around the plugin's content.
    pub const BAR_CHIP_PADDING: Span = Span::new(2.0, 20.0, 1.0);
    /// Gap between neighbouring chips of a zone.
    pub const BAR_CHIP_SPACING: Span = Span::new(0.0, 16.0, 1.0);
    /// Share of the accent mixed into the bar fill (0–1).
    pub const BAR_TINT: Span = Span::new(0.0, 0.5, 0.01);
    pub const BAR_RIM_WIDTH: Span = Span::new(0.0, 3.0, 0.5);
    /// Multiplier of the specular rim / shadow alphas.
    pub const INTENSITY: Span = Span::new(0.0, 2.0, 0.05);
    pub const BAR_TEXT_SCALE: Span = Span::new(0.75, 1.5, 0.05);
    /// Hairline / micro pill height.
    pub const BAR_COMPACT_HEIGHT: Span = Span::new(18.0, 30.0, 1.0);
    /// Width of a left / right bar.
    pub const BAR_VERTICAL_WIDTH: Span = Span::new(32.0, 64.0, 1.0);
    /// Thickness of the screen frame around a side bar's display.
    pub const SCREEN_FRAME: Span = Span::new(2.0, 16.0, 1.0);
    pub const BAR_ICON_SCALE: Span = Span::new(0.75, 1.6, 0.05);
    /// Background opacity (0–1).
    pub const OPACITY: Span = Span::new(0.0, 1.0, 0.05);

    pub const SIDEBAR_WIDTH: Span = Span::new(280.0, 520.0, 4.0);
    pub const SIDE_PANEL_RADIUS: Span = Span::new(0.0, 40.0, 1.0);
    pub const CARD_RADIUS: Span = Span::new(0.0, 32.0, 1.0);
    /// Gap between a floating side panel and the screen edges / the bar.
    pub const SIDE_PANEL_MARGIN: Span = Span::new(0.0, 32.0, 1.0);
    pub const CARD_SPACING: Span = Span::new(0.0, 32.0, 1.0);
    pub const CARD_PADDING: Span = Span::new(4.0, 28.0, 1.0);

    pub const PANEL_RADIUS: Span = Span::new(8.0, 40.0, 1.0);
    /// Multiplier of a panel's natural width.
    pub const PANEL_WIDTH_SCALE: Span = Span::new(0.75, 1.5, 0.05);
    /// Tallest panel, as a share of the screen height the bar leaves free.
    pub const PANEL_MAX_HEIGHT: Span = Span::new(0.4, 1.0, 0.05);
    pub const PANEL_PADDING: Span = Span::new(0.0, 24.0, 1.0);
    pub const PANEL_GAP: Span = Span::new(0.0, 32.0, 1.0);
    /// Wait (ms) after the pointer leaves a hover-opened panel, the
    /// launcher or the session menu before it closes.
    pub const LEAVE_DELAY_MS: Span = Span::new(0.0, 2000.0, 20.0);
    /// How long (ms) a panel's action message (stopped, copied, failed…)
    /// stays up.
    pub const TOAST_MS: Span = Span::new(1000.0, 15000.0, 250.0);

    /// Spring overrides (same bounds as `[motion]`).
    pub const SPRING_STIFFNESS: Span = Span::new(60.0, 900.0, 5.0);
    pub const SPRING_DAMPING: Span = Span::new(4.0, 60.0, 0.5);
}

/// Defaults that keep the shipped look.
pub mod defaults {
    /// Bar chip padding / spacing (the original `SLOT_PADDING` / `SLOT_GAP`).
    pub const BAR_CHIP_PADDING_PX: f32 = 9.0;
    pub const BAR_CHIP_SPACING_PX: f32 = 2.0;
    pub const BAR_RIM_WIDTH_PX: f32 = 1.0;
    pub const BAR_COMPACT_HEIGHT_PX: f32 = 24.0;
    pub const BAR_VERTICAL_WIDTH_PX: f32 = 40.0;
    pub const SCREEN_FRAME_PX: f32 = 6.0;
    pub const BAR_SHADOW_STRENGTH: f32 = 0.5;
    /// Side panel corner radius (floating side panels, or every bar style
    /// but the edge frame) and the card radius / padding / spacing.
    pub const SIDE_PANEL_RADIUS_PX: f32 = 26.0;
    pub const CARD_RADIUS_PX: f32 = 18.0;
    pub const CARD_PADDING_PX: f32 = 14.0;
    pub const CARD_SPACING_PX: f32 = 12.0;
    pub const SIDE_PANEL_MARGIN_PX: f32 = 8.0;
    /// Drop-down panel inner padding and gap to the bar (floating styles).
    pub const PANEL_PADDING_PX: f32 = 6.0;
    pub const PANEL_GAP_PX: f32 = 10.0;
    pub const PANEL_MAX_HEIGHT: f32 = 0.92;
    /// Close delay after the pointer leaves (the original `LEAVE_DELAY` /
    /// `LEAVE_GRACE`).
    pub const LEAVE_DELAY_MS: u32 = 320;
    /// Panel action messages (the original 4 s `TOAST`).
    pub const TOAST_MS: u32 = 4000;
    /// Shipped springs of the drop-down panels and the sidebar.
    pub const PANEL_SPRING_STIFFNESS: f32 = 300.0;
    pub const PANEL_SPRING_DAMPING: f32 = 23.0;
    pub const SIDEBAR_SPRING_STIFFNESS: f32 = 260.0;
    pub const SIDEBAR_SPRING_DAMPING: f32 = 25.0;
}

/// Where the bar's center zone sits.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarCenterAlign {
    /// The middle of the screen.
    #[default]
    Screen,
    /// The middle of the space left between the start and end zones.
    Between,
    /// Right after the start zone.
    Start,
    /// Right before the end zone.
    End,
}

/// What separates neighbouring chips of a bar zone.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BarSeparator {
    #[default]
    None,
    Dot,
    Line,
}

/// A spring that follows `[motion]` or uses its own stiffness / damping.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpringOverride {
    /// Use `stiffness` / `damping` instead of the shipped spring tuned by
    /// the global motion settings.
    pub custom: bool,
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringOverride {
    pub const fn shipped(stiffness: f32, damping: f32) -> Self {
        Self { custom: false, stiffness, damping }
    }

    /// The custom `(stiffness, damping)`, clamped, when switched on.
    pub fn custom(&self) -> Option<(f32, f32)> {
        self.custom.then(|| (ranges::SPRING_STIFFNESS.clamp(self.stiffness, defaults::PANEL_SPRING_STIFFNESS), ranges::SPRING_DAMPING.clamp(self.damping, defaults::PANEL_SPRING_DAMPING)))
    }
}

impl Default for SpringOverride {
    fn default() -> Self {
        Self::shipped(defaults::PANEL_SPRING_STIFFNESS, defaults::PANEL_SPRING_DAMPING)
    }
}

fn sidebar_spring() -> SpringOverride {
    SpringOverride::shipped(defaults::SIDEBAR_SPRING_STIFFNESS, defaults::SIDEBAR_SPRING_DAMPING)
}

/// How a side panel meets the screen.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarAttach {
    /// Flush with the screen edge (joined to the edge frame by flares).
    #[default]
    Flush,
    /// Floating clear of the screen edges and the bar, rounded all round.
    Floating,
}

/// Size of the sidebar's header row (tabs, pin, close).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidebarHeader {
    #[default]
    Regular,
    Compact,
}

/// Where the sidebar's header row sits.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TabPosition {
    #[default]
    Top,
    /// Pinned to the bottom, under the scrolling cards.
    Bottom,
}

/// Look of the sidebar and the power dock (`[sidebar.look]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarLook {
    pub attach: SidebarAttach,
    /// Outer corner radius (floating, or any bar style but the edge frame).
    pub radius: f32,
    /// Gap to the screen edges and the bar while floating.
    pub margin: f32,
    /// Background opacity (0–1).
    pub opacity: f32,
    /// Corner radius and inner padding of cards (sidebar and panels).
    pub card_radius: f32,
    pub card_padding: f32,
    /// Gap between sidebar cards.
    pub card_spacing: f32,
    pub header: SidebarHeader,
    pub tab_position: TabPosition,
    /// The slide-in spring.
    #[serde(default = "sidebar_spring")]
    pub spring: SpringOverride,
}

impl Default for SidebarLook {
    fn default() -> Self {
        Self {
            attach: SidebarAttach::Flush,
            radius: defaults::SIDE_PANEL_RADIUS_PX,
            margin: defaults::SIDE_PANEL_MARGIN_PX,
            opacity: 1.0,
            card_radius: defaults::CARD_RADIUS_PX,
            card_padding: defaults::CARD_PADDING_PX,
            card_spacing: defaults::CARD_SPACING_PX,
            header: SidebarHeader::Regular,
            tab_position: TabPosition::Top,
            spring: sidebar_spring(),
        }
    }
}

impl SidebarLook {
    pub fn floating(&self) -> bool {
        self.attach == SidebarAttach::Floating
    }

    /// The floating gap (0 while flush).
    pub fn inset(&self) -> f32 {
        if self.floating() { self.margin_px() } else { 0.0 }
    }

    /// Floats when set to, and also while `bar` has no edge frame to join:
    /// glass pills, the island, the micro pill or a floating frame. A flush
    /// panel there looked bolted onto the screen edge under a bar that
    /// floats. Beside a side bar it stays as set.
    pub fn floats_with(&self, bar: &crate::BarConfig) -> bool {
        let side_bar = matches!(bar.position, crate::BarPosition::Left | crate::BarPosition::Right);
        self.floating() || (!side_bar && (!bar.layout.is_attached() || bar.floating_frame))
    }

    /// [`Self::inset`] for the panel as drawn beside `bar` ([`Self::floats_with`]).
    pub fn inset_with(&self, bar: &crate::BarConfig) -> f32 {
        if self.floats_with(bar) { self.margin_px() } else { 0.0 }
    }

    fn margin_px(&self) -> f32 {
        ranges::SIDE_PANEL_MARGIN.clamp(self.margin, defaults::SIDE_PANEL_MARGIN_PX)
    }
}

/// Horizontal placement of a drop-down panel.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelAnchor {
    /// Centered under the chip that opened it.
    #[default]
    Pill,
    /// Centered on the screen.
    Center,
    /// Against the screen edge of the chip's side (start / end zones).
    Edge,
}

/// How a drop-down panel opens.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelAnimation {
    /// Grows out of its chip (pours out of the edge frame).
    #[default]
    Morph,
    /// Fades in while sliding a short way from the bar.
    Slide,
    /// Appears at once.
    None,
}

/// Drop-down panels opened from the bar (`[panels]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelsConfig {
    /// Multiplier of each panel's natural width.
    pub width_scale: f32,
    /// Tallest panel, as a share of the screen height the bar leaves.
    pub max_height: f32,
    /// Inner padding around the content (`None`: 0 for the edge frame, a
    /// little breathing room for floating styles).
    pub padding: Option<f32>,
    /// Gap between a floating bar and its panels.
    pub gap: f32,
    pub anchor: PanelAnchor,
    /// Background opacity (0–1).
    pub opacity: f32,
    /// Multiplier of the floating panels' shadow.
    pub shadow_strength: f32,
    pub animation: PanelAnimation,
    pub spring: SpringOverride,
    /// Close delay (ms) after the pointer leaves a panel, the launcher or
    /// the session menu.
    pub leave_delay_ms: u32,
    /// How long (ms) a panel's action message stays up.
    pub toast_ms: u32,
}

impl Default for PanelsConfig {
    fn default() -> Self {
        Self {
            width_scale: 1.0,
            max_height: defaults::PANEL_MAX_HEIGHT,
            padding: None,
            gap: defaults::PANEL_GAP_PX,
            anchor: PanelAnchor::Pill,
            opacity: 1.0,
            shadow_strength: 1.0,
            animation: PanelAnimation::Morph,
            spring: SpringOverride::default(),
            leave_delay_ms: defaults::LEAVE_DELAY_MS,
            toast_ms: defaults::TOAST_MS,
        }
    }
}

impl PanelsConfig {
    /// Inner padding for a bar style (`attached`: the edge frame).
    pub fn padding_for(&self, attached: bool) -> f32 {
        let default = if attached { 0.0 } else { defaults::PANEL_PADDING_PX };
        self.padding.map_or(default, |p| ranges::PANEL_PADDING.clamp(p, default))
    }

    pub fn gap(&self) -> f32 {
        ranges::PANEL_GAP.clamp(self.gap, defaults::PANEL_GAP_PX)
    }

    pub fn width_scale(&self) -> f32 {
        ranges::PANEL_WIDTH_SCALE.clamp(self.width_scale, 1.0)
    }

    pub fn max_height(&self) -> f32 {
        ranges::PANEL_MAX_HEIGHT.clamp(self.max_height, defaults::PANEL_MAX_HEIGHT)
    }

    /// Close delay after the pointer leaves, clamped.
    pub fn leave_delay(&self) -> std::time::Duration {
        millis(ranges::LEAVE_DELAY_MS, self.leave_delay_ms)
    }

    /// How long an action message stays up, clamped.
    pub fn toast(&self) -> std::time::Duration {
        millis(ranges::TOAST_MS, self.toast_ms)
    }
}

/// `ms` clamped to `span`, as a duration.
pub fn millis(span: Span, ms: u32) -> std::time::Duration {
    std::time::Duration::from_millis(span.clamp(ms as f32, span.min) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_clamp_and_reject_non_finite_values() {
        assert_eq!(ranges::OPACITY.clamp(1.7, 1.0), 1.0);
        assert_eq!(ranges::OPACITY.clamp(-0.2, 1.0), 0.0);
        assert_eq!(ranges::PANEL_GAP.clamp(f32::NAN, 10.0), 10.0);
        assert_eq!(ranges::PANEL_GAP.clamp(f32::INFINITY, 10.0), 10.0);
        for span in [ranges::BAR_CHIP_PADDING, ranges::CARD_RADIUS, ranges::PANEL_WIDTH_SCALE, ranges::SPRING_DAMPING] {
            assert!(span.min < span.max && span.step > 0.0);
        }
    }

    #[test]
    fn defaults_sit_inside_their_spans() {
        let l = SidebarLook::default();
        let p = PanelsConfig::default();
        for (v, span) in [
            (defaults::BAR_CHIP_PADDING_PX, ranges::BAR_CHIP_PADDING),
            (defaults::BAR_CHIP_SPACING_PX, ranges::BAR_CHIP_SPACING),
            (defaults::BAR_RIM_WIDTH_PX, ranges::BAR_RIM_WIDTH),
            (defaults::BAR_COMPACT_HEIGHT_PX, ranges::BAR_COMPACT_HEIGHT),
            (defaults::BAR_VERTICAL_WIDTH_PX, ranges::BAR_VERTICAL_WIDTH),
            (defaults::SCREEN_FRAME_PX, ranges::SCREEN_FRAME),
            (l.radius, ranges::SIDE_PANEL_RADIUS),
            (l.card_radius, ranges::CARD_RADIUS),
            (l.card_padding, ranges::CARD_PADDING),
            (l.card_spacing, ranges::CARD_SPACING),
            (l.margin, ranges::SIDE_PANEL_MARGIN),
            (p.max_height, ranges::PANEL_MAX_HEIGHT),
            (p.gap, ranges::PANEL_GAP),
            (defaults::PANEL_PADDING_PX, ranges::PANEL_PADDING),
            (defaults::PANEL_SPRING_STIFFNESS, ranges::SPRING_STIFFNESS),
            (defaults::SIDEBAR_SPRING_DAMPING, ranges::SPRING_DAMPING),
        ] {
            assert!((span.min..=span.max).contains(&v), "{v} outside {span:?}");
        }
    }

    #[test]
    fn panel_padding_follows_the_bar_style_until_set() {
        let mut p = PanelsConfig::default();
        assert_eq!(p.padding_for(true), 0.0, "edge-frame panels hang flush from the bar");
        assert_eq!(p.padding_for(false), defaults::PANEL_PADDING_PX);
        p.padding = Some(99.0);
        assert_eq!(p.padding_for(true), ranges::PANEL_PADDING.max);
    }

    #[test]
    fn spring_override_is_off_until_switched_on() {
        let mut s = SpringOverride::default();
        assert_eq!(s.custom(), None);
        s.custom = true;
        s.stiffness = 5000.0;
        assert_eq!(s.custom(), Some((ranges::SPRING_STIFFNESS.max, defaults::PANEL_SPRING_DAMPING)));
        assert_eq!(SidebarLook::default().spring.stiffness, defaults::SIDEBAR_SPRING_STIFFNESS);
    }

    #[test]
    fn older_configs_without_the_new_tables_load_with_the_shipped_look() {
        let look: SidebarLook = toml::from_str("radius = 12.0").unwrap();
        assert_eq!(look.radius, 12.0);
        assert_eq!(look.spring, sidebar_spring(), "a partial table keeps the sidebar's own spring");
        let panels: PanelsConfig = toml::from_str("").unwrap();
        assert_eq!(panels, PanelsConfig::default());
        let panels: PanelsConfig = toml::from_str("anchor = \"edge\"\nanimation = \"slide\"\npadding = 4.0").unwrap();
        assert_eq!((panels.anchor, panels.animation, panels.padding), (PanelAnchor::Edge, PanelAnimation::Slide, Some(4.0)));
    }

    #[test]
    fn older_bar_tables_keep_the_shipped_look() {
        let bar: crate::BarConfig = toml::from_str("height = 28.0\ncorner_radius = 10.0").unwrap();
        assert_eq!(bar.height, 28.0);
        assert_eq!((bar.chip_padding(), bar.chip_spacing()), (defaults::BAR_CHIP_PADDING_PX, defaults::BAR_CHIP_SPACING_PX));
        assert_eq!((bar.text_scale(), bar.icon_scale(), bar.tint, bar.shadow, bar.floating_frame), (1.0, 1.0, 0.0, false, false));
        assert_eq!((bar.center_align, bar.separator), (BarCenterAlign::Screen, BarSeparator::None));
        // A whole config from before these tables loads, and round-trips.
        let cfg: crate::ShellConfig = toml::from_str("[bar]\nheight = 30.0\n[sidebar]\nwidth = 380.0\npinned = true").unwrap();
        assert_eq!(cfg.panels, PanelsConfig::default());
        assert_eq!(cfg.sidebar.look, SidebarLook::default());
        assert!(cfg.sidebar.pinned(), "the legacy sidebar flag still reads");
        let again: crate::ShellConfig = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(again.panels, cfg.panels);
        assert_eq!(again.sidebar.look, cfg.sidebar.look);
        assert_eq!(again.bar, cfg.bar);
    }

    #[test]
    fn out_of_range_bar_values_are_clamped() {
        let bar = crate::BarConfig { chip_padding: 500.0, chip_spacing: f32::NAN, text_scale: 0.1, icon_scale: 9.0, corner_radius: -4.0, ..Default::default() };
        assert_eq!(bar.chip_padding(), ranges::BAR_CHIP_PADDING.max);
        assert_eq!(bar.chip_spacing(), defaults::BAR_CHIP_SPACING_PX);
        assert_eq!(bar.text_scale(), ranges::BAR_TEXT_SCALE.min);
        assert_eq!(bar.icon_scale(), ranges::BAR_ICON_SCALE.max);
        assert_eq!(bar.shape_radius(), 0.0);
    }
}
