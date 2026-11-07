//! The opt-in command agent's downloads and non-executing test bench.

use super::{
    StudioView,
    compositor_page::{badge, button, card, heading},
};
use crate::{
    registry::{self, Section},
    tokens::*,
};
use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::command_agent::{self as agent, Event, Job, Request, models};
use mandara_ui::{
    bidi::Bidi as _,
    progress_fill::{self as fill, Observed, ProgressFill},
    state::config,
    style::ClipRounded as _,
    text_field::TextField,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{rc::Rc, time::Instant};

#[derive(Clone, Copy)]
enum Action {
    Required,
    Download(usize),
    Delete(usize),
    Pause,
    Cancel,
    Purge,
}

enum Download {
    Progress(usize, u64, u64),
    Done(usize),
    Failed(usize, String),
    Stopped(usize),
}
struct DownloadJob {
    job: Job,
    discard: Arc<AtomicBool>,
}

pub(super) struct AgentPage {
    input: Entity<TextField>,
    job: Option<Job>,
    task: Option<gpui::Task<()>>,
    download: Option<DownloadJob>,
    download_task: Option<gpui::Task<()>>,
    installed: [bool; 3],
    fills: [ProgressFill; 3],
    observed: [Observed; 3],
    bytes: [(u64, u64); 3],
    matches: Vec<agent::Match>,
    preview: String,
    error: Option<String>,
    active: bool,
    visibility: Option<gpui::Subscription>,
}

impl AgentPage {
    /// Create the bench input and query model metadata away from the UI thread.
    fn new(cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        let input = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Try a shell command…"),
                false,
                Rc::new(move |query, _, cx| {
                    let _ = weak.update(cx, |this: &mut Self, cx| this.test(query, cx));
                }),
                cx,
            )
        });
        let this = Self {
            input,
            job: None,
            task: None,
            download: None,
            download_task: None,
            installed: [false; 3],
            fills: std::array::from_fn(|_| ProgressFill::default()),
            observed: [Observed::Idle; 3],
            bytes: [(0, 0); 3],
            matches: Vec::new(),
            preview: String::new(),
            error: None,
            active: false,
            visibility: None,
        };
        cx.spawn(async move |this, cx| {
            let status = cx.background_executor().spawn(async { models::ALL.map(|m| m.installed()) }).await;
            let _ = this.update(cx, |this, cx| {
                this.installed = status;
                cx.notify();
            });
        })
        .detach();
        this
    }

    /// Cancel this bench request and release its displayed matches and payload.
    fn purge(&mut self, cx: &mut Context<Self>) {
        self.job = None;
        self.task = None;
        self.active = false;
        self.matches.clear();
        self.preview.clear();
        mandara_services::memory::request_trim();
        cx.notify();
    }

    /// Compile a bench prompt without dispatching any of its resulting actions.
    fn test(&mut self, input: String, cx: &mut Context<Self>) {
        self.purge(cx);
        self.error = None;
        let query = agent::query(&input).unwrap_or(input.trim()).to_owned();
        if query.is_empty() {
            return;
        }
        if query.len() > agent::MAX_QUERY_BYTES {
            self.error = Some(t("This command is too long").into());
            return;
        }
        let engine = match config(cx).read(cx).get().launcher.agent.engine {
            mandara_config::agent::AgentEngine::FunctionGemma => agent::Engine::FunctionGemma,
            mandara_config::agent::AgentEngine::Qwen => agent::Engine::Qwen,
        };
        let catalog = match serde_json::from_value(cx.global::<mandara_plugins::PluginRegistry>().commands.catalog_json()) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        };
        let job = Job::default();
        let cancel = job.cancellation();
        self.job = Some(job);
        self.active = true;
        self.task = Some(cx.spawn(async move |this, cx| {
            let (tx, rx) = async_channel::bounded(2);
            let work = cx.background_executor().spawn(async move {
                if let Err(error) = agent::run(Request { query, catalog, engine, prepare_index: false }, cancel, tx.clone()) {
                    let _ = tx.try_send(Event::Error { message: error.to_string() });
                }
            });
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match event {
                            Event::Matches { matches, retrieval_ms } => {
                                this.matches = matches;
                                this.preview = tf("Retrieved in {time} ms", &[("time", &format!("{retrieval_ms:.1}"))]);
                            }
                            Event::Ready { actions, inference_ms, .. } => {
                                this.preview = format!(
                                    "{}\n{}",
                                    tf("Compiled in {time} ms", &[("time", &format!("{inference_ms:.0}"))]),
                                    serde_json::to_string_pretty(&actions).unwrap_or_default()
                                );
                            }
                            Event::Error { message } => {
                                this.error = Some(message);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            work.await;
            let _ = this.update(cx, |this, cx| {
                this.active = false;
                this.job = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Process one owned model queue and coalesce byte updates into bounded events.
    fn download(&mut self, queue: Vec<usize>, cx: &mut Context<Self>) {
        if self.download_task.is_some() {
            return;
        }
        self.error = None;
        let job = Job::default();
        let cancel = job.cancellation();
        let discard = Arc::new(AtomicBool::new(false));
        self.download = Some(DownloadJob { job, discard: discard.clone() });
        for &i in &queue {
            self.observed[i] = Observed::Working;
        }
        let catalog = serde_json::from_value(cx.global::<mandara_plugins::PluginRegistry>().commands.catalog_json()).unwrap_or_default();
        self.download_task = Some(cx.spawn(async move |this, cx| {
            let (tx, rx) = async_channel::bounded(8);
            let work = cx.background_executor().spawn(async move {
                let mut completed = true;
                for &i in &queue {
                    let result = models::download(models::ALL[i], &cancel, |done, total| {
                        let _ = tx.try_send(Download::Progress(i, done, total));
                    });
                    match result {
                        Ok(()) => {
                            if tx.send_blocking(Download::Done(i)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            completed = false;
                            let _ =
                                tx.send_blocking(if cancel.load(Ordering::Acquire) { Download::Stopped(i) } else { Download::Failed(i, error.to_string()) });
                            break;
                        }
                    }
                }
                if completed && models::ROUTER.installed() {
                    let (events, replies) = async_channel::bounded(2);
                    let result =
                        agent::run(Request { query: String::new(), catalog, engine: agent::Engine::FunctionGemma, prepare_index: true }, cancel, events);
                    if let Err(e) = result {
                        let _ = tx.send_blocking(Download::Failed(0, e.to_string()));
                    }
                    if let Ok(Event::Error { message }) = replies.try_recv() {
                        let _ = tx.send_blocking(Download::Failed(0, message));
                    }
                }
                if discard.load(Ordering::Acquire) {
                    for i in queue {
                        for file in models::ALL[i].files {
                            if let Err(error) = std::fs::remove_file(models::ALL[i].file(&format!("{}.part", file.name)))
                                && error.kind() != std::io::ErrorKind::NotFound
                            {
                                let _ = tx.send_blocking(Download::Failed(i, error.to_string()));
                            }
                        }
                    }
                }
            });
            while let Ok(event) = rx.recv().await {
                if this
                    .update(cx, |this, cx| {
                        match event {
                            Download::Progress(i, done, total) => {
                                this.bytes[i] = (done, total);
                                this.observed[i] = Observed::Progress { done, total };
                            }
                            Download::Stopped(i) => {
                                this.observed[i] = Observed::Idle;
                            }
                            Download::Done(i) => {
                                this.installed[i] = true;
                                this.observed[i] = Observed::Done;
                            }
                            Download::Failed(i, error) => {
                                this.error = Some(error);
                                this.observed[i] = Observed::Failed;
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
            work.await;
            let _ = this.update(cx, |this, cx| {
                this.download = None;
                this.download_task = None;
                for state in &mut this.observed {
                    if matches!(state, Observed::Working | Observed::Progress { .. }) {
                        *state = Observed::Idle;
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Route page controls through the owned download and inference jobs.
    fn perform(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Required => {
                let selected = config(cx).read(cx).get().launcher.agent.engine;
                self.download(vec![0, if selected == mandara_config::agent::AgentEngine::Qwen { 2 } else { 1 }], cx);
            }
            Action::Download(i) => self.download(vec![i], cx),
            Action::Delete(i) => self.delete(i, cx),
            Action::Pause => {
                self.download = None;
                cx.notify();
            }
            Action::Cancel => {
                if let Some(job) = self.download.take() {
                    job.discard.store(true, Ordering::Release);
                    drop(job.job);
                }
                cx.notify();
            }
            Action::Purge => {
                self.purge(cx);
                agent::purge();
                if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
                    cx.background_executor()
                        .spawn(async move {
                            if let Err(error) = (ipc.0)(vec!["agent".into(), "purge".into()]) {
                                tracing::warn!("agent purge: {error}");
                            }
                        })
                        .detach();
                }
            }
        }
    }

    /// Give mouse and keyboard activation the same page action and focus styling.
    fn action_button(
        id: impl Into<SharedString>,
        glyph: &'static str,
        label: impl Into<SharedString>,
        filled: bool,
        action: Action,
        p: &mandara_theme::MandaraPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = p.primary_container.hsla();
        button(id, glyph, label, filled, p)
            .tab_index(0)
            .focus_visible(move |style| style.bg(focus))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.perform(action, cx)))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.keystroke.modifiers.modified() {
                    this.perform(action, cx);
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }

    /// Remove an installed model off the UI thread after bench work is cancelled.
    fn delete(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.download_task.is_some() {
            self.error = Some(t("Pause downloads before deleting models").into());
            cx.notify();
            return;
        }
        self.purge(cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { models::ALL[i].remove() }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.installed[i] = false;
                        this.observed[i] = Observed::Idle;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for AgentPage {
    /// Draw model cards with the shared progress fill and cancel work when hidden.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.visibility.is_none() {
            let weak = cx.weak_entity();
            self.visibility = Some(window.observe_window_visibility(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    if !window.is_visible() {
                        this.purge(cx);
                        this.download = None;
                        this.download_task = None;
                        this.observed = [Observed::Idle; 3];
                    } else {
                        cx.notify();
                    }
                });
            }));
        }
        let p = mandara_core::frame_palette(window, cx);
        let cfg = config(cx).read(cx).snapshot();
        let count = cx.global::<mandara_plugins::PluginRegistry>().commands.all().len();
        let status = if self.download.is_some() {
            t("Downloading…")
        } else if self.active {
            t("Compiling locally…")
        } else {
            t("Idle — no inference worker")
        };
        let mut controls = div().row().flex_wrap().gap(px(SPACE_8_PX)).child(Self::action_button(
            "agent-download-required",
            "download",
            t("Download all required models"),
            true,
            Action::Required,
            &p,
            cx,
        ));
        if self.download.is_some() {
            controls = controls
                .child(Self::action_button("agent-pause", "pause", t("Pause downloads"), false, Action::Pause, &p, cx))
                .child(Self::action_button("agent-cancel", "close", t("Cancel download"), false, Action::Cancel, &p, cx));
        }
        let mut body = div()
            .row()
            .flex_col()
            .gap(px(SPACE_16_PX))
            .child(
                card(&p)
                    .child(heading(
                        t("Local commands, on demand"),
                        t("Models run offline on your CPU. Closing the launcher, removing !, or 15 seconds without a new query releases the inference worker."),
                        &p,
                    ))
                    .child(
                        div()
                            .row()
                            .flex_wrap()
                            .gap(px(SPACE_8_PX))
                            .child(badge(status, p.primary))
                            .child(badge(tf("{count} available commands", &[("count", &count)]), p.on_surface_variant)),
                    ),
            )
            .child(controls);
        for (i, model) in models::ALL.into_iter().enumerate() {
            let now = Instant::now();
            self.fills[i].observe(self.observed[i], now);
            let visual = self.fills[i].frame(cfg.motion.reduce_motion, now);
            if visual.animating {
                window.request_animation_frame();
            }
            if let Some(after) = visual.wake_in {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(after).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
            let (done, total) = self.bytes[i];
            let status = if total > 0 {
                format!("{} / {}", mandara_services::self_update::human_size(done), mandara_services::self_update::human_size(total))
            } else {
                mandara_services::self_update::human_size(model.download_size())
            };
            body = body.child(
                card(&p).relative().clip_rounded().children(fill::layer(&visual, mandara_ui::bidi::rtl(), &p)).child(
                    div().relative().row().flex_col().gap(px(SPACE_8_PX)).child(heading(model.title, format!("{} · {}", status, model.license), &p)).child(
                        div()
                            .row()
                            .flex_wrap()
                            .gap(px(SPACE_8_PX))
                            .child(Self::action_button(
                                SharedString::from(format!("agent-download-{i}")),
                                "download",
                                if self.installed[i] { t("Verify / resume") } else { t("Download") },
                                false,
                                Action::Download(i),
                                &p,
                                cx,
                            ))
                            .child(Self::action_button(
                                SharedString::from(format!("agent-delete-{i}")),
                                "trash",
                                t("Delete / reclaim disk space"),
                                false,
                                Action::Delete(i),
                                &p,
                                cx,
                            )),
                    ),
                ),
            );
        }
        body = body.child(
            card(&p)
                .child(heading(t("Test bench"), t("Enter a prompt to inspect retrieval and the exact payload. Tests never execute commands."), &p))
                .child(self.input.clone())
                .child(div().row().child(Self::action_button("agent-purge", "close", t("Purge agent from RAM now"), false, Action::Purge, &p, cx)))
                .children(self.matches.iter().map(|m| div().text_size(px(TEXT_12_PX)).child(format!("{} · {:.3}", m.command.path, m.score))))
                .child(div().text_size(px(TEXT_12_PX)).child(self.preview.clone())),
        );
        if let Some(error) = &self.error {
            body = body.child(div().text_color(p.urgent_coral.hsla()).child(error.clone()));
        }
        body
    }
}

impl StudioView {
    /// Create the page lazily and prepend its declarative settings controls.
    pub(super) fn agent_page(&mut self, cfg: &mandara_config::ShellConfig, p: &mandara_theme::MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows(registry::in_section(Section::AiShellAgent), cfg, p, cx);
        let page = self.agent_page.get_or_insert_with(|| cx.new(AgentPage::new)).clone();
        div().row().flex_col().gap(px(SPACE_16_PX)).child(rows).child(page).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn required_downloads_have_a_router_and_one_compiler() {
        assert_eq!(models::ALL[0].id, models::ROUTER.id);
        assert_eq!(models::ALL[1].id, models::FUNCTION_GEMMA.id);
        assert_eq!(models::ALL[2].id, models::QWEN.id);
    }
}
