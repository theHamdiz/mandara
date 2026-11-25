//! Marketplace plugin updates in Studio: the compact Update
//! action in the plugin detail modal and the "N plugin updates · Update
//! all" banner (Plugins → Installed and the About update card). Same method
//! as the shell: binary patch first, the full archive as the fallback; the
//! shell hot-reloads the plugin and its enabled state is kept. A release
//! that fails the compliance check isn't applied: it is flagged, and its
//! row says why and offers "Install anyway".

use mandara_ui::bidi::Bidi as _;
use std::sync::Arc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px, prelude::FluentBuilder as _};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::marketplace::{Flag, Vetting};
use mandara_services::self_update::{ApplyOptions, plugins::{self, PluginUpdate}};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;

use super::StudioView;
use super::compositor_page::button;
use super::store_page::{pill, Tone, ButtonSize};
use super::updates_card::{JobHandle, describe, id_for, lock, progress_bar, reporter, watch_job};
use crate::tokens::*;

impl StudioView {
    pub(super) fn available_plugin_updates(&self) -> impl Iterator<Item = &PluginUpdate> {
        self.updates.plugins.iter().filter(|update| self.market.update_available(update))
    }

    pub(super) fn plugin_update(&self, id: &str) -> Option<&PluginUpdate> {
        self.available_plugin_updates().find(|update| update.id == id)
    }
    /// The flag of `u`'s new release, when it failed the compliance check.
    pub(super) fn update_flag(&mut self, u: &PluginUpdate, cx: &mut Context<Self>) -> Option<Flag> {
        self.release_flag(&u.repo, &u.latest, cx)
    }

    /// The flag of release `latest` of `repo`, when it failed the check.
    fn release_flag(&mut self, repo: &str, latest: &str, cx: &mut Context<Self>) -> Option<Flag> {
        self.market_local_prime(cx);
        self.market.flags.get(repo).filter(|f| f.tag == latest).cloned()
    }

    /// Updates `id`; `force` installs a release that failed the check.
    pub(super) fn plugin_update_start(&mut self, id: String, force: bool, cx: &mut Context<Self>) {
        let Some(u) = self.plugin_update(&id).cloned() else { return };
        if self.updates.plugin_jobs.contains_key(&id) {
            return;
        }
        let job: JobHandle = Arc::default();
        self.updates.plugin_jobs.insert(id.clone(), job.clone());
        let worker = job.clone();
        let check = super::market_page::vetting_check(cx);
        std::thread::spawn(move || {
            let result = plugins::apply(&u, ApplyOptions::default(), &Vetting { check: &check, force }, &reporter(worker.clone()));
            lock(&worker).result = Some(result);
        });
        watch_job(job, cx, move |view, result, cx| {
            view.updates.plugin_applied(&id, result.is_err());
            view.updates.plugin_jobs.remove(&id);
            view.market_local_refresh(cx);
            match result {
                Ok(report) => {
                    view.market.message = None;
                    view.updates.plugins.retain(|u| u.id != id);
                    super::market_page::reload_plugins(cx);
                    view.prompt_plugin_permissions(&id);
                    view.updates.plugin_message = Some((true, format!("{id}: {}", describe(&report))));
                }
                Err(e) => {
                    view.updates.plugins.retain(|update| update.id != id);
                    view.updates.plugin_message = Some((false, format!("{id}: {e}")));
                }
            }
        });
        cx.notify();
    }

    pub(super) fn plugin_update_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.available_plugin_updates().map(|u| u.id.clone()).collect();
        for id in ids {
            self.plugin_update_start(id, false, cx);
        }
    }

    /// The modal's update action for marketplace plugin `id`, when a newer
    /// release is known.
    pub(super) fn plugin_update_action(&mut self, id: &str, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.updates_prime(cx);
        let u = self.plugin_update(id)?;
        if let Some(job) = self.updates.plugin_jobs.get(id) {
            let j = lock(job);
            return Some(div().w_full().min_w(px(0.0)).row().flex_col().gap(px(SPACE_4_PX)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if j.step.is_empty() { t("Updating…").to_string() } else { j.step.clone() })).child(progress_bar(j.done, j.total, p)).into_any_element());
        }
        let (repo, latest, target) = (u.repo.clone(), u.latest.clone(), u.id.clone());
        if let Some(flag) = self.release_flag(&repo, &latest, cx) {
            return Some(self.install_anyway(id_for("plugin-update", id).to_string(), &flag, p, cx, move |this, cx| this.plugin_update_start(target.clone(), true, cx)).into_any_element());
        }
        Some(pill(id_for("plugin-update", id), Some("download"), t("Update"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plugin_update_start(target.clone(), false, cx))).into_any_element())
    }

    /// "2 plugin updates · Update all" with one row per plugin, or `None`
    /// when every marketplace plugin is current (and nothing to report).
    pub(super) fn plugin_updates_banner(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.updates_prime(cx);
        // Only what a row shows (not the downloads each update carries).
        let pending: Vec<(String, String, String, String)> = self.available_plugin_updates().map(|u| (u.id.clone(), u.repo.clone(), u.installed.clone(), u.latest.clone())).collect();
        let message = self.updates.plugin_message.clone().map(|(ok, text)| {
            div().row().flex_col().gap(px(SPACE_8_PX)).child(super::form::note(&text, !ok, p))
                .when(!ok, |row| row.child(button("plugin-update-retry", "refresh", t("Check again"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.updates_check(true, cx)))))
                .into_any_element()
        });
        if pending.is_empty() {
            return message;
        }
        let mut rows = div().row().flex_col().gap(px(SPACE_6_PX));
        for (id, repo, installed, latest) in &pending {
            let action = self.plugin_update_action(id, p, cx);
            let warning = self.release_flag(repo, latest, cx).map(|f| super::market_page::flag_note(&f, p));
            rows = rows.children(warning).child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(SPACE_10_PX))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(icon("package", ICON_14_PX, p.on_surface_variant)).child(div().font_weight(FontWeight::MEDIUM).child(id.clone())).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(format!("{installed} → {latest}"))))
                    .children(action),
            );
        }
        let title = if pending.len() == 1 { t("1 plugin update").to_string() } else { tf("{count} plugin updates", &[("count", &pending.len())]) };
        let busy = !self.updates.plugin_jobs.is_empty();
        let header = div()
            .row()
            .items_center()
            .justify_between()
            .gap(px(SPACE_10_PX))
            .child(div().row().items_center().gap(px(SPACE_8_PX)).child(icon("download", ICON_16_PX, p.primary)).child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(title)))
            .children((pending.len() > 1 && !busy).then(|| button("plugin-update-all", "download", t("Update all"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.plugin_update_all(cx)))));
        Some(
            div()
                .p(px(SPACE_16_PX))
                .rounded(px(RADIUS_18_PX))
                .bg(p.surface_container_high.with_alpha(ALPHA_60).hsla())
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(header)
                .child(rows)
                .children(message)
                .into_any_element(),
        )
    }
}
