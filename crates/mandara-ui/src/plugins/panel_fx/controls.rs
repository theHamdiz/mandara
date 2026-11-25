//! Shared keyboard activation for panel controls with pointer handlers.

use std::rc::Rc;
use gpui::{App, Div, InteractiveElement as _, Stateful, StatefulInteractiveElement as _, Styled as _, Window};
use mandara_core::ToGpui as _;
use mandara_theme::MandaraPalette;

pub type Action = Rc<dyn Fn(&mut Window, &mut App)>;

fn activates(key: &str, modified: bool, held: bool) -> bool {
    !modified && !held && matches!(key, "enter" | "space")
}

pub fn keyboard(element: Stateful<Div>, palette: &MandaraPalette, action: Action) -> Stateful<Div> {
    let focus = palette.primary_container;
    let foreground = palette.on_surface;
    element.focusable().tab_index(0)
        .focus_visible(move |style| style.bg(focus.hsla()).text_color(foreground.hsla()))
        .on_key_down(move |event, window, cx| {
            if activates(&event.keystroke.key, event.keystroke.modifiers.modified(), event.is_held) {
                action(window, cx);
                cx.stop_propagation();
            }
        })
}

pub fn action(element: Stateful<Div>, palette: &MandaraPalette, action: Action) -> Stateful<Div> {
    let click = action.clone();
    keyboard(element, palette, action).on_click(move |_, window, cx| click(window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_unmodified_activation_keys_start_an_action_and_repeats_do_not_restart_it() {
        for key in ["enter", "space"] {
            assert!(activates(key, false, false));
            assert!(!activates(key, true, false));
            assert!(!activates(key, false, true));
        }
        for key in ["escape", "tab", "a"] { assert!(!activates(key, false, false)); }
    }
}
