//! Compact prepared labels and ranking, independent of Studio's element tree.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::SharedString;
use mandara_i18n::Lang;
use mandara_ui::launcher_ext::Usage;

/// The existing ranking card shows only its eight most-used results.
pub(in crate::app) const USAGE_ROWS: usize = 8;
/// Frecency is formatted to a tenth; check at most once a second near a
/// rounding boundary and at least once a minute while the page is visible.
const MIN_REFRESH: f64 = 1.0;
const MAX_REFRESH: f64 = 60.0;
const SECONDS_PER_DAY: f64 = 86_400.0;

/// The settings a prepared snapshot depends on: language, usage half-life
/// (stored as its bits, so equality is exact) and icon theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Key {
    pub language: Lang,
    half_life: u32,
    pub icon_theme: Option<Arc<str>>,
}

impl Key {
    /// A key for these settings (allocates the icon theme name once).
    pub fn new(language: Lang, half_life: f32, icon_theme: Option<&str>) -> Self { Self { language, half_life: half_life.to_bits(), icon_theme: icon_theme.map(Arc::from) } }
    /// Compares with borrowed settings, so an unchanged frame allocates nothing.
    pub fn matches(&self, language: Lang, half_life: f32, icon_theme: Option<&str>) -> bool {
        self.language == language && self.half_life == half_life.to_bits() && self.icon_theme.as_deref() == icon_theme
    }
    /// The half-life in days, at least 0.1 (so ageing never divides by zero).
    fn days(&self) -> f64 { f64::from(f32::from_bits(self.half_life).max(0.1)) }
}

/// One source instance fixes these paths for its visit. A new visit reads
/// current XDG roots; filesystem revisions identify changes within them.
#[derive(Clone, Debug)]
pub(super) struct Roots {
    pub applications: Vec<PathBuf>,
    pub usage: PathBuf,
    pub preferences: Vec<PathBuf>,
}

/// Where prepared data comes from: the real files, or a fixture in tests.
/// Every call runs on the blocking worker.
pub(super) trait Source: Send + Sync + 'static {
    /// The paths to watch for changes.
    fn roots(&self) -> &Roots;
    /// The installed apps, in the key's language and icon theme.
    fn catalog(&self, key: &Key) -> Vec<mandara_ui::apps::App>;
    /// The launch history.
    fn usage(&self) -> Usage;
}

/// Only metadata actually used by the page survives discovery; executable,
/// comments, keyword indices and desktop actions are dropped on the worker.
pub(in crate::app) struct AppInfo {
    pub id: String,
    pub name: SharedString,
    pub icon: Option<SharedString>,
}

/// One ranking row: its label, current score and that score as shown.
pub(in crate::app) struct Row {
    pub label: SharedString,
    pub score: f64,
    pub display: SharedString,
}

/// What the page draws, shared by `Arc` (never copied per frame), tagged
/// with the key and revisions it was made for.
pub(in crate::app) struct Snapshot {
    pub(super) key: Key,
    pub(super) revisions: (u64, u64),
    pub apps: Arc<[AppInfo]>,
    pub rows: Arc<[Row]>,
}

impl Snapshot {
    /// Match ids before names, in discovery order, just like the former
    /// per-row scan. A borrowed result never clones the whole catalog.
    pub fn resolve(&self, name: &str) -> Option<&AppInfo> { resolve(&self.apps, name) }
}

/// The app whose id, then whose name, equals `name` (ASCII case ignored).
fn resolve<'a>(apps: &'a [AppInfo], name: &str) -> Option<&'a AppInfo> {
    let name = name.trim();
    apps.iter().find(|app| app.id.eq_ignore_ascii_case(name)).or_else(|| apps.iter().find(|app| app.name.eq_ignore_ascii_case(name)))
}

/// A retained top entry: its usage key and score at `scored_at`.
struct Ranked {
    key: String,
    score: f64,
}

/// Retains one compact catalog and eight scores, not a second usage history.
pub(super) struct Prepared<S: Source> {
    source: Arc<S>,
    catalog_key: Option<(Lang, Option<Arc<str>>, u64)>,
    usage_key: Option<(u32, u64)>,
    apps: Arc<[AppInfo]>,
    top: Vec<Ranked>,
    scored_at: i64,
    future_entries: bool,
}

impl<S: Source> Prepared<S> {
    /// Nothing prepared yet: the first snapshot reads the catalog and history.
    pub fn new(source: Arc<S>) -> Self {
        Self { source, catalog_key: None, usage_key: None, apps: Arc::from([]), top: Vec::new(), scored_at: 0, future_entries: false }
    }

    /// Changed source revisions and captured settings prepare once. Aging a
    /// score only touches the retained eight entries and performs no I/O.
    pub fn snapshot(&mut self, key: &Key, revisions: (u64, u64), now: i64) -> Snapshot {
        if !self.catalog_key.as_ref().is_some_and(|(language, theme, revision)| *language == key.language && theme == &key.icon_theme && *revision == revisions.0) {
            self.apps = self.source.catalog(key).into_iter().map(|app| AppInfo {
                id: app.id, name: app.name.into(), icon: app.icon.map(|path| path.to_string_lossy().into_owned().into()),
            }).collect();
            self.catalog_key = Some((key.language, key.icon_theme.clone(), revisions.0));
        }
        if self.usage_key != Some((key.half_life, revisions.1)) || self.future_entries || now < self.scored_at {
            self.top.clear();
            self.top.reserve(USAGE_ROWS);
            self.future_entries = false;
            for (name, entry) in self.source.usage().entries {
                self.future_entries |= entry.last > now;
                let score = age(entry.score, now.saturating_sub(entry.last), key);
                // BTreeMap order and insertion after equal scores preserve the
                // former stable descending sort, without collecting all keys.
                let at = self.top.partition_point(|row| row.score.total_cmp(&score).is_ge());
                if at < USAGE_ROWS {
                    if self.top.len() == USAGE_ROWS { self.top.pop(); }
                    self.top.insert(at, Ranked { key: name, score });
                }
            }
            self.scored_at = now;
            self.usage_key = Some((key.half_life, revisions.1));
        }
        let factor = age(1.0, now.saturating_sub(self.scored_at), key);
        let rows = self.top.iter().map(|rank| {
            let score = rank.score * factor;
            Row { label: label(&rank.key, &self.apps), score, display: format!("{score:.1}").into() }
        }).collect();
        Snapshot { key: key.clone(), revisions, apps: self.apps.clone(), rows }
    }

    /// The common age factor cannot change rank order. Wake only when a
    /// displayed tenth may change, bounded for clock adjustments.
    pub fn next_refresh(&self, key: &Key, now: i64) -> Option<Duration> {
        // A clock correction can leave timestamps in the future. Their
        // clamped age is not the common factor used for ordinary history.
        if self.future_entries { return Some(Duration::from_secs_f64(MIN_REFRESH)); }
        let factor = age(1.0, now.saturating_sub(self.scored_at), key);
        self.top.iter().map(|row| row.score * factor).filter(|score| score.is_finite() && *score > 0.05).map(|score| {
            let next = (score * 10.0).round() / 10.0 - 0.05;
            if next > 0.0 { (score / next).log2() * key.days() * SECONDS_PER_DAY } else { MAX_REFRESH }
        }).reduce(f64::min).map(|seconds| Duration::from_secs_f64(seconds.ceil().clamp(MIN_REFRESH, MAX_REFRESH)))
    }
}

/// `score` halved for every half-life in `elapsed` seconds (negative
/// elapsed time counts as none).
fn age(score: f64, elapsed: i64, key: &Key) -> f64 {
    score * 0.5_f64.powf(elapsed.max(0) as f64 / (key.days() * SECONDS_PER_DAY))
}

/// How a usage key reads in the ranking: the app's name for `app:`, the
/// command line for `cmd:`, otherwise the key without its kind.
fn label(key: &str, apps: &[AppInfo]) -> SharedString {
    match key.split_once(':') {
        Some(("app", id)) => resolve(apps, id).map(|app| app.name.clone()).unwrap_or_else(|| id.to_owned().into()),
        Some(("cmd", line)) => format!("› {line}").into(),
        Some((_, rest)) => rest.to_owned().into(),
        None => key.to_owned().into(),
    }
}

#[cfg(test)]
mod tests;
