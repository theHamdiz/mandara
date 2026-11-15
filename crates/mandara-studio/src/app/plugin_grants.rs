//! Studio → Plugins → a third-party plugin's page: what the person lets it
//! do, and where its code and issue tracker live.
//!
//! * **Permissions:** every capability its plugin.toml asks for that needs
//!   the person's yes (media, wallpapers, folders, a network host, another
//!   plugin) is a switch, off until switched on. Switching one off revokes
//!   it at once: the shell checks every request against
//!   `[plugins.grants.<id>]` when it arrives and closes what relied on it.
//!   Folders for `files.folders` are chosen with the desktop's own picker.
//! * **Links:** "Source code" and "Report an issue" open the plugin's
//!   repository and tracker (from plugin.toml's `repository` / `support`,
//!   its GitHub homepage, or the marketplace repository it came from).

use std::path::{Path, PathBuf};

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_plugins::PluginManifest;
use mandara_plugins::extension::{self, Capability};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::plugin_modal::action;
use crate::tokens::*;

/// Most folders one plugin may be given.
const MAX_FOLDERS: usize = 16;

impl StudioView {
    /// The switches for the capabilities `m` asks for (none: `None`).
    pub(super) fn plugin_grants(&mut self, m: &PluginManifest, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let asks: Vec<Capability> = extension::split_capabilities(&m.permissions).0.into_iter().filter(Capability::needs_grant).collect();
        if asks.is_empty() {
            return None;
        }
        let granted = cfg.plugins.granted(&m.id);
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX)).child(
            div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("It asks for these. Each stays off until you switch it on, and you can switch it off again at any time."))),
        );
        for c in asks {
            let name = c.to_string();
            let on = granted.contains(&name);
            let what = match c.target() {
                Some((key, value)) => tf(c.describe(), &[(key, &value)]),
                None => tr(c.describe()).to_string(),
            };
            let (id, cap) = (m.id.clone(), name.clone());
            let row = div()
                .id(SharedString::from(format!("plugin-grant-{}-{name}", m.id)))
                .px(px(SPACE_14_PX))
                .py(px(SPACE_12_PX))
                .rounded(px(RADIUS_16_PX))
                .bg(p.surface_container.hsla())
                .row()
                .items_center()
                .gap(px(SPACE_12_PX))
                .cursor_pointer()
                .child(div().size(px(DIM_36_PX)).flex_none().rounded(px(RADIUS_11_PX)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_14).hsla()).child(icon(c.icon(), ICON_16_PX, p.primary)))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_3_PX))
                        .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(what)))
                        .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_family(mandara_ui::fonts::fonts(cx).mono.clone()).child(name.clone())),
                )
                .child(mandara_ui::components::toggles::switch(on, mandara_ui::style::toggle_design(), p))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    update_config(cx, |c| {
                        c.plugins.set_granted(&id, &cap, !on);
                    });
                    cx.notify();
                }));
            list = list.child(row);
            if c == Capability::Folders && on {
                list = list.child(self.granted_folders(m, cfg.plugins.granted_folders(&m.id), p, cx));
            }
        }
        // Grants add up: what it may read, it could send where it connects.
        let has = |c: &Capability| granted.iter().any(|g| Capability::parse(g).is_ok_and(|g| g == *c));
        let reads = [Capability::MediaRead, Capability::ClipboardRead, Capability::WindowsRead, Capability::Folders, Capability::WallpaperRead, Capability::SystemRead, Capability::LauncherGlobal];
        let online = granted.iter().any(|g| matches!(Capability::parse(g), Ok(Capability::Net(_))));
        if online && reads.iter().any(has) {
            list = list.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.warning_amber.hsla())
                    .child(icon("alert", ICON_12_PX, p.warning_amber))
                    .child(mandara_ui::bidi::paragraph(t("It can also send what it sees to the sites it connects to. Keep both on only for plugins you trust."))),
            );
        }
        Some(list.into_any_element())
    }

    /// The folders given to `m`, each removable, and "Add a folder".
    fn granted_folders(&mut self, m: &PluginManifest, folders: &[PathBuf], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).pad_start(px(SPACE_14_PX));
        for (at, folder) in folders.iter().enumerate() {
            let (id, folder_path) = (m.id.clone(), folder.clone());
            row = row.child(
                div()
                    .id(SharedString::from(format!("plugin-folder-{}-{at}", m.id)))
                    .h(px(DIM_32_PX))
                    .px(px(SPACE_12_PX))
                    .rounded_full()
                    .bg(p.surface_container_high.hsla())
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .text_size(px(TEXT_12_PX))
                    .child(icon("folder", ICON_12_PX, p.on_surface_variant))
                    .child(folder.display().to_string())
                    .child(
                        div()
                            .id(SharedString::from(format!("plugin-folder-remove-{}-{at}", m.id)))
                            .cursor_pointer()
                            .child(icon("close", ICON_12_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                update_config(cx, |c| {
                                    if let Some(g) = c.plugins.grants.get_mut(&id) {
                                        g.folders.retain(|f| *f != folder_path);
                                    }
                                });
                                cx.notify();
                            })),
                    ),
            );
        }
        if folders.len() < MAX_FOLDERS {
            let id = m.id.clone();
            row = row.child(action(format!("plugin-folder-add-{}", m.id), "plus", t("Add a folder").into(), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                let id = id.clone();
                let title = t("Choose a folder for this plugin");
                let pick = cx.background_executor().spawn(async move { mandara_services::file_chooser::pick_folder(title, dirs::picture_dir()) });
                cx.spawn(async move |this, cx| {
                    let Ok(Some(folder)) = pick.await else { return };
                    let _ = this.update(cx, |_, cx| {
                        update_config(cx, |c| {
                            let grant = c.plugins.grants.entry(id.clone()).or_default();
                            if !grant.folders.contains(&folder) && grant.folders.len() < MAX_FOLDERS {
                                grant.folders.push(folder);
                            }
                        });
                        cx.notify();
                    });
                })
                .detach();
            })));
        }
        row.into_any_element()
    }

    /// "Source code" and "Report an issue" for a third-party plugin in
    /// `dir`, opened in the browser.
    pub(super) fn plugin_links(&self, m: &PluginManifest, dir: &Path, p: &MandaraPalette) -> Vec<gpui::Stateful<gpui::Div>> {
        let market = self.market_repo_of(dir).map(|repo| format!("https://github.com/{repo}"));
        let repository = if m.repository.is_empty() { market.clone().unwrap_or_default() } else { m.repository.clone() };
        let source = extension::source_url(&repository, &m.homepage);
        let issues = extension::issues_url(&m.support, &repository, &m.homepage);
        let mut out = Vec::new();
        if let Some(url) = source {
            out.push(action("plugin-modal-source", "git-branch", t("Source code").into(), false, p).on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&url);
            }));
        }
        if let Some(url) = issues {
            out.push(action("plugin-modal-issues", "bug", t("Report an issue").into(), false, p).on_click(move |_, _, _| {
                let _ = mandara_services::session::open_uri(&url);
            }));
        }
        out
    }
}
