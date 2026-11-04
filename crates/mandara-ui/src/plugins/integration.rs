//! On-demand integration data. A response belongs to the configuration
//! that requested it; an older request cannot repaint a different server.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::App;
use mandara_i18n::t;

use super::panel_fx::ViewSet;

pub(super) struct Data<K, V> {
    key: Option<K>,
    generation: u64,
    pending: Option<u64>,
    finished: Option<Instant>,
    pub value: Option<Arc<V>>,
    pub error: Option<String>,
    pub views: ViewSet,
}

impl<K, V> Default for Data<K, V> {
    fn default() -> Self {
        Self { key: None, generation: 0, pending: None, finished: None, value: None, error: None, views: ViewSet::default() }
    }
}

impl<K: PartialEq, V> Data<K, V> {
    pub fn busy(&self) -> bool { self.pending.is_some() }

    /// Clear the old server even when the new configuration is empty and
    /// will not issue a request.
    pub fn select(&mut self, key: K) {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.value = None;
            self.error = None;
            self.pending = None;
            self.finished = None;
        }
    }

    /// An explicit action changed the source. A read that began before it
    /// must not overwrite the subsequent refresh.
    pub fn invalidate(&mut self) {
        self.pending = None;
        self.finished = None;
    }

    fn begin(&mut self, key: K, force: bool, age: Duration, now: Instant) -> Option<u64> {
        self.select(key);
        if self.busy() || (!force && self.finished.is_some_and(|at| now.saturating_duration_since(at) < age)) { return None; }
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some(self.generation);
        self.error = None;
        Some(self.generation)
    }

    fn finish(&mut self, generation: u64, result: Result<V, String>, now: Instant) -> bool {
        if self.pending != Some(generation) { return false; }
        self.pending = None;
        self.finished = Some(now);
        match result {
            Ok(value) => { self.value = Some(Arc::new(value)); self.error = None; }
            Err(error) => self.error = Some(error),
        }
        true
    }
}

/// Call only from a visible panel or an explicit command. The job receives
/// the previous successful value for conditional requests and read history.
pub(super) fn load<K: PartialEq + 'static, V: Send + Sync + 'static>(
    data: &Rc<RefCell<Data<K, V>>>, key: K, force: bool, age: Duration,
    job: impl FnOnce(Option<Arc<V>>) -> Result<V, String> + Send + 'static, cx: &mut App,
) {
    let Some(generation) = data.borrow_mut().begin(key, force, age, Instant::now()) else { return };
    let previous = data.borrow().value.clone();
    let data = Rc::clone(data);
    let views = data.borrow().views.clone();
    views.notify(cx);
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(move || job(previous)).await;
        if data.borrow_mut().finish(generation, result, Instant::now()) { views.notify_async(cx); }
    }).detach();
}

pub(super) struct Action<K> {
    pending: Option<K>,
    error: Option<(K, String)>,
}

impl<K> Default for Action<K> {
    fn default() -> Self { Self { pending: None, error: None } }
}

impl<K: PartialEq> Action<K> {
    pub fn busy(&self) -> bool { self.pending.is_some() }
    pub fn error_for(&self, key: &K) -> Option<&str> { self.error.as_ref().filter(|(owner, _)| owner == key).map(|(_, error)| error.as_str()) }

    fn begin(&mut self, key: K) -> bool {
        if self.busy() { return false; }
        self.pending = Some(key);
        self.error = None;
        true
    }

    fn finish(&mut self, key: K, result: &Result<serde_json::Value, String>) {
        self.pending = None;
        self.error = result.as_ref().err().map(|error| (key, error.clone()));
    }
}

/// One explicit action at a time per integration. Both IPC and buttons use
/// the same pending reply, error state and completion refresh.
pub(super) fn act<K: Clone + PartialEq + 'static>(
    action: &Rc<RefCell<Action<K>>>, key: K, views: ViewSet,
    work: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static,
    done: impl FnOnce(&mut App) + 'static, cx: &mut App,
) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    if !action.borrow_mut().begin(key.clone()) {
        let _ = tx.try_send(Err(t("An action is still running").into()));
        return rx;
    }
    let action = Rc::clone(action);
    views.notify(cx);
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(work).await;
        action.borrow_mut().finish(key, &result);
        let _ = tx.try_send(result.map(|value| value.to_string()));
        cx.update(done);
        views.notify_async(cx);
    }).detach();
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGE: Duration = Duration::from_secs(30);

    #[test]
    fn changing_servers_discards_the_old_response_and_private_data() {
        let now = Instant::now();
        let mut data: Data<&str, String> = Data::default();
        let old = data.begin("old", false, AGE, now).unwrap();
        let new = data.begin("new", false, AGE, now).unwrap();
        assert!(!data.finish(old, Ok("old server data".into()), now));
        assert!(data.value.is_none());
        assert!(data.finish(new, Ok("new server data".into()), now));
        assert_eq!(data.value.as_deref().map(String::as_str), Some("new server data"));
    }

    #[test]
    fn a_failed_refresh_keeps_the_last_success_and_does_not_retry_each_frame() {
        let now = Instant::now();
        let mut data: Data<&str, &str> = Data::default();
        let first = data.begin("server", false, AGE, now).unwrap();
        assert!(data.begin("server", true, AGE, now).is_none(), "coalesce in-flight refreshes");
        data.finish(first, Ok("headlines"), now);
        assert!(data.begin("server", false, AGE, now).is_none());
        let retry = data.begin("server", true, AGE, now).unwrap();
        data.finish(retry, Err("offline".into()), now);
        assert_eq!(data.value.as_deref(), Some(&"headlines"));
        assert_eq!(data.error.as_deref(), Some("offline"));
        assert!(data.begin("server", false, AGE, now).is_none());
        assert!(data.begin("server", false, AGE, now + AGE).is_some());
    }

    #[test]
    fn clearing_a_server_or_finishing_an_action_rejects_earlier_reads() {
        let now = Instant::now();
        let mut data: Data<&str, &str> = Data::default();
        let before_action = data.begin("server", false, AGE, now).unwrap();
        data.invalidate();
        let after_action = data.begin("server", false, AGE, now).unwrap();
        assert!(!data.finish(before_action, Ok("old state"), now));
        assert!(data.finish(after_action, Ok("new state"), now));
        data.select("");
        assert!(data.value.is_none());
        assert!(!data.busy());
    }

    #[test]
    fn action_errors_belong_to_their_server_and_parallel_clicks_are_coalesced() {
        let mut action = Action::default();
        assert!(action.begin("first"));
        assert!(!action.begin("second"));
        action.finish("first", &Err("failed fixture".into()));
        assert_eq!(action.error_for(&"first"), Some("failed fixture"));
        assert_eq!(action.error_for(&"second"), None);
        assert!(action.begin("second"));
        assert!(action.error_for(&"first").is_none());
    }
}
