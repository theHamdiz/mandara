//! Studio → About → "Mandara updates": the shell's update card. Binary
//! patches are the main update method (a few MB instead of the whole
//! release); the full download is the automatic fallback. Logic in
//! `mandara_services::self_update`; plugin updates in [`super::plugin_updates`].

use mandara_ui::bidi::Bidi as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::self_update::{self as su, ApplyOptions, ApplyReport, Install, Method, Progress, UpdateInfo, install, plugins};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;

use super::StudioView;
use super::compositor_page::button;
use crate::registry::{self, Section};
use crate::tokens::*;

/// Repaint interval while an update runs.
pub(super) const JOB_REPAINT: Duration = Duration::from_millis(150);
/// Background angle of the card's wash (matches the About hero).
const WASH_ANGLE: f32 = 135.0;

/// A running update's progress, shared with its worker thread.
#[derive(Default)]
pub(super) struct Job {
    pub(super) step: String,
    pub(super) done: u64,
    pub(super) total: u64,
    /// The new binary is in place (only the language packs remain).
    pub(super) installed: bool,
    pub(super) result: Option<Result<ApplyReport, String>>,
}

pub(super) type JobHandle = Arc<Mutex<Job>>;

/// Card state.
#[derive(Default)]
pub struct Updates {
    primed: bool,
    pub(super) info: Option<UpdateInfo>,
    pub(super) install: Option<Install>,
    /// The installed binary's size (what the update replaces), for the
    /// "N MB smaller / larger" line.
    pub(super) binary_size: Option<u64>,
    pub(super) checking: bool,
    error: Option<String>,
    job: Option<JobHandle>,
    outcome: Option<Result<ApplyReport, String>>,
    pub(super) plugins: Vec<plugins::PluginUpdate>,
    pub(super) plugin_jobs: std::collections::HashMap<String, JobHandle>,
    pub(super) plugin_message: Option<(bool, String)>,
    /// About → What's new: the release shown open (the running one first).
    pub(super) whats_new: Option<String>,
    /// The card's own background follows the update (download, apply,
    /// failure, success).
    fill: mandara_ui::progress_fill::ProgressFill,
}

pub(super) fn lock(job: &JobHandle) -> std::sync::MutexGuard<'_, Job> {
    job.lock().unwrap_or_else(|e| e.into_inner())
}

/// A progress callback writing into `job`.
pub(super) fn reporter(job: JobHandle) -> impl Fn(Progress) {
    move |p| {
        let mut j = lock(&job);
        match p {
            Progress::Step(s) => j.step = s,
            Progress::Bytes { done, total } => (j.done, j.total) = (done, total),
        }
    }
}

/// "Updated to 0.2.0 with a binary patch: 4.2 MB instead of 38 MB".
pub(super) fn describe(r: &ApplyReport) -> String {
    let sizes = if r.full_size > r.downloaded { tf("{downloaded} instead of {full}", &[("downloaded", &su::human_size(r.downloaded)), ("full", &su::human_size(r.full_size))]) } else { su::human_size(r.downloaded) };
    let mut s = match r.method {
        Method::Delta => tf("Updated to {version} with a binary patch: {sizes}", &[("version", &r.version), ("sizes", &sizes)]),
        Method::Full => tf("Updated to {version} with the full download: {sizes}", &[("version", &r.version), ("sizes", &sizes)]),
    };
    if let Some(reason) = &r.fallback {
        s.push_str(" · ");
        s.push_str(&tf("the patch couldn't be used ({reason})", &[("reason", reason)]));
    }
    s
}

/// A thin progress bar.
pub(super) fn progress_bar(done: u64, total: u64, p: &MandaraPalette) -> AnyElement {
    let fraction = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    div()
        .h(px(DIM_6_PX))
        .w_full()
        .rounded_full()
        .bg(p.surface_container_high.hsla())
        .child(div().h_full().rounded_full().bg(p.primary.hsla()).w(gpui::relative(fraction)))
        .into_any_element()
}

/// Polls `job` until it has a result, repainting, then hands it to `done`.
pub(super) fn watch_job(job: JobHandle, cx: &mut Context<StudioView>, done: impl FnOnce(&mut StudioView, Result<ApplyReport, String>, &mut Context<StudioView>) + 'static) {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(JOB_REPAINT).await;
            let result = lock(&job).result.take();
            let alive = this.update(cx, |_, cx| cx.notify()).is_ok();
            if let Some(result) = result {
                let _ = this.update(cx, |view, cx| {
                    done(view, result, cx);
                    cx.notify();
                });
                return;
            }
            if !alive {
                return;
            }
        }
    })
    .detach();
}

/// The login screen's language, when Mandara Login is set up.
pub(super) fn greeter_language() -> Option<mandara_i18n::Lang> {
    mandara_greeter::config::GreeterConfig::load_from(std::path::Path::new(mandara_greeter::config::SYSTEM_CONFIG)).map(|c| c.lang())
}

/// Restarts the running shell when it runs the binary just replaced.
fn restart_shell_if_running(exe: &std::path::Path, cx: &App) {
    if !install::running_from(exe) {
        return;
    }
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        std::thread::spawn(move || {
            if let Err(err) = (ipc.0)(vec!["restart".into()]) {
                tracing::warn!("update: restarting the shell failed: {err}");
            }
        });
    }
}

impl StudioView {
    /// Loads the last answers (no network), detects the install, and runs
    /// an automatic check when the cached one is older than the interval.
    pub(super) fn updates_prime(&mut self, cx: &mut Context<Self>) {
        if self.updates.primed {
            return;
        }
        self.updates.primed = true;
        self.updates.whats_new = super::whats_new::current_version();
        self.updates.info = su::cached();
        self.updates.plugins = plugins::cached();
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot().updates.clone();
        let task = cx.background_executor().spawn(async move {
            let install = install::detect();
            let size = su::binary_size(install.updatable(false).ok());
            (install, size)
        });
        cx.spawn(async move |this, cx| {
            let (install, size) = task.await;
            let _ = this.update(cx, |view, cx| {
                view.updates.install = Some(install);
                view.updates.binary_size = size;
                cx.notify();
            });
        })
        .detach();
        if cfg.self_check && !mandara_services::offline::is_offline() {
            self.updates_check(false, cx);
        }
    }

    /// Checks the shell (and, when enabled, marketplace plugins).
    pub(super) fn updates_check(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        self.updates.error = None;
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot().updates.clone();
        let interval = cfg.interval();
        let with_plugins = cfg.plugins;
        let task = cx.background_executor().spawn(async move { (su::check(force, interval), if with_plugins { Some(plugins::check_all(force, interval)) } else { None }) });
        cx.spawn(async move |this, cx| {
            let (shell, plugin_result) = task.await;
            let _ = this.update(cx, |view, cx| {
                view.updates.checking = false;
                match shell {
                    Ok(info) => view.updates.info = Some(info),
                    Err(e) => view.updates.error = Some(e),
                }
                match plugin_result {
                    Some(Ok(list)) => view.updates.plugins = list,
                    Some(Err(e)) => view.updates.plugin_message = Some((false, e)),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Installs the update: binary patch first (unless `full_only`), the
    /// full download on any failure; then restarts the shell.
    fn updates_apply(&mut self, full_only: bool, cx: &mut Context<Self>) {
        let (Some(info), Some(install)) = (self.updates.info.clone(), self.updates.install.clone()) else { return };
        if self.updates.job.is_some() {
            return;
        }
        let exe = match install.updatable(false) {
            Ok(p) => p.to_path_buf(),
            Err(e) => {
                self.updates.outcome = Some(Err(e));
                return;
            }
        };
        let job: JobHandle = Arc::default();
        lock(&job).step = t("Preparing…").to_string();
        self.updates.job = Some(job.clone());
        self.updates.outcome = None;
        let worker = job.clone();
        let target = exe.clone();
        // The new version starts translated: its packs for every language
        // in use download before the restart (a failure only means it
        // downloads them when it starts).
        let languages = su::langpacks::languages_in_use(mandara_ui::state::config(cx).read(cx).get(), greeter_language());
        std::thread::spawn(move || {
            let progress = reporter(worker.clone());
            let result = su::apply(&info, &target, ApplyOptions { full_only, dry_run: false }, &progress);
            if result.is_ok() {
                // The binary is in: the card stays full while the packs
                // (a few hundred KB) come down, saying what it does.
                let packs_progress = |p: Progress| {
                    if let Progress::Step(s) = p {
                        let mut j = lock(&worker);
                        (j.step, j.done, j.total, j.installed) = (s, 0, 0, true);
                    }
                };
                let packs = su::langpacks::prepare_update(&info.latest, &languages, &packs_progress);
                for (lang, e) in &packs.failed {
                    tracing::warn!("update: {} language pack for {}: {e}", lang.code(), info.latest);
                }
            }
            lock(&worker).result = Some(result);
        });
        watch_job(job, cx, move |view, result, cx| {
            view.updates.job = None;
            if result.is_ok() {
                // The card shows its success first, then the shell restarts.
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(mandara_ui::progress_fill::SUCCESS_HOLD).await;
                    cx.update(|cx| restart_shell_if_running(&exe, cx));
                })
                .detach();
            }
            view.updates.outcome = Some(result);
        });
        cx.notify();
    }

    /// The card (About page).
    pub(super) fn updates_card(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut gpui::Window, cx: &mut Context<Self>) -> AnyElement {
        use mandara_ui::progress_fill::{self as fill, Observed, Tone};
        self.updates_prime(cx);
        // The card fills with the real download, then stays full while the
        // update is applied and verified; red on failure, green on success.
        let now = std::time::Instant::now();
        let observed = match (&self.updates.job, &self.updates.outcome) {
            (Some(j), _) => {
                let j = lock(j);
                if j.installed || (j.total > 0 && j.done >= j.total) { Observed::Working } else { Observed::Progress { done: j.done, total: j.total } }
            }
            (None, Some(Ok(_))) => Observed::Done,
            (None, Some(Err(_))) => Observed::Failed,
            (None, None) => Observed::Idle,
        };
        self.updates.fill.observe(observed, now);
        let visual = self.updates.fill.frame(cfg.motion.reduce_motion, now);
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
        let failed = matches!(self.updates.outcome, Some(Err(_)));
        let u = &self.updates;
        let current = su::running_version().to_string();
        let available = u.info.as_ref().filter(|i| i.available).cloned();
        let blocked = u.install.as_ref().map(|i| i.updatable(false)).and_then(Result::err);
        let running = u.job.is_some();
        let installed = matches!(u.outcome, Some(Ok(_)));

        let (glyph, tone, title, subtitle): (&str, mandara_theme::Color, String, String) = if running {
            ("download", p.primary, t("Updating Mandara…").to_string(), u.job.as_ref().map(|j| lock(j).step.clone()).unwrap_or_default())
        } else if installed {
            ("check", p.success_emerald, t("Update installed").to_string(), t("The shell restarts on the new version. Reopen Studio to use it here too.").to_string())
        } else if failed {
            ("close", p.urgent_coral, t("The update didn't install").to_string(), tf("You have {version}", &[("version", &current)]))
        } else if let Some(info) = &available {
            ("download", p.primary, tf("Version {version} available", &[("version", &info.latest)]), tf("You have {version}", &[("version", &current)]))
        } else if u.checking {
            ("refresh", p.on_surface_variant, t("Checking for updates…").to_string(), tf("You have {version}", &[("version", &current)]))
        } else if u.info.is_some() {
            ("check", p.success_emerald, t("Up to date").to_string(), tf("Mandara {version} is the latest version", &[("version", &current)]))
        } else {
            ("refresh", p.on_surface_variant, t("Mandara updates").to_string(), tf("You have {version}", &[("version", &current)]))
        };

        // The headline size comparison: the patch is the main method.
        let size_line = available.as_ref().map(|info| match (info.patch.as_ref(), info.full_size()) {
            (Some(patch), Some(full)) => tf("Smart update: {patch} instead of {full}", &[("patch", &su::human_size(patch.download.size)), ("full", &su::human_size(full))]),
            (None, Some(full)) => tf("Full update: {size} (no patch from your version)", &[("size", &su::human_size(full))]),
            _ => t("No download for this system in the release").to_string(),
        });
        // How the new binary compares with the installed one (it can shrink).
        let change_line = available.as_ref().zip(u.binary_size).and_then(|(info, size)| info.size_change(size)).map(su::size_change_text);

        let header = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_16_PX))
            .child(div().flex_none().size(px(DIM_48_PX)).rounded_full().bg(tone.with_alpha(ALPHA_15).hsla()).row().items_center().justify_center().child(fill::check_badge(&visual, p).unwrap_or_else(|| icon(glyph, ICON_22_PX, tone).into_any_element())))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_220_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(subtitle))
                    .children(size_line.map(|s| div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).text_color(p.primary.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(s)))
                    .children(change_line.map(|s| div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(s))),
            );

        // Actions.
        let mut actions = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX));
        if let Some(info) = &available
            && blocked.is_none()
            && !running
            && !installed
            && u.install.is_some()
        {
            actions = actions.child(button("upd-now", "download", t("Update now"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.updates_apply(false, cx))));
            if info.patch.is_some() {
                actions = actions.child(button("upd-full", "package", t("Download full update"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.updates_apply(true, cx))));
            }
        }
        if !running {
            actions = actions.child(button("upd-check", "refresh", if u.checking { t("Checking…") } else { t("Check now") }, false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.updates_check(true, cx))));
        }
        if let Some(url) = available.as_ref().map(|i| i.notes_url.clone()).filter(|u| !u.is_empty()) {
            actions = actions.child(button("upd-notes", "external", t("Release notes"), false, p).on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&url);
            }));
        }

        // What the available release brings (its CHANGELOG.md entry).
        let incoming = available.as_ref().filter(|_| !installed).map(|info| mandara_version::changelog::parse_notes(&info.latest, &info.notes)).filter(|r| !r.sections.is_empty()).map(|r| {
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .px(px(SPACE_16_PX))
                .py(px(SPACE_14_PX))
                .rounded(px(RADIUS_16_PX))
                .bg(p.surface_container_high.with_alpha(ALPHA_60).hsla())
                .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::SEMIBOLD).child(tf("What's in {version}", &[("version", &r.version)])))
                .child(super::whats_new::release_notes(&r, p))
        });
        // The card's own background is the progress bar; the bytes say how far.
        let progress = u.job.as_ref().and_then(|j| {
            let j = lock(j);
            (j.total > 0 && !j.installed).then(|| div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{} / {}", su::human_size(j.done), su::human_size(j.total))))
        });
        let outcome = u.outcome.as_ref().map(|r| match r {
            Ok(report) => super::form::note(&describe(report), false, p),
            Err(e) => super::form::note(e, true, p),
        });
        let error = u.error.as_ref().map(|e| super::form::note(e, true, p));
        let blocked_note = blocked.map(|why| {
            div()
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .rounded(px(RADIUS_12_PX))
                .bg(p.warning_amber.with_alpha(ALPHA_12).hsla())
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .text_size(px(TEXT_12_5_PX))
                .child(icon("package", ICON_14_PX, p.warning_amber))
                .child(why)
        });

        let method = div()
            .text_size(px(TEXT_12_PX))
            .line_height(px(LEADING_19_PX))
            .text_color(p.on_surface_variant.hsla())
            .child(mandara_ui::bidi::paragraph(t("Updates install as small binary patches against the version you have, checked byte for byte. If a patch can't be used, Mandara falls back to the full download automatically. Mandara's own plugins are built in and update with the shell.")));

        let plugin_banner = self.plugin_updates_banner(p, cx);
        let settings = self.rows_plain(registry::in_section(Section::About), self.page.column_w - 2.0 * SPACE_20_PX, cfg, p, cx);

        div()
            .id("about-updates")
            .relative()
            .overflow_hidden()
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .when(available.is_some() && visual.tone == Tone::None, |el| el.child(div().absolute().inset_0().bg(mandara_core::oklab_gradient(WASH_ANGLE, p.primary.with_alpha(ALPHA_12), p.tertiary.with_alpha(ALPHA_5)))))
            .children(fill::layer(&visual, mandara_ui::bidi::rtl(), p))
            .child(
                div()
                    .relative()
                    .p(px(SPACE_20_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_14_PX))
                    .child(header)
                    .children(incoming)
                    .children(progress)
                    .children(outcome)
                    .children(error)
                    .children(blocked_note)
                    .child(actions)
                    .child(method)
                    .children(plugin_banner)
                    .child(settings),
            )
            .into_any_element()
    }
}

/// `SharedString` for element ids built from plugin ids.
pub(super) fn id_for(prefix: &str, id: &str) -> SharedString {
    SharedString::from(format!("{prefix}-{id}"))
}
