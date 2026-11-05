//! Centered preset thumbnails use the same compact geometry as the bar.

use gpui::{Div, ParentElement, Styled, div, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_theme::MandaraPalette;
use mandara_ui::bar::layout::{Metrics, compute};

use super::Preset;
use crate::tokens::ALPHA_75;

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

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::BarLayout;

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
