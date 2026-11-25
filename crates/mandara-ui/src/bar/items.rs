//! The shapes and colours of the bar's item capsules (`bar.item_capsules`,
//! side bars with pill items): capsules, tiles, squircles and connected
//! groups for items wider than tall (`bar.item_shape`), and round buttons
//! or Material 3 Expressive shapes for icon-sized ones (`bar.icon_shape`,
//! `side_bars.icon_shape`), in the capsule colour or a solid accent or
//! tertiary tone (`bar.icon_fill`).
//!
//! Pure geometry: [`tile`] maps an item's rect to what is drawn under it
//! (plain numbers and fixed-size values, nothing allocated), and the shell
//! paints it. The bar's zones run along physical screen sides (they are
//! never mirrored for right-to-left languages), so a connected group's
//! rounded ends are its physical ends.

use gpui::{Bounds, Corners, Pixels, point, px, size};
use mandara_config::surfaces::{BarIconFill, BarIconShape, BarItemShape};
use mandara_theme::{Color, MandaraPalette};

use crate::components::shapes::Shape;

/// An item at most this many times as long as it is thick counts as
/// icon-sized.
pub const ICON_ASPECT: f32 = 1.2;
/// Tiles take the bar's corner radius, at most this share of their
/// thickness (they never round into capsules).
pub const TILE_MAX_SHARE: f32 = 0.34;
/// A squircle's corner reaches this many times the bar's radius (as the
/// squircle cards do), at most half the item's thickness.
pub const SQUIRCLE_REACH: f32 = 1.6;
/// The small inner corners where connected items meet.
pub const CONNECTED_INNER_PX: f32 = 4.0;
/// The solid icon fills: how far the accent goes toward its container
/// tone (a vivid middle tone that the bar's light and dark icons both read
/// on), and how far the tertiary colour goes toward the bar's surface.
const ACCENT_TONE: f32 = 0.45;
const TERTIARY_TONE: f32 = 0.55;

/// What is drawn under one item.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Tile {
    /// A rounded rectangle with these corners.
    Rounded(Corners<Pixels>),
    /// Continuous superellipse corners reaching this far.
    Squircle(f32),
    /// An expressive shape filling the bounds (a square).
    Shape(Shape),
}

/// An item's tile and where it sits: the item's rect, or the square in its
/// middle for an icon-sized item drawn as a round shape.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Placed {
    pub tile: Tile,
    pub bounds: Bounds<Pixels>,
    /// Drawn in the icon colour.
    pub icon: bool,
}

/// How a bar (or side bar) draws its items.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemLook {
    pub item: BarItemShape,
    pub icon: BarIconShape,
    /// The bar's corner radius (scaled), capped at half an item's thickness
    /// for capsules.
    pub radius: f32,
    /// A side bar: items run down the screen.
    pub vertical: bool,
}

impl ItemLook {
    /// Icon-sized items are made square (laid out at least as long as they
    /// are thick) so they read as round buttons.
    pub fn squares_icons(&self) -> bool {
        self.icon != BarIconShape::Match
    }
}

fn len_across(r: &Bounds<Pixels>, vertical: bool) -> (f32, f32) {
    let (w, h) = (f32::from(r.size.width), f32::from(r.size.height));
    if vertical { (h, w) } else { (w, h) }
}

/// The square of side `d` centred in `r`.
fn centred_square(r: Bounds<Pixels>, d: f32) -> Bounds<Pixels> {
    let (w, h) = (f32::from(r.size.width), f32::from(r.size.height));
    Bounds { origin: point(r.origin.x + px((w - d) / 2.0), r.origin.y + px((h - d) / 2.0)), size: size(px(d), px(d)) }
}

/// The tile under the item at `r`, the `index`th of `count` visible items
/// of its group (in screen order: left to right, top to bottom).
pub fn tile(look: &ItemLook, r: Bounds<Pixels>, index: usize, count: usize) -> Placed {
    let (along, across) = len_across(&r, look.vertical);
    let thick = along.min(across).max(0.0);
    let half = thick / 2.0;
    let icon = look.icon != BarIconShape::Match && along <= across * ICON_ASPECT;
    if icon {
        let square = centred_square(r, thick);
        let round = |shape| Placed { tile: Tile::Shape(shape), bounds: square, icon: true };
        return match look.icon {
            BarIconShape::Circle | BarIconShape::Match => Placed { tile: Tile::Rounded(Corners::all(px(half))), bounds: square, icon: true },
            BarIconShape::Cookie => round(Shape::Cookie),
            BarIconShape::Clover => round(Shape::Clover),
            BarIconShape::Flower => round(Shape::Flower),
            BarIconShape::Burst => round(Shape::Burst),
            BarIconShape::Squircle => Placed { tile: Tile::Squircle((look.radius * SQUIRCLE_REACH).min(half)), bounds: square, icon: true },
        };
    }
    let tile = match look.item {
        BarItemShape::Capsule => Tile::Rounded(Corners::all(px(look.radius.min(half)))),
        BarItemShape::Tile => Tile::Rounded(Corners::all(px(look.radius.min(across * TILE_MAX_SHARE).min(half)))),
        BarItemShape::Squircle => Tile::Squircle((look.radius * SQUIRCLE_REACH).min(half)),
        BarItemShape::Connected => Tile::Rounded(connected(index, count, thick, look.vertical)),
    };
    Placed { tile, bounds: r, icon: false }
}

/// A connected group's corners for item `index` of `count`, `thick` its
/// shorter side: fully round at the group's two ends, small where items
/// meet.
pub fn connected(index: usize, count: usize, thick: f32, vertical: bool) -> Corners<Pixels> {
    let outer = px(thick.max(0.0) / 2.0);
    let inner = px(CONNECTED_INNER_PX.min(thick.max(0.0) / 2.0));
    let first = index == 0;
    let last = index + 1 >= count;
    let (start, end) = (if first { outer } else { inner }, if last { outer } else { inner });
    if vertical {
        Corners { top_left: start, top_right: start, bottom_left: end, bottom_right: end }
    } else {
        Corners { top_left: start, bottom_left: start, top_right: end, bottom_right: end }
    }
}

/// The corners of an item's hover and active wash: the tile's own, or the
/// rounded rectangle inside a squircle or round shape.
pub fn wash_corners(p: &Placed) -> Corners<Pixels> {
    match p.tile {
        Tile::Rounded(c) => c,
        Tile::Squircle(reach) => Corners::all(px(reach)),
        Tile::Shape(_) => Corners::all(p.bounds.size.width / 2.0),
    }
}

/// The colour icon-sized items are drawn in: the capsule colour, or a
/// solid accent or tertiary tone that reads under the bar's icons.
pub fn icon_color(fill: BarIconFill, capsule: Color, palette: &MandaraPalette) -> Color {
    match fill {
        BarIconFill::Match => capsule,
        BarIconFill::Accent => palette.primary.mix(palette.primary_container, ACCENT_TONE),
        BarIconFill::Tertiary => palette.tertiary.mix(palette.surface_base, TERTIARY_TONE),
    }
}

/// The opaque colour a round shape is painted in (its discs overlap, so a
/// see-through colour would double up where they do): `color` composited
/// over the bar's own `under` colour. `None` over a see-through bar, which
/// then draws a plain circle.
pub fn opaque(color: Color, under: Color) -> Option<Color> {
    const OPAQUE: f32 = 0.999;
    if color.alpha() >= OPAQUE {
        Some(color)
    } else if under.alpha() >= OPAQUE {
        Some(color.over(under))
    } else {
        None
    }
}

/// Space between a bar item's tile and the workspace / tag buttons inside
/// it, on each side across the bar.
pub const TAG_INSET_PX: f32 = 4.0;
/// The tag buttons' thickness across the bar: at least the shipped pill's,
/// at most a comfortable round button's.
pub const TAG_MIN_PX: f32 = 22.0;
pub const TAG_MAX_PX: f32 = 32.0;

/// How a bar that shapes its items draws the workspace / tag buttons of
/// the plugin it holds: each button in the bar's own shapes (a flower, a
/// cookie, a squircle, a connected group…), coloured with its icon fill.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TagButtons {
    pub look: ItemLook,
    pub fill: BarIconFill,
    /// The buttons' thickness across the bar.
    pub thick: f32,
}

impl TagButtons {
    /// Square buttons in the icon shape, each with one app icon or the
    /// number (a side bar always; a top or bottom bar with round icon
    /// shapes). Otherwise buttons take the item shape and the shipped
    /// pill's content: the number and up to three app icons.
    pub fn square(&self) -> bool {
        self.look.vertical || self.look.squares_icons()
    }

    /// Button `index` of `count` with content `along` long, placed at the
    /// origin.
    pub fn place(&self, along: f32, index: usize, count: usize) -> Placed {
        let t = self.thick;
        let along = if self.square() { t } else { along.max(t) };
        let r = if self.look.vertical { Bounds { origin: point(px(0.0), px(0.0)), size: size(px(t), px(along)) } } else { Bounds { origin: point(px(0.0), px(0.0)), size: size(px(along), px(t)) } };
        tile(&self.look, r, index, count)
    }
}

/// The tag buttons of `plugin` drawn on a side bar (`vertical`) or a top /
/// bottom one: `Some` while that bar draws its items on tiles of their own
/// (`bar.item_capsules`, every bar style; a side bar with pill items).
/// Plain values only (no allocation).
pub fn tag_buttons(cfg: &mandara_config::ShellConfig, vertical: bool, plugin: &str) -> Option<TagButtons> {
    let scale = crate::style::radius_scale();
    let thick = |room: f32| (room - 2.0 * TAG_INSET_PX).clamp(TAG_MIN_PX, TAG_MAX_PX);
    // In a column form beside a top or bottom bar, the plugin sits on the
    // side bar that holds it (the left one first).
    let side = vertical.then(|| [false, true].into_iter().find(|right| cfg.side_bar_on(*right) && cfg.side_bars.side(*right).zones().iter().any(|z| z.iter().any(|id| id == plugin)))).flatten();
    match side {
        Some(right) => {
            let items = cfg.side_bars.items(right, &cfg.bar);
            if items.chips != mandara_config::side_bars::SideBarChips::Pills {
                return None;
            }
            let m = super::side::metrics(cfg, right, scale);
            Some(TagButtons { look: ItemLook { item: items.item_shape, icon: items.icon_shape, radius: m.radius, vertical: true }, fill: items.icon_fill, thick: thick(items.chip_size) })
        }
        None => {
            let bar = &cfg.bar;
            if !bar.item_capsules {
                return None;
            }
            let m = super::layout::Metrics::of(cfg, scale);
            Some(TagButtons { look: ItemLook { item: bar.item_shape, icon: bar.icon_shape, radius: m.radius, vertical }, fill: bar.icon_fill, thick: thick(m.height - 2.0 * m.chip_inset()) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
    }

    fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
        let e = px(1e-3);
        inner.origin.x + e >= outer.origin.x && inner.origin.y + e >= outer.origin.y && inner.origin.x + inner.size.width <= outer.origin.x + outer.size.width + e && inner.origin.y + inner.size.height <= outer.origin.y + outer.size.height + e
    }

    fn corners_fit(c: Corners<Pixels>, b: Bounds<Pixels>) -> bool {
        let half = b.size.width.min(b.size.height) / 2.0 + px(1e-3);
        [c.top_left, c.top_right, c.bottom_left, c.bottom_right].into_iter().all(|v| v >= px(0.0) && v <= half)
    }

    #[test]
    fn every_tile_stays_inside_its_item() {
        for vertical in [false, true] {
            for item in BarItemShape::ALL {
                for icon in BarIconShape::ALL {
                    for radius in [0.0, 10.0, 22.0] {
                        let look = ItemLook { item, icon, radius, vertical };
                        for rect in [r(10.0, 6.0, 40.0, 40.0), r(10.0, 6.0, 120.0, 40.0), r(3.0, 3.0, 36.0, 90.0), r(0.0, 0.0, 0.0, 0.0)] {
                            for (index, count) in [(0, 1), (0, 3), (1, 3), (2, 3)] {
                                let p = tile(&look, rect, index, count);
                                assert!(inside(p.bounds, rect), "{look:?} {rect:?}: {p:?}");
                                match p.tile {
                                    Tile::Rounded(c) => assert!(corners_fit(c, p.bounds), "{look:?}: {c:?}"),
                                    Tile::Squircle(reach) => assert!(reach >= 0.0 && reach <= f32::from(p.bounds.size.width.min(p.bounds.size.height)) / 2.0 + 1e-3),
                                    Tile::Shape(_) => assert_eq!(p.bounds.size.width, p.bounds.size.height, "round shapes sit in a square"),
                                }
                                assert!(corners_fit(wash_corners(&p), p.bounds));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn icon_sized_items_take_the_icon_shape_and_wide_ones_the_item_shape() {
        let look = ItemLook { item: BarItemShape::Tile, icon: BarIconShape::Cookie, radius: 12.0, vertical: false };
        let icon = tile(&look, r(0.0, 0.0, 40.0, 40.0), 0, 1);
        assert_eq!(icon.tile, Tile::Shape(Shape::Cookie));
        assert!(icon.icon);
        let wide = tile(&look, r(0.0, 0.0, 120.0, 40.0), 0, 1);
        assert_eq!(wide.tile, Tile::Rounded(Corners::all(px(12.0))));
        assert!(!wide.icon);
        // Tiles never round into capsules, whatever the bar radius.
        let tall_radius = ItemLook { radius: 40.0, ..look };
        assert_eq!(tile(&tall_radius, r(0.0, 0.0, 120.0, 40.0), 0, 1).tile, Tile::Rounded(Corners::all(px(40.0 * TILE_MAX_SHARE))));
        // Match keeps icons in the item shape (the shipped capsules).
        let plain = ItemLook { item: BarItemShape::Capsule, icon: BarIconShape::Match, radius: 10.0, vertical: false };
        assert!(!plain.squares_icons());
        assert_eq!(tile(&plain, r(0.0, 0.0, 34.0, 24.0), 0, 1), Placed { tile: Tile::Rounded(Corners::all(px(10.0))), bounds: r(0.0, 0.0, 34.0, 24.0), icon: false });
        // On a side bar "long" runs down the screen.
        let side = ItemLook { vertical: true, ..look };
        assert!(tile(&side, r(0.0, 0.0, 36.0, 40.0), 0, 1).icon);
        assert!(!tile(&side, r(0.0, 0.0, 36.0, 90.0), 0, 1).icon);
    }

    #[test]
    fn a_connected_group_rounds_its_ends_and_mirrors_end_to_end() {
        for vertical in [false, true] {
            for count in 1..=5 {
                for i in 0..count {
                    let c = connected(i, count, 30.0, vertical);
                    let m = connected(count - 1 - i, count, 30.0, vertical);
                    // Read from the other end, the group is its mirror image.
                    if vertical {
                        assert_eq!((c.top_left, c.bottom_left), (m.bottom_left, m.top_left));
                    } else {
                        assert_eq!((c.top_left, c.top_right), (m.top_right, m.top_left));
                        assert_eq!((c.bottom_left, c.bottom_right), (m.bottom_right, m.bottom_left));
                    }
                    let round = px(15.0);
                    let start = if vertical { c.top_left } else { c.bottom_left };
                    assert_eq!(start == round, i == 0, "{vertical} {i}/{count}: only the first item's start is round");
                    let end = if vertical { c.bottom_right } else { c.top_right };
                    assert_eq!(end == round, i + 1 == count);
                }
            }
        }
        assert_eq!(connected(0, 1, 30.0, false), Corners::all(px(15.0)), "a lone item is a capsule");
    }

    #[test]
    fn every_style_keeps_its_icon_items_round() {
        use mandara_config::bar_styles::BarStyle;
        /// A bare icon item's content: one icon at the bar's icon scale.
        const ICON_CONTENT_PX: f32 = 16.0;
        for style in BarStyle::ALL {
            let mut cfg = mandara_config::ShellConfig::default();
            style.apply(&mut cfg);
            if cfg.bar.icon_shape == BarIconShape::Match {
                continue;
            }
            let m = crate::bar::layout::Metrics::of(&cfg, 1.0);
            let across = m.height - 2.0 * m.chip_inset();
            let along = m.chip_length(ICON_CONTENT_PX * cfg.bar.icon_scale());
            let rect = if m.vertical { r(0.0, 0.0, across, along) } else { r(0.0, 0.0, along, across) };
            let look = ItemLook { item: cfg.bar.item_shape, icon: cfg.bar.icon_shape, radius: m.radius, vertical: m.vertical };
            assert!(tile(&look, rect, 0, 1).icon, "{style:?}: an icon item is a {:?}, not a tile", cfg.bar.icon_shape);
        }
    }

    #[test]
    fn round_shapes_get_an_opaque_colour_or_none() {
        let bar = Color::hex("#1d1b20");
        let wash = Color::hex("#ffffff").with_alpha(0.08);
        let solid = opaque(wash, bar).expect("over an opaque bar");
        assert!(solid.alpha() > 0.999 && solid.lightness() > bar.lightness());
        assert_eq!(opaque(bar, wash), Some(bar), "an opaque colour stays");
        assert_eq!(opaque(wash, bar.with_alpha(0.5)), None, "over a see-through bar: a plain circle");
    }
}
