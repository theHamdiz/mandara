//! Keyboard focus between Beacon's query, modes and app categories.

use gpui::{App, Context, FocusHandle, Keystroke, ScrollHandle, Window};
use mandara_config::workflow::LauncherStyle;
use std::rc::Rc;

#[derive(Debug, PartialEq, Eq)]
enum Destination {
    Next,
    Previous,
    Button,
    Search,
}

#[derive(Default)]
struct Categories {
    scroll: ScrollHandle,
    focus: Rc<[FocusHandle]>,
    modes: usize,
    rtl: Option<bool>,
    last_focus: Option<usize>,
}

impl Categories {
    /// Reveal changed keyboard focus without undoing manual scrolling on repaint.
    fn reveal(&mut self, focused: Option<usize>) -> Option<usize> {
        if focused == self.last_focus {
            return None;
        }
        self.last_focus = focused;
        focused
    }
}

/// Search is first, then visible modes, then All and the available categories.
pub(super) fn category_index(modes: usize, category: usize) -> isize {
    (modes + category + 1) as isize
}

/// Reversed insertion keeps RTL overflow inside GPUI's negative scroll range.
pub(super) fn category_child(index: usize, count: usize, rtl: bool) -> usize {
    if rtl { count - 1 - index } else { index }
}

/// Keep native category focus visible, including after its first layout.
/// The guarded one-frame retry does not retain the view or poll while hidden.
pub(super) fn categories<V: 'static>(
    count: usize,
    modes: usize,
    window: &mut Window,
    cx: &mut Context<V>,
) -> (ScrollHandle, Rc<[FocusHandle]>) {
    let state = window.use_keyed_state("beacon-category-focus", cx, |_, _| Categories::default());
    let rtl = crate::bidi::rtl();
    let (scroll, focus, reveal, focused) = state.update(cx, |state, cx| {
        let changed = state.focus.len() != count || state.modes != modes || state.rtl != Some(rtl);
        if state.focus.len() != count || state.modes != modes {
            let mut focus = state.focus.to_vec();
            focus.resize_with(count, || cx.focus_handle().tab_stop(true));
            for (index, handle) in focus.iter_mut().enumerate() {
                *handle = handle.clone().tab_index(category_index(modes, index));
            }
            state.focus = focus.into();
            state.modes = modes;
        }
        state.rtl = Some(rtl);
        let focused = state
            .focus
            .iter()
            .position(|focus| focus.is_focused(window));
        let reveal = state
            .reveal(focused)
            .or_else(|| changed.then_some(focused.unwrap_or(0)));
        if let Some(index) = reveal {
            state
                .scroll
                .scroll_to_item(category_child(index, count, rtl));
        }
        (state.scroll.clone(), state.focus.clone(), reveal, focused)
    });
    if let Some(index) = reveal {
        let weak = state.downgrade();
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            {
                let state = state.read(cx);
                if state.rtl != Some(rtl)
                    || state.focus.len() != count
                    || state.modes != modes
                    || state.last_focus != focused
                {
                    return;
                }
                state
                    .scroll
                    .scroll_to_item(category_child(index, count, rtl));
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        });
    }
    (scroll, focus)
}

/// Beacon reserves Tab for focus and leaves activation with a focused button;
/// other keys return to search. Other launcher styles keep their own routing.
fn destination(style: LauncherStyle, button_focused: bool, key: &Keystroke) -> Option<Destination> {
    if style != LauncherStyle::Beacon {
        return None;
    }
    let modifiers = key.modifiers;
    if key.key == "tab" && !modifiers.control && !modifiers.alt && !modifiers.platform {
        return Some(if modifiers.shift {
            Destination::Previous
        } else {
            Destination::Next
        });
    }
    if button_focused && matches!(key.key.as_str(), "enter" | "space") && !modifiers.modified() {
        return Some(Destination::Button);
    }
    Some(Destination::Search)
}

/// Return true when the query handler must leave this key to native focus or
/// button activation. Typing and result navigation return focus to the query.
pub(super) fn handle(
    style: LauncherStyle,
    search: &FocusHandle,
    key: &Keystroke,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let button_focused = !search.is_focused(window) && search.contains_focused(window, cx);
    match destination(style, button_focused, key) {
        Some(Destination::Next) => window.focus_next(cx),
        Some(Destination::Previous) => window.focus_prev(cx),
        // GPUI pairs Enter/Space keydown and keyup into the button's on_click.
        Some(Destination::Button) => return true,
        Some(Destination::Search) => {
            if !search.is_focused(window) {
                search.focus(window, cx);
            }
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

    /// Parse the same key spelling used by GPUI's native event routing.
    fn route(key: &'static str, button: bool) -> Destination {
        destination(
            LauncherStyle::Beacon,
            button,
            &Keystroke::parse(key).unwrap(),
        )
        .unwrap()
    }

    /// Hidden providers leave no holes or collisions before the category stops.
    #[test]
    fn category_order_follows_only_the_visible_modes() {
        for modes in [3, 4] {
            assert_eq!(category_index(modes, 0), modes as isize + 1);
            assert_eq!(category_index(modes, 1), modes as isize + 2);
        }
    }

    /// Reading-order focus reaches the opposite physical endpoint in RTL.
    #[test]
    fn rtl_categories_reveal_the_reversed_physical_child() {
        assert_eq!(category_child(0, 12, true), 11);
        assert_eq!(category_child(11, 12, true), 0);
        assert_eq!(category_child(0, 12, false), 0);
        assert_eq!(category_child(11, 12, false), 11);
    }

    /// A stable focus leaves wheel offsets alone; returning to a chip reveals it.
    #[test]
    fn only_changed_category_focus_requests_scroll() {
        let mut state = Categories::default();
        assert_eq!(state.reveal(None), None);
        assert_eq!(state.reveal(Some(4)), Some(4));
        assert_eq!(state.reveal(Some(4)), None);
        assert_eq!(state.reveal(None), None);
        assert_eq!(state.reveal(Some(0)), Some(0));
    }

    /// Traversal direction stays the same whether search or a mode has focus.
    #[test]
    fn tab_and_shift_tab_traverse_from_search_and_buttons() {
        for button in [false, true] {
            assert_eq!(route("tab", button), Destination::Next);
            assert_eq!(route("shift-tab", button), Destination::Previous);
        }
    }

    /// Only unmodified activation keys stay with a button; query edits and
    /// existing launcher shortcuts continue through the search handler.
    #[test]
    fn focused_buttons_keep_native_activation_and_other_keys_edit_the_query() {
        for key in ["enter", "space"] {
            assert_eq!(route(key, true), Destination::Button);
            assert_eq!(route(key, false), Destination::Search);
        }
        for key in [
            "a",
            "left",
            "right",
            "up",
            "down",
            "backspace",
            "escape",
            "ctrl-1",
            "ctrl-4",
            "ctrl-a",
            "ctrl-tab",
            "alt-tab",
            "cmd-tab",
            "ctrl-enter",
        ] {
            assert_eq!(route(key, true), Destination::Search, "{key}");
        }
    }

    /// Beacon's focus routing must leave Classic and grid navigation alone.
    #[test]
    fn classic_results_and_grid_categories_keep_their_keys() {
        for search in [false, true] {
            for key in [
                "tab",
                "shift-tab",
                "enter",
                "space",
                "up",
                "down",
                "left",
                "right",
            ] {
                assert_eq!(
                    destination(
                        LauncherStyle::Classic,
                        search,
                        &Keystroke::parse(key).unwrap()
                    ),
                    None
                );
            }
        }
    }
}
