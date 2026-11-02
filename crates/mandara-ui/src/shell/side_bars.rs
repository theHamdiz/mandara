//! The side bars inside the shell surface (`[side_bars]`, geometry in
//! [`crate::bar::side`]). Their groups are three more bar zones per side
//! (after the main bar's three): the same chips, panels, input masking and
//! per-output rules as the main bar, laid out down the screen edge.
//!
//! Joined to a top or bottom bar, a side bar is a column of the bar's own
//! fill hanging from it with concave fillets, so the three read as one
//! frame. Separate ones float as glass pills, an island or a slab. An
//! unpinned side bar slides off its edge and comes back when the pointer
//! rests on that edge.

use std::sync::Arc;

use gpui::{AnyElement, Hsla, RenderImage};
use mandara_config::side_bars::{SideBarAccent, SideBarChips, SideBarShape};

use super::*;
use crate::bar::side;

/// Zones of the main bar (start, centre, end); each side bar's three
/// groups (top, middle, bottom) follow, left then right.
pub(super) const MAIN_ZONES: usize = 3;
pub(super) const ZONES: usize = MAIN_ZONES + 2 * GROUPS;
/// Groups per side bar.
const GROUPS: usize = 3;

/// Reveal / hide spring of an unpinned side bar (the bar auto-hide's feel).
const REVEAL_SPRING: SpringConfig = SpringConfig::new(320.0, 30.0, 1.0);
/// Shown fraction below which a hidden side bar offers its reveal strip.
const STRIP_BELOW: f32 = 0.5;
/// Alpha of the pill / island wash behind side bar items at strength 1.
const CHIP_WASH_ALPHA: f32 = 0.14;
/// Alpha of an item's wash under the pointer (scaled by its hover).
const HOVER_WASH_ALPHA: f32 = 0.1;

/// `(side, group)` of a side bar zone index (`None` for the main bar's).
pub(super) fn split(z: usize) -> Option<(usize, usize)> {
    (MAIN_ZONES..ZONES).contains(&z).then(|| ((z - MAIN_ZONES) / GROUPS, (z - MAIN_ZONES) % GROUPS))
}

/// The palette role a side bar lights up with.
pub(super) fn accent_color(a: SideBarAccent, p: &MandaraPalette) -> Color {
    match a {
        SideBarAccent::Theme => p.primary,
        SideBarAccent::Secondary => p.secondary,
        SideBarAccent::Tertiary => p.tertiary,
        SideBarAccent::Emerald => p.success_emerald,
        SideBarAccent::Amber => p.warning_amber,
        SideBarAccent::Coral => p.urgent_coral,
    }
}

/// Everything a side bar's layout depends on: its groups re-target when
/// any of it changes.
#[derive(Clone, Debug, PartialEq)]
struct SideKey {
    widths: [f32; 3],
    span: (u32, u32),
    chip: u32,
    metrics: Metrics,
}

/// One side bar's live state on this output.
pub(super) struct SideBar {
    pub(super) zones: [Vec<Slot>; 3],
    /// Group rects and the shared backdrop in rail space (see
    /// [`crate::bar::side`]); the span places them on the screen.
    groups: [SpringValue<Bounds<Pixels>>; 3],
    backdrop: SpringValue<Bounds<Pixels>>,
    pill_radius: SpringValue<Pixels>,
    /// 1 = out, 0 = slid off its edge.
    shown: SpringValue<f32>,
    /// The pointer is on the bar (or rested on its edge long enough).
    hovering: bool,
    hide_task: Option<Task<()>>,
    reveal_task: Option<Task<()>>,
    applied: Option<SideKey>,
    pub(super) metrics: Metrics,
    /// `(y, length)` of the screen edge its items are laid out on.
    pub(super) span: (f32, f32),
    /// Items' extent across the bar.
    pub(super) chip: f32,
    /// It shows on this output this frame.
    pub(super) on: bool,
}

impl SideBar {
    pub(super) fn new(right: bool) -> Self {
        let zero = Bounds::default();
        let spring = SpringConfig::default();
        Self {
            zones: [Vec::new(), Vec::new(), Vec::new()],
            groups: [SpringValue::new(zero, spring), SpringValue::new(zero, spring), SpringValue::new(zero, spring)],
            backdrop: SpringValue::new(zero, spring),
            pill_radius: SpringValue::new(px(INITIAL_PILL_RADIUS_PX), spring),
            shown: SpringValue::new(0.0, REVEAL_SPRING),
            hovering: false,
            hide_task: None,
            reveal_task: None,
            applied: None,
            metrics: Metrics::default_for(BarLayout::MinimalEdge, if right { BarPosition::Right } else { BarPosition::Left }),
            span: (0.0, 0.0),
            chip: 0.0,
            on: false,
        }
    }

    pub(super) fn shown(&self) -> f32 {
        self.shown.value().clamp(0.0, 1.0)
    }

    /// The bar's group pill radius right now.
    pub(super) fn pill_radius(&self) -> f32 {
        f32::from(self.pill_radius.value())
    }

    fn step(&mut self, dt: f32) -> bool {
        let mut animating = false;
        for g in &mut self.groups {
            animating |= g.step(dt);
        }
        animating |= self.backdrop.step(dt);
        animating |= self.pill_radius.step(dt);
        animating |= self.shown.step(dt);
        for zone in &mut self.zones {
            for slot in zone {
                slot.hover.set_target(if slot.hovered { 1.0 } else { 0.0 });
                animating |= slot.hover.step(dt);
            }
        }
        animating
    }
}

/// How one zone's chips are drawn (the main bar's or a side bar's).
pub(super) struct ZoneLook {
    pub m: Metrics,
    pub pill_radius: f32,
    /// Room across the bar for an item's content (wider content hides).
    pub room: f32,
    /// One capsule under the whole group / one under every item.
    pub group_capsule: bool,
    pub item_capsules: bool,
    pub fill: Color,
    pub edge: Option<Color>,
    pub accent: Color,
    pub hover: Color,
    pub separator: BarSeparator,
}

impl ShellView {
    /// The slots of zone `z` (main bar or side bar).
    pub(super) fn zone(&self, z: usize) -> Option<&Vec<Slot>> {
        match split(z) {
            Some((s, g)) => Some(&self.sides[s].zones[g]),
            None => self.zones.get(z),
        }
    }

    pub(super) fn zone_mut(&mut self, z: usize) -> Option<&mut Vec<Slot>> {
        match split(z) {
            Some((s, g)) => Some(&mut self.sides[s].zones[g]),
            None => self.zones.get_mut(z),
        }
    }

    /// The bar a zone belongs to: its metrics.
    pub(super) fn zone_metrics(&self, z: usize) -> Metrics {
        split(z).map_or(self.metrics, |(s, _)| self.sides[s].metrics)
    }

    /// Whether the visible panel came out of side bar `side`.
    fn panel_from(&self, side: usize) -> bool {
        self.panel_visible && self.panel_slot.and_then(|p| split(p.0)).is_some_and(|(s, _)| s == side)
    }

    /// Lays out both side bars for this frame and sets where they slide.
    pub(super) fn update_sides(&mut self, cfg: &ShellConfig) {
        let scale = crate::style::radius_scale();
        let bar_here = cfg.bar_on_output(&self.monitor);
        let span = side::span(&self.metrics, self.surface.1);
        for s in 0..2 {
            let right = s == 1;
            let from_panel = self.panel_from(s);
            let bar = &mut self.sides[s];
            if !bar_here || !cfg.side_bar_on(right) {
                bar.on = false;
                bar.applied = None;
                bar.hovering = false;
                bar.shown.snap_to(0.0);
                continue;
            }
            let m = side::metrics(cfg, right, scale);
            let chip = cfg.side_bars.chip_size();
            bar.on = true;
            bar.metrics = m;
            bar.span = span;
            bar.chip = chip;
            // Every group fitted into its share of the edge (as the main
            // bar's zones are: `ShellView::fit_zones`).
            let lengths: [smallvec::SmallVec<[f32; 16]>; 3] = std::array::from_fn(|g| bar.zones[g].iter().map(|sl| content_length(sl, true, chip)).collect());
            let wanted: [f32; 3] = std::array::from_fn(|g| layout::zone_width(&lengths[g], &m));
            let limits = layout::zone_limits(&m, span.1, wanted);
            let mut widths = [0.0; 3];
            for g in 0..GROUPS {
                let fit = layout::fit_slots(&lengths[g], &m, limits[g], g == GROUPS - 1);
                let mut kept: smallvec::SmallVec<[f32; 16]> = smallvec::SmallVec::new();
                for ((slot, limit), len) in bar.zones[g].iter().zip(fit).zip(&lengths[g]) {
                    slot.limit.set(limit);
                    kept.push(limit.map_or(0.0, |l| len.min(l)));
                }
                widths[g] = layout::zone_width(&kept, &m);
            }
            let key = SideKey { widths: widths.map(f32::round), span: (span.0 as u32, span.1 as u32), chip: chip as u32, metrics: m };
            if bar.applied.as_ref() != Some(&key) {
                // The first layout (or the first with items) jumps there.
                let first = bar.applied.as_ref().is_none_or(|k| k.widths.iter().all(|w| *w <= 0.0) || k.metrics.layout != m.layout);
                let geo = layout::compute(&m, span.1, widths);
                for (g, t) in bar.groups.iter_mut().zip(geo.pills) {
                    g.set_target(t);
                }
                bar.backdrop.set_target(geo.backdrop);
                bar.pill_radius.set_target(geo.pill_radius);
                if first {
                    for g in &mut bar.groups {
                        g.snap_to_target();
                    }
                    bar.backdrop.snap_to_target();
                    bar.pill_radius.snap_to_target();
                }
                bar.applied = Some(key);
            }
            let out = cfg.side_bars.side(right).pinned || bar.hovering || from_panel;
            bar.shown.set_target(if out { 1.0 } else { 0.0 });
            if cfg.motion.reduce_motion {
                bar.shown.snap_to_target();
            }
        }
    }

    pub(super) fn step_sides(&mut self, dt: f32) -> bool {
        let mut animating = false;
        for bar in &mut self.sides {
            animating |= bar.step(dt);
        }
        animating
    }

    /// Horizontal slide of side bar `s` this frame.
    fn side_shift(&self, s: usize) -> f32 {
        let bar = &self.sides[s];
        side::shift(&bar.metrics, s == 1, bar.shown())
    }

    /// A rail-space rect of side bar `s` on the screen, slid as it is now.
    fn side_rect(&self, s: usize, r: Bounds<Pixels>) -> Bounds<Pixels> {
        let bar = &self.sides[s];
        let b = side::to_screen(s == 1, r, self.surface, bar.span.0);
        Bounds { origin: point(b.origin.x + px(self.side_shift(s)), b.origin.y), size: b.size }
    }

    /// Chip rects of every visible slot in side bar zone `z`.
    pub(super) fn side_slot_rects(&self, z: usize) -> Vec<Option<Bounds<Pixels>>> {
        let Some((s, g)) = split(z) else { return Vec::new() };
        let bar = &self.sides[s];
        if !bar.on {
            return vec![None; bar.zones[g].len()];
        }
        let extents: smallvec::SmallVec<[f32; 16]> = bar.zones[g].iter().map(|sl| sl.limit.get().map_or(0.0, |l| content_length(sl, true, bar.chip).min(l))).collect();
        side::chip_rects(&bar.metrics, bar.groups[g].value(), &extents, bar.chip).into_iter().map(|r| r.map(|r| self.side_rect(s, r))).collect()
    }

    /// Collapsed and expanded rects of a panel opened from a side bar chip.
    pub(super) fn side_panel_rects(&self, cfg: &ShellConfig, (z, i): (usize, usize), target: gpui::Size<Pixels>) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let (s, _) = split(z)?;
        let bar = &self.sides[s];
        let chip = self.side_slot_rects(z).get(i).copied().flatten()?;
        Some(side::panel_rects(&bar.metrics, s == 1, self.surface, bar.span, chip, target, cfg.panels.animation))
    }

    /// Whether side bar `s` draws the main bar's fillet on its side (it is
    /// joined there, out or not).
    pub(super) fn side_takes_corner(&self, cfg: &ShellConfig, s: usize) -> bool {
        cfg.side_bars_join() && self.sides[s].on
    }

    /// A separate side bar's fill: the frame colour, tinted toward its
    /// accent, at its opacity.
    pub(super) fn side_fill(&self, cfg: &ShellConfig, s: usize, palette: &MandaraPalette) -> Color {
        let sb = &cfg.side_bars;
        let base = frame_color(palette);
        let tinted = if sb.tint() > 0.0 { base.mix(accent_color(sb.side(s == 1).accent, palette), sb.tint()) } else { base };
        tinted.with_alpha(base.alpha() * sb.opacity())
    }

    /// How side bar `s`'s chips are drawn.
    pub(super) fn side_look(&self, cfg: &ShellConfig, s: usize, palette: &MandaraPalette) -> ZoneLook {
        let sb = &cfg.side_bars;
        let accent = accent_color(sb.side(s == 1).accent, palette);
        ZoneLook {
            m: self.sides[s].metrics,
            pill_radius: self.sides[s].pill_radius(),
            room: self.sides[s].chip,
            group_capsule: sb.chips == SideBarChips::Island,
            item_capsules: sb.chips == SideBarChips::Pills,
            fill: accent.with_alpha(CHIP_WASH_ALPHA * sb.chip_fill()),
            edge: None,
            accent,
            hover: accent.with_alpha(if cfg.bar.hover_highlight { HOVER_WASH_ALPHA } else { 0.0 }),
            separator: BarSeparator::None,
        }
    }

    /// The side bars' own shapes: joined columns with their fillets, or
    /// floating pills / island / slab of glass. `joined_fill` is the main
    /// bar's fill; `(y0, y1)` the screen span a joined column covers (to the
    /// main bar's edge, following its auto-hide); `seam` the overlap that
    /// hides the join on opaque fills.
    pub(super) fn side_frames(&self, cfg: &ShellConfig, palette: &MandaraPalette, joined_fill: Hsla, (y0, y1): (f32, f32), seam: f32, scale: f32, frost: Option<&Arc<RenderImage>>, out: (f32, f32)) -> Vec<AnyElement> {
        let mut els = Vec::new();
        let joined = cfg.side_bars_join();
        let bar_on_top = cfg.bar.position != BarPosition::Bottom;
        let snap = |v: f32| mandara_core::snap(v, scale);
        let sb = &cfg.side_bars;
        let click_through = cfg.bar.click_through;
        for (s, bar) in self.sides.iter().enumerate() {
            if !bar.on {
                continue;
            }
            let right = s == 1;
            let shown = bar.shown();
            if joined {
                let col = side::joined_column(right, self.surface.0, bar.metrics.height, shown, (y0, y1), snap(bar.metrics.flare), bar_on_top);
                let (x, y, w, h) = col.rect;
                if w > HIDDEN_EPSILON {
                    // Overlaps the main bar by the seam on opaque fills.
                    let (y, h) = if bar_on_top { (y - seam, h + seam) } else { (y, h + seam) };
                    let (x0, x1) = (snap(x), snap(x + w));
                    els.push(
                        div()
                            .absolute()
                            .left(px(x0))
                            .top(px(snap(y)))
                            .w(px(x1 - x0))
                            .h(px(snap(y + h) - snap(y)))
                            .bg(joined_fill)
                            .when(!click_through, |el| el.child(self.mask.region()))
                            .into_any_element(),
                    );
                }
                for &(fx, fy, r, c) in &col.flares {
                    els.push(concave_flare(snap(fx), snap(fy), r, c, joined_fill));
                }
                continue;
            }
            if shown <= HIDDEN_EPSILON {
                continue;
            }
            // Separate: glass shapes with their own fill, rim and shadow.
            let m = &bar.metrics;
            let fill = self.side_fill(cfg, s, palette).hsla();
            let rim_k = sb.rim_intensity() * if crate::style::borders() { 1.0 } else { 0.0 };
            let rim = mandara_core::glass_rim_width(
                palette.specular_rim_light.with_alpha(RIM_LIGHT_ALPHA * rim_k),
                palette.specular_rim_shadow.with_alpha(RIM_SHADOW_ALPHA * rim_k),
                ranges::BAR_RIM_WIDTH.clamp(cfg.bar.rim_width, mandara_config::surfaces::defaults::BAR_RIM_WIDTH_PX),
            );
            let mut depth = if sb.shadow { crate::plugins::panel_fx::elevation_scaled(palette, shown, cfg.bar.shadow_strength) } else { Vec::new() };
            depth.extend(rim);
            let shapes: smallvec::SmallVec<[(Bounds<Pixels>, f32); 3]> = match sb.shape {
                SideBarShape::Slab => smallvec::smallvec![(bar.backdrop.value(), m.frame_radius())],
                SideBarShape::Island => smallvec::smallvec![(bar.backdrop.value(), bar.pill_radius())],
                SideBarShape::Pills => bar.groups.iter().map(|g| (g.value(), bar.pill_radius())).filter(|(g, _)| f32::from(g.size.width) >= 1.0).collect(),
            };
            for (shape, radius) in shapes {
                let b = self.side_rect(s, shape);
                let (x, y) = (f32::from(b.origin.x), f32::from(b.origin.y));
                els.push(
                    place(b)
                        .rounded(px(radius))
                        .shadow(depth.clone())
                        .when_some(frost.filter(|_| sb.glass), |el, frost| {
                            // The wallpaper, frosted, under a translucent fill.
                            el.child(
                                div().absolute().size_full().rounded(px(radius)).overflow_hidden().child(
                                    gpui::StyledImage::object_fit(gpui::img(gpui::ImageSource::Render(frost.clone())), gpui::ObjectFit::Cover).absolute().left(px(-x)).top(px(-y)).w(px(out.0)).h(px(out.1)),
                                ),
                            )
                        })
                        .child(div().absolute().size_full().rounded(px(radius)).bg(fill))
                        .when(!click_through, |el| el.child(self.mask.region()))
                        .into_any_element(),
                );
            }
        }
        els
    }

    /// Unpinned side bars: the strip on their edge that brings them out
    /// (after `reveal_delay_ms` of rest), and while out a band over them
    /// that starts the hide timer when the pointer leaves.
    pub(super) fn side_triggers(&self, cfg: &ShellConfig, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut els = Vec::new();
        let (reveal, hide) = (cfg.side_bars.reveal_delay(), cfg.side_bars.hide_delay());
        for (s, bar) in self.sides.iter().enumerate() {
            let right = s == 1;
            if !bar.on || cfg.side_bars.side(right).pinned {
                continue;
            }
            let (y, h) = bar.span;
            if bar.shown() < STRIP_BELOW && !bar.hovering {
                els.push(
                    div()
                        .id(("side-bar-reveal", s))
                        .absolute()
                        .left(px(if right { self.surface.0 - EDGE_STRIP } else { 0.0 }))
                        .top(px(y))
                        .w(px(EDGE_STRIP))
                        .h(px(h))
                        // Entering counts only on real motion (GPUI keeps a
                        // stale pointer position once it left the surface).
                        .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, window, cx| {
                            if window.is_window_hovered() && this.sides[s].reveal_task.is_none() {
                                this.arm_side_reveal(s, reveal, cx);
                            }
                        }))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, _| {
                            if !*hovered {
                                this.sides[s].reveal_task = None;
                            }
                        }))
                        .child(self.mask.region())
                        .into_any_element(),
                );
            } else {
                // Over the bar without joining the input region (its own
                // shapes already do): leaving it starts the hide timer.
                let depth = layout::exclusive_zone(&bar.metrics);
                let x = if right { self.surface.0 - depth } else { 0.0 };
                els.push(
                    div()
                        .id(("side-bar-band", s))
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(depth))
                        .h(px(h))
                        .on_mouse_move(cx.listener(move |this, _: &gpui::MouseMoveEvent, window, cx| {
                            let bar = &mut this.sides[s];
                            if window.is_window_hovered() && (!bar.hovering || bar.hide_task.is_some()) {
                                bar.hovering = true;
                                bar.hide_task = None;
                                cx.notify();
                            }
                        }))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if !*hovered {
                                this.schedule_side_hide(s, hide, cx);
                            }
                        }))
                        .into_any_element(),
                );
            }
        }
        els
    }

    /// Brings side bar `s` out once the pointer has rested on its edge for
    /// `delay`.
    fn arm_side_reveal(&mut self, s: usize, delay: Duration, cx: &mut Context<Self>) {
        self.sides[s].reveal_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                let bar = &mut this.sides[s];
                bar.reveal_task = None;
                bar.hide_task = None;
                bar.hovering = true;
                cx.notify();
            });
        }));
    }

    /// Slides side bar `s` away `delay` after the pointer left it, unless
    /// it comes back.
    fn schedule_side_hide(&mut self, s: usize, delay: Duration, cx: &mut Context<Self>) {
        self.sides[s].hide_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                let bar = &mut this.sides[s];
                bar.hide_task = None;
                bar.hovering = false;
                cx.notify();
            });
        }));
    }
}

/// A slot's content length along its bar; 0 (hidden) for content wider
/// than the room across a side bar (plugins without a compact column
/// form).
pub(super) fn content_length(slot: &Slot, vertical: bool, cross_room: f32) -> f32 {
    if vertical && slot.measured_cross.get() > cross_room + MEASURE_EPSILON_PX {
        return 0.0;
    }
    slot.measured.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_zones_follow_the_main_bar() {
        assert_eq!(split(0), None);
        assert_eq!(split(MAIN_ZONES - 1), None);
        assert_eq!(split(MAIN_ZONES), Some((0, 0)));
        assert_eq!(split(ZONES - 1), Some((1, 2)));
        assert_eq!(split(ZONES), None);
        for s in 0..2 {
            for g in 0..GROUPS {
                assert_eq!(split(MAIN_ZONES + s * GROUPS + g), Some((s, g)));
            }
        }
        // Panel slot addresses stay clear of the menu / detached ones.
        assert!(ZONES < DETACHED_SLOT.0 && ZONES < MENU_SLOT.0);
    }
}
