//! Bar geometry shared by the shell host.

pub mod layout;
pub mod autohide;

/// Whether the bar runs along a side edge: bar plugins then draw their
/// compact column form (stacked clock digits, a vertical tag strip, icons
/// over values). Content wider than the side bar is hidden by the shell.
pub fn vertical(cx: &gpui::App) -> bool {
    crate::state::config(cx).read(cx).get().bar.position.is_vertical()
}
