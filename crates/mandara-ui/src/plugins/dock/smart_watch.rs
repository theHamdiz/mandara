//! The geometry monitor follows smart hiding, rather than the dock's lifetime.

use mandara_config::workflow::DockAutoHide;

/// Lifecycle change for the dock's compositor-geometry monitor.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Action {
    Keep,
    Start,
    Stop,
}

/// Keeps exactly one monitor while smart hiding needs window geometry, and
/// stops it when another hide mode can work without periodic checks.
pub(super) fn plan(mode: DockAutoHide, running: bool) -> Action {
    match (mode == DockAutoHide::Smart, running) {
        (true, false) => Action::Start,
        (false, true) => Action::Stop,
        _ => Action::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_hide_modes_starts_one_monitor_and_stops_it_when_unneeded() {
        assert_eq!(plan(DockAutoHide::Never, false), Action::Keep);
        assert_eq!(plan(DockAutoHide::Always, false), Action::Keep);
        assert_eq!(plan(DockAutoHide::Smart, false), Action::Start);
        assert_eq!(plan(DockAutoHide::Smart, true), Action::Keep);
        assert_eq!(plan(DockAutoHide::Always, true), Action::Stop);
        assert_eq!(plan(DockAutoHide::Never, true), Action::Stop);
    }
}
