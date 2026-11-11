//! Pure geometry for the morphing bar: where each group, the shared backdrop
//! and the expanded dashboard should rest for a given layout. Kept free of
//! GPUI state so it is fully unit-testable.

use gpui::{Bounds, Pixels, Size, point, px, size};
use mandara_config::surfaces::{BarCenterAlign, BarSeparator, PanelAnchor, ranges};
use mandara_config::{BarLayout, BarPosition, ShellConfig};
use mandara_core::FlareCenter;
use smallvec::SmallVec;

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
/// Rounding slack when testing whether a panel fits beside the sidebar.
const FIT_SLACK_PX: f32 = 0.01;
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
    /// Notch: the share of the edge it spans.
    pub notch_width: f32,
    /// Notch: how wide its items need it, px ([`notch_fit`]; 0 until they
    /// are measured). It grows past [`Self::notch_width`] to hold them.
    pub notch_fit: f32,
    /// Inset of the items from the bar's long edges (`bar.item_inset`).
    pub item_inset: f32,
    /// The shortest an item is along the bar: its thickness across while
    /// icon-sized items are drawn as round shapes (they're made square),
    /// else 0.
    pub min_chip: f32,
    /// The screen frame of a flush top or bottom edge frame: the thin
    /// border on the edges the bar doesn't cover (0 = none).
    pub frame_strips: f32,
    /// The corner where a joined, pinned left rail meets a top or bottom
    /// bar: a square as wide as the rail (0 = no rail there). The start
    /// zone's first item (the launcher in every preset) sits in its middle
    /// when it fits; the start zone's other items begin clear of the rail.
    pub corner: f32,
    /// That first item's length along the bar ([`Self::corner_length`]; 0
    /// until it is measured).
    pub corner_item: f32,
}

impl Metrics {
    /// Resolves `cfg` with corner radii scaled by `radius_scale`.
    pub fn of(cfg: &ShellConfig, radius_scale: f32) -> Self {
        let bar = &cfg.bar;
        let d = mandara_config::defaults::BAR_HEIGHT_PX;
        let spacing = bar.chip_spacing();
        // Joined side bars draw the bar as the edge frame (`ShellConfig::bar_layout`).
        let layout = cfg.bar_layout();
        let joined = cfg.side_bars_join();
        let vertical = bar.position.is_vertical();
        let frame = bar.frame_thickness();
        // A side bar frames the display (`screen_frame`); a flush,
        // full-width top or bottom bar draws thin strips on the other
        // edges (`frame_strips`). The other styles have no frame.
        let screen_frame = if vertical { frame } else { 0.0 };
        let frame_strips = if !vertical && layout.is_full_width() { frame } else { 0.0 };
        let item_inset = bar.item_inset();
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
            floating_frame: layout.is_full_width() && bar.floating_frame && frame <= 0.0 && !joined,
            panel_gap: cfg.panels.gap(),
            panel_pad: cfg.panels.padding_for(layout.is_attached()),
            panel_width_scale: cfg.panels.width_scale(),
            panel_max_height: cfg.panels.max_height(),
            anchor: cfg.panels.anchor,
            vertical,
            screen_frame,
            accent_line: layout == BarLayout::Hairline && bar.accent_line,
            notch_width: bar.notch_width(),
            notch_fit: 0.0,
            item_inset,
            min_chip: if bar.item_capsules && bar.icon_shape != mandara_config::surfaces::BarIconShape::Match {
                let inset = if layout.is_compact() { COMPACT_CHIP_INSET } else { item_inset };
                (height - inset * 2.0).max(0.0)
            } else {
                0.0
            },
            frame_strips,
            corner: if !vertical && joined && cfg.side_bar_on(false) && cfg.side_bars.left.pinned { cfg.side_bars.width() } else { 0.0 },
            corner_item: 0.0,
        }
    }

    /// How long the start zone's first item is laid out when content
    /// `content` long sits in the rail's corner: an icon that fits the
    /// bar's round items is exactly their square (a round button in the
    /// corner, its padding dropped), other content keeps its padding as far
    /// as the corner allows. Without a corner, or for content wider than
    /// it, its usual [`Self::chip_length`].
    pub fn corner_length(&self, content: f32) -> f32 {
        let full = self.chip_length(content);
        if self.corner <= 0.0 || content > self.corner {
            full
        } else if self.min_chip > 0.0 && content <= self.min_chip {
            self.min_chip
        } else {
            full.min(self.corner)
        }
    }

    /// Whether the start zone's first item sits in the rail's corner: there
    /// is one and what the item draws fits inside it (an icon drawn as a
    /// round shape is its square; the padding round it draws nothing).
    pub fn corner_holds_item(&self) -> bool {
        let round = self.min_chip > 0.0 && self.corner_item <= self.min_chip * crate::bar::items::ICON_ASPECT;
        let drawn = if round { self.corner_item.min(self.min_chip) } else { self.corner_item };
        self.corner > 0.0 && self.corner_item >= 1.0 && drawn <= self.corner + CORNER_SLACK_PX
    }

    /// Where the start zone begins along the bar (the bar's start edge is
    /// 0): its first item centred in the rail's corner, clear of the rail
    /// when that item doesn't fit there, else inside the screen flare.
    pub fn start_inset(&self, surface_width: f32) -> f32 {
        if self.corner_holds_item() {
            (self.corner - self.corner_item) / 2.0
        } else if self.corner > 0.0 {
            self.corner + self.chip_inset()
        } else {
            edge_inset(self, surface_width)
        }
    }

    /// Extra room after the corner's item, so the next item starts clear
    /// of the rail by the item inset (none when the bar's item gap already
    /// does).
    pub fn corner_gap(&self) -> f32 {
        if !self.corner_holds_item() {
            return 0.0;
        }
        let next = (self.corner + self.corner_item) / 2.0 + self.chip_gap;
        (self.corner + self.chip_inset() - next).max(0.0)
    }

    /// Inset of chips from the bar's edges (smaller in the compact styles).
    pub fn chip_inset(&self) -> f32 {
        if self.layout.is_compact() { COMPACT_CHIP_INSET } else { self.item_inset }
    }

    /// Inset of items inside a floating pill or the island (the item
    /// inset; the shipped 3 px by default).
    pub fn pill_inset(&self) -> f32 {
        self.item_inset
    }

    /// An item's length along the bar for content `w` long: the content
    /// and its padding, at least [`Self::min_chip`].
    pub fn chip_length(&self, w: f32) -> f32 {
        (w + self.chip_padding * 2.0).max(self.min_chip)
    }

    /// Where an item's content (`w` long) starts inside its chip: centred
    /// in a squared-up item, after the padding otherwise.
    pub fn content_inset(&self, w: f32) -> f32 {
        (self.chip_length(w) - w) / 2.0
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

    /// The edge frame's rect (top-oriented): the full edge, inset by the
    /// edge and side gaps while it floats, or the notch's middle share,
    /// widened to hold its items ([`Self::notch_fit`]) up to the edge less
    /// its shoulders. Items that need more than that are fitted like any
    /// edge frame's ([`zone_limits`], [`fit_slots`]).
    pub fn frame(&self, surface_width: f32) -> Bounds<Pixels> {
        if self.notch() {
            let w = (surface_width * self.notch_width).round();
            let flare = self.flare.max(0.0);
            let fit = self.notch_fit.min(surface_width - flare * 2.0);
            if fit > w {
                // Grown: whole px with the same room either side (the items
                // are placed from both ends), the shoulders on screen.
                let x = ((surface_width - fit) / 2.0).floor().max(flare.ceil());
                return rect(x, 0.0, (surface_width - x * 2.0).max(0.0), self.height);
            }
            rect(((surface_width - w) / 2.0).round(), 0.0, w, self.height)
        } else if self.floating_frame {
            rect(self.side_gap, self.edge_gap, (surface_width - self.side_gap * 2.0).max(0.0), self.height)
        } else {
            rect(0.0, 0.0, surface_width, self.height)
        }
    }

    /// Corner radius of the floating frame or the notch's far corners
    /// (capped at half its height).
    pub fn frame_radius(&self) -> f32 {
        if self.floating_frame || self.notch() { self.radius.min(self.height / 2.0) } else { 0.0 }
    }

    /// The notch: an attached frame narrower than the edge.
    pub fn notch(&self) -> bool {
        self.layout == BarLayout::Notch && !self.vertical
    }

    /// Distance from the bar's screen edge to its far side (top-oriented y
    /// of the bar's bottom edge).
    pub fn bar_far_edge(&self) -> f32 {
        if self.attached() && !self.floating_frame { self.height } else { self.edge_gap + self.height }
    }
}

/// Rounding slack when testing whether an item fits the rail's corner, px.
const CORNER_SLACK_PX: f32 = 0.01;

/// [`zone_width`] of the bar's start zone: with its first item in the
/// rail's corner, that item at its corner length and the room that keeps
/// the next one clear of the rail.
pub fn start_zone_width(slot_widths: &[f32], m: &Metrics) -> f32 {
    let w = zone_width(slot_widths, m);
    if !m.corner_holds_item() {
        return w;
    }
    let mut visible = slot_widths.iter().copied().filter(|w| *w >= 1.0);
    let trim = visible.next().map_or(0.0, |first| m.chip_length(first) - m.corner_item);
    let gap = if visible.next().is_some() { m.corner_gap() } else { 0.0 };
    w - trim + gap
}

/// Width of a zone made of slots with the given content widths (hidden
/// slots — zero width — take no space).
pub fn zone_width(slot_widths: &[f32], m: &Metrics) -> f32 {
    let mut total = 0.0;
    let mut visible = 0usize;
    for w in slot_widths.iter().copied().filter(|w| *w >= 1.0) {
        total += m.chip_length(w);
        visible += 1;
    }
    if visible == 0 { 0.0 } else { total + m.chip_gap * (visible - 1) as f32 }
}

/// Rounding slack the notch keeps past its items' width, px: the fitting
/// floors each slot's room, which must never squeeze one that fits.
const NOTCH_FIT_SLACK_PX: f32 = 1.0;

/// How wide the notch must be to hold zones made of these slots (content
/// widths in bar order, as for [`zone_width`]) whole: the zones, the gaps
/// that keep them apart ([`zone_limits`]) and its rounded ends, in whole
/// px. 0 without items. Allocation-free: run on every bar layout.
pub fn notch_fit(m: &Metrics, zones: [&[f32]; 3]) -> f32 {
    // Each slot rounded up, so the floored room fitting gives it is never
    // short of its content.
    let fit = |slots: &[f32]| {
        let (mut total, mut visible) = (0.0, 0usize);
        for w in slots.iter().copied().filter(|w| *w >= 1.0) {
            total += m.chip_length(w.ceil());
            visible += 1;
        }
        if visible == 0 { 0.0 } else { total + m.chip_gap * (visible - 1) as f32 }
    };
    let [l, c, r] = zones.map(fit);
    if l + c + r <= 0.0 {
        return 0.0;
    }
    // The middle zone keeps a gap on either side; without one the two
    // sides keep one between them (as `spread_limits` splits the room).
    let gaps = if c > 0.0 { ZONE_GAP * 2.0 } else { ZONE_GAP };
    let ends = (m.frame_radius() * 0.5 + m.chip_inset()) * 2.0;
    (l + c + r + gaps + ends + NOTCH_FIT_SLACK_PX).ceil()
}

/// Narrowest a slot is squeezed to before it is left out of a full zone
/// (room for a word and an ellipsis). A slot that fits whole always stays.
pub const MIN_SQUEEZED_SLOT: f32 = 48.0;

/// Splits `room` between two neighbours that want `a` and `b`: each may take
/// whatever the other leaves, and when both want more than half, each gets
/// half. Returns the most each may take; together they never exceed `room`.
fn fair_split(room: f32, a: f32, b: f32) -> (f32, f32) {
    let room = room.max(0.0);
    let half = room / 2.0;
    (room - b.min(half), room - a.min(half))
}

/// Where the edge styles' outer chips start, from either end of the bar:
/// inside the screen flares, or the floating frame's / the notch's rounded
/// ends.
fn edge_inset(m: &Metrics, surface_width: f32) -> f32 {
    if m.floating_frame || m.notch() { x_of(&m.frame(surface_width)) + m.frame_radius() * 0.5 + m.chip_inset() } else { m.side_gap + m.flare * 0.5 }
}

/// The island's (and micro pill's) gap between groups and inset of chips.
fn island_spacing(m: &Metrics) -> (f32, f32) {
    if m.layout == BarLayout::Micro { (MICRO_GAP, COMPACT_CHIP_INSET) } else { (ISLAND_GAP, m.pill_inset()) }
}

/// [`zone_limits`] for groups placed apart between `start` and `end`
/// (outer widths, so a floating pill's insets included).
fn spread_limits(align: BarCenterAlign, keep_clear: bool, surface_width: f32, (start, end): (f32, f32), [l, c, r]: [f32; 3]) -> [f32; 3] {
    let room = (end - start).max(0.0);
    let c = c.min(room);
    if c <= 0.0 {
        let (a, b) = fair_split(room - ZONE_GAP, l, r);
        return [a, room, b];
    }
    if align == BarCenterAlign::Screen && !keep_clear {
        // The middle group stays on the screen's middle: each side gets
        // the room up to it.
        let x = (surface_width - c) / 2.0;
        return [(x - ZONE_GAP - start).max(0.0), room, (end - x - c - ZONE_GAP).max(0.0)];
    }
    let (a, b) = fair_split(room - c - ZONE_GAP * 2.0, l, r);
    [a, room, b]
}

/// The most each zone (start, centre, end) may take along the bar, in the
/// units of [`zone_width`], so zones never run into each other however long
/// their content (a window title) grows. `content` is what each zone wants.
///
/// The centre zone keeps its place and may take the whole bar. Beside a
/// screen-centred group each side gets the room up to it, less
/// [`ZONE_GAP`]; with the other alignments (and in the narrow notch, which
/// moves its middle group aside) a side may take whatever the other leaves,
/// half each when both are full. The island packs its groups, so only the
/// whole island is bounded (by the bar less its side gaps).
pub fn zone_limits(m: &Metrics, surface_width: f32, content: [f32; 3]) -> [f32; 3] {
    match m.layout {
        BarLayout::MinimalEdge | BarLayout::Hairline | BarLayout::Notch => {
            let inset = edge_inset(m, surface_width);
            spread_limits(m.center, m.notch(), surface_width, (m.start_inset(surface_width), surface_width - inset), content)
        }
        BarLayout::FloatingPills => {
            let pad = m.pill_inset() * 2.0;
            let pills = content.map(|c| if c > 0.0 { c + pad } else { 0.0 });
            spread_limits(m.center, false, surface_width, (m.side_gap, surface_width - m.side_gap), pills).map(|l| (l - pad).max(0.0))
        }
        BarLayout::ContiguousIsland | BarLayout::Micro => {
            let (gap, inset) = island_spacing(m);
            let g = |i: usize| if m.layout == BarLayout::Micro && content[i] <= 0.0 { 0.0 } else { gap };
            let room = (surface_width - m.side_gap * 2.0 - (g(0) / 2.0 + g(1) + g(2) / 2.0 + inset * 2.0)).max(0.0);
            let centre = content[1].min(room);
            let (l, r) = fair_split(room - centre, content[0], content[2]);
            [l, room, r]
        }
    }
}

/// Fits a zone's slots (content widths in bar order, as for
/// [`zone_width`]) into the zone's `limit`, from its outer edge inward (the
/// end zone's outer edge is its end: `from_end`). Returns the most content
/// width each slot may take, whole px, or `None` for a slot left out
/// because the zone is full (and for hidden slots, which take no space).
///
/// A slot's limit depends only on the slots outside it, so a slot squeezed
/// to its limit (a long window title, ellipsized) never feeds back into its
/// own room. The innermost slots give way first: they are squeezed down to
/// [`MIN_SQUEEZED_SLOT`], and the ones with less room than that are left
/// out. The kept slots always fit: their [`zone_width`] is at most `limit`.
pub fn fit_slots(slot_widths: &[f32], m: &Metrics, limit: f32, from_end: bool) -> smallvec::SmallVec<[Option<f32>; 16]> {
    let mut out: smallvec::SmallVec<[Option<f32>; 16]> = smallvec::smallvec![None; slot_widths.len()];
    let mut used = 0.0;
    let mut first = true;
    let n = slot_widths.len();
    for k in 0..n {
        let i = if from_end { n - 1 - k } else { k };
        let w = slot_widths[i];
        if w < 1.0 {
            continue;
        }
        let gap = if first { 0.0 } else { m.chip_gap };
        let room = (limit - used - gap - m.chip_padding * 2.0).floor();
        // (A squared-up item needs its whole square.)
        if (w > room && room < MIN_SQUEEZED_SLOT) || m.chip_length(w.min(room).max(0.0)) > limit - used - gap {
            // Full: this slot and every one inside it stay out.
            break;
        }
        out[i] = Some(room);
        used += gap + m.chip_length(w.min(room));
        first = false;
    }
    out
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
/// `start_end` and the end zone beginning at `end_start`. `keep_clear`
/// keeps even a screen-centred zone off its neighbours while there's room
/// (the notch is too narrow to rely on the screen's middle being free).
fn center_x(align: BarCenterAlign, surface_width: f32, start_end: f32, end_start: f32, cw: f32, keep_clear: bool) -> f32 {
    let x = match align {
        BarCenterAlign::Screen => (surface_width - cw) / 2.0,
        BarCenterAlign::Between => start_end + (end_start - start_end - cw) / 2.0,
        BarCenterAlign::Start => start_end + ZONE_GAP,
        BarCenterAlign::End => end_start - ZONE_GAP - cw,
    };
    // Never under a neighbour while there is room beside them; the true
    // screen center stays exact when it fits (the shipped look).
    let (lo, hi) = (start_end + ZONE_GAP, end_start - ZONE_GAP - cw);
    if lo <= hi && (align != BarCenterAlign::Screen || keep_clear) { x.clamp(lo, hi) } else { x }
}

/// Computes resting geometry. `content` holds each zone's width (see
/// [`zone_width`]); `surface_width` is the layer surface width.
pub fn compute(m: &Metrics, surface_width: f32, content: [f32; 3]) -> BarGeometry {
    let h = m.height;
    let inset_y = m.chip_inset();
    match m.layout {
        BarLayout::MinimalEdge | BarLayout::Hairline | BarLayout::Notch => {
            let frame = m.frame(surface_width);
            let chip_h = h - inset_y * 2.0;
            let chip_w = |i: usize| content[i];
            let inset = edge_inset(m, surface_width);
            let start = m.start_inset(surface_width);
            let y = y_of(&frame) + inset_y;
            let left = rect(start, y, chip_w(0), chip_h);
            let right = rect(surface_width - inset - chip_w(2), y, chip_w(2), chip_h);
            let cx = center_x(m.center, surface_width, start + chip_w(0), x_of(&right), chip_w(1), m.notch());
            let center = rect(cx, y, chip_w(1), chip_h);
            BarGeometry { pills: [left, center, right], backdrop: frame, separate_pills: false, pill_radius: px(m.radius.min(chip_h / 2.0)) }
        }
        BarLayout::FloatingPills => {
            let pill_w = |i: usize| if content[i] > 0.0 { content[i] + m.pill_inset() * 2.0 } else { 0.0 };
            let y = m.edge_gap;
            let left = rect(m.side_gap, y, pill_w(0), h);
            let right = rect(surface_width - m.side_gap - pill_w(2), y, pill_w(2), h);
            let cx = center_x(m.center, surface_width, m.side_gap + pill_w(0), x_of(&right), pill_w(1), false);
            let center = rect(cx, y, pill_w(1), h);
            BarGeometry { pills: [left, center, right], backdrop: union(&[left, center, right]), separate_pills: true, pill_radius: px(m.radius.min(h / 2.0)) }
        }
        BarLayout::ContiguousIsland | BarLayout::Micro => {
            let y = m.edge_gap;
            // The micro pill: tighter group gaps; empty groups take none.
            let (gap, inset) = island_spacing(m);
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
    let margin = panel_margin(m, surface_width);
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

/// How far a panel stays from the ends of the bar (`surface_width` long):
/// clear of the screen flares beside an attached bar (and of a notch's or
/// floating frame's corners), the floating gap otherwise.
fn panel_margin(m: &Metrics, surface_width: f32) -> f32 {
    if m.attached() {
        // A notch's panels stay on its straight edge, clear of its corners.
        let frame_inset = if m.notch() {
            x_of(&m.frame(surface_width)) + m.frame_radius() - m.flare
        } else if m.floating_frame {
            m.side_gap + m.frame_radius()
        } else {
            0.0
        };
        frame_inset + m.flare * 2.0 + FLARE_CLEARANCE
    } else {
        m.side_gap
    }
}

/// The gap a panel keeps from an open sidebar: room for both shapes'
/// flares beside an attached bar, the floating gap otherwise.
pub fn sidebar_gap(m: &Metrics) -> f32 {
    if m.attached() { m.flare * 2.0 + FLARE_CLEARANCE } else { m.side_gap }
}

/// Where the fully open sidebar sits across a surface `surface_width`
/// wide: `(x0, x1)`, `width` wide, `inset` from its edge (a floating
/// sidebar's margin) and beside a side bar `beside_bar` thick on that edge.
pub fn sidebar_span(surface_width: f32, width: f32, inset: f32, beside_bar: f32, right: bool) -> (f32, f32) {
    if right {
        let x1 = surface_width - beside_bar - inset;
        (x1 - width, x1)
    } else {
        let x0 = beside_bar + inset;
        (x0, x0 + width)
    }
}

/// A drop-down panel of a top or bottom bar (top-oriented), kept out from
/// under an open sidebar spanning `side` (see [`sidebar_span`]). A panel
/// that would overlap it moves beside it, [`sidebar_gap`] away and inside
/// the bar's margins, to whichever side moves it least (so it mirrors for
/// a sidebar on the left). One that already clears it, or that neither
/// side has room for (a narrow screen), keeps its place and is drawn over
/// the sidebar.
pub fn clear_of_sidebar(m: &Metrics, panel: Bounds<Pixels>, side: (f32, f32), surface_width: f32) -> Bounds<Pixels> {
    let (x, w) = (x_of(&panel), w_of(&panel));
    let (s0, s1) = side;
    if x + w <= s0 || x >= s1 || s1 <= s0 {
        return panel;
    }
    let (margin, gap) = (panel_margin(m, surface_width), sidebar_gap(m));
    let fits = |nx: f32| nx >= margin - FIT_SLACK_PX && nx + w <= surface_width - margin + FIT_SLACK_PX;
    [s0 - gap - w, s1 + gap]
        .into_iter()
        .filter(|&nx| fits(nx))
        .min_by(|a, b| (a - x).abs().total_cmp(&(b - x).abs()))
        .map_or(panel, |nx| rect(nx, y_of(&panel), w, h_of(&panel)))
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

/// Where `at` (output px) falls along the bar, when it lies in the band
/// the bar takes along its edge on an output `output` px big: the whole
/// length of the edge, from the screen edge out to the bar's far side, so
/// the margin between a floating bar and the edge and every gap between
/// its pieces count. The screen frame on the other edges does not. A
/// handful of comparisons: no state, nothing allocated.
pub fn band_hit(m: &Metrics, output: (f32, f32), at: (f32, f32)) -> Option<f32> {
    let band = reserved_rect(m, output);
    let (x, y) = at;
    let (x0, y0) = (x_of(&band), y_of(&band));
    let inside = x >= x0 && x < x0 + w_of(&band) && y >= y0 && y < y0 + h_of(&band);
    inside.then_some(if m.vertical { y } else { x })
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

/// A rect as plain numbers `(x, y, w, h)`.
pub type Strip = (f32, f32, f32, f32);

/// The thin screen frame of a flush top or bottom bar
/// ([`Metrics::frame_strips`]): its strips and fillets, surface px.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EdgeFrame {
    /// The strip along the far edge, then the left and right ones (none on
    /// a side a joined side bar covers).
    pub strips: [Option<Strip>; 3],
    /// Fillets in the opening's two far corners, `(x, y, r, centre)`: the
    /// `r × r` square at `(x, y)` and its arc's centre.
    pub flares: [Option<(f32, f32, f32, FlareCenter)>; 2],
    /// How far in from the screen's sides the bar's own fillets sit.
    pub inset: f32,
}

/// The screen frame of the flush top or bottom bar `m` on a `surface`
/// whose inner edge is at `bar_edge` (surface y: the bar's bottom for a top
/// bar, its top for a bottom one), with fillets of radius `flare`.
/// `covered` says which sides (left, right) a joined side bar covers: it
/// draws that side's strip and far fillet itself. `None` without a frame.
pub fn edge_frame(m: &Metrics, surface: (f32, f32), bar_edge: f32, flare: f32, covered: [bool; 2]) -> Option<EdgeFrame> {
    let t = m.frame_strips;
    if t <= 0.0 || m.vertical || !m.attached() || m.floating_frame || m.notch() {
        return None;
    }
    let (w, h) = surface;
    let r = flare.max(0.0);
    let bottom = m.position == BarPosition::Bottom;
    // The opening between the bar and the far strip.
    let (open_y0, open_y1) = if bottom { (t, bar_edge) } else { (bar_edge, h - t) };
    let far = if bottom { (0.0, 0.0, w, t) } else { (0.0, h - t, w, t) };
    let side = |x: f32| (x, open_y0, t, (open_y1 - open_y0).max(0.0));
    let (left_c, right_c) = if bottom { (FlareCenter::BottomRight, FlareCenter::BottomLeft) } else { (FlareCenter::TopRight, FlareCenter::TopLeft) };
    let flare_y = if bottom { open_y0 } else { open_y1 - r };
    let fillet = |x: f32, c: FlareCenter| (r > 0.0).then_some((x, flare_y, r, c));
    Some(EdgeFrame {
        strips: [Some(far), (!covered[0]).then(|| side(0.0)), (!covered[1]).then(|| side(w - t))],
        flares: [if covered[0] { None } else { fillet(t, left_c) }, if covered[1] { None } else { fillet(w - t - r, right_c) }],
        inset: t,
    })
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

/// A flush side panel (the sidebar) joined to the bar and the screen like
/// a puzzle piece: one column of the bar's material, concave fillets where
/// its inner edge meets the bar or a screen edge, and, beside a notch, the
/// span the notch keeps once the column butts against it.
#[derive(Clone, Debug, PartialEq)]
pub struct SideColumn {
    /// The column, `(x, y, w, h)` in surface px.
    pub column: (f32, f32, f32, f32),
    /// Fillets: the `r × r` square at `(x, y)` and its arc's centre.
    pub flares: Flares,
    /// Beside a notch the column reaches its edge; once it reaches the
    /// notch the two join: the notch's span `(x, w)` along the edge and
    /// whether its start / end butts against the column (square, no
    /// shoulder).
    pub notch: Option<(f32, f32, bool, bool)>,
}

/// A column joins at its two ends; neither join needs a heap buffer.
pub type Flares = SmallVec<[(f32, f32, f32, FlareCenter); 2]>;

/// How the flush side panel at `x` (its slid position, width `w`, on the
/// right edge when `right`) joins the bar of `m` on a `surface`: its box
/// `span` (`(y, h)`: the height beside the bar, or the whole height beside a
/// side bar) and fillets of radius `flare` at both ends of its inner edge.
/// A column only reaches a screen edge the bar doesn't cover; next to the
/// notch it rises to the notch's edge and merges into the notch.
pub fn side_column(m: &Metrics, surface: (f32, f32), x: f32, w: f32, right: bool, span: (f32, f32), flare: f32) -> SideColumn {
    let (sw, sh) = surface;
    let r = flare.max(0.0);
    // The inner edge, and where its fillets' squares start.
    let inner = if right { x } else { x + w };
    let fx = if right { inner - r } else { inner };
    // Arc centres of a fillet below a horizontal surface (top) and above one
    // (bottom): the filled part hugs the column and that surface.
    let (top_c, bottom_c) = if right { (FlareCenter::BottomLeft, FlareCenter::TopLeft) } else { (FlareCenter::BottomRight, FlareCenter::TopRight) };
    let (mut y0, mut y1) = (span.0, span.0 + span.1);
    let mut flares = Flares::new();
    let mut notch = None;
    if m.notch() {
        let frame = m.frame(sw);
        let (nx, nw, nh) = (x_of(&frame), w_of(&frame), m.height);
        let bottom = m.position == BarPosition::Bottom;
        // The column reaches the notch's edge.
        if bottom { y1 = sh } else { y0 = 0.0 }
        // Joined once the column's shoulder would touch the notch's.
        let joined = if right { inner <= nx + nw + r } else { inner >= nx - r };
        if joined {
            notch = Some(if right {
                let end = inner.clamp(nx, nx + nw);
                (nx, end - nx, false, true)
            } else {
                let start = inner.clamp(nx, nx + nw);
                // A column short of the notch stretches the notch to it.
                let start = if inner < nx { inner } else { start };
                (start, nx + nw - start, true, false)
            });
            flares.push(if bottom { (fx, sh - nh - r, r, bottom_c) } else { (fx, nh, r, top_c) });
        } else {
            flares.push(if bottom { (fx, sh - r, r, bottom_c) } else { (fx, 0.0, r, top_c) });
        }
        flares.push(if bottom { (fx, 0.0, r, top_c) } else { (fx, sh - r, r, bottom_c) });
    } else {
        flares.push((fx, y0, r, top_c));
        flares.push((fx, y1 - r, r, bottom_c));
    }
    flares.retain(|f| f.2 > 0.0);
    SideColumn { column: (x, y0, w, (y1 - y0).max(0.0)), flares, notch }
}

/// Smallest rect containing all `rects`.
pub fn union(rects: &[Bounds<Pixels>]) -> Bounds<Pixels> {
    union_iter(rects.iter().copied()).unwrap_or_default()
}

/// Smallest containing rect without collecting a filtered group into a buffer.
/// Empty groups have no bounds; callers decide whether to draw them.
pub fn union_iter(rects: impl IntoIterator<Item = Bounds<Pixels>>) -> Option<Bounds<Pixels>> {
    let mut it = rects.into_iter();
    let first = it.next()?;
    let (mut x0, mut y0) = (x_of(&first), y_of(&first));
    let (mut x1, mut y1) = (x0 + w_of(&first), y0 + h_of(&first));
    for r in it {
        x0 = x0.min(x_of(&r));
        y0 = y0.min(y_of(&r));
        x1 = x1.max(x_of(&r) + w_of(&r));
        y1 = y1.max(y_of(&r) + h_of(&r));
    }
    Some(rect(x0, y0, x1 - x0, y1 - y0))
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

    #[test]
    fn notch_hangs_in_the_middle_and_keeps_its_groups_apart() {
        let mut m = Metrics::default_for(BarLayout::Notch, BarPosition::Top);
        m.notch_width = 0.5;
        let frame = m.frame(1920.0);
        assert_eq!((x_of(&frame), w_of(&frame), y_of(&frame)), (480.0, 960.0, 0.0));
        assert!(m.attached() && !m.floating_frame);
        let g = compute(&m, 1920.0, [200.0, 300.0, 300.0]);
        assert!(x_of(&g.pills[0]) >= x_of(&frame), "the left group sits inside the notch");
        assert!(x_of(&g.pills[2]) + w_of(&g.pills[2]) <= x_of(&frame) + w_of(&frame), "so does the right one");
        assert!(x_of(&g.pills[1]) + w_of(&g.pills[1]) <= x_of(&g.pills[2]), "the middle group keeps clear of the right one");
        // Panels stay on the notch's straight edge.
        let panel = dashboard_rect(&m, g.pills[0], size(px(300.0), px(200.0)), 1920.0, 1000.0);
        assert!(x_of(&panel) >= x_of(&frame) + m.frame_radius());
    }

    #[test]
    fn a_notch_grows_to_hold_its_items_and_squeezes_only_at_the_edge() {
        use mandara_config::bar_styles::BarStyle;
        let sw = 1920.0;
        let mut m = styled(BarStyle::PharosBeam);
        let share = m.frame(sw);
        assert_eq!(w_of(&share), 960.0, "half the edge while its items fit");
        // The items: content widths of the slots in each zone.
        let fitted = |m: &Metrics, zones: [&[f32]; 3]| {
            let wanted = zones.map(|z| zone_width(z, m));
            let limits = zone_limits(m, sw, wanted);
            std::array::from_fn::<_, 3, _>(|z| fit_slots(zones[z], m, limits[z], z == 2))
        };
        let whole = |zones: [&[f32]; 3], fit: &[smallvec::SmallVec<[Option<f32>; 16]>; 3]| {
            zones.iter().zip(fit).all(|(z, f)| z.iter().zip(f).all(|(w, l)| l.is_some_and(|l| l >= *w)))
        };
        // Few items: the notch keeps its share.
        let small: [&[f32]; 3] = [&[24.0, 180.0], &[60.0, 120.0], &[24.0, 24.0, 24.0]];
        m.notch_fit = notch_fit(&m, small);
        assert!(m.notch_fit < 960.0);
        assert_eq!(m.frame(sw), share);
        assert!(whole(small, &fitted(&m, small)));
        // Wider than half the output (the shipped items): it grows to
        // hold every one whole, centred, and stays inside its shoulders.
        let wide: [&[f32]; 3] = [&[24.0, 320.5, 96.0], &[60.0, 220.0], &[90.0, 140.3, 24.0, 24.0, 24.0, 24.0]];
        m.notch_fit = notch_fit(&m, wide);
        assert!(m.notch_fit > 960.0 && m.notch_fit < sw - m.flare * 2.0);
        let frame = m.frame(sw);
        assert!(w_of(&frame) >= m.notch_fit && w_of(&frame) <= m.notch_fit + 1.0);
        assert_eq!(x_of(&frame), sw - x_of(&frame) - w_of(&frame), "centred on whole px");
        let fit = fitted(&m, wide);
        assert!(whole(wide, &fit), "nothing squeezed or left out");
        let widths = std::array::from_fn(|z| zone_width(wide[z], &m));
        let g = compute(&m, sw, widths);
        assert!(x_of(&g.pills[0]) >= x_of(&frame) && x_of(&g.pills[2]) + w_of(&g.pills[2]) <= x_of(&frame) + w_of(&frame));
        assert!(x_of(&g.pills[0]) + w_of(&g.pills[0]) <= x_of(&g.pills[1]) && x_of(&g.pills[1]) + w_of(&g.pills[1]) <= x_of(&g.pills[2]));
        // Wider than the output: the notch stops at the edge less its
        // shoulders, and the edge frame's fitting squeezes the innermost
        // slot (a long title) instead of leaving items out.
        let huge: [&[f32]; 3] = [&[24.0, 320.0, 1400.0], &[60.0, 220.0], &[90.0, 140.0, 24.0, 24.0]];
        m.notch_fit = notch_fit(&m, huge);
        assert!(m.notch_fit > sw);
        let frame = m.frame(sw);
        assert!(x_of(&frame) >= m.flare && x_of(&frame) + w_of(&frame) <= sw - m.flare, "the shoulders stay on screen");
        let fit = fitted(&m, huge);
        assert!(fit.iter().all(|z| z.iter().all(Option::is_some)), "every item kept");
        assert!(fit[0][2].is_some_and(|l| l < 1400.0), "the title is squeezed");
        // A notch set wider than that by hand keeps its width.
        m.notch_width = 1.0;
        assert_eq!(w_of(&m.frame(sw)), sw);
        // No items yet: the configured share.
        let mut fresh = styled(BarStyle::PharosBeam);
        fresh.notch_fit = notch_fit(&fresh, [&[], &[0.0], &[]]);
        assert_eq!((fresh.notch_fit, fresh.frame(sw)), (0.0, share));
    }

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
        assert_eq!(y_of(&chip), sh - c.height + c.chip_inset());
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
        assert_eq!(y_of(&chip), f.edge_gap + f.chip_inset());
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

    /// One frame of the shell's zone layout: limits from what each zone
    /// shows (`measured`), every zone fitted into its limit, then the
    /// geometry. Returns it and what each slot measures next frame (a
    /// squeezed slot measures its limit; a left-out one is drawn off
    /// screen at its own width).
    fn fit_frame(m: &Metrics, along: f32, intrinsic: &[Vec<f32>; 3], measured: &[Vec<f32>; 3]) -> (BarGeometry, [Vec<f32>; 3]) {
        let wanted: [f32; 3] = std::array::from_fn(|z| zone_width(&measured[z], m));
        let limits = zone_limits(m, along, wanted);
        let mut next = intrinsic.clone();
        let content: [f32; 3] = std::array::from_fn(|z| {
            let fit = fit_slots(&measured[z], m, limits[z], z == Group::Right.index());
            let kept: Vec<f32> = measured[z].iter().zip(&fit).map(|(w, f)| f.map_or(0.0, |l| w.min(l))).collect();
            for (i, f) in fit.iter().enumerate() {
                next[z][i] = f.map_or(intrinsic[z][i], |l| intrinsic[z][i].min(l));
            }
            let total = zone_width(&kept, m);
            assert!(total <= limits[z] + 0.01, "zone {z} fits its limit: {total} > {}", limits[z]);
            total
        });
        (compute(m, along, content), next)
    }

    fn end_of(b: &Bounds<Pixels>) -> f32 {
        x_of(b) + w_of(b)
    }

    #[test]
    fn zones_never_overlap_in_any_style_position_alignment_or_width() {
        // A long window title on the start side, a clock in the middle, a
        // few icons at the end (issue #21).
        let intrinsic = [vec![24.0, 70.0, 900.0], vec![110.0], vec![60.0, 40.0, 30.0]];
        let layouts = [BarLayout::MinimalEdge, BarLayout::FloatingPills, BarLayout::ContiguousIsland, BarLayout::Hairline, BarLayout::Notch, BarLayout::Micro];
        let aligns = [BarCenterAlign::Screen, BarCenterAlign::Between, BarCenterAlign::Start, BarCenterAlign::End];
        for layout in layouts {
            for position in BarPosition::ALL {
                for align in aligns {
                    for along in [360.0, 480.0, 800.0, 1280.0, 1920.0, 2560.0] {
                        let mut m = Metrics::default_for(layout, position);
                        m.center = align;
                        let mut measured = intrinsic.clone();
                        let mut last = None;
                        // Every frame is clean, and a few frames settle it.
                        for _ in 0..4 {
                            let (g, next) = fit_frame(&m, along, &intrinsic, &measured);
                            let [l, c, r] = &g.pills;
                            let what = format!("{layout:?} {position:?} {align:?} at {along}");
                            for p in &g.pills {
                                assert!(x_of(p) >= -0.01 && end_of(p) <= along + 0.01, "{what}: {p:?} stays on the bar");
                            }
                            if w_of(c) > 0.0 {
                                assert!(end_of(l) <= x_of(c) + 0.01, "{what}: the start zone ends before the centre one ({l:?} / {c:?})");
                                assert!(end_of(c) <= x_of(r) + 0.01, "{what}: the centre zone ends before the end one ({c:?} / {r:?})");
                            }
                            assert!(end_of(l) <= x_of(r) + 0.01, "{what}: the start zone ends before the end one");
                            measured = next;
                            last = Some(g);
                        }
                        let (settled, _) = fit_frame(&m, along, &intrinsic, &measured);
                        assert_eq!(Some(settled), last, "{layout:?} {position:?} {align:?} at {along}: settles");
                    }
                }
            }
        }
    }

    #[test]
    fn a_long_title_is_squeezed_beside_a_screen_centred_clock() {
        let m = Metrics::default_for(BarLayout::MinimalEdge, BarPosition::Top);
        let zones = [vec![24.0, 900.0], vec![110.0], vec![60.0]];
        // Without fitting, the title runs under the clock (the bug).
        let raw = compute(&m, 1280.0, std::array::from_fn(|z| zone_width(&zones[z], &m)));
        assert!(end_of(&raw.pills[0]) > x_of(&raw.pills[1]));
        let (g, next) = fit_frame(&m, 1280.0, &zones, &zones);
        let [l, c, _] = &g.pills;
        assert!((x_of(c) + w_of(c) / 2.0 - 640.0).abs() < 0.01, "the clock stays on the screen's middle");
        assert!((x_of(c) - end_of(l) - ZONE_GAP).abs() < 1.0, "the title takes the room up to it, less the gap");
        assert!(next[0][1] < 900.0 && next[0][1] >= MIN_SQUEEZED_SLOT, "the title is ellipsized, not left out");
        assert_eq!(next[0][0], 24.0, "the outer chip keeps its width");
        // A bar that fits is laid out exactly as before.
        let small = [vec![24.0, 200.0], vec![110.0], vec![60.0]];
        let (fits, _) = fit_frame(&m, 1920.0, &small, &small);
        assert_eq!(fits, compute(&m, 1920.0, std::array::from_fn(|z| zone_width(&small[z], &m))));
    }

    #[test]
    fn slots_give_way_from_the_inside_and_the_last_one_drops_out() {
        let m = Metrics::default_for(BarLayout::MinimalEdge, BarPosition::Top);
        let pad = m.chip_padding * 2.0;
        // Room for the first slot whole and a squeezed second one.
        let limit = 40.0 + pad + m.chip_gap + 100.0 + pad;
        let fit = fit_slots(&[40.0, 300.0, 50.0], &m, limit, false);
        assert_eq!(fit[0], Some((limit - pad).floor()));
        assert_eq!(fit[1], Some(100.0), "squeezed into what is left");
        assert_eq!(fit[2], None, "no room left: left out");
        // The end zone gives way from its start (the side facing the centre).
        let fit = fit_slots(&[50.0, 300.0, 40.0], &m, limit, true);
        assert_eq!((fit[0], fit[1]), (None, Some(100.0)));
        // Hidden slots take no room; a slot that fits whole is never left out.
        let fit = fit_slots(&[0.0, 30.0], &m, 30.0 + pad, false);
        assert_eq!(fit, smallvec::SmallVec::<[Option<f32>; 16]>::from_slice(&[None, Some(30.0)]));
        assert_eq!(fit_slots(&[20.0], &m, 20.0 + pad, false)[0], Some(20.0), "fits exactly, below the squeeze minimum");
        assert_eq!(fit_slots(&[200.0], &m, 30.0 + pad, false)[0], None, "too little room to squeeze into");
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

    /// Issue #62: a drop-down never ends up under an open sidebar.
    #[test]
    fn panels_move_beside_an_open_sidebar() {
        let natural = size(px(500.0), px(400.0));
        for m in [edge(), m(BarLayout::FloatingPills)] {
            let (margin, gap) = (panel_margin(&m, 1920.0), sidebar_gap(&m));
            // Sidebar on the right; the battery chip sits above it.
            let chip = rect(1780.0, 4.0, 60.0, 22.0);
            let panel = dashboard_rect(&m, chip, natural, 1920.0, 1000.0);
            let side = sidebar_span(1920.0, 420.0, 0.0, 0.0, true);
            assert_eq!(side, (1500.0, 1920.0));
            assert!(x_of(&panel) + w_of(&panel) > side.0, "it would overlap");
            let moved = clear_of_sidebar(&m, panel, side, 1920.0);
            assert!((x_of(&moved) + w_of(&moved) - (side.0 - gap)).abs() < 0.01, "beside it, its gap away");
            assert_eq!((moved.size, y_of(&moved)), (panel.size, y_of(&panel)), "only its x changes");
            // Mirrored: a sidebar on the left (RTL or by choice).
            let chip = rect(80.0, 4.0, 60.0, 22.0);
            let panel = dashboard_rect(&m, chip, natural, 1920.0, 1000.0);
            let side = sidebar_span(1920.0, 420.0, 12.0, 0.0, false);
            assert_eq!(side, (12.0, 432.0));
            let moved = clear_of_sidebar(&m, panel, side, 1920.0);
            assert!((x_of(&moved) - (side.1 + gap)).abs() < 0.01);
            // A panel already clear of it stays put.
            let far = dashboard_rect(&m, rect(1000.0, 4.0, 60.0, 22.0), natural, 1920.0, 1000.0);
            assert_eq!(clear_of_sidebar(&m, far, side, 1920.0), far);
            // Narrow screen: no room beside the sidebar, so it stays where
            // it was (and is drawn over the sidebar instead).
            let chip = rect(700.0, 4.0, 60.0, 22.0);
            let panel = dashboard_rect(&m, chip, natural, 800.0, 1000.0);
            let side = sidebar_span(800.0, 420.0, 0.0, 0.0, true);
            assert_eq!(clear_of_sidebar(&m, panel, side, 800.0), panel);
            // Never off the screen: the moved panel keeps the bar's margins.
            let side = sidebar_span(1100.0, 420.0, 0.0, 0.0, true);
            let panel = dashboard_rect(&m, rect(1000.0, 4.0, 60.0, 22.0), natural, 1100.0, 1000.0);
            let moved = clear_of_sidebar(&m, panel, side, 1100.0);
            assert!(x_of(&moved) >= margin - 0.01 && x_of(&moved) + w_of(&moved) <= side.0 - gap + 0.01);
        }
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

    /// A bar's band: the whole length of its edge, from the screen edge to
    /// the bar's far side (the margin of a floating bar included), on every
    /// edge; the screen frame on the other edges is the desktop's.
    #[test]
    fn the_band_hit_covers_the_bars_strip_on_every_edge() {
        let (w, h) = SCREEN;
        let split = |position: BarPosition, edge_gap: f32| {
            let mut cfg = ShellConfig::default();
            cfg.bar.layout = BarLayout::FloatingPills;
            cfg.bar.position = position;
            cfg.bar.height = 36.0;
            cfg.bar.edge_gap = edge_gap;
            Metrics::of(&cfg, 1.0)
        };
        // Top: the 10 px margin above the pills, the gaps between them, and
        // the far ends of the edge are all in the band; just past it isn't.
        let top = split(BarPosition::Top, 10.0);
        assert_eq!(exclusive_zone(&top), 46.0);
        assert_eq!(band_hit(&top, SCREEN, (5.0, 2.0)), Some(5.0), "the margin by the screen edge");
        assert_eq!(band_hit(&top, SCREEN, (1200.0, 45.5)), Some(1200.0), "the pills' far side");
        assert_eq!(band_hit(&top, SCREEN, (w - 0.5, 20.0)), Some(w - 0.5), "the far end of the edge");
        assert_eq!(band_hit(&top, SCREEN, (1200.0, 46.0)), None, "just below the band");
        assert_eq!(band_hit(&top, SCREEN, (1200.0, 700.0)), None, "the desktop");
        // Bottom: mirrored.
        let bottom = split(BarPosition::Bottom, 10.0);
        assert_eq!(band_hit(&bottom, SCREEN, (300.0, h - 1.0)), Some(300.0), "the margin by the screen edge");
        assert_eq!(band_hit(&bottom, SCREEN, (300.0, h - 46.0)), Some(300.0), "the pills' far side");
        assert_eq!(band_hit(&bottom, SCREEN, (300.0, h - 46.5)), None, "just above the band");
        assert_eq!(band_hit(&bottom, SCREEN, (300.0, 10.0)), None, "the top edge isn't the bar's");
        // No margin: the band is the bar's height.
        let flush = split(BarPosition::Top, 0.0);
        assert_eq!(band_hit(&flush, SCREEN, (10.0, 35.5)), Some(10.0));
        assert_eq!(band_hit(&flush, SCREEN, (10.0, 36.0)), None);
        // A floating edge frame keeps its margin in the band too.
        let mut cfg = ShellConfig::default();
        cfg.bar.floating_frame = true;
        cfg.bar.edge_gap = 8.0;
        let floating = Metrics::of(&cfg, 1.0);
        assert_eq!(band_hit(&floating, SCREEN, (4.0, 3.0)), Some(4.0));
        assert_eq!(band_hit(&floating, SCREEN, (4.0, exclusive_zone(&floating))), None);
        // A flush top bar's screen frame (the strips on the other edges) is
        // the desktop's.
        let mut cfg = ShellConfig::default();
        cfg.bar.screen_frame = true;
        let framed = Metrics::of(&cfg, 1.0);
        assert!(framed.frame_strips > 0.0);
        assert_eq!(band_hit(&framed, SCREEN, (1.0, h - 1.0)), None, "the far strip");
        assert_eq!(band_hit(&framed, SCREEN, (1.0, exclusive_zone(&framed) + 4.0)), None, "a side strip");
        // Side bars, framed or not: the band is the bar's column along its
        // whole edge (measured down the bar); the frame strips aren't.
        for frame in [false, true] {
            for position in [BarPosition::Left, BarPosition::Right] {
                let m = side(position, frame);
                let zone = exclusive_zone(&m);
                let (inner, outer) = if position == BarPosition::Left { (zone - 0.5, zone + 1.0) } else { (w - zone, w - zone - 1.0) };
                assert_eq!(band_hit(&m, SCREEN, (inner, 700.0)), Some(700.0), "{position:?} frame {frame}");
                assert_eq!(band_hit(&m, SCREEN, (inner, 0.0)), Some(0.0), "{position:?}: the top end");
                assert_eq!(band_hit(&m, SCREEN, (inner, h - 0.5)), Some(h - 0.5), "{position:?}: the bottom end");
                assert_eq!(band_hit(&m, SCREEN, (outer, 700.0)), None, "{position:?}: past the bar");
                if frame {
                    let t = m.screen_frame;
                    assert_eq!(band_hit(&m, SCREEN, (w / 2.0, t / 2.0)), None, "{position:?}: the top frame strip");
                    assert_eq!(band_hit(&m, SCREEN, (w / 2.0, h - t / 2.0)), None, "{position:?}: the bottom frame strip");
                }
            }
        }
        // Off the output: never.
        assert_eq!(band_hit(&top, SCREEN, (-1.0, 5.0)), None);
        assert_eq!(band_hit(&top, SCREEN, (w, 5.0)), None);
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

    fn notch(position: BarPosition) -> Metrics {
        Metrics::default_for(BarLayout::Notch, position)
    }

    #[test]
    fn a_column_beside_a_full_edge_bar_flares_into_the_bar_and_the_far_edge() {
        let m = edge();
        let c = side_column(&m, (1600.0, 900.0), 0.0, 380.0, false, (30.0, 870.0), 12.0);
        assert_eq!(c.column, (0.0, 30.0, 380.0, 870.0));
        assert_eq!(c.flares.as_slice(), &[(380.0, 30.0, 12.0, FlareCenter::BottomRight), (380.0, 888.0, 12.0, FlareCenter::TopRight)]);
        assert_eq!(c.notch, None);
        // On the right edge the fillets sit left of its inner edge.
        let c = side_column(&m, (1600.0, 900.0), 1220.0, 380.0, true, (30.0, 870.0), 12.0);
        assert_eq!(c.flares.as_slice(), &[(1208.0, 30.0, 12.0, FlareCenter::BottomLeft), (1208.0, 888.0, 12.0, FlareCenter::TopLeft)]);
        // A hairline (no flares) meets the column square.
        assert!(side_column(&m, (1600.0, 900.0), 0.0, 380.0, false, (24.0, 876.0), 0.0).flares.is_empty());
    }

    #[test]
    fn a_column_joins_the_notch_like_a_puzzle_piece() {
        let (surface, m) = ((1600.0, 900.0), notch(BarPosition::Top));
        let frame = m.frame(surface.0);
        let (nx, nw) = (x_of(&frame), w_of(&frame));
        // Wider than the gap left of the notch: it rises to the top edge and
        // the notch starts where the column ends, square, with a fillet
        // tucked under the notch.
        let c = side_column(&m, surface, 0.0, nx + 100.0, false, (m.height, surface.1 - m.height), 12.0);
        assert_eq!(c.column, (0.0, 0.0, nx + 100.0, surface.1));
        assert_eq!(c.notch, Some((nx + 100.0, nw - 100.0, true, false)));
        assert_eq!(c.flares[0], (nx + 100.0, m.height, 12.0, FlareCenter::BottomRight));
        assert_eq!(c.flares[1], (nx + 100.0, surface.1 - 12.0, 12.0, FlareCenter::TopRight));
        // Still sliding in, short of the notch: two separate pieces hanging
        // from the edge, each with its own shoulder.
        let c = side_column(&m, surface, -300.0, 380.0, false, (m.height, surface.1 - m.height), 12.0);
        assert!(80.0 < nx - 12.0, "the test column must stop short of the notch");
        assert_eq!(c.notch, None);
        assert_eq!(c.flares[0], (80.0, 0.0, 12.0, FlareCenter::BottomRight));
        // A right sidebar ends the notch at its inner edge.
        let c = side_column(&m, surface, nx + nw - 50.0, 450.0, true, (m.height, surface.1 - m.height), 12.0);
        assert_eq!(c.notch, Some((nx, nw - 50.0, false, true)));
        // Under a bottom notch the column rises from the bottom edge.
        let m = notch(BarPosition::Bottom);
        let c = side_column(&m, surface, 0.0, nx + 100.0, false, (0.0, surface.1 - m.height), 12.0);
        assert_eq!(c.column, (0.0, 0.0, nx + 100.0, surface.1));
        assert_eq!(c.flares[0], (nx + 100.0, surface.1 - m.height - 12.0, 12.0, FlareCenter::TopRight));
        assert_eq!(c.flares[1], (nx + 100.0, 0.0, 12.0, FlareCenter::BottomRight));
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

    fn styled(style: mandara_config::bar_styles::BarStyle) -> Metrics {
        let mut cfg = ShellConfig::default();
        style.apply(&mut cfg);
        Metrics::of(&cfg, 1.0)
    }

    #[test]
    fn round_icon_items_are_squared_and_their_content_centred() {
        use mandara_config::bar_styles::BarStyle;
        let m = styled(BarStyle::Expressive);
        let across = m.height - m.chip_inset() * 2.0;
        assert_eq!(m.min_chip, across, "icon items are as long as they are thick");
        // A small icon becomes a square, its content in the middle.
        let icon = across - m.chip_padding * 2.0 - 4.0;
        assert_eq!(m.chip_length(icon), across);
        assert!((m.content_inset(icon) - (across - icon) / 2.0).abs() < 1e-4);
        // Wide items keep their padding.
        assert_eq!(m.chip_length(120.0), 120.0 + m.chip_padding * 2.0);
        assert_eq!(m.content_inset(120.0), m.chip_padding);
        // Zones and fitting count the squares.
        assert_eq!(zone_width(&[icon, icon], &m), across * 2.0 + m.chip_gap);
        let fit = fit_slots(&[icon, icon, icon], &m, across * 2.0 + m.chip_gap + 1.0, false);
        assert_eq!(fit.iter().filter(|f| f.is_some()).count(), 2, "a square that doesn't fit stays out");
        let kept: smallvec::SmallVec<[f32; 4]> = fit.iter().map(|f| f.map_or(0.0, |l| icon.min(l))).collect();
        assert!(zone_width(&kept, &m) <= across * 2.0 + m.chip_gap + 1.0);
        // The shipped bar squares nothing and keeps its 3 px inset.
        let plain = edge();
        assert_eq!((plain.min_chip, plain.chip_inset()), (0.0, mandara_config::surfaces::defaults::BAR_ITEM_INSET_PX));
        assert_eq!(plain.chip_length(16.0), 16.0 + plain.chip_padding * 2.0);
        // The tile inset follows the setting.
        let mut cfg = ShellConfig::default();
        cfg.bar.item_inset = 9.0;
        assert_eq!(Metrics::of(&cfg, 1.0).chip_inset(), 9.0);
        cfg.bar.layout = BarLayout::Hairline;
        assert_eq!(Metrics::of(&cfg, 1.0).chip_inset(), COMPACT_CHIP_INSET, "the compact styles keep theirs");
    }

    #[test]
    fn the_launcher_sits_in_the_corner_where_the_bar_meets_the_rail() {
        use mandara_config::bar_styles::BarStyle;
        const SURFACE_W: f32 = 1707.0;
        /// The launcher's content: one icon at the bar's icon scale.
        const ICON_PX: f32 = 16.0;
        /// The launcher's own mark (its art, larger than an icon).
        const LOGO_PX: f32 = 24.0;
        for style in [BarStyle::Expressive, BarStyle::RamlTram] {
            for position in [BarPosition::Top, BarPosition::Bottom] {
                // The style's own sizes, a taller bar, a wide rail and one
                // just wider than the launcher's button.
                let h = style.height();
                let tight = styled(style).min_chip + 2.0;
                for (height, rail) in [(h, None), (h + 4.0, None), (h + 12.0, Some(64.0)), (h, Some(tight))] {
                    for scale in [1.0, 1.25, 1.5, 2.0] {
                        let mut cfg = ShellConfig::default();
                        style.apply(&mut cfg);
                        cfg.bar.position = position;
                        cfg.bar.height = height;
                        if let Some(w) = rail {
                            cfg.side_bars.width = w;
                        }
                        let what = format!("{style:?} {position:?} height {height} rail {rail:?} at {scale}");
                        let mut m = Metrics::of(&cfg, 1.0);
                        assert_eq!(m.corner, cfg.side_bars.width(), "{what}: the corner is the rail's width");
                        // A plain icon and the launcher's larger mark.
                        for content in [ICON_PX * cfg.bar.icon_scale(), LOGO_PX] {
                            m.corner_item = m.corner_length(content);
                            assert!(m.corner_holds_item(), "{what}");
                            let zone = [content, 40.0, 30.0];
                            let widths = [start_zone_width(&zone, &m), 80.0, 120.0];
                            let geo = compute(&m, SURFACE_W, widths);
                            // The launcher's chip as the shell places it, the
                            // shape drawn for it, snapped to the output's pixels
                            // like everything it draws.
                            let x = x_of(&geo.pills[0]);
                            let (y, h) = (m.chip_inset(), m.height - m.chip_inset() * 2.0);
                            let look = crate::bar::items::ItemLook { item: cfg.bar.item_shape, icon: cfg.bar.icon_shape, radius: m.radius, vertical: false };
                            let drawn = crate::bar::items::tile(&look, rect(x, y, m.corner_item, h), 0, 1).bounds;
                            assert!(crate::bar::items::tile(&look, drawn, 0, 1).icon, "{what}: the launcher is a round button");
                            let (x0, y0, w0, h0) = mandara_core::snap_rect(x_of(&drawn), y_of(&drawn), w_of(&drawn), h_of(&drawn), scale);
                            let rail_w = mandara_core::snap(m.corner, scale);
                            let one_px = 1.0 / scale + 1e-3;
                            assert!(((x0 + w0 / 2.0) - rail_w / 2.0).abs() <= one_px, "{what}: centred on the rail's width ({x0} + {w0} in {rail_w})");
                            assert!(((y0 + h0 / 2.0) - m.height / 2.0).abs() <= one_px, "{what}: centred on the bar's height");
                            assert!(x0 >= -one_px && x0 + w0 <= rail_w + one_px, "{what}: inside the corner, not nudged into the bar");
                            // The next item starts clear of the rail.
                            let next = x + m.corner_item + m.chip_gap + m.corner_gap();
                            assert!(next >= m.corner + m.chip_inset() - 1e-3, "{what}: the next item at {next}");
                            // The zone's width counts the corner's item at its
                            // length and that room, so it still fits.
                            assert!((widths[0] - (zone_width(&zone, &m) - (m.chip_length(content) - m.corner_item) + m.corner_gap())).abs() < 1e-3);
                        }
                    }
                }
            }
        }
        // An item too wide for the corner: the zone starts clear of the rail.
        let mut cfg = ShellConfig::default();
        BarStyle::RamlTram.apply(&mut cfg);
        let mut m = Metrics::of(&cfg, 1.0);
        m.corner_item = 200.0;
        assert!(!m.corner_holds_item() && m.corner_gap() == 0.0);
        assert_eq!(m.start_inset(SURFACE_W), m.corner + m.chip_inset());
        // No joined rail, or one that slides away: no corner.
        assert_eq!(edge().corner, 0.0);
        cfg.side_bars.left.pinned = false;
        assert_eq!(Metrics::of(&cfg, 1.0).corner, 0.0);
        cfg.side_bars.left.pinned = true;
        cfg.side_bars.joined = false;
        assert_eq!(Metrics::of(&cfg, 1.0).corner, 0.0);
        let mut rail = ShellConfig::default();
        BarStyle::Rail.apply(&mut rail);
        assert_eq!(Metrics::of(&rail, 1.0).corner, 0.0, "a side bar has no corner");
        assert_eq!(crate::bar::side::metrics(&cfg, false, 1.0).corner, 0.0);
    }

    #[test]
    fn a_framed_top_or_bottom_bar_borders_the_other_three_edges() {
        use mandara_config::bar_styles::BarStyle;
        let surface = (1920.0, 1080.0);
        let m = styled(BarStyle::Expressive);
        assert_eq!(m.position, BarPosition::Bottom);
        let t = m.frame_strips;
        assert!(t > 0.0 && m.screen_frame == 0.0, "a horizontal bar frames with strips");
        let bar_edge = surface.1 - m.height;
        for covered in [[false, false], [true, false], [false, true], [true, true]] {
            let f = edge_frame(&m, surface, bar_edge, m.flare, covered).expect("framed");
            assert_eq!(f.inset, t);
            assert_eq!(f.strips[0], Some((0.0, 0.0, surface.0, t)), "the far (top) edge");
            for (i, side) in [f.strips[1], f.strips[2]].into_iter().enumerate() {
                assert_eq!(side.is_none(), covered[i], "a covered side keeps no strip");
                assert_eq!(f.flares[i].is_none(), covered[i], "nor its fillet");
                if let Some((x, y, w, h)) = side {
                    assert_eq!((y, y + h, w), (t, bar_edge, t), "between the far strip and the bar, no overlap");
                    assert!(x == 0.0 || x + w == surface.0);
                }
                if let Some((x, y, r, _)) = f.flares[i] {
                    assert!(x >= t && x + r <= surface.0 - t && y == t, "inside the opening, against the far strip");
                }
            }
        }
        // A top bar's frame is the same, mirrored.
        let mut top = m;
        top.position = BarPosition::Top;
        let f = edge_frame(&top, surface, top.height, top.flare, [false, false]).unwrap();
        assert_eq!(f.strips[0], Some((0.0, surface.1 - t, surface.0, t)));
        assert_eq!(f.strips[1], Some((0.0, top.height, t, surface.1 - t - top.height)));
        assert_eq!(f.flares[0].map(|f| f.1 + f.2), Some(surface.1 - t));
        // No frame: off, or a style without full-width strips.
        assert!(edge_frame(&edge(), surface, 30.0, 18.0, [false, false]).is_none());
        let mut cfg = ShellConfig::default();
        cfg.bar.screen_frame = true;
        cfg.bar.layout = BarLayout::FloatingPills;
        assert_eq!(Metrics::of(&cfg, 1.0).frame_strips, 0.0);
        cfg.bar.layout = BarLayout::MinimalEdge;
        cfg.bar.floating_frame = true;
        let framed = Metrics::of(&cfg, 1.0);
        assert!(framed.frame_strips > 0.0 && !framed.floating_frame, "the frame wins over floating, as beside a side bar");
    }
}
