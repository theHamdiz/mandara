//! The About header's project and community links. Their small spring
//! gesture ends at rest, so keeping the page open costs no animation work.

use std::time::Instant;

use gpui::{AnyElement, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div, px};
use mandara_config::ShellConfig;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_core::spring::MotionProfile;
use mandara_i18n::t;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;

use super::{PROJECT_URL, StudioView, open_link};
use crate::tokens::*;

pub(super) const DISCORD_URL: &str = "https://discord.gg/kPBaS2reQ3";
/// A gentle rock with one small overshoot, using the global spring feel.
const LINK_SPRING: SpringConfig = SpringConfig::new(360.0, 19.0, 1.0);
/// The glyph tilts by eight degrees and lifts two pixels inside its button.
const TILT_RAD: f32 = 8.0 * std::f32::consts::PI / 180.0;
const LIFT_PX: f32 = 2.0;

pub(super) struct Links {
    hovered: [bool; 2],
    values: [SpringValue<f32>; 2],
    clock: FrameClock,
}

impl Default for Links {
    fn default() -> Self {
        Self { hovered: [false; 2], values: std::array::from_fn(|_| SpringValue::new(0.0, LINK_SPRING)), clock: FrameClock::new() }
    }
}

impl Links {
    fn step(&mut self, dt: f32, profile: &MotionProfile) -> bool {
        let mut moving = false;
        for (value, hovered) in self.values.iter_mut().zip(self.hovered) {
            value.set_target(if hovered { 1.0 } else { 0.0 });
            moving |= value.step_in(dt, profile);
        }
        moving
    }
}

fn link_element(glyph: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div().id(glyph).role(gpui::Role::Link).aria_label(label).focusable().tab_index(0).cursor_pointer()
}

impl StudioView {
    pub(super) fn about_header_links(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let motion = &cfg.motion;
        let profile = MotionProfile::new(motion.stiffness, motion.damping, motion.mass, motion.reduce_motion);
        let dt = self.about_links.clock.tick(Instant::now());
        if self.about_links.step(dt, &profile) { window.request_animation_frame(); }
        else { self.about_links.clock.idle(); }
        let buttons = [("github", t("GitHub"), PROJECT_URL), ("discord", t("Discord"), DISCORD_URL)]
            .into_iter().enumerate().map(|(index, (glyph, label, url))| {
                // Reduced motion keeps the hover colour, without a tilt or lift.
                let amount = if profile.reduce { 0.0 } else { self.about_links.values[index].value() };
                let direction = if index == 0 { -1.0 } else { 1.0 };
                link_element(glyph, label)
                    .h(px(DIM_44_PX)).px(px(SPACE_14_PX)).rounded(px(RADIUS_14_PX))
                    .border_2().border_color(p.primary.with_alpha(0.0).hsla())
                    .row().items_center().gap(px(SPACE_8_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_80).hsla()).text_color(p.on_surface.hsla())
                    .text_size(px(TEXT_12_5_PX))
                    .hover(|style| style.bg(p.primary.with_alpha(ALPHA_14).hsla()))
                    .focus_visible(|style| style.bg(p.primary.with_alpha(ALPHA_20).hsla()).border_color(p.primary.hsla()))
                    .child(div().relative().top(px(-LIFT_PX * amount)).child(gpui::svg().path(format!("icons/{glyph}.svg"))
                        .size(px(ICON_22_PX)).text_color(p.primary.hsla())
                        .with_transformation(gpui::Transformation::rotate(gpui::radians(direction * TILT_RAD * amount)))))
                    .child(label)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.about_links.hovered[index] = *hovered;
                        cx.notify();
                    }))
                    .on_click(move |_, _, _| open_link(url))
                    .into_any_element()
            });
        div().row().flex_wrap().gap(px(SPACE_8_PX)).children(buttons).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_links_have_an_accessible_role_and_name() {
        use gpui::Element as _;
        for (glyph, label) in [("github", "GitHub"), ("discord", "Discord")] {
            let element = link_element(glyph, label);
            assert_eq!(element.a11y_role(), Some(gpui::Role::Link));
            let mut node = gpui::accesskit::Node::new(gpui::Role::Link);
            element.write_a11y_info(&mut node);
            assert_eq!(node.label(), Some(label));
        }
    }

    #[test]
    fn hover_settles_and_leaving_returns_both_links_to_rest() {
        let mut links = Links::default();
        let profile = MotionProfile::default();
        links.hovered[0] = true;
        for _ in 0..300 { links.step(1.0 / 60.0, &profile); }
        assert_eq!(links.values[0].value(), 1.0);
        assert_eq!(links.values[1].value(), 0.0);
        assert!(!links.step(1.0 / 60.0, &profile));
        links.hovered = [false; 2];
        for _ in 0..300 { links.step(1.0 / 60.0, &profile); }
        assert_eq!(links.values[0].value(), 0.0);
        assert!(!links.step(1.0 / 60.0, &profile));
    }

    #[test]
    fn reduced_motion_never_requests_an_animation() {
        let mut links = Links { hovered: [true; 2], ..Default::default() };
        assert!(!links.step(1.0 / 60.0, &MotionProfile { reduce: true, ..Default::default() }));
        assert_eq!(DISCORD_URL, "https://discord.gg/kPBaS2reQ3");
        assert_eq!(PROJECT_URL, "https://github.com/theHamdiz/mandara");
    }
}
