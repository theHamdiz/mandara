//! The count badge of a bar item (unread notifications, pending updates,
//! new crash reports): one look and one cap for every plugin.
//!
//! On the shipped bar the count sits beside the icon as a small pill. Where
//! the bar draws its icon items as round shapes or squares (a bar style's
//! flowers, cookies and circles, every side bar), the count rides on the
//! shape instead: a pill centred on the shape's outline at its top-right
//! diagonal, the same way on every size, growing toward the middle for
//! two and three digits and never past the room the bar keeps around the
//! shape. The item keeps the icon's size either way, so the shape never
//! changes when a count comes or goes. Counts past [`MAX_SHOWN`] read
//! `99+`; the figures are tabular, so the pill never jitters.
//!
//! The geometry ([`overlay`]) is pure and unit-tested; [`counted`] draws it.

use gpui::{AnyElement, App, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

/// The largest count a badge shows; more reads `99+`.
pub const MAX_SHOWN: usize = 99;

/// The badge beside an icon on the shipped bar.
const INLINE_GAP_PX: f32 = 4.0;
const INLINE_H_PX: f32 = 16.0;
const INLINE_PAD_X_PX: f32 = 4.0;
const INLINE_TEXT_PX: f32 = 10.0;

/// A badge on a round item: its height as a share of the shape's size,
/// within these bounds, and its text as a share of its height.
const OVERLAY_H_SHARE: f32 = 0.36;
const OVERLAY_H_PX: (f32, f32) = (12.0, 15.0);
const OVERLAY_TEXT_SHARE: f32 = 0.66;
const OVERLAY_TEXT_PX: (f32, f32) = (8.0, 10.0);
/// Its side padding as a share of its height.
const OVERLAY_PAD_SHARE: f32 = 0.28;
/// A tabular figure's advance (and `+`'s) in bold, in em.
const FIGURE_EM: f32 = 0.62;
/// How far past the shape a badge may reach, at most (it stays within the
/// room the bar keeps around its items, whichever is less).
const MAX_SPILL_PX: f32 = 3.0;
/// The ring in the bar's colour that cuts a badge out of the shape and
/// the icon under it.
const RING_PX: f32 = 1.5;

/// What a badge reads: the count, or `99+` past [`MAX_SHOWN`].
pub fn label(n: usize) -> SharedString {
    if n > MAX_SHOWN { SharedString::new_static("99+") } else { n.to_string().into() }
}

/// A badge's rect relative to the top-left of the icon it rides on, and
/// its text size.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Overlay {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: f32,
}

/// Where a badge of `glyphs` characters rides on an item drawn as a round
/// shape (or square) `square` px across, centred on an `icon`-px icon, with
/// `spill` px of bar around the shape. Its centre sits on the shape's
/// outline at the top-right diagonal; a wider badge grows toward the
/// middle so it ends at most [`MAX_SPILL_PX`] (or `spill`) past the shape.
pub fn overlay(square: f32, spill: f32, icon: f32, glyphs: usize) -> Overlay {
    let r = square.max(0.0) / 2.0;
    let h = (square * OVERLAY_H_SHARE).clamp(OVERLAY_H_PX.0, OVERLAY_H_PX.1);
    let text = (h * OVERLAY_TEXT_SHARE).clamp(OVERLAY_TEXT_PX.0, OVERLAY_TEXT_PX.1);
    let w = (glyphs.max(1) as f32 * text * FIGURE_EM + h * OVERLAY_PAD_SHARE * 2.0).max(h);
    // From the shape's centre, y up the screen negative.
    let c = r * std::f32::consts::FRAC_1_SQRT_2;
    let bound = r + spill.clamp(0.0, MAX_SPILL_PX);
    let x = (c - w / 2.0).min(bound - w).max(-bound);
    let y = (-c - h / 2.0).max(-bound);
    let half = icon / 2.0;
    Overlay { x: x + half, y: y + half, w, h, text }
}

/// How far across the item's round shape (or square) is, and how much
/// bar there is around it, for plugin `plugin` drawn on a side bar
/// (`vertical`) or a top / bottom one: `None` on a bar that draws its
/// icon items plainly, where the count sits beside the icon.
pub fn round_item(cfg: &mandara_config::ShellConfig, vertical: bool, plugin: &str) -> Option<(f32, f32)> {
    let scale = crate::style::radius_scale();
    if vertical && !cfg.bar.position.is_vertical() {
        // A side bar beside a top or bottom bar: the one holding it.
        let right = [false, true].into_iter().find(|right| cfg.side_bar_on(*right) && cfg.side_bars.side(*right).zones().iter().any(|z| z.iter().any(|id| id == plugin))).unwrap_or(false);
        let chip = cfg.side_bars.items(right, &cfg.bar).chip_size;
        let width = super::side::metrics(cfg, right, scale).height;
        return Some((chip, ((width - chip) / 2.0).max(0.0)));
    }
    let m = super::layout::Metrics::of(cfg, scale);
    let round = cfg.bar.item_capsules && cfg.bar.icon_shape != mandara_config::surfaces::BarIconShape::Match;
    (vertical || round).then(|| (m.height - m.chip_inset() * 2.0, m.chip_inset()))
}

/// A count badge's colours: its fill, the figures on it, and an optional
/// glow (a crash report's alarm).
#[derive(Copy, Clone, Debug)]
pub struct Badge {
    pub fill: Color,
    pub ink: Color,
    pub glow: Option<(Color, f32)>,
}

/// `icon` (`icon_px` across) with `n`'s badge for plugin `plugin`: on the
/// item's shape on bars that draw round items, beside it on the others.
/// Without a count, just the icon.
pub fn counted(icon: impl IntoElement, icon_px: f32, n: usize, badge: Badge, plugin: &str, palette: &MandaraPalette, cx: &App) -> AnyElement {
    if n == 0 {
        return icon.into_any_element();
    }
    let text = label(n);
    let cfg = crate::state::config(cx).read(cx).get();
    let pill = |h: f32, text_px: f32| {
        div()
            .h(px(h))
            .rounded_full()
            .bg(badge.fill.hsla())
            .text_color(badge.ink.hsla())
            .text_size(px(text_px))
            .font_weight(FontWeight::BOLD)
            .font_features(crate::fonts::tabular_nums())
            .flex()
            .items_center()
            .justify_center()
            .when_some(badge.glow, |el, (c, blur)| el.shadow(vec![crate::plugins::panel_fx::glow(c, blur, 0.0, 0.0)]))
    };
    match round_item(cfg, super::vertical(cx), plugin) {
        Some((square, spill)) => {
            let o = overlay(square, spill, icon_px, text.chars().count());
            let ring = crate::shell::frame_color(palette);
            div()
                .relative()
                .size(px(icon_px))
                .flex()
                .items_center()
                .justify_center()
                .child(icon)
                .child(pill(o.h, o.text).absolute().left(px(o.x)).top(px(o.y)).w(px(o.w)).border(px(RING_PX)).border_color(ring.hsla()).child(text))
                .into_any_element()
        }
        None => div()
            .flex()
            .items_center()
            .gap(px(INLINE_GAP_PX))
            .child(icon)
            .child(pill(INLINE_H_PX, INLINE_TEXT_PX).min_w(px(INLINE_H_PX)).px(px(INLINE_PAD_X_PX)).child(text))
            .into_any_element(),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::shapes::{Shape, unit};

    /// Whether a point (from the shape's centre, y down) is inside `shape`
    /// drawn `square` across.
    fn in_shape(shape: Shape, square: f32, x: f32, y: f32) -> bool {
        let r = square / 2.0;
        unit(shape).iter().any(|d| (x / r - d.x).hypot(y / r - d.y) <= d.r + 1e-3)
    }

    #[test]
    fn counts_past_99_read_99_plus() {
        assert_eq!(label(2), "2");
        assert_eq!(label(14), "14");
        assert_eq!(label(99), "99");
        for n in [100, 1000, 12_345, usize::MAX] {
            assert_eq!(label(n), "99+", "{n}");
        }
    }

    #[test]
    fn a_badge_rides_the_shape_and_never_leaves_the_bar() {
        // Every round item a style draws: the bar's squares and the rails'.
        let mut sizes: Vec<(String, f32, f32, f32)> = Vec::new();
        for style in mandara_config::bar_styles::BarStyle::ALL {
            let mut cfg = mandara_config::ShellConfig::default();
            style.apply(&mut cfg);
            let icon = 16.0 * cfg.bar.icon_scale();
            let vertical = cfg.bar.position.is_vertical();
            if let Some((square, spill)) = round_item(&cfg, vertical, "notifications") {
                sizes.push((format!("{style:?} bar"), square, spill, icon));
            }
            if cfg.side_bar_on(false) {
                let (square, spill) = round_item(&cfg, true, "workspaces").expect("a side bar always rides its badges");
                sizes.push((format!("{style:?} rail"), square, spill, icon));
            }
        }
        assert!(sizes.len() >= 6, "{sizes:?}");
        for (what, square, spill, icon) in sizes {
            let r = square / 2.0;
            let bound = r + spill.min(MAX_SPILL_PX) + 1e-3;
            let mut last_w = 0.0;
            for n in [2, 14, 99, 100, 1000] {
                let o = overlay(square, spill, icon, label(n).chars().count());
                // From the shape's centre.
                let (x0, y0) = (o.x - icon / 2.0, o.y - icon / 2.0);
                let (x1, y1) = (x0 + o.w, y0 + o.h);
                assert!(x0 >= -bound && x1 <= bound && y0 >= -bound && y1 <= bound, "{what} {n}: {o:?} stays within the bar around the shape");
                assert!(o.h >= OVERLAY_H_PX.0 && o.text >= OVERLAY_TEXT_PX.0, "{what}: legible");
                assert!(o.w >= o.h - 1e-3, "{what} {n}: a round pill at least");
                assert!(o.w >= last_w, "{what} {n}: it grows with the figures");
                last_w = o.w;
                // It sits on the top-right of the shape: its middle above and
                // right of the icon's, on the outline for a single figure.
                assert!(x0 + o.w / 2.0 > 0.0 && y0 + o.h / 2.0 < 0.0, "{what} {n}");
                if n < 10 {
                    let (cx, cy) = (x0 + o.w / 2.0, y0 + o.h / 2.0);
                    assert!((cx.hypot(cy) - r).abs() < 1.0, "{what}: centred on the outline ({cx}, {cy}) r {r}");
                    // Every shape reaches out under it: the badge is anchored
                    // to the shape, not floating beside it.
                    for shape in Shape::ALL {
                        assert!(in_shape(shape, square, cx * 0.85, cy * 0.85), "{what}: the {shape:?} reaches the badge");
                    }
                }
            }
            // The pill's ends are round and its figures fit it.
            let o = overlay(square, spill, icon, 3);
            assert!(3.0 * o.text * FIGURE_EM <= o.w);
        }
    }

    #[test]
    fn the_shipped_bar_keeps_its_count_beside_the_icon() {
        let cfg = mandara_config::ShellConfig::default();
        assert_eq!(round_item(&cfg, false, "notifications"), None);
        // A side bar or a vertical bar always rides it on the item.
        assert!(round_item(&cfg, true, "notifications").is_some());
        let mut rail = mandara_config::ShellConfig::default();
        mandara_config::bar_styles::BarStyle::Rail.apply(&mut rail);
        let (square, spill) = round_item(&rail, true, "notifications").unwrap();
        let m = super::super::layout::Metrics::of(&rail, 1.0);
        assert_eq!(square + spill * 2.0, m.height, "the Rail's flower and the room round it make the rail");
    }
}
