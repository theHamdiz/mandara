//! Window chrome and navigation geometry for independent Studio Looks.

use gpui::{
    AnyElement, ClickEvent, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div,
    prelude::FluentBuilder as _, px,
};
use mandara_config::{
    ShellConfig,
    studio::{FrameDesign, StudioFrame},
};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::{
    bidi::Bidi as _,
    components::{
        CardStyle, SurfaceCorners,
        surfaces::{CardSurface as _, Fill, Surface as _},
    },
    icons::icon,
    style::ClipRounded as _,
};

use crate::tokens::*;

/// The chassis's corner reach and title accent use the existing card geometry.
const CHASSIS_RADIUS_SCALE: f32 = 6.0;
/// Traffic lights sit inside full-sized, keyboard-accessible window buttons.
const LIGHT_DIAMETER: f32 = 12.0;
/// A paper window alternates generous and tight corners.
const PAPER_SMALL_CORNER: f32 = 6.0;

pub(super) fn frame(config: &ShellConfig) -> Option<StudioFrame> {
    config.studio.as_deref().map(|s| s.frame.bounded())
}

pub(super) fn sidebar_width(frame: Option<&StudioFrame>, rail: bool) -> f32 {
    if rail {
        RAIL_W_PX
    } else {
        frame.map_or(NAV_W_PX, |f| f.sidebar_width)
    }
}

pub(super) fn sidebar(mut el: Div, frame: Option<&StudioFrame>, p: &MandaraPalette) -> Div {
    let Some(f) = frame else {
        return el.bg(p.surface_container_low.hsla());
    };
    el = el
        .m(px(f.sidebar_inset))
        .rounded(px(f.nav_radius))
        .bg(match f.design {
            FrameDesign::Glass => p.surface_container_high.with_alpha(ALPHA_70),
            FrameDesign::Chassis | FrameDesign::Console | FrameDesign::Terminal => {
                p.surface_container_low
            }
            FrameDesign::Comic => p.primary_container,
            FrameDesign::Editorial => p.surface_base,
            FrameDesign::Classic | FrameDesign::Header => p.surface_container_low,
        }
        .hsla());
    match f.design {
        FrameDesign::Chassis => {
            el.border_b(px(f.border))
                .border_color(p.primary.hsla())
                .rounded_br(px((f.radius * CHASSIS_RADIUS_SCALE)
                    .min(mandara_config::studio::ranges::RADIUS.max)))
        }
        FrameDesign::Console => {
            el.border(px(f.border))
                .border_color(p.primary.hsla())
                .rounded_tl(px((f.radius * CHASSIS_RADIUS_SCALE)
                    .min(mandara_config::studio::ranges::RADIUS.max)))
                .rounded_br(px((f.radius * CHASSIS_RADIUS_SCALE)
                    .min(mandara_config::studio::ranges::RADIUS.max)))
        }
        FrameDesign::Comic | FrameDesign::Terminal => {
            el.border(px(f.border)).border_color(p.on_surface.hsla())
        }
        FrameDesign::Editorial => el.border_b(px(f.border)).border_color(p.outline.hsla()),
        _ => el,
    }
}

pub(super) fn titlebar(
    frame: Option<&StudioFrame>,
    p: &MandaraPalette,
    font: SharedString,
) -> AnyElement {
    let f = frame.cloned().unwrap_or_default();
    let independent = frame.is_some();
    let lights = f.design == FrameDesign::Glass;
    let button = |id: &'static str,
                  glyph: &'static str,
                  label: &'static str,
                  color: mandara_theme::Color,
                  danger: bool| {
        div()
            .id(id)
            .size(px(DIM_30_PX))
            .rounded(px(if lights || !independent {
                RADIUS_15_PX
            } else {
                f.nav_radius
            }))
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .focusable()
            .tab_index(0)
            .tooltip(super::compositor_page::tip(tr(label), p))
            .focus(|s| s.bg(p.primary_container.hsla()))
            .hover(move |s| {
                s.bg(if danger {
                    p.urgent_coral.with_alpha(ALPHA_25)
                } else {
                    p.on_surface.with_alpha(ALPHA_8)
                }
                .hsla())
            })
            .child(if lights {
                div()
                    .size(px(LIGHT_DIAMETER))
                    .rounded_full()
                    .bg(color.hsla())
                    .child(icon(
                        glyph,
                        LIGHT_DIAMETER,
                        p.on_surface.with_alpha(ALPHA_70),
                    ))
                    .into_any_element()
            } else {
                icon(glyph, ICON_14_PX, p.on_surface_variant).into_any_element()
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    };
    let close = button("win-close", "close", tk("Close"), p.urgent_coral, true)
        .on_click(|_: &ClickEvent, window, _| window.remove_window());
    let min = button("win-min", "minus", tk("Minimize"), p.warning_amber, false)
        .on_click(|_, window, _| window.minimize_window());
    let max = button(
        "win-max",
        "maximize",
        tk("Maximize"),
        p.success_emerald,
        false,
    )
    .on_click(|_, window, _| window.zoom_window());
    let mut controls = div()
        .ltr_row()
        .gap(px(SPACE_4_PX))
        .items_center()
        .flex_none();
    controls = if f.controls_at_start {
        controls.child(close).child(min).child(max)
    } else {
        controls.child(min).child(max).child(close)
    };
    let caption = div()
        .flex_1()
        .min_w(px(0.0))
        .truncate()
        .font_family(font)
        .font_weight(FontWeight::SEMIBOLD)
        .when(independent, |el| el.child(t("Mandara Studio")));
    let bar = div()
        .id("studio-titlebar")
        .h(px(f.title_height))
        .flex_none()
        .px(px(SPACE_12_PX))
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .when(
            matches!(
                f.design,
                FrameDesign::Header
                    | FrameDesign::Chassis
                    | FrameDesign::Console
                    | FrameDesign::Terminal
            ),
            |el| {
                el.bg(if f.header_accent {
                    p.primary_container
                } else {
                    p.surface_container
                }
                .hsla())
                    .border_b(px(f.border))
                    .border_color(
                        if matches!(f.design, FrameDesign::Chassis | FrameDesign::Console) {
                            p.primary
                        } else {
                            p.outline
                        }
                        .hsla(),
                    )
            },
        )
        .when(f.design == FrameDesign::Comic, |el| {
            el.bg(p.primary_container.hsla())
                .border_b(px(f.border))
                .border_color(p.on_surface.hsla())
        })
        .when(f.header_accent, |el| el.bg(p.primary_container.hsla()))
        .on_mouse_down(MouseButton::Left, |e, window, _| {
            if e.click_count >= 2 {
                window.zoom_window();
            } else {
                super::begin_move(window);
            }
        });
    if f.controls_at_start {
        bar.child(controls).child(caption).into_any_element()
    } else {
        bar.child(caption).child(controls).into_any_element()
    }
}

/// The default keeps the original window; selected Looks own the entire frame.
pub(super) fn window(
    frame: Option<&StudioFrame>,
    nav: AnyElement,
    content: AnyElement,
    title: Option<AnyElement>,
    p: &MandaraPalette,
) -> Div {
    let Some(f) = frame else {
        return div()
            .size_full()
            .row()
            .bg(p.surface_base.hsla())
            .child(nav)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .row()
                    .flex_col()
                    .children(title)
                    .child(content),
            );
    };
    let family = match f.design {
        FrameDesign::Chassis => SurfaceCorners::Cut,
        FrameDesign::Console => SurfaceCorners::Asymmetric,
        _ => SurfaceCorners::Rounded,
    };
    let radius = if f.design == FrameDesign::Chassis {
        (f.radius * CHASSIS_RADIUS_SCALE).min(mandara_config::studio::ranges::RADIUS.max)
    } else {
        f.radius
    };
    let mut surface = div()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .row()
        .flex_col()
        .clipped_card_surface_as(
            family,
            CardStyle::Tonal,
            radius,
            if f.design == FrameDesign::Glass {
                Fill(
                    p.surface_base.with_alpha(f.opacity),
                    Some(p.surface_container_low.with_alpha(f.opacity)),
                )
            } else {
                Fill(p.surface_base.with_alpha(f.opacity), None)
            },
            if f.design == FrameDesign::Glass {
                mandara_core::material::glass_rim_width(
                    p.specular_rim_light,
                    p.specular_rim_shadow,
                    f.border,
                )
            } else {
                Vec::new()
            },
            p,
        )
        .border(px(f.border))
        .border_color(
            match f.design {
                FrameDesign::Comic | FrameDesign::Terminal => p.on_surface,
                FrameDesign::Chassis | FrameDesign::Console => p.primary,
                _ => p.outline.with_alpha(ALPHA_35),
            }
            .hsla(),
        );
    if f.design == FrameDesign::Editorial {
        surface = surface
            .with_corners(gpui::Corners {
                top_left: px(f.radius),
                top_right: px(PAPER_SMALL_CORNER),
                bottom_left: px(PAPER_SMALL_CORNER),
                bottom_right: px(f.radius),
            })
            .clip_rounded();
    }
    let body = div().flex_1().min_h(px(0.0)).row();
    let body = if nav_at_end(f) {
        body.child(content).child(nav)
    } else {
        body.child(nav).child(content)
    };
    surface = surface.children(title).child(body);
    div()
        .size_full()
        .p(px(f.inset))
        .row()
        .flex_col()
        .child(surface)
}

fn nav_at_end(frame: &StudioFrame) -> bool {
    frame.design == FrameDesign::Console
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::studio::StudioLook;

    #[test]
    fn frame_geometry_is_independent_of_desktop_looks_and_uses_the_rail() {
        let config = ShellConfig {
            studio: Some(Box::new(StudioLook::Telemetry.appearance())),
            ..Default::default()
        };
        let selected = frame(&config).unwrap();
        assert_eq!(sidebar_width(Some(&selected), true), RAIL_W_PX);
        assert_eq!(
            sidebar_width(Some(&selected), false),
            selected.sidebar_width
        );
        for look in mandara_config::looks::LOOKS {
            assert_eq!(
                frame(&look.apply(&config, Default::default())),
                Some(selected.clone())
            );
        }
        assert!(frame(&ShellConfig::default()).is_none());
        assert!(nav_at_end(&StudioLook::Telemetry.appearance().frame));
        assert!(!nav_at_end(&StudioLook::Arcade.appearance().frame));
        assert_ne!(
            StudioLook::Telemetry.appearance().frame.design,
            StudioLook::Arcade.appearance().frame.design
        );
    }
}
