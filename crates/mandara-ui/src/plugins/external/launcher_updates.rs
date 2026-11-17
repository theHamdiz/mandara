//! Reply-driven launcher invalidation; no polling or hidden-window work.

use gpui::{App, Entity, EventEmitter, Global};
use gpui::AppContext as _;

/// A delivery batch is small; larger batches invalidate every active plugin query.
const MAX_CHANGED: usize = 64;

pub(crate) struct Updates;
impl EventEmitter<Change> for Updates {}

struct Bus(Entity<Updates>);
impl Global for Bus {}

pub(crate) struct Change {
    all: bool,
    plugins: Vec<String>,
}

impl Change {
    pub fn affects(&self, queried: &[String]) -> bool {
        !queried.is_empty() && (self.all || queried.iter().any(|plugin| self.plugins.contains(plugin)))
    }
}

/// Created when the first launcher opens. Subscriptions end with their views.
pub(crate) fn events(cx: &mut App) -> Entity<Updates> {
    if let Some(bus) = cx.try_global::<Bus>() { return bus.0.clone(); }
    let events = cx.new(|_| Updates);
    cx.set_global(Bus(events.clone()));
    events
}

pub(super) fn notify(plugins: &[String], cx: &mut App) {
    if plugins.is_empty() { return; }
    let Some(events) = cx.try_global::<Bus>().map(|bus| bus.0.clone()) else { return };
    let all = plugins.len() > MAX_CHANGED;
    let change = Change { all, plugins: if all { Vec::new() } else { plugins.to_vec() } };
    events.update(cx, |_, cx| cx.emit(change));
}

pub(super) fn all(cx: &mut App) {
    let Some(events) = cx.try_global::<Bus>().map(|bus| bus.0.clone()) else { return };
    events.update(cx, |_, cx| cx.emit(Change { all: true, plugins: Vec::new() }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_refresh_only_queries_that_used_the_plugin() {
        let updates = Change { all: false, plugins: vec!["gifs".into()] };
        assert!(updates.affects(&["gifs".into()]));
        assert!(!updates.affects(&["moon".into()]));
        assert!(!updates.affects(&[]));
        let all = Change { all: true, plugins: Vec::new() };
        assert!(all.affects(&["moon".into()]));
        assert!(!all.affects(&[]));
    }
}
