//! Underlined page tabs with native activation and scoped arrow navigation.

use gpui::{AnyElement, ClickEvent, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;

use crate::tokens::*;
use std::rc::Rc;

/// The active page tab has a two-pixel underline in every Studio page.
const TAB_UNDERLINE_PX: f32 = 2.0;

/// Supplies stable identity and display text without tying tabs to one page.
pub(super) trait PageTab: Copy + Eq + 'static {
    /// Stable element identifier, unique within the tab bar.
    fn slug(self) -> &'static str;
    /// Visible text, already translated into the current interface language.
    fn label(self) -> String;
    /// Name in the shell's shared icon set.
    fn icon(self) -> &'static str;
}

/// Resolves horizontal arrows in reading order, wrapping at either end.
/// Home and End select array endpoints; activation and unrelated keys stay
/// with GPUI. A nonempty list requires `at < count`.
fn arrow_target(at: usize, count: usize, key: &str, rtl: bool) -> Option<usize> {
    if count == 0 { return None; }
    match key {
        "left" | "right" => {
            let forward = (key == "right") != rtl;
            Some(if forward { (at + 1) % count } else { (at + count - 1) % count })
        }
        "home" => Some(0), "end" => Some(count - 1), _ => None,
    }
}

struct TabState {
    scroll: ScrollHandle,
    focus: Vec<FocusHandle>,
    last_focus: Option<usize>,
}

/// Renders focusable tabs with arrow selection and scrolling to the focused tab.
/// Keep `id` stable and distinct from other tab bars in the window so their
/// focus and scroll state remain separate. `select` owns the page transition.
pub(super) fn bar<V: 'static, T: PageTab>(id: &'static str, tabs: &[T], current: T, p: &MandaraPalette, select: fn(&mut V, T, &mut Window, &mut Context<V>), window: &mut Window, cx: &mut Context<V>) -> AnyElement {
    let state = window.use_keyed_state(SharedString::from(format!("{id}-state")), cx, |_, _| TabState { scroll: ScrollHandle::new(), focus: Vec::new(), last_focus: None });
    let (scroll, focus) = state.update(cx, |state, cx| {
        state.focus.resize_with(tabs.len(), || cx.focus_handle());
        let focused = state.focus.iter().position(|handle| handle.is_focused(window));
        if focused != state.last_focus {
            if let Some(index) = focused { state.scroll.scroll_to_item(index); }
            state.last_focus = focused;
        }
        (state.scroll.clone(), state.focus.clone())
    });
    let mut row = div().id(id).row().overflow_x_scroll().track_scroll(&scroll).border_b_1().border_color(p.outline.with_alpha(ALPHA_35).hsla());
    let choices: Rc<[T]> = Rc::from(tabs);
    for (at, &tab) in tabs.iter().enumerate() {
        let on = tab == current;
        let choices = choices.clone();
        row = row.child(div()
            .id(SharedString::from(format!("{id}-{}", tab.slug())))
            .relative().flex_none().h(px(DIM_36_PX)).px(px(SPACE_10_PX))
            .row().items_center().gap(px(SPACE_6_PX)).cursor_pointer().track_focus(&focus[at]).tab_index(0)
            .text_size(px(TEXT_13_PX))
            .focus(|style| style.bg(p.surface_container_high.hsla()).text_color(p.on_surface.hsla()))
            .when(on, |el| el.text_color(p.on_surface.hsla()).font_weight(FontWeight::SEMIBOLD))
            .when(!on, |el| el.text_color(p.on_surface_variant.hsla()).hover(|style| style.text_color(p.on_surface.hsla())))
            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| select(view, tab, window, cx)))
            .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift { return; }
                let Some(next) = arrow_target(at, choices.len(), &event.keystroke.key, mandara_i18n::language().is_rtl()) else { return };
                if next > at { for _ in at..next { window.focus_next(cx); } }
                else { for _ in next..at { window.focus_prev(cx); } }
                select(view, choices[next], window, cx);
                cx.stop_propagation();
            }))
            .child(icon(tab.icon(), ICON_14_PX, if on { p.primary } else { p.on_surface_variant }))
            .child(tab.label())
            .when(on, |el| el.child(div().absolute().bottom_0().left(px(SPACE_8_PX)).right(px(SPACE_8_PX)).h(px(TAB_UNDERLINE_PX)).rounded_full().bg(p.primary.hsla()))));
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_navigation_wraps_in_reading_order_without_handling_activation() {
        assert_eq!(arrow_target(0, 5, "left", false), Some(4));
        assert_eq!(arrow_target(4, 5, "right", false), Some(0));
        assert_eq!(arrow_target(0, 5, "right", true), Some(4));
        assert_eq!(arrow_target(4, 5, "left", true), Some(0));
        assert_eq!(arrow_target(2, 5, "home", true), Some(0));
        assert_eq!(arrow_target(2, 5, "end", false), Some(4));
        for key in ["enter", "space", "tab", "up", "escape"] { assert_eq!(arrow_target(0, 5, key, false), None); }
        assert_eq!(arrow_target(0, 0, "left", false), None);
    }
}
