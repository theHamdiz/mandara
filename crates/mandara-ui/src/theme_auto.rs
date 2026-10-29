//! Automatic light/dark switching (`theme.auto`): follow the desktop's
//! color-scheme preference, or a daily schedule. `theme.mode` stays the
//! single source of truth everything else reads; this only writes it when
//! the *desired* mode changes (a boundary passes, the desktop flips, or the
//! setting itself changes), so a manual toggle in between is respected.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::App;
use mandara_config::{ThemeAuto, ThemeMode};
use mandara_services::appearance::{self, Preference, parse_hm, scheduled_dark};

use crate::state::{config, update_config};

/// The light / dark schedule is checked this often (minute resolution; config changes apply at once).
const EVALUATE_EVERY: Duration = Duration::from_secs(30);

#[derive(Default)]
struct AutoState {
    /// The desktop preference last reported by the portal.
    system: Option<Preference>,
    /// `(auto, light_at, dark_at)` the last decision was made under.
    key: Option<(ThemeAuto, String, String)>,
    /// Last desired mode that was applied.
    applied: Option<bool>,
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

fn evaluate(state: &RefCell<AutoState>, cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let t = &cfg.theme;
    let key = (t.auto, t.light_at.clone(), t.dark_at.clone());
    let mut st = state.borrow_mut();
    if st.key.as_ref() != Some(&key) {
        st.key = Some(key);
        st.applied = None;
    }
    let Some(dark) = desired(t.auto, &t.light_at, &t.dark_at, st.system) else { return };
    if st.applied == Some(dark) {
        return;
    }
    st.applied = Some(dark);
    drop(st);
    if t.mode.is_dark() != dark {
        tracing::info!("theme: switching to {} ({:?})", if dark { "dark" } else { "light" }, t.auto);
        update_config(cx, |c| c.theme.mode = if dark { ThemeMode::Dark } else { ThemeMode::Light });
    }
}

pub fn init(cx: &mut App) {
    let state = Rc::new(RefCell::new(AutoState::default()));
    let prefs = appearance::watch();
    let s = state.clone();
    cx.spawn(async move |cx| {
        while let Ok(pref) = prefs.recv().await {
            s.borrow_mut().system = Some(pref);
            cx.update(|cx| evaluate(&s, cx));
        }
    })
    .detach();
    // Clock + setting changes: re-evaluate twice a minute (cheap) and right
    // after any config change.
    let s = state.clone();
    cx.spawn(async move |cx| loop {
        cx.update(|cx| evaluate(&s, cx));
        cx.background_executor().timer(EVALUATE_EVERY).await;
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
    }
}
