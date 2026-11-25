//! The font picker: every font setting is a searchable drop-down of the
//! families installed on this system, each shown in its own face. Typing a
//! name that isn't installed offers it anyway ("Use “…”"), so a font you're
//! about to install can be set now; it falls back like any missing family.
//!
//! [`FontSelect`] is one picker (Studio's settings and the login screen
//! page each keep one per setting); it reports a pick through its callback.
//! The trigger keeps its size in its row; the list is a floating popover
//! ([`super::popover`]) under the trigger, or above it when there's more
//! room there, with a search field focused on open and keyboard navigation
//! (Up / Down / Page Up / Page Down / Home / End, Enter picks, Esc closes).

use std::rc::Rc;

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::text_field::TextField;

use super::popover::{self, Anchor, Appear, TriggerBounds};
use crate::tokens::*;

/// Families listed at once (the search narrows them).
const MAX_LISTED: usize = 80;
/// The list scrolls past this height.
const LIST_H_PX: f32 = 280.0;
/// The trigger's width in its row.
const PICKER_W_PX: f32 = 300.0;
/// The popover's own width (not its container's).
const POPOVER_W_PX: f32 = 340.0;
/// Sample size in the list.
const SAMPLE_PX: f32 = 15.0;
/// One row of the list.
const ITEM_H_PX: f32 = DIM_34_PX;
/// Rows Page Up / Page Down move.
const PAGE_ROWS: usize = 8;
/// The popover's padding, the search field and the gap under it.
const CHROME_H_PX: f32 = 2.0 * SPACE_6_PX + DIM_36_PX + SPACE_6_PX;

/// What a pick does: receives the family (empty = back to the default).
pub type OnPick = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Installed families, sorted and once each (hidden `.`-families left out).
pub fn installed(cx: &App) -> Rc<Vec<String>> {
    let mut names: Vec<String> = cx.text_system().all_font_names().into_iter().filter(|n| !n.starts_with('.') && !n.trim().is_empty()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    Rc::new(names)
}

/// The families matching `query` (every word, any case), best first:
/// prefix matches before the rest.
pub fn matches<'a>(names: &'a [String], query: &str) -> Vec<&'a String> {
    let q = query.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<&String> = names.iter().filter(|n| words.iter().all(|w| n.to_lowercase().contains(w))).collect();
    hits.sort_by_key(|n| !n.to_lowercase().starts_with(&q));
    hits.truncate(MAX_LISTED);
    hits
}

/// The highlighted row after a navigation key, in a list of `len` rows.
fn step(key: &str, at: usize, len: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    Some(match key {
        "down" => (at + 1).min(last),
        "up" => at.saturating_sub(1),
        "pagedown" => (at + PAGE_ROWS).min(last),
        "pageup" => at.saturating_sub(PAGE_ROWS),
        "home" => 0,
        "end" => last,
        _ => return None,
    })
}

/// One row of the open list: what it says, the face it's drawn in and the
/// family it picks.
struct Item {
    label: SharedString,
    face: Option<SharedString>,
    chosen: bool,
    value: String,
}

/// One font picker.
pub struct FontSelect {
    id: SharedString,
    value: String,
    /// What an empty value means (offered as "Default"); `None`: a family
    /// is always set.
    fallback: Option<String>,
    /// The search field, while the popover is open or closing.
    search: Option<Entity<TextField>>,
    names: Option<Rc<Vec<String>>>,
    on_pick: OnPick,
    trigger: TriggerBounds,
    appear: Appear,
    /// The highlighted row (keyboard), and the query it belongs to.
    cursor: (usize, String),
}

impl FontSelect {
    pub fn new(id: impl Into<SharedString>, on_pick: OnPick) -> Self {
        Self { id: id.into(), value: String::new(), fallback: None, search: None, names: None, on_pick, trigger: TriggerBounds::default(), appear: Appear::default(), cursor: (0, String::new()) }
    }

    /// Shows `value` and what empty means.
    pub fn sync(&mut self, value: &str, fallback: Option<&str>) {
        self.value = value.to_string();
        self.fallback = fallback.map(str::to_string);
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.appear.is_open() {
            self.close(cx);
            return;
        }
        let me = cx.entity().downgrade();
        let search = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search fonts, or type any name"),
                true,
                Rc::new(move |_, _, cx| {
                    let _ = me.update(cx, |_, cx| cx.notify());
                }),
                cx,
            )
        });
        search.update(cx, |f, cx| f.focus(window, cx));
        self.names.get_or_insert_with(|| installed(cx));
        self.search = Some(search);
        self.cursor = (usize::MAX, String::new());
        self.appear.open();
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.appear.close();
        cx.notify();
    }

    fn pick(&mut self, family: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close(cx);
        self.value = family.clone();
        (self.on_pick)(family, window, cx);
    }

    /// The rows for `query`: "Use “…”" for a name that isn't installed,
    /// "Default", then the matching families.
    fn items(&mut self, query: &str, cx: &App) -> Vec<Item> {
        let names = self.names.get_or_insert_with(|| installed(cx)).clone();
        let exact = names.iter().any(|n| n.eq_ignore_ascii_case(query));
        let mut items = Vec::new();
        if !query.is_empty() && !exact {
            items.push(Item { label: tf("Use “{font}”", &[("font", &query)]).into(), face: None, chosen: false, value: query.to_string() });
        }
        if self.fallback.is_some() && query.is_empty() {
            items.push(Item { label: t("Default").into(), face: None, chosen: self.value.trim().is_empty(), value: String::new() });
        }
        for name in matches(&names, query) {
            items.push(Item { label: name.clone().into(), face: Some(name.clone().into()), chosen: name.eq_ignore_ascii_case(&self.value), value: name.clone() });
        }
        items
    }

    fn popover(&mut self, appear: f32, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let search = self.search.clone()?;
        let bounds = self.trigger.get()?;
        let query = search.read(cx).value.trim().to_string();
        let items = self.items(&query, cx);
        // The highlight starts on the current family (or the first row) and
        // goes back there whenever the search changes.
        let key = format!("{}-list", self.id);
        let (scroll, bar) = mandara_ui::scrollbar::keyed(key.clone());
        if self.cursor.0 == usize::MAX || self.cursor.1 != query {
            self.cursor = (items.iter().position(|i| i.chosen).unwrap_or(0), query.clone());
            scroll.scroll_to_item(self.cursor.0);
        }
        let cursor = self.cursor.0.min(items.len().saturating_sub(1));
        let rtl = mandara_i18n::language().is_rtl();
        let viewport = window.viewport_size();
        let list_h = (items.len().max(1) as f32 * ITEM_H_PX + 2.0 * SPACE_4_PX).min(LIST_H_PX);
        let anchor = Anchor::place((f32::from(bounds.origin.x), f32::from(bounds.origin.y), f32::from(bounds.size.width), f32::from(bounds.size.height)), CHROME_H_PX + list_h, f32::from(viewport.height), rtl);
        let list_h = list_h.min((anchor.max_h - CHROME_H_PX).max(ITEM_H_PX));

        let picks: Vec<String> = items.iter().map(|i| i.value.clone()).collect();
        let rows: Vec<gpui::AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(n, item)| {
                let value = item.value;
                let lit = n == cursor;
                div()
                    .id(("font-item", n))
                    .flex_none()
                    .h(px(ITEM_H_PX))
                    .px(px(SPACE_10_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .rounded(px(RADIUS_10_PX))
                    .cursor_pointer()
                    .when(lit, |el| el.bg(p.primary.with_alpha(ALPHA_14).hsla()))
                    .when(!lit, |el| el.hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla())))
                    .child(div().flex_none().w(px(ICON_14_PX)).when(item.chosen, |el| el.child(mandara_ui::icons::icon("check", ICON_14_PX, p.primary))))
                    .child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(SAMPLE_PX)).text_color(if item.chosen { p.primary.hsla() } else { p.on_surface.hsla() }).when_some(item.face, |el, f| el.font_family(f)).child(item.label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.pick(value.clone(), window, cx)))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        let list = div()
            .id(SharedString::from(key))
            .h(px(list_h))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .p(px(SPACE_4_PX))
            .row()
            .flex_col()
            .children(rows)
            .when(empty, |el| el.child(div().h(px(ITEM_H_PX)).px(px(SPACE_10_PX)).row().items_center().text_color(p.on_surface_variant.hsla()).child(t("No fonts match"))));
        let panel = popover::surface(POPOVER_W_PX, p)
            .p(px(SPACE_6_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            // The field draws its own box.
            .child(div().h(px(DIM_36_PX)).row().items_center().child(div().flex_1().min_w(px(0.0)).child(search)))
            .child(mandara_ui::scrollbar::with_scrollbar(list, &scroll, &bar, p))
            .capture_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let key = e.keystroke.key.as_str();
                match key {
                    "escape" => this.close(cx),
                    "enter" => {
                        if let Some(v) = picks.get(this.cursor.0.min(picks.len().saturating_sub(1))).cloned() {
                            this.pick(v, window, cx);
                        }
                    }
                    _ => {
                        let Some(next) = step(key, this.cursor.0.min(picks.len().saturating_sub(1)), picks.len()) else { return };
                        this.cursor.0 = next;
                        scroll.scroll_to_item(next);
                        cx.notify();
                    }
                }
                cx.stop_propagation();
            }));
        let trigger = self.trigger.clone();
        let me = cx.entity().downgrade();
        Some(popover::layer("font-popover", anchor, appear, panel, move |window, cx| {
            // A press on the trigger toggles it itself.
            if trigger.get().is_some_and(|b| b.contains(&window.mouse_position())) {
                return;
            }
            let _ = me.update(cx, |this, cx| this.close(cx));
        }))
    }
}

impl Render for FontSelect {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        let reduce = mandara_ui::state::config(cx).read(cx).get().motion.reduce_motion;
        let appear = if self.search.is_some() { self.appear.frame(reduce, window) } else { None };
        if appear.is_none() {
            // Closed (and done animating): let the field go.
            self.search = None;
        }
        let open = self.appear.is_open();
        let empty = self.value.trim().is_empty();
        let label: SharedString = match &self.fallback {
            Some(f) if empty && f.is_empty() => t("Default").into(),
            Some(f) if empty => tf("Default ({font})", &[("font", f)]).into(),
            _ => self.value.clone().into(),
        };
        let face: SharedString = if empty { self.fallback.clone().unwrap_or_default().into() } else { self.value.clone().into() };
        let popover = appear.and_then(|a| self.popover(a, &p, window, cx));
        div()
            .id(self.id.clone())
            .relative()
            .flex_none()
            .w(px(PICKER_W_PX))
            .h(px(DIM_36_PX))
            .px(px(SPACE_12_PX))
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .rounded(px(RADIUS_12_PX))
            .bg(p.surface_container_high.hsla())
            .border_1()
            .border_color(if open { p.primary.hsla() } else { p.on_surface.with_alpha(ALPHA_10).hsla() })
            .cursor_pointer()
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_55).hsla()))
            .child(self.trigger.tracker())
            .child(div().flex_1().min_w(px(0.0)).truncate().text_start().when(!face.is_empty(), |el| el.font_family(face)).text_size(px(TEXT_14_PX)).child(label))
            .child(mandara_ui::icons::icon(if open { "arrow-up" } else { "arrow-down" }, ICON_14_PX, if open { p.primary } else { p.on_surface_variant }))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle(window, cx)))
            .children(popover)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_every_word_prefixes_first() {
        let names: Vec<String> = ["Noto Sans Arabic", "Inter", "Inter Display", "JetBrains Mono", "Cairo", "Noto Sans"].map(String::from).to_vec();
        assert_eq!(matches(&names, "inter"), [&names[1], &names[2]]);
        assert_eq!(matches(&names, "sans noto"), [&names[0], &names[5]]);
        assert_eq!(matches(&names, "no"), [&names[0], &names[5], &names[3]], "prefix matches first");
        assert_eq!(matches(&names, "").len(), names.len());
        assert!(matches(&names, "zzz").is_empty());
    }

    #[test]
    fn keys_move_the_highlight_within_the_list() {
        assert_eq!(step("down", 0, 3), Some(1));
        assert_eq!(step("down", 2, 3), Some(2));
        assert_eq!(step("up", 0, 3), Some(0));
        assert_eq!(step("pagedown", 1, 30), Some(1 + PAGE_ROWS));
        assert_eq!(step("pageup", 3, 30), Some(0));
        assert_eq!((step("home", 7, 9), step("end", 0, 9)), (Some(0), Some(8)));
        assert_eq!(step("down", 0, 0), None);
        assert_eq!(step("a", 0, 3), None);
    }
}
