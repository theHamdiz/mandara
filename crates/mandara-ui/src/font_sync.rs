//! Watches typography settings and pushes Mandara's families to the
//! desktop when System font sync is on (`mandara_services::font_sync`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Task};
use mandara_config::{FontSyncConfig, TypographyConfig};
use mandara_services::font_sync::{self, ApplyEnv, FontGsettingsCommand};

/// Debounce while the user flips several toggles or picks fonts.
const SETTLE_MS: u64 = 400;
const SETTLE: Duration = Duration::from_millis(SETTLE_MS);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    sync: FontSyncConfig,
    ui: String,
    display: String,
    mono: String,
}

impl Key {
    fn from(t: &TypographyConfig) -> Self {
        Self { sync: t.sync.clone(), ui: t.ui_font.clone(), display: t.display_font.clone(), mono: t.mono_font.clone() }
    }
}

/// Applies or restores system fonts from the current config (worker thread).
pub fn reconcile_now(cfg: &mandara_config::ShellConfig) {
    let cfg = cfg.clone();
    let _ = std::thread::Builder::new().name("mandara-font-sync".into()).spawn(move || {
        if let Err(err) = font_sync::reconcile(&cfg, &ApplyEnv::current(), &FontGsettingsCommand) {
            tracing::warn!(error = %err, "system font sync");
        }
    });
}

/// Observe the config store and reconcile after typography / sync changes settle.
pub fn init_font_sync(cx: &mut App) {
    let last: Rc<RefCell<Option<Key>>> = Rc::default();
    let pending: Rc<RefCell<Option<Task<()>>>> = Rc::default();
    let on_change = Rc::new(RefCell::new({
        let last = Rc::clone(&last);
        let pending = Rc::clone(&pending);
        move |cx: &mut App| {
            let cfg = crate::state::config(cx).read(cx).get().clone();
            let key = Key::from(&cfg.typography);
            if last.borrow().as_ref() == Some(&key) {
                return;
            }
            *last.borrow_mut() = Some(key);
            let pending_slot = Rc::clone(&pending);
            *pending.borrow_mut() = Some(cx.spawn(async move |cx| {
                cx.background_executor().timer(SETTLE).await;
                let _ = cx.update(|cx| {
                    let cfg = crate::state::config(cx).read(cx).get().clone();
                    reconcile_now(&cfg);
                });
                *pending_slot.borrow_mut() = None;
            }));
        }
    }));
    (on_change.borrow_mut())(cx);
    cx.observe(&crate::state::config(cx), move |_, cx| {
        (on_change.borrow_mut())(cx);
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_tracks_sync_and_families() {
        let mut t = TypographyConfig::default();
        let a = Key::from(&t);
        t.sync.enabled = true;
        assert_ne!(a, Key::from(&t));
        t.ui_font = "Cairo".into();
        assert_ne!(a, Key::from(&t));
    }
}
