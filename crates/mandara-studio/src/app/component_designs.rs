//! Studio → Themes → **Components**: how the shell's own controls look.
//!
//! One card per component: its designs side by side, each drawn as the
//! real control (drag a preview to try it, click the design to use it),
//! then the settings that refine the component. The section lives on the
//! Themes page, right before Fine-tune and its Style group, because these
//! are looks like corner roundness and shadows (Studio keeps its pages).
//!
//! A component is one [`Component`] entry: the registry choice it writes,
//! a line per design, the rows shown under the picker and how a preview is
//! drawn. Switches, chips or progress bars join as further entries in
//! [`COMPONENTS`]; the sliders are the first.

use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::slider::{Look, Size, SliderDesign};
use serde_json::Value;

use super::compositor_page::{badge, card, heading};
use super::themes_page::section_head;
use super::{StudioView, grid};
use crate::registry::{self, Control, Setting};
use crate::tokens::*;

/// Where the previews' value starts (the share of the track they fill).
pub(super) const PREVIEW_VALUE: f32 = 0.6;
/// The design tiles: their width range and the gap between them.
const DESIGN_GRID: CardGrid = CardGrid { min_w: 210.0, max_w: 340.0, gap: SPACE_12_PX };
/// The well a preview sits in: its height and side padding.
const PREVIEW_WELL_H_PX: f32 = DIM_64_PX;
const PREVIEW_WELL_PAD_PX: f32 = SPACE_14_PX;
/// The icon the slider previews carry inside their fill.
const PREVIEW_ICON: &str = "volume-high";

/// A component whose look is picked from designs.
pub(super) struct Component {
    /// The card's title and line.
    pub title: &'static str,
    pub detail: &'static str,
    /// The design setting: a registry choice.
    pub path: &'static str,
    /// One line per design, in the choice's order.
    pub blurbs: &'static [&'static str],
    /// More of the component's settings, as rows under the designs.
    pub rows: &'static [&'static str],
    /// Draws a live preview of the choice's design `index`.
    pub preview: fn(&mut StudioView, usize, &MandaraPalette, &mut Context<StudioView>) -> AnyElement,
}

/// Every component with designs, in the order the section shows them.
pub(super) const COMPONENTS: &[Component] = &[Component {
    title: tk("Sliders"),
    detail: tk("The volume, brightness and mixer sliders in the main menu, panels and the sidebar"),
    path: "/style/slider_design",
    blurbs: &[
        tk("The fill, a small gap, an upright handle, then the rest of the track ending in a dot"),
        tk("One continuous track with a short grip inside the end of the fill"),
        tk("A round knob on the end of the fill that stretches into a pill while you hold it"),
        tk("A gentle wave along the fill that flows while you drag"),
        tk("Ten rounded steps that fill one after another"),
    ],
    rows: &["/style/slider_handle"],
    preview: slider_preview,
}];

/// The component whose design setting is `path`.
pub(super) fn by_path(path: &str) -> Option<&'static Component> {
    COMPONENTS.iter().find(|c| c.path == path)
}

/// A real thick slider in design `index`, sharing one preview value.
fn slider_preview(this: &mut StudioView, index: usize, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let look = Look { icon: Some(PREVIEW_ICON), design: SliderDesign::ALL.get(index).copied(), ..Default::default() };
    let studio = cx.entity().downgrade();
    mandara_ui::slider::slider(ElementId::from(("component-slider", index)), this.component_preview, Size::Large, look, p, move |v, _, cx| {
        let _ = studio.update(cx, |this, cx| {
            this.component_preview = v;
            cx.notify();
        });
    })
    .into_any_element()
}

impl StudioView {
    /// The Components section of the Themes page.
    pub(super) fn components_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let head = section_head(t("Components"), t("How the shell's controls look. Each preview is the real control: drag it to try a design, click a design to use it"), Vec::new(), display, p);
        let mut section = div().row().flex_col().gap(px(SPACE_14_PX)).child(head);
        for component in COMPONENTS {
            section = section.child(self.component_card(component, cfg, p, cx));
        }
        section.into_any_element()
    }

    /// A component's card: title, its designs, then its other settings.
    fn component_card(&mut self, component: &'static Component, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let inner = (self.page.column_w - 2.0 * SPACE_18_PX).max(0.0);
        let picker = registry::by_path(component.path).map(|setting| self.design_picker(component, setting, inner, cfg, p, cx));
        let rows: Vec<&Setting> = component.rows.iter().filter_map(|path| registry::by_path(path)).collect();
        let rows = (!rows.is_empty()).then(|| self.group_card(&rows, inner, cfg, p, cx));
        card(p).child(div().px(px(SPACE_4_PX)).child(heading(tr(component.title), tr(component.detail), p))).children(picker).children(rows).into_any_element()
    }

    /// The designs of `component` (its choice `setting`) as tiles at most
    /// `width` wide, each with a live preview; a click on a tile picks it.
    pub(super) fn design_picker(&mut self, component: &'static Component, setting: &Setting, width: f32, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let Control::Choice(choices) = setting.control else { return div().into_any_element() };
        let current = super::choice_current(&registry::get(cfg, setting));
        let mut tiles = Vec::with_capacity(choices.len());
        for (i, (raw, label)) in choices.iter().enumerate() {
            let on = *raw == current;
            let preview = (component.preview)(self, i, p, cx);
            let blurb = component.blurbs.get(i).copied().unwrap_or_default();
            let pick = setting.clone();
            let value = registry::choice_value(raw);
            tiles.push(
                div()
                    .id((component.path, i))
                    .rounded(px(RADIUS_18_PX))
                    .p(px(SPACE_10_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .cursor_pointer()
                    .bg(if on { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container_high.hsla() })
                    .border_2()
                    .border_color(if on { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                    .child(
                        div()
                            .h(px(PREVIEW_WELL_H_PX))
                            .px(px(PREVIEW_WELL_PAD_PX))
                            .rounded(px(RADIUS_14_PX))
                            .bg(p.surface_container.hsla())
                            .row()
                            .items_center()
                            .child(div().flex_1().min_w(px(0.0)).child(preview))
                            // Trying a preview doesn't pick its design.
                            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                    )
                    .child(
                        div()
                            .px(px(SPACE_4_PX))
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).child(tr(label)))
                            .when(on, |el| el.child(badge(t("In use"), p.primary))),
                    )
                    .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(blurb))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&pick, Value::clone(&value), cx)))
                    .into_any_element(),
            );
        }
        grid::fit(width, DESIGN_GRID).render(tiles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each component's choice, blurbs and rows line up, and the slider's
    /// choices are exactly the config's designs in order (previews draw
    /// design `index`).
    #[test]
    fn every_component_lines_up_with_its_setting() {
        for c in COMPONENTS {
            let setting = registry::by_path(c.path).unwrap_or_else(|| panic!("{}: no setting", c.path));
            let Control::Choice(choices) = setting.control else { panic!("{}: a choice", c.path) };
            assert_eq!(choices.len(), c.blurbs.len(), "{}: one line per design", c.path);
            assert_eq!(setting.group, crate::registry::desktop_settings::COMPONENTS, "{}: in the Components group", c.path);
            for row in c.rows {
                let row = registry::by_path(row).unwrap_or_else(|| panic!("{row}: no setting"));
                assert_eq!(row.group, crate::registry::desktop_settings::COMPONENTS, "{}: drawn in the Components section, not twice", row.path);
            }
        }
        let Control::Choice(choices) = registry::by_path("/style/slider_design").unwrap().control else { unreachable!() };
        for ((raw, _), design) in choices.iter().zip(SliderDesign::ALL) {
            assert_eq!(serde_json::from_value::<SliderDesign>(registry::choice_value(raw)).unwrap(), design, "{raw}");
        }
        assert_eq!(choices.len(), SliderDesign::ALL.len());
    }

    #[test]
    fn the_handle_row_says_when_only_thin_sliders_use_it() {
        let handle = registry::by_path("/style/slider_handle").unwrap();
        let mut cfg = ShellConfig::default();
        for design in SliderDesign::ALL {
            cfg.style.slider_design = design;
            assert_eq!(registry::note(handle, &cfg).is_some(), !design.uses_handle(), "{design:?}");
            assert!(registry::locked(handle, &cfg).is_none(), "{design:?}: it still shapes the thin sliders, so it stays editable");
        }
    }
}
