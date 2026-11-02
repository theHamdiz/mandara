//! Presentation mode's bookkeeping: what it switches (do not disturb, keep
//! awake, hidden desktop widgets), the state each switch was in before so
//! turning it off restores exactly that, and when screen sharing turns it on
//! and off by itself.
//!
//! Pure logic; the shell reads the live switches, applies the [`Plan`]s
//! and keeps the [`Session`] while the mode is on.

use serde::Serialize;

/// The switches presentation mode controls.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Switches {
    /// Notifications' do not disturb.
    pub dnd: bool,
    /// The screen is kept awake (idle inhibited).
    pub keep_awake: bool,
    /// Desktop widgets hidden.
    pub widgets_hidden: bool,
}

/// What to change: `Some(state)` for each switch to set.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub dnd: Option<bool>,
    pub keep_awake: Option<bool>,
    pub widgets_hidden: Option<bool>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.dnd.is_none() && self.keep_awake.is_none() && self.widgets_hidden.is_none()
    }
}

/// The mode while it is on.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Session {
    /// When it turned on, milliseconds since the Unix epoch.
    pub since_ms: i64,
    /// The switches as they were before.
    pub saved: Switches,
    /// Turned on by screen sharing (and so turned off when it stops).
    pub auto: bool,
}

/// Turning on: remembers `current` and switches on whatever is off.
pub fn turn_on(current: Switches, now_ms: i64, auto: bool) -> (Session, Plan) {
    let on = |was: bool| (!was).then_some(true);
    (Session { since_ms: now_ms, saved: current, auto }, Plan { dnd: on(current.dnd), keep_awake: on(current.keep_awake), widgets_hidden: on(current.widgets_hidden) })
}

/// Turning off: puts back each switch that differs from how it was before
/// (a switch that was already on stays on).
pub fn turn_off(session: &Session, current: Switches) -> Plan {
    let back = |was: bool, now: bool| (was != now).then_some(was);
    Plan {
        dnd: back(session.saved.dnd, current.dnd),
        keep_awake: back(session.saved.keep_awake, current.keep_awake),
        widgets_hidden: back(session.saved.widgets_hidden, current.widgets_hidden),
    }
}

/// What screen sharing asks for: `Some(true)` to turn on by itself,
/// `Some(false)` to turn off what it turned on, `None` to leave it.
/// `declined`: you turned it off yourself during this share, so it stays
/// off until the share ends.
pub fn follow_sharing(auto_enabled: bool, sharing: bool, session: Option<&Session>, declined: bool) -> Option<bool> {
    match session {
        None if auto_enabled && sharing && !declined => Some(true),
        Some(s) if s.auto && !sharing => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_on_saves_and_switches_on_what_is_off() {
        let before = Switches { dnd: true, keep_awake: false, widgets_hidden: false };
        let (session, plan) = turn_on(before, 1_000, false);
        assert_eq!(session.saved, before);
        assert_eq!(plan, Plan { dnd: None, keep_awake: Some(true), widgets_hidden: Some(true) }, "DND was already on: left alone");
        let all_on = Switches { dnd: true, keep_awake: true, widgets_hidden: true };
        assert!(turn_on(all_on, 0, false).1.is_empty());
    }

    #[test]
    fn turning_off_restores_exactly_the_previous_state() {
        let before = Switches { dnd: true, keep_awake: false, widgets_hidden: false };
        let (session, _) = turn_on(before, 0, false);
        let during = Switches { dnd: true, keep_awake: true, widgets_hidden: true };
        assert_eq!(turn_off(&session, during), Plan { dnd: None, keep_awake: Some(false), widgets_hidden: Some(false) }, "DND stays on: you had it on");
        // Switched by hand while presenting: only what differs is put back.
        let (session, _) = turn_on(Switches::default(), 0, false);
        let touched = Switches { dnd: false, keep_awake: true, widgets_hidden: true };
        assert_eq!(turn_off(&session, touched), Plan { dnd: None, keep_awake: Some(false), widgets_hidden: Some(false) });
        assert!(turn_off(&session, Switches::default()).is_empty());
    }

    #[test]
    fn screen_sharing_turns_it_on_and_only_its_own_session_off() {
        assert_eq!(follow_sharing(true, true, None, false), Some(true));
        assert_eq!(follow_sharing(true, true, None, true), None, "you turned it off during this share");
        assert_eq!(follow_sharing(false, true, None, false), None, "the setting is off");
        let (auto, _) = turn_on(Switches::default(), 0, true);
        let (manual, _) = turn_on(Switches::default(), 0, false);
        assert_eq!(follow_sharing(true, false, Some(&auto), false), Some(false));
        assert_eq!(follow_sharing(true, false, Some(&manual), false), None, "you turned it on: it stays on");
        assert_eq!(follow_sharing(true, true, Some(&auto), false), None);
    }
}
