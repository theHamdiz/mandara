//! Automatic light/dark switching (`theme.auto`): follow the desktop's
//! color-scheme preference, or a daily schedule. `theme.mode` stays the
//! single source of truth everything else reads; this only writes it when
//! the *desired* mode changes (a boundary passes, the desktop flips, or the
//! setting itself changes), so a manual toggle in between is respected.

mod decision;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{App, Task};
use mandara_config::ThemeAuto;
use mandara_services::appearance::{self, Preference, parse_hm, scheduled_dark};

use crate::state::{config, update_config};

/// The light / dark schedule is checked this often (minute resolution; config changes apply at once).
const EVALUATE_EVERY: Duration = Duration::from_secs(30);

struct AutoState {
    /// The desktop preference last reported by the portal.
    system: Option<Preference>,
    decision: Option<decision::Decision>,
    persist: async_channel::Sender<decision::Saved>,
    clock: Option<Task<()>>,
}

fn follows_clock(auto: ThemeAuto) -> bool {
    auto == ThemeAuto::Schedule
}

fn desired(auto: ThemeAuto, light_at: &str, dark_at: &str, system: Option<Preference>) -> Option<bool> {
    match auto {
        ThemeAuto::Manual => None,
        ThemeAuto::System => system.flatten(),
        ThemeAuto::Schedule => {
            let now = chrono::Local::now();
            let minutes = chrono::Timelike::hour(&now) * 60 + chrono::Timelike::minute(&now);
            Some(scheduled_dark(minutes, parse_hm(light_at)?, parse_hm(dark_at)?))
        }
    }
}

fn evaluate(state: &Rc<RefCell<AutoState>>, cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let t = &cfg.theme;
    let mut st = state.borrow_mut();
    match (follows_clock(t.auto), st.clock.is_some()) {
        (true, false) => {
            let weak = Rc::downgrade(state);
            st.clock = Some(cx.spawn(async move |cx| loop {
                cx.background_executor().timer(EVALUATE_EVERY).await;
                let Some(state) = weak.upgrade() else { break };
                cx.update(|cx| evaluate(&state, cx));
            }));
        }
        (false, true) => st.clock = None,
        _ => {}
    }
    let target = desired(t.auto, &t.light_at, &t.dark_at, st.system);
    let Some(decision) = st.decision.as_mut() else { return };
    let change = decision.evaluate(t, target);
    if let Some(saved) = change.save
        && let Err(error) = st.persist.try_send(saved)
    {
        tracing::error!("automatic theme state writer stopped: {error}");
    }
    drop(st);
    if let Some(mode) = change.mode {
        tracing::info!("theme: switching to {} ({:?})", if mode.is_dark() { "dark" } else { "light" }, t.auto);
        update_config(cx, |c| c.theme.mode = mode);
    }
}

/// Restore automatic theme state off-thread and follow shared appearance preferences.
pub fn init(cx: &mut App) {
    let initial_theme = config(cx).read(cx).get().theme.clone();
    let path = mandara_config::paths::state_dir().join("theme-auto.json");
    let load_path = path.clone();
    let loaded = cx.background_executor().spawn(async move {
        decision::Saved::load(&load_path).unwrap_or_else(|error| {
            tracing::warn!("automatic theme state: {error:#}");
            None
        })
    });
    let (persist, writes) = async_channel::unbounded::<decision::Saved>();
    cx.background_executor().spawn(async move {
        while let Ok(mut saved) = writes.recv().await {
            while let Ok(newer) = writes.try_recv() {
                saved = newer;
            }
            if let Err(error) = saved.save(&path) {
                tracing::error!("saving automatic theme state: {error:#}");
            }
        }
    }).detach();
    let state = Rc::new(RefCell::new(AutoState { system: None, decision: None, persist, clock: None }));
    let s = state.clone();
    cx.spawn(async move |cx| {
        let saved = loaded.await;
        s.borrow_mut().decision = Some(decision::Decision::new(&initial_theme, saved));
        cx.update(|cx| evaluate(&s, cx));
    }).detach();
    let prefs = appearance::watch();
    let s = state.clone();
    cx.spawn(async move |cx| {
        while let Ok(pref) = prefs.recv().await {
            s.borrow_mut().system = Some(pref.dark);
            cx.update(|cx| {
                crate::style::system_motion_changed(pref.reduced_motion, cx);
                evaluate(&s, cx);
            });
        }
    })
    .detach();
    let s = state;
    let store = config(cx);
    cx.observe(&store, move |_, cx| evaluate(&s, cx)).detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_never_decides() {
        assert_eq!(desired(ThemeAuto::Manual, "07:00", "19:00", Some(Some(true))), None);
        assert_eq!(desired(ThemeAuto::System, "07:00", "19:00", Some(Some(false))), Some(false));
        assert_eq!(desired(ThemeAuto::System, "07:00", "19:00", Some(None)), None);
        assert_eq!(desired(ThemeAuto::Schedule, "bad", "19:00", None), None);
        assert!(!follows_clock(ThemeAuto::Manual));
        assert!(!follows_clock(ThemeAuto::System));
        assert!(follows_clock(ThemeAuto::Schedule));
    }
}
