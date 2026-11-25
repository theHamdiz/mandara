//! The depth model row (Studio → Wallpaper → Depth, and the Wallpaper
//! depth plugin's settings): whether the model is installed, a Download
//! button, live progress while the shell downloads it, the error with Retry
//! when it fails, and a nudge to pick Model once it is there.
//!
//! The shell owns the download (`depth download`; picking Model starts one
//! too), so the row reads `depth status` — once when it first shows, then
//! every [`POLL`] only while a download runs. Without the shell (tests,
//! embedding) it downloads in Studio itself.

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Task, div, px};
use mandara_config::{DepthMode, ShellConfig};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf};
use mandara_services::{depth_model, models};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use serde_json::Value;

use super::StudioView;
use crate::tokens::*;

/// How often a running download's progress is read.
const POLL: Duration = Duration::from_millis(250);
/// The progress track's width.
const BAR_W_PX: f32 = 150.0;
const BAR_H_PX: f32 = 6.0;
const BYTES_PER_MB: f64 = 1_000_000.0;

#[derive(Clone, Debug, Default, PartialEq)]
enum Phase {
    /// Not asked yet.
    #[default]
    Unknown,
    Idle,
    Running {
        done: u64,
    },
    Failed(String),
}

/// The row's state, kept on the Studio view.
#[derive(Default)]
pub struct DepthModelRow {
    phase: Phase,
    /// The status poll while a download runs (dropping it stops polling).
    poll: Option<Task<()>>,
}

/// What `depth status` says about the download.
fn phase_of(status: &Value) -> Phase {
    let model = &status["model"];
    match &model["download"] {
        Value::String(s) if s == "running" => Phase::Running { done: model["downloaded"].as_u64().unwrap_or(0) },
        Value::Object(o) => Phase::Failed(o.get("failed").and_then(Value::as_str).unwrap_or_default().to_string()),
        _ => Phase::Idle,
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / BYTES_PER_MB)
}

impl StudioView {
    /// Reads the shell's download state now, and keeps polling while a
    /// download runs.
    fn depth_model_poll(&mut self, cx: &mut Context<Self>) {
        let Some(first) = crate::perf::query(&["depth", "status"], cx) else {
            return;
        };
        self.depth_model.poll = Some(cx.spawn(async move |this, cx| {
            let mut next = Some(first);
            while let Some(task) = next.take() {
                let phase = task.await.map(|s| phase_of(&s)).unwrap_or(Phase::Idle);
                let running = matches!(phase, Phase::Running { .. });
                let alive = this
                    .update(cx, |this, cx| {
                        this.depth_model.phase = phase;
                        cx.notify();
                    })
                    .is_ok();
                if !alive || !running {
                    break;
                }
                cx.background_executor().timer(POLL).await;
                next = this.update(cx, |_, cx| crate::perf::query(&["depth", "status"], cx)).ok().flatten();
            }
            let _ = this.update(cx, |this, _| this.depth_model.poll = None);
        }));
    }

    /// Starts the download (in the shell, or here without one).
    fn depth_model_download(&mut self, cx: &mut Context<Self>) {
        self.depth_model.phase = Phase::Running { done: 0 };
        cx.notify();
        if let Some(task) = crate::perf::query(&["depth", "download"], cx) {
            // The reply comes when the download ends; progress and errors
            // come from polling `depth status` meanwhile.
            task.detach();
            self.depth_model_poll(cx);
            return;
        }
        let done = Arc::new(AtomicU64::new(0));
        let result: Arc<Mutex<Option<Result<(), String>>>> = Arc::default();
        let (progress, outcome) = (done.clone(), result.clone());
        cx.background_executor()
            .spawn(async move {
                let r = models::find(depth_model::MODEL_ID)
                    .ok_or_else(|| t("unknown model").to_string())
                    .and_then(|model| model.download(&|bytes, _| progress.store(bytes, Ordering::Relaxed)));
                *outcome.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(r);
            })
            .detach();
        self.depth_model.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let finished = result.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
                let phase = match finished {
                    None => Phase::Running { done: done.load(Ordering::Relaxed) },
                    Some(Ok(())) => Phase::Idle,
                    Some(Err(e)) => Phase::Failed(e),
                };
                let running = matches!(phase, Phase::Running { .. });
                let alive = this
                    .update(cx, |this, cx| {
                        this.depth_model.phase = phase;
                        if !running {
                            this.depth_model.poll = None;
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive || !running {
                    break;
                }
            }
        }));
    }

    /// The row's control: status, progress, errors and the next step.
    pub(super) fn depth_model_control(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if self.depth_model.phase == Phase::Unknown && self.depth_model.poll.is_none() {
            // A download may already run (picking Model starts one).
            self.depth_model.phase = Phase::Idle;
            self.depth_model_poll(cx);
        }
        let total = models::find(depth_model::MODEL_ID).map_or(depth_model::SIZE, |m| m.download_size()).max(1);
        let pill = |label: String, color: mandara_theme::Color| {
            div()
                .px(px(SPACE_9_PX))
                .h(px(DIM_22_PX))
                .rounded_full()
                .row()
                .items_center()
                .flex_none()
                .bg(color.with_alpha(ALPHA_14).hsla())
                .text_color(color.hsla())
                .text_size(px(TEXT_11_5_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .h(px(DIM_32_PX))
                .px(px(SPACE_16_PX))
                .rounded_full()
                .row()
                .items_center()
                .flex_none()
                .cursor_pointer()
                .bg(p.primary.with_alpha(ALPHA_14).hsla())
                .text_color(p.primary.hsla())
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_22).hsla()))
                .child(mandara_ui::bidi::paragraph(t(label)))
        };
        let row = div().row().items_center().gap(px(SPACE_9_PX));
        match self.depth_model.phase.clone() {
            Phase::Running { done } => {
                let share = (done as f32 / total as f32).clamp(0.0, 1.0);
                row.child(
                    div()
                        .row()
                        .flex_col()
                        .gap(px(SPACE_4_PX))
                        .child(
                            div()
                                .w(px(BAR_W_PX))
                                .h(px(BAR_H_PX))
                                .rounded_full()
                                .bg(p.surface_container_high.hsla())
                                .child(div().h_full().w(px(BAR_W_PX * share)).rounded_full().bg(mandara_core::oklab_gradient(90.0, p.primary, p.tertiary))),
                        )
                        .child(
                            div()
                                .text_size(px(TEXT_11_5_PX))
                                .text_color(p.on_surface_variant.hsla())
                                .child(tf("Downloading… {done} of {total} MB", &[("done", &megabytes(done)), ("total", &megabytes(total))])),
                        ),
                )
                .into_any_element()
            }
            Phase::Failed(why) => row
                .child(
                    div()
                        .max_w(px(BAR_W_PX * 1.6))
                        .text_size(px(TEXT_11_5_PX))
                        .text_color(p.urgent_coral.hsla())
                        .child(mandara_ui::bidi::paragraph(tf("Download failed: {error}", &[("error", &why)]))),
                )
                .child(button("depth-model-retry", "Retry").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.depth_model_download(cx))))
                .into_any_element(),
            Phase::Idle | Phase::Unknown if depth_model::installed() => {
                let row = row.child(pill(t("Installed").to_string(), p.success_emerald));
                if cfg.wallpaper.depth.mode == DepthMode::Model {
                    row.into_any_element()
                } else {
                    row.child(button("depth-model-use", "Use the model").on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        if let Some(s) = crate::registry::by_path("/wallpaper/depth/mode") {
                            this.apply(s, Value::from("model"), cx);
                        }
                    })))
                    .into_any_element()
                }
            }
            Phase::Idle | Phase::Unknown => row
                .child(pill(tf("{size} MB", &[("size", &megabytes(total))]), p.on_surface_variant))
                .child(button("depth-model-download", "Download").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.depth_model_download(cx))))
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_shell_download_state() {
        assert_eq!(phase_of(&json!({ "model": { "download": "running", "downloaded": 1200 } })), Phase::Running { done: 1200 });
        assert_eq!(phase_of(&json!({ "model": { "download": { "failed": "offline" } } })), Phase::Failed("offline".into()));
        assert_eq!(phase_of(&json!({ "model": { "download": "idle" } })), Phase::Idle);
        assert_eq!(phase_of(&json!({})), Phase::Idle);
        assert_eq!(megabytes(27_258_801), "27.3");
    }
}
