//! Pure geometry of the side bars (`[side_bars]`): a left and a right bar
//! of plugin chips beside the main bar, laid out by the main bar's own
//! engine ([`super::layout`]) in *rail space* (along the bar = x from the
//! bar's start, away from the screen edge = y) and placed on the screen.
//! Joined to a top or bottom bar they hang from it as one frame with
//! concave fillets; separate ones float as pills, an island or a slab.

use gpui::{Bounds, Pixels, Size, point, px, size};
use mandara_config::{BarPosition, ShellConfig};
use mandara_core::FlareCenter;

use super::layout::{self, Metrics, PILL_INSET};

/// Extra travel past the edge while a side bar hides, so its shadow and
/// rim leave the screen too, px.
const HIDE_OVERSHOOT_PX: f32 = 2.0;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

fn edge(right: bool) -> BarPosition {
    if right { BarPosition::Right } else { BarPosition::Left }
}

/// The left (`right == false`) or right side bar's resolved geometry
/// settings, radii scaled by `radius_scale`. Panels keep `[panels]`.
pub fn metrics(cfg: &ShellConfig, right: bool, radius_scale: f32) -> Metrics {
    let sb = &cfg.side_bars;
    let joined = cfg.side_bars_join();
    let layout = sb.layout(joined);
    let base = Metrics::of(cfg, radius_scale);
    Metrics {
        layout,
        position: edge(right),
        height: sb.width(),
        // Joined, the bar is flush with its edge.
        edge_gap: if joined { 0.0 } else { sb.edge_gap() },
        side_gap: sb.edge_gap(),
        radius: sb.corner_radius() * radius_scale,
        flare: sb.flare_radius() * radius_scale,
        chip_padding: sb.chip_padding(),
        chip_gap: sb.chip_spacing(),
        center: sb.align,
        floating_frame: !joined && layout == mandara_config::BarLayout::MinimalEdge,
        panel_pad: cfg.panels.padding_for(layout.is_attached()),
        vertical: true,
        screen_frame: 0.0,
        accent_line: false,
        ..base
    }
}

/// The stretch of the screen edge a side bar lays its items out on, as
/// `(y, length)`: beside the main bar's band (below a top bar, above a
/// bottom one) or the whole height beside a side bar. `main` is the main
/// bar's metrics, `surface_h` the surface height.
pub fn span(main: &Metrics, surface_h: f32) -> (f32, f32) {
    let zone = layout::exclusive_zone(main);
    match main.position {
        BarPosition::Top => (zone, (surface_h - zone).max(0.0)),
        BarPosition::Bottom => (0.0, (surface_h - zone).max(0.0)),
        BarPosition::Left | BarPosition::Right => (0.0, surface_h),
    }
}

/// A rail-space rect on the screen, for the side bar whose span starts at
/// `y0`.
pub fn to_screen(right: bool, b: Bounds<Pixels>, surface: (f32, f32), y0: f32) -> Bounds<Pixels> {
    let s = layout::orient(edge(right), b, surface);
    Bounds { origin: point(s.origin.x, s.origin.y + px(y0)), size: s.size }
}

/// The inverse of [`to_screen`].
pub fn from_screen(right: bool, b: Bounds<Pixels>, surface: (f32, f32), y0: f32) -> Bounds<Pixels> {
    let s = Bounds { origin: point(b.origin.x, b.origin.y - px(y0)), size: b.size };
    layout::unorient(edge(right), s, surface)
}

/// The space the left / right side bar reserves on `output` (its exclusive
/// zone, plus the bar's gap to windows): its depth while it is pinned and
/// shows there, none while it slides away or is off.
pub fn reserved(cfg: &ShellConfig, right: bool, output: &str, radius_scale: f32) -> f32 {
    if cfg.side_bar_on(right) && cfg.side_bars.side(right).pinned && cfg.bar_on_output(output) {
        layout::exclusive_zone(&metrics(cfg, right, radius_scale)) + cfg.bar.window_gap()
    } else {
        0.0
    }
}

/// How far a side bar travels to hide: its whole reserved depth (and a
/// little more for a floating one's shadow).
pub fn travel(m: &Metrics) -> f32 {
    let far = layout::exclusive_zone(m);
    if m.attached() && !m.floating_frame { far } else { far + HIDE_OVERSHOOT_PX }
}

/// Horizontal shift of a side bar that is `shown` (0 hidden … 1 out).
pub fn shift(m: &Metrics, right: bool, shown: f32) -> f32 {
    let d = travel(m) * (1.0 - shown.clamp(0.0, 1.0));
    if right { d } else { -d }
}

/// The chips of one group, in rail space: `group` is the group's laid-out
/// rect, `extents` each item's content length along the bar (0 = hidden),
/// `chip` the items' extent across the bar. Items are centred across the
/// bar's depth and run along it with the bar's padding and spacing.
pub fn chip_rects(m: &Metrics, group: Bounds<Pixels>, extents: &[f32], chip: f32) -> Vec<Option<Bounds<Pixels>>> {
    let inset = if m.attached() { 0.0 } else { PILL_INSET };
    let depth_start = layout::exclusive_zone(m) - m.height;
    let y = depth_start + (m.height - chip) / 2.0;
    let mut x = f32::from(group.origin.x) + inset;
    extents
        .iter()
        .map(|&w| {
            if w < 1.0 {
                return None;
            }
            let along = w + m.chip_padding * 2.0;
            let r = rect(x, y, along, chip);
            x += along + m.chip_gap;
            Some(r)
        })
        .collect()
}

/// Collapsed and expanded rects (screen) of a panel opened from a side
/// bar's chip at `chip` (screen): it pours out sideways from the bar (or
/// floats beside a floating one), centred on the chip along the bar's span
/// `(y0, len)`, never over the main bar's band.
pub fn panel_rects(m: &Metrics, right: bool, surface: (f32, f32), (y0, len): (f32, f32), chip: Bounds<Pixels>, target: Size<Pixels>, animation: mandara_config::surfaces::PanelAnimation) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let chip = from_screen(right, chip, surface, y0);
    let free = surface.0 - layout::exclusive_zone(m);
    let expanded = layout::dashboard_rect(m, chip, target, len, free);
    let collapsed = layout::start_rect(m, animation, chip, expanded);
    (to_screen(right, collapsed, surface, y0), to_screen(right, expanded, surface, y0))
}

/// A joined side bar's column and its two fillets.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// The visible column, `(x, y, w, h)` in surface px.
    pub rect: (f32, f32, f32, f32),
    /// Fillets: the `r × r` square at `(x, y)` and its arc's centre.
    pub flares: Vec<(f32, f32, f32, FlareCenter)>,
}

/// The column of a side bar joined to the main bar, `width` wide and
/// `shown` out (it slides off its edge while hidden), spanning `y0..y1` of
/// the screen. A concave fillet of radius `flare` joins its inner edge to
/// the main bar (at `y0` when the bar is on top, else at `y1`); the one at
/// the screen edge grows with `shown`, so a hiding bar leaves no corner
/// behind.
pub fn joined_column(right: bool, surface_w: f32, width: f32, shown: f32, (y0, y1): (f32, f32), flare: f32, bar_on_top: bool) -> Column {
    let p = shown.clamp(0.0, 1.0);
    let w = width * p;
    let x = if right { surface_w - w } else { 0.0 };
    let inner = if right { x } else { w };
    let r = flare.max(0.0);
    let fx = if right { inner - r } else { inner };
    // Fillets below a horizontal surface (the top) and above one (the
    // bottom), hugging the column.
    let (top_c, bottom_c) = if right { (FlareCenter::BottomLeft, FlareCenter::TopLeft) } else { (FlareCenter::BottomRight, FlareCenter::TopRight) };
    let (r_top, r_bottom) = if bar_on_top { (r, r * p) } else { (r * p, r) };
    let mut flares = vec![(if right { inner - r_top } else { fx }, y0, r_top, top_c), (if right { inner - r_bottom } else { fx }, y1 - r_bottom, r_bottom, bottom_c)];
    flares.retain(|f| f.2 > 0.0);
    Column { rect: (x, y0, w, (y1 - y0).max(0.0)), flares }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::BarLayout;
    use mandara_config::side_bars::SideBarShape;

    const SURFACE: (f32, f32) = (1920.0, 1080.0);

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

    fn on() -> ShellConfig {
        let mut cfg = ShellConfig::default();
        cfg.side_bars.enabled = true;
        cfg
    }

    #[test]
    fn joined_side_bars_are_flush_edge_frames_and_separate_ones_float() {
        let mut cfg = on();
        let m = metrics(&cfg, false, 1.0);
        assert!(m.attached() && !m.floating_frame && m.vertical);
        assert_eq!((m.position, m.height), (BarPosition::Left, cfg.side_bars.width()));
        assert_eq!(layout::exclusive_zone(&m), cfg.side_bars.width(), "joined: it reserves its width");
        assert_eq!(metrics(&cfg, true, 1.0).position, BarPosition::Right);
        cfg.side_bars.joined = false;
        let pills = metrics(&cfg, false, 1.0);
        assert_eq!(pills.layout, BarLayout::FloatingPills);
        assert_eq!(layout::exclusive_zone(&pills), cfg.side_bars.edge_gap() + cfg.side_bars.width());
        cfg.side_bars.shape = SideBarShape::Slab;
        let slab = metrics(&cfg, false, 1.0);
        assert!(slab.attached() && slab.floating_frame, "a slab floats as a frame and panels pour out of it");
    }

    #[test]
    fn the_span_starts_below_a_top_bar_and_ends_above_a_bottom_one() {
        let cfg = on();
        let main = Metrics::of(&cfg, 1.0);
        let (y0, len) = span(&main, SURFACE.1);
        assert_eq!(y0, layout::exclusive_zone(&main));
        assert_eq!(y0 + len, SURFACE.1);
        let bottom = Metrics::default_for(BarLayout::MinimalEdge, BarPosition::Bottom);
        let (y0, len) = span(&bottom, SURFACE.1);
        assert_eq!((y0, y0 + len), (0.0, SURFACE.1 - layout::exclusive_zone(&bottom)));
        let side = Metrics::default_for(BarLayout::MinimalEdge, BarPosition::Right);
        assert_eq!(span(&side, SURFACE.1), (0.0, SURFACE.1));
    }

    #[test]
    fn rail_space_round_trips_on_both_edges() {
        let r = rect(100.0, 3.0, 60.0, 34.0);
        for right in [false, true] {
            let s = to_screen(right, r, SURFACE, 40.0);
            assert_eq!(from_screen(right, s, SURFACE, 40.0), r);
            assert_eq!(y_of(&s), 140.0, "along the bar is down the screen, from the span's start");
            assert_eq!(h_of(&s), 60.0);
        }
        assert_eq!(x_of(&to_screen(false, r, SURFACE, 0.0)), 3.0, "left: against the left edge");
        assert_eq!(x_of(&to_screen(true, r, SURFACE, 0.0)) + 34.0, SURFACE.0 - 3.0, "right: against the right edge");
    }

    /// The whole layout of one side bar on the screen: every chip of the
    /// three groups, top to bottom.
    fn chips(cfg: &ShellConfig, right: bool, extents: [&[f32]; 3]) -> Vec<Bounds<Pixels>> {
        let m = metrics(cfg, right, 1.0);
        let (y0, len) = span(&Metrics::of(cfg, 1.0), SURFACE.1);
        let widths: [f32; 3] = std::array::from_fn(|z| layout::zone_width(extents[z], &m));
        let geo = layout::compute(&m, len, widths);
        (0..3).flat_map(|z| chip_rects(&m, geo.pills[z], extents[z], cfg.side_bars.chip_size())).flatten().map(|r| to_screen(right, r, SURFACE, y0)).collect()
    }

    #[test]
    fn chips_stack_down_the_edge_inside_the_span_without_overlapping() {
        let extents: [&[f32]; 3] = [&[18.0, 40.0], &[18.0], &[18.0, 18.0]];
        for joined in [true, false] {
            for shape in [SideBarShape::Pills, SideBarShape::Island, SideBarShape::Slab] {
                for right in [false, true] {
                    let mut cfg = on();
                    cfg.side_bars.joined = joined;
                    cfg.side_bars.shape = shape;
                    let what = format!("joined {joined}, {shape:?}, right {right}");
                    let (y0, _) = span(&Metrics::of(&cfg, 1.0), SURFACE.1);
                    let list = chips(&cfg, right, extents);
                    assert_eq!(list.len(), 5, "{what}");
                    let depth = layout::exclusive_zone(&metrics(&cfg, right, 1.0));
                    for (a, b) in list.iter().zip(list.iter().skip(1)) {
                        assert!(y_of(a) + h_of(a) <= y_of(b) + 0.01, "{what}: {a:?} then {b:?}");
                    }
                    for c in &list {
                        assert!(y_of(c) >= y0 && y_of(c) + h_of(c) <= SURFACE.1, "{what}: {c:?} stays beside the top bar");
                        let (from_edge, to_edge) = if right { (SURFACE.0 - x_of(c) - w_of(c), SURFACE.0 - x_of(c)) } else { (x_of(c), x_of(c) + w_of(c)) };
                        assert!(from_edge >= 0.0 && to_edge <= depth + 0.01, "{what}: {c:?} sits on the bar");
                        assert_eq!(w_of(c), cfg.side_bars.chip_size());
                    }
                }
            }
        }
    }

    #[test]
    fn panels_pour_sideways_and_never_over_the_top_bar() {
        let cfg = on();
        let main = Metrics::of(&cfg, 1.0);
        let sp = span(&main, SURFACE.1);
        let target = size(px(360.0), px(420.0));
        for right in [false, true] {
            let m = metrics(&cfg, right, 1.0);
            // A chip right under the top bar.
            let chip = to_screen(right, rect(10.0, 3.0, 30.0, 34.0), SURFACE, sp.0);
            let (collapsed, expanded) = panel_rects(&m, right, SURFACE, sp, chip, target, mandara_config::surfaces::PanelAnimation::Morph);
            assert!(y_of(&expanded) >= sp.0, "right {right}: below the top bar");
            if right {
                assert_eq!(x_of(&expanded) + w_of(&expanded), SURFACE.0 - m.height, "hangs from the right bar");
            } else {
                assert_eq!(x_of(&expanded), m.height, "hangs from the left bar");
            }
            assert_eq!(w_of(&collapsed), 0.0, "it pours out of the bar");
            assert!(layout::morph_progress_on(&m, expanded, collapsed, expanded) > 0.99);
        }
    }

    #[test]
    fn a_hidden_side_bar_leaves_the_screen() {
        let cfg = on();
        let m = metrics(&cfg, false, 1.0);
        assert_eq!(shift(&m, false, 1.0), 0.0);
        assert_eq!(shift(&m, false, 0.0), -m.height);
        assert_eq!(shift(&m, true, 0.0), m.height);
    }

    #[test]
    fn the_joined_column_meets_the_top_bar_with_a_fillet() {
        let c = joined_column(false, 1920.0, 40.0, 1.0, (30.0, 1080.0), 18.0, true);
        assert_eq!(c.rect, (0.0, 30.0, 40.0, 1050.0));
        assert_eq!(c.flares, vec![(40.0, 30.0, 18.0, FlareCenter::BottomRight), (40.0, 1062.0, 18.0, FlareCenter::TopRight)]);
        let r = joined_column(true, 1920.0, 40.0, 1.0, (30.0, 1080.0), 18.0, true);
        assert_eq!(r.rect.0, 1880.0);
        assert_eq!(r.flares[0], (1862.0, 30.0, 18.0, FlareCenter::BottomLeft));
        // Hidden: only the fillet into the bar stays (it is the top bar's
        // own screen flare then).
        let hidden = joined_column(false, 1920.0, 40.0, 0.0, (30.0, 1080.0), 18.0, true);
        assert_eq!(hidden.rect.2, 0.0);
        assert_eq!(hidden.flares, vec![(0.0, 30.0, 18.0, FlareCenter::BottomRight)]);
        // Beside a bottom bar the fillet into the bar is at the bottom.
        let b = joined_column(false, 1920.0, 40.0, 0.0, (0.0, 1050.0), 18.0, false);
        assert_eq!(b.flares, vec![(0.0, 1032.0, 18.0, FlareCenter::TopRight)]);
    }
}
