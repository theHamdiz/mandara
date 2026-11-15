//! Studio → Maintenance page: health checks with repairs, downloads,
//! storage, backups and factory reset (logic in [`crate::maintenance`]).

use mandara_ui::bidi::Bidi as _;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_services::models;
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;

use super::{StudioView, grouped};
use crate::maintenance::{self as m, CheckId, Health};
use crate::tokens::*;

/// Page state.
#[derive(Default)]
pub struct Maint {
    results: HashMap<CheckId, Health>,
    checking: bool,
    /// Running jobs by key (`check:Fonts`, `model:ocr-det`, `caches`, …)
    /// with their progress in per mille (`u32::MAX` = indeterminate).
    jobs: HashMap<String, Arc<AtomicU32>>,
    message: Option<(bool, String)>,
    reset_armed: Option<Instant>,
    cache_bytes: Option<u64>,
    backups: Vec<PathBuf>,
    generation: u64,
    icons: Option<Option<String>>,
}

impl Maint {
    pub(super) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.results.clear();
    }

    /// Discard results from a check that started before the page was invalidated.
    fn finish(&mut self, generation: u64, results: HashMap<CheckId, Health>) -> bool {
        self.checking = false;
        if generation != self.generation { return false; }
        self.results = results;
        true
    }

    /// Offer bulk repair only when a failing check has an automatic repair path.
    fn can_repair(&self, compositor: bool, missing_models: bool) -> bool {
        missing_models || self.results.iter().any(|(id, health)| !health.is_ok() && *id != CheckId::Icons && (!matches!(id, CheckId::CompositorInclude | CheckId::CompositorDropIn | CheckId::CompositorConflicts) || compositor))
    }
}

fn run_ipc(cx: &App, commands: Vec<Vec<String>>) {
    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
        std::thread::spawn(move || {
            for c in commands {
                if let Err(err) = (ipc.0)(c.clone()) {
                    tracing::warn!("maintenance: `{}` failed: {err}", c.join(" "));
                }
            }
        });
    }
}

/// Wait after a fix's follow-up commands before re-checking, so the shell
/// has acted on them.
const FOLLOWUP_SETTLE: Duration = Duration::from_millis(1200);
/// Repaint interval while a job reports progress.
const JOB_REPAINT: Duration = Duration::from_millis(200);
/// Job progress is reported in thousandths.
const PERMILLE: u64 = 1000;
const PERCENT: u64 = 100;
/// "Factory reset" needs a second click within this long.
const RESET_ARM_WINDOW: Duration = Duration::from_secs(5);
/// Most recent backups listed.
const BACKUPS_SHOWN: usize = 5;

/// Look of a maintenance pill button.
#[derive(Copy, Clone, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Tonal,
    Danger,
    /// A destructive action waiting for its confirming second click.
    DangerArmed,
}

fn human(bytes: u64) -> String {
    mandara_ui::widgets::system::format_bytes(bytes)
}

impl StudioView {
    /// Re-runs every check (and refreshes storage + backups).
    pub(super) fn maint_refresh(&mut self, cx: &mut Context<Self>) {
        if self.maint.checking {
            return;
        }
        self.maint.checking = true;
        let generation = self.maint.generation;
        let fonts = cx.text_system().all_font_names();
        let icons = mandara_ui::state::config(cx).read(cx).get().appearance.icon_theme.clone();
        let task = cx.background_executor().spawn(async move {
            let results: HashMap<CheckId, Health> = CheckId::ALL.iter().map(|id| (*id, if *id == CheckId::Icons { m::icon_theme(icons.as_deref()) } else { m::check(*id, &fonts) })).collect();
            (results, m::cache_size(), m::backups())
        });
        cx.spawn(async move |this, cx| {
            let (results, cache, backups) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.maint.finish(generation, results) {
                    this.maint.cache_bytes = Some(cache);
                    this.maint.backups = backups;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Runs `work` in the background under `key`, showing progress, then
    /// the message and any shell follow-ups; re-checks afterwards.
    fn maint_job(&mut self, key: String, cx: &mut Context<Self>, work: impl FnOnce(Arc<AtomicU32>) -> Result<(String, Vec<Vec<String>>), String> + Send + 'static) {
        if self.maint.jobs.contains_key(&key) {
            return;
        }
        let progress = Arc::new(AtomicU32::new(u32::MAX));
        self.maint.jobs.insert(key.clone(), progress.clone());
        let p = progress.clone();
        let task = cx.background_executor().spawn(async move { work(p) });
        // Repaint while it runs so progress moves.
        cx.spawn({
            let key = key.clone();
            async move |this, cx| loop {
                cx.background_executor().timer(JOB_REPAINT).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.maint.jobs.contains_key(&key)
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            // Give the shell a moment to act on follow-ups before re-checking.
            let followups = result.as_ref().map(|(_, f)| f.clone()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.maint.jobs.remove(&key);
                this.maint.message = Some(match &result {
                    Ok((msg, _)) => (true, msg.clone()),
                    Err(e) => (false, e.clone()),
                });
                run_ipc(cx, followups.clone());
                cx.notify();
            });
            if !followups.is_empty() {
                cx.background_executor().timer(FOLLOWUP_SETTLE).await;
            }
            let _ = this.update(cx, |this, cx| this.maint_refresh(cx));
        })
        .detach();
        cx.notify();
    }

    /// Run one repair in the background and refresh its diagnostic afterward.
    fn maint_fix(&mut self, id: CheckId, cx: &mut Context<Self>) {
        if id == CheckId::Icons {
            self.show(crate::registry::Section::Themes, cx);
            return;
        }
        let fonts = cx.text_system().all_font_names();
        self.maint_job(format!("check:{id:?}"), cx, move |p| {
            m::fix(id, &fonts, &move |done, total| p.store(u32::try_from(done * PERMILLE / total.max(1)).unwrap_or(u32::MAX - 1), Ordering::Relaxed))
        });
    }

    /// Repair eligible checks; icon-theme choices remain an explicit Themes action.
    fn maint_repair_all(&mut self, cx: &mut Context<Self>) {
        let broken: Vec<CheckId> = CheckId::ALL.iter().copied().filter(|id| self.maint.results.get(id).is_some_and(|h| !h.is_ok())).collect();
        // Pointless without a compositor; skip checks that can't be fixed here.
        for id in broken {
            if id == CheckId::Icons { continue; }
            if matches!(id, CheckId::CompositorInclude | CheckId::CompositorDropIn | CheckId::CompositorConflicts) && mandara_compositor::detected().is_none() {
                continue;
            }
            self.maint_fix(id, cx);
        }
        for model in models::MODELS.iter().filter(|m| !m.on_demand && !m.installed()) {
            self.maint_model(model.id, true, cx);
        }
    }

    fn maint_model(&mut self, id: &'static str, download: bool, cx: &mut Context<Self>) {
        self.maint_job(format!("model:{id}"), cx, move |p| {
            let model = models::find(id).ok_or(t("unknown model"))?;
            if download {
                model.download(&move |done, total| p.store(u32::try_from(done * PERMILLE / total.max(1)).unwrap_or(u32::MAX - 1), Ordering::Relaxed))?;
                Ok((tf("{model} installed", &[("model", &tr(model.title))]), Vec::new()))
            } else {
                model.remove()?;
                Ok((tf("{model} removed", &[("model", &tr(model.title))]), Vec::new()))
            }
        });
    }

    /// Downloads or removes the Qur'an plugin's text and meaning here, then
    /// lets the shell load (or forget) them.
    fn maint_quran(&mut self, download: bool, cx: &mut Context<Self>) {
        use mandara_services::quran::store;
        self.maint_job("quran".into(), cx, move |p| {
            if download {
                let cfg = mandara_config::ConfigStore::open_default().map(|s| s.snapshot()).unwrap_or_default();
                let chosen: String = cfg.plugins.setting("quran", "translation", String::new());
                let edition = mandara_services::quran::meaning_edition(mandara_i18n::language().code(), &chosen);
                store::ensure(edition.as_deref(), &move |pr: store::Progress| p.store(u32::try_from(pr.done * PERMILLE / pr.total.max(1)).unwrap_or(u32::MAX - 1), Ordering::Relaxed))?;
                Ok((t("The Qur'an is downloaded").into(), vec![vec!["quran".into(), "download".into()]]))
            } else {
                let freed = store::remove()?;
                Ok((tf("The Qur'an's downloads are removed ({size})", &[("size", &human(freed))]), vec![vec!["quran".into(), "remove".into()]]))
            }
        });
    }

    /// Refresh diagnostics after relevant settings change and show actions per check.
    pub(super) fn maintenance(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let icons = mandara_ui::state::config(cx).read(cx).get().appearance.icon_theme.clone();
        if self.maint.icons.as_ref() != Some(&icons) {
            self.maint.invalidate();
            self.maint.icons = Some(icons);
        }
        if self.maint.results.is_empty() && !self.maint.checking {
            self.maint_refresh(cx);
        }
        let stack = self.stack_rows;
        let mono = mandara_ui::fonts::fonts(cx).mono.clone();
        let jobs = self.maint.jobs.clone();
        // A pill button; shows progress while its job runs.
        let button = |id: String, label: String, job: Option<&String>, kind: ButtonKind| {
            let running = job.and_then(|k| jobs.get(k)).map(|a| a.load(Ordering::Relaxed));
            let (bg, fg) = match kind {
                ButtonKind::Primary => (p.primary, p.on_primary),
                ButtonKind::Danger => (p.urgent_coral.with_alpha(ALPHA_14), p.urgent_coral),
                ButtonKind::DangerArmed => (p.urgent_coral, p.on_primary),
                ButtonKind::Tonal => (p.on_surface.with_alpha(ALPHA_8), p.on_surface),
            };
            let text = match running {
                Some(u32::MAX) => t("Working…").to_string(),
                Some(pm) => format!("{}%", pm / (PERMILLE / PERCENT) as u32),
                None => label,
            };
            div()
                .id(SharedString::from(id))
                .h(px(DIM_30_PX))
                .px(px(SPACE_12_PX))
                .rounded_full()
                .row()
                .items_center()
                .flex_none()
                .bg(bg.hsla())
                .text_color(fg.hsla())
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .when(running.is_none(), |el| el.cursor_pointer().hover(|s| s.opacity(ALPHA_85)))
                .when(running.is_some(), |el| el.opacity(ALPHA_75))
                .child(text)
        };
        let health = |h: Option<&Health>| {
            let (label, color) = match h {
                Some(Health::Ok(_)) => (t("OK"), p.success_emerald),
                Some(Health::Warn(_)) => (t("Attention"), p.warning_amber),
                Some(Health::Broken(_)) => (t("Broken"), p.urgent_coral),
                None => (t("Checking"), p.on_surface_variant),
            };
            grouped::status(label, color, p)
        };
        let presence = |installed: bool| if installed { grouped::status(t("Installed"), p.success_emerald, p) } else { grouped::status(t("Not installed"), p.on_surface.with_alpha(ALPHA_30), p) };

        // ── Overview ──────────────────────────────────────────────────────
        let missing_models = models::MODELS.iter().filter(|m| !m.on_demand && !m.installed()).count();
        let issues = self.maint.results.values().filter(|h| !h.is_ok()).count() + missing_models;
        let can_repair = self.maint.can_repair(mandara_compositor::detected().is_some(), missing_models > 0);
        let all_good = !self.maint.results.is_empty() && issues == 0;
        let tone = if all_good { p.success_emerald } else { p.warning_amber };
        let summary = if self.maint.checking && self.maint.results.is_empty() {
            t("Checking your installation…").to_string()
        } else if all_good {
            t("Everything looks healthy").to_string()
        } else if issues == 1 {
            t("1 thing to look at").to_string()
        } else {
            tf("{n} things to look at", &[("n", &issues)])
        };
        let hero = grouped::group(
            vec![div()
                .w_full()
                .p(px(if stack { SPACE_16_PX } else { SPACE_20_PX }))
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_16_PX))
                .child(div().size(px(DIM_48_PX)).rounded(px(RADIUS_14_PX)).row().items_center().justify_center().flex_none().bg(tone.with_alpha(ALPHA_15).hsla()).child(icon(if all_good { "check" } else { "refresh" }, ICON_24_PX, tone)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(DIM_200_PX))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_3_PX))
                        .child(div().text_size(px(TEXT_17_PX)).font_weight(FontWeight::SEMIBOLD).child(summary))
                        .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Config, compositor integration, app themes, fonts, plugin folders and downloadable models.")))),
                )
                .child(button("maint-check".into(), if self.maint.checking { t("Checking…").into() } else { t("Run checks").into() }, None, ButtonKind::Tonal).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.maint_refresh(cx))))
                .when(can_repair && !self.maint.results.is_empty(), |el| el.child(button("maint-repair".into(), t("Repair all").into(), None, ButtonKind::Primary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.maint_repair_all(cx)))))
                .into_any_element()],
            false,
            p,
        );

        let message = self.maint.message.clone().map(|(ok, msg)| super::form::note(&msg, !ok, p));

        // ── Checks ────────────────────────────────────────────────────────
        let mut checks = Vec::with_capacity(CheckId::ALL.len());
        for id in CheckId::ALL {
            let h = self.maint.results.get(&id).cloned();
            let key = format!("check:{id:?}");
            let fixable = h.as_ref().is_some_and(|h| !h.is_ok()) && !(matches!(id, CheckId::CompositorInclude | CheckId::CompositorDropIn | CheckId::CompositorConflicts) && mandara_compositor::detected().is_none());
            let row = grouped::row(Some(id.icon()), tr(id.title())).caption(h.as_ref().map(|h| h.detail().to_string()).unwrap_or_default()).trailing(health(h.as_ref()));
            let row = if fixable { row.trailing(button(format!("fix-{id:?}"), tr(id.fix_label()), Some(&key), ButtonKind::Tonal).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.maint_fix(id, cx)))) } else { row };
            checks.push(row.render(stack, mono.clone(), p));
        }
        let checks = grouped::section(grouped::header(t("Health checks"), Some(t("Each repair is safe: your compositor config only ever gets one include line, with a backup.").into()), Vec::new(), p), grouped::group(checks, true, p));

        // ── Downloads ─────────────────────────────────────────────────────
        let mut downloads = Vec::new();
        for model in models::MODELS.iter().filter(|model| !mandara_services::command_agent::models::ALL.iter().any(|agent| agent.id == model.id)) {
            let installed = model.installed();
            let key = format!("model:{}", model.id);
            let id = model.id;
            let action = if installed {
                button(format!("rm-{id}"), t("Remove").into(), Some(&key), ButtonKind::Tonal).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.maint_model(id, false, cx)))
            } else {
                button(format!("dl-{id}"), t("Download").into(), Some(&key), ButtonKind::Primary).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.maint_model(id, true, cx)))
            };
            downloads.push(grouped::row(Some(model.icon), tr(model.title)).caption(format!("{} · {}", tr(model.purpose), human(model.download_size()))).trailing(presence(installed)).trailing(action).render(stack, mono.clone(), p));
        }

        // The Hadith plugin's library (downloaded by the shell, which hears
        // about a removal through its status command).
        let (collections, bytes) = mandara_services::hadith::store::summary();
        let installed = collections > 0;
        let key = "hadith-library".to_string();
        let caption = if installed {
            format!("{} · {}", tf("{n} collections from hadith-api (public domain)", &[("n", &collections)]), human(bytes))
        } else {
            t("Collections the Hadith plugin downloads from hadith-api (public domain)").to_string()
        };
        let action = if installed {
            button("rm-hadith".into(), t("Remove").into(), Some(&key), ButtonKind::Tonal).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.maint_job("hadith-library".into(), cx, |_| {
                    let freed = mandara_services::hadith::store::remove_all()?;
                    Ok((tf("Hadith library removed · {size} freed", &[("size", &human(freed))]), vec![vec!["hadith".into(), "status".into()]]))
                });
            }))
        } else {
            button("dl-hadith".into(), t("Download").into(), Some(&key), ButtonKind::Primary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.maint_job("hadith-library".into(), cx, |_| Ok((t("Downloading in the Hadith panel").to_string(), vec![vec!["hadith".into(), "download".into()]])));
            }))
        };
        downloads.push(grouped::row(Some("hadith"), t("Hadith library")).caption(caption).trailing(presence(installed)).trailing(action).render(stack, mono.clone(), p));
        // The Qur'an plugin's text and meaning (downloaded, never bundled).
        {
            use mandara_services::quran::store;
            let installed = store::installed_bytes();
            let key = "quran".to_string();
            let size = installed.unwrap_or(store::META_BYTES + store::TEXT_BYTES + store::MEANING_BYTES);
            let action = if installed.is_some() {
                button("rm-quran".into(), t("Remove").into(), Some(&key), ButtonKind::Tonal).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.maint_quran(false, cx)))
            } else {
                button("dl-quran".into(), t("Download").into(), Some(&key), ButtonKind::Primary).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.maint_quran(true, cx)))
            };
            downloads.push(grouped::row(Some("book"), t("Qur'an")).caption(format!("{} · {}", t("The Tanzil text and its meaning, for the Qur'an plugin (api.alquran.cloud)"), human(size))).trailing(presence(installed.is_some())).trailing(action).render(stack, mono.clone(), p));
        }
        let downloads = grouped::section(grouped::header(t("Downloads"), Some(t("Models for copying text from screenshots and for the wallpaper depth effect's model mode. From Hugging Face, all Apache-2.0; the depth model downloads only when you pick that mode.").into()), Vec::new(), p), grouped::group(downloads, true, p));

        // ── Storage, backups & reset ──────────────────────────────────────
        let armed = self.maint.reset_armed.is_some_and(|t| t.elapsed() < RESET_ARM_WINDOW);
        let mut storage = vec![
            grouped::row(Some("disk"), t("Caches"))
                .caption(t("Thumbnails and Wallhaven previews; they rebuild on demand."))
                .trailing(div().font_features(mandara_ui::fonts::tabular_nums()).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(self.maint.cache_bytes.map(human).unwrap_or_else(|| "…".into())))
                .trailing(button("maint-cache".into(), t("Clear").into(), Some(&"caches".to_string()), ButtonKind::Tonal).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.maint_job("caches".into(), cx, |_| m::clear_caches().map(|freed| (tf("Freed {size}", &[("size", &human(freed))]), Vec::new())));
                })))
                .render(stack, mono.clone(), p),
            grouped::row(Some("download"), t("Backups & factory reset"))
                .caption(t("Backups copy ~/.config/mandara. Factory reset backs up first, then restores every default."))
                .wrap()
                .trailing(button("maint-backup".into(), t("Back up now").into(), Some(&"backup".to_string()), ButtonKind::Tonal).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.maint_job("backup".into(), cx, |_| m::backup_now("manual").map(|p| (tf("Backed up to {folder}", &[("folder", &p.display())]), Vec::new())));
                })))
                .trailing(button("maint-reset".into(), if armed { t("Click again to reset").into() } else { t("Factory reset").into() }, Some(&"reset".to_string()), if armed { ButtonKind::DangerArmed } else { ButtonKind::Danger }).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if this.maint.reset_armed.is_some_and(|t| t.elapsed() < RESET_ARM_WINDOW) {
                        this.maint.reset_armed = None;
                        this.maint_job("reset".into(), cx, |_| m::factory_reset().map(|(b, f)| (tf("Defaults restored — your old setup is in {folder}", &[("folder", &b.display())]), f)));
                    } else {
                        this.maint.reset_armed = Some(Instant::now());
                        cx.notify();
                    }
                })))
                .render(stack, mono.clone(), p),
        ];
        for (i, b) in self.maint.backups.iter().take(BACKUPS_SHOWN).enumerate() {
            let name = b.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let path = b.clone();
            storage.push(
                grouped::row(Some("clock"), name)
                    .trailing(button(format!("restore-{i}"), t("Restore").into(), Some(&format!("restore:{i}")), ButtonKind::Tonal).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let path = path.clone();
                        this.maint_job(format!("restore:{i}"), cx, move |_| m::restore(&path).map(|f| (t("Backup restored").to_string(), f)));
                    })))
                    .render(false, mono.clone(), p),
            );
        }
        let storage = grouped::section(grouped::header(t("Storage & backups"), None, Vec::new(), p), grouped::group(storage, true, p));

        div().row().flex_col().gap(px(GROUP_SECTION_GAP_PX)).child(div().row().flex_col().gap(px(BENTO_GAP_PX)).child(hero).children(message)).child(checks).child(downloads).child(storage).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_previous_visit_cannot_restore_a_stale_health_warning() {
        let mut state = Maint { checking: true, results: HashMap::from([(CheckId::Icons, Health::Warn("old theme".into()))]), ..Default::default() };
        let old = state.generation;
        state.invalidate();
        assert!(state.results.is_empty());
        assert!(!state.finish(old, HashMap::from([(CheckId::Icons, Health::Warn("old theme".into()))])));
        assert!(!state.checking && state.results.is_empty());
        assert!(state.finish(state.generation, HashMap::from([(CheckId::Icons, Health::Ok("new theme".into()))])));
        assert!(state.results[&CheckId::Icons].is_ok());
    }

    #[test]
    fn repair_all_only_appears_when_it_can_do_work() {
        let mut state = Maint { results: HashMap::from([(CheckId::Icons, Health::Warn("missing".into()))]), ..Default::default() };
        assert!(!state.can_repair(true, false));
        assert!(state.can_repair(true, true));
        state.results.insert(CheckId::Fonts, Health::Warn("missing".into()));
        assert!(state.can_repair(true, false));
        state.results.remove(&CheckId::Fonts);
        state.results.insert(CheckId::CompositorConflicts, Health::Warn("unavailable".into()));
        assert!(!state.can_repair(false, false));
        assert!(state.can_repair(true, false));
    }
}
