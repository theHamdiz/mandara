//! Launcher page data is prepared once, shared by its rows and released on hide.

mod model;
mod watch;
mod worker;

use std::sync::Arc;

use gpui::{Context, Subscription, Task, Window};
use mandara_config::ShellConfig;

use super::StudioView;
use super::page_visit::PageVisit;
use crate::registry::Section;
use model::{Key, Snapshot};

pub(super) use model::USAGE_ROWS;

/// The Launcher page's prepared data while the page is shown: the settings
/// it was asked for, the latest accepted snapshot and the worker lease.
#[derive(Default)]
pub(super) struct LauncherData {
    key: Option<Key>,
    snapshot: Option<Arc<Snapshot>>,
    lease: Option<worker::Lease>,
    delivery: Option<Task<()>>,
    visibility: Option<Subscription>,
    visit: PageVisit,
}

impl LauncherData {
    /// Close the event queue before cancelling delivery. The worker drops
    /// late results and tears its native watcher down off the UI thread.
    fn stop(&mut self) {
        self.lease = None;
        self.delivery = None;
        self.snapshot = None;
        self.key = None;
        self.visit = PageVisit::default();
    }

    /// Leaving search/page navigation also removes the visibility observer.
    pub fn release(&mut self) {
        self.stop();
        self.visibility = None;
    }

    /// The snapshot to draw, only when it matches the current settings.
    pub fn snapshot(&self) -> Option<&Arc<Snapshot>> {
        self.snapshot.as_ref().filter(|snapshot| self.key.as_ref() == Some(&snapshot.key))
    }
}

impl StudioView {
    /// Called at the page's lifecycle boundary, before its element tree.
    /// Warm scrolling compares borrowed settings and performs no source I/O.
    pub(super) fn launcher_follow(&mut self, cfg: &ShellConfig, window: &mut Window, cx: &mut Context<Self>) {
        if self.section != Section::Launcher || !self.query.is_empty() {
            if self.launcher_data.lease.is_some() || self.launcher_data.visibility.is_some() { self.launcher_data.release(); }
            return;
        }
        if self.launcher_data.visibility.is_none() {
            let weak = cx.weak_entity();
            self.launcher_data.visibility = Some(window.observe_window_visibility(move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
                    this.launcher_follow(&cfg, window, cx);
                });
            }));
        }
        if !window.is_visible() {
            if self.launcher_data.lease.is_some() { self.launcher_data.stop(); }
            return;
        }
        let language = cfg.general.locale.lang();
        let half_life = cfg.launcher.usage.half_life_days;
        let theme = cfg.appearance.icon_theme.as_deref();
        if self.launcher_data.key.as_ref().is_some_and(|key| key.matches(language, half_life, theme)) { return; }
        let key = Key::new(language, half_life, theme);
        self.launcher_data.key = Some(key.clone());
        if let Some(lease) = &self.launcher_data.lease {
            lease.request(key);
            return;
        }
        let (lease, updates) = worker::start(key, cx.background_executor().clone());
        self.launcher_data.lease = Some(lease);
        let visit = self.launcher_data.visit.clone();
        self.launcher_data.delivery = Some(cx.spawn(async move |this, cx| {
            while let Ok(snapshot) = updates.recv().await {
                if this.update(cx, |this, cx| {
                    if !this.launcher_data.visit.accepts(&visit) || !this.launcher_data.lease.as_ref().is_some_and(|lease| lease.accepts(&snapshot)) { return; }
                    this.launcher_data.snapshot = Some(snapshot);
                    cx.notify();
                }).is_err() { break; }
            }
        }));
    }
}
