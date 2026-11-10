//! Centered preset thumbnails use the same compact geometry as the bar.

use gpui::{Div, ParentElement, Styled, div, linear_color_stop, linear_gradient, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::bar_styles::BarStyle;
use mandara_core::ToGpui;
use mandara_theme::MandaraPalette;
use mandara_ui::bar::layout::{Metrics, compute};

use super::Preset;
use crate::tokens::{ALPHA_30, ALPHA_75, RADIUS_1_5_PX, RADIUS_3_PX, RADIUS_4_PX, RADIUS_6_PX, SPACE_6_PX};

/// A uniform item width makes the preset's arrangement readable in miniature.
const ITEM_WIDTH: f32 = 24.0;
/// Reference screen width leaves room for all three preset groups.
const SCREEN_WIDTH: f32 = 600.0;
/// Fit the reference bar into a preset card without changing its proportions.
const SCALE: f32 = 0.4;
/// Visible item marks occupy the middle half of the bar's height.
const ITEM_HEIGHT_FRACTION: f32 = 0.5;

struct Picture {
    width: f32,
    height: f32,
    items: Vec<f32>,
}

fn picture(preset: &Preset) -> Picture {
    let mut cfg = ShellConfig::default();
    cfg.side_bars.enabled = false;
    cfg.bar.layout = preset.layout;
    cfg.bar.position = preset.position;
    cfg.bar.height = preset.height;
    let groups = [preset.left, preset.center, preset.right];
    let widths = groups.map(|items| items.len() as f32 * ITEM_WIDTH);
    let metrics = Metrics::of(&cfg, 1.0);
    let geometry = compute(&metrics, SCREEN_WIDTH, widths);
    let start = f32::from(geometry.backdrop.origin.x);
    let items = groups.into_iter().zip(geometry.pills).flat_map(|(items, bounds)| {
        let x = f32::from(bounds.origin.x) + (f32::from(bounds.size.width) - items.len() as f32 * ITEM_WIDTH) / 2.0;
        (0..items.len()).map(move |i| (x + i as f32 * ITEM_WIDTH + ITEM_WIDTH / 4.0 - start) * SCALE)
    }).collect();
    Picture { width: f32::from(geometry.backdrop.size.width) * SCALE, height: metrics.height * SCALE, items }
}

pub(super) fn centered(preset: &Preset, palette: &MandaraPalette) -> Div {
    let picture = picture(preset);
    let item_height = picture.height * ITEM_HEIGHT_FRACTION;
    div().relative().w(px(picture.width)).h(px(picture.height)).rounded_full().bg(palette.surface_container_high.hsla())
        .children(picture.items.into_iter().map(|x| div().absolute().left(px(x)).top(px((picture.height - item_height) / 2.0))
            .w(px(ITEM_WIDTH * SCALE / 2.0)).h(px(item_height)).rounded_full().bg(palette.primary.with_alpha(ALPHA_75).hsla())))
}

/// A style preset's miniature: its frame and bars, round buttons and
/// tiles, as small marks in the card's mock area.
pub(super) fn styled(style: BarStyle, palette: &MandaraPalette) -> Div {
    let bar = palette.surface_container_high.hsla();
    let tile = palette.surface_container.hsla();
    let button = palette.primary_container.hsla();
    let accent = palette.primary.with_alpha(ALPHA_75).hsla();
    let dot = |d: f32, fill| div().flex_none().size(px(d)).rounded_full().bg(fill);
    let block = |w: f32, h: f32, r: f32, fill| div().flex_none().w(px(w)).h(px(h)).rounded(px(r)).bg(fill);
    let frame = || div().absolute().inset_0().rounded(px(RADIUS_6_PX)).border(px(MOCK_FRAME_PX)).border_color(bar);
    match style {
        BarStyle::Expressive => div()
            .relative()
            .size_full()
            .child(frame())
            // The rail down the left edge, its scalloped badges.
            .child(div().absolute().left_0().top_0().bottom_0().w(px(MOCK_RAIL_PX)).rounded_l(px(RADIUS_6_PX)).bg(bar).flex().flex_col().items_center().justify_center().gap(px(MOCK_GAP_PX)).child(dot(MOCK_DOT_PX, accent)).child(dot(MOCK_DOT_PX, button)))
            // The tall bar along the bottom: round buttons, then tiles.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(MOCK_BAR_PX))
                    .rounded_b(px(RADIUS_6_PX))
                    .bg(bar)
                    .flex()
                    .items_center()
                    .gap(px(MOCK_GAP_PX))
                    .pl(px(MOCK_RAIL_PX + MOCK_GAP_PX))
                    .child(dot(MOCK_BUTTON_PX, button))
                    .child(dot(MOCK_BUTTON_PX, button))
                    .child(dot(MOCK_BUTTON_PX, button))
                    .child(block(MOCK_TILE_W_PX, MOCK_BUTTON_PX, RADIUS_1_5_PX, tile))
                    .child(block(MOCK_TILE_W_PX, MOCK_BUTTON_PX, RADIUS_1_5_PX, tile)),
            ),
        BarStyle::TileDock => div().size_full().flex().flex_col().justify_end().items_center().child(
            div()
                .flex()
                .items_center()
                .gap(px(MOCK_GAP_PX))
                .p(px(MOCK_GAP_PX))
                .rounded(px(RADIUS_4_PX))
                .bg(bar)
                .children((0..MOCK_DOCK_TILES).map(|_| block(MOCK_SQUARE_PX, MOCK_SQUARE_PX, RADIUS_3_PX, button))),
        ),
        BarStyle::Rail => div()
            .relative()
            .size_full()
            .child(frame())
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(MOCK_WIDE_RAIL_PX))
                    .rounded_l(px(RADIUS_6_PX))
                    .bg(bar)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_between()
                    .py(px(MOCK_GAP_PX))
                    .child(dot(MOCK_BUTTON_PX, palette.tertiary.hsla()))
                    .child(block(MOCK_BUTTON_PX, MOCK_TILE_W_PX / 2.0, RADIUS_1_5_PX, tile))
                    .child(dot(MOCK_BUTTON_PX, palette.tertiary.hsla())),
            ),
        BarStyle::Segmented => {
            // Three groups of connected tiles: round outer ends, small inner corners.
            let group = |n: usize| {
                div().flex().gap(px(MOCK_SEGMENT_GAP_PX)).children((0..n).map(move |i| {
                    let (first, last) = (i == 0, i + 1 == n);
                    div()
                        .w(px(MOCK_SEGMENT_PX))
                        .h(px(MOCK_BUTTON_PX))
                        .bg(bar)
                        .rounded(px(MOCK_SEGMENT_INNER_PX))
                        .when(first, |el| el.rounded_l(px(MOCK_BUTTON_PX / 2.0)))
                        .when(last, |el| el.rounded_r(px(MOCK_BUTTON_PX / 2.0)))
                }))
            };
            div().size_full().flex().items_start().justify_between().child(group(3)).child(group(2)).child(group(4))
        }
        BarStyle::SystemMonitor => {
            // A stat block: a square-cornered tile holding a tiny bar graph.
            let graph = |bars: [f32; 4]| {
                div().flex_none().h(px(MOCK_STAT_PX)).px(px(MOCK_STAT_PAD_PX)).rounded(px(RADIUS_1_5_PX)).bg(tile).flex().items_end().gap(px(MOCK_STAT_GAP_PX)).pb(px(MOCK_STAT_PAD_PX))
                    .children(bars.map(|h| div().w(px(MOCK_STAT_BAR_PX)).h(px(h)).rounded(px(MOCK_STAT_BAR_PX / 2.0)).bg(accent)))
            };
            div().size_full().flex().flex_col().justify_end().child(
                div()
                    .h(px(MOCK_BAR_PX))
                    .rounded(px(RADIUS_3_PX))
                    .bg(bar)
                    .px(px(MOCK_STAT_GAP_PX * 2.0))
                    .flex()
                    .items_center()
                    .gap(px(MOCK_STAT_GAP_PX * 2.0))
                    .child(block(MOCK_STAT_PX, MOCK_STAT_PX, RADIUS_1_5_PX, tile))
                    .child(block(MOCK_STAT_WIDE_PX, MOCK_STAT_PX, RADIUS_1_5_PX, tile))
                    .child(div().flex_1())
                    .child(graph(MOCK_GRAPH_A))
                    .child(graph(MOCK_GRAPH_B))
                    .child(graph(MOCK_GRAPH_C))
                    .child(div().flex_1())
                    .child(block(MOCK_STAT_WIDE_PX, MOCK_STAT_PX, RADIUS_1_5_PX, tile))
                    .child(block(MOCK_STAT_PX, MOCK_STAT_PX, RADIUS_1_5_PX, tile)),
            )
        }
        BarStyle::StanleyBridge => {
            // Three docks spanning the bottom, tertiary squircle buttons.
            let tertiary = palette.tertiary.mix(palette.surface_base, MOCK_TERTIARY_TONE).hsla();
            let dock = |n: usize| div().flex().gap(px(MOCK_SEGMENT_GAP_PX * 2.0)).p(px(MOCK_SEGMENT_GAP_PX * 2.0)).rounded(px(RADIUS_4_PX)).bg(bar).children((0..n).map(move |_| block(MOCK_BUTTON_PX, MOCK_BUTTON_PX, RADIUS_3_PX, tertiary)));
            div().size_full().flex().items_end().justify_between().child(dock(3)).child(dock(2)).child(dock(4))
        }
        BarStyle::PharosBeam => {
            // A lantern hanging from the top edge, its light falling below.
            let lantern = div()
                .mt(px(-SPACE_6_PX))
                .h(px(MOCK_LANTERN_PX))
                .px(px(MOCK_GAP_PX))
                .rounded_b(px(RADIUS_6_PX))
                .bg(bar)
                .flex()
                .items_center()
                .gap(px(MOCK_SEGMENT_GAP_PX * 2.0))
                .child(dot(MOCK_BUTTON_PX, button))
                .child(block(MOCK_TILE_W_PX / 2.0, MOCK_BUTTON_PX, RADIUS_1_5_PX, tile))
                .child(dot(MOCK_BUTTON_PX, accent))
                .child(block(MOCK_TILE_W_PX / 2.0, MOCK_BUTTON_PX, RADIUS_1_5_PX, tile))
                .child(dot(MOCK_BUTTON_PX, button));
            let beam = div().w(px(MOCK_BEAM_W_PX)).h(px(MOCK_BEAM_H_PX)).rounded_b(px(RADIUS_6_PX)).bg(linear_gradient(180.0, linear_color_stop(palette.primary.with_alpha(ALPHA_30).hsla(), 0.0), linear_color_stop(palette.primary.with_alpha(0.0).hsla(), 1.0)));
            div().size_full().flex().flex_col().items_center().child(lantern).child(beam)
        }
        BarStyle::RamlTram => {
            // One line across the top and down the left edge: tiles along
            // the top, flower stops down the rail.
            let stop = palette.tertiary.mix(palette.surface_base, MOCK_TERTIARY_TONE).hsla();
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top_0()
                        .h(px(MOCK_TOP_PX))
                        .rounded_t(px(RADIUS_6_PX))
                        .bg(bar)
                        .flex()
                        .items_center()
                        .justify_between()
                        .pl(px(MOCK_RAIL_PX + MOCK_GAP_PX))
                        .pr(px(MOCK_GAP_PX))
                        .child(block(MOCK_TILE_W_PX, MOCK_DOT_PX, RADIUS_1_5_PX, tile))
                        .child(block(MOCK_TILE_W_PX / 2.0, MOCK_DOT_PX, RADIUS_1_5_PX, tile))
                        .child(div().flex().gap(px(MOCK_SEGMENT_GAP_PX * 2.0)).child(dot(MOCK_DOT_PX, stop)).child(dot(MOCK_DOT_PX, stop))),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(MOCK_RAIL_PX))
                        .rounded_l(px(RADIUS_6_PX))
                        .bg(bar)
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(MOCK_SEGMENT_GAP_PX * 2.0))
                        .pt(px(MOCK_TOP_PX))
                        .children((0..MOCK_STOPS).map(move |i| dot(MOCK_DOT_PX, if i == 0 { accent } else { stop }))),
                )
        }
    }
}

/// Sizes of the style miniatures.
const MOCK_FRAME_PX: f32 = 2.0;
const MOCK_RAIL_PX: f32 = 9.0;
const MOCK_WIDE_RAIL_PX: f32 = 13.0;
const MOCK_BAR_PX: f32 = 12.0;
const MOCK_DOT_PX: f32 = 5.0;
const MOCK_BUTTON_PX: f32 = 7.0;
const MOCK_SQUARE_PX: f32 = 8.0;
const MOCK_TILE_W_PX: f32 = 16.0;
const MOCK_GAP_PX: f32 = 3.0;
const MOCK_DOCK_TILES: usize = 5;
const MOCK_SEGMENT_PX: f32 = 9.0;
const MOCK_SEGMENT_GAP_PX: f32 = 1.0;
const MOCK_SEGMENT_INNER_PX: f32 = 1.5;
/// System monitor: stat blocks, a narrow and a wide one, and the bar
/// graphs' bars (heights of three little graphs).
const MOCK_STAT_PX: f32 = 8.0;
const MOCK_STAT_WIDE_PX: f32 = 11.0;
const MOCK_STAT_PAD_PX: f32 = 1.0;
const MOCK_STAT_GAP_PX: f32 = 1.0;
const MOCK_STAT_BAR_PX: f32 = 1.5;
const MOCK_GRAPH_A: [f32; 4] = [2.0, 5.0, 3.0, 6.0];
const MOCK_GRAPH_B: [f32; 4] = [5.0, 2.5, 6.0, 4.0];
const MOCK_GRAPH_C: [f32; 4] = [2.0, 4.0, 2.5, 5.0];
/// The tertiary buttons' tone (as the bar's tertiary fill).
const MOCK_TERTIARY_TONE: f32 = 0.35;
/// Pharos Beam: the lantern's height (reaching the card's top edge) and
/// its light below.
const MOCK_LANTERN_PX: f32 = 13.0;
const MOCK_BEAM_W_PX: f32 = 34.0;
const MOCK_BEAM_H_PX: f32 = 12.0;
/// Raml Tram: the top line's height and the stops down the rail.
const MOCK_TOP_PX: f32 = 9.0;
const MOCK_STOPS: usize = 4;

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::BarLayout;

    /// Preset thumbnails preserve their actual item counts, relative size and visible bounds.
    #[test]
    fn micro_preview_keeps_the_real_compact_height_and_its_four_items() {
        let micro = super::super::PRESETS.iter().find(|p| p.layout == BarLayout::Micro).unwrap();
        let island = super::super::PRESETS.iter().find(|p| p.layout == BarLayout::ContiguousIsland).unwrap();
        let compact = picture(micro);
        let roomy = picture(island);
        assert_eq!(compact.items.len(), 4);
        assert_eq!(roomy.items.len(), 7);
        assert!(compact.width < roomy.width);
        assert!(compact.height < roomy.height);
        for p in [compact, roomy] {
            assert!(p.items.iter().all(|x| *x >= 0.0 && *x + ITEM_WIDTH * SCALE / 2.0 <= p.width));
        }
    }
}
