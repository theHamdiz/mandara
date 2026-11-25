//! Per-view redraw deadlines. Replaced timers cannot invalidate a newer frame.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{App, EntityId, Window};

/// Nearby deadlines share the earlier redraw.
const SLACK: Duration = Duration::from_millis(4);

#[derive(Default)]
struct Pending(HashMap<EntityId, Instant>);

impl Pending {
    fn schedule(&mut self, view: EntityId, due: Instant) -> bool {
        if self.0.get(&view).is_some_and(|existing| *existing <= due + SLACK) {
            return false;
        }
        self.0.insert(view, due);
        true
    }

    fn fired(&mut self, view: EntityId, due: Instant) -> bool {
        if self.0.get(&view) != Some(&due) {
            return false;
        }
        self.0.remove(&view);
        true
    }
}

thread_local! { static PENDING: RefCell<Pending> = RefCell::default(); }

/// Schedule from a visible render. Earlier pending deadlines win; after
/// hiding, the last timer expires without scheduling another one.
pub fn redraw_in(delay: Duration, window: &mut Window, cx: &mut App) {
    let view = window.current_view();
    let due = Instant::now() + delay;
    if !PENDING.with(|pending| pending.borrow_mut().schedule(view, due)) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(delay).await;
        if PENDING.with(|pending| pending.borrow_mut().fired(view, due)) {
            cx.update(|cx| cx.notify(view));
        }
    }).detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_superseded_deadline_cannot_redraw_or_remove_its_replacement() {
        let mut pending = Pending::default();
        let view = EntityId::from(1);
        let now = Instant::now();
        let slow = now + SLACK * 20;
        let fast = now + SLACK;
        assert!(pending.schedule(view, slow));
        assert!(pending.schedule(view, fast));
        assert!(!pending.fired(view, slow));
        assert_eq!(pending.0.get(&view), Some(&fast));
        assert!(pending.fired(view, fast));
        assert!(!pending.fired(view, slow));
        assert!(pending.0.is_empty());
    }

    #[test]
    fn a_new_deadline_survives_an_old_timer_from_the_previous_frame() {
        let mut pending = Pending::default();
        let view = EntityId::from(1);
        let other = EntityId::from(2);
        let now = Instant::now();
        let old = now + SLACK;
        let next = now + SLACK * 4;
        assert!(pending.schedule(view, old));
        assert!(!pending.schedule(view, next));
        assert!(pending.schedule(other, next));
        assert!(pending.fired(view, old));
        assert!(pending.schedule(view, next));
        assert!(!pending.fired(view, old));
        assert!(pending.fired(view, next));
        assert!(pending.fired(other, next));
    }
}
