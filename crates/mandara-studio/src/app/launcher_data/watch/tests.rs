//! Actual directory creation, atomic replacement and teardown notifications.

use super::*;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(3);

fn receive(events: &async_channel::Receiver<()>) {
    let deadline = Instant::now() + TIMEOUT;
    while events.try_recv().is_err() {
        assert!(Instant::now() < deadline, "launcher source change was not reported");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Missing roots are followed down as packages create directories, then
/// their contents trigger catalog invalidation without a periodic scan.
#[test]
fn missing_application_roots_are_followed_and_watches_stop_on_drop() {
    let temp = tempfile::tempdir().unwrap();
    let apps = temp.path().join("exports/share/applications");
    let roots = Roots { applications: vec![apps.clone()], usage: temp.path().join("state/launcher-usage.toml"), preferences: vec![] };
    let revisions = Arc::new(Revisions::default());
    let (wake, events) = async_channel::bounded(1);
    let retained_sender = wake.clone();
    let mut watch = Watch::new(roots, revisions.clone(), wake).unwrap();
    std::fs::create_dir_all(&apps).unwrap();
    receive(&events);
    watch.refresh().unwrap();
    while events.try_recv().is_ok() {}
    let before = revisions.get();
    std::fs::write(apps.join("new.desktop"), "[Desktop Entry]\nName=Demo\nExec=true\n").unwrap();
    receive(&events);
    assert!(revisions.get().0 > before.0);
    drop(watch);
    assert!(retained_sender.is_closed());
    assert!(matches!(retained_sender.try_send(()), Err(async_channel::TrySendError::Closed(()))));
    while events.try_recv().is_ok() {}
    std::fs::remove_file(apps.join("new.desktop")).unwrap();
    assert!(events.is_closed());
    assert!(events.try_recv().is_err());
}

/// Atomic usage writes and external icon-theme changes invalidate only
/// their source domain; the replacement file is read in full.
#[test]
fn atomic_history_and_theme_preference_replacements_are_reported() {
    let temp = tempfile::tempdir().unwrap();
    let usage = temp.path().join("launcher-usage.toml");
    let preference = temp.path().join("settings.ini");
    let revisions = Arc::new(Revisions::default());
    let (wake, events) = async_channel::bounded(1);
    let _watch = Watch::new(Roots { applications: vec![], usage: usage.clone(), preferences: vec![preference.clone()] }, revisions.clone(), wake).unwrap();
    let scratch = temp.path().join("pending");
    std::fs::write(&scratch, "[entries.demo]\nscore=2.5\nlast=123\n").unwrap();
    std::fs::rename(&scratch, &usage).unwrap();
    receive(&events);
    assert_eq!(revisions.get().0, 0);
    assert!(revisions.get().1 > 0);
    assert_eq!(super::super::worker::read_usage(&usage).entries["demo"].score, 2.5);
    while events.try_recv().is_ok() {}
    let before = revisions.get().0;
    std::fs::write(&preference, "[Settings]\ngtk-icon-theme-name=Demo\n").unwrap();
    let deadline = Instant::now() + TIMEOUT;
    while revisions.get().0 == before {
        assert!(Instant::now() < deadline, "icon-theme preference change was not reported");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(revisions.get().0 > before);
    std::fs::write(&usage, "invalid {").unwrap();
    assert!(super::super::worker::read_usage(&usage).entries.is_empty());
}

/// A backend may already have retired a deleted root. Both the missing-root
/// refresh and a replacement at the same path must install working watches.
#[test]
fn removed_and_replaced_application_roots_resume_notifications() {
    let temp = tempfile::tempdir().unwrap();
    let apps = temp.path().join("applications");
    std::fs::create_dir(&apps).unwrap();
    let revisions = Arc::new(Revisions::default());
    let (wake, events) = async_channel::bounded(1);
    let mut watch = Watch::new(Roots { applications: vec![apps.clone()], usage: temp.path().join("usage.toml"), preferences: vec![] }, revisions.clone(), wake).unwrap();
    // Simulate notify's automatic retirement deterministically on the real backend.
    watch.watcher.unwatch(&apps).unwrap();
    std::fs::remove_dir(&apps).unwrap();
    watch.refresh().unwrap();
    assert!(!watch.watching.contains(&apps));
    std::fs::create_dir(&apps).unwrap();
    watch.refresh().unwrap();
    assert!(watch.watching.contains(&apps));
    // Replacement can also finish before the coalesced refresh observes the gap.
    watch.watcher.unwatch(&apps).unwrap();
    std::fs::remove_dir(&apps).unwrap();
    std::fs::create_dir(&apps).unwrap();
    watch.refresh().unwrap();
    while events.try_recv().is_ok() {}
    let before = revisions.get().0;
    let written = apps.join("after-replacement.desktop");
    std::fs::write(&written, "[Desktop Entry]\nName=Demo\nExec=true\n").unwrap();
    let deadline = Instant::now() + TIMEOUT;
    while revisions.get().0 == before || !revisions.observed_paths.lock().unwrap().contains(&written) {
        assert!(Instant::now() < deadline, "replacement watch did not report {}", written.display());
        std::thread::sleep(Duration::from_millis(10));
    }
    receive(&events);
    assert!(revisions.get().0 > before);
}
