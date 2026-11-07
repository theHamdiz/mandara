//! A replayable spring preview, owned only while the Motion page is visible.

use super::{
    StudioView,
    compositor_page::{button, card, heading},
};
use crate::{
    registry::{self, Section},
    tokens::*,
};
use gpui::{AnyElement, AppContext, Context, Entity, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px};
use mandara_config::{MotionConfig, motion::Preset};
use mandara_core::{
    FrameClock, ToGpui,
    spring::{SpringConfig, SpringValue},
};
use mandara_i18n::{t, tk, tr};
use mandara_ui::{
    bidi::Bidi,
    state::{config, update_config},
};
use std::time::Instant;

/// Enough travel to see overshoot without the dot reaching the card's edges.
const PREVIEW_TRAVEL_PX: f32 = 160.0;
const PRESET_LABELS: [&str; 5] = [tk("Default"), tk("Snappy"), tk("Smooth"), tk("Bouncy"), tk("Subtle")];

pub(super) struct Preview {
    motion: MotionConfig,
    spring: SpringValue<f32>,
    clock: FrameClock,
    visibility: Option<gpui::Subscription>,
}

impl Preview {
    /// Replay live edits while this page owns the preview entity.
    fn new(motion: MotionConfig, cx: &mut Context<Self>) -> Self {
        cx.observe(&config(cx), |this: &mut Self, cfg, cx| {
            let motion = cfg.read(cx).get().motion;
            if motion != this.motion {
                this.motion = motion;
                this.replay();
                cx.notify();
            }
        })
        .detach();
        Self {
            motion,
            spring: SpringValue::new(0.0, SpringConfig::default()),
            clock: FrameClock::new(),
            visibility: None,
        }
    }

    /// Restart the demonstration without changing any user setting.
    fn replay(&mut self) {
        self.spring.snap_to(0.0);
        self.spring.set_target(1.0);
        self.clock.idle();
    }
}

impl Render for Preview {
    /// Animate only while visible and unsettled, using the current motion profile.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.visibility.is_none() {
            let weak = cx.weak_entity();
            self.visibility = Some(window.observe_window_visibility(move |_, window, cx| {
                if !window.is_visible() {
                    let _ = weak.update(cx, |this, _| {
                        this.spring.snap_to(0.0);
                        this.clock.idle();
                    });
                }
            }));
        }
        let p = mandara_core::frame_palette(window, cx);
        let cfg = config(cx).read(cx).snapshot();
        let profile = mandara_ui::style::motion_profile(&cfg);
        if window.is_visible() && self.spring.step_in(self.clock.tick_vsync(Instant::now(), window.refresh_interval(cx)), &profile) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        card(&p)
            .child(heading(
                t("Live preview"),
                t("A slide and fade using your current spring. Replay after adjusting the controls."),
                &p,
            ))
            .child(
                div().h(px(DIM_48_PX)).relative().child(
                    div()
                        .absolute()
                        .left(px(self.spring.value() * PREVIEW_TRAVEL_PX))
                        .size(px(DIM_32_PX))
                        .rounded_full()
                        .bg(p.primary.hsla())
                        .opacity(self.spring.value().clamp(0.0, 1.0)),
                ),
            )
            .child(
                div()
                    .row()
                    .gap(px(SPACE_8_PX))
                    .child(button("motion-replay", "play", t("Replay"), true, &p).on_click(cx.listener(|this, _, _, cx| {
                        this.replay();
                        cx.notify();
                    })))
                    .child(
                        button("motion-preview-reset", "refresh", t("Reset preview"), false, &p).on_click(cx.listener(|this, _, _, cx| {
                            this.spring.snap_to(0.0);
                            this.clock.idle();
                            cx.notify();
                        })),
                    ),
            )
    }
}

impl StudioView {
    /// Keep preset selection, live preview and registry controls on the Motion page.
    pub(super) fn motion_page(&mut self, cfg: &mandara_config::ShellConfig, p: &mandara_theme::MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let preview: Entity<Preview> = self.motion_preview.get_or_insert_with(|| cx.new(|cx| Preview::new(cfg.motion, cx))).clone();
        let current = cfg.motion.preset();
        let presets = card(p)
            .child(heading(
                t("Animation presets"),
                if current.is_none() {
                    t("Custom")
                } else {
                    t("Choose a feel, then adjust the spring below")
                },
                p,
            ))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_8_PX))
                    .children(Preset::ALL.into_iter().zip(PRESET_LABELS).map(|(preset, label)| {
                        button(format!("motion-{preset:?}"), "sliders", tr(label), current == Some(preset), p)
                            .on_click(move |_, _, cx| update_config(cx, |cfg| preset.apply(&mut cfg.motion)))
                    })),
            );
        let rows = self.rows(registry::in_section(Section::Motion), cfg, p, cx);
        div().row().flex_col().gap(px(SPACE_18_PX)).child(presets).child(preview).child(rows).into_any_element()
    }
}
