//! A replayable spring preview, owned only while the Motion page is visible.

use super::{
    StudioView,
    component_designs::caption,
    compositor_page::{button, card, heading},
    grid,
};
use crate::{
    registry::{self, Section},
    tokens::*,
};
use gpui::{AnyElement, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px};
use mandara_config::{MotionConfig, motion::Preset};
use mandara_core::{
    FrameClock, ToGpui,
    choreo::Choreography,
    spring::{SpringConfig, SpringValue},
};
use mandara_i18n::{t, tf, tr};
use mandara_ui::{
    bidi::Bidi,
    state::{config, update_config},
};
use std::time::Instant;

/// Enough travel to see overshoot without the dot reaching the card's edges.
const PREVIEW_TRAVEL_PX: f32 = 160.0;
/// The preview's row of tiles: how many, their size, and how far they
/// travel as they cascade in.
const PREVIEW_TILES: usize = 6;
const PREVIEW_TILE_W_PX: f32 = DIM_40_PX;
const PREVIEW_TILE_H_PX: f32 = DIM_28_PX;
const PREVIEW_TILE_TRAVEL_PX: f32 = SPACE_18_PX;
/// The tiles' inner dot and how far it trails its tile (parallax).
const PREVIEW_INNER_PX: f32 = SPACE_10_PX;
const PREVIEW_PARALLAX_PX: f32 = SPACE_10_PX;
/// Wallpaper settings the Motion page shows beside its own.
pub(super) const WALLPAPER_ROWS: &[&str] = &[
    "/wallpaper/transition",
    "/wallpaper/transition_origin",
    "/wallpaper/crossfade_ms",
    // Sketch's own options (shown as not applicable under the others).
    "/wallpaper/sketch/ink",
    "/wallpaper/sketch/line_width",
    "/wallpaper/sketch/glow",
    "/wallpaper/sketch/draw_order",
    "/wallpaper/sketch/draw_share",
    "/wallpaper/sketch/fill_style",
    "/wallpaper/sketch/keep_outline",
];
/// The preset tiles.
const PRESET_GRID: CardGrid = CardGrid { min_w: 190.0, max_w: 320.0, gap: SPACE_12_PX };

pub(super) struct Preview {
    motion: MotionConfig,
    /// A preset under the pointer, previewed without changing any setting.
    hovered: Option<Preset>,
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
            hovered: None,
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

    /// Preview `preset` while the pointer rests on its tile (`None`: the
    /// current settings again).
    fn hover(&mut self, preset: Option<Preset>) {
        if self.hovered != preset {
            self.hovered = preset;
            if preset.is_some() {
                self.replay();
            }
        }
    }

    /// The motion being shown: the hovered preset applied over the
    /// current settings (accessibility and intensity stay the user's).
    fn shown(&self) -> MotionConfig {
        let mut m = self.motion;
        if let Some(p) = self.hovered {
            p.apply(&mut m);
        }
        m
    }
}

/// `(left, top, width, height)` of the preview dot at spring value `v`
/// moving at `velocity` (per second): it slides across the track, and with
/// a liquid choreography stretches behind its travel and thins.
fn dot_rect(c: &Choreography, v: f32, velocity: f32) -> (f32, f32, f32, f32) {
    let speed = velocity * PREVIEW_TRAVEL_PX;
    let (left, w) = c.stretch_span(v * PREVIEW_TRAVEL_PX, DIM_32_PX, speed);
    let h = c.thinned(DIM_32_PX, speed);
    (left, (DIM_48_PX - h) / 2.0, w, h)
}

/// `(dx, dy, opacity)` of preview tile `i`: the choreography's cascade,
/// or — with none — the classic reveal (all together, sliding up).
fn tile_motion(c: &Choreography, v: f32, i: usize, rtl: bool) -> (f32, f32, f32) {
    c.item(v, i, (0.0, 1.0), rtl, PREVIEW_TILE_TRAVEL_PX).unwrap_or_else(|| {
        let left = (1.0 - v).clamp(0.0, 1.0);
        (0.0, left * SPACE_10_PX, v.clamp(0.0, 1.0))
    })
}

impl Render for Preview {
    /// Animate only while visible and unsettled, using the shown motion.
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
        let shown = self.shown();
        let reduced = shown.reduced(mandara_core::spring::system_reduced_motion());
        let profile = mandara_ui::style::motion_profile_of(&shown);
        let choreo = if reduced { Choreography::NEUTRAL } else { mandara_ui::style::choreography(&shown) };
        // Springs advance at the global speed; a previewed preset plays at its own.
        let speed = mandara_core::spring::time_scale();
        let rate = if speed > 0.0 { shown.speed.clamp(mandara_core::spring::MIN_TIME_SCALE, mandara_core::spring::MAX_TIME_SCALE) / speed } else { 1.0 };
        let dt = self.clock.tick_vsync(Instant::now(), window.refresh_interval(cx)) * rate;
        if window.is_visible() && self.spring.step_in(dt, &profile) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let v = self.spring.value();
        let (left, top, w, h) = dot_rect(&choreo, v, self.spring.velocity()[0]);
        let rtl = mandara_i18n::language().is_rtl();
        let tiles = (0..PREVIEW_TILES).map(|i| {
            let (dx, dy, alpha) = tile_motion(&choreo, v, i, rtl);
            let (ix, iy) = choreo.parallax_offset(v, (0.0, 1.0), rtl, PREVIEW_PARALLAX_PX);
            div()
                .relative()
                .left(px(dx))
                .top(px(dy))
                .opacity(alpha)
                .w(px(PREVIEW_TILE_W_PX))
                .h(px(PREVIEW_TILE_H_PX))
                .rounded(px(RADIUS_10_PX))
                .bg(p.surface_container_high.hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(div().relative().left(px(ix)).top(px(iy)).size(px(PREVIEW_INNER_PX)).rounded_full().bg(p.primary.with_alpha(ALPHA_88).hsla()))
        });
        let detail = match self.hovered {
            Some(preset) => tf("Previewing {preset}: {feel}", &[("preset", &tr(preset.name())), ("feel", &tr(preset.feel()))]),
            None => t("Your current spring and choreography. Point at a preset to preview it; replay after adjusting the controls.").to_string(),
        };
        card(&p)
            .child(heading(t("Live preview"), detail, &p))
            .child(
                div().h(px(DIM_48_PX)).relative().child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(w))
                        .h(px(h))
                        .rounded_full()
                        .bg(p.primary.hsla())
                        .opacity(v.clamp(0.0, 1.0)),
                ),
            )
            .child(div().row().gap(px(SPACE_8_PX)).h(px(DIM_48_PX)).items_center().overflow_hidden().children(tiles))
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
        let inner = (self.page.column_w - 2.0 * SPACE_18_PX).max(0.0);
        let tiles = Preset::ALL.into_iter().enumerate().map(|(i, preset)| {
            let on = current == Some(preset);
            let hover_preview = preview.clone();
            div()
                .id(("motion-preset", i))
                .rounded(px(RADIUS_18_PX))
                .p(px(SPACE_10_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_6_PX))
                .cursor_pointer()
                .bg(if on { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container_high.hsla() })
                .border_2()
                .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                .children(caption(tr(preset.name()), tr(preset.feel()), on, p))
                .on_hover(move |hovered: &bool, _, cx| {
                    let hovered = *hovered;
                    hover_preview.update(cx, |this, cx| {
                        this.hover(if hovered { Some(preset) } else { this.hovered.filter(|h| *h != preset) });
                        cx.notify();
                    });
                })
                .on_click(move |_: &ClickEvent, _, cx| update_config(cx, |cfg| preset.apply_to(cfg)))
                .into_any_element()
        });
        let presets = card(p)
            .child(heading(
                t("Animation presets"),
                if current.is_none() {
                    t("Custom")
                } else {
                    t("Each preset sets the spring and how content enters; adjust either below")
                },
                p,
            ))
            .child(grid::fit(inner, PRESET_GRID).render(tiles.collect::<Vec<_>>()));
        let rows = self.rows(registry::in_section(Section::Motion), cfg, p, cx);
        // The wallpaper's transition lives on the Wallpaper page too: both
        // edit the same settings.
        let wall: Vec<&registry::Setting> = WALLPAPER_ROWS.iter().filter_map(|path| registry::by_path(path)).collect();
        let wall_rows = self.group_card(&wall, inner, cfg, p, cx);
        let wallpaper = card(p).child(heading(t("Wallpaper change"), t("How a new wallpaper replaces the old one (also on the Wallpaper page)"), p)).child(wall_rows);
        div().row().flex_col().gap(px(SPACE_18_PX)).child(presets).child(preview).child(rows).child(wallpaper).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_core::choreo::Entrance;

    #[test]
    fn the_preview_rests_on_the_same_geometry_for_every_preset() {
        for preset in Preset::ALL {
            let mut m = MotionConfig::default();
            preset.apply(&mut m);
            let c = mandara_ui::style::choreography(&m);
            assert_eq!(dot_rect(&c, 1.0, 0.0), (PREVIEW_TRAVEL_PX, (DIM_48_PX - DIM_32_PX) / 2.0, DIM_32_PX, DIM_32_PX), "{preset:?}");
            for i in 0..PREVIEW_TILES {
                assert_eq!(tile_motion(&c, 1.0, i, false), (0.0, 0.0, 1.0), "{preset:?} tile {i}");
            }
        }
    }

    #[test]
    fn previews_show_each_personality() {
        let liquid = Choreography::new(Entrance::Anchor, 0.0, 1.0, 0.0, 1.0);
        let (_, _, w, h) = dot_rect(&liquid, 0.5, 6.0);
        assert!(w > DIM_32_PX && h < DIM_32_PX, "a moving liquid dot stretches and thins");
        let wave = Choreography::new(Entrance::Rise, 1.0, 0.0, 0.0, 1.0);
        assert!(tile_motion(&wave, 0.4, 0, false).2 > tile_motion(&wave, 0.4, 5, false).2, "tiles cascade");
        assert!(tile_motion(&wave, 0.2, 2, false).1 > 0.0, "rising from below");
        // Reduced motion previews nothing beyond an opacity-free snap.
        assert_eq!(tile_motion(&Choreography::NEUTRAL, 1.0, 3, false), (0.0, 0.0, 1.0));
    }
}
