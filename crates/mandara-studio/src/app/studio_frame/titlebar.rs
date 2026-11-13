//! Window controls and the shared header surface for Studio and its previews.

use crate::tokens::*;
use gpui::{
    AnyElement, ClickEvent, Div, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, WindowControls, div,
    prelude::FluentBuilder as _, px,
};
use mandara_config::studio::{FrameDesign, StudioFrame};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tk};
use mandara_theme::MandaraPalette;
use mandara_ui::{bidi::Bidi as _, icons::icon};

/// Traffic lights have an eight-pixel gap and non-overlapping hit targets.
const LIGHT_DIAMETER: f32 = 12.0;
const LIGHT_GLYPH: f32 = 8.0;
const LIGHT_TARGET_WIDTH: f32 = 20.0;
const LIGHTS_GROUP: &str = "studio-window-lights";
const WINDOW_BUTTONS: f32 = 3.0;

#[derive(Copy, Clone)]
enum Control {
    Close,
    Minimize,
    Maximize,
}

impl Control {
    fn supported(self, capabilities: WindowControls) -> bool {
        match self {
            Self::Close => true,
            Self::Minimize => capabilities.minimize,
            Self::Maximize => capabilities.maximize,
        }
    }
    fn apply(self, window: &mut Window) {
        match self {
            Self::Close => window.remove_window(),
            Self::Minimize => window.minimize_window(),
            Self::Maximize => window.zoom_window(),
        }
    }
}

fn target_width(lights: bool) -> f32 {
    if lights {
        LIGHT_TARGET_WIDTH
    } else {
        DIM_30_PX
    }
}

fn header_border(frame: &StudioFrame) -> f32 {
    match frame.design {
        FrameDesign::Header
        | FrameDesign::Chassis
        | FrameDesign::Console
        | FrameDesign::Comic
        | FrameDesign::Terminal => frame.border,
        FrameDesign::Classic | FrameDesign::Glass | FrameDesign::Editorial => 0.0,
    }
}

fn control_height(frame: &StudioFrame) -> f32 {
    DIM_30_PX.min(frame.title_height - header_border(frame))
}

pub(in crate::app) fn titlebar(
    frame: Option<&StudioFrame>,
    p: &MandaraPalette,
    font: SharedString,
    capabilities: WindowControls,
) -> AnyElement {
    let f = frame.cloned().unwrap_or_default();
    let independent = frame.is_some();
    let lights = f.design == FrameDesign::Glass;
    let button = |id: &'static str,
                  glyph: &'static str,
                  label: &'static str,
                  color: mandara_theme::Color,
                  danger: bool,
                  control: Control| {
        let enabled = control.supported(capabilities);
        let reason = tk("Not supported by this compositor");
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(SharedString::new_static(t(label)))
            .w(px(target_width(lights)))
            .h(px(control_height(&f)))
            .rounded(px(if lights || !independent {
                RADIUS_15_PX
            } else {
                f.nav_radius
            }))
            .row()
            .items_center()
            .justify_center()
            .when(enabled, |el| el.cursor_pointer().focusable().tab_index(0))
            .when(!enabled, |el| {
                el.opacity(ALPHA_40)
                    .aria_description(SharedString::new_static(t(reason)))
            })
            .tooltip(crate::app::compositor_page::tip(
                SharedString::new_static(t(if enabled { label } else { reason })),
                p,
            ))
            .focus(|s| s.bg(p.primary_container.hsla()))
            .when(!lights && enabled, |el| {
                el.hover(move |s| {
                    s.bg(if danger {
                        p.urgent_coral.with_alpha(ALPHA_25)
                    } else {
                        p.on_surface.with_alpha(ALPHA_8)
                    }
                    .hsla())
                })
            })
            .child(if lights {
                div()
                    .size(px(LIGHT_DIAMETER))
                    .rounded_full()
                    .bg(color.hsla())
                    .row()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(LIGHT_GLYPH))
                            .opacity(0.0)
                            .group_hover(LIGHTS_GROUP, |s| s.opacity(ALPHA_70))
                            .child(icon(glyph, LIGHT_GLYPH, p.on_surface)),
                    )
                    .into_any_element()
            } else {
                icon(glyph, ICON_14_PX, p.on_surface_variant).into_any_element()
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when(enabled, |el| {
                el.on_click(move |_: &ClickEvent, window, _| control.apply(window))
            })
    };
    let close = button(
        "win-close",
        "close",
        tk("Close"),
        p.urgent_coral,
        true,
        Control::Close,
    );
    let min = button(
        "win-min",
        "minus",
        tk("Minimize"),
        p.warning_amber,
        false,
        Control::Minimize,
    );
    let max = button(
        "win-max",
        "maximize",
        tk("Maximize"),
        p.success_emerald,
        false,
        Control::Maximize,
    );
    let mut controls = div()
        .ltr_row()
        .group(SharedString::new_static(LIGHTS_GROUP))
        .gap(px(if lights { 0.0 } else { SPACE_4_PX }))
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
        .text_size(px(TEXT_12_PX))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.on_surface_variant.hsla())
        .when(lights, |el| el.text_center())
        .child(t("Mandara Studio"));
    let bar = title_surface(div(), &f, p)
        .id("studio-titlebar")
        .h(px(f.title_height))
        .flex_none()
        .px(px(SPACE_12_PX))
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .on_mouse_down(MouseButton::Left, |e, window, _| {
            if e.click_count >= 2 {
                if window.window_controls().maximize {
                    window.zoom_window();
                }
            } else {
                crate::app::begin_move(window);
            }
        });
    if f.controls_at_start {
        bar.child(controls)
            .child(caption)
            .when(lights, |el| {
                el.child(div().w(px(WINDOW_BUTTONS * LIGHT_TARGET_WIDTH)).flex_none())
            })
            .into_any_element()
    } else {
        bar.when(lights, |el| {
            el.child(div().w(px(WINDOW_BUTTONS * LIGHT_TARGET_WIDTH)).flex_none())
        })
        .child(caption)
        .child(controls)
        .into_any_element()
    }
}

/// Real windows and their still previews share the same header surfaces.
pub(in crate::app) fn title_surface(el: Div, f: &StudioFrame, p: &MandaraPalette) -> Div {
    el.when(
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
                .border_b(px(header_border(f)))
                .border_color(
                    if matches!(f.design, FrameDesign::Chassis | FrameDesign::Console) {
                        p.primary.with_alpha(ALPHA_35)
                    } else {
                        p.outline
                    }
                    .hsla(),
                )
        },
    )
    .when(f.design == FrameDesign::Comic, |el| {
        el.bg(p.surface_container.hsla())
            .border_b(px(header_border(f)))
            .border_color(p.on_surface.hsla())
    })
    .when(f.header_accent, |el| el.bg(p.primary_container.hsla()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capabilities_gate_only_the_controls_the_compositor_cannot_provide() {
        for minimize in [false, true] {
            for maximize in [false, true] {
                let capabilities = WindowControls {
                    minimize,
                    maximize,
                    ..Default::default()
                };
                assert!(Control::Close.supported(capabilities));
                assert_eq!(Control::Minimize.supported(capabilities), minimize);
                assert_eq!(Control::Maximize.supported(capabilities), maximize);
            }
        }
    }

    #[test]
    fn compact_lights_keep_separate_targets_inside_the_smallest_header() {
        let gap = target_width(true) - LIGHT_DIAMETER;
        assert!(gap > 0.0 && gap < LIGHT_DIAMETER);
        assert!(target_width(true) < target_width(false));
        for look in mandara_config::studio::StudioLook::ALL {
            let mut frame = look.appearance().frame;
            frame.title_height = mandara_config::studio::ranges::TITLE_HEIGHT.min;
            frame.border = mandara_config::studio::ranges::BORDER.max;
            assert!(control_height(&frame) + header_border(&frame) <= frame.title_height);
            assert!(control_height(&frame) >= LIGHT_DIAMETER);
        }
    }
}
