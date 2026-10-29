//! Pure geometry for the morphing bar: where each group, the shared backdrop
//! and the expanded dashboard should rest for a given layout. Kept free of
//! GPUI state so it is fully unit-testable.

use gpui::{Bounds, Pixels, Size, point, px, size};
use mandara_config::surfaces::{BarCenterAlign, BarSeparator, PanelAnchor, ranges};
use mandara_config::{BarLayout, BarPosition, ShellConfig};

/// The three widget groups of the bar.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Left,
    Center,
    Right,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Left, Group::Center, Group::Right];

    pub fn index(self) -> usize {
        match self {
            Group::Left => 0,
            Group::Center => 1,
            Group::Right => 2,
        }
    }
}

/// Inset of slot chips inside a floating pill.
pub const PILL_INSET: f32 = 3.0;
/// Vertical inset of chips inside the edge frame.
pub const CHIP_INSET: f32 = 3.0;
/// Gap between groups inside a contiguous island.
pub const ISLAND_GAP: f32 = 12.0;
/// Gap between groups inside the micro pill (tighter: it is one tiny pill).
pub const MICRO_GAP: f32 = 4.0;
/// Inset of chips inside the compact styles (hairline, micro pill).
pub const COMPACT_CHIP_INSET: f32 = 1.0;
/// Smallest gap between zones when the center zone hugs a neighbour.
pub const ZONE_GAP: f32 = 8.0;
/// Smallest chip gap that leaves room for a separator mark.
pub const SEPARATOR_MIN_GAP: f32 = 9.0;
/// Extra margin keeping an attached panel clear of the screen flares.
const FLARE_CLEARANCE: f32 = 2.0;
/// Fade & slide: how far a panel travels from the bar while it fades in.
pub const PANEL_SLIDE_PX: f32 = 14.0;
/// Panels anchored to an edge: chips in the outer thirds of the screen
/// push their panel against that edge.
const EDGE_ANCHOR_THIRD: f32 = 1.0 / 3.0;

/// Everything the bar's geometry depends on, resolved once per frame from
/// the config (clamped, radii scaled by the global roundness): cheap to
/// copy and compare, so it doubles as the "re-layout?" key.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Metrics {
    pub layout: BarLayout,
    pub position: BarPosition,
    pub height: f32,
    pub edge_gap: f32,
    pub side_gap: f32,
    /// The bar's own corner radius (pills, island, edge chips, floating frame).
    pub radius: f32,
    /// Concave flare radius of the edge frame.
    pub flare: f32,
    pub chip_padding: f32,
    /// Gap between chips (widened to fit a separator).
    pub chip_gap: f32,
    pub center: BarCenterAlign,
    /// The edge frame floats clear of the screen edges.
    pub floating_frame: bool,
    /// Drop-down panels: gap to a floating bar, inner padding, width
    /// multiplier, tallest share of the free height, anchoring.
    pub panel_gap: f32,
    pub panel_pad: f32,
    pub panel_width_scale: f32,
    pub panel_max_height: f32,
    pub anchor: PanelAnchor,
    /// A left / right bar: geometry is solved along the bar ("bar space",
    /// main axis = x) and transposed onto the screen by [`orient`].
    pub vertical: bool,
    /// Screen frame thickness around a side bar's display (0 = none).
    pub screen_frame: f32,
    /// Hairline: a 1 px accent line along the bar's inner edge.
    pub accent_line: bool,
}

impl Metrics {
    /// Resolves `cfg` with corner radii scaled by `radius_scale`.
    pub fn of(cfg: &ShellConfig, radius_scale: f32) -> Self {
        let bar = &cfg.bar;
        let d = mandara_config::defaults::BAR_HEIGHT_PX;
        let spacing = bar.chip_spacing();
        let layout = bar.effective_layout();
        let vertical = bar.position.is_vertical();
        let screen_frame = bar.frame_thickness();
        let height = if vertical {
            bar.vertical_width()
        } else if layout.is_compact() {
            bar.compact_height()
        } else {
            ranges::BAR_HEIGHT.clamp(bar.height, d)
        };
        // The hairline is one crisp strip: no flares bulging out of it.
        let flare = if layout == BarLayout::Hairline { 0.0 } else { ranges::BAR_FLARE_RADIUS.clamp(bar.flare_radius, mandara_config::defaults::BAR_FLARE_RADIUS_PX) * radius_scale };
        Self {
            layout,
            position: bar.position,
            height,
            edge_gap: ranges::BAR_EDGE_GAP.clamp(bar.edge_gap, mandara_config::defaults::BAR_EDGE_GAP_PX),
            side_gap: ranges::BAR_SIDE_GAP.clamp(bar.side_gap, mandara_config::defaults::BAR_SIDE_GAP_PX),
            radius: bar.shape_radius() * radius_scale,
            flare,
            chip_padding: bar.chip_padding(),
            chip_gap: if bar.separator == BarSeparator::None { spacing } else { spacing.max(SEPARATOR_MIN_GAP) },
            center: bar.center_align,
            floating_frame: layout.is_attached() && bar.floating_frame && screen_frame <= 0.0,
            panel_gap: cfg.panels.gap(),
            panel_pad: cfg.panels.padding_for(layout.is_attached()),
            panel_width_scale: cfg.panels.width_scale(),
            panel_max_height: cfg.panels.max_height(),
            anchor: cfg.panels.anchor,
            vertical,
            screen_frame,
            accent_line: layout == BarLayout::Hairline && bar.accent_line,
        }
    }

    /// Inset of chips from the bar's edges (smaller in the compact styles).
    pub fn chip_inset(&self) -> f32 {
        if self.layout.is_compact() { COMPACT_CHIP_INSET } else { CHIP_INSET }
    }

    /// Metrics of the default config at roundness 1 (tests, first frame).
    pub fn default_for(layout: BarLayout, position: BarPosition) -> Self {
        let mut cfg = ShellConfig::default();
        cfg.bar.layout = layout;
        cfg.bar.position = position;
        Self::of(&cfg, 1.0)
    }

    pub fn attached(&self) -> bool {
        self.layout.is_attached()
    }

    /// The edge frame's rect (top-oriented): the full edge, or inset by the
    /// edge and side gaps while it floats.
    pub fn frame(&self, surface_width: f32) -> Bounds<Pixels> {
        if self.floating_frame {
            rect(self.side_gap, self.edge_gap, (surface_width - self.side_gap * 2.0).max(0.0), self.height)
        } else {
            rect(0.0, 0.0, surface_width, self.height)
        }
    }

    /// Corner radius of the floating frame (capped at half its height).
    pub fn frame_radius(&self) -> f32 {
        if self.floating_frame { self.radius.min(self.height / 2.0) } else { 0.0 }
    }

    /// Distance from the bar's screen edge to its far side (top-oriented y
    /// of the bar's bottom edge).
    pub fn bar_far_edge(&self) -> f32 {
        if self.attached() && !self.floating_frame { self.height } else { self.edge_gap + self.height }
    }
}

/// Width of a zone made of slots with the given content widths (hidden
/// slots — zero width — take no space).
pub fn zone_width(slot_widths: &[f32], m: &Metrics) -> f32 {
    let mut total = 0.0;
    let mut visible = 0usize;
    for w in slot_widths.iter().copied().filter(|w| *w >= 1.0) {
        total += w + m.chip_padding * 2.0;
        visible += 1;
    }
    if visible == 0 { 0.0 } else { total + m.chip_gap * (visible - 1) as f32 }
}

/// Resting geometry for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct BarGeometry {
    /// Per-group bounds (left, center, right): pills, island segments or
    /// edge-frame hover chips.
    pub pills: [Bounds<Pixels>; 3],
    /// Shared backdrop for island / edge layouts.
    pub backdrop: Bounds<Pixels>,
    /// Whether the per-pill glass is visible (vs the shared backdrop).
    pub separate_pills: bool,
    /// Group corner radius (edge frame: its hover chips).
    pub pill_radius: Pixels,
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

fn x_of(b: &Bounds<Pixels>) -> f32 {
    f32::from(b.origin.x)
}
fn y_of(b: &Bounds<Pixels>) -> f32 {
    f32::from(b.origin.y)
}
fn w_of(b: &Bounds<Pixels>) -> f32 {
    f32::from(b.size.width)
}
fn h_of(b: &Bounds<Pixels>) -> f32 {
    f32::from(b.size.height)
}

/// X of the center zone (width `cw`) between the start zone ending at
/// `start_end` and the end zone beginning at `end_start`.
fn center_x(align: BarCenterAlign, surface_width: f32, start_end: f32, end_start: f32, cw: f32) -> f32 {
    let x = match align {
        BarCenterAlign::Screen => (surface_width - cw) / 2.0,
        BarCenterAlign::Between => start_end + (end_start - start_end - cw) / 2.0,
        BarCenterAlign::Start => start_end + ZONE_GAP,
        BarCenterAlign::End => end_start - ZONE_GAP - cw,
    };
    // Never under a neighbour while there is room beside them; the true
    // screen center stays exact when it fits (the shipped look).
    let (lo, hi) = (start_end + ZONE_GAP, end_start - ZONE_GAP - cw);
    if lo <= hi && align != BarCenterAlign::Screen { x.clamp(lo, hi) } else { x }
}

/// Computes resting geometry. `content` holds each zone's width (see
/// [`zone_width`]); `surface_width` is the layer surface width.
pub fn compute(m: &Metrics, surface_width: f32, content: [f32; 3]) -> BarGeometry {
    let h = m.height;
    let inset_y = m.chip_inset();
    match m.layout {
        BarLayout::MinimalEdge | BarLayout::Hairline => {
            let frame = m.frame(surface_width);
            let chip_h = h - inset_y * 2.0;
            let chip_w = |i: usize| content[i];
            // Outer chips sit inside the screen flares (or the floating
            // frame's rounded ends).
            let inset = if m.floating_frame { x_of(&frame) + m.frame_radius() * 0.5 + inset_y } else { m.side_gap + m.flare * 0.5 };
            let y = y_of(&frame) + inset_y;
            let left = rect(inset, y, chip_w(0), chip_h);
            let right = rect(surface_width - inset - chip_w(2), y, chip_w(2), chip_h);
            let cx = center_x(m.center, surface_width, inset + chip_w(0), x_of(&right), chip_w(1));
            let center = rect(cx, y, chip_w(1), chip_h);
            BarGeometry { pills: [left, center, right], backdrop: frame, separate_pills: false, pill_radius: px(m.radius.min(chip_h / 2.0)) }
        }
        BarLayout::FloatingPills => {
            let pill_w = |i: usize| if content[i] > 0.0 { content[i] + PILL_INSET * 2.0 } else { 0.0 };
            let y = m.edge_gap;
            let left = rect(m.side_gap, y, pill_w(0), h);
            let right = rect(surface_width - m.side_gap - pill_w(2), y, pill_w(2), h);
            let cx = center_x(m.center, surface_width, m.side_gap + pill_w(0), x_of(&right), pill_w(1));
            let center = rect(cx, y, pill_w(1), h);
            BarGeometry { pills: [left, center, right], backdrop: union(&[left, center, right]), separate_pills: true, pill_radius: px(m.radius.min(h / 2.0)) }
        }
        BarLayout::ContiguousIsland | BarLayout::Micro => {
            let y = m.edge_gap;
            // The micro pill: tighter group gaps; empty groups take none.
            let (gap, inset) = if m.layout == BarLayout::Micro { (MICRO_GAP, COMPACT_CHIP_INSET) } else { (ISLAND_GAP, PILL_INSET) };
            let g = |i: usize| if m.layout == BarLayout::Micro && content[i] <= 0.0 { 0.0 } else { gap };
            let total = content.iter().sum::<f32>() + g(0) / 2.0 + g(1) + g(2) / 2.0 + inset * 2.0;
            let total = total.min(surface_width - m.side_gap * 2.0);
            let x0 = (surface_width - total) / 2.0;
            let left = rect(x0, y, content[0] + inset + g(0) / 2.0, h);
            let center = rect(x_of(&left) + w_of(&left), y, content[1] + g(1), h);
            let right = rect(x_of(&center) + w_of(&center), y, content[2] + inset + g(2) / 2.0, h);
            BarGeometry { pills: [left, center, right], backdrop: rect(x0, y, total, h), separate_pills: false, pill_radius: px(m.radius.min(h / 2.0)) }
        }
    }
}

/// Height the bar reserves from its screen edge (exclusive zone).
pub fn exclusive_zone(m: &Metrics) -> f32 {
    m.bar_far_edge()
}

/// Where the dashboard island starts its morph (and returns to): the pill
/// itself for floating layouts; a zero-height sliver under the chip for the
/// edge frame, so the panel pours out of the bar.
pub fn collapsed_rect(m: &Metrics, pill: Bounds<Pixels>) -> Bounds<Pixels> {
    if m.attached() { rect(x_of(&pill), m.bar_far_edge(), w_of(&pill), 0.0) } else { pill }
}

/// Where a panel starts (and returns to) for its open animation: the
/// morph's chip / sliver, a short slide from the bar, or the expanded rect
/// itself (no animation).
pub fn start_rect(m: &Metrics, animation: mandara_config::surfaces::PanelAnimation, pill: Bounds<Pixels>, expanded: Bounds<Pixels>) -> Bounds<Pixels> {
    use mandara_config::surfaces::PanelAnimation;
    match animation {
        PanelAnimation::Morph => collapsed_rect(m, pill),
        PanelAnimation::Slide => rect(x_of(&expanded), y_of(&expanded) - PANEL_SLIDE_PX, w_of(&expanded), h_of(&expanded)),
        PanelAnimation::None => expanded,
    }
}

/// Expanded dashboard rect, clamped inside the surface: centered on the
/// originating chip, the screen, or pushed to the chip's screen edge
/// ([`PanelAnchor`]); at most `panel_max_height` of `free_h`. Attached
/// panels hang from the bar bottom; floating ones float `panel_gap` below
/// the pill.
pub fn dashboard_rect(m: &Metrics, origin_pill: Bounds<Pixels>, dashboard: Size<Pixels>, surface_width: f32, free_h: f32) -> Bounds<Pixels> {
    let attached = m.attached();
    let margin = if attached {
        let frame_inset = if m.floating_frame { m.side_gap + m.frame_radius() } else { 0.0 };
        frame_inset + m.flare * 2.0 + FLARE_CLEARANCE
    } else {
        m.side_gap
    };
    let pad = m.panel_pad * 2.0;
    // Along the bar: the panel's width (its height beside a side bar);
    // away from it: its height (its width beside a side bar).
    let (along, away) = if m.vertical {
        (f32::from(dashboard.height) + pad, f32::from(dashboard.width) * m.panel_width_scale + pad)
    } else {
        (f32::from(dashboard.width) * m.panel_width_scale + pad, f32::from(dashboard.height) + pad)
    };
    let w = along.min(surface_width - margin * 2.0).max(0.0);
    let chip_mid = x_of(&origin_pill) + w_of(&origin_pill) / 2.0;
    let center_x = match m.anchor {
        PanelAnchor::Pill => chip_mid,
        PanelAnchor::Center => surface_width / 2.0,
        PanelAnchor::Edge if chip_mid < surface_width * EDGE_ANCHOR_THIRD => margin + w / 2.0,
        PanelAnchor::Edge if chip_mid > surface_width * (1.0 - EDGE_ANCHOR_THIRD) => surface_width - margin - w / 2.0,
        PanelAnchor::Edge => chip_mid,
    };
    let x = (center_x - w / 2.0).clamp(margin, (surface_width - margin - w).max(margin));
    let top = if attached { m.bar_far_edge() } else { y_of(&origin_pill) + h_of(&origin_pill) + m.panel_gap };
    let max_h = if free_h > 0.0 { free_h * m.panel_max_height } else { f32::MAX };
    rect(x, top, w, away.min(max_h))
}

/// Mirrors a rect vertically inside a surface `surface_h` tall
/// (`y' = surface_h − y − h`). An involution: mirroring twice is identity.
pub fn mirror_y(b: Bounds<Pixels>, surface_h: f32) -> Bounds<Pixels> {
    rect(x_of(&b), surface_h - y_of(&b) - h_of(&b), w_of(&b), h_of(&b))
}

/// Places top-oriented geometry for the bar's screen `position`: identity at
/// the top, mirrored for a bottom bar. Everything in this module is computed
/// for a top bar; mirroring the collapsed and expanded panel rects makes the
/// Bounds spring grow panels *upward* from a bottom bar (the bottom edge of
/// an attached panel stays pinned to the bar while its top rises).
///
/// A side bar's geometry is solved in *bar space* (the bar along x, its
/// far edge at y = thickness) and transposed: a left bar maps bar-space
/// `(x, y)` to screen `(y, x)`, a right bar additionally mirrors x. Panels
/// then grow sideways out of the bar.
pub fn orient(position: BarPosition, b: Bounds<Pixels>, surface: (f32, f32)) -> Bounds<Pixels> {
    match position {
        BarPosition::Top => b,
        BarPosition::Bottom => mirror_y(b, surface.1),
        BarPosition::Left => transpose(b),
        BarPosition::Right => mirror_x(transpose(b), surface.0),
    }
}

/// The inverse of [`orient`]: a screen rect back in bar space.
pub fn unorient(position: BarPosition, b: Bounds<Pixels>, surface: (f32, f32)) -> Bounds<Pixels> {
    match position {
        BarPosition::Top => b,
        BarPosition::Bottom => mirror_y(b, surface.1),
        BarPosition::Left => transpose(b),
        BarPosition::Right => transpose(mirror_x(b, surface.0)),
    }
}

/// Swaps the axes of a rect (`(x, y, w, h)` → `(y, x, h, w)`).
pub fn transpose(b: Bounds<Pixels>) -> Bounds<Pixels> {
    rect(y_of(&b), x_of(&b), h_of(&b), w_of(&b))
}

/// Mirrors a rect horizontally inside a surface `surface_w` wide.
pub fn mirror_x(b: Bounds<Pixels>, surface_w: f32) -> Bounds<Pixels> {
    rect(surface_w - x_of(&b) - w_of(&b), y_of(&b), w_of(&b), h_of(&b))
}

/// The surface's extent along the bar (its width, or height for a side
/// bar) and away from it.
pub fn bar_axes(m: &Metrics, surface: (f32, f32)) -> (f32, f32) {
    if m.vertical { (surface.1, surface.0) } else { surface }
}

/// The screen rect the bar reserves (its exclusive zone along its edge).
pub fn reserved_rect(m: &Metrics, surface: (f32, f32)) -> Bounds<Pixels> {
    let (along, _) = bar_axes(m, surface);
    orient(m.position, rect(0.0, 0.0, along, exclusive_zone(m)), surface)
}

/// The screen frame around a side bar's display: the three thin strips on
/// the edges without the bar, and the inner opening whose corners get
/// concave flares. `None` without a frame.
pub fn screen_frame(m: &Metrics, surface: (f32, f32)) -> Option<([Bounds<Pixels>; 3], Bounds<Pixels>)> {
    let t = m.screen_frame;
    if t <= 0.0 || !m.vertical {
        return None;
    }
    let (along, away) = bar_axes(m, surface);
    let zone = exclusive_zone(m);
    // Bar space: the bar at y = 0..zone; strips at both ends and the far side.
    let strips = [rect(0.0, zone, t, away - zone), rect(along - t, zone, t, away - zone), rect(t, away - t, (along - t * 2.0).max(0.0), t)];
    let inner = rect(t, zone, (along - t * 2.0).max(0.0), (away - zone - t).max(0.0));
    Some((strips.map(|r| orient(m.position, r, surface)), orient(m.position, inner, surface)))
}

/// [`morph_progress`] for rects on the screen: a side bar's panels morph
/// along x, so their rects are taken back to bar space first.
pub fn morph_progress_on(m: &Metrics, current: Bounds<Pixels>, collapsed: Bounds<Pixels>, expanded: Bounds<Pixels>) -> f32 {
    if m.vertical { morph_progress(transpose(current), transpose(collapsed), transpose(expanded)) } else { morph_progress(current, collapsed, expanded) }
}

/// The band of the screen the bar reserves, as `(y, height)` in surface
/// coordinates: from the top edge, or ending at the bottom edge.
pub fn reserved_band(m: &Metrics, surface_h: f32) -> (f32, f32) {
    let zone = exclusive_zone(m);
    match m.position {
        BarPosition::Top => (0.0, zone),
        BarPosition::Bottom => (surface_h - zone, zone),
        // A side bar reserves no height (see [`reserved_rect`]).
        BarPosition::Left | BarPosition::Right => (0.0, 0.0),
    }
}

/// Smallest rect containing all `rects`.
pub fn union(rects: &[Bounds<Pixels>]) -> Bounds<Pixels> {
    let mut it = rects.iter();
    let Some(first) = it.next() else { return Bounds::default() };
    let (mut x0, mut y0) = (x_of(first), y_of(first));
    let (mut x1, mut y1) = (x0 + w_of(first), y0 + h_of(first));
    for r in it {
        x0 = x0.min(x_of(r));
        y0 = y0.min(y_of(r));
        x1 = x1.max(x_of(r) + w_of(r));
        y1 = y1.max(y_of(r) + h_of(r));
    }
    rect(x0, y0, x1 - x0, y1 - y0)
}

/// Morph progress `0..=1` of an island between its collapsed and expanded
/// rects, based on height (the dominant axis of the morph) — or on the
/// travel for a slide that keeps its size.
pub fn morph_progress(current: Bounds<Pixels>, collapsed: Bounds<Pixels>, expanded: Bounds<Pixels>) -> f32 {
    let span = h_of(&expanded) - h_of(&collapsed);
    if span.abs() >= f32::EPSILON {
        return ((h_of(&current) - h_of(&collapsed)) / span).clamp(0.0, 1.0);
    }
    let travel = y_of(&expanded) - y_of(&collapsed);
    if travel.abs() >= f32::EPSILON {
        return ((y_of(&current) - y_of(&collapsed)) / travel).clamp(0.0, 1.0);
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(layout: BarLayout) -> Metrics {
        Metrics::default_for(layout, BarPosition::Top)
    }

    fn edge() -> Metrics {
        m(BarLayout::MinimalEdge)
    }

    /// No free-height cap.
    const FREE: f32 = 0.0;

    #[test]
    fn zone_width_skips_hidden_slots() {
        let e = edge();
        assert_eq!(zone_width(&[], &e), 0.0);
        assert_eq!(zone_width(&[0.0, 0.0], &e), 0.0);
        assert_eq!(zone_width(&[20.0], &e), 20.0 + e.chip_padding * 2.0);
        assert_eq!(zone_width(&[20.0, 0.0, 30.0], &e), 50.0 + e.chip_padding * 4.0 + e.chip_gap);
    }

    #[test]
    fn defaults_are_slim_edge_frame() {
        let c = edge();
        assert!(c.height <= 32.0, "Mandara bars are slim");
        assert_eq!(BarLayout::default(), BarLayout::MinimalEdge);
        assert_eq!(exclusive_zone(&c), c.height);
    }

    #[test]
    fn edge_chips_are_inset_and_centered() {
        let c = edge();
        let g = compute(&c, 1920.0, [120.0, 90.0, 150.0]);
        assert_eq!(g.backdrop, rect(0.0, 0.0, 1920.0, c.height));
        let mid = x_of(&g.pills[1]) + w_of(&g.pills[1]) / 2.0;
        assert!((mid - 960.0).abs() < 0.01);
        assert!(x_of(&g.pills[0]) >= c.side_gap);
        assert!(h_of(&g.pills[0]) < c.height);
    }

    #[test]
    fn floating_pills_are_anchored_left_center_right() {
        let g = compute(&m(BarLayout::FloatingPills), 1920.0, [120.0, 90.0, 150.0]);
        assert_eq!(g.pills[0].origin.x, px(8.0));
        assert_eq!(g.pills[0].size.width, px(120.0 + PILL_INSET * 2.0));
        let r = g.pills[2];
        assert!((x_of(&r) + w_of(&r) - 1912.0).abs() < 0.01);
        assert!(g.separate_pills);
    }

    #[test]
    fn contiguous_island_groups_are_adjacent_and_centered() {
        let g = compute(&m(BarLayout::ContiguousIsland), 1920.0, [120.0, 90.0, 150.0]);
        let [l, c, r] = g.pills;
        assert!((x_of(&l) + w_of(&l) - x_of(&c)).abs() < 0.01);
        assert!((x_of(&c) + w_of(&c) - x_of(&r)).abs() < 0.01);
        assert_eq!(union(&g.pills), g.backdrop);
    }

    #[test]
    fn attached_dashboard_hangs_from_bar() {
        let c = edge();
        let chip = rect(900.0, 4.0, 120.0, 22.0);
        let d = dashboard_rect(&c, chip, size(px(420.0), px(520.0)), 1920.0, FREE);
        assert_eq!(y_of(&d), c.height);
        assert!((x_of(&d) + 210.0 - 960.0).abs() < 0.01);
        let collapsed = collapsed_rect(&c, chip);
        assert_eq!(h_of(&collapsed), 0.0);
        // Near the screen edge the panel stays clear of the screen flare.
        let edge_chip = rect(10.0, 4.0, 80.0, 22.0);
        let d = dashboard_rect(&c, edge_chip, size(px(420.0), px(520.0)), 1920.0, FREE);
        assert!(x_of(&d) >= c.flare * 2.0);
    }

    #[test]
    fn floating_dashboard_floats_below_pill() {
        let pill = rect(900.0, 6.0, 120.0, 30.0);
        let f = m(BarLayout::FloatingPills);
        let d = dashboard_rect(&f, pill, size(px(420.0), px(520.0)), 1920.0, FREE);
        assert_eq!(y_of(&d), 6.0 + 30.0 + f.panel_gap);
        assert_eq!(h_of(&d), 520.0 + f.panel_pad * 2.0);
        assert_eq!(collapsed_rect(&f, pill), pill);
    }

    fn bottom(layout: BarLayout) -> Metrics {
        Metrics::default_for(layout, BarPosition::Bottom)
    }

    #[test]
    fn mirror_is_an_involution_and_top_is_identity() {
        let r = rect(10.0, 6.0, 120.0, 30.0);
        assert_eq!(mirror_y(r, 1440.0), rect(10.0, 1440.0 - 36.0, 120.0, 30.0));
        assert_eq!(mirror_y(mirror_y(r, 1440.0), 1440.0), r);
        assert_eq!(orient(BarPosition::Top, r, (1920.0, 1440.0)), r);
        assert_eq!(orient(BarPosition::Bottom, r, (1920.0, 1440.0)), mirror_y(r, 1440.0));
    }

    #[test]
    fn bottom_edge_frame_sits_on_the_bottom_edge() {
        let c = bottom(BarLayout::MinimalEdge);
        let sh = 1440.0;
        let g = compute(&c, 1920.0, [120.0, 90.0, 150.0]);
        let backdrop = orient(c.position, g.backdrop, (1920.0, sh));
        assert_eq!(y_of(&backdrop) + h_of(&backdrop), sh);
        assert_eq!(y_of(&backdrop), sh - c.height);
        // Chips keep their inset from the bar's edges.
        let chip = orient(c.position, g.pills[0], (1920.0, sh));
        assert_eq!(y_of(&chip), sh - c.height + CHIP_INSET);
        assert_eq!(reserved_band(&c, sh), (sh - c.height, c.height));
        assert_eq!(reserved_band(&edge(), sh), (0.0, edge().height));
    }

    #[test]
    fn bottom_attached_panel_rises_from_bar() {
        let c = bottom(BarLayout::MinimalEdge);
        let sh = 1440.0;
        let bar_top = sh - c.height;
        let g = compute(&c, 1920.0, [120.0, 90.0, 150.0]);
        let chip = g.pills[1]; // top-oriented
        let collapsed = orient(c.position, collapsed_rect(&c, chip), (1920.0, sh));
        let expanded = orient(c.position, dashboard_rect(&c, chip, size(px(420.0), px(520.0)), 1920.0, FREE), (1920.0, sh));
        // Collapsed: a zero-height sliver on the bar's top edge.
        assert_eq!(h_of(&collapsed), 0.0);
        assert_eq!(y_of(&collapsed), bar_top);
        // Expanded: its bottom edge is pinned to the bar, it grows upward.
        assert_eq!(y_of(&expanded) + h_of(&expanded), bar_top);
        assert_eq!(y_of(&expanded), bar_top - 520.0);
        // Linear interpolation (what the Bounds spring does per channel)
        // keeps the bottom edge on the bar throughout the morph.
        for t in [0.25_f32, 0.5, 0.9] {
            let y = y_of(&collapsed) + (y_of(&expanded) - y_of(&collapsed)) * t;
            let h = h_of(&collapsed) + (h_of(&expanded) - h_of(&collapsed)) * t;
            assert!((y + h - bar_top).abs() < 1e-3);
        }
    }

    #[test]
    fn bottom_floating_panel_floats_above_pill() {
        for layout in [BarLayout::FloatingPills, BarLayout::ContiguousIsland] {
            let c = bottom(layout);
            let sh = 1440.0;
            let g = compute(&c, 1920.0, [120.0, 90.0, 150.0]);
            let pill = orient(c.position, g.pills[2], (1920.0, sh));
            assert_eq!(y_of(&pill) + h_of(&pill), sh - c.edge_gap, "pill floats edge_gap above the bottom edge");
            let top_pill = orient(c.position, pill, (1920.0, sh));
            let collapsed = orient(c.position, collapsed_rect(&c, top_pill), (1920.0, sh));
            let expanded = orient(c.position, dashboard_rect(&c, top_pill, size(px(420.0), px(520.0)), 1920.0, FREE), (1920.0, sh));
            assert_eq!(collapsed, pill);
            assert_eq!(y_of(&expanded) + h_of(&expanded), y_of(&pill) - c.panel_gap);
            assert!(y_of(&expanded) >= 0.0);
        }
    }

    #[test]
    fn progress_tracks_height() {
        let pill = rect(0.0, 0.0, 100.0, 40.0);
        let dash = rect(0.0, 0.0, 420.0, 520.0);
        assert_eq!(morph_progress(pill, pill, dash), 0.0);
        assert_eq!(morph_progress(dash, pill, dash), 1.0);
        assert!((morph_progress(rect(0.0, 0.0, 200.0, 280.0), pill, dash) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn every_radius_control_changes_every_layout() {
        let mut cfg = ShellConfig::default();
        for layout in BarLayout::ALL {
            cfg.bar.layout = layout;
            cfg.bar.corner_radius = 4.0;
            let small = compute(&Metrics::of(&cfg, 1.0), 1920.0, [120.0, 90.0, 150.0]).pill_radius;
            cfg.bar.corner_radius = 10.0;
            let big = compute(&Metrics::of(&cfg, 1.0), 1920.0, [120.0, 90.0, 150.0]).pill_radius;
            assert!(big > small, "{layout:?}: pill / chip radius follows bar.corner_radius");
            let none = compute(&Metrics::of(&cfg, 0.0), 1920.0, [120.0, 90.0, 150.0]).pill_radius;
            assert_eq!(none, px(0.0), "{layout:?}: roundness 0 squares the bar");
        }
        // Radii are capped at half the bar height (a pill, never a blob).
        cfg.bar.corner_radius = 22.0;
        cfg.bar.layout = BarLayout::FloatingPills;
        let g = compute(&Metrics::of(&cfg, 2.0), 1920.0, [120.0, 90.0, 150.0]);
        assert_eq!(g.pill_radius, px(cfg.bar.height / 2.0));
    }

    #[test]
    fn floating_frame_insets_the_edge_frame_and_its_panels() {
        let mut cfg = ShellConfig::default();
        cfg.bar.floating_frame = true;
        let f = Metrics::of(&cfg, 1.0);
        let g = compute(&f, 1920.0, [120.0, 90.0, 150.0]);
        assert_eq!(g.backdrop, rect(f.side_gap, f.edge_gap, 1920.0 - f.side_gap * 2.0, f.height));
        assert_eq!(exclusive_zone(&f), f.edge_gap + f.height);
        assert!(f.frame_radius() > 0.0);
        let chip = g.pills[1];
        assert_eq!(y_of(&chip), f.edge_gap + CHIP_INSET);
        let d = dashboard_rect(&f, chip, size(px(420.0), px(520.0)), 1920.0, FREE);
        assert_eq!(y_of(&d), f.edge_gap + f.height, "panels still pour out of the frame");
        // Off for the other styles.
        cfg.bar.layout = BarLayout::FloatingPills;
        assert!(!Metrics::of(&cfg, 1.0).floating_frame);
    }

    #[test]
    fn center_zone_alignment() {
        let mut cfg = ShellConfig::default();
        let content = [300.0, 100.0, 400.0];
        let at = |cfg: &ShellConfig| compute(&Metrics::of(cfg, 1.0), 1920.0, content).pills;
        let [l, c, r] = at(&cfg);
        assert!((x_of(&c) + w_of(&c) / 2.0 - 960.0).abs() < 0.01, "screen center by default");
        cfg.bar.center_align = BarCenterAlign::Start;
        assert!((at(&cfg)[1].origin.x - (l.origin.x + l.size.width) - px(ZONE_GAP)).abs() < px(0.01));
        cfg.bar.center_align = BarCenterAlign::End;
        let c2 = at(&cfg)[1];
        assert!((x_of(&r) - (x_of(&c2) + w_of(&c2)) - ZONE_GAP).abs() < 0.01);
        cfg.bar.center_align = BarCenterAlign::Between;
        let c3 = at(&cfg)[1];
        let (gap_l, gap_r) = (x_of(&c3) - (x_of(&l) + w_of(&l)), x_of(&r) - (x_of(&c3) + w_of(&c3)));
        assert!((gap_l - gap_r).abs() < 0.01, "centered in the free space");
    }

    #[test]
    fn chip_spacing_padding_and_separators_widen_zones() {
        let mut cfg = ShellConfig::default();
        let base = zone_width(&[20.0, 30.0], &Metrics::of(&cfg, 1.0));
        cfg.bar.chip_padding = 14.0;
        cfg.bar.chip_spacing = 6.0;
        let wide = Metrics::of(&cfg, 1.0);
        assert_eq!(zone_width(&[20.0, 30.0], &wide), 50.0 + 14.0 * 4.0 + 6.0);
        assert!(zone_width(&[20.0, 30.0], &wide) > base);
        cfg.bar.chip_spacing = 0.0;
        cfg.bar.separator = BarSeparator::Line;
        assert_eq!(Metrics::of(&cfg, 1.0).chip_gap, SEPARATOR_MIN_GAP, "a separator always gets room");
    }

    #[test]
    fn panel_width_height_anchor_and_animation() {
        let mut cfg = ShellConfig::default();
        let chip = rect(1700.0, 4.0, 60.0, 22.0);
        let natural = size(px(400.0), px(900.0));
        let base = dashboard_rect(&Metrics::of(&cfg, 1.0), chip, natural, 1920.0, 1000.0);
        assert!(h_of(&base) <= 1000.0 * cfg.panels.max_height + 0.01, "tall panels are capped");
        cfg.panels.width_scale = 1.25;
        cfg.panels.anchor = PanelAnchor::Edge;
        let m = Metrics::of(&cfg, 1.0);
        let wide = dashboard_rect(&m, chip, natural, 1920.0, 1000.0);
        assert_eq!(w_of(&wide), 500.0);
        assert!((x_of(&wide) + w_of(&wide) - (1920.0 - m.flare * 2.0 - FLARE_CLEARANCE)).abs() < 0.01, "pushed to the end edge");
        cfg.panels.anchor = PanelAnchor::Center;
        let mid = dashboard_rect(&Metrics::of(&cfg, 1.0), chip, natural, 1920.0, 1000.0);
        assert!((x_of(&mid) + w_of(&mid) / 2.0 - 960.0).abs() < 0.01);
        use mandara_config::surfaces::PanelAnimation;
        let slide = start_rect(&m, PanelAnimation::Slide, chip, wide);
        assert_eq!(slide.size, wide.size);
        assert_eq!(morph_progress(slide, slide, wide), 0.0, "a slide's progress follows its travel");
        assert_eq!(morph_progress(wide, slide, wide), 1.0);
        assert_eq!(start_rect(&m, PanelAnimation::None, chip, wide), wide);
    }

    fn side(position: BarPosition, frame: bool) -> Metrics {
        let mut cfg = ShellConfig::default();
        cfg.bar.position = position;
        cfg.bar.screen_frame = frame;
        Metrics::of(&cfg, 1.0)
    }

    const SCREEN: (f32, f32) = (2560.0, 1440.0);

    #[test]
    fn orient_and_unorient_are_inverse_on_every_edge() {
        let r = rect(10.0, 4.0, 120.0, 30.0);
        for pos in BarPosition::ALL {
            assert_eq!(unorient(pos, orient(pos, r, SCREEN), SCREEN), r, "{pos:?}");
        }
        assert_eq!(orient(BarPosition::Left, r, SCREEN), rect(4.0, 10.0, 30.0, 120.0));
        assert_eq!(orient(BarPosition::Right, r, SCREEN), rect(SCREEN.0 - 34.0, 10.0, 30.0, 120.0));
    }

    #[test]
    fn side_bar_reserves_its_width_on_its_edge_only() {
        for pos in [BarPosition::Left, BarPosition::Right] {
            let m = side(pos, false);
            assert!(m.vertical && m.attached(), "a side bar is an edge frame");
            let w = mandara_config::surfaces::defaults::BAR_VERTICAL_WIDTH_PX;
            assert_eq!(exclusive_zone(&m), w);
            let band = reserved_rect(&m, SCREEN);
            assert_eq!(h_of(&band), SCREEN.1, "the full height");
            assert_eq!(w_of(&band), w);
            assert_eq!(x_of(&band), if pos == BarPosition::Left { 0.0 } else { SCREEN.0 - w });
            assert_eq!(reserved_band(&m, SCREEN.1), (0.0, 0.0), "no height is reserved");
        }
        // The screen frame reserves nothing beyond the bar.
        assert_eq!(exclusive_zone(&side(BarPosition::Left, true)), exclusive_zone(&side(BarPosition::Left, false)));
    }

    #[test]
    fn side_bar_chips_stack_inside_the_strip() {
        let m = side(BarPosition::Left, false);
        let g = compute(&m, SCREEN.1, [120.0, 60.0, 200.0]);
        let strip = reserved_rect(&m, SCREEN);
        for p in g.pills.iter().map(|p| orient(m.position, *p, SCREEN)) {
            // Inside the strip across, stacked along its height.
            assert!(x_of(&p) >= x_of(&strip) && x_of(&p) + w_of(&p) <= x_of(&strip) + w_of(&strip) + 0.01, "{p:?}");
            assert!(y_of(&p) >= 0.0 && y_of(&p) + h_of(&p) <= SCREEN.1);
        }
        let [start, middle, end] = g.pills.map(|p| orient(m.position, p, SCREEN));
        assert!(y_of(&start) < y_of(&middle) && y_of(&middle) < y_of(&end), "start on top, end at the bottom");
        assert!((y_of(&middle) + h_of(&middle) / 2.0 - SCREEN.1 / 2.0).abs() < 0.5, "the middle group is centred");
        // The input region (the backdrop) is the strip itself.
        assert_eq!(orient(m.position, g.backdrop, SCREEN), strip);
    }

    #[test]
    fn side_bar_panels_open_sideways_next_to_their_chip() {
        for pos in [BarPosition::Left, BarPosition::Right] {
            let m = side(pos, false);
            let g = compute(&m, SCREEN.1, [120.0, 60.0, 200.0]);
            let chip = g.pills[2]; // bar space
            let panel = size(px(420.0), px(520.0));
            let free = SCREEN.0 - exclusive_zone(&m);
            let expanded = orient(pos, dashboard_rect(&m, chip, panel, SCREEN.1, free), SCREEN);
            let collapsed = orient(pos, collapsed_rect(&m, chip), SCREEN);
            assert_eq!((w_of(&expanded), h_of(&expanded)), (420.0, 520.0), "screen size kept (width across, height along)");
            let zone = exclusive_zone(&m);
            match pos {
                BarPosition::Left => assert_eq!(x_of(&expanded), zone, "hangs from the bar's right edge"),
                _ => assert_eq!(x_of(&expanded) + w_of(&expanded), SCREEN.0 - zone),
            }
            assert!(y_of(&expanded) >= 0.0 && y_of(&expanded) + h_of(&expanded) <= SCREEN.1);
            assert_eq!(w_of(&collapsed), 0.0, "pours out of the bar: a zero-width sliver");
            assert_eq!(morph_progress_on(&m, collapsed, collapsed, expanded), 0.0);
            assert_eq!(morph_progress_on(&m, expanded, collapsed, expanded), 1.0);
        }
    }

    #[test]
    fn screen_frame_borders_the_other_three_edges() {
        let m = side(BarPosition::Left, true);
        let (strips, inner) = screen_frame(&m, SCREEN).expect("frame on");
        let t = m.screen_frame;
        let zone = exclusive_zone(&m);
        assert_eq!(inner, rect(zone, t, SCREEN.0 - zone - t, SCREEN.1 - t * 2.0));
        for s in strips {
            assert!(w_of(&s).min(h_of(&s)) <= t + 0.01, "{s:?} is thin");
        }
        assert!(screen_frame(&side(BarPosition::Left, false), SCREEN).is_none());
        // Top bars never draw it.
        let mut cfg = ShellConfig::default();
        cfg.bar.screen_frame = true;
        assert!(screen_frame(&Metrics::of(&cfg, 1.0), SCREEN).is_none());
    }

    #[test]
    fn hairline_and_micro_are_compact() {
        let hair = m(BarLayout::Hairline);
        assert_eq!(hair.height, mandara_config::surfaces::defaults::BAR_COMPACT_HEIGHT_PX);
        assert_eq!(hair.flare, 0.0, "a crisp strip");
        assert!(hair.attached() && hair.accent_line);
        let g = compute(&hair, 1920.0, [200.0, 100.0, 300.0]);
        assert_eq!(g.backdrop, rect(0.0, 0.0, 1920.0, hair.height), "full width");
        let micro = m(BarLayout::Micro);
        let g = compute(&micro, 1920.0, [0.0, 180.0, 0.0]);
        let island = compute(&m(BarLayout::ContiguousIsland), 1920.0, [0.0, 180.0, 0.0]);
        assert!(w_of(&g.backdrop) < w_of(&island.backdrop), "tighter than the island");
        assert!((x_of(&g.backdrop) + w_of(&g.backdrop) / 2.0 - 960.0).abs() < 0.5, "centred");
        assert!(h_of(&g.backdrop) < h_of(&island.backdrop));
    }
}
