//! Registry entries for the side bars (`[side_bars]`) on the Bar page:
//! the switches, shape, items, material and motion. Which plugins each side
//! bar holds is arranged in the Bar page's layout designer. Slider bounds
//! come from `mandara_config::side_bars::ranges`, the spans the shell
//! clamps to.

use mandara_config::ShellConfig;
use mandara_config::side_bars::{SideBarChips, SideBarShape, ranges};
use mandara_i18n::tk;

use super::{Control, Section, Setting, s, span};

/// Group names (in the Bar page's order).
pub const SIDE_BARS: &str = tk("Side bars");
pub const SIDE_BAR_SHAPE: &str = tk("Side bar shape");
pub const SIDE_BAR_ITEMS: &str = tk("Side bar items");
pub const SIDE_BAR_MATERIAL: &str = tk("Side bar material");

const SHAPES: &[(&str, &str)] = &[("pills", tk("A pill per group")), ("island", tk("One island")), ("slab", tk("A full-length slab"))];
const CHIPS: &[(&str, &str)] = &[("icons", tk("Bare icons")), ("pills", tk("A pill per item")), ("island", tk("An island per group"))];
const ALIGN: &[(&str, &str)] = &[("screen", tk("Middle of the edge")), ("between", tk("Between the others")), ("start", tk("Under the top group")), ("end", tk("Over the bottom group"))];
const ACCENTS: &[(&str, &str)] = &[("theme", tk("Theme accent")), ("secondary", tk("Secondary")), ("tertiary", tk("Tertiary")), ("emerald", tk("Emerald")), ("amber", tk("Amber")), ("coral", tk("Coral"))];

/// Notes shown in place of a control that does nothing right now.
const TURN_ON_FIRST: &str = tk("Turn on Side bars first");
const TOP_OR_BOTTOM_ONLY: &str = tk("Only a top or bottom bar joins the side bars");
const SHARES_BAR_MATERIAL: &str = tk("Joined side bars share the bar's material, so the frame reads as one piece");
const JOINED_IS_FRAME: &str = tk("Joined side bars hang from the bar as one frame");
const FLARES_ONLY: &str = tk("Joined side bars and the slab only");
const RIM_FIRST: &str = tk("Turn on Rim light first");
const WASH_FIRST: &str = tk("Pick pills or islands for the items first");
const UNPIN_FIRST: &str = tk("Unpin a side bar first");
const SIDE_OFF: &str = tk("Turn on this side bar first");
const JOINED_EDGE_FRAME: &str = tk("Joined side bars draw the bar as the edge frame; your style comes back when they're separate");
const SIDE_BAR_EDGE: &str = tk("A side bar holds this edge now; turn side bars off (Bar page) to use the sidebar here");

pub fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    let ms = |r: mandara_config::surfaces::Span| span(r, " ms");
    vec![
        // Bar → Side bars
        s(Bar, SIDE_BARS, "/side_bars/enabled", tk("Side bars"), tk("A slim bar of plugin icons and pills on the left and right edges; panels pour out sideways. They replace the widget sidebar on their edges"), Toggle),
        s(Bar, SIDE_BARS, "/side_bars/joined", tk("Join with the bar"), tk("The top bar and the side bars become one frame with curved corners. The bar is drawn as the edge frame while they're joined; its own style comes back when they're separate"), Toggle),
        s(Bar, SIDE_BARS, "/side_bars/left/enabled", tk("Left side bar"), tk("Show the bar on the left edge"), Toggle),
        s(Bar, SIDE_BARS, "/side_bars/left/pinned", tk("Pin the left side bar"), tk("Pinned, it stays and windows tile beside it; unpinned, it slides away and comes back when the pointer touches the edge"), Toggle),
        s(Bar, SIDE_BARS, "/side_bars/left/accent", tk("Left accent"), tk("The colour its items light up and wash with"), Choice(ACCENTS)),
        s(Bar, SIDE_BARS, "/side_bars/right/enabled", tk("Right side bar"), tk("Show the bar on the right edge"), Toggle),
        s(Bar, SIDE_BARS, "/side_bars/right/pinned", tk("Pin the right side bar"), tk("Pinned, it stays and windows tile beside it; unpinned, it slides away and comes back when the pointer touches the edge"), Toggle),
        s(Bar, SIDE_BARS, "/side_bars/right/accent", tk("Right accent"), tk("The colour its items light up and wash with"), Choice(ACCENTS)),
        s(Bar, SIDE_BARS, "/side_bars/reveal_delay_ms", tk("Reveal delay"), tk("How long the pointer rests on the edge before an unpinned side bar slides out"), ms(ranges::REVEAL_DELAY_MS)),
        s(Bar, SIDE_BARS, "/side_bars/hide_delay_ms", tk("Hide delay"), tk("How long an unpinned side bar waits after the pointer leaves before it slides away"), ms(ranges::HIDE_DELAY_MS)),
        // Bar → Side bar shape
        s(Bar, SIDE_BAR_SHAPE, "/side_bars/width", tk("Width"), tk("How wide each side bar is"), span(ranges::WIDTH, " px")),
        s(Bar, SIDE_BAR_SHAPE, "/side_bars/shape", tk("Shape"), tk("Separate side bars: a glass pill per group, one island, or a slab along the whole edge"), Choice(SHAPES)),
        s(Bar, SIDE_BAR_SHAPE, "/side_bars/edge_gap", tk("Edge gap"), tk("Space between a separate side bar and the screen edge, and around the groups at its ends"), span(ranges::EDGE_GAP, " px")),
        s(Bar, SIDE_BAR_SHAPE, "/side_bars/corner_radius", tk("Corner radius"), tk("Roundness of the pills, island, slab and items"), span(ranges::CORNER_RADIUS, " px")),
        s(Bar, SIDE_BAR_SHAPE, "/side_bars/flare_radius", tk("Corner curves"), tk("The curves where a side bar meets the bar and the screen edge, and where its panels pour out"), span(ranges::FLARE_RADIUS, " px")),
        s(Bar, SIDE_BAR_SHAPE, "/side_bars/align", tk("Middle group"), tk("Where the middle group sits along the edge"), Choice(ALIGN)),
        // Bar → Side bar items
        s(Bar, SIDE_BAR_ITEMS, "/side_bars/chips", tk("Item style"), tk("Bare icons, every item on its own pill, or each group on one island"), Choice(CHIPS)),
        s(Bar, SIDE_BAR_ITEMS, "/side_bars/chip_size", tk("Item size"), tk("How wide each item is across the side bar"), span(ranges::CHIP_SIZE, " px")),
        s(Bar, SIDE_BAR_ITEMS, "/side_bars/chip_padding", tk("Item padding"), tk("Space above and below each item's content"), span(ranges::CHIP_PADDING, " px")),
        s(Bar, SIDE_BAR_ITEMS, "/side_bars/chip_spacing", tk("Item spacing"), tk("Gap between neighbouring items"), span(ranges::CHIP_SPACING, " px")),
        s(Bar, SIDE_BAR_ITEMS, "/side_bars/chip_fill", tk("Wash strength"), tk("How strongly the pills and islands behind items stand out"), span(ranges::CHIP_FILL, "×")),
        // Bar → Side bar material
        s(Bar, SIDE_BAR_MATERIAL, "/side_bars/opacity", tk("Background opacity"), tk("Lower it to let the desktop (or the frosted glass) show through"), span(mandara_config::surfaces::ranges::OPACITY, "")),
        s(Bar, SIDE_BAR_MATERIAL, "/side_bars/tint", tk("Accent tint"), tk("Mixes each side bar's accent into its background"), span(ranges::TINT, "")),
        s(Bar, SIDE_BAR_MATERIAL, "/side_bars/glass", tk("Frosted glass"), tk("Your wallpaper, frosted, behind the side bars; lower the opacity to see it"), Toggle),
        s(Bar, SIDE_BAR_MATERIAL, "/side_bars/rim", tk("Rim light"), tk("A thin specular highlight around the glass"), Toggle),
        s(Bar, SIDE_BAR_MATERIAL, "/side_bars/rim_intensity", tk("Rim intensity"), tk("Strength of the highlight and shade"), span(mandara_config::surfaces::ranges::INTENSITY, "×")),
        s(Bar, SIDE_BAR_MATERIAL, "/side_bars/shadow", tk("Shadow"), tk("A soft shadow under separate side bars"), Toggle),
    ]
}

/// Why the side bar setting at `path` (or a bar / sidebar setting the side
/// bars override) has no effect under `cfg`.
pub fn locked(path: &str, cfg: &ShellConfig) -> Option<&'static str> {
    let sb = &cfg.side_bars;
    let joined = cfg.side_bars_join();
    // Settings the side bars take over while they're on.
    match path {
        "/bar/layout" | "/bar/floating_frame" if joined => return Some(JOINED_EDGE_FRAME),
        p if p.starts_with("/sidebar/") && !p.starts_with("/sidebar/look/") && cfg.sidebar.enabled && !cfg.sidebar_active() => return Some(SIDE_BAR_EDGE),
        _ => {}
    }
    let rest = path.strip_prefix("/side_bars/")?;
    if rest != "enabled" && !sb.enabled {
        return Some(TURN_ON_FIRST);
    }
    let material = matches!(rest, "opacity" | "tint" | "glass" | "rim" | "rim_intensity" | "shadow");
    match rest {
        "joined" if cfg.bar.position.is_vertical() => Some(TOP_OR_BOTTOM_ONLY),
        "shape" if joined => Some(JOINED_IS_FRAME),
        _ if material && joined => Some(SHARES_BAR_MATERIAL),
        "flare_radius" if !joined && sb.shape != SideBarShape::Slab => Some(FLARES_ONLY),
        "rim_intensity" if !sb.rim => Some(RIM_FIRST),
        "chip_fill" if sb.chips == SideBarChips::Icons => Some(WASH_FIRST),
        "reveal_delay_ms" | "hide_delay_ms" if sb.left.pinned && sb.right.pinned => Some(UNPIN_FIRST),
        "left/pinned" | "left/accent" if !sb.left.enabled => Some(SIDE_OFF),
        "right/pinned" | "right/accent" if !sb.right.enabled => Some(SIDE_OFF),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_bar_controls_explain_themselves() {
        let mut cfg = ShellConfig::default();
        assert_eq!(locked("/side_bars/enabled", &cfg), None);
        assert_eq!(locked("/side_bars/width", &cfg), Some(TURN_ON_FIRST));
        assert_eq!(locked("/bar/layout", &cfg), None, "off: the bar keeps its own style");
        assert_eq!(locked("/sidebar/mode", &cfg), None);
        cfg.side_bars.enabled = true;
        assert_eq!(locked("/side_bars/width", &cfg), None);
        assert_eq!(locked("/side_bars/opacity", &cfg), Some(SHARES_BAR_MATERIAL), "joined by default");
        assert_eq!(locked("/side_bars/shape", &cfg), Some(JOINED_IS_FRAME));
        assert_eq!(locked("/bar/layout", &cfg), Some(JOINED_EDGE_FRAME));
        assert_eq!(locked("/sidebar/mode", &cfg), Some(SIDE_BAR_EDGE), "both side bars on: the sidebar gives way");
        assert_eq!(locked("/sidebar/look/radius", &cfg), None, "the power dock still uses the look");
        assert_eq!(locked("/side_bars/reveal_delay_ms", &cfg), Some(UNPIN_FIRST));
        cfg.side_bars.right.pinned = false;
        assert_eq!(locked("/side_bars/reveal_delay_ms", &cfg), None);
        cfg.side_bars.joined = false;
        assert_eq!(locked("/side_bars/opacity", &cfg), None);
        assert_eq!(locked("/side_bars/flare_radius", &cfg), Some(FLARES_ONLY));
        cfg.side_bars.shape = SideBarShape::Slab;
        assert_eq!(locked("/side_bars/flare_radius", &cfg), None);
        assert_eq!(locked("/side_bars/chip_fill", &cfg), Some(WASH_FIRST));
        cfg.side_bars.left.enabled = false;
        assert_eq!(locked("/side_bars/left/accent", &cfg), Some(SIDE_OFF));
    }
}
