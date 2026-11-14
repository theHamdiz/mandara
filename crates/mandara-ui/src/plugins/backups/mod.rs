//! Encrypted backup history, local snapshots and reviewed storage maintenance.

mod jobs;
mod panel;
mod storage;
mod cleanup;
mod timeline;
mod commands;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

use gpui::{App, IntoElement as _, px, size};
use mandara_config::ShellConfig;
use mandara_i18n::{Lang, tk};
use mandara_plugins::{PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::backups::{Progress, Settings, Summary};
use mandara_services::backups::{MAX_PATH_BYTES, MAX_SOURCES};

use super::{integration, model_view, panel_fx::ViewSet, settings_cache::{ConfigSettings, SettingsCache}};

pub const ID: &str = "backups";
/// The three storage views share a generous, fixed panel silhouette.
const PANEL_W: f32 = 560.0;
const PANEL_H: f32 = 660.0;
const CHIP_PX: f32 = 16.0;

struct Preferences(Settings, LocalSettings);
struct LocalSettings { source: std::path::PathBuf, directory: std::path::PathBuf, cleanup_days: u64, archive_ready: bool, backup_ready: bool, format: timeline::Format }
impl ConfigSettings for Preferences {
    type Scope = ();
    fn resolve(config: &ShellConfig, language: Lang, _: ()) -> Self {
        let table = config.plugins.settings.get(ID);
        let path = |value: Option<&str>| value.filter(|value| value.len() <= MAX_PATH_BYTES).unwrap_or_default().into();
        let settings = Settings {
            repository: path(table.and_then(|table| table.get("repository")).and_then(toml::Value::as_str)),
            password_file: path(table.and_then(|table| table.get("password_file")).and_then(toml::Value::as_str)),
            sources: table.and_then(|table| table.get("sources")).and_then(toml::Value::as_array)
                .into_iter().flatten().take(MAX_SOURCES + 1).map(|value| path(value.as_str())).collect(),
        };
        let archive_ready = settings.validate().is_ok();
        let backup_ready = archive_ready && !settings.sources.is_empty();
        Self(settings, LocalSettings {
            source: path(table.and_then(|table| table.get("snapshot_source")).and_then(toml::Value::as_str)),
            directory: path(table.and_then(|table| table.get("snapshot_directory")).and_then(toml::Value::as_str)),
            cleanup_days: config.plugins.setting(ID, "cleanup_age_days", mandara_services::backups::cleanup::DEFAULT_AGE_DAYS)
                .clamp(mandara_services::backups::cleanup::AGE_DAYS_RANGE.0, mandara_services::backups::cleanup::AGE_DAYS_RANGE.1),
            archive_ready,
            backup_ready,
            format: timeline::Format::new(config, language),
        })
    }
}

type History = Rc<RefCell<integration::Data<(Settings, Lang, timeline::Format), Vec<timeline::Row>>>>;
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page { #[default] Archive, Snapshots, Cleanup, Maintenance }
struct Model {
    page: Page,
    storage: storage::State,
    cleanup: cleanup::State,
    history: History,
    history_settings: Option<(Settings, Lang, timeline::Format)>,
    history_page: usize,
    views: ViewSet,
    active: Option<Arc<AtomicBool>>,
    read_cancel: Arc<AtomicBool>,
    status: &'static str,
    progress: Arc<Mutex<Progress>>,
    summary: Option<Summary>,
    summary_scope: Option<(std::path::PathBuf, std::path::PathBuf)>,
    error: Option<String>,
    opened: bool,
    close_watch: Rc<Cell<bool>>,
    visibility: super::panel_fx::visibility::Visibility,
}
impl Default for Model {
    fn default() -> Self {
        Self { page: Page::Archive, storage: storage::State::default(), cleanup: cleanup::State::default(), history: integration::evicting(), history_settings: None, history_page: 0, views: ViewSet::default(), active: None, read_cancel: Arc::default(), status: tk("Working…"), progress: Arc::default(), summary: None, summary_scope: None, error: None, opened: false, close_watch: Rc::default(), visibility: Default::default() }
    }
}
impl Model {
    fn retain_summary_for(&mut self, settings: &Settings) {
        if self.summary_scope.as_ref().is_none_or(|(repository, password)| repository != &settings.repository || password != &settings.password_file) {
            self.summary = None;
            self.summary_scope = None;
        }
    }
    fn select_page(&mut self, page: Page) {
        if self.page == page { return; }
        self.cleanup.invalidate_review();
        match self.page {
            Page::Archive => {
                self.read_cancel.store(true, Ordering::Release);
                self.history.borrow_mut().clear();
                self.history_settings = None;
            }
            Page::Snapshots | Page::Maintenance => self.storage.clear(),
            Page::Cleanup => self.cleanup.clear(),
        }
        self.page = page;
        if page == Page::Archive { self.read_cancel = Arc::new(AtomicBool::new(false)); }
    }
}
type Shared = Rc<RefCell<Model>>;
thread_local! {
    static MODEL: RefCell<Option<Shared>> = const { RefCell::new(None) };
    static SETTINGS: RefCell<SettingsCache<Preferences>> = const { RefCell::new(SettingsCache::new()) };
}
fn model() -> Shared { MODEL.with(|slot| slot.borrow_mut().get_or_insert_with(Rc::default).clone()) }
fn preferences(cx: &App) -> Rc<Preferences> {
    let config = crate::state::config(cx).read(cx).snapshot();
    SETTINGS.with(|cache| cache.borrow_mut().get(&config, mandara_i18n::language(), ()))
}

pub(super) fn stop() {
    if let Some(model) = MODEL.with(|slot| slot.borrow_mut().take()) {
        let mut model = model.borrow_mut();
        if let Some(cancel) = model.active.take() { cancel.store(true, Ordering::Release); }
        model.read_cancel.store(true, Ordering::Release);
        model.history.borrow_mut().clear();
        model.summary = None;
        model.error = None;
        model.opened = false;
        model.visibility.clear();
        model.storage.clear();
        model.cleanup.clear();
        model.views = ViewSet::default();
    }
    SETTINGS.with(|cache| cache.borrow_mut().clear());
}

pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("Backups"), tk("Encrypted backups, local snapshots and reviewed disk cleanup"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        let shared = model();
        let bar_model = shared.clone();
        let bar = model_view(None, move |p, window, cx| {
            let model = bar_model.borrow();
            model.views.track(window);
            if model.active.is_some() { super::panel_fx::redraw_in(jobs::PROGRESS_TICK, window, cx); }
            crate::icons::icon("box", crate::style::bar_icon(CHIP_PX), if model.error.is_some() { p.warning_amber } else { p.primary }).into_any_element()
        }, window, cx);
        let context = context.clone();
        let view = model_view(None, move |p, window, cx| panel::render(&shared, &context, p, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view, size: Rc::new(Cell::new(size(px(PANEL_W), px(PANEL_H)))) }), ..PluginInstance::empty() }
    });
    commands::register(registry);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_restore_locations_survive_page_and_presentation_changes_in_the_same_repository() {
        let settings = Settings { repository: "/archive".into(), password_file: "/password".into(), sources: Vec::new() };
        let restored_to = std::path::PathBuf::from("/copies/Restored-12345678");
        let mut model = Model { summary: Some(Summary { restored_to: Some(restored_to.clone()), ..Summary::default() }), summary_scope: Some((settings.repository.clone(), settings.password_file.clone())), ..Model::default() };
        model.select_page(Page::Cleanup);
        model.select_page(Page::Archive);
        model.retain_summary_for(&settings);
        assert_eq!(model.summary.as_ref().unwrap().restored_to.as_ref(), Some(&restored_to));
        let mut changed = settings.clone();
        changed.sources.push("/other-source".into());
        model.retain_summary_for(&changed);
        assert!(model.summary.is_some());
        for changed in [Settings { repository: "/other".into(), ..settings.clone() }, Settings { password_file: "/other".into(), ..settings.clone() }] {
            model.summary = Some(Summary::default());
            model.summary_scope = Some((settings.repository.clone(), settings.password_file.clone()));
            model.retain_summary_for(&changed);
            assert!(model.summary.is_none());
            assert!(model.summary_scope.is_none());
        }
    }
    #[test]
    fn oversized_preferences_are_bounded_before_copying_them_into_a_view() {
        let mut config = ShellConfig::default();
        let table = config.plugins.settings.entry(ID.into()).or_default();
        table.insert("sources".into(), toml::Value::Array(vec![toml::Value::String("/sample".into()); MAX_SOURCES * 100]));
        table.insert("repository".into(), toml::Value::String("r".repeat(MAX_PATH_BYTES + 1)));
        let resolved = Preferences::resolve(&config, mandara_i18n::language(), ());
        assert_eq!(resolved.0.sources.len(), MAX_SOURCES + 1);
        assert!(resolved.0.repository.as_os_str().is_empty());
        assert!(resolved.0.validate().is_err());
        assert!(!resolved.1.archive_ready && !resolved.1.backup_ready);
    }

    #[test]
    fn archive_reads_and_backup_actions_have_distinct_configuration_requirements() {
        let mut config = ShellConfig::default();
        let table = config.plugins.settings.entry(ID.into()).or_default();
        table.insert("repository".into(), toml::Value::String("/archive".into()));
        table.insert("password_file".into(), toml::Value::String("/secret".into()));
        let archive = Preferences::resolve(&config, mandara_i18n::language(), ());
        assert!(archive.1.archive_ready);
        assert!(!archive.1.backup_ready);
        config.plugins.settings.get_mut(ID).unwrap().insert("sources".into(), toml::Value::Array(vec![toml::Value::String("/documents".into())]));
        assert!(Preferences::resolve(&config, mandara_i18n::language(), ()).1.backup_ready);
    }

    #[test]
    fn leaving_a_page_cancels_its_reads_and_drops_view_only_state() {
        let mut model = Model::default();
        let previous = model.read_cancel.clone();
        model.history_settings = Some((Settings::default(), mandara_i18n::language(), timeline::Format::new(&ShellConfig::default(), mandara_i18n::language())));
        model.select_page(Page::Cleanup);
        assert!(previous.load(Ordering::Acquire));
        assert!(model.history_settings.is_none());
        model.select_page(Page::Archive);
        assert!(!model.read_cancel.load(Ordering::Acquire));
    }
}
