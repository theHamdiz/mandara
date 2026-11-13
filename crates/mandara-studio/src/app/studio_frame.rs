//! Window chrome and navigation geometry for independent Studio Looks.

mod titlebar;
pub(super) use titlebar::{title_surface, titlebar};

use gpui::{AnyElement, Div, ParentElement, Styled, canvas, div, px};
use mandara_config::{
    ShellConfig,
    studio::{FrameDesign, StudioFrame},
};
use mandara_core::ToGpui as _;
use mandara_theme::MandaraPalette;
use mandara_ui::{
    bidi::Bidi as _,
    components::{
        CardStyle, SurfaceCorners, cut,
        surfaces::{CardSurface as _, Fill, Surface as _, shaped},
    },
    style::ClipRounded as _,
};

use crate::tokens::*;

/// The chassis's corner reach and title accent use the existing card geometry.
const CHASSIS_RADIUS_SCALE: f32 = 6.0;
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
            FrameDesign::Comic => p.surface_container_low,
            FrameDesign::Editorial => p.surface_base,
            FrameDesign::Classic | FrameDesign::Header => p.surface_container_low,
        }
        .hsla());
    match f.design {
        FrameDesign::Chassis => el
            .border_b(px(f.border))
            .border_color(p.primary.with_alpha(ALPHA_35).hsla())
            .with_corners(sidebar_corners(f, mandara_ui::bidi::rtl())),
        FrameDesign::Console => el
            .border(px(f.border))
            .border_color(p.primary.with_alpha(ALPHA_35).hsla())
            .with_corners(sidebar_corners(f, mandara_ui::bidi::rtl())),
        FrameDesign::Comic => el.border(px(f.border)).border_color(p.on_surface.hsla()),
        FrameDesign::Terminal => {
            let el = if mandara_ui::bidi::rtl() {
                el.border_l(px(f.border))
            } else {
                el.border_r(px(f.border))
            };
            el.border_color(p.outline.hsla())
        }
        FrameDesign::Editorial => el.border_b(px(f.border)).border_color(p.outline.hsla()),
        _ => el,
    }
}

fn alternating_corners(large: f32, small: f32, rtl: bool) -> gpui::Corners<gpui::Pixels> {
    let (start, end) = if rtl {
        (px(small), px(large))
    } else {
        (px(large), px(small))
    };
    gpui::Corners {
        top_left: start,
        top_right: end,
        bottom_left: end,
        bottom_right: start,
    }
}

fn sidebar_corners(frame: &StudioFrame, rtl: bool) -> gpui::Corners<gpui::Pixels> {
    let reach =
        (frame.radius * CHASSIS_RADIUS_SCALE).min(mandara_config::studio::ranges::RADIUS.max);
    if frame.design == FrameDesign::Console {
        return alternating_corners(reach, frame.nav_radius, rtl);
    }
    let mut corners = gpui::Corners::all(px(frame.nav_radius));
    if rtl {
        corners.bottom_left = px(reach);
    } else {
        corners.bottom_right = px(reach);
    }
    corners
}

/// The default keeps the original window; selected Looks own the entire frame.
pub(super) fn window(
    frame: Option<&StudioFrame>,
    nav: AnyElement,
    content: AnyElement,
    title: Option<AnyElement>,
    p: &MandaraPalette,
) -> Div {
    window_with(frame, nav, content, title, p, None)
}

pub(super) fn preview(
    frame: &StudioFrame,
    nav: AnyElement,
    content: AnyElement,
    title: AnyElement,
    p: &MandaraPalette,
    ink: mandara_config::desktop::InkStyle,
) -> Div {
    window_with(Some(frame), nav, content, Some(title), p, Some(ink))
}

fn window_with(
    frame: Option<&StudioFrame>,
    nav: AnyElement,
    content: AnyElement,
    title: Option<AnyElement>,
    p: &MandaraPalette,
    ink: Option<mandara_config::desktop::InkStyle>,
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
    let surface = div()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .row()
        .flex_col();
    let fill = if f.design == FrameDesign::Glass {
        Fill(
            p.surface_base.with_alpha(f.opacity),
            Some(p.surface_container_low.with_alpha(f.opacity)),
        )
    } else {
        Fill(p.surface_base.with_alpha(f.opacity), None)
    };
    let rim = if f.design == FrameDesign::Glass {
        mandara_core::material::glass_rim_width(
            p.specular_rim_light,
            p.specular_rim_shadow,
            f.border,
        )
    } else {
        Vec::new()
    };
    let mut surface = if let Some(ink) = ink {
        let mut surface = surface.clipped_card_geometry_as(family, radius, fill, rim, p);
        if let Some(texture) = mandara_ui::plugins::panel_fx::ink_texture_as(ink.texture, p) {
            surface = surface.relative().child(texture);
        }
        if ink.shadow > 0.0 {
            surface = surface.shadow(mandara_ui::plugins::panel_fx::ink_shadow(
                p, ink.shadow, 1.0,
            ));
        }
        surface
    } else {
        surface.clipped_card_surface_as(family, CardStyle::Tonal, radius, fill, rim, p)
    };
    let outline = match f.design {
        FrameDesign::Comic => p.on_surface,
        FrameDesign::Chassis | FrameDesign::Console => p.primary.with_alpha(ALPHA_35),
        _ => p.outline.with_alpha(ALPHA_35),
    };
    if let Some(corner) = shaped(family, radius) {
        let ink = cut::Ink::solid(None, Some((outline, f.border)));
        // The outline follows the chassis silhouette, outside its child mask.
        surface = surface.p(px(f.border)).relative().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    window.with_outer_content_mask(|window| {
                        cut::paint_shape(window, bounds, corner, ink)
                    });
                },
            )
            .absolute()
            .inset_0(),
        );
    } else {
        surface = surface.border(px(f.border)).border_color(outline.hsla());
    }
    if f.design == FrameDesign::Editorial {
        surface = surface
            .with_corners(alternating_corners(
                f.radius,
                PAPER_SMALL_CORNER,
                mandara_ui::bidi::rtl(),
            ))
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
    fn asymmetric_frames_and_navigation_mirror_without_changing_their_size() {
        for look in [StudioLook::Arcade, StudioLook::Telemetry] {
            let frame = look.appearance().frame;
            let ltr = sidebar_corners(&frame, false);
            let rtl = sidebar_corners(&frame, true);
            assert_eq!(ltr.top_left, rtl.top_right);
            assert_eq!(ltr.top_right, rtl.top_left);
            assert_eq!(ltr.bottom_left, rtl.bottom_right);
            assert_eq!(ltr.bottom_right, rtl.bottom_left);
        }
        let radius = StudioLook::Paper.appearance().frame.radius;
        let ltr = alternating_corners(radius, PAPER_SMALL_CORNER, false);
        let rtl = alternating_corners(radius, PAPER_SMALL_CORNER, true);
        assert_eq!(ltr.top_left, rtl.top_right);
        assert_eq!(ltr.bottom_right, rtl.bottom_left);
        assert_ne!(ltr.top_left, ltr.top_right);
    }

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
