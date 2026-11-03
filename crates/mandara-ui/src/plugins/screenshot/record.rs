//! The screen recorder's part of the screenshot plugin: the bar chip while
//! recording (red dot + timer, a pause glyph when paused, the countdown),
//! the panel's recording section (what records and how, start / pause /
//! stop / mute, the display to record) and its recent recordings.

use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement as _, IntoElement, ParentElement, StatefulInteractiveElement as _, Styled, div, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_plugins::PanelCx;
use mandara_services::recording::backend::{self, Backend, Installed};
use mandara_services::recording::history::Recent;
use mandara_services::recording::plan;
use mandara_services::recording::session::format_elapsed;
use mandara_theme::MandaraPalette;

use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{alpha, icon_button, list_row, metrics, reveal, text, toggle_tile, tokens};
use crate::recorder::{self, Source};
use crate::state::{shell, update_config};

/// Lead-in before a recording started from the panel (lets the panel
/// close first).
pub(super) const RECORD_LEAD_IN: Duration = Duration::from_millis(250);
/// Bar chip glyph, recording dot and gap.
const BAR_ICON_PX: f32 = 15.0;
const REC_DOT_PX: f32 = 9.0;
const REC_GAP_PX: f32 = 6.0;
/// Recent recordings listed in the panel.
pub(super) const RECENT_SHOWN: usize = 3;
/// Height of a section title line in the panel.
pub(super) const SECTION_H_PX: f32 = 18.0;
/// Display chips: height and the icon buttons on recent rows.
pub(super) const CHIP_H_PX: f32 = 26.0;
const ROW_BUTTON_PX: f32 = 28.0;
/// Opacity of a tile that can't be used right now.
const LOCKED_OPACITY: f32 = 0.45;

/// The bar chip: the camera, or the recorder's state.
pub(super) fn bar_chip(palette: &MandaraPalette, cx: &mut App) -> AnyElement {
    let Some(rec) = recorder::recorder(cx) else { return camera(palette) };
    let r = rec.read(cx);
    let coral = palette.urgent_coral;
    let timer = |text: String, dim: bool| div().font_features(crate::fonts::tabular_nums()).text_color(if dim { palette.on_surface_variant.hsla() } else { coral.hsla() }).child(text);
    if let Some(left) = r.countdown_left() {
        return div().ltr_row().items_center().gap(px(REC_GAP_PX)).child(icon("timer", crate::style::bar_icon(BAR_ICON_PX), coral)).child(timer(left.to_string(), false)).into_any_element();
    }
    match (r.clock(), r.paused(), r.busy) {
        (Some(clock), true, _) => div().ltr_row().items_center().gap(px(REC_GAP_PX)).child(icon("pause", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface_variant)).child(timer(clock, true)).into_any_element(),
        // Recording: red dot + elapsed time (click to stop).
        (Some(clock), false, _) => div().ltr_row().items_center().gap(px(REC_GAP_PX)).child(div().size(px(REC_DOT_PX)).rounded_full().bg(coral.hsla())).child(timer(clock, false)).into_any_element(),
        (None, _, true) => div().flex().items_center().child(icon("timer", crate::style::bar_icon(BAR_ICON_PX), coral)).into_any_element(),
        _ => camera(palette),
    }
}

fn camera(palette: &MandaraPalette) -> AnyElement {
    div().flex().items_center().child(icon("camera", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface)).into_any_element()
}

/// A tile that can't be used right now: dimmed, with why.
fn locked_tile(id: &'static str, icon_name: &'static str, label: String, reason: String, palette: &MandaraPalette) -> AnyElement {
    div()
        .id(id)
        .flex_1()
        .min_w(px(0.0))
        .h(px(metrics::TILE_H))
        .px(px(tokens::MD))
        .rounded(px(crate::style::radius(tokens::LG)))
        .bg(palette.surface_container_high.hsla())
        .opacity(LOCKED_OPACITY)
        .row()
        .items_center()
        .gap(px(tokens::SM + metrics::NUDGE))
        .child(icon(icon_name, metrics::TILE_ICON, palette.on_surface))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).truncate().child(label))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(reason)),
        )
        .into_any_element()
}

/// What the next recording uses, or why it can't start: `GPU Screen
/// Recorder · H.264 · MP4 · 30 fps`.
fn summary(cx: &App) -> Result<String, String> {
    if let Some(s) = recorder::recorder(cx).and_then(|r| r.read(cx).active.as_ref().map(|s| (s.backend(), s.plan.summary()))) {
        return Ok(format!("{} · {}", tr(s.0.label()), s.1));
    }
    let settings = recorder::settings(cx);
    let installed = Installed::cached();
    let backend = installed.resolve(settings.backend).map_err(tr)?;
    let info = match (&installed.gpu_screen_recorder, backend) {
        (Some(path), Backend::GpuScreenRecorder) => backend::gsr_info(path),
        _ => None,
    };
    Ok(format!("{} · {}", tr(backend.label()), plan::plan(&settings, backend, info.as_ref()).summary()))
}

/// Which display "Record screen" takes.
fn display_label(cx: &App) -> String {
    let chosen = recorder::settings(cx).monitor;
    if chosen.trim().is_empty() { t("Focused display").into() } else { chosen }
}

/// The recording section: a title with what records, the action tiles and
/// (several displays, idle) the display chips. Returns the blocks and
/// their height.
pub(super) fn section(p: &PanelCx, first: usize, palette: &MandaraPalette, cx: &mut App) -> (Vec<AnyElement>, f32) {
    let mut blocks = Vec::new();
    let mut height = 0.0;
    let gap = tokens::MD;
    let info = summary(cx);
    let title = div()
        .h(px(SECTION_H_PX))
        .row()
        .items_center()
        .justify_between()
        .gap(px(tokens::SM))
        .child(div().flex_none().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t("Screen recording")))
        .child(
            div()
                .min_w(px(0.0))
                .text_size(px(text::LABEL))
                .truncate()
                .text_color(if info.is_ok() { palette.on_surface_variant.hsla() } else { palette.urgent_coral.hsla() })
                .child(match info {
                    Ok(s) | Err(s) => s,
                }),
        );
    blocks.push(reveal(p, first, title));
    height += SECTION_H_PX + gap;

    let Some(rec) = recorder::recorder(cx) else { return (blocks, height) };
    let (left, busy, active) = {
        let r = rec.read(cx);
        let active = r.active.as_ref().map(|s| (r.clock().unwrap_or_default(), s.is_paused(), s.pause_reason(), s.plan.audio.mic(), r.mic_muted));
        (r.countdown_left(), r.busy, active)
    };
    let mut row = div().row().gap(px(tokens::SM));
    if let Some(left) = left {
        row = row.child(toggle_tile("rec-cancel", "close", t("Cancel"), Some(tf("Starts in {seconds} s", &[("seconds", &left)])), true, palette, |_, cx| {
            let _ = recorder::stop(cx);
        }));
    } else if let Some((clock, paused, pause_reason, mic, mic_muted)) = active.clone() {
        row = row.child(toggle_tile("rec-stop", "stop", t("Stop recording"), Some(clock), true, palette, |_, cx| {
            shell(cx).update(cx, |s, cx| s.close_panel_now(cx));
            let _ = recorder::stop(cx);
        }));
        row = match pause_reason {
            Some(reason) => row.child(locked_tile("rec-pause", "pause", t("Pause").into(), tr(reason), palette)),
            None => row.child(toggle_tile(
                "rec-pause",
                if paused { "play" } else { "pause" },
                if paused { t("Resume") } else { t("Pause") },
                Some(if paused { t("Paused").into() } else { t("Recording").into() }),
                paused,
                palette,
                |_, cx| {
                    let _ = recorder::set_paused(None, cx);
                },
            )),
        };
        if mic {
            row = row.child(toggle_tile(
                "rec-mic",
                if mic_muted { "mic-off" } else { "mic" },
                if mic_muted { t("Unmute mic") } else { t("Mute mic") },
                Some(if mic_muted { t("Muted").into() } else { t("On").into() }),
                mic_muted,
                palette,
                |_, cx| {
                    let _ = recorder::set_mic_muted(None, cx);
                },
            ));
        }
    } else if busy {
        row = row.child(locked_tile("rec-busy", "timer", t("Working…").into(), t("Starting or saving the recording").into(), palette));
    } else {
        let start = |source: Source| {
            move |_: &mut gpui::Window, cx: &mut App| {
                let panel_was_open = shell(cx).update(cx, |s, cx| s.close_panel_now(cx));
                let lead_in = if panel_was_open { RECORD_LEAD_IN } else { Duration::ZERO };
                if let Err(err) = recorder::start(source.clone(), lead_in, cx) {
                    crate::snip::report(Err(err), cx);
                }
            }
        };
        row = row
            .child(toggle_tile("rec-screen", "monitor", t("Screen"), Some(display_label(cx)), false, palette, start(Source::Screen(None))))
            .child(toggle_tile("rec-region", "crop", t("Region"), Some(t("Select an area").into()), false, palette, start(Source::Region)))
            .child(toggle_tile("rec-window", "window", t("Window"), Some(t("Focused window").into()), false, palette, start(Source::Window)));
    }
    blocks.push(reveal(p, first + 1, row));
    height += metrics::TILE_H + gap;

    // Several displays: which one "Record screen" takes.
    let outputs = crate::outputs::geometries(cx);
    if outputs.len() > 1 && left.is_none() && active.is_none() && !busy {
        let chosen = recorder::settings(cx).monitor;
        let chip = |id: String, label: String, value: String, on: bool| {
            let fg = if on { palette.on_primary } else { palette.on_surface };
            div()
                .id(gpui::SharedString::from(id))
                .h(px(CHIP_H_PX))
                .px(px(tokens::MD))
                .rounded_full()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(text::LABEL))
                .text_color(fg.hsla())
                .bg(if on { palette.primary.hsla() } else { palette.surface_container_high.hsla() })
                .hover(move |s| if on { s } else { s.bg(palette_hover(fg)) })
                .child(label)
                .on_click(move |_, _, cx| {
                    let value = value.clone();
                    update_config(cx, move |c| {
                        c.plugins.settings.entry(super::ID.to_string()).or_default().insert("record_monitor".into(), toml::Value::String(value));
                    });
                })
        };
        let mut chips = div().row().flex_wrap().gap(px(tokens::XS)).child(chip("rec-out-focused".into(), t("Focused display").into(), String::new(), chosen.trim().is_empty()));
        for o in &outputs {
            chips = chips.child(chip(format!("rec-out-{}", o.name), o.name.clone(), o.name.clone(), chosen == o.name));
        }
        blocks.push(reveal(p, first + 2, chips));
        height += CHIP_H_PX + gap;
    }
    (blocks, height)
}

fn palette_hover(fg: mandara_theme::Color) -> gpui::Hsla {
    fg.with_alpha(alpha::HOVER).hsla()
}

/// `1:05 · 12.3 MB`.
fn recent_detail(r: &Recent) -> String {
    tf("{duration} · {size} MB", &[("duration", &format_elapsed(r.duration())), ("size", &format!("{:.1}", r.bytes as f64 / 1_048_576.0))])
}

/// The recent recordings: open (click), show in folder, move to the Trash.
pub(super) fn recent(p: &PanelCx, index: usize, palette: &MandaraPalette, cx: &mut App) -> Option<(AnyElement, f32)> {
    let list = recorder::recent(cx)?;
    if list.is_empty() {
        return None;
    }
    let shown: Vec<Recent> = list.into_iter().take(RECENT_SHOWN).collect();
    let height = SECTION_H_PX + tokens::XS + shown.len() as f32 * metrics::ROW_H + tokens::MD;
    let mut col = div().flex().flex_col().gap(px(tokens::XS)).child(div().h(px(SECTION_H_PX)).flex().items_center().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t("Recent recordings")));
    let mut rows = div().flex().flex_col();
    for (i, r) in shown.into_iter().enumerate() {
        let (open, folder, trash) = (r.path.clone(), r.path.clone(), r.path.clone());
        let buttons = div()
            .row()
            .gap(px(tokens::XS))
            // The row opens the file: a button's click stops there.
            .child(icon_button(("rec-folder", i), "folder", ROW_BUTTON_PX, palette.on_surface_variant, None, move |_, cx| {
                cx.stop_propagation();
                recorder::reveal(&folder);
            }))
            .child(icon_button(("rec-trash", i), "trash", ROW_BUTTON_PX, palette.on_surface_variant, None, move |_, cx| {
                cx.stop_propagation();
                recorder::trash(trash.clone(), cx);
            }))
            .into_any_element();
        rows = rows.child(list_row(("rec-recent", i), icon("play", metrics::TILE_ICON, palette.primary), r.name(), Some(recent_detail(&r)), Some(buttons), false, palette, move |_, _| recorder::open(&open)));
    }
    col = col.child(rows);
    Some((reveal(p, index, col), height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_detail_reads_length_and_size() {
        let r = Recent { path: "/v/a.mp4".into(), secs: 65.4, bytes: 12 * 1_048_576 + 314_573, at: 0 };
        assert_eq!(recent_detail(&r), "1:05 · 12.3 MB");
    }
}
