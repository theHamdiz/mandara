//! On-demand integration data. A response belongs to the configuration
//! that requested it; an older request cannot repaint a different server.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::App;
use mandara_i18n::t;

use super::panel_fx::ViewSet;

mod reads;

struct Read<V> {
    generation: u64,
    previous: Option<Arc<V>>,
    work: Box<dyn FnOnce(Option<Arc<V>>) -> Result<V, String> + Send>,
    views: ViewSet,
}

pub(super) struct Data<K, V> {
    key: Option<K>,
    generation: u64,
    pending: Option<u64>,
    finished: Option<Instant>,
    pub value: Option<Arc<V>>,
    pub error: Option<String>,
    pub views: ViewSet,
    trim_evictions: bool,
    reads: Option<reads::Slot<Read<V>>>,
}

impl<K, V> Default for Data<K, V> {
    fn default() -> Self {
        Self { key: None, generation: 0, pending: None, finished: None, value: None, error: None, views: ViewSet::default(), trim_evictions: false, reads: None }
    }
}

impl<K: PartialEq, V> Data<K, V> {
    pub fn busy(&self) -> bool { self.pending.is_some() }

    /// Clear the old server even when the new configuration is empty and
    /// will not issue a request.
    pub fn select(&mut self, key: K) {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            let released = self.value.take().is_some();
            let queued = self.reads.as_mut().is_some_and(reads::Slot::clear);
            if (released || queued) && self.trim_evictions { mandara_services::memory::request_trim(); }
            self.error = None;
            self.pending = None;
            self.finished = None;
        }
    }

    /// Drops this cache's snapshot and subscribers when the integration stops.
    /// In-flight work may still finish, but its result is rejected. Preserve
    /// the generation across reuse so it cannot match a newly started read.
    pub fn clear(&mut self) {
        let released = self.value.take().is_some();
        let queued = self.reads.as_mut().is_some_and(reads::Slot::clear);
        self.key = None;
        self.pending = None;
        self.finished = None;
        self.error = None;
        self.views = ViewSet::default();
        if (released || queued) && self.trim_evictions { mandara_services::memory::request_trim(); }
    }

    /// An explicit action changed the source. A read that began before it
    /// must not overwrite the subsequent refresh.
    pub fn invalidate(&mut self) {
        if let Some(reads) = &mut self.reads { reads.clear(); }
        self.pending = None;
        self.finished = None;
    }

    /// Start a read only when no request is pending and its cached result is due.
    /// `force` bypasses the age check, never an existing request. The returned
    /// generation must accompany completion even if the same key is selected again.
    fn begin(&mut self, key: K, force: bool, age: Duration, now: Instant) -> Option<u64> {
        self.select(key);
        if self.busy() || (!force && self.finished.is_some_and(|at| now.saturating_duration_since(at) < age)) { return None; }
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some(self.generation);
        self.error = None;
        Some(self.generation)
    }

    /// Reject a stale generation and retain the last successful value on refresh errors.
    /// Errors also record completion time, preventing retries on every render.
    fn finish(&mut self, generation: u64, result: Result<V, String>, now: Instant) -> bool {
        if self.pending != Some(generation) {
            let released = result.is_ok();
            drop(result);
            if released && self.trim_evictions { mandara_services::memory::request_trim(); }
            return false;
        }
        self.pending = None;
        self.finished = Some(now);
        match result {
            Ok(value) => { self.value = Some(Arc::new(value)); self.error = None; }
            Err(error) => self.error = Some(error),
        }
        true
    }
}

/// View-only reads run sequentially with one replaceable successor. Eviction
/// requests allocator upkeep, including late results rejected after closing.
pub(super) fn evicting<K, V>() -> Rc<RefCell<Data<K, V>>> {
    Rc::new(RefCell::new(Data { trim_evictions: true, reads: Some(reads::Slot::default()), ..Data::default() }))
}

/// Call only from a visible panel or an explicit command. The job receives
/// the previous successful value for conditional requests and read history.
/// Its work runs off the UI thread and may outlive `clear`; only a current
/// generation may publish a result or notify the captured subscribers.
pub(super) fn load<K: PartialEq + 'static, V: Send + Sync + 'static>(
    data: &Rc<RefCell<Data<K, V>>>, key: K, force: bool, age: Duration,
    job: impl FnOnce(Option<Arc<V>>) -> Result<V, String> + Send + 'static, cx: &mut App,
) {
    let Some(generation) = data.borrow_mut().begin(key, force, age, Instant::now()) else { return };
    let previous = data.borrow().value.clone();
    if data.borrow().reads.is_some() {
        let views = data.borrow().views.clone();
        let read = Read { generation, previous, work: Box::new(job), views: views.clone() };
        let next = data.borrow_mut().reads.as_mut().and_then(|slot| slot.push(read));
        views.notify(cx);
        if let Some(read) = next { run_bounded(data.clone(), read, cx); }
        return;
    }
    let data = Rc::clone(data);
    let views = data.borrow().views.clone();
    views.notify(cx);
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(move || job(previous)).await;
        if data.borrow_mut().finish(generation, result, Instant::now()) { views.notify_async(cx); }
    }).detach();
}

fn run_bounded<K: PartialEq + 'static, V: Send + Sync + 'static>(
    data: Rc<RefCell<Data<K, V>>>, read: Read<V>, cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let Read { generation, previous, work, views } = read;
        let result = mandara_services::runtime::unblock(move || work(previous)).await;
        let (changed, next) = {
            let mut state = data.borrow_mut();
            let changed = state.finish(generation, result, Instant::now());
            let next = state.reads.as_mut().and_then(reads::Slot::complete);
            (changed, next)
        };
        if changed { views.notify_async(cx); }
        if let Some(read) = next { cx.update(|cx| run_bounded(data, read, cx)); }
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

    /// Allow one explicit action at a time for this integration.
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
    fn view_only_eviction_drops_current_and_late_snapshots_without_restoring_the_cache() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Snapshot(Arc<AtomicUsize>);
        impl Drop for Snapshot { fn drop(&mut self) { self.0.fetch_add(1, Ordering::Relaxed); } }
        let dropped = Arc::new(AtomicUsize::new(0));
        let data = evicting::<(), Snapshot>();
        let now = Instant::now();
        let first = data.borrow_mut().begin((), false, AGE, now).unwrap();
        assert!(data.borrow_mut().finish(first, Ok(Snapshot(dropped.clone())), now));
        let late = data.borrow_mut().begin((), true, AGE, now).unwrap();
        data.borrow_mut().clear();
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(!data.borrow_mut().finish(late, Ok(Snapshot(dropped.clone())), now));
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert!(data.borrow().value.is_none());
    }

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
    fn disabling_releases_cached_data_and_rejects_late_responses_after_reenable() {
        let now = Instant::now();
        let mut data: Data<&str, String> = Data::default();
        let first = data.begin("server", false, AGE, now).unwrap();
        data.finish(first, Ok("cached headlines".into()), now);
        let cached = Arc::downgrade(data.value.as_ref().unwrap());
        assert!(data.begin("server", false, AGE, now).is_none(), "enabled caches stay warm");
        let stale = data.begin("server", true, AGE, now).unwrap();
        data.clear();
        assert!(cached.upgrade().is_none(), "disabled snapshots are released");
        assert!(!data.busy());
        assert!(!data.finish(stale, Ok("late disabled response".into()), now));
        let fresh = data.begin("server", false, AGE, now).unwrap();
        assert_ne!(fresh, stale);
        assert!(!data.finish(stale, Ok("late response after reenable".into()), now));
        assert!(data.finish(fresh, Ok("fresh response".into()), now));
        assert_eq!(data.value.as_deref().map(String::as_str), Some("fresh response"));
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
