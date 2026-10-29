//! Studio → Input: keyboard layouts (order, variants, the switch shortcut,
//! the bar's short labels), repeat and Num Lock, mouse and touchpad
//! (acceleration, speed, natural scrolling, tapping…) and
//! focus-follows-pointer. The cursor theme, size and hiding moved to
//! Themes → Cursor (`cursor_page.rs`); a card here links there.
//!
//! Everything is written to the compositor drop-in Mandara owns
//! (`[compositor.input]` → `mango.conf` / `niri.kdl` `input {}` /
//! `hyprland.conf` `input {}`), never to your own compositor file; "Your
//! config" keeps whatever your file says.

use mandara_ui::bidi::Bidi as _;
use std::rc::Rc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::compositor::Target;
use mandara_config::input::{self, KeyboardLayout, XkbCatalog};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use super::compositor_page::{badge, card, chip, group_title, heading, target_name, tip};
use super::form;
use crate::registry::system_settings::{FOCUS, KEYBOARD, MOUSE, TOUCHPAD};
use crate::registry::{self, Section, Setting};
use crate::tokens::*;

/// Layout search results shown at once.
const SEARCH_RESULTS: usize = 8;
/// Common layout-switch shortcuts first (the rest via search).
const SWITCH_SHORTCUTS: &[(&str, &str)] = &[
    ("grp:alt_shift_toggle", tk("Alt+Shift")),
    ("grp:win_space_toggle", tk("Super+Space")),
    ("grp:ctrl_shift_toggle", tk("Ctrl+Shift")),
    ("grp:caps_toggle", tk("Caps Lock")),
    ("grp:alt_space_toggle", tk("Alt+Space")),
    ("grp:shifts_toggle", tk("Both Shift keys")),
];
/// The bar label field's width.
const LABEL_FIELD_W_PX: f32 = 90.0;

#[derive(Default)]
struct InputStudio {
    catalog: Option<Rc<XkbCatalog>>,
    /// Layout picked in the search, choosing its variant.
    picking: Option<String>,
}
impl Global for InputStudio {}

fn st(cx: &mut gpui::App) -> &mut InputStudio {
    cx.default_global::<InputStudio>()
}

fn running() -> Option<Target> {
    mandara_compositor::detect().map(|k| match k {
        mandara_compositor::CompositorKind::MangoWM => Target::Mango,
        mandara_compositor::CompositorKind::Niri => Target::Niri,
        mandara_compositor::CompositorKind::Hyprland => Target::Hyprland,
    })
}

impl StudioView {
    fn input_catalog(cx: &mut Context<Self>) -> Rc<XkbCatalog> {
        if let Some(c) = st(cx).catalog.clone() {
            return c;
        }
        let c = Rc::new(input::xkb_catalog());
        st(cx).catalog = Some(c.clone());
        c
    }

    pub(super) fn input_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let group = |g: &str| -> Vec<&'static Setting> { registry::in_section(Section::Input).filter(|s| s.group == g).collect() };
        let live = running();
        let status = match live {
            Some(t) => tf("Written to Mandara's {compositor} drop-in; “Your config” keeps your own setting.", &[("compositor", &target_name(t))]),
            None => t("Saved now; applied the next time Mandara runs under MangoWM, Niri or Hyprland.").into(),
        };
        let layouts = self.input_layouts(cfg, p, cx);
        let keyboard = self.rows(group(KEYBOARD), cfg, p, cx);
        let mouse = self.rows(group(MOUSE), cfg, p, cx);
        let touchpad = self.rows(group(TOUCHPAD), cfg, p, cx);
        let cursor = self.input_cursor_link(cfg, p, cx);
        let focus_rows = self.rows(group(FOCUS), cfg, p, cx);
        let section = |title: String, body: Vec<AnyElement>| div().row().flex_col().gap(px(SPACE_10_PX)).child(group_title(title, p)).children(body);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(status))
            .child(section(t("Keyboard layouts").into(), vec![layouts]))
            .child(keyboard)
            .child(super::grid::pair(self.page.column_w, DIM_320_PX, SPACE_14_PX, div().flex_1().child(mouse), div().flex_1().child(touchpad)))
            .child(cursor)
            .child(focus_rows)
            .into_any_element()
    }

    fn input_layouts(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let kb = cfg.compositor.input.keyboard.clone();
        let catalog = Self::input_catalog(cx);
        let describe = |name: &str, variant: &str| -> String {
            let l = catalog.layouts.iter().find(|l| l.name == name);
            let base = l.map(|l| l.description.clone()).unwrap_or_else(|| name.to_string());
            if variant.is_empty() { base } else { l.and_then(|l| l.variants.iter().find(|(v, _)| v == variant).map(|(_, d)| d.clone())).unwrap_or_else(|| format!("{base} ({variant})")) }
        };
        let mut col = card(p);
        if kb.layouts.is_empty() {
            col = col.child(heading(t("Your compositor's layouts"), t("Add a layout to manage them here; the first one is the default."), p));
        }
        let n = kb.layouts.len();
        for (i, l) in kb.layouts.iter().enumerate() {
            let label_key = l.layout.clone();
            let label = self.list_field(
                format!("kb-label-{i}"),
                kb.labels.get(&l.layout).map(String::as_str).unwrap_or_default(),
                t("Bar label"),
                Rc::new(move |c, v| {
                    let v = v.trim().to_string();
                    if v.is_empty() {
                        c.compositor.input.keyboard.labels.remove(&label_key);
                    } else {
                        c.compositor.input.keyboard.labels.insert(label_key.clone(), v);
                    }
                }),
                cx,
            );
            let mover = |id: String, glyph: &'static str, to: Option<usize>, cx: &mut Context<Self>| {
                div()
                    .id(SharedString::from(id))
                    .size(px(DIM_28_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .when(to.is_some(), |el| el.cursor_pointer().hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
                    .when(to.is_none(), |el| el.opacity(ALPHA_30))
                    .child(icon(glyph, ICON_13_PX, p.on_surface_variant))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(to) = to {
                            update_config(cx, move |c| {
                                let list = &mut c.compositor.input.keyboard.layouts;
                                match to {
                                    usize::MAX => {
                                        list.remove(i);
                                    }
                                    j => list.swap(i, j),
                                }
                            });
                            this.list_fields.retain(|k, _| !k.starts_with("kb-label-"));
                        }
                    }))
            };
            col = col.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .px(px(SPACE_6_PX))
                    .child(div().w(px(DIM_44_PX)).flex_none().child(badge(l.layout.clone(), if i == 0 { p.primary } else { p.on_surface_variant })))
                    .child(div().flex_1().min_w(px(0.0)).row().flex_col().child(div().font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(describe(&l.layout, &l.variant))).when(i == 0, |el| el.child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Default")))))
                    .child(div().w(px(LABEL_FIELD_W_PX)).flex_none().id(SharedString::from(format!("kb-label-wrap-{i}"))).tooltip(tip(t("What the bar shows for this layout, e.g. EN or ع"), p)).child(label))
                    .child(mover(format!("kb-up-{i}"), "arrow-up", (i > 0).then(|| i - 1), cx))
                    .child(mover(format!("kb-down-{i}"), "arrow-down", (i + 1 < n).then_some(i + 1), cx))
                    .child(mover(format!("kb-rm-{i}"), "close", Some(usize::MAX), cx)),
            );
        }
        // Add a layout: search, then pick a variant.
        let search = form::input("kb-search", t("Add a layout: search “arabic”, “german”, “dvorak”…"), false, cx);
        let q = form::value("kb-search", cx).to_lowercase();
        let picking = st(cx).picking.clone();
        let mut results = div().row().flex_col().gap(px(SPACE_4_PX));
        if let Some(name) = picking.clone() {
            if let Some(l) = catalog.layouts.iter().find(|l| l.name == name) {
                let variants = std::iter::once((String::new(), t("Standard").to_string())).chain(l.variants.iter().cloned());
                results = results.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tf("{layout}: choose a variant", &[("layout", &l.description)]))).child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(variants.map(|(v, d)| {
                    let (name, v2) = (name.clone(), v.clone());
                    chip(SharedString::from(format!("kb-var-{name}-{v}")), d, false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let entry = KeyboardLayout { layout: name.clone(), variant: v2.clone() };
                        update_config(cx, move |c| {
                            if !c.compositor.input.keyboard.layouts.contains(&entry) {
                                c.compositor.input.keyboard.layouts.push(entry);
                            }
                        });
                        st(cx).picking = None;
                        form::clear("kb-search", cx);
                    }))
                })));
            }
        } else if !q.is_empty() {
            for l in catalog.layouts.iter().filter(|l| l.name.to_lowercase().contains(&q) || l.description.to_lowercase().contains(&q)).take(SEARCH_RESULTS) {
                let name = l.name.clone();
                results = results.child(
                    div()
                        .id(SharedString::from(format!("kb-add-{}", l.name)))
                        .px(px(SPACE_12_PX))
                        .py(px(SPACE_8_PX))
                        .rounded(px(RADIUS_12_PX))
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .cursor_pointer()
                        .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                        .child(badge(l.name.clone(), p.on_surface_variant))
                        .child(div().flex_1().child(l.description.clone()))
                        .when(!l.variants.is_empty(), |el| el.child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(tf("{count} variants", &[("count", &l.variants.len())]))))
                        .child(icon("plus", ICON_13_PX, p.primary))
                        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            st(cx).picking = Some(name.clone());
                            cx.notify();
                        })),
                );
            }
        }
        // Switch shortcut.
        let current = kb.switch_shortcut.clone();
        let mut shortcuts: Vec<(String, String)> = SWITCH_SHORTCUTS.iter().map(|(o, l)| (o.to_string(), tr(l))).collect();
        if let Some(c) = current.as_ref().filter(|c| !shortcuts.iter().any(|(o, _)| o == *c)) {
            let d = catalog.switch_options.iter().find(|(o, _)| o == c).map(|(_, d)| d.clone()).unwrap_or_else(|| c.clone());
            shortcuts.push((c.clone(), d));
        }
        let switch_row = div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Switch layouts with")))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_6_PX))
                    .child(chip("kb-switch-none", t("Your config"), current.is_none(), p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| update_config(cx, |c| c.compositor.input.keyboard.switch_shortcut = None))))
                    .children(shortcuts.into_iter().map(|(opt, label)| {
                        let active = current.as_deref() == Some(opt.as_str());
                        let o = opt.clone();
                        chip(SharedString::from(format!("kb-switch-{opt}")), label, active, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            let o = o.clone();
                            update_config(cx, move |c| c.compositor.input.keyboard.switch_shortcut = Some(o));
                        }))
                    })),
            );
        col.child(search).child(results).when(kb.layouts.len() > 1 || current.is_some(), |el| el.child(switch_row)).into_any_element()
    }

    /// The cursor moved to Themes → Cursor: what is set now, and a link.
    fn input_cursor_link(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let c = &cfg.compositor.input.cursor;
        let now = match (&c.theme, c.size) {
            (Some(theme), Some(size)) => tf("{theme} at {size} px", &[("theme", theme), ("size", &size)]),
            (Some(theme), None) => theme.clone(),
            (None, Some(size)) => tf("Your compositor's cursor at {size} px", &[("size", &size)]),
            (None, None) => t("Your compositor's cursor").to_string(),
        };
        card(p)
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(div().size(px(DIM_34_PX)).rounded(px(RADIUS_11_PX)).row().items_center().justify_center().flex_none().bg(p.surface_container_high.hsla()).child(icon("pointer", ICON_16_PX, p.primary)))
                    .child(div().flex_1().min_w(px(0.0)).child(heading(t("Cursor theme, size and hiding are in Themes → Cursor"), now, p)))
                    .child(super::compositor_page::button("input-cursor-open", "arrow", t("Open"), true, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_cursor_section(cx)))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_input_group_has_rows() {
        for g in [KEYBOARD, MOUSE, TOUCHPAD, FOCUS] {
            assert!(registry::in_section(Section::Input).any(|s| s.group == g), "{g}");
        }
        assert!(!registry::in_section(Section::Input).any(|s| s.path.starts_with("/compositor/input/cursor/")), "the cursor lives on Themes");
        assert!(SWITCH_SHORTCUTS.iter().all(|(o, _)| o.starts_with("grp:")));
    }
}
