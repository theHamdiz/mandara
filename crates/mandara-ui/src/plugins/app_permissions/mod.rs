//! Per-app sandbox grants, with scoped personal overrides and a single reversible change.

mod panel;
mod rows;
mod actions;
mod commands;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

use gpui::{App, Entity, IntoElement as _, px, size};
use mandara_i18n::tk;
use mandara_plugins::{PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::app_permissions::{App as InstalledApp, Backend as _, Capability, Flatpak, Session, Undo};

use super::{integration, model_view, panel_fx::ViewSet};

pub const ID: &str = "app-permissions";
/// Space for an app shelf and a focused permission detail view.
const PANEL_W: f32 = 580.0;
const PANEL_H: f32 = 680.0;
const CHIP_PX: f32 = 16.0;
/// Renders do not rescan; each opening or explicit Refresh starts one read.
const CACHE_SECONDS: u64 = 24 * 60 * 60;
const CACHE_AGE: Duration = Duration::from_secs(CACHE_SECONDS);

type Data<K, V> = Rc<RefCell<integration::Data<K, V>>>;
struct Model {
    apps: Data<(), Vec<rows::Row>>,
    session: Data<InstalledApp, Session>,
    selected: Option<InstalledApp>,
    undo: Option<Undo>,
    active: Option<Arc<AtomicBool>>,
    read_cancel: Arc<AtomicBool>,
    error: Option<String>,
    changed: Option<InstalledApp>,
    search: Option<Entity<crate::text_field::TextField>>,
    query: String,
    page: usize,
    opened: bool,
    details: bool,
    views: ViewSet,
    close_watch: Rc<Cell<bool>>,
    visibility: super::panel_fx::visibility::Visibility,
}
impl Default for Model {
    fn default() -> Self {
        Self { apps: integration::evicting(), session: integration::evicting(), selected: None, undo: None, active: None, read_cancel: Arc::default(), error: None, changed: None, search: None, query: String::new(), page: 0, opened: false, details: false, views: ViewSet::default(), close_watch: Rc::default(), visibility: Default::default() }
    }
}
impl Model {
    fn cancel_reads(&mut self) {
        self.read_cancel.store(true, Ordering::Release);
        self.apps.borrow_mut().invalidate();
        self.session.borrow_mut().invalidate();
    }
    fn renew_reads(&mut self) -> Arc<AtomicBool> {
        self.cancel_reads();
        self.read_cancel = Arc::new(AtomicBool::new(false));
        self.read_cancel.clone()
    }
}
type Shared = Rc<RefCell<Model>>;
thread_local! { static MODEL: RefCell<Option<Shared>> = const { RefCell::new(None) }; }
fn model() -> Shared { MODEL.with(|slot| slot.borrow_mut().get_or_insert_with(Rc::default).clone()) }

pub(super) fn stop() {
    if let Some(model) = MODEL.with(|slot| slot.borrow_mut().take()) {
        let mut model = model.borrow_mut();
        model.read_cancel.store(true, Ordering::Release);
        if let Some(cancel) = model.active.take() { cancel.store(true, Ordering::Release); }
        model.apps.borrow_mut().clear();
        model.session.borrow_mut().clear();
        model.selected = None;
        model.search = None;
        model.query = String::new();
        model.undo = None;
        model.opened = false;
        model.visibility.clear();
        model.views = ViewSet::default();
    }
}

fn refresh(model: &Shared, force: bool, cx: &mut App) {
    let mut state = model.borrow_mut();
    if state.active.is_some() { return; }
    let cancel = state.renew_reads();
    integration::load(&state.apps, (), force, CACHE_AGE, move |_| Flatpak::default().apps(&cancel).map(|apps| apps.into_iter().map(rows::Row::new).collect()).map_err(|error| format!("{error:#}")), cx);
}

fn select(model: &Shared, app: InstalledApp, cx: &mut App) {
    let session = {
        let mut state = model.borrow_mut();
        if state.active.is_some() { return; }
        state.renew_reads();
        if state.selected.as_ref() != Some(&app) { state.error = None; }
        state.selected = Some(app.clone());
        state.details = false;
        state.session.clone()
    };
    let cancel = model.borrow().read_cancel.clone();
    integration::load(&session, app.clone(), true, CACHE_AGE, move |_| Flatpak::default().inspect(&app, &cancel).map_err(|error| format!("{error:#}")), cx);
    model.borrow().views.notify(cx);
}

use actions::{change, undo};

pub fn register(registry: &mut PluginRegistry) {
    registry.register(PluginManifest::builtin(ID, tk("App permissions"), tk("Review Flatpak app access and change personal sandbox grants with Undo"), Some(Zone::Right)).opt_in(), |window, cx, context| {
        let shared = model();
        let bar = model_view(None, |p, _, _| crate::icons::icon("shield", crate::style::bar_icon(CHIP_PX), p.primary).into_any_element(), window, cx);
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
    fn refresh_renews_a_cancelled_token_and_invalidates_pending_reads() {
        let mut model = Model::default();
        let previous = model.read_cancel.clone();
        previous.store(true, Ordering::Release);
        let refreshed = model.renew_reads();
        assert!(previous.load(Ordering::Acquire));
        assert!(!refreshed.load(Ordering::Acquire));
        let next = model.renew_reads();
        assert!(refreshed.load(Ordering::Acquire));
        assert!(!next.load(Ordering::Acquire));
        assert!(!model.apps.borrow().busy());
        assert!(!model.session.borrow().busy());
    }
}
