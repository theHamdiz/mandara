//! Keyboard focus between Beacon's query and its available mode buttons.

use gpui::{App, FocusHandle, Keystroke, Window};
use mandara_config::workflow::LauncherStyle;

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    Next,
    Previous,
    Button,
    Search,
}

fn destination(style: LauncherStyle, button_focused: bool, key: &Keystroke) -> Option<Destination> {
    if style != LauncherStyle::Beacon { return None; }
    let modifiers = key.modifiers;
    if key.key == "tab" && !modifiers.control && !modifiers.alt && !modifiers.platform {
        return Some(if modifiers.shift { Destination::Previous } else { Destination::Next });
    }
    if button_focused && matches!(key.key.as_str(), "enter" | "space") && !modifiers.modified() {
        return Some(Destination::Button);
    }
    Some(Destination::Search)
}

/// Return true when the query handler must leave this key to native focus or
/// button activation. Typing and result navigation return focus to the query.
pub(super) fn handle(style: LauncherStyle, search: &FocusHandle, key: &Keystroke, window: &mut Window, cx: &mut App) -> bool {
    let button_focused = !search.is_focused(window) && search.contains_focused(window, cx);
    match destination(style, button_focused, key) {
        Some(Destination::Next) => window.focus_next(cx),
        Some(Destination::Previous) => window.focus_prev(cx),
        // GPUI pairs Enter/Space keydown and keyup into the button's on_click.
        Some(Destination::Button) => return true,
        Some(Destination::Search) => {
            if !search.is_focused(window) { search.focus(window, cx); }
            return false;
        }
        None => return false,
    }
    cx.stop_propagation();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(key: &'static str, button: bool) -> Destination {
        destination(LauncherStyle::Beacon, button, &Keystroke::parse(key).unwrap()).unwrap()
    }

    #[test]
    fn tab_and_shift_tab_traverse_from_search_and_buttons() {
        for button in [false, true] {
            assert_eq!(route("tab", button), Destination::Next);
            assert_eq!(route("shift-tab", button), Destination::Previous);
        }
    }

    #[test]
    fn focused_buttons_keep_native_activation_and_other_keys_edit_the_query() {
        for key in ["enter", "space"] {
            assert_eq!(route(key, true), Destination::Button);
            assert_eq!(route(key, false), Destination::Search);
        }
        for key in ["a", "left", "right", "up", "down", "backspace", "escape", "ctrl-1", "ctrl-4", "ctrl-a", "ctrl-tab", "alt-tab", "cmd-tab", "ctrl-enter"] {
            assert_eq!(route(key, true), Destination::Search, "{key}");
        }
    }

    #[test]
    fn classic_results_and_grid_categories_keep_their_keys() {
        for search in [false, true] {
            for key in ["tab", "shift-tab", "enter", "space", "up", "down", "left", "right"] {
                assert_eq!(destination(LauncherStyle::Classic, search, &Keystroke::parse(key).unwrap()), None);
            }
        }
    }
}
