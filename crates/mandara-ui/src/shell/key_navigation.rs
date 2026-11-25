//! Tab traversal for visible shell panels and sidebar cards.

use gpui::{App, Keystroke, Window};

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Next,
    Previous,
}

/// Leave modified shortcuts to their owning view or compositor.
fn step(key: &Keystroke) -> Option<Step> {
    let modifiers = key.modifiers;
    if key.key != "tab" || modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    Some(if modifiers.shift { Step::Previous } else { Step::Next })
}

/// Report a handled key only when traversal actually finds another target.
pub(super) fn traverse(key: &Keystroke, window: &mut Window, cx: &mut App) -> bool {
    let before = window.focused(cx);
    match step(key) {
        Some(Step::Next) => window.focus_next(cx),
        Some(Step::Previous) => window.focus_prev(cx),
        None => return false,
    }
    window.focused(cx) != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_traversal_preserves_sidebar_shortcuts_and_unrelated_keys() {
        assert_eq!(step(&Keystroke::parse("tab").unwrap()), Some(Step::Next));
        assert_eq!(step(&Keystroke::parse("shift-tab").unwrap()), Some(Step::Previous));
        for key in ["ctrl-tab", "ctrl-shift-tab", "alt-tab", "cmd-tab", "enter", "escape"] {
            assert_eq!(step(&Keystroke::parse(key).unwrap()), None, "{key}");
        }
    }
}
