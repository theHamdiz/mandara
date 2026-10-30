//! Pomodoro plugin: focus / break timer with a progress ring, session dots
//! and notifications at each phase change. Durations come from
//! `[plugins.settings.pomodoro]` (`focus_minutes`, `short_break_minutes`,
//! `long_break_minutes`, `long_break_every`, `auto_continue`).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use crate::fonts::{fonts, tabular_nums};
use crate::icons::{icon, ring};
use crate::kit::{card, icon_button, reveal, text, tokens};
use crate::productivity::{POMODORO_ID as ID, Phase, PomodoroRules, Productivity, productivity};

/// Dial: ring stroke, track alpha, label gap, and the clock's size as a
/// share of the dial.
const RING_STROKE_PX: f32 = 7.0;
const TRACK_ALPHA: f32 = 0.08;
const DIAL_GAP_PX: f32 = 2.0;
const CLOCK_FRAC: f32 = 0.2;
/// Session dots.
const DOT_PX: f32 = 7.0;
const DOT_GAP_PX: f32 = 5.0;
const DOT_OFF_ALPHA: f32 = 0.12;
/// Controls: round side buttons and the primary pill.
const SIDE_BUTTON_PX: f32 = 34.0;
const MAIN_H_PX: f32 = 40.0;
const MAIN_PAD_X_PX: f32 = 20.0;
const MAIN_HOVER_OPACITY: f32 = 0.92;
const ICON_PX: f32 = 16.0;
/// Dial diameters in the panel and in the sidebar card.
const PANEL_DIAL_PX: f32 = 168.0;
const CARD_DIAL_PX: f32 = 136.0;
const PANEL_W_PX: f32 = 300.0;
const PANEL_H_PX: f32 = 340.0;
/// Bar chip.
const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 5.0;
/// Element-id slots of the controls under a surface's id prefix.
const ID_RESET: usize = 0;
const ID_TOGGLE: usize = 1;
const ID_SKIP: usize = 2;

fn phase_color(phase: Phase, palette: &MandaraPalette) -> Color {
    match phase {
        Phase::Focus => palette.primary,
        Phase::ShortBreak => palette.success_emerald,
        Phase::LongBreak => palette.tertiary,
    }
}

fn body(p: &Productivity, rules: PomodoroRules, palette: &MandaraPalette, ring_d: f32, prefix: &'static str, mono: gpui::SharedString, reveal_cx: Option<&PanelCx>) -> AnyElement {
    let pom = &p.pomodoro;
    let color = phase_color(pom.phase, palette);
    let running = pom.running();
    let dial = div()
        .relative()
        .size(px(ring_d))
        .flex_none()
        .child(ring(pom.progress().clamp(0.0, 1.0), ring_d, RING_STROKE_PX, color, palette.on_surface.with_alpha(TRACK_ALPHA)))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(DIAL_GAP_PX))
                .child(div().font_family(mono).font_features(tabular_nums()).text_size(px(ring_d * CLOCK_FRAC)).font_weight(FontWeight::MEDIUM).child(pom.clock()))
                .child(div().text_size(px(text::LABEL)).text_color(color.hsla()).font_weight(FontWeight::MEDIUM).child(pom.phase.label())),
        );
    // One dot per focus session in the current long-break cycle.
    let in_cycle = pom.completed % rules.long_every;
    let dots = div().flex().gap(px(DOT_GAP_PX)).children((0..rules.long_every).map(|i| {
        div().size(px(DOT_PX)).rounded_full().bg(if i < in_cycle { color.hsla() } else { palette.on_surface.with_alpha(DOT_OFF_ALPHA).hsla() })
    }));
    let controls = div()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(tokens::SM))
        .child(icon_button((prefix, ID_RESET), "refresh", SIDE_BUTTON_PX, palette.on_surface_variant, Some(palette.surface_container_high), |_, cx| {
            productivity(cx).update(cx, |p, cx| p.reset(cx))
        }))
        .child(
            div()
                .id((prefix, ID_TOGGLE))
                .h(px(MAIN_H_PX))
                .px(px(MAIN_PAD_X_PX))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(tokens::SM))
                .cursor_pointer()
                .bg(color.hsla())
                .text_color(color.best_on(palette.on_primary, palette.on_surface).hsla())
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.opacity(MAIN_HOVER_OPACITY))
                .child(icon(if running { "pause" } else { "play" }, ICON_PX, color.best_on(palette.on_primary, palette.on_surface)))
                .child(if running { mandara_i18n::t("Pause") } else if pom.left() < pom.length { mandara_i18n::t("Resume") } else { mandara_i18n::t("Start") })
                .on_click(|_, _, cx| productivity(cx).update(cx, |p, cx| p.toggle(cx))),
        )
        .child(icon_button((prefix, ID_SKIP), "skip", SIDE_BUTTON_PX, palette.on_surface_variant, Some(palette.surface_container_high), |_, cx| {
            productivity(cx).update(cx, |p, cx| p.skip(cx))
        }));
    let head = div()
        .flex()
        .items_center()
        .gap(px(tokens::SM))
        .child(icon("timer", ICON_PX, color))
        .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::t("Pomodoro")))
        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(match pom.completed {
            0 => mandara_i18n::t("No sessions yet").to_string(),
            1 => mandara_i18n::t("1 session").to_string(),
            n => mandara_i18n::tf("{count} sessions", &[("count", &n)]),
        }));
    let blocks: [AnyElement; 4] = [
        head.into_any_element(),
        div().flex().justify_center().child(dial).into_any_element(),
        div().flex().justify_center().child(dots).into_any_element(),
        controls.into_any_element(),
    ];
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::MD))
        .children(blocks.into_iter().enumerate().map(|(i, b)| match reveal_cx {
            Some(r) => reveal(r, i, b),
            None => b,
        }))
        .into_any_element()
}

fn status(cx: &gpui::App) -> serde_json::Value {
    let p = &productivity(cx).read(cx).pomodoro;
    json!({
        "phase": p.phase,
        "running": p.running(),
        "remaining_secs": p.left().as_secs(),
        "remaining": p.clock(),
        "completed_sessions": p.completed,
    })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, mandara_i18n::tk("Pomodoro"), mandara_i18n::tk("Focus timer with breaks and session tracking"),Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let pom = &productivity(cx).read(cx).pomodoro;
                let active = pom.running() || pom.left() < pom.length;
                let color = phase_color(pom.phase, palette);
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon("timer", crate::style::bar_icon(BAR_ICON_PX), if active { color } else { palette.on_surface }))
                    .when(active, |el| {
                        el.child(
                            div()
                                .font_features(tabular_nums())
                                .text_color(if pom.running() { palette.on_surface.hsla() } else { palette.on_surface_variant.hsla() })
                                .child(pom.clock()),
                        )
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        let pcx = panel_cx.clone();
        let panel = fn_view(
            None,
            move |palette, _, cx| {
                let rules = PomodoroRules::from_config(cx);
                let mono = fonts(cx).mono.clone();
                let p = productivity(cx).read(cx);
                div().size_full().p(px(tokens::LG)).child(body(p, rules, palette, PANEL_DIAL_PX, "panel-pom", mono, Some(&pcx))).into_any_element()
            },
            window,
            cx,
        );
        let sidebar = fn_view(
            None,
            |palette, _, cx| {
                let rules = PomodoroRules::from_config(cx);
                let mono = fonts(cx).mono.clone();
                let p = productivity(cx).read(cx);
                card(palette).child(body(p, rules, palette, CARD_DIAL_PX, "card-pom", mono, None)).into_any_element()
            },
            window,
            cx,
        );
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: Rc::new(Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX)))) }),
            sidebar: Some(sidebar),
            on_click: None,
            // Right-click the bar item to start / pause without opening it.
            on_secondary_click: Some(Rc::new(|_, cx| productivity(cx).update(cx, |p, cx| p.toggle(cx)))),
        }
    });
    r.command(
        ID,
        CommandSpec::new("pomodoro", mandara_i18n::tk("Control the focus timer"), |a, cx| {
            let action = a.get(0).unwrap_or("status");
            let p = productivity(cx);
            match action {
                "start" => p.update(cx, |p, cx| p.start(cx)),
                "pause" => p.update(cx, |p, cx| {
                    p.pause();
                    cx.notify();
                }),
                "toggle" => p.update(cx, |p, cx| p.toggle(cx)),
                "reset" => p.update(cx, |p, cx| p.reset(cx)),
                "skip" => p.update(cx, |p, cx| p.skip(cx)),
                "status" => {}
                other => return Err(format!("unknown action {other:?}")),
            }
            Ok(status(cx))
        })
        .arg(ArgSpec::optional("action", ArgKind::Enum, mandara_i18n::tk("what to do (default: status)")).choices(&["start", "pause", "toggle", "reset", "skip", "status"]))
        .example("pomodoro start")
        .example("pomodoro status"),
    );
}
