//! Lazy usage history with one reader and a coalesced, ordered disk writer.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::sync::{Arc, LazyLock, Mutex};

use super::{Usage, UsageEntry};

/// Usage history is a small score table, not a general document store.
const MAX_HISTORY_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Default)]
struct State {
    usage: Usage,
    pending: BTreeMap<String, Pending>,
    ready: Option<async_channel::Receiver<()>>,
    loaded: bool,
    reset: bool,
}

/// Pending launches compose an affine transform of the score on disk. This
/// preserves every launch and half-life edit while the initial read is blocked.
struct Pending {
    first: i64,
    half: f32,
    last: i64,
    multiplier: f64,
    addition: f64,
}

impl State {
    fn record(&mut self, key: &str, at: i64, half: f32) {
        if key.len() > super::MAX_USAGE_KEY_BYTES { return; }
        if !self.loaded {
            match self.pending.entry(key.into()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Pending {
                        first: at,
                        half,
                        last: at,
                        multiplier: 1.0,
                        addition: 1.0,
                    });
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let pending = entry.get_mut();
                    let factor = super::decay(1.0, at - pending.last, half);
                    pending.multiplier *= factor;
                    pending.addition = pending.addition * factor + 1.0;
                    pending.last = at;
                }
            }
        }
        self.usage.record(key, at, half);
        self.usage.prune(at, half);
        self.pending.retain(|key, _| self.usage.entries.contains_key(key));
    }

    fn install(&mut self, mut history: Usage) -> bool {
        let dirty = self.reset || !self.pending.is_empty();
        if !self.reset {
            for (key, pending) in std::mem::take(&mut self.pending) {
                let score = history.score(&key, pending.first, pending.half) * pending.multiplier
                    + pending.addition;
                history.entries.insert(
                    key,
                    UsageEntry {
                        score,
                        last: pending.last,
                    },
                );
            }
            self.usage = history;
        }
        self.pending.clear();
        self.usage.bound();
        self.loaded = true;
        dirty
    }

    fn reset(&mut self) -> usize {
        let count = self.usage.entries.len();
        self.usage = Usage::default();
        self.pending.clear();
        self.reset = true;
        count
    }
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

pub(super) fn ready() -> async_channel::Receiver<()> {
    let mut state = STATE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(ready) = &state.ready {
        return ready.clone();
    }
    let (done, receive) = async_channel::bounded(1);
    state.ready = Some(receive.clone());
    mandara_services::runtime::spawn("launcher-usage-reader", async move {
        mandara_services::runtime::unblock(|| initialize(&STATE, || read_file(&super::usage_path()), |snapshot| WRITER.submit(snapshot))).await;
        drop(done);
    });
    receive
}

fn initialize(state: &Mutex<State>, load: impl FnOnce() -> Usage, submit: impl FnOnce(Arc<Usage>)) {
    let mut history = load();
    history.bound();
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    if state.install(history) {
        submit(Arc::new(state.usage.clone()));
    }
}

pub(super) fn read_file(path: &std::path::Path) -> Usage {
    let mut text = String::new();
    let result = std::fs::File::open(path)
        .and_then(|file| file.take(MAX_HISTORY_BYTES + 1).read_to_string(&mut text));
    if result.is_err() || text.len() as u64 > MAX_HISTORY_BYTES {
        return Usage::default();
    }
    toml::from_str(&text).unwrap_or_default()
}

pub(super) fn read<T>(read: impl FnOnce(&Usage) -> T) -> T {
    ready();
    read(
        &STATE
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .usage,
    )
}

pub(super) fn record(key: &str, at: i64, half: f32) {
    ready();
    let mut state = STATE.lock().unwrap_or_else(|error| error.into_inner());
    state.record(key, at, half);
    if state.loaded {
        WRITER.submit(Arc::new(state.usage.clone()));
    }
}

pub(super) fn reset() -> usize {
    ready();
    let mut state = STATE.lock().unwrap_or_else(|error| error.into_inner());
    let count = state.reset();
    if state.loaded {
        WRITER.submit(Arc::new(state.usage.clone()));
    }
    count
}

struct Writer(async_channel::Sender<Arc<Usage>>);
static WRITER: LazyLock<Writer> = LazyLock::new(|| Writer::new(super::usage_path()));
impl Writer {
    fn new(path: std::path::PathBuf) -> Self {
        let (sender, receiver) = async_channel::bounded::<Arc<Usage>>(1);
        mandara_services::runtime::spawn("launcher-usage-writer", async move {
            while let Ok(snapshot) = receiver.recv().await {
                let path = path.clone();
                let result = mandara_services::runtime::unblock(move || {
                    let text = toml::to_string(snapshot.as_ref())?;
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    mandara_config::write_atomic(&path, text.as_bytes())?;
                    Ok::<_, anyhow::Error>(())
                })
                .await;
                if let Err(error) = result {
                    tracing::warn!(%error, "could not save launcher usage");
                }
            }
        });
        Self(sender)
    }
    fn submit(&self, usage: Arc<Usage>) {
        let _ = self.0.force_send(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_history_is_prepared_without_holding_the_ui_state_lock() {
        let state = Mutex::new(State::default());
        let (loading, started) = std::sync::mpsc::channel();
        let (resume, proceed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let shared = &state;
            let worker = scope.spawn(move || initialize(shared, || {
                loading.send(()).unwrap();
                proceed.recv().unwrap();
                let mut history = Usage::default();
                for index in 0..32_000 {
                    history.entries.insert(format!("app:{index}"), UsageEntry { score: 1.0, last: index });
                }
                history
            }, |_| panic!("an unchanged history must not be submitted")));
            started.recv().unwrap();
            assert!(state.try_lock().is_ok(), "UI access stays available during import");
            resume.send(()).unwrap();
            worker.join().unwrap();
        });
        let state = state.lock().unwrap();
        assert_eq!(state.usage.entries.len(), super::super::MAX_USAGE_ENTRIES);
        assert_eq!(state.usage.entries.values().map(|entry| entry.last).min(), Some(32_000 - super::super::MAX_USAGE_ENTRIES as i64));
    }

    #[test]
    fn a_launch_after_initialization_cannot_be_overwritten_by_its_older_snapshot() {
        let mut initial = State::default();
        initial.record("first", 1, 7.0);
        let state = Mutex::new(initial);
        let (sender, receiver) = async_channel::bounded(1);
        let writer = Writer(sender);
        let (submitting, submitted) = std::sync::mpsc::channel();
        let (resume, proceed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let state = &state;
            let writer = &writer;
            let reader = scope.spawn(move || initialize(state, Usage::default, |snapshot| {
                submitting.send(()).unwrap();
                proceed.recv().unwrap();
                writer.submit(snapshot);
            }));
            submitted.recv().unwrap();
            let launch = scope.spawn(move || {
                let mut state = state.lock().unwrap();
                state.record("second", 2, 7.0);
                writer.submit(Arc::new(state.usage.clone()));
            });
            assert!(state.try_lock().is_err(), "submission keeps the state-update order");
            resume.send(()).unwrap();
            reader.join().unwrap();
            launch.join().unwrap();
        });
        let latest = receiver.try_recv().unwrap();
        assert!(receiver.try_recv().is_err());
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("usage.toml");
        mandara_config::write_atomic(&path, toml::to_string(latest.as_ref()).unwrap().as_bytes()).unwrap();
        let saved = read_file(&path);
        assert!(saved.entries.contains_key("first"));
        assert!(saved.entries.contains_key("second"));
    }

    #[test]
    fn launches_during_loading_match_serial_usage_across_half_life_edits() {
        let mut history = Usage::default();
        let now = super::super::now();
        history.record("app:a", now - 400, 7.0);
        let mut expected = history.clone();
        let mut state = State::default();
        for (at, half) in [(now - 300, 7.0), (now - 200, 3.0), (now - 100, 9.0)] {
            expected.record("app:a", at, half);
            state.record("app:a", at, half);
        }
        assert!(state.install(history));
        let actual = &state.usage.entries["app:a"];
        assert!((actual.score - expected.entries["app:a"].score).abs() < f64::EPSILON * 8.0);
        assert_eq!(actual.last, now - 100);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn resetting_during_loading_cannot_restore_old_history() {
        let mut history = Usage::default();
        let now = super::super::now();
        history.record("old", now - 300, 7.0);
        let mut state = State::default();
        state.record("before-reset", now - 200, 7.0);
        assert_eq!(state.reset(), 1);
        state.record("after-reset", now - 100, 7.0);
        state.install(history);
        assert_eq!(
            state
                .usage
                .entries
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["after-reset"]
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn stalled_history_reads_keep_pending_launch_storage_bounded() {
        let mut state = State::default();
        let now = super::super::now();
        for index in 0..super::super::MAX_USAGE_ENTRIES + 64 {
            state.record(&format!("file:{index}"), now + index as i64, 7.0);
        }
        state.record(&"x".repeat(super::super::MAX_USAGE_KEY_BYTES + 1), now, 7.0);
        assert_eq!(state.pending.len(), super::super::MAX_USAGE_ENTRIES);
        assert_eq!(state.usage.entries.len(), super::super::MAX_USAGE_ENTRIES);
    }

    #[test]
    fn reading_does_not_rewrite_an_existing_history() {
        let mut state = State::default();
        assert!(!state.install(Usage::default()));
    }
}
