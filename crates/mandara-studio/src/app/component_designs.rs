//! Studio → Themes → **Components**: how the shell's own controls look.
//!
//! At the top, the **presets**: one click sets every component's design at
//! once (Classic, Material You, Expressive, Minimal, Brutalist soft), each
//! shown as a small strip of its button, switch and meter. Then one card
//! per component: its designs side by side, each drawn as the real control
//! (drag, flip or type into a preview to try it, click the design to use
//! it), then the settings that refine the component. The section lives on
//! the Themes page, right before Fine-tune and its Style group, because
//! these are looks like corner roundness and shadows (Studio keeps its
//! pages).
//!
//! A component is one [`Component`] entry: the registry choice it writes,
//! a line per design, the rows shown under the picker and how a preview is
//! drawn.


use gpui::{AnyElement, ClickEvent, Context, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::components::ComponentPreset;
use mandara_core::ToGpui;
use mandara_i18n::{t, tk, tr};
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::components::{ButtonDesign, CardStyle, ChipDesign, InputDesign, ProgressDesign, SurfaceCorners, ToggleDesign};
use mandara_ui::slider::{Look, Size, SliderDesign};
use serde_json::Value;

use super::compositor_page::{badge, button_as, card, heading};
use super::themes_page::section_head;
use super::{StudioView, choice, grid};
use crate::registry::{self, Control, Setting};
use crate::tokens::*;

/// Where the previews' value starts (the share of the track they fill).
pub(super) const PREVIEW_VALUE: f32 = 0.6;
/// The design tiles: their width range and the gap between them.
const DESIGN_GRID: CardGrid = CardGrid { min_w: 210.0, max_w: 340.0, gap: SPACE_12_PX };
/// The preset tiles.
const PRESET_GRID: CardGrid = CardGrid { min_w: 190.0, max_w: 320.0, gap: SPACE_12_PX };
/// The well a preview sits in: its height and side padding.
const PREVIEW_WELL_H_PX: f32 = DIM_64_PX;
const PREVIEW_WELL_PAD_PX: f32 = SPACE_14_PX;
/// A tile caption: its title row's height and the lines its description
/// keeps (longer ones end in an ellipsis), the same in every tile.
const CAPTION_TITLE_H_PX: f32 = DIM_24_PX;
const CAPTION_LINES: usize = 3;
/// A preset's strip: its button and switch with the meter under them.
const PRESET_STRIP_H_PX: f32 = DIM_88_PX;
/// The icon the slider previews carry inside their fill.
const PREVIEW_ICON: &str = "volume-high";
/// A meter preview's thickness, and a card preview's height and radius.
const PREVIEW_METER_PX: f32 = DIM_6_PX;
const PREVIEW_CARD_H_PX: f32 = DIM_48_PX;
const PREVIEW_CARD_RADIUS_PX: f32 = RADIUS_12_PX;
/// The check box and radio in a toggle preview.
const PREVIEW_MARK_PX: f32 = DIM_18_PX;
/// A text-field preview's sample text (a place name, the same in every
/// language) and hint.
const PREVIEW_FIELD_TEXT: &str = "El-Mandara";
const PREVIEW_FIELD_HINT: &str = tk("Place");
/// The options of a chip preview.
const PREVIEW_OPTIONS: [&str; 3] = [tk("Day"), tk("Week"), tk("Month")];

/// What the previews show while you try them (never saved).
pub(super) struct Previews {
    /// The toggle previews are on.
    pub on: bool,
    /// The chip previews' chosen option.
    pub option: usize,
}

impl Default for Previews {
    fn default() -> Self {
        Self { on: true, option: 1 }
    }
}

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
pub(super) const COMPONENTS: &[Component] = &[
    Component {
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
    },
    Component {
        title: tk("Buttons"),
        detail: tk("Buttons in panels, the sidebar, the main menu and Studio, and the round icon buttons. Filled buttons stay the main action in every design"),
        path: "/style/button_design",
        blurbs: &[
            tk("Fully rounded pills and round icon buttons"),
            tk("Rounded squares with soft corners"),
            tk("Round at rest; the corners square off while you press and spring back"),
            tk("Corners cut at an angle for a sharper, technical look"),
            tk("Raised on a soft shadow that lifts on hover and settles as you press"),
        ],
        rows: &[],
        preview: button_preview,
    },
    Component {
        title: tk("Toggles"),
        detail: tk("Switches, check boxes and radio buttons everywhere, including tray menus"),
        path: "/style/toggle_design",
        blurbs: &[
            tk("A glowing gradient switch and each place's own check boxes"),
            tk("An outlined track whose small thumb grows and shows a check when on"),
            tk("The thumb stretches as it slides, boxes round into circles as they fill"),
            tk("Soft and round: a big thumb, circular check boxes, filled radio buttons"),
        ],
        rows: &[],
        preview: toggle_preview,
    },
    Component {
        title: tk("Text fields"),
        detail: tk("Text fields in Studio and the shell, and the search boxes in panels"),
        path: "/style/input_design",
        blurbs: &[
            tk("A rounded field with an accent edge while you type"),
            tk("A tonal field with an underline that thickens in the accent colour"),
            tk("An outline that thickens while focused, with the hint floating into it"),
            tk("A fully rounded field with an accent ring while you type"),
        ],
        rows: &[],
        preview: input_preview,
    },
    Component {
        title: tk("Chips and choices"),
        detail: tk("Choices between a few options in Studio and in panels, and filter chips"),
        path: "/style/chip_design",
        blurbs: &[
            tk("Options as pills on one soft track"),
            tk("One outlined group with fine dividers; the chosen option is tinted"),
            tk("Separate buttons close together; the chosen one fills and rounds fully"),
        ],
        rows: &[],
        preview: chip_preview,
    },
    Component {
        title: tk("Progress"),
        detail: tk("Meters for disks, temperatures, batteries and processes, and Studio's progress bars"),
        path: "/style/progress_design",
        blurbs: &[
            tk("A rounded track with the fill over it"),
            tk("The fill, a small gap, then the track ending in a dot"),
            tk("The fill drawn as a gentle wave, flat when nearly empty or full"),
            tk("Ten rounded steps"),
        ],
        rows: &[],
        preview: progress_preview,
    },
    Component {
        title: tk("Card corners"),
        detail: tk("The corner family of cards in panels, the sidebar and Studio; Corner roundness still sets their size"),
        path: "/style/surface_corners",
        blurbs: &[
            tk("Every corner rounded alike"),
            tk("Large leading top and trailing bottom corners, small ones between"),
            tk("Small, crisp corners"),
            tk("Smooth continuous corners that ease into the edges"),
            tk("Corners cut at an angle"),
        ],
        rows: &[],
        preview: corners_preview,
    },
    Component {
        title: tk("Card style"),
        detail: tk("How cards stand off their panel; panels and popups keep Shadows and Panel edges"),
        path: "/style/card_style",
        blurbs: &[tk("A tonal surface with a soft rim of light"), tk("Raised on a soft shadow"), tk("Flat, with a fine outline")],
        rows: &[],
        preview: card_style_preview,
    },
];

/// The presets with their names and lines, in `ComponentPreset::ALL`'s order.
const PRESETS: [(&str, &str); 5] = [
    (tk("Classic"), tk("Mandara's own look, as it has always been")),
    (tk("Material You"), tk("Material 3: outlined toggles, filled fields, tinted choices")),
    (tk("Expressive"), tk("Shapes that morph as you press, waves and mixed corners")),
    (tk("Minimal"), tk("Square buttons, round toggles and fine outlines")),
    (tk("Brutalist soft"), tk("Cut corners, steps and outlines with a gentle touch")),
];

/// A preset's name (`tk` text).
pub(super) fn preset_name(preset: ComponentPreset) -> &'static str {
    ComponentPreset::ALL.iter().position(|p| *p == preset).map_or(PRESETS[0].0, |i| PRESETS[i].0)
}

/// Settings the presets card shows under its tiles.
const PRESET_ROWS: &[&str] = &["/looks/keep_components"];

/// The component whose design setting is `path`.
pub(super) fn by_path(path: &str) -> Option<&'static Component> {
    COMPONENTS.iter().find(|c| c.path == path)
}

/// Design `index` of `all`, or its classic first one.
fn nth<T: Copy>(all: &[T], index: usize) -> T {
    all.get(index).copied().unwrap_or(all[0])
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

/// A filled and a tonal button in design `index`.
fn button_preview(_: &mut StudioView, index: usize, p: &MandaraPalette, _: &mut Context<StudioView>) -> AnyElement {
    let design = nth(&ButtonDesign::ALL, index);
    div()
        .row()
        .items_center()
        .gap(px(SPACE_8_PX))
        .overflow_hidden()
        .child(button_as(design, "component-button-filled", "check", t("Apply"), true, p))
        .child(button_as(design, "component-button-tonal", "undo", t("Undo"), false, p))
        .into_any_element()
}

/// A switch, a check box and a radio in design `index`; clicking any
/// flips them all.
fn toggle_preview(this: &mut StudioView, index: usize, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let design = nth(&ToggleDesign::ALL, index);
    let on = this.components.on;
    let (check, radio) = if design == ToggleDesign::Classic {
        (super::apps_page::checkbox_as(design, on, p), mandara_ui::components::toggles::classic_radio(on, PREVIEW_MARK_PX, p))
    } else {
        (mandara_ui::components::toggles::checkbox(Some(on), PREVIEW_MARK_PX, design, p), mandara_ui::components::toggles::radio(on, PREVIEW_MARK_PX, design, p))
    };
    let flip = |id: &'static str, el: AnyElement, cx: &mut Context<StudioView>| {
        div()
            .id(id)
            .cursor_pointer()
            .child(el)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.components.on = !this.components.on;
                cx.notify();
            }))
    };
    div()
        .row()
        .items_center()
        .gap(px(SPACE_14_PX))
        .child(flip("component-switch", mandara_ui::plugins::panel_fx::switch_as(design, on, p).into_any_element(), cx))
        .child(flip("component-check", check, cx))
        .child(flip("component-radio", radio, cx))
        .into_any_element()
}

/// A still field in design `index` holding a place name, so the outlined
/// design shows its floating hint (a picture: no field kept per preview).
fn input_preview(_: &mut StudioView, index: usize, p: &MandaraPalette, _: &mut Context<StudioView>) -> AnyElement {
    mandara_ui::components::inputs::sample(nth(&InputDesign::ALL, index), PREVIEW_FIELD_TEXT, t(PREVIEW_FIELD_HINT), p).w_full().into_any_element()
}

/// A three-way choice in design `index`; its options can be picked.
fn chip_preview(this: &mut StudioView, index: usize, p: &MandaraPalette, cx: &mut Context<StudioView>) -> AnyElement {
    let design = nth(&ChipDesign::ALL, index);
    let items = PREVIEW_OPTIONS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let studio = cx.entity().downgrade();
            choice::ChoiceItem::new(("component-option", i), tr(label), this.components.option == i, move |_, _, cx| {
                let _ = studio.update(cx, |this, cx| {
                    this.components.option = i;
                    cx.notify();
                });
            })
        })
        .collect();
    choice::segmented_as(design, choice::Room::beside(DIM_220_PX), &choice::SETTING_CHOICE, items, p)
}

/// A meter in design `index` at the slider previews' value.
fn progress_preview(this: &mut StudioView, index: usize, p: &MandaraPalette, _: &mut Context<StudioView>) -> AnyElement {
    let design = nth(&ProgressDesign::ALL, index);
    let to = mandara_ui::plugins::panel_fx::partner(p.primary).with_lightness(p.primary.lightness());
    div().w_full().child(mandara_ui::plugins::panel_fx::meter_as(design, this.component_preview, PREVIEW_METER_PX, p.primary, to, p)).into_any_element()
}

/// A small card in corner family `index` (and the current card style).
fn corners_preview(_: &mut StudioView, index: usize, p: &MandaraPalette, _: &mut Context<StudioView>) -> AnyElement {
    let family = nth(&SurfaceCorners::ALL, index);
    preview_card(family, mandara_ui::style::card_style(), p)
}

/// A small card in card style `index` (and the current corner family).
fn card_style_preview(_: &mut StudioView, index: usize, p: &MandaraPalette, _: &mut Context<StudioView>) -> AnyElement {
    let style = nth(&CardStyle::ALL, index);
    preview_card(mandara_ui::style::surface_corners(), style, p)
}

fn preview_card(family: SurfaceCorners, style: CardStyle, p: &MandaraPalette) -> AnyElement {
    use mandara_ui::components::surfaces::{CardSurface as _, Fill};
    let rim = mandara_ui::plugins::panel_fx::rim(p);
    div()
        .w_full()
        .h(px(PREVIEW_CARD_H_PX))
        .card_surface_as(family, style, PREVIEW_CARD_RADIUS_PX, Fill(p.surface_container_high, None), rim, p)
        .px(px(SPACE_12_PX))
        .row()
        .items_center()
        .gap(px(SPACE_8_PX))
        .child(mandara_ui::icons::icon("calendar", ICON_16_PX, p.primary))
        .child(div().h(px(DIM_8_PX)).flex_1().rounded_full().bg(p.on_surface_variant.with_alpha(ALPHA_25).hsla()))
        .into_any_element()
}

/// A design or preset tile's caption, the same height in every tile so
/// rows line up whatever the language: the name on one line (ellipsized),
/// then its line clamped to [`CAPTION_LINES`] lines.
pub(super) fn caption(name: String, line: String, on: bool, p: &MandaraPalette) -> [AnyElement; 2] {
    let title = div()
        .h(px(CAPTION_TITLE_H_PX))
        .px(px(SPACE_4_PX))
        .row()
        .items_center()
        .gap(px(SPACE_8_PX))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).child(name))
        .when(on, |el| el.child(badge(t("In use"), p.primary)));
    let body = div()
        .h(px(CAPTION_LINES as f32 * LEADING_17_PX))
        .mb(px(SPACE_4_PX))
        .px(px(SPACE_4_PX))
        .overflow_hidden()
        .text_size(px(TEXT_12_PX))
        .line_height(px(LEADING_17_PX))
        .text_color(p.on_surface_variant.hsla())
        .line_clamp(CAPTION_LINES)
        .child(mandara_ui::bidi::paragraph(line));
    [title.into_any_element(), body.into_any_element()]
}

impl StudioView {
    /// The Components section of the Themes page.
    pub(super) fn components_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let head = section_head(t("Components"), t("How the shell's controls look. Pick a preset to set them all at once, or a design per component: each preview is the real control, so try it, then click a design to use it"), Vec::new(), display, p);
        let mut section = div().row().flex_col().gap(px(SPACE_14_PX)).child(head).child(self.presets_card(cfg, p, cx));
        for component in COMPONENTS {
            section = section.child(self.component_card(component, cfg, p, cx));
        }
        section.into_any_element()
    }

    /// The presets: one tile each, its name, line and a strip of its
    /// button, switch and meter; a click sets every design.
    fn presets_card(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let inner = (self.page.column_w - 2.0 * SPACE_18_PX).max(0.0);
        let current = ComponentPreset::current(&cfg.style);
        let to = mandara_ui::plugins::panel_fx::partner(p.primary).with_lightness(p.primary.lightness());
        let tiles = ComponentPreset::ALL.into_iter().zip(PRESETS).enumerate().map(|(i, (preset, (name, line)))| {
            let d = preset.designs();
            let on = current == Some(preset);
            // The preset's button and switch, its meter under them.
            let strip = div()
                .h(px(PRESET_STRIP_H_PX))
                .px(px(SPACE_12_PX))
                .rounded(px(RADIUS_14_PX))
                .bg(p.surface_container.hsla())
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(SPACE_10_PX))
                .child(div().row().items_center().gap(px(SPACE_10_PX)).child(button_as(d.button, "preset-button", "check", t("Apply"), true, p)).child(mandara_ui::plugins::panel_fx::switch_as(d.toggle, true, p)))
                .child(div().w_full().child(mandara_ui::plugins::panel_fx::meter_as(d.progress, PREVIEW_VALUE, PREVIEW_METER_PX, p.primary, to, p)));
            div()
                .id(("component-preset", i))
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
                // The strip is a picture of the preset: a press there picks it too.
                .child(strip)
                .children(caption(tr(name), tr(line), on, p))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| mandara_ui::state::update_config(cx, move |c| preset.apply(&mut c.style))))
                .into_any_element()
        });
        let grid = grid::fit(inner, PRESET_GRID).render(tiles.collect::<Vec<_>>());
        let detail = if current.is_some() { t("Sets every component below at once; corner size, shadows, panel edges and the slider handle stay yours") } else { t("Your own mix of designs. A preset sets every component below at once; corner size, shadows, panel edges and the slider handle stay yours") };
        let rows: Vec<&Setting> = PRESET_ROWS.iter().filter_map(|path| registry::by_path(path)).collect();
        let rows = self.group_card(&rows, inner, cfg, p, cx);
        card(p).child(div().px(px(SPACE_4_PX)).child(heading(t("Presets"), detail, p))).child(grid).child(rows).into_any_element()
    }

    /// A component's card: title, its designs, then its other settings.
    fn component_card(&mut self, component: &'static Component, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let inner = (self.page.column_w - 2.0 * SPACE_18_PX).max(0.0);
        let picker = registry::by_path(component.path).map(|setting| self.design_picker(component, setting, inner, cfg, p, cx));
        let rows: Vec<&Setting> = component.rows.iter().filter_map(|path| registry::by_path(path)).collect();
        let rows = (!rows.is_empty()).then(|| self.group_card(&rows, inner, cfg, p, cx));
        let note = registry::by_path(component.path).and_then(|s| registry::note(s, cfg));
        let detail = note.unwrap_or(component.detail);
        card(p).child(div().px(px(SPACE_4_PX)).child(heading(tr(component.title), tr(detail), p))).children(picker).children(rows).into_any_element()
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
                    .children(caption(tr(label), tr(blurb), on, p))
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
    use serde::de::DeserializeOwned;

    /// The choices of `path` are exactly `all`, in order (previews draw
    /// design `index`).
    fn lines_up<T: DeserializeOwned + PartialEq + std::fmt::Debug + Copy>(path: &str, all: &[T]) {
        let Control::Choice(choices) = registry::by_path(path).unwrap().control else { panic!("{path}: a choice") };
        assert_eq!(choices.len(), all.len(), "{path}");
        for ((raw, _), design) in choices.iter().zip(all) {
            assert_eq!(serde_json::from_value::<T>(registry::choice_value(raw)).unwrap(), *design, "{path}: {raw}");
        }
    }

    /// Each component's choice, blurbs and rows line up, and every choice
    /// is exactly the config's designs in order.
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
        lines_up("/style/slider_design", &SliderDesign::ALL);
        lines_up("/style/button_design", &ButtonDesign::ALL);
        lines_up("/style/toggle_design", &ToggleDesign::ALL);
        lines_up("/style/input_design", &InputDesign::ALL);
        lines_up("/style/chip_design", &ChipDesign::ALL);
        lines_up("/style/progress_design", &ProgressDesign::ALL);
        lines_up("/style/surface_corners", &SurfaceCorners::ALL);
        lines_up("/style/card_style", &CardStyle::ALL);
        // Every Components setting has its card (or sits under one).
        for s in registry::in_section(registry::Section::Themes).filter(|s| s.group == crate::registry::desktop_settings::COMPONENTS) {
            assert!(by_path(s.path).is_some() || COMPONENTS.iter().any(|c| c.rows.contains(&s.path)) || PRESET_ROWS.contains(&s.path), "{}: shown in the section", s.path);
        }
        for preset in ComponentPreset::ALL {
            assert!(!preset_name(preset).is_empty());
        }
        assert_eq!(preset_name(ComponentPreset::BrutalistSoft), PRESETS[4].0);
        assert_eq!(PRESETS.len(), ComponentPreset::ALL.len());
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

    #[test]
    fn raised_cards_say_when_shadows_are_off() {
        let style = registry::by_path("/style/card_style").unwrap();
        let mut cfg = ShellConfig::default();
        cfg.style.card_style = CardStyle::Raised;
        assert!(registry::note(style, &cfg).is_none());
        cfg.style.shadows = false;
        assert!(registry::note(style, &cfg).is_some());
        cfg.style.card_style = CardStyle::Outlined;
        assert!(registry::note(style, &cfg).is_none());
    }
}
