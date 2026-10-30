//! Registry entries for the bar's layout and appearance extras, the side
//! panels' look (`[sidebar.look]`, shared by the power dock) and the
//! drop-down panels (`[panels]`). Slider bounds come from
//! `mandara_config::surfaces::ranges`, the same spans the shell clamps to.

use mandara_config::surfaces::{defaults, ranges};
use mandara_config::{BarLayout, ShellConfig};
use mandara_i18n::tk;

use super::{Control, Section, Setting, s, span};

/// Group names.
pub const APPEARANCE: &str = tk("Appearance");
pub const PANELS: &str = tk("Drop-down panels");
/// How drop-down panels open (animation and spring).
pub const PANEL_MOTION: &str = tk("Panel animation");
const LAYOUT: &str = tk("Layout");
const MOTION: &str = tk("Motion");

const CENTER_ALIGN: &[(&str, &str)] = &[("screen", tk("Screen center")), ("between", tk("Between the sides")), ("start", tk("After the start group")), ("end", tk("Before the end group"))];
const SEPARATORS: &[(&str, &str)] = &[("none", tk("None")), ("dot", tk("Dot")), ("line", tk("Line"))];
const ATTACH: &[(&str, &str)] = &[("flush", tk("Flush with the edge")), ("floating", tk("Floating"))];
const HEADERS: &[(&str, &str)] = &[("regular", tk("Regular")), ("compact", tk("Compact"))];
const TAB_POSITIONS: &[(&str, &str)] = &[("top", tk("Top")), ("bottom", tk("Bottom"))];
const ANCHORS: &[(&str, &str)] = &[("pill", tk("Under its button")), ("center", tk("Screen center")), ("edge", tk("Screen edge"))];
const ANIMATIONS: &[(&str, &str)] = &[("morph", tk("Morph from the bar")), ("slide", tk("Fade & slide")), ("none", tk("None"))];

/// Notes shown in place of a control that does nothing in the current setup.
const EDGE_FRAME_ONLY: &str = tk("Edge frame only");
const FLOATING_ONLY: &str = tk("Floating styles and the floating frame");
const ISLAND_TOGETHER: &str = tk("The island keeps its groups together");
const HANGS_FROM_BAR: &str = tk("Edge-frame panels hang from the bar");
const FLOATING_SIDE_ONLY: &str = tk("Floating side panels only");
const FLARED_FLUSH: &str = tk("Flush side panels join the edge frame with flares");
const CUSTOM_SPRING_OFF: &str = tk("Turn on Custom spring first");
const SIDE_BAR_FRAME: &str = tk("A side bar is always an edge frame");
const COMPACT_ONLY: &str = tk("Hairline and micro pill only");
const HAIRLINE_ONLY: &str = tk("Hairline style only");
const SIDE_BAR_WIDTH_INSTEAD: &str = tk("A side bar uses Side bar width");
const COMPACT_HEIGHT_INSTEAD: &str = tk("This style uses Compact height");
const SIDE_BAR_ONLY: &str = tk("Left or right bar only");
const FRAME_FIRST: &str = tk("Turn on Screen frame (side bar) first");
const SIDE_BAR_STAYS: &str = tk("A side bar always stays on screen");
const FRAME_FIRST_OFF: &str = tk("The screen frame already joins the bar to the edges");

pub fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    vec![
        // Bar → Layout
        s(Bar, LAYOUT, "/bar/compact_height", tk("Compact height"), tk("Thickness of the hairline and the micro pill"), span(ranges::BAR_COMPACT_HEIGHT, " px")),
        s(Bar, APPEARANCE, "/bar/accent_line", tk("Accent line"), tk("A 1 px accent line along the hairline's inner edge"), Toggle),
        s(Bar, LAYOUT, "/bar/vertical_width", tk("Side bar width"), tk("Width of a left or right bar"), span(ranges::BAR_VERTICAL_WIDTH, " px")),
        s(Bar, tk("Screen frame"), "/bar/screen_frame", tk("Screen frame"), tk("A thin border round the whole display, joined to the side bar; it reserves no space of its own"), Toggle),
        s(Bar, tk("Screen frame"), "/bar/frame_thickness", tk("Frame thickness"), tk("Width of the screen frame's border"), span(ranges::SCREEN_FRAME, " px")),
        s(Bar, tk("Screen frame"), "/bar/floating_frame", tk("Float the edge frame"), tk("Inset the edge frame by the edge gap and side inset, with rounded ends; panels still pour out of it"), Toggle),
        s(Bar, LAYOUT, "/bar/center_align", tk("Center group"), tk("Where the middle group sits"), Choice(CENTER_ALIGN)),
        s(Bar, tk("Spacing"), "/bar/chip_padding", tk("Item padding"), tk("Space inside each bar item, around its content"), span(ranges::BAR_CHIP_PADDING, " px")),
        s(Bar, tk("Spacing"), "/bar/chip_spacing", tk("Item spacing"), tk("Gap between neighbouring bar items"), span(ranges::BAR_CHIP_SPACING, " px")),
        s(Bar, tk("Spacing"), "/bar/separator", tk("Separators"), tk("A mark between neighbouring bar items"), Choice(SEPARATORS)),
        // Bar → Appearance
        s(Bar, APPEARANCE, "/bar/tint", tk("Accent tint"), tk("Mixes the theme accent into the bar's background"), span(ranges::BAR_TINT, "")),
        s(Bar, APPEARANCE, "/bar/rim_width", tk("Rim width"), tk("Thickness of the glass edge highlight"), span(ranges::BAR_RIM_WIDTH, " px")),
        s(Bar, APPEARANCE, "/bar/rim_intensity", tk("Rim intensity"), tk("Strength of the glass edge highlight and shade"), span(ranges::INTENSITY, "×")),
        s(Bar, APPEARANCE, "/bar/shadow", tk("Shadow"), tk("A soft shadow under the bar"), Toggle),
        s(Bar, APPEARANCE, "/bar/shadow_strength", tk("Shadow strength"), tk("How dark the bar's shadow is"), span(ranges::INTENSITY, "×")),
        s(Bar, APPEARANCE, "/bar/text_scale", tk("Text size"), tk("Bar text relative to the interface text size"), span(ranges::BAR_TEXT_SCALE, "×")),
        s(Bar, APPEARANCE, "/bar/icon_scale", tk("Icon size"), tk("Bar icons relative to their designed size"), span(ranges::BAR_ICON_SCALE, "×")),
        // Bar → Drop-down panels
        s(Bar, PANELS, "/panels/width_scale", tk("Width"), tk("Panels wider or narrower than their natural width"), span(ranges::PANEL_WIDTH_SCALE, "×")),
        s(Bar, PANELS, "/panels/max_height", tk("Maximum height"), tk("Tallest panel, as a share of the screen height the bar leaves free"), span(ranges::PANEL_MAX_HEIGHT, "×")),
        s(
            Bar,
            PANELS,
            "/panels/padding",
            tk("Inner padding"),
            tk("Space between a panel's edge and its content (unset: none for the edge frame, a little for floating styles)"),
            OptionalSlider { min: f64::from(ranges::PANEL_PADDING.min), max: f64::from(ranges::PANEL_PADDING.max), step: f64::from(ranges::PANEL_PADDING.step), unit: " px", default: f64::from(defaults::PANEL_PADDING_PX) },
        ),
        s(Bar, PANELS, "/panels/gap", tk("Gap to the bar"), tk("Space between a floating bar and its panels"), span(ranges::PANEL_GAP, " px")),
        s(Bar, PANELS, "/panels/anchor", tk("Horizontal position"), tk("Center panels under their button, on the screen, or push them to the screen edge on their side"), Choice(ANCHORS)),
        s(Bar, PANELS, "/panels/opacity", tk("Background opacity"), tk("Lower it to let the desktop show through panels"), span(ranges::OPACITY, "")),
        s(Bar, PANELS, "/panels/shadow_strength", tk("Shadow strength"), tk("Shadow under floating panels and floating side panels"), span(ranges::INTENSITY, "×")),
        s(Bar, PANEL_MOTION, "/panels/animation", tk("Open animation"), tk("How panels appear"), Choice(ANIMATIONS)),
        s(Bar, PANEL_MOTION, "/panels/spring/custom", tk("Custom spring"), tk("Use your own stiffness and damping instead of the global motion settings"), Toggle),
        s(Bar, PANEL_MOTION, "/panels/spring/stiffness", tk("Stiffness"), tk("Higher is snappier"), span(ranges::SPRING_STIFFNESS, "")),
        s(Bar, PANEL_MOTION, "/panels/spring/damping", tk("Damping"), tk("Higher settles with less bounce"), span(ranges::SPRING_DAMPING, "")),
        // Sidebars → Layout
        s(Sidebar, LAYOUT, "/sidebar/look/attach", tk("Attachment"), tk("Flush with the screen edge, or floating clear of the edges and the bar (sidebar and power dock)"), Choice(ATTACH)),
        s(Sidebar, LAYOUT, "/sidebar/look/margin", tk("Margin"), tk("Gap between a floating side panel and the screen edges and bar"), span(ranges::SIDE_PANEL_MARGIN, " px")),
        s(Sidebar, LAYOUT, "/sidebar/look/tab_position", tk("Header position"), tk("Tabs and buttons above the cards, or pinned at the bottom"), Choice(TAB_POSITIONS)),
        s(Sidebar, LAYOUT, "/sidebar/look/header", tk("Header size"), tk("Size of the tabs and buttons"), Choice(HEADERS)),
        // Sidebars → Appearance
        s(Sidebar, APPEARANCE, "/sidebar/look/radius", tk("Corner radius"), tk("Outer corners of the sidebar and the power dock"), span(ranges::SIDE_PANEL_RADIUS, " px")),
        s(Sidebar, APPEARANCE, "/sidebar/look/opacity", tk("Background opacity"), tk("Lower it to let the desktop show through (floating or rounded side panels)"), span(ranges::OPACITY, "")),
        s(Sidebar, APPEARANCE, "/sidebar/look/card_radius", tk("Card radius"), tk("Corner radius of cards in the sidebar and panels"), span(ranges::CARD_RADIUS, " px")),
        s(Sidebar, APPEARANCE, "/sidebar/look/card_padding", tk("Card padding"), tk("Space inside cards"), span(ranges::CARD_PADDING, " px")),
        s(Sidebar, APPEARANCE, "/sidebar/look/card_spacing", tk("Card spacing"), tk("Gap between sidebar cards"), span(ranges::CARD_SPACING, " px")),
        // Sidebars → Motion
        s(Sidebar, MOTION, "/sidebar/look/spring/custom", tk("Custom spring"), tk("Use your own stiffness and damping instead of the global motion settings"), Toggle),
        s(Sidebar, MOTION, "/sidebar/look/spring/stiffness", tk("Stiffness"), tk("Higher is snappier"), span(ranges::SPRING_STIFFNESS, "")),
        s(Sidebar, MOTION, "/sidebar/look/spring/damping", tk("Damping"), tk("Higher settles with less bounce"), span(ranges::SPRING_DAMPING, "")),
    ]
}

/// Why the setting at `path` has no effect under `cfg` (shown in place of
/// its control), so every visible control visibly does something.
pub fn locked(path: &str, cfg: &ShellConfig) -> Option<&'static str> {
    let attached = cfg.bar.effective_layout().is_attached();
    let vertical = cfg.bar.position.is_vertical();
    let compact = !vertical && cfg.bar.layout.is_compact();
    let flush_frame = attached && !cfg.bar.floating_frame;
    let side_floating = cfg.sidebar.look.floats_with(&cfg.bar);
    // Beside a floating bar the side panels always float (no frame to join).
    let bar_floats = !vertical && (!attached || cfg.bar.floating_frame);
    match path {
        "/bar/layout" if vertical => Some(SIDE_BAR_FRAME),
        "/bar/compact_height" if !compact => Some(COMPACT_ONLY),
        "/bar/accent_line" if vertical || cfg.bar.layout != BarLayout::Hairline => Some(HAIRLINE_ONLY),
        "/bar/height" if vertical => Some(SIDE_BAR_WIDTH_INSTEAD),
        "/bar/height" if compact => Some(COMPACT_HEIGHT_INSTEAD),
        "/bar/vertical_width" | "/bar/screen_frame" if !vertical => Some(SIDE_BAR_ONLY),
        "/bar/frame_thickness" if !vertical || !cfg.bar.screen_frame => Some(FRAME_FIRST),
        "/bar/auto_hide/mode" if vertical => Some(SIDE_BAR_STAYS),
        "/bar/floating_frame" if cfg.bar.screen_frame && vertical => Some(FRAME_FIRST_OFF),
        "/bar/flare_radius" | "/bar/floating_frame" if !attached => Some(EDGE_FRAME_ONLY),
        "/bar/edge_gap" | "/bar/rim_width" | "/bar/rim_intensity" | "/bar/shadow" | "/bar/shadow_strength" if flush_frame => Some(FLOATING_ONLY),
        "/bar/shadow_strength" if !cfg.bar.shadow => Some(tk("Turn on Shadow first")),
        "/bar/center_align" if cfg.bar.layout == BarLayout::ContiguousIsland => Some(ISLAND_TOGETHER),
        "/panels/gap" if attached => Some(HANGS_FROM_BAR),
        "/sidebar/look/attach" if bar_floats => Some(tk("Floats beside a floating bar; flush joins the edge frame")),
        "/sidebar/look/margin" if !side_floating => Some(FLOATING_SIDE_ONLY),
        "/sidebar/look/radius" if flush_frame && !side_floating => Some(FLARED_FLUSH),
        "/panels/spring/stiffness" | "/panels/spring/damping" if !cfg.panels.spring.custom => Some(CUSTOM_SPRING_OFF),
        "/sidebar/look/spring/stiffness" | "/sidebar/look/spring/damping" if !cfg.sidebar.look.spring.custom => Some(CUSTOM_SPRING_OFF),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_specific_controls_explain_themselves() {
        let mut cfg = ShellConfig::default();
        assert_eq!(locked("/bar/flare_radius", &cfg), None, "flares belong to the default edge frame");
        assert_eq!(locked("/bar/rim_width", &cfg), Some(FLOATING_ONLY));
        assert_eq!(locked("/bar/corner_radius", &cfg), None, "the bar radius works in every layout");
        cfg.bar.floating_frame = true;
        assert_eq!(locked("/bar/rim_width", &cfg), None, "the floating frame has a rim");
        cfg.bar.layout = BarLayout::FloatingPills;
        assert_eq!(locked("/bar/flare_radius", &cfg), Some(EDGE_FRAME_ONLY));
        assert_eq!(locked("/panels/gap", &cfg), None);
        cfg.bar.layout = BarLayout::ContiguousIsland;
        assert_eq!(locked("/bar/center_align", &cfg), Some(ISLAND_TOGETHER));
        assert_eq!(locked("/panels/spring/damping", &cfg), Some(CUSTOM_SPRING_OFF));
        cfg.panels.spring.custom = true;
        assert_eq!(locked("/panels/spring/damping", &cfg), None);
    }

    #[test]
    fn every_entry_has_a_valid_default_and_range() {
        let cfg = ShellConfig::default();
        for e in entries() {
            let v = super::super::get(&cfg, &e);
            if let Control::Slider { min, max, .. } = e.control {
                let n = v.as_f64().unwrap_or_else(|| panic!("{} default is not a number", e.path));
                assert!((min..=max).contains(&n), "{} default {n} outside {min}..{max}", e.path);
            }
        }
    }
}
