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

/// The page whose tab bar has this element id, when third-party plugins may
/// add tabs to it ([`mandara_plugins::extension::STUDIO_TAB_PAGES`]).
fn plugin_page(id: &str) -> Option<&'static str> {
    match id {
        "themes-tabs" => Some("themes"),
        "bars-tabs" => Some("bars"),
        "desktop-tabs" => Some("desktop"),
        "wallpaper-tabs" => Some("wallpaper"),
        _ => None,
    }
}

/// The plugin tab chosen on each page: `(plugin id, tab id)`. Choosing a
/// built-in tab clears it; it is forgotten when the plugin's tab goes.
#[derive(Default)]
struct PluginTabChoice(std::collections::HashMap<&'static str, (String, String)>);
impl gpui::Global for PluginTabChoice {}

fn choose_plugin_tab(page: &'static str, choice: Option<(String, String)>, cx: &mut gpui::App) {
    let map = &mut cx.default_global::<PluginTabChoice>().0;
    match choice {
        Some(c) => map.insert(page, c),
        None => map.remove(page),
    };
}

/// The plugin tab shown on `page`, if one is chosen and still offered.
fn chosen_plugin_tab(page: &str, cx: &gpui::App) -> Option<(String, String)> {
    let chosen = cx.try_global::<PluginTabChoice>()?.0.get(page)?.clone();
    mandara_ui::plugins::external::studio_tabs(page, cx).iter().any(|(plugin, _, tab)| *plugin == chosen.0 && tab.id == chosen.1).then_some(chosen)
}

/// The chosen plugin tab's body for the tab bar `id`, drawn in place of the
/// page's own tab: an attribution line (it is a third-party plugin's
/// content) above the plugin's view. `None` when no plugin tab is chosen.
pub(super) fn plugin_body(id: &'static str, width: f32, p: &MandaraPalette, window: &mut Window, cx: &mut gpui::App) -> Option<AnyElement> {
    let page = plugin_page(id)?;
    let (plugin, tab) = chosen_plugin_tab(page, cx)?;
    let name = mandara_ui::plugins::external::studio_tabs(page, cx).into_iter().find(|(id, _, _)| *id == plugin).map(|(_, name, _)| name).unwrap_or_else(|| plugin.clone());
    let by = div()
        .row()
        .items_center()
        .gap(px(SPACE_6_PX))
        .text_size(px(TEXT_12_PX))
        .text_color(p.on_surface_variant.hsla())
        .child(icon("plug", ICON_12_PX, p.on_surface_variant))
        .child(mandara_i18n::tf("Added by the {plugin} plugin", &[("plugin", &name)]));
    let body = mandara_ui::plugins::external::studio_tab_view(&plugin, &tab, width, p, window, cx);
    Some(div().row().flex_col().gap(px(SPACE_14_PX)).child(by).child(body).into_any_element())
}

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
    last_selection: Option<&'static str>,
}

impl TabState {
    /// Returns a reveal target only when selection changes. Repeated renders
    /// leave manual scrolling alone, even if selection is outside the viewport.
    fn selected_target<T: PageTab>(&mut self, tabs: &[T], current: T) -> Option<usize> {
        if self.last_selection == Some(current.slug()) { return None; }
        self.last_selection = Some(current.slug());
        tabs.iter().position(|tab| *tab == current)
    }
}

/// Renders focusable tabs with arrow selection and scrolling to the focused tab.
/// Keep `id` stable and distinct from other tab bars in the window so their
/// focus and scroll state remain separate. `select` owns the page transition.
/// Native activation handles Enter/Space; arrows move focus in reading order.
/// A guarded frame callback retries scrolling after the initial layout,
/// without retaining the view or scheduling frames after state settles.
pub(super) fn bar<V: 'static, T: PageTab>(id: &'static str, tabs: &[T], current: T, p: &MandaraPalette, select: fn(&mut V, T, &mut Window, &mut Context<V>), window: &mut Window, cx: &mut Context<V>) -> AnyElement {
    let state = window.use_keyed_state(SharedString::from(format!("{id}-state")), cx, |_, _| TabState { scroll: ScrollHandle::new(), focus: Vec::new(), last_focus: None, last_selection: None });
    let (scroll, focus, reveal, focused) = state.update(cx, |state, cx| {
        state.focus.resize_with(tabs.len(), || cx.focus_handle().tab_index(0).tab_stop(true));
        let mut reveal = state.selected_target(tabs, current);
        let focused = state.focus.iter().position(|handle| handle.is_focused(window));
        if focused != state.last_focus {
            if focused.is_some() { reveal = focused; }
            state.last_focus = focused;
        }
        if let Some(index) = reveal { state.scroll.scroll_to_item(index); }
        (state.scroll.clone(), state.focus.clone(), reveal, focused)
    });
    if let Some(index) = reveal {
        let state = state.downgrade();
        let view = cx.entity().downgrade();
        // The first prepaint initializes the scroll viewport. Retry once after
        // that layout, provided navigation has not moved to another tab.
        window.on_next_frame(move |_, cx| {
            let Some(state) = state.upgrade() else { return; };
            {
                let state = state.read(cx);
                if state.last_selection != Some(current.slug()) || state.last_focus != focused { return; }
                state.scroll.scroll_to_item(index);
            }
            let _ = view.update(cx, |_, cx| cx.notify());
        });
    }
    let mut row = div().id(id).row().overflow_x_scroll().track_scroll(&scroll).border_b_1().border_color(p.outline.with_alpha(ALPHA_35).hsla());
    let choices: Rc<[T]> = Rc::from(tabs);
    let page = plugin_page(id);
    let plugin_chosen = page.and_then(|page| chosen_plugin_tab(page, cx));
    for (at, &tab) in tabs.iter().enumerate() {
        let on = tab == current && plugin_chosen.is_none();
        let choices = choices.clone();
        row = row.child(div()
            .id(SharedString::from(format!("{id}-{}", tab.slug())))
            .relative().flex_none().h(px(DIM_36_PX)).px(px(SPACE_10_PX))
            .row().items_center().gap(px(SPACE_6_PX)).cursor_pointer().track_focus(&focus[at]).tab_index(0)
            .text_size(px(TEXT_13_PX))
            .focus(|style| style.bg(p.surface_container_high.hsla()).text_color(p.on_surface.hsla()))
            .when(on, |el| el.text_color(p.on_surface.hsla()).font_weight(FontWeight::SEMIBOLD))
            .when(!on, |el| el.text_color(p.on_surface_variant.hsla()).hover(|style| style.text_color(p.on_surface.hsla())))
            .on_click(cx.listener(move |view, _: &ClickEvent, window, cx| {
                if let Some(page) = page {
                    choose_plugin_tab(page, None, cx);
                }
                select(view, tab, window, cx);
            }))
            .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift { return; }
                let Some(next) = arrow_target(at, choices.len(), &event.keystroke.key, mandara_i18n::language().is_rtl()) else { return };
                if next > at { for _ in at..next { window.focus_next(cx); } }
                else { for _ in next..at { window.focus_prev(cx); } }
                if let Some(page) = page {
                    choose_plugin_tab(page, None, cx);
                }
                select(view, choices[next], window, cx);
                cx.stop_propagation();
            }))
            .child(icon(tab.icon(), ICON_14_PX, if on { p.primary } else { p.on_surface_variant }))
            .child(tab.label())
            .when(on, |el| el.child(div().absolute().bottom_0().left(px(SPACE_8_PX)).right(px(SPACE_8_PX)).h(px(TAB_UNDERLINE_PX)).rounded_full().bg(p.primary.hsla()))));
    }
    // Tabs third-party plugins add, after the page's own (Enter or Space
    // chooses one, like a click).
    if let Some(page) = page {
        for (plugin, _, tab) in mandara_ui::plugins::external::studio_tabs(page, cx) {
            let on = plugin_chosen.as_ref().is_some_and(|(p, t)| *p == plugin && *t == tab.id);
            let icon_name = mandara_ui::icons::ICON_NAMES.iter().find(|x| **x == tab.icon).copied().unwrap_or("plug");
            let choice = (plugin.clone(), tab.id.clone());
            let (click, key) = (choice.clone(), choice);
            row = row.child(div()
                .id(SharedString::from(format!("{id}-plugin-{plugin}-{}", tab.id)))
                .relative().flex_none().h(px(DIM_36_PX)).px(px(SPACE_10_PX))
                .row().items_center().gap(px(SPACE_6_PX)).cursor_pointer().tab_index(0)
                .text_size(px(TEXT_13_PX))
                .focus(|style| style.bg(p.surface_container_high.hsla()).text_color(p.on_surface.hsla()))
                .when(on, |el| el.text_color(p.on_surface.hsla()).font_weight(FontWeight::SEMIBOLD))
                .when(!on, |el| el.text_color(p.on_surface_variant.hsla()).hover(|style| style.text_color(p.on_surface.hsla())))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    choose_plugin_tab(page, Some(click.clone()), cx);
                    cx.notify();
                }))
                .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        choose_plugin_tab(page, Some(key.clone()), cx);
                        cx.notify();
                        cx.stop_propagation();
                    }
                }))
                .child(icon(icon_name, ICON_14_PX, if on { p.primary } else { p.on_surface_variant }))
                .child(tab.title.clone())
                .when(on, |el| el.child(div().absolute().bottom_0().left(px(SPACE_8_PX)).right(px(SPACE_8_PX)).h(px(TAB_UNDERLINE_PX)).rounded_full().bg(p.primary.hsla()))));
        }
    }
    row.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_and_changed_selection_are_revealed_without_resetting_manual_scroll() {
        use crate::registry::bars::BarsTab;
        let tabs = BarsTab::ALL;
        let mut state = TabState { scroll: ScrollHandle::new(), focus: Vec::new(), last_focus: None, last_selection: None };
        assert_eq!(state.selected_target(&tabs, BarsTab::Edges), Some(3));
        assert_eq!(state.selected_target(&tabs, BarsTab::Edges), None);
        assert_eq!(state.selected_target(&tabs, BarsTab::Bar), Some(0));
        assert_eq!(state.selected_target(&tabs, BarsTab::Bar), None);
    }

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
