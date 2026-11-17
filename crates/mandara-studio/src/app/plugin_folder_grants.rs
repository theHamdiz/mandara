//! Folders a person approves for a plugin, with optional Steam Workshop suggestions.

use std::path::{Path, PathBuf};
use gpui::{AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_plugins::PluginManifest;
use mandara_services::steam_libraries::{FolderDiscovery as _, SteamWorkshop};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;
use super::{StudioView, plugin_modal::action};
use crate::tokens::*;

/// Most folders one plugin may be given.
const MAX_FOLDERS: usize = 16;

#[derive(Clone, PartialEq, Eq)]
struct Choice {
    plugin: String,
    modal_generation: u64,
    permission_generation: u64,
}

#[derive(Default)]
pub(super) struct Suggestions {
    requests: crate::latest::Latest<String>,
    picker: crate::latest::Latest<Choice>,
    folders: Vec<PathBuf>,
    searched: bool,
    generation: u64,
}

impl Suggestions {
    pub fn close(&mut self) {
        self.requests.set(None);
        self.picker.set(None);
        self.folders = Vec::new();
        self.searched = false;
        self.generation = self.generation.wrapping_add(1);
    }

    fn loading(&self) -> bool { self.requests.desired().is_some() && !self.searched }
}

fn grant_folder(config: &mut ShellConfig, plugin: &str, folder: &Path) -> bool {
    if !config.plugins.granted(plugin).iter().any(|cap| cap == "files.folders") { return false; }
    let grant = config.plugins.grants.entry(plugin.to_string()).or_default();
    if grant.folders.len() >= MAX_FOLDERS || grant.folders.iter().any(|f| f == folder) { return false; }
    grant.folders.push(folder.to_path_buf());
    true
}

fn grant_picked_folder(config: &mut ShellConfig, plugin: &str, folder: &Path, generation: u64) -> bool {
    config.plugins.grants.get(plugin).is_some_and(|grant| grant.generation == generation)
        && grant_folder(config, plugin, folder)
}

impl StudioView {
    /// The folders given to `m`, each removable, and "Add a folder".
    pub(super) fn granted_folders(&mut self, m: &PluginManifest, folders: &[PathBuf], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).pad_start(px(SPACE_14_PX));
        for (at, folder) in folders.iter().enumerate() {
            let (id, folder_path) = (m.id.clone(), folder.clone());
            let (key_id, key_folder) = (id.clone(), folder_path.clone());
            row = row.child(
                div()
                    .id(SharedString::from(format!("plugin-folder-{}-{at}", m.id)))
                    .min_h(px(DIM_32_PX))
                    .max_w_full()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_6_PX))
                    .rounded(px(RADIUS_16_PX))
                    .bg(p.surface_container_high.hsla())
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .text_size(px(TEXT_12_PX))
                    .child(icon("folder", ICON_12_PX, p.on_surface_variant))
                    .child(div().flex_1().min_w(px(0.0)).child(mandara_ui::bidi::paragraph(folder.display().to_string())))
                    .child(
                        div()
                            .id(SharedString::from(format!("plugin-folder-remove-{}-{at}", m.id)))
                            .focusable().tab_index(0)
                            .flex_none()
                            .cursor_pointer()
                            .child(icon("close", ICON_12_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_plugin_folder(&id, &folder_path, cx)))
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    this.remove_plugin_folder(&key_id, &key_folder, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            );
        }
        if folders.len() < MAX_FOLDERS {
            let id = m.id.clone();
            let key_id = id.clone();
            row = row.child(action(format!("plugin-folder-add-{}", m.id), "plus", t("Add a folder").into(), false, p)
                .focusable().tab_index(0)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose_plugin_folder(&id, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.choose_plugin_folder(&key_id, cx);
                        cx.stop_propagation();
                    }
                })));
        }
        div().row().flex_col().gap(px(SPACE_8_PX)).child(row).child(self.folder_suggestions(&m.id, folders, p, cx)).into_any_element()
    }

    fn remove_plugin_folder(&mut self, id: &str, folder: &Path, cx: &mut Context<Self>) {
        update_config(cx, |config| {
            if let Some(grant) = config.plugins.grants.get_mut(id) { grant.folders.retain(|path| path != folder); }
        });
        cx.notify();
    }

    fn choose_plugin_folder(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.modal.folder_discovery.picker.desired().is_some() { return; }
        self.modal.clear_message();
        let permission_generation = mandara_ui::state::config(cx).read(cx).get().plugins.grants.get(id).map_or(0, |grant| grant.generation);
        self.modal.folder_discovery.picker.set(Some(Choice { plugin: id.to_string(), modal_generation: self.modal.folder_discovery.generation, permission_generation }));
        self.pump_plugin_folder_picker(cx);
    }

    fn pump_plugin_folder_picker(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.modal.folder_discovery.picker.begin() else { return };
        let allowed = mandara_ui::state::config(cx).read(cx).get().plugins.grants.get(&request.key.plugin).is_some_and(|grant| grant.generation == request.key.permission_generation && grant.capabilities.iter().any(|capability| capability == "files.folders"));
        if !allowed || request.key.modal_generation != self.modal.folder_discovery.generation || !self.modal.showing_plugin(&request.key.plugin) {
            self.modal.folder_discovery.picker.finish(request);
            self.modal.folder_discovery.picker.set(None);
            return;
        }
        let title = t("Choose a folder for this plugin");
        cx.spawn(async move |this, cx| {
            let picked = mandara_services::runtime::unblock(move || mandara_services::file_chooser::pick_folder(title, dirs::picture_dir())).await;
            let _ = this.update(cx, |this, cx| {
                let current = this.modal.folder_discovery.picker.is_current(&request);
                if current && request.key.modal_generation == this.modal.folder_discovery.generation && this.modal.showing_plugin(&request.key.plugin) {
                    match picked {
                        Ok(Some(folder)) => { update_config(cx, |config| { grant_picked_folder(config, &request.key.plugin, &folder, request.key.permission_generation); }); }
                        Ok(None) => {}
                        Err(error) => this.modal.show_error(error),
                    }
                }
                this.modal.folder_discovery.picker.finish(request);
                if current { this.modal.folder_discovery.picker.set(None); }
                this.pump_plugin_folder_picker(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn find_plugin_folders(&mut self, id: &str, cx: &mut Context<Self>) {
        let suggestions = &mut self.modal.folder_discovery;
        if suggestions.loading() && suggestions.requests.desired().is_some_and(|current| current == id) { return; }
        suggestions.requests.set(Some(id.to_string()));
        suggestions.requests.refresh();
        suggestions.folders = Vec::new();
        suggestions.searched = false;
        self.pump_folder_discovery(cx);
        cx.notify();
    }

    fn pump_folder_discovery(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.modal.folder_discovery.requests.begin() else { return };
        cx.spawn(async move |this, cx| {
            let folders = mandara_services::runtime::unblock(|| {
                dirs::home_dir().map(|home| SteamWorkshop { home: &home }.folders()).unwrap_or_default()
            }).await;
            let _ = this.update(cx, |this, cx| {
                if this.modal.folder_discovery.requests.finish(request) {
                    this.modal.folder_discovery.folders = folders;
                    this.modal.folder_discovery.searched = true;
                    cx.notify();
                }
                this.pump_folder_discovery(cx);
            });
        }).detach();
    }

    fn approve_plugin_folder(&mut self, id: &str, folder: &Path, cx: &mut Context<Self>) {
        if !self.modal.showing_plugin(id) || !self.modal.folder_discovery.folders.iter().any(|candidate| candidate == folder) { return; }
        update_config(cx, |config| { grant_folder(config, id, folder); });
        cx.notify();
    }

    fn folder_suggestions(&mut self, id: &str, granted: &[PathBuf], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.modal.folder_discovery.loading();
        let click_id = id.to_string();
        let key_id = click_id.clone();
        let label = if loading { t("Searching…") } else { t("Find Steam Workshop folders") };
        let find = action(format!("plugin-steam-find-{id}"), "search", label.into(), false, p)
            .focusable().tab_index(0)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.find_plugin_folders(&click_id, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.find_plugin_folders(&key_id, cx);
                    cx.stop_propagation();
                }
            }));
        let mut rows = div().row().flex_col().gap(px(SPACE_8_PX)).pad_start(px(SPACE_14_PX)).child(find);
        if self.modal.folder_discovery.searched && self.modal.folder_discovery.folders.is_empty() {
            rows = rows.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("No Steam Workshop folders found.")));
        }
        for (at, folder) in self.modal.folder_discovery.folders.iter().filter(|f| !granted.contains(f)).enumerate() {
            let click_id = id.to_string();
            let key_id = click_id.clone();
            let click_folder = folder.clone();
            let key_folder = click_folder.clone();
            let allow = action(format!("plugin-steam-allow-{id}-{at}"), "plus", t("Allow").into(), false, p)
                .focusable().tab_index(0)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.approve_plugin_folder(&click_id, &click_folder, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.approve_plugin_folder(&key_id, &key_folder, cx);
                        cx.stop_propagation();
                    }
                }));
            let mut row = div().row().items_center().gap(px(SPACE_8_PX))
                .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_PX)).child(mandara_ui::bidi::paragraph(folder.display().to_string())));
            if granted.len() < MAX_FOLDERS { row = row.child(allow); }
            rows = rows.child(row);
        }
        rows.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_does_not_grant_folders_without_the_current_permission() {
        let mut config = ShellConfig::default();
        let folder = Path::new("/approved/steamapps/workshop/content/431960");
        assert!(!grant_folder(&mut config, "workshop", folder));
        config.plugins.set_granted("workshop", "files.folders", true);
        assert!(grant_folder(&mut config, "workshop", folder));
        assert!(!grant_folder(&mut config, "workshop", folder));
        assert!(config.plugins.granted_folders("other").is_empty());
        config.plugins.set_granted("workshop", "files.folders", false);
        assert!(!grant_folder(&mut config, "workshop", Path::new("/another")));
    }

    #[test]
    fn closing_releases_suggestions_and_keeps_one_discovery_in_flight() {
        let mut suggestions = Suggestions::default();
        suggestions.requests.set(Some("first".into()));
        let old = suggestions.requests.begin().unwrap();
        suggestions.folders.push("/old".into());
        let generation = suggestions.generation;
        suggestions.close();
        assert!(suggestions.folders.is_empty());
        assert_ne!(suggestions.generation, generation);
        suggestions.requests.set(Some("second".into()));
        assert!(suggestions.requests.begin().is_none());
        assert!(!suggestions.requests.finish(old));
        let current = suggestions.requests.begin().unwrap();
        assert_eq!(current.key, "second");
        assert!(suggestions.requests.finish(current));
    }

    #[test]
    fn reopening_waits_for_the_previous_folder_picker_and_ignores_its_reply() {
        let mut suggestions = Suggestions::default();
        let choice = Choice { plugin: "workshop".into(), modal_generation: 0, permission_generation: 1 };
        suggestions.picker.set(Some(choice.clone()));
        let old = suggestions.picker.begin().unwrap();
        suggestions.close();
        suggestions.picker.set(Some(Choice { modal_generation: suggestions.generation, ..choice }));
        assert!(suggestions.picker.begin().is_none());
        assert!(!suggestions.picker.finish(old));
        let current = suggestions.picker.begin().unwrap();
        assert_eq!(current.key.modal_generation, suggestions.generation);
        assert!(suggestions.picker.finish(current));
    }

    #[test]
    fn folder_approval_respects_the_existing_limit() {
        let mut config = ShellConfig::default();
        config.plugins.set_granted("workshop", "files.folders", true);
        for i in 0..MAX_FOLDERS { assert!(grant_folder(&mut config, "workshop", &PathBuf::from(format!("/folder-{i}")))); }
        assert!(!grant_folder(&mut config, "workshop", Path::new("/overflow")));
    }

    #[test]
    fn a_folder_picker_cannot_complete_under_a_restored_permission() {
        let mut config = ShellConfig::default();
        config.plugins.set_granted("workshop", "files.folders", true);
        config.plugins.grants.get_mut("workshop").unwrap().generation = 3;
        let folder = Path::new("/approved");
        assert!(!grant_picked_folder(&mut config, "workshop", folder, 1));
        assert!(config.plugins.granted_folders("workshop").is_empty());
        assert!(grant_picked_folder(&mut config, "workshop", folder, 3));
    }
}
