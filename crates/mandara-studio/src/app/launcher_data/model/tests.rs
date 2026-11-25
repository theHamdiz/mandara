//! Differential ranking and source-call controls for the prepared page model.

use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use mandara_ui::launcher_ext::UsageEntry;

struct FakeSource {
    roots: Roots,
    catalogs: AtomicUsize,
    reads: AtomicUsize,
    usage: Mutex<Usage>,
}

impl FakeSource {
    fn new(usage: Usage) -> Arc<Self> {
        Arc::new(Self { roots: Roots { applications: vec![], usage: PathBuf::from("fixture-usage"), preferences: vec![] }, catalogs: AtomicUsize::new(0), reads: AtomicUsize::new(0), usage: Mutex::new(usage) })
    }
    fn counts(&self) -> (usize, usize) { (self.catalogs.load(Ordering::Relaxed), self.reads.load(Ordering::Relaxed)) }
}

impl Source for FakeSource {
    fn roots(&self) -> &Roots { &self.roots }
    fn catalog(&self, key: &Key) -> Vec<mandara_ui::apps::App> {
        self.catalogs.fetch_add(1, Ordering::Relaxed);
        (0..12).map(|index| mandara_ui::apps::App { id: format!("app-{index}"), name: format!("{} {index}", key.language.code()), ..Default::default() }).collect()
    }
    fn usage(&self) -> Usage {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.usage.lock().unwrap().clone()
    }
}

fn history() -> Usage {
    Usage { entries: (0..12).map(|index| (format!("app:app-{index}"), UsageEntry { score: f64::from(index + 1), last: 100 + i64::from(index) })).collect() }
}

/// The warm/aging path does no discovery or history read. Every actual
/// dependency changes only the preparation it invalidates.
#[test]
fn prepared_sources_reuse_catalog_and_history_with_complete_keys() {
    let source = FakeSource::new(history());
    let mut model = Prepared::new(source.clone());
    let key = Key::new(Lang::En, 14.0, None);
    let first = model.snapshot(&key, (0, 0), 200);
    for now in 201..300 {
        let warm = model.snapshot(&key, (0, 0), now);
        assert!(Arc::ptr_eq(&first.apps, &warm.apps));
    }
    assert_eq!(source.counts(), (1, 1));
    let arabic = model.snapshot(&Key::new(Lang::Ar, 14.0, None), (0, 0), 300);
    assert!(arabic.rows.iter().all(|row| row.label.starts_with("ar ")));
    assert_eq!(source.counts(), (2, 1));
    model.snapshot(&Key::new(Lang::Ar, 14.0, Some("other-theme")), (0, 0), 300);
    assert_eq!(source.counts(), (3, 1));
    let changed = Key::new(Lang::Ar, 7.0, Some("other-theme"));
    model.snapshot(&changed, (0, 0), 300);
    assert_eq!(source.counts(), (3, 2));
    model.snapshot(&changed, (1, 0), 300);
    assert_eq!(source.counts(), (4, 2));
    source.usage.lock().unwrap().entries.clear();
    assert!(model.snapshot(&changed, (1, 1), 300).rows.is_empty());
    assert_eq!(source.counts(), (4, 3));
}

/// The old per-label lookup is a real positive control: eight rows cause
/// eight catalog calls. One preparation produces the same labels/order.
#[test]
fn one_catalog_replaces_eight_scans_and_preserves_stable_ranking() {
    let source = FakeSource::new(history());
    let key = Key::new(Lang::En, 14.0, None);
    let usage = source.usage();
    let mut old: Vec<_> = usage.entries.keys().map(|name| (name.clone(), usage.score(name, 200, 14.0))).collect();
    old.sort_by(|left, right| right.1.total_cmp(&left.1));
    old.truncate(USAGE_ROWS);
    let expected: Vec<_> = old.iter().map(|(name, score)| {
        let apps = source.catalog(&key);
        let id = name.strip_prefix("app:").unwrap();
        (apps.iter().find(|app| app.id == id).unwrap().name.clone(), *score)
    }).collect();
    assert_eq!(source.counts(), (USAGE_ROWS, 1));
    let mut prepared = Prepared::new(source.clone());
    let result = prepared.snapshot(&key, (0, 0), 200);
    assert_eq!(source.counts(), (USAGE_ROWS + 1, 2));
    for (row, (name, score)) in result.rows.iter().zip(expected) {
        assert_eq!(row.label.as_ref(), name);
        assert!((row.score - score).abs() < f64::EPSILON);
    }
    assert_eq!(prepared.top.len(), USAGE_ROWS);
    let later = prepared.snapshot(&key, (0, 0), 86_600);
    for (row, (name, _)) in later.rows.iter().zip(old) {
        assert!((row.score - usage.score(&name, 86_600, 14.0)).abs() < 1e-12);
    }
    assert_eq!(source.counts(), (USAGE_ROWS + 1, 2));
}

/// Equal scores keep BTreeMap order; id matches beat duplicate display
/// names, and non-app results retain their established labels.
#[test]
fn ties_resolution_and_non_application_labels_match_existing_rules() {
    let source = FakeSource::new(Usage { entries: ["app:missing", "cmd:echo demo", "file:/tmp/demo", "literal"].map(|name| (name.into(), UsageEntry { score: 1.0, last: 100 })).into() });
    let mut prepared = Prepared::new(source);
    let result = prepared.snapshot(&Key::new(Lang::En, 14.0, None), (0, 0), 100);
    assert_eq!(result.rows.iter().map(|row| row.label.as_ref()).collect::<Vec<_>>(), ["missing", "› echo demo", "/tmp/demo", "literal"]);
    assert_eq!(result.resolve(" APP-1 ").unwrap().name.as_ref(), "en 1");
    assert_eq!(result.resolve("EN 2").unwrap().id, "app-2");
    assert!(result.resolve("unknown").is_none());
    let apps = [AppInfo { id: "other".into(), name: "wanted".into(), icon: None }, AppInfo { id: "wanted".into(), name: "Exact id".into(), icon: None }];
    assert_eq!(resolve(&apps, "wanted").unwrap().name.as_ref(), "Exact id");
}

/// Large scores refresh before their displayed tenth changes; empty books
/// schedule no age wakeups. Old catalogs are released with the page model.
#[test]
fn age_deadlines_are_bounded_and_page_storage_can_be_released() {
    let source = FakeSource::new(Usage { entries: [("app:app-1".into(), UsageEntry { score: 1_000_000.0, last: 100 })].into() });
    let key = Key::new(Lang::En, 14.0, None);
    let mut prepared = Prepared::new(source.clone());
    let snapshot = prepared.snapshot(&key, (0, 0), 100);
    let weak = Arc::downgrade(&snapshot.apps);
    assert!(prepared.next_refresh(&key, 100).unwrap() <= Duration::from_secs(2));
    drop(snapshot);
    drop(prepared);
    assert!(weak.upgrade().is_none());
    let mut empty = Prepared::new(FakeSource::new(Usage::default()));
    empty.snapshot(&key, (0, 0), 100);
    assert!(empty.next_refresh(&key, 100).is_none());
}

/// Backward clock changes and future timestamps preserve the old per-entry
/// clamped decay instead of applying a common factor that no longer fits.
#[test]
fn corrected_clocks_preserve_history_scores() {
    let source = FakeSource::new(Usage { entries: [("app:app-1".into(), UsageEntry { score: 8.0, last: 500 }), ("app:app-2".into(), UsageEntry { score: 4.0, last: 100 })].into() });
    let key = Key::new(Lang::En, 14.0, None);
    let mut model = Prepared::new(source.clone());
    for now in [200, 300, 600, 150] {
        let snapshot = model.snapshot(&key, (0, 0), now);
        for row in snapshot.rows.iter() {
            let app = if row.label.as_ref() == "en 1" { "app:app-1" } else { "app:app-2" };
            assert_eq!(row.score, source.usage.lock().unwrap().score(app, now, 14.0));
        }
    }
    assert_eq!(source.counts(), (1, 4));
}
