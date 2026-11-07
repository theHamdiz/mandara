//! A compact placement map, with the same physical sides as the desktop.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px, prelude::FluentBuilder as _};
use mandara_config::{BarLayout, BarPosition, ShellConfig};
use mandara_config::workflow::{LauncherAlign, LauncherEdge, LauncherPosition};
use mandara_core::ToGpui;
use mandara_i18n::{t, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::{bidi::Bidi as _, icons::icon, state::update_config, style::ClipRounded as _};

use super::{StudioView, choice, compositor_page::{button, card, tip}, launcher_page::{LAUNCHER_POSITION_PATH, shell_ipc}};
use crate::{registry, tokens::*};

/// Maximum map width; smaller settings columns scale the illustration down.
const MAP_WIDTH: f32 = 300.0;
/// A wide screen illustration keeps the chooser shorter than its settings.
const MAP_ASPECT: f32 = 0.56;
/// Targets remain comfortably separated even in the smallest Studio column.
const TARGET: f32 = 26.0;
/// Three positions share each edge; keep their hit targets separate.
const EDGE_TARGETS: f32 = 3.0;
/// The diagram bar remains visible after scaling to a miniature.
const BAR: f32 = 7.0;
/// Insets keep the targets clear of the screen's rounded corners.
const INSET: f32 = 10.0;
/// Representative content width for styles whose real width follows plugins.
const COMPACT_SHARE: f32 = 0.6;
/// The selected launcher is an illustration, not another interactive control.
const PREVIEW_SHARE: (f32, f32) = (0.34, 0.32);

fn map_height(width: f32) -> f32 { (width * MAP_ASPECT).max(TARGET * EDGE_TARGETS + INSET * 2.0 + BAR * 2.0 + SPACE_8_PX * 2.0) }

fn horizontal_span(cfg: &ShellConfig, width: f32) -> (f32, f32) {
    let share = match cfg.bar_layout() {
        BarLayout::Notch => cfg.bar.notch_width(),
        BarLayout::Micro | BarLayout::ContiguousIsland => COMPACT_SHARE,
        _ => 1.0,
    };
    let span = (width - INSET * 2.0) * share;
    ((width - span) / 2.0, span)
}

/// Absolute coordinates keep left/right physical in every interface language.
fn target(cfg: &ShellConfig, position: LauncherPosition, width: f32) -> (f32, f32) {
    let height = map_height(width);
    let Some(edge) = position.edge() else { return ((width - TARGET) / 2.0, (height - TARGET) / 2.0) };
    let at_bar = !cfg.bar.position.is_vertical() && (edge == LauncherEdge::Top) == (cfg.bar.position == BarPosition::Top);
    let (start, span) = if at_bar {
        let (_, actual) = horizontal_span(cfg, width);
        let span = actual.max(TARGET * EDGE_TARGETS + SPACE_4_PX * 2.0).min(width - INSET * 2.0);
        ((width - span) / 2.0, span)
    } else { (INSET, width - INSET * 2.0) };
    let x = match position.align() {
        LauncherAlign::Left => start,
        LauncherAlign::Center => start + (span - TARGET) / 2.0,
        LauncherAlign::Right => start + span - TARGET,
    };
    let gap = INSET + if at_bar { BAR } else { 0.0 };
    let y = if edge == LauncherEdge::Top { gap } else { height - gap - TARGET };
    (x, y)
}

pub(super) fn label(position: LauncherPosition) -> SharedString {
    crate::registry::workflow_settings::POSITIONS.iter().find(|(id, _)| *id == position.id()).map(|(_, label)| tr(label).into()).unwrap_or_else(|| position.id().into())
}

impl StudioView {
    pub(super) fn launcher_placement(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let width = MAP_WIDTH.min((self.page.column_w - SPACE_14_PX * 2.0).max(DIM_180_PX));
        let height = map_height(width);
        let current = cfg.launcher.look.position;
        let (bar_x, bar_w) = horizontal_span(cfg, width);
        let mut map = div().relative().w(px(width)).h(px(height)).flex_none().rounded(px(RADIUS_14_PX)).clip_rounded().bg(p.surface_dim.hsla());
        if cfg.bar.position.is_vertical() {
            let x = if cfg.bar.position == BarPosition::Left { 0.0 } else { width - BAR };
            map = map.child(div().absolute().left(px(x)).top_0().w(px(BAR)).h_full().bg(p.primary.with_alpha(ALPHA_30).hsla()));
        } else {
            let y = if cfg.bar.position == BarPosition::Top { 0.0 } else { height - BAR };
            map = map.child(div().absolute().left(px(bar_x)).top(px(y)).w(px(bar_w)).h(px(BAR)).rounded(px(RADIUS_4_PX)).bg(p.primary.with_alpha(ALPHA_30).hsla()));
        }
        let (x, y) = target(cfg, current, width);
        let (preview_w, preview_h) = (width * PREVIEW_SHARE.0, height * PREVIEW_SHARE.1);
        let preview_x = match current.align() {
            LauncherAlign::Left if current.edge().is_some() => x,
            LauncherAlign::Right if current.edge().is_some() => x + TARGET - preview_w,
            _ => x + (TARGET - preview_w) / 2.0,
        }.clamp(INSET, width - INSET - preview_w);
        let preview_y = if current.edge() == Some(LauncherEdge::Bottom) { y + TARGET - preview_h } else { y };
        map = map.child(div().absolute().left(px(preview_x)).top(px(preview_y)).w(px(preview_w)).h(px(preview_h)).rounded(px(RADIUS_9_PX)).bg(p.primary_container.hsla()).border_1().border_color(p.primary.with_alpha(ALPHA_30).hsla()));
        for (index, position) in LauncherPosition::ALL.into_iter().enumerate() {
            let (x, y) = target(cfg, position, width);
            let selected = position == current;
            let (fill, ink) = if selected { (p.primary, p.on_primary) } else { (p.surface_container_high, p.on_surface_variant) };
            map = map.child(div().id(("launcher-place", index)).absolute().left(px(x)).top(px(y)).size(px(TARGET)).rounded_full().focusable().tab_index(0).flex().items_center().justify_center().cursor_pointer().bg(fill.hsla()).when(!selected, |control| control.hover(|s| s.bg(p.primary_container.hsla()))).focus(|s| s.border_2().border_color(p.primary.hsla())).tooltip(tip(label(position), p)).child(icon(if selected { "check" } else { "search" }, ICON_12_PX, ink)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, |cfg| cfg.launcher.look.position = position))).on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") { update_config(cx, |cfg| cfg.launcher.look.position = position); cx.stop_propagation(); }
            })));
        }
        let at_bar = current.edge().is_some_and(|edge| !cfg.bar.position.is_vertical() && (edge == LauncherEdge::Top) == (cfg.bar.position == BarPosition::Top));
        let description = match current.edge() {
            None => t("Floats in the middle of the screen and scales in."),
            Some(_) if cfg.bar.position.is_vertical() => t("A side bar shares no edge with the launcher: it opens from the bare screen edge, beside the bar."),
            Some(_) if !at_bar => t("The bar is on the other edge: the launcher opens from the bare screen edge instead of growing out of the bar."),
            Some(_) if cfg.bar.layout.is_attached() => t("Pours out of the bar with curved flares, like its panels."),
            Some(_) => t("A floating card under the bar, the panels' gap away from it."),
        };
        let setting = registry::in_section(registry::Section::Launcher).find(|setting| setting.path == LAUNCHER_POSITION_PATH).expect("launcher placement is registered");
        let room = if self.stack_rows { self.page.column_w - SPACE_14_PX * 2.0 } else { self.page.column_w - width - SPACE_22_PX - SPACE_14_PX * 2.0 };
        let chooser = self.control(setting, choice::Room::under(room.max(DIM_180_PX)), cfg, p, cx);
        let preview = button("launcher-place-preview", "eye", t("Preview"), false, p).focusable().tab_index(0).on_click(|_, _, cx| shell_ipc(cx, &["launcher", "open"])).on_key_down(|event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") { shell_ipc(cx, &["launcher", "open"]); cx.stop_propagation(); }
        });
        let info = div().flex_1().min_w(px(DIM_180_PX)).row().flex_col().gap(px(SPACE_10_PX)).child(div().row().items_center().gap(px(SPACE_10_PX)).child(div().flex_1().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(label(current))).child(preview)).child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(description))).child(chooser);
        card(p).child(div().row().when(self.stack_rows, |row| row.flex_col()).items_center().gap(px(SPACE_22_PX)).child(map).child(info)).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_target_fits_at_both_widths_and_both_bar_edges() {
        for width in [DIM_180_PX, MAP_WIDTH] {
            for edge in [BarPosition::Top, BarPosition::Bottom, BarPosition::Left, BarPosition::Right] {
                let mut cfg = ShellConfig::default();
                cfg.bar.position = edge;
                for layout in [BarLayout::Notch, BarLayout::Micro, BarLayout::FloatingPills] {
                    cfg.bar.layout = layout;
                    for position in LauncherPosition::ALL {
                        let (x, y) = target(&cfg, position, width);
                        assert!(x >= 0.0 && x + TARGET <= width, "{layout:?}/{edge:?}/{position:?}");
                        assert!(y >= 0.0 && y + TARGET <= map_height(width), "{layout:?}/{edge:?}/{position:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn compact_map_targets_follow_the_bar_without_moving_the_opposite_edge() {
        let mut cfg = ShellConfig::default();
        cfg.bar.position = BarPosition::Top;
        cfg.bar.layout = BarLayout::Micro;
        assert!(target(&cfg, LauncherPosition::TopLeft, MAP_WIDTH).0 > INSET);
        assert_eq!(target(&cfg, LauncherPosition::BottomLeft, MAP_WIDTH).0, INSET);
        let left = target(&cfg, LauncherPosition::TopLeft, MAP_WIDTH).0;
        let right = target(&cfg, LauncherPosition::TopRight, MAP_WIDTH).0;
        assert_eq!(left + right + TARGET, MAP_WIDTH);
    }
}
