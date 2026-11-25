//! A transient chat: one owned stream, a bounded transcript and no background polling.

use crate::kit::tokens;
use crate::plugins::integration::{self, Data};
use crate::plugins::panel_fx::visibility::Visibility;
use crate::plugins::panel_fx::ViewSet;
use crate::text_field::TextField;
use gpui::{AnyElement, App, AppContext as _, Entity, SharedString, Window};
use mandara_i18n::t;
use mandara_plugins::PanelCx;
use mandara_services::local_ai::chat::{
    self as service, Message, Role, Session, Settings,
};
use mandara_theme::MandaraPalette;
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The transcript scrolls independently from the composer.
pub(super) const WIDTH: f32 = 540.0;
pub(super) const HEIGHT: f32 = 620.0;
/// A server's available models change only after an explicit refresh or a reopen.
const CATALOG_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// A model search needs a short name, never an entire pasted prompt.
const MAX_QUERY_BYTES: usize = 256;

#[derive(Default)]
struct Progress {
    text: String,
    shown: SharedString,
    finished: Option<Result<(), String>>,
}

impl Progress {
    /// The worker only appends; unrelated frames reuse the last displayed text.
    fn display(&mut self) -> SharedString {
        if self.shown.len() != self.text.len() { self.shown = self.text.as_str().into(); }
        self.shown.clone()
    }
}

mod view;

#[derive(Default)]
struct Model {
    settings: Option<Arc<Settings>>,
    history: Vec<Message>,
    progress: Option<Arc<Mutex<Progress>>>,
    session: Option<Session>,
    running: bool,
    generation: u64,
    session_id: String,
    models: bool,
    picker: bool,
    selected: Option<SharedString>,
    error: Option<String>,
    catalog: Rc<RefCell<Data<Arc<Settings>, Vec<CatalogName>>>>,
    views: ViewSet,
    visibility: Visibility,
    watch: Rc<Cell<bool>>,
}

impl Model {
    fn finish(&mut self, progress: &mut Progress, model: &str, cancelled: bool) -> Result<String, String> {
        let outcome = progress.finished.take().unwrap_or_else(|| Err(t("The model returned no text").into()));
        let reply = if cancelled {
            Err(t("Cancelled").into())
        } else {
            outcome.and_then(|()| {
                if progress.text.is_empty() { Err(t("The model returned no text").into()) }
                else { Ok(serde_json::json!({"model":model,"reply":progress.text}).to_string()) }
            })
        };
        if !progress.text.is_empty() {
            self.history.push(Message { role: Role::Assistant, content: std::mem::take(&mut progress.text).into() });
        }
        self.error = if cancelled { None } else { reply.as_ref().err().cloned() };
        if let Some(settings) = self.settings.as_ref() { service::trim(&mut self.history, settings.context_bytes); }
        reply
    }
}

type Shared = Rc<RefCell<Model>>;
thread_local! { static MODEL: OnceCell<Shared> = const { OnceCell::new() }; }
fn model() -> Shared {
    MODEL.with(|cell| cell.get_or_init(|| Rc::new(RefCell::new(Model { catalog: integration::evicting(), ..Model::default() }))).clone())
}

struct CatalogName {
    label: SharedString,
    folded: Box<str>,
}
impl CatalogName {
    fn new(name: String) -> Self {
        Self { folded: name.to_lowercase().into_boxed_str(), label: name.into() }
    }
}

fn notify(model: &Shared, cx: &mut App) {
    model.borrow().views.notify(cx);
}

/// Follow streamed text while the reader stays at the end; preserve a scroll
/// back through earlier answers instead of pulling it down on every chunk.
fn following_tail(offset: f32, maximum: f32) -> bool { offset + maximum <= tokens::SM }

fn load_catalog(model: &Shared, settings: Arc<Settings>, force: bool, cx: &mut App) {
    let catalog = model.borrow().catalog.clone();
    let request = settings.clone();
    integration::load(&catalog, settings, force, CATALOG_AGE, move |_| {
        let key = super::api_key(&request)?;
        service::catalog(&request, key.as_ref()).map(|names| names.into_iter().map(CatalogName::new).collect())
    }, cx);
}

fn clear(model: &mut Model) {
    if let Some(session) = model.session.take() {
        session.cancel();
    }
    model.generation = model.generation.wrapping_add(1);
    model.history = Vec::new();
    model.progress = None;
    model.error = None;
    model.session_id.clear();
    model.session_id.shrink_to_fit();
    model.picker = false;
    model.catalog.borrow_mut().clear();
    // Keep `running` until the cancelled worker returns. Reopening cannot multiply workers.
}

pub(super) fn discard() -> bool {
    MODEL.with(|cell| {
        let Some(model) = cell.get() else { return false };
        let mut state = model.borrow_mut();
        clear(&mut state);
        state.settings = None;
        state.selected = None;
        state.visibility.clear();
        state.views = ViewSet::default();
        true
    })
}

pub(super) struct State {
    input: Entity<TextField>,
    search: Entity<TextField>,
    query: Rc<RefCell<String>>,
    watch: Rc<Cell<bool>>,
    scroll: gpui::ScrollHandle,
}
impl State {
    pub fn new(cx: &mut App) -> Self {
        let shared = model();
        let weak = Rc::downgrade(&shared);
        let input = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Write a message…"),
                true,
                Rc::new(move |_, _, cx| {
                    if let Some(model) = weak.upgrade() {
                        notify(&model, cx);
                    }
                }),
                cx,
            )
            .multiline()
            .design(crate::components::InputDesign::Filled)
            .embedded()
            .parent_commits()
            .max_bytes(service::MAX_PROMPT_BYTES)
        });
        let weak = Rc::downgrade(&shared);
        let query: Rc<RefCell<String>> = Rc::default();
        let search_query = query.clone();
        let search = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search models or workspaces"),
                true,
                Rc::new(move |value, _, cx| {
                    *search_query.borrow_mut() = value.to_lowercase();
                    if let Some(model) = weak.upgrade() {
                        notify(&model, cx);
                    }
                }),
                cx,
            )
            .max_bytes(MAX_QUERY_BYTES)
        });
        Self {
            input,
            search,
            query,
            watch: Rc::default(),
            scroll: gpui::ScrollHandle::new(),
        }
    }
}

fn submit(model: &Shared, input: &Entity<TextField>, cx: &mut App) {
    let text = input.read(cx).value.trim().to_owned();
    match send(text, cx) {
        Ok(_) => input.update(cx, |input, cx| {
            input.set("");
            cx.notify();
        }),
        Err(error) => {
            model.borrow_mut().error = Some(error);
            notify(model, cx);
        }
    }
}

pub(super) fn stop(cx: &mut App) -> bool {
    let shared = model();
    let stopped = if let Some(session) = &shared.borrow().session {
        session.cancel();
        true
    } else {
        false
    };
    notify(&shared, cx);
    stopped
}

pub(super) fn reset(cx: &mut App) {
    let shared = model();
    clear(&mut shared.borrow_mut());
    notify(&shared, cx);
}

/// UI and IPC share the same gate; command replies never create a second stream.
pub(super) fn send(
    text: String,
    cx: &mut App,
) -> Result<async_channel::Receiver<Result<String, String>>, String> {
    let model = model();
    if model.borrow().running {
        return Err(t("Wait for the current reply or press Stop").into());
    }
    let preferences = super::cached_settings(&crate::state::config(cx).read(cx).snapshot());
    if model.borrow().settings.as_ref() != Some(&preferences) {
        let mut state = model.borrow_mut();
        clear(&mut state);
        state.selected = Some(preferences.model.as_str().into());
        state.settings = Some(preferences.clone());
    }
    let mut settings = (*preferences).clone();
    if let Some(selected) = &model.borrow().selected { settings.model = selected.to_string(); }
    if text.trim().is_empty() {
        return Err(t("Enter a message").into());
    }
    if text.len() > service::MAX_PROMPT_BYTES {
        return Err(t("Keep a message under 8 KiB").into());
    }
    if settings.model.is_empty() {
        return Err(t("Choose a model or workspace in Studio").into());
    }
    let session_id = if model.borrow().session_id.is_empty() {
        service::session_id()?
    } else {
        model.borrow().session_id.clone()
    };
    let reply_model = settings.model.clone();
    let (generation, messages, session, progress) = {
        let mut state = model.borrow_mut();
        state.error = None;
        state.picker = false;
        state.session_id = session_id.clone();
        // A failed turn has no assistant answer to include in the next request.
        if state
            .history
            .last()
            .is_some_and(|message| message.role == Role::User)
        {
            state.history.pop();
        }
        state.history.push(Message {
            role: Role::User,
            content: text.into(),
        });
        service::trim(&mut state.history, settings.context_bytes);
        state.running = true;
        let session = Session::default();
        let progress = Arc::new(Mutex::new(Progress::default()));
        state.session = Some(session.clone());
        state.progress = Some(progress.clone());
        (state.generation, state.history.clone(), session, progress)
    };
    let (tx, rx) = async_channel::bounded(1);
    let (reply_tx, reply_rx) = async_channel::bounded(1);
    let worker_progress = progress.clone();
    mandara_services::runtime::spawn("local-ai-chat", async move {
        let wake = tx.clone();
        let result = mandara_services::runtime::unblock(move || {
            if session.cancelled() { return Ok(()); }
            let key = super::api_key(&settings)?;
            session.chat(&settings, &messages, &session_id, key.as_ref(), |text| {
                if session.cancelled() {
                    return false;
                }
                worker_progress
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .text
                    .push_str(text);
                let _ = wake.try_send(());
                true
            })
        })
        .await;
        progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finished = Some(result);
        let _ = tx.try_send(());
    });
    let weak = Rc::downgrade(&model);
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            let Some(model) = weak.upgrade() else { return };
            cx.update(|cx| notify(&model, cx));
        }
        let Some(model) = weak.upgrade() else { return };
        let mut reply = Err(t("Cancelled").to_owned());
        {
            let mut state = model.borrow_mut();
            state.running = false;
            let cancelled = state
                .session
                .take()
                .is_some_and(|session| session.cancelled());
            if state.generation == generation
                && let Some(progress) = state.progress.take()
            {
                let mut progress = progress.lock().unwrap_or_else(|error| error.into_inner());
                reply = state.finish(&mut progress, &reply_model, cancelled);
            }
        }
        let _ = reply_tx.try_send(reply);
        cx.update(|cx| notify(&model, cx));
    })
    .detach();
    notify(&model, cx);
    Ok(reply_rx)
}

pub(super) fn show_models() -> bool {
    model().borrow().models
}
pub(super) fn open_chat(cx: &mut App) {
    let model = model();
    model.borrow_mut().models = false;
    super::evict();
    mandara_services::memory::request_trim();
    notify(&model, cx);
}

/// Called for both tabs so the last panel owns cache eviction and cancellation.
pub(super) fn track(state: &State, context: &PanelCx, window: &mut Window, cx: &mut App) {
    let shared = model();
    let input = state.input.downgrade();
    let search = state.search.downgrade();
    let query = state.query.clone();
    crate::plugins::panel_fx::visibility::on_close(context, &state.watch, cx, move |cx| {
        *query.borrow_mut() = String::new();
        for field in [input, search]
            .into_iter()
            .filter_map(|field| field.upgrade())
        {
            field.update(cx, |field, cx| {
                field.set("");
                cx.notify();
            });
        }
    });
    shared.borrow().views.track(window);
    shared.borrow().visibility.track(context);
    let watch = shared.borrow().watch.clone();
    let close = shared.clone();
    shared
        .borrow()
        .visibility
        .on_last_close(&watch, cx, move |_| {
            {
                let mut state = close.borrow_mut();
                clear(&mut state);
                state.visibility.clear();
            }
            super::evict();
            mandara_services::memory::request_trim();
        });
}

pub(super) fn render(state: &State, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    view::render(state, palette, window, cx)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopping_keeps_partial_text_and_the_command_result_without_a_false_ui_error() {
        let mut state = Model::default();
        state.history.push(Message { role: Role::User, content: "Question".into() });
        let mut progress = Progress { text: "Partial reply".into(), finished: Some(Err("connection interrupted".into())), ..Progress::default() };
        assert_eq!(state.finish(&mut progress, "fixture", true).unwrap_err(), t("Cancelled"));
        assert!(state.error.is_none());
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.history[1].role, Role::Assistant);
        assert_eq!(state.history[1].content.as_ref(), "Partial reply");
        let mut failed = Progress { finished: Some(Err("server unavailable".into())), ..Progress::default() };
        assert!(state.finish(&mut failed, "fixture", false).is_err());
        assert_eq!(state.error.as_deref(), Some("server unavailable"));
    }
    #[test]
    fn disabling_releases_chat_data_and_cancels_without_multiplying_workers() {
        let shared = model();
        let session = Session::default();
        { let mut state = shared.borrow_mut(); state.running = true; state.session = Some(session.clone());
          state.history.push(Message { role: Role::User, content: "private".into() });
          state.selected = Some("model".into()); }
        assert!(discard());
        assert!(session.cancelled());
        let state = shared.borrow();
        assert!(state.running && state.history.is_empty());
        assert!(state.settings.is_none() && state.selected.is_none());
        assert_eq!(state.history.capacity(), 0);
    }
    #[test]
    fn catalog_search_is_prepared_once_for_unicode_names() {
        let name = CatalogName::new("MODÈLE-東京".into());
        assert_eq!(name.label, "MODÈLE-東京");
        assert!(name.folded.contains("modèle"));
        assert!(name.folded.contains("東京"));
    }
    #[test]
    fn unchanged_stream_frames_share_the_display_buffer() {
        let mut progress = Progress { text: "a".repeat(100), ..Progress::default() };
        let first = progress.display();
        assert_eq!(first.as_ptr(), progress.display().as_ptr());
        progress.text.push_str("next");
        let next = progress.display();
        assert_ne!(first.as_ptr(), next.as_ptr());
        assert_eq!(next.len(), 104);
        assert_eq!(next.as_ptr(), progress.display().as_ptr());
    }
    #[test]
    fn streamed_text_follows_the_tail_without_overriding_a_manual_scroll_back() {
        assert!(following_tail(0.0, 0.0));
        assert!(following_tail(-100.0, 100.0));
        assert!(following_tail(-96.0, 100.0));
        assert!(!following_tail(-50.0, 100.0));
        assert!(!following_tail(0.0, 100.0));
    }
    #[test]
    fn closing_drops_the_transcript_and_cancels_the_stream_without_releasing_the_worker_gate() {
        let mut model = Model {
            running: true,
            session: Some(Session::default()),
            history: vec![Message {
                role: Role::User,
                content: "private".into(),
            }],
            session_id: "previous-chat".into(),
            selected: Some("chosen-model".into()),
            ..Model::default()
        };
        let session = model.session.clone().unwrap();
        clear(&mut model);
        assert!(session.cancelled());
        assert!(model.running);
        assert!(
            model.history.is_empty() && model.progress.is_none() && model.session_id.is_empty()
        );
        assert_eq!(model.history.capacity(), 0);
        assert_eq!(model.selected.as_deref(), Some("chosen-model"), "keep the small model choice, not the transcript");
    }
}
