//! The font picker: every font setting is a searchable drop-down of the
//! families installed on this system, each shown in its own face. Typing a
//! name that isn't installed offers it anyway ("Use “…”"), so a font you're
//! about to install can be set now; it falls back like any missing family.
//!
//! [`FontSelect`] is one picker (Studio's settings and the login screen
//! page each keep one per setting); it reports a pick through its callback.

use std::rc::Rc;

use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::text_field::TextField;

use crate::tokens::*;

/// Families listed at once (the search narrows them).
const MAX_LISTED: usize = 80;
/// The list scrolls past this height.
const LIST_H_PX: f32 = 260.0;
/// The button and the list are this wide.
const PICKER_W_PX: f32 = 300.0;
/// Sample size in the list.
const SAMPLE_PX: f32 = 15.0;

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

/// One font picker.
pub struct FontSelect {
    id: SharedString,
    value: String,
    /// What an empty value means (offered as "Default"); `None`: a family
    /// is always set.
    fallback: Option<String>,
    search: Option<Entity<TextField>>,
    names: Option<Rc<Vec<String>>>,
    on_pick: OnPick,
}

impl FontSelect {
    pub fn new(id: impl Into<SharedString>, on_pick: OnPick) -> Self {
        Self { id: id.into(), value: String::new(), fallback: None, search: None, names: None, on_pick }
    }

    /// Shows `value` and what empty means.
    pub fn sync(&mut self, value: &str, fallback: Option<&str>) {
        self.value = value.to_string();
        self.fallback = fallback.map(str::to_string);
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.take().is_none() {
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
        }
        cx.notify();
    }

    fn pick(&mut self, family: String, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self.value = family.clone();
        (self.on_pick)(family, window, cx);
        cx.notify();
    }

    fn list(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(search) = self.search.clone() else { return div().into_any_element() };
        let query = search.read(cx).value.trim().to_string();
        let names = self.names.get_or_insert_with(|| installed(cx)).clone();
        let hits = matches(&names, &query);
        let exact = names.iter().any(|n| n.eq_ignore_ascii_case(&query));
        let current = self.value.clone();
        let mut items: Vec<gpui::AnyElement> = Vec::new();
        let mut item = |id: SharedString, label: SharedString, face: Option<SharedString>, chosen: bool, choose: String, cx: &mut Context<Self>| {
            items.push(
                div()
                    .id(id)
                    .h(px(DIM_34_PX))
                    .px(px(SPACE_10_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .rounded(px(RADIUS_8_PX))
                    .cursor_pointer()
                    .when(chosen, |el| el.bg(p.primary.with_alpha(ALPHA_16).hsla()))
                    .hover(|s| s.bg(p.surface_container_high.hsla()))
                    .child(div().w(px(ICON_14_PX)).when(chosen, |el| el.child(mandara_ui::icons::icon("check", ICON_14_PX, p.primary))))
                    .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(SAMPLE_PX)).when_some(face, |el, f| el.font_family(f)).child(label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.pick(choose.clone(), window, cx)))
                    .into_any_element(),
            );
        };
        if !query.is_empty() && !exact {
            item("font-use-typed".into(), tf("Use “{font}”", &[("font", &query)]).into(), None, false, query.clone(), cx);
        }
        if self.fallback.is_some() && query.is_empty() {
            item("font-default".into(), t("Default").into(), None, current.trim().is_empty(), String::new(), cx);
        }
        for (i, name) in hits.into_iter().enumerate() {
            item(SharedString::from(format!("font-{i}")), name.clone().into(), Some(name.clone().into()), name.eq_ignore_ascii_case(&current), name.clone(), cx);
        }
        if items.is_empty() {
            items.push(div().h(px(DIM_34_PX)).px(px(SPACE_10_PX)).row().items_center().text_color(p.on_surface_variant.hsla()).child(t("No fonts match")).into_any_element());
        }
        let key = format!("{}-list", self.id);
        let (scroll, bar) = mandara_ui::scrollbar::keyed(key.clone());
        let list = div().id(SharedString::from(key)).max_h(px(LIST_H_PX)).overflow_y_scroll().track_scroll(&scroll).child(div().row().flex_col().p(px(SPACE_4_PX)).children(items));
        div()
            .w(px(PICKER_W_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .p(px(SPACE_6_PX))
            .rounded(px(RADIUS_12_PX))
            .bg(p.surface_container.hsla())
            .border_1()
            .border_color(p.outline.with_alpha(ALPHA_35).hsla())
            .child(div().h(px(DIM_34_PX)).px(px(SPACE_10_PX)).row().items_center().rounded(px(RADIUS_8_PX)).bg(p.surface_container_high.hsla()).child(search))
            .child(mandara_ui::scrollbar::with_scrollbar(list, &scroll, &bar, p))
            .into_any_element()
    }
}

impl Render for FontSelect {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = mandara_core::frame_palette(window, cx);
        let open = self.search.is_some();
        let empty = self.value.trim().is_empty();
        let label: SharedString = match &self.fallback {
            Some(f) if empty && f.is_empty() => t("Default").into(),
            Some(f) if empty => tf("Default ({font})", &[("font", f)]).into(),
            _ => self.value.clone().into(),
        };
        let face: SharedString = if empty { self.fallback.clone().unwrap_or_default().into() } else { self.value.clone().into() };
        let button = div()
            .id(self.id.clone())
            .w(px(PICKER_W_PX))
            .h(px(DIM_36_PX))
            .px(px(SPACE_12_PX))
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .rounded(px(RADIUS_10_PX))
            .bg(p.surface_container_high.hsla())
            .border_1()
            .border_color(if open { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_35).hsla() })
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_container.hsla()))
            .child(div().flex_1().min_w(px(0.0)).truncate().when(!face.is_empty(), |el| el.font_family(face)).text_size(px(TEXT_14_PX)).child(label))
            .child(mandara_ui::icons::icon(if open { "chevron-up" } else { "chevron-down" }, ICON_14_PX, p.on_surface_variant))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.toggle(window, cx)));
        let mut col = div().row().flex_col().gap(px(SPACE_6_PX)).child(button);
        if open {
            col = col.child(self.list(&p, cx));
        }
        col
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
}
