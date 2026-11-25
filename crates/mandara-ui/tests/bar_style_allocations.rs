//! The bar styles lay out and shape their items without allocating: a
//! frame of every style preset (Expressive, Tile dock, Rail, Segmented,
//! System monitor, Stanley Bridge, Pharos Beam, Raml Tram and the shipped
//! bar) fits its zones, places every item's tile and every workspace / tag
//! button in the bar's shapes, walks the discs of its round shapes and its
//! screen frame from fixed arrays, iterators and plain numbers. The shapes'
//! unit discs are worked out once and kept, so only the first frame builds
//! them.

use std::hint::black_box;

use gpui::{Bounds, Pixels, point, px, size};
use mandara_config::ShellConfig;
use mandara_config::bar_styles::BarStyle;
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bar::items::{self, ItemLook, TagButtons, Tile};
use mandara_ui::bar::layout::{self, Metrics};
use mandara_ui::components::shapes;

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;

/// Frames per measurement (a two-second hover animation at 144 Hz).
const FRAMES: usize = 288;
/// A 2560 × 1440 output, the owner's logical size at 1.5×.
const SURFACE: (f32, f32) = (2560.0, 1440.0);
/// Content widths of a busy bar: icons, a clock, a window title.
const ZONES: [[f32; 5]; 3] = [[18.0, 18.0, 18.0, 160.0, 0.0], [64.0, 18.0, 120.0, 0.0, 0.0], [18.0, 18.0, 18.0, 18.0, 44.0]];

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) }
}

/// Tags on the strip and the content length of each button (a number, or
/// a number and app icons).
const TAGS: [f32; 9] = [22.0, 54.0, 36.0, 54.0, 71.0, 22.0, 22.0, 22.0, 22.0];

/// One frame of the bar `m` with item look `look` and tag buttons `tags`,
/// `t` into a hover (items breathe by a pixel), folded into a number.
fn frame(m: &Metrics, look: &ItemLook, tags: Option<&TagButtons>, palette: &MandaraPalette, t: f32) -> f32 {
    let mut sum = 0.0;
    // The workspace / tag buttons in the bar's shapes.
    if let Some(b) = tags {
        for (i, along) in TAGS.iter().enumerate() {
            let placed = b.place(along + t, i, TAGS.len());
            sum += f32::from(items::wash_corners(&placed).top_left) + f32::from(placed.bounds.size.width);
            if let Tile::Shape(shape) = placed.tile {
                for disc in shapes::discs(shape, 0.0, 0.0, f32::from(placed.bounds.size.width)) {
                    sum += disc.x + disc.y + disc.r;
                }
            }
        }
    }
    let (along, _) = layout::bar_axes(m, SURFACE);
    let wanted: [f32; 3] = std::array::from_fn(|z| layout::zone_width(&ZONES[z], m));
    let limits = layout::zone_limits(m, along, wanted);
    let mut widths = [0.0; 3];
    for z in 0..3 {
        let fit = layout::fit_slots(&ZONES[z], m, limits[z], z == 2);
        let mut kept = [0.0f32; 5];
        for (k, (w, limit)) in ZONES[z].iter().zip(fit.iter()).enumerate() {
            kept[k] = limit.map_or(0.0, |l| w.min(l));
        }
        widths[z] = layout::zone_width(&kept, m);
        // Every visible item's tile, as the shell places them.
        let across = m.height - m.chip_inset() * 2.0;
        let count = kept.iter().filter(|w| **w >= 1.0).count();
        let mut x = 0.0;
        for (k, w) in kept.iter().copied().filter(|w| *w >= 1.0).enumerate() {
            let len = m.chip_length(w) + t;
            let r = if m.vertical { rect(m.chip_inset(), x, across, len) } else { rect(x, m.chip_inset(), len, across) };
            x += len + m.chip_gap;
            let placed = items::tile(look, r, k, count);
            sum += f32::from(items::wash_corners(&placed).top_left) + m.content_inset(w);
            if let Tile::Shape(shape) = placed.tile {
                for disc in shapes::discs(shape, f32::from(placed.bounds.origin.x), f32::from(placed.bounds.origin.y), f32::from(placed.bounds.size.width)) {
                    sum += disc.x + disc.y + disc.r;
                }
            }
            let fill = items::icon_color(mandara_config::surfaces::BarIconFill::Accent, palette.surface_container_high, palette);
            sum += items::opaque(fill.with_alpha(0.5), palette.surface_base).map_or(0.0, |c| c.lightness());
        }
    }
    let g = layout::compute(m, along, widths);
    sum += f32::from(g.backdrop.size.width);
    if let Some(f) = layout::edge_frame(m, SURFACE, SURFACE.1 - m.height, m.flare, [m.position == mandara_config::BarPosition::Bottom, false]) {
        for (x, y, w, h) in f.strips.into_iter().flatten() {
            sum += x + y + w + h;
        }
        for (x, y, r, _) in f.flares.into_iter().flatten() {
            sum += x + y + r;
        }
    }
    sum
}

#[test]
fn every_bar_style_lays_out_and_shapes_its_frames_without_allocating() {
    let palette = mandara_theme::engine::default_palette(true);
    let mut setups: Vec<(&str, Metrics, ItemLook, Option<TagButtons>)> = Vec::new();
    for style in BarStyle::ALL {
        let mut cfg = ShellConfig::default();
        style.apply(&mut cfg);
        let m = Metrics::of(&cfg, 1.0);
        let look = ItemLook { item: cfg.bar.item_shape, icon: cfg.bar.icon_shape, radius: m.radius, vertical: m.vertical };
        let tags = items::tag_buttons(&cfg, m.vertical, "workspaces");
        assert!(tags.is_some(), "{}: its tags take its shapes", style.id());
        setups.push((style.id(), m, look, tags));
        // A style's rail (Expressive's cookies, Raml Tram's flowers).
        if style.rail().is_some() {
            cfg.side_bars.left.middle = vec!["workspaces".into()];
            let rail = cfg.side_bars.icon_shape(&cfg.bar);
            let side = mandara_ui::bar::side::metrics(&cfg, false, 1.0);
            let tags = items::tag_buttons(&cfg, true, "workspaces");
            assert!(tags.is_some_and(|t| t.look.vertical && t.square()), "{}: the rail's tags are round buttons", style.id());
            setups.push(("rail", side, ItemLook { item: cfg.bar.item_shape, icon: rail, radius: side.radius, vertical: true }, tags));
        }
    }
    // A side bar styled on its own, beside a differently styled main bar.
    for (style, name) in BarStyle::SIDE.into_iter().zip(["side bar expressive", "side bar tile-dock", "side bar rail", "side bar segmented"]) {
        let mut cfg = ShellConfig::default();
        BarStyle::Segmented.apply(&mut cfg);
        cfg.side_bars.enabled = true;
        style.apply_to_side(&mut cfg, true);
        cfg.side_bars.right.top = vec!["workspaces".into()];
        let side = mandara_ui::bar::side::metrics(&cfg, true, 1.0);
        let items = cfg.side_bars.items(true, &cfg.bar);
        let tags = items::tag_buttons(&cfg, true, "workspaces");
        setups.push((name, side, ItemLook { item: items.item_shape, icon: items.icon_shape, radius: side.radius, vertical: true }, tags));
    }
    let plain = ShellConfig::default();
    let m = Metrics::of(&plain, 1.0);
    assert!(items::tag_buttons(&plain, false, "workspaces").is_none(), "the shipped bar keeps its liquid pills");
    setups.push(("shipped", m, ItemLook { item: plain.bar.item_shape, icon: plain.bar.icon_shape, radius: m.radius, vertical: false }, None));
    // Every shape's unit discs, worked out once (as the first frame does).
    for shape in shapes::Shape::ALL {
        black_box(shapes::unit(shape));
    }
    for (name, m, look, tags) in &setups {
        let counted = allocations(|| {
            for i in 0..FRAMES {
                black_box(frame(m, look, tags.as_ref(), &palette, (i % 2) as f32));
            }
        });
        assert_eq!(counted, (0, 0), "{name}: no allocation over {FRAMES} frames");
    }
    // A positive control: collecting the same discs does allocate.
    let control = allocations(|| {
        black_box(shapes::discs(shapes::Shape::Burst, 0.0, 0.0, 40.0).collect::<Vec<_>>());
        black_box(Color::hex("#ffffff").to_hex());
    });
    assert!(control.0 >= 2, "the counter sees allocations ({control:?})");
    eprintln!("bar styles: 0 allocations over {FRAMES} frames for {} setups; collecting control {control:?}", setups.len());
}
