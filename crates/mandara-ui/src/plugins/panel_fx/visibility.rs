//! Close watches borrow panel presence; replacing a host cannot keep a watch alive.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use gpui::App;
use mandara_plugins::{PanelCx, WeakPanelCx};
use smallvec::SmallVec;

/// Matches the existing close grace so a quick reopen reuses its warm snapshot.
const CLOSE_SECONDS: u64 = 1;
const CLOSE_POLL: Duration = Duration::from_secs(CLOSE_SECONDS);

#[derive(Clone, Default)]
pub(in crate::plugins) struct Visibility(Rc<RefCell<SmallVec<[WeakPanelCx; 2]>>>);

impl Visibility {
    pub fn track(&self, panel: &PanelCx) {
        let panel = panel.downgrade();
        let mut panels = self.0.borrow_mut();
        panels.retain(|panel| panel.progress().is_some_and(|progress| progress > 0.0));
        if panels.iter().any(|entry| entry.same_surface(&panel)) { return; }
        if panels.len() >= super::VIEWSET_CAP { panels.remove(0); }
        panels.push(panel);
    }

    fn visible(&self) -> bool { self.0.borrow().iter().any(|panel| panel.progress().is_some_and(|progress| progress > 0.0)) }

    pub fn clear(&self) { *self.0.borrow_mut() = SmallVec::new(); }

    pub fn on_last_close(&self, armed: &Rc<Cell<bool>>, cx: &mut App, on_close: impl FnOnce(&mut App) + 'static) {
        let panels = self.clone();
        watch(move || panels.visible(), armed, cx, on_close);
    }
}

pub(in crate::plugins) fn on_close(panel: &PanelCx, armed: &Rc<Cell<bool>>, cx: &mut App, on_close: impl FnOnce(&mut App) + 'static) {
    let panel = panel.downgrade();
    watch(move || panel.progress().is_some_and(|progress| progress > 0.0), armed, cx, on_close);
}

fn watch(visible: impl Fn() -> bool + 'static, armed: &Rc<Cell<bool>>, cx: &mut App, on_close: impl FnOnce(&mut App) + 'static) {
    if armed.replace(true) { return; }
    let armed = armed.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(CLOSE_POLL).await;
            if !visible() { break; }
        }
        armed.set(false);
        cx.update(on_close);
    }).detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_snapshots_stay_open_until_the_last_surface_closes() {
        let panels = Visibility::default();
        let first = PanelCx::for_monitor("first"); let second = PanelCx::for_monitor("second");
        first.set_progress(1.0); second.set_progress(1.0);
        panels.track(&first); panels.track(&first); panels.track(&second);
        assert_eq!(panels.0.borrow().len(), 2);
        first.set_progress(0.0);
        assert!(panels.visible());
        drop(second);
        assert!(!panels.visible(), "a discarded open context cannot retain a close watch");
        panels.clear();
        assert!(panels.0.borrow().is_empty());
    }
}
