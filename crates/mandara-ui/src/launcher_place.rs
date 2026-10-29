//! Pure geometry of the launcher's placements (`[launcher.look] position`):
//! where the launcher rests, the shape that joins it to the bar or the
//! screen edge, and where its open animation starts. Kept free of GPUI
//! state (like [`crate::bar::layout`]) so every placement is unit-tested.
//!
//! * **Attached at the bar's edge** — the edge frame pours the launcher out
//!   of the bar with concave flares, exactly like a drop-down panel;
//!   floating pills / the island get a floating card the panels' gap away.
//!   Left / right sit against the screen sides (clear of the frame's screen
//!   flares, or in line with the outer pills); in the zone holding the
//!   bar's launcher button it can grow out of the button instead.
//! * **Attached at the other edge** (no bar there) — a drawer from the bare
//!   screen edge: with flares for the edge-frame style (the shipped
//!   Caelestia-style bottom drawer), flush with rounded inner corners for
//!   the floating styles.
//! * **Bottom with the dock on** — a card floating above the dock (never
//!   covered by it).
//! * **Center** — a floating card in the middle (scale + fade).
//!
//! Left and right are physical: a right-to-left bar puts its launcher
//! button on the right, so only the right-hand placements align to it.

use mandara_config::surfaces::PanelAnimation;
use mandara_config::workflow::{LauncherAlign, LauncherEdge, LauncherPosition};
use mandara_config::BarPosition;
use smallvec::SmallVec;

use crate::bar::layout::{CHIP_INSET, Metrics, PANEL_SLIDE_PX, PILL_INSET, exclusive_zone};

/// Gap kept to screen sides (and above / below a floating card), px.
pub const SCREEN_MARGIN: f32 = 24.0;
/// Extra room keeping a drawer's own flare clear of the frame's screen
/// flares (as panels do).
const FLARE_CLEARANCE: f32 = 2.0;
/// The center card starts its scale + fade at this share of its size.
pub const CENTER_START_SCALE: f32 = 0.92;
/// Screen thirds: a launcher button in a placement's third is "in its zone".
const ZONE_THIRD: f32 = 1.0 / 3.0;

/// A rectangle in surface (= output) coordinates, logical px.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Linear blend toward `to` (`t` may overshoot a little: springs).
    pub fn lerp(self, to: Rect, t: f32) -> Rect {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Rect::new(l(self.x, to.x), l(self.y, to.y), l(self.w, to.w).max(0.0), l(self.h, to.h).max(0.0))
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Top-oriented → the given edge (mirrored for the bottom).
    fn orient(self, edge: LauncherEdge, screen_h: f32) -> Rect {
        match edge {
            LauncherEdge::Top => self,
            LauncherEdge::Bottom => Rect::new(self.x, screen_h - self.y - self.h, self.w, self.h),
        }
    }

    pub fn to_bounds(self) -> gpui::Bounds<gpui::Pixels> {
        gpui::Bounds { origin: gpui::point(gpui::px(self.x), gpui::px(self.y)), size: gpui::size(gpui::px(self.w), gpui::px(self.h)) }
    }

    pub fn from_bounds(b: gpui::Bounds<gpui::Pixels>) -> Self {
        Rect::new(f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height))
    }
}

/// How the launcher's shape meets its surroundings.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Join {
    /// Pours out of the bar / screen edge with concave flares.
    Flares,
    /// Flush with the screen edge, rounded on its open side.
    Flush,
    /// A floating card (elevation, rim, all corners rounded).
    Card,
}

/// Everything a placement depends on.
#[derive(Copy, Clone, Debug)]
pub struct Inputs {
    pub position: LauncherPosition,
    /// The output's logical size.
    pub screen: (f32, f32),
    /// Wanted width and the content's full height right now.
    pub size: (f32, f32),
    /// Tallest the launcher may grow (the center card centers on it, so
    /// its top stays put while results change).
    pub max_h: f32,
    /// The bar's geometry, and whether the bar shows on this output.
    pub bar: Metrics,
    pub bar_here: bool,
    /// Room a bottom dock keeps free (band + gap), when it's on.
    pub dock_clearance: Option<f32>,
    /// The bar's launcher button on this output `(x, width)`, when the
    /// launcher should align to it.
    pub button: Option<(f32, f32)>,
    pub animation: PanelAnimation,
}

/// Where the launcher rests and how it animates.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Placement {
    /// Fully open.
    pub rect: Rect,
    /// Where the open animation starts (and the close returns to).
    pub start: Rect,
    pub join: Join,
    /// The edge it is attached to (`None`: the center card).
    pub edge: Option<LauncherEdge>,
    /// Concave flare radius (0 unless [`Join::Flares`]).
    pub flare: f32,
    /// Corner radius to start from (the launcher pill's, when it morphs
    /// out of a floating pill; `None`: the panel radius throughout).
    pub start_radius: Option<f32>,
    /// The whole launcher fades with its progress (center, slide).
    pub fades: bool,
    /// The bar's band `(y, h)` the dim leaves alone (joined to the bar).
    pub keep_bright: Option<(f32, f32)>,
    /// Leaving it with the pointer closes it (attached placements).
    pub hover_close: bool,
    /// It grows out of the bar's launcher button.
    pub from_button: bool,
}

/// The launcher edge a bar shares (`None`: a side bar shares none — the
/// launcher keeps its edge and opens beside the bar).
fn bar_edge(p: BarPosition) -> Option<LauncherEdge> {
    match p {
        BarPosition::Top => Some(LauncherEdge::Top),
        BarPosition::Bottom => Some(LauncherEdge::Bottom),
        BarPosition::Left | BarPosition::Right => None,
    }
}

/// Room a side bar (and its screen frame) takes from the left and right
/// of the screen: `(left, right)`.
fn side_clearance(i: &Inputs) -> (f32, f32) {
    let m = &i.bar;
    if !i.bar_here || !m.vertical {
        return (0.0, 0.0);
    }
    let (zone, frame) = (exclusive_zone(m), m.screen_frame);
    if m.position == BarPosition::Left { (zone, frame) } else { (frame, zone) }
}

/// Whether a button centered at `mid` lies in `align`'s third of the screen.
pub fn in_zone(align: LauncherAlign, mid: f32, screen_w: f32) -> bool {
    match align {
        LauncherAlign::Left => mid < screen_w * ZONE_THIRD,
        LauncherAlign::Right => mid > screen_w * (1.0 - ZONE_THIRD),
        LauncherAlign::Center => (screen_w * ZONE_THIRD..=screen_w * (1.0 - ZONE_THIRD)).contains(&mid),
    }
}

/// Solves a placement.
pub fn place(i: &Inputs) -> Placement {
    match i.position.edge() {
        None => center(i),
        Some(edge) => attached(i, edge),
    }
}

fn center(i: &Inputs) -> Placement {
    let (sw, sh) = i.screen;
    let m = &i.bar;
    let band = if i.bar_here { exclusive_zone(m) } else { 0.0 };
    let top_free = if i.bar_here && m.position == BarPosition::Top { band } else { 0.0 } + SCREEN_MARGIN;
    let bottom_free = if i.bar_here && m.position == BarPosition::Bottom { band } else { 0.0 } + i.dock_clearance.unwrap_or(0.0) + SCREEN_MARGIN;
    let (side_l, side_r) = side_clearance(i);
    let w = i.size.0.min(sw - SCREEN_MARGIN * 2.0 - side_l - side_r).max(0.0);
    let h = i.size.1.min((sh - top_free - bottom_free).max(0.0));
    let max_h = i.max_h.max(h).min((sh - top_free - bottom_free).max(h));
    // Centered on its tallest size, so the top stays put as results change;
    // never under the bar or the dock.
    let y = ((sh - max_h) / 2.0).max(top_free).min((sh - bottom_free - h).max(top_free));
    let rect = Rect::new(side_l + (sw - side_l - side_r - w) / 2.0, y, w, h);
    let start = match i.animation {
        PanelAnimation::None => rect,
        _ => {
            let (sw0, sh0) = (w * CENTER_START_SCALE, h * CENTER_START_SCALE);
            Rect::new(rect.x + (w - sw0) / 2.0, rect.y + (h - sh0) / 2.0, sw0, sh0)
        }
    };
    Placement { rect, start, join: Join::Card, edge: None, flare: 0.0, start_radius: None, fades: true, keep_bright: None, hover_close: false, from_button: false }
}

fn attached(i: &Inputs, edge: LauncherEdge) -> Placement {
    let (sw, sh) = i.screen;
    let m = &i.bar;
    let at_bar = i.bar_here && bar_edge(m.position) == Some(edge);
    let (side_l, side_r) = side_clearance(i);
    let dock = if edge == LauncherEdge::Bottom { i.dock_clearance } else { None };
    let band = if at_bar { exclusive_zone(m) } else { 0.0 };
    // (shape, distance from the screen edge to the launcher's near side,
    // side margin)
    let (join, line, margin) = if let Some(dock) = dock {
        // Floats above the dock (and a bottom bar), never covered by it.
        (Join::Card, band + dock, if at_bar && !m.attached() { m.side_gap } else { SCREEN_MARGIN })
    } else if at_bar && m.attached() {
        let frame_inset = if m.floating_frame { m.side_gap + m.frame_radius() } else { 0.0 };
        (Join::Flares, m.bar_far_edge(), frame_inset + m.flare * 2.0 + FLARE_CLEARANCE)
    } else if at_bar {
        (Join::Card, m.bar_far_edge() + m.panel_gap, m.side_gap)
    } else if m.attached() {
        // The bare screen edge in the edge-frame style: the shipped drawer.
        (Join::Flares, 0.0, m.flare + SCREEN_MARGIN)
    } else {
        (Join::Flush, 0.0, SCREEN_MARGIN)
    };
    let w = i.size.0.min(sw - margin * 2.0 - side_l - side_r).max(0.0);
    // Room up to the far side (and a bar there).
    let far = if i.bar_here && !at_bar && !m.vertical { exclusive_zone(m) } else { 0.0 } + SCREEN_MARGIN;
    // A screen frame runs along this edge too.
    let line = line + if i.bar_here && m.vertical { m.screen_frame } else { 0.0 };
    let h = i.size.1.min((sh - line - far).max(0.0));
    let align = i.position.align();
    let lo = margin + side_l;
    let hi = (sw - margin - side_r - w).max(lo);
    // The bar's launcher button, when it sits in this placement's zone of
    // the bar's own edge.
    let button = i.button.filter(|(bx, bw)| at_bar && dock.is_none() && in_zone(align, bx + bw / 2.0, sw));
    let x = match (button, align) {
        (Some((bx, bw)), _) => (bx + bw / 2.0 - w / 2.0).clamp(lo, hi),
        (None, LauncherAlign::Left) => lo,
        (None, LauncherAlign::Right) => hi,
        (None, LauncherAlign::Center) => ((sw - w) / 2.0).clamp(lo, hi),
    };
    // Solved top-oriented, then mirrored onto the bottom edge.
    let rect = Rect::new(x, line, w, h);
    let mut start_radius = None;
    let start = match i.animation {
        PanelAnimation::None => rect,
        PanelAnimation::Slide => Rect::new(x, line - PANEL_SLIDE_PX, w, h),
        PanelAnimation::Morph => match button {
            // Out of the floating pill itself (as panels morph).
            Some((bx, bw)) if join == Join::Card => {
                let chip_h = m.height - PILL_INSET * 2.0;
                start_radius = Some(m.radius.min(chip_h / 2.0));
                Rect::new(bx, m.edge_gap + PILL_INSET, bw, chip_h)
            }
            // A sliver under the button: it pours out of the frame there.
            Some((bx, bw)) => Rect::new(bx, line, bw, 0.0),
            None => Rect::new(x, line, w, 0.0),
        },
    };
    let keep_bright = match m.position {
        BarPosition::Top if at_bar => Some((0.0, band)),
        BarPosition::Bottom if at_bar => Some((sh - band, band)),
        _ => None,
    };
    Placement {
        rect: rect.orient(edge, sh),
        start: start.orient(edge, sh),
        join,
        edge: Some(edge),
        flare: if join == Join::Flares { m.flare } else { 0.0 },
        start_radius,
        fades: i.animation == PanelAnimation::Slide,
        keep_bright,
        hover_close: true,
        from_button: button.is_some(),
    }
}

/// The placement `cfg` asks for on a `screen`-sized output, for content
/// `content_h` tall (`bar_here`: the bar shows on this output; `button`:
/// its launcher button there).
pub fn for_config(cfg: &mandara_config::ShellConfig, screen: (f32, f32), content_h: f32, bar_here: bool, button: Option<(f32, f32)>) -> Placement {
    let look = &cfg.launcher.look;
    let (w, max_h) = look.size();
    place(&Inputs {
        position: look.position,
        screen,
        size: (w, content_h.min(max_h)),
        max_h,
        bar: Metrics::of(cfg, crate::style::radius_scale()),
        bar_here,
        dock_clearance: cfg.dock_bottom_clearance(),
        button: button.filter(|_| look.align_to_button),
        animation: cfg.panels.animation,
    })
}

/// Top-oriented y of the bar's chips (for a chip rect from its x / width).
pub fn chip_y(m: &Metrics) -> (f32, f32) {
    if m.attached() { (f32::from(m.frame(0.0).origin.y) + CHIP_INSET, m.height - CHIP_INSET * 2.0) } else { (m.edge_gap + PILL_INSET, m.height - PILL_INSET * 2.0) }
}

// ── The bar's launcher button, per output ───────────────────────────────

/// Where the bar's launcher button rests on each output: `(output, x,
/// width)` in output coordinates. The bar publishes it only while the
/// launcher is open (nothing is kept or computed otherwise).
#[derive(Default)]
pub struct LauncherButtons(SmallVec<[(String, f32, f32); 2]>);
impl gpui::Global for LauncherButtons {}

/// Records `output`'s launcher button (`None`: not in its bar).
pub fn publish_button(output: &str, chip: Option<(f32, f32)>, cx: &mut gpui::App) {
    let current = cx.try_global::<LauncherButtons>().and_then(|b| b.0.iter().find(|(o, _, _)| o == output).map(|(_, x, w)| (*x, *w)));
    if current == chip && (chip.is_some() || !cx.has_global::<LauncherButtons>()) {
        return;
    }
    let buttons = cx.default_global::<LauncherButtons>();
    buttons.0.retain(|(o, _, _)| o != output);
    if let Some((x, w)) = chip {
        buttons.0.push((output.to_string(), x, w));
    }
}

/// The launcher button on `output` (or the only one known, when the
/// output's name isn't).
pub fn button_on(output: Option<&str>, cx: &gpui::App) -> Option<(f32, f32)> {
    let b = &cx.try_global::<LauncherButtons>()?.0;
    match output {
        Some(name) => b.iter().find(|(o, _, _)| o == name),
        None if b.len() == 1 => b.first(),
        None => None,
    }
    .map(|(_, x, w)| (*x, *w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::BarLayout;

    const SCREEN: (f32, f32) = (2560.0, 1440.0);
    const SIZE: (f32, f32) = (600.0, 480.0);

    fn inputs(position: LauncherPosition, layout: BarLayout, bar: BarPosition) -> Inputs {
        Inputs {
            position,
            screen: SCREEN,
            size: SIZE,
            max_h: 560.0,
            bar: Metrics::default_for(layout, bar),
            bar_here: true,
            dock_clearance: None,
            button: None,
            animation: PanelAnimation::Morph,
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn edge_frame_placements_pour_out_of_the_bar_like_panels() {
        for bar in [BarPosition::Top, BarPosition::Bottom] {
            let m = Metrics::default_for(BarLayout::MinimalEdge, bar);
            let band = exclusive_zone(&m);
            for pos in [LauncherPosition::TopLeft, LauncherPosition::TopCenter, LauncherPosition::TopRight, LauncherPosition::BottomLeft, LauncherPosition::BottomCenter, LauncherPosition::BottomRight] {
                let p = place(&inputs(pos, BarLayout::MinimalEdge, bar));
                let on_bar = (pos.edge() == Some(LauncherEdge::Top)) == (bar == BarPosition::Top);
                assert_eq!(p.join, Join::Flares, "{pos:?} / {bar:?}");
                assert_eq!(p.flare, m.flare);
                // Joined to the bar's far edge, or to the bare screen edge.
                let line = if on_bar { band } else { 0.0 };
                match pos.edge().unwrap() {
                    LauncherEdge::Top => assert!(near(p.rect.y, line), "{pos:?} {bar:?} {p:?}"),
                    LauncherEdge::Bottom => assert!(near(p.rect.bottom(), SCREEN.1 - line), "{pos:?} {bar:?} {p:?}"),
                }
                assert_eq!(p.keep_bright.is_some(), on_bar);
                // Grows from a sliver on that line.
                assert_eq!(p.start.h, 0.0);
                let start_line = if pos.edge() == Some(LauncherEdge::Top) { p.start.y } else { p.start.bottom() };
                let rest_line = if pos.edge() == Some(LauncherEdge::Top) { p.rect.y } else { p.rect.bottom() };
                assert!(near(start_line, rest_line));
                // Its own flares stay clear of the screen sides (and of the
                // frame's screen flares on the bar's edge).
                assert!(p.rect.x - p.flare >= if on_bar { m.flare } else { 0.0 }, "{pos:?} {p:?}");
                assert!(p.rect.right() + p.flare <= SCREEN.0 - if on_bar { m.flare } else { 0.0 });
            }
        }
    }

    #[test]
    fn left_center_right_line_up_with_the_screen() {
        let l = place(&inputs(LauncherPosition::TopLeft, BarLayout::FloatingPills, BarPosition::Top));
        let c = place(&inputs(LauncherPosition::TopCenter, BarLayout::FloatingPills, BarPosition::Top));
        let r = place(&inputs(LauncherPosition::TopRight, BarLayout::FloatingPills, BarPosition::Top));
        let m = Metrics::default_for(BarLayout::FloatingPills, BarPosition::Top);
        // Floating pills: a card in line with the outer pills, the panels'
        // gap under the bar.
        assert_eq!(l.join, Join::Card);
        assert!(near(l.rect.x, m.side_gap));
        assert!(near(r.rect.right(), SCREEN.0 - m.side_gap));
        assert!(near(c.rect.x + c.rect.w / 2.0, SCREEN.0 / 2.0));
        assert!(near(l.rect.y, m.edge_gap + m.height + m.panel_gap));
        assert_eq!((l.rect.w, l.rect.h), SIZE);
    }

    #[test]
    fn floating_styles_on_the_other_edge_sit_flush_without_flares() {
        for layout in [BarLayout::FloatingPills, BarLayout::ContiguousIsland] {
            let p = place(&inputs(LauncherPosition::BottomRight, layout, BarPosition::Top));
            assert_eq!((p.join, p.flare), (Join::Flush, 0.0));
            assert!(near(p.rect.bottom(), SCREEN.1));
            assert!(near(p.rect.right(), SCREEN.0 - SCREEN_MARGIN));
            assert_eq!(p.keep_bright, None, "not joined to the bar");
        }
    }

    #[test]
    fn a_bottom_dock_keeps_the_launcher_above_it() {
        let dock = 76.0;
        for bar in [BarPosition::Top, BarPosition::Bottom] {
            for layout in BarLayout::ALL {
                let band = if bar == BarPosition::Bottom { exclusive_zone(&Metrics::default_for(layout, bar)) } else { 0.0 };
                for pos in [LauncherPosition::BottomLeft, LauncherPosition::BottomCenter, LauncherPosition::BottomRight, LauncherPosition::Center] {
                    let mut i = inputs(pos, layout, bar);
                    i.dock_clearance = Some(dock);
                    let p = place(&i);
                    assert_eq!(p.join, Join::Card, "{pos:?} {layout:?} {bar:?}");
                    assert!(p.rect.bottom() <= SCREEN.1 - dock - band + 0.01, "{pos:?} {layout:?} {bar:?} covered by the dock: {p:?}");
                    if !pos.is_center() {
                        assert!(near(p.rect.bottom(), SCREEN.1 - dock - band), "sits right above it");
                    }
                }
                // Top placements ignore a bottom dock.
                let mut i = inputs(LauncherPosition::TopCenter, layout, bar);
                i.dock_clearance = Some(dock);
                assert_eq!(place(&i), place(&inputs(LauncherPosition::TopCenter, layout, bar)));
            }
        }
    }

    #[test]
    fn center_floats_in_the_middle_with_a_scale_and_fade() {
        let mut i = inputs(LauncherPosition::Center, BarLayout::MinimalEdge, BarPosition::Top);
        i.size = (720.0, 300.0);
        i.max_h = 640.0;
        let p = place(&i);
        assert_eq!((p.join, p.edge, p.fades, p.hover_close), (Join::Card, None, true, false));
        assert!(near(p.rect.x + p.rect.w / 2.0, SCREEN.0 / 2.0));
        assert!(near(p.rect.y, (SCREEN.1 - 640.0) / 2.0), "centered on its tallest size: {p:?}");
        // Fewer results keep the top where it was.
        i.size.1 = 120.0;
        assert!(near(place(&i).rect.y, p.rect.y));
        // Scale + fade from its center.
        assert!(near(p.start.x + p.start.w / 2.0, p.rect.x + p.rect.w / 2.0));
        assert!(near(p.start.y + p.start.h / 2.0, p.rect.y + p.rect.h / 2.0));
        assert!(near(p.start.w, p.rect.w * CENTER_START_SCALE));
        // A short screen keeps it under the bar.
        i.screen = (1280.0, 720.0);
        i.size.1 = 640.0;
        let tight = place(&i);
        assert!(tight.rect.y >= exclusive_zone(&i.bar) + SCREEN_MARGIN - 0.01, "{tight:?}");
        // No animation: it simply appears.
        i.animation = PanelAnimation::None;
        let p = place(&i);
        assert_eq!(p.start, p.rect);
    }

    #[test]
    fn the_launcher_button_anchors_only_in_its_own_zone() {
        // An LTR bar: the launcher button leads on the left.
        let button = (14.0, 34.0);
        let mut i = inputs(LauncherPosition::TopLeft, BarLayout::FloatingPills, BarPosition::Top);
        i.button = Some(button);
        let p = place(&i);
        assert!(p.from_button);
        // Morphs out of the pill itself, with the pill's radius.
        assert!(near(p.start.x, button.0) && near(p.start.w, button.1));
        assert!(p.start_radius.is_some());
        // Not in the right-hand zone: top-right ignores it.
        i.position = LauncherPosition::TopRight;
        assert!(!place(&i).from_button);
        // RTL: the bar mirrors and the button leads on the right; the
        // physical right placement follows it, the left one doesn't.
        i.button = Some((SCREEN.0 - 14.0 - 34.0, 34.0));
        let r = place(&i);
        assert!(r.from_button);
        assert!(near(r.rect.right(), SCREEN.0 - i.bar.side_gap), "clamped inside the margin");
        i.position = LauncherPosition::TopLeft;
        assert!(!place(&i).from_button);
        assert!(near(place(&i).rect.x, i.bar.side_gap), "left stays physically left");
        // A button in the middle third pulls top-center to it.
        i.position = LauncherPosition::TopCenter;
        i.button = Some((1400.0, 34.0));
        assert!(near(place(&i).rect.x + SIZE.0 / 2.0, 1417.0));
        // The bar's edge only: a bottom placement under a top bar doesn't.
        i.position = LauncherPosition::BottomCenter;
        assert!(!place(&i).from_button);
        // Edge frame: a sliver under the button, flares and all.
        let mut e = inputs(LauncherPosition::TopLeft, BarLayout::MinimalEdge, BarPosition::Top);
        e.button = Some((40.0, 34.0));
        let p = place(&e);
        assert_eq!((p.join, p.start.h), (Join::Flares, 0.0));
        assert!(near(p.start.x, 40.0) && near(p.start.y, exclusive_zone(&e.bar)));
    }

    #[test]
    fn slide_and_none_follow_the_panels_animation_setting() {
        let mut i = inputs(LauncherPosition::BottomCenter, BarLayout::MinimalEdge, BarPosition::Bottom);
        i.animation = PanelAnimation::Slide;
        let p = place(&i);
        assert!(p.fades);
        assert!(near(p.start.y - p.rect.y, PANEL_SLIDE_PX), "slides toward the bar it hangs from");
        i.animation = PanelAnimation::None;
        let p = place(&i);
        assert_eq!(p.start, p.rect);
    }

    #[test]
    fn no_bar_on_this_output_means_a_bare_edge() {
        let mut i = inputs(LauncherPosition::TopCenter, BarLayout::FloatingPills, BarPosition::Top);
        i.bar_here = false;
        i.button = Some((1260.0, 40.0));
        let p = place(&i);
        assert_eq!(p.join, Join::Flush);
        assert!(near(p.rect.y, 0.0) && !p.from_button);
    }

    #[test]
    fn narrow_screens_clamp_the_width() {
        let mut i = inputs(LauncherPosition::TopRight, BarLayout::MinimalEdge, BarPosition::Top);
        i.screen = (500.0, 800.0);
        let p = place(&i);
        assert!(p.rect.x - p.flare >= 0.0 && p.rect.right() + p.flare <= 500.0, "{p:?}");
        assert!(p.rect.w > 0.0);
    }

    #[test]
    fn rect_lerp_blends_and_never_goes_negative() {
        let a = Rect::new(0.0, 0.0, 10.0, 0.0);
        let b = Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!(a.lerp(b, 0.5), Rect::new(5.0, 10.0, 20.0, 20.0));
        assert_eq!(a.lerp(b, -0.5).h, 0.0);
        assert_eq!(Rect::from_bounds(b.to_bounds()), b);
    }

    #[test]
    fn zones_split_the_screen_in_thirds() {
        assert!(in_zone(LauncherAlign::Left, 100.0, 900.0));
        assert!(in_zone(LauncherAlign::Center, 450.0, 900.0));
        assert!(in_zone(LauncherAlign::Right, 800.0, 900.0));
        assert!(!in_zone(LauncherAlign::Center, 100.0, 900.0));
    }
}
