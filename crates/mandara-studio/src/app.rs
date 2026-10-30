//! The Studio window: sidebar navigation + searchable, registry-driven
//! settings pages, a theme gallery and plugin management. Every change goes
//! straight into the shared config entity, so the live shell updates at once.

use mandara_ui::bidi::Bidi as _;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{PluginOrigin, PluginRegistry};
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::{config, update_config};
use serde_json::Value;

mod maint_page;
mod nav;
mod market_page;
mod store_page;
pub(crate) use store_page::{HIDDEN_PLUGINS, PLUGIN_CATEGORIES, plugin_art_cache_clear, plugin_icon, plugin_tile};
mod plugin_updates;
mod updates_card;
mod designer;
mod sidebar_tabs_page;
mod greeter_page;
mod keyrings_card;
mod users_page;
pub use users_page::set_demo as set_users_demo;
mod compositor_page;
mod keybind_conflicts;
mod audio_page;
mod notifications_page;
pub(crate) mod power_page;
mod displays_page;
mod diagnostics_page;
mod frame_meter;
mod form;
mod network_page;
mod compositor_presets;
mod input_page;
pub mod apps_page;
mod profiles_page;
mod launcher_page;
mod dock_page;
mod switcher_page;
mod desktop_widgets_page;
mod looks_page;
mod app_themes_page;
mod themes_page;
mod cursor_page;
mod plugin_offer;
mod swatch_list;
mod wallgen_logic;
mod wallgen_page;
pub(crate) mod grid;
pub mod plugin_modal;
pub mod plugins_local;
#[cfg(test)]
mod settings_audit;

use crate::perf::{self, PerfMonitor};
use crate::registry::{self, Control, Section, Setting};
use crate::text_field::TextField;
use crate::tokens::*;

/// Drags the window: the compositor's own grab where the standard
/// `xdg_toplevel.move` is ignored (MangoWM), the standard request otherwise.
fn begin_move(window: &mut Window) {
    if !mandara_compositor::begin_interactive_move() {
        window.start_window_move();
    }
}

pub struct StudioView {
    section: Section,
    search: Entity<TextField>,
    query: String,
    fields: HashMap<&'static str, Entity<TextField>>,
    error: Option<String>,
    perf: Entity<PerfMonitor>,
    icon_search: Entity<TextField>,
    icon_query: String,
    icon_path: Entity<TextField>,
    /// Seeds extracted from the current wallpaper (path, hex colors).
    wallpaper_seeds: Option<(std::path::PathBuf, Rc<[mandara_theme::Color]>)>,
    /// The current wallpaper's picker thumbnail (wallpaper, thumbnail file),
    /// made off the UI thread together with the seeds.
    wallpaper_thumb: Option<(std::path::PathBuf, std::path::PathBuf)>,
    seeds_loading: Option<std::path::PathBuf>,
    /// Themes page: every theme with its palette for the current mode, read
    /// once per visit (user themes are TOML files on disk).
    themes: Option<themes_page::ThemeGallery>,
    /// Icon picker: the query and the theme icons matching it.
    icon_matches: Option<(String, Rc<[String]>)>,
    /// Studio → Sidebars & hover → Tabs editor state.
    sidebar_tabs: sidebar_tabs_page::TabsEditor,
    /// Responsive breakpoints, recomputed from the window width each frame.
    rail: bool,
    stack_rows: bool,
    /// Page geometry for the current window width (grids size from it).
    page: grid::PageMetrics,
    /// Text fields of the slash-command / skill editors, keyed by row+field.
    list_fields: HashMap<String, Entity<TextField>>,
    /// Keybind row waiting for a chord (Studio → Keyboard shortcuts).
    capturing: Option<usize>,
    focus: gpui::FocusHandle,
    maint: maint_page::Maint,
    /// About → Mandara updates, and marketplace plugin updates.
    updates: updates_card::Updates,
    /// Plugins page: the plugin detail / settings modal.
    modal: plugin_modal::PluginModal,
    /// Plugins page tab (Installed / Marketplace / Local).
    plugins_tab: plugins_local::PluginsTab,
    /// The plugin store (search, filter chips, featured hero).
    store: store_page::Store,
    market: market_page::Market,
    local: plugins_local::Local,
    /// The page scroller (deep links scroll to a section).
    content_scroll: gpui::ScrollHandle,
    /// Sidebar: keyboard focus, scroller, and the remembered collapsed
    /// groups (`crate::ui_state`).
    nav_focus: gpui::FocusHandle,
    nav_scroll: gpui::ScrollHandle,
    ui: crate::ui_state::UiState,
}

impl StudioView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.observe(&config(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        profiles_page::track_history(cx);
        let perf = perf::new(cx);
        cx.observe(&perf, |this: &mut Self, _, cx| {
            if matches!(this.section, Section::Performance | Section::Home) && this.query.is_empty() {
                cx.notify();
            }
        })
        .detach();
        let this = cx.entity().downgrade();
        let search = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search settings"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = this.update(cx, |studio, cx| {
                        studio.query = value;
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        let this = cx.entity().downgrade();
        let icon_search = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search every icon in your theme…"),
                true,
                Rc::new(move |value, _, cx| {
                    let _ = this.update(cx, |studio, cx| {
                        studio.icon_query = value;
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        let icon_path = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("…or an image path, e.g. ~/Pictures/logo.svg"),
                false,
                Rc::new(|value, _, cx| {
                    let value = value.trim().to_string();
                    if !value.is_empty() {
                        update_config(cx, |c| c.appearance.launcher_icon = value);
                    }
                }),
                cx,
            )
            .clear_on_commit()
        });
        Self { section: Section::Home, search, query: String::new(), fields: HashMap::new(), error: None, perf, icon_search, icon_query: String::new(), icon_path, wallpaper_seeds: None, wallpaper_thumb: None, seeds_loading: None, themes: None, icon_matches: None, sidebar_tabs: Default::default(), rail: false, stack_rows: false, page: grid::PageMetrics::for_width(crate::DEFAULT_W_PX), list_fields: HashMap::new(), capturing: None, focus: cx.focus_handle(), maint: Default::default(), updates: Default::default(), modal: Default::default(), plugins_tab: Default::default(), store: Default::default(), market: Default::default(), local: Default::default(), content_scroll: Default::default(), nav_focus: cx.focus_handle(), nav_scroll: Default::default(), ui: crate::ui_state::load() }
    }

    /// Navigates to a section. Entering a page drops what it cached from
    /// disk, so a visit always shows fresh files, and opens it at the top
    /// (the content scroller is shared: a page must not inherit the last
    /// one's offset, which left short pages "stuck" at their bottom).
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        if section != self.section {
            self.content_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            self.themes = None;
            self.icon_matches = None;
            plugin_art_cache_clear();
            if self.section == Section::LoginScreen {
                greeter_page::release(cx);
            }
            if self.section == Section::Themes {
                cursor_page::release(cx);
            }
            if self.section == Section::Wallpaper {
                wallgen_page::release(cx);
            }
        }
        self.section = section;
        self.query.clear();
        self.search.update(cx, |f, _| f.value.clear());
        cx.notify();
    }

    fn apply(&mut self, setting: &Setting, value: Value, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        match registry::set(&cfg, setting, value) {
            Ok(next) => {
                self.error = None;
                update_config(cx, |c| *c = next);
            }
            Err(err) => self.error = Some(err),
        }
        cx.notify();
    }

    /// Restores every setting of a group to its default in one edit.
    fn reset_group(&mut self, settings: &[Setting], cx: &mut Context<Self>) {
        let mut next = (*config(cx).read(cx).snapshot()).clone();
        for s in settings {
            if let Ok(c) = registry::set(&next, s, registry::default_value(s)) {
                next = c;
            }
        }
        self.error = None;
        update_config(cx, |c| *c = next);
        cx.notify();
    }

    fn field(&mut self, setting: &Setting, current: String, placeholder: &'static str, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = self.fields.get(setting.path) {
            f.update(cx, |f, _| f.sync(&current));
            return f.clone();
        }
        let this = cx.entity().downgrade();
        let setting2 = setting.clone();
        let field = cx.new(|cx| {
            TextField::new(
                current,
                placeholder,
                false,
                Rc::new(move |value, _, cx| {
                    let setting = setting2.clone();
                    let _ = this.update(cx, |studio, cx| {
                        let json = match setting.control {
                            Control::OptionalText { .. } if value.trim().is_empty() => Value::Null,
                            Control::List { .. } => Value::Array(value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(Value::from).collect()),
                            Control::Number { min, max } => match value.trim().parse::<f64>() {
                                Ok(n) => Value::from(n.clamp(min, max)),
                                Err(_) => {
                                    studio.error = Some(tf("{setting} must be a number", &[("setting", &tr(setting.label))]));
                                    return;
                                }
                            },
                            _ => Value::from(value),
                        };
                        studio.apply(&setting, json, cx);
                    });
                }),
                cx,
            )
        });
        self.fields.insert(setting.path, field.clone());
        field
    }

    fn control(&mut self, setting: &Setting, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let value = registry::get(cfg, setting);
        match setting.control.clone() {
            Control::Time => {
                let raw = value.as_str().unwrap_or("00:00").to_string();
                let (h, m) = raw.split_once(':').and_then(|(h, m)| Some((h.parse::<i32>().ok()?, m.parse::<i32>().ok()?))).unwrap_or((0, 0));
                let step = |id: &'static str, label: &'static str, dh: i32, dm: i32, cx: &mut Context<Self>| {
                    let setting = setting.clone();
                    div()
                        .id(SharedString::from(format!("{id}{}", setting.path)))
                        .size(px(DIM_26_PX))
                        .rounded_full()
                        .row()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(p.on_surface_variant.hsla())
                        .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                        .child(label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            let total = ((h * MINUTES_PER_HOUR + m + dh * MINUTES_PER_HOUR + dm) % MINUTES_PER_DAY + MINUTES_PER_DAY) % MINUTES_PER_DAY;
                            this.apply(&setting, Value::from(format!("{:02}:{:02}", total / MINUTES_PER_HOUR, total % MINUTES_PER_HOUR)), cx);
                        }))
                };
                let unit = |text: String| {
                    div()
                        .min_w(px(DIM_40_PX))
                        .h(px(DIM_32_PX))
                        .rounded(px(RADIUS_10_PX))
                        .bg(p.surface_container_high.hsla())
                        .row()
                        .items_center()
                        .justify_center()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .font_features(mandara_ui::fonts::tabular_nums())
                        .child(text)
                };
                // Clock times read HH:MM left to right in every language.
                div()
                    .ltr_row()
                    .items_center()
                    .gap(px(SPACE_4_PX))
                    .child(step("h-", "‹", -1, 0, cx))
                    .child(unit(format!("{h:02}")))
                    .child(step("h+", "›", 1, 0, cx))
                    .child(div().px(px(2.0)).font_weight(gpui::FontWeight::SEMIBOLD).child(":"))
                    .child(step("m-", "‹", 0, -TIME_STEP_MINUTES, cx))
                    .child(unit(format!("{m:02}")))
                    .child(step("m+", "›", 0, TIME_STEP_MINUTES, cx))
                    .into_any_element()
            }
            Control::Toggle if let Some(reason) = registry::locked(setting, cfg) => div()
                .id(SharedString::from(format!("t{}", setting.path)))
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(reason))))
                .child(div().opacity(LOCKED_OPACITY).child(mandara_ui::plugins::network::switch(false, p)))
                .into_any_element(),
            Control::Toggle => {
                let on = value.as_bool().unwrap_or(false);
                let setting = setting.clone();
                div()
                    .id(SharedString::from(format!("t{}", setting.path)))
                    .cursor_pointer()
                    .child(mandara_ui::plugins::network::switch(on, p))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting, Value::from(!on), cx)))
                    .into_any_element()
            }
            Control::Slider { min, max, step, unit } => {
                let v = value.as_f64().unwrap_or(min);
                let frac = ((v - min) / (max - min)).clamp(0.0, 1.0) as f32;
                let setting2 = setting.clone();
                let this = cx.entity().downgrade();
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(div().w(px(DIM_220_PX)).child(mandara_ui::kit::slider(
                        SharedString::from(format!("s{}", setting.path)),
                        frac,
                        p,
                        move |f, _, cx| {
                            let raw = min + (max - min) * f as f64;
                            let snapped = (raw / step).round() * step;
                            let setting = setting2.clone();
                            let _ = this.update(cx, |studio, cx| studio.apply(&setting, Value::from(snapped), cx));
                        },
                    )))
                    .child(
                        div()
                            .w(px(DIM_76_PX))
                            .text_end()
                            .text_size(px(TEXT_12_PX))
                            .text_color(p.on_surface_variant.hsla())
                            .child(if step >= 1.0 { format!("{v:.0}{unit}") } else if step >= TWO_DECIMAL_STEP { format!("{v:.2}{unit}") } else { format!("{v:.3}{unit}") }),
                    )
                    .into_any_element()
            }
            Control::Choice(choices) => {
                let current = match &value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(2.0))
                    .p(px(SPACE_3_PX))
                    .rounded(px(RADIUS_12_PX))
                    .bg(p.surface_container_high.hsla())
                    .children(choices.iter().map(|(raw, label)| {
                        let active = *raw == current;
                        let setting = setting.clone();
                        div()
                            .id(SharedString::from(format!("c{}{raw}", setting.path)))
                            .px(px(SPACE_12_PX))
                            .h(px(DIM_28_PX))
                            .rounded(px(RADIUS_9_PX))
                            .row()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(TEXT_12_5_PX))
                            .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).font_weight(FontWeight::MEDIUM))
                            .when(!active, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla())))
                            .child(mandara_ui::bidi::paragraph(tr(label)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting, registry::choice_value(raw), cx)))
                    }))
                    .into_any_element()
            }
            Control::Text { placeholder } | Control::OptionalText { placeholder } => {
                let current = value.as_str().unwrap_or_default().to_string();
                self.field(setting, current, placeholder, cx).into_any_element()
            }
            Control::Number { .. } => {
                let current = value.as_f64().map(|n| format!("{n}")).unwrap_or_default();
                self.field(setting, current, "", cx).into_any_element()
            }
            Control::List { placeholder } => {
                let current = value.as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
                self.field(setting, current, placeholder, cx).into_any_element()
            }
            Control::OptionalSlider { min, max, step, unit, default } => {
                let setting2 = setting.clone();
                match value.as_f64() {
                    // Unset: say so, and offer to take over.
                    None => div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Your config")))
                        .child(
                            div()
                                .id(SharedString::from(format!("ov{}", setting.path)))
                                .px(px(SPACE_12_PX))
                                .h(px(DIM_28_PX))
                                .rounded_full()
                                .row()
                                .items_center()
                                .cursor_pointer()
                                .bg(p.primary.with_alpha(ALPHA_14).hsla())
                                .text_color(p.primary.hsla())
                                .text_size(px(TEXT_12_5_PX))
                                .child(t("Override"))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting2, Value::from(default), cx))),
                        )
                        .into_any_element(),
                    Some(v) => {
                        let frac = ((v - min) / (max - min)).clamp(0.0, 1.0) as f32;
                        let this = cx.entity().downgrade();
                        let slide = setting.clone();
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_12_PX))
                            .child(div().w(px(DIM_200_PX)).child(mandara_ui::kit::slider(SharedString::from(format!("s{}", setting.path)), frac, p, move |f, _, cx| {
                                let raw = min + (max - min) * f as f64;
                                let snapped = (raw / step).round() * step;
                                let setting = slide.clone();
                                let _ = this.update(cx, |studio, cx| studio.apply(&setting, Value::from(snapped), cx));
                            })))
                            .child(div().w(px(DIM_56_PX)).text_end().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(if step >= 1.0 { format!("{v:.0}{unit}") } else { format!("{v:.2}{unit}") }))
                            .child(
                                div()
                                    .id(SharedString::from(format!("rs{}", setting.path)))
                                    .px(px(SPACE_10_PX))
                                    .h(px(DIM_26_PX))
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .cursor_pointer()
                                    .text_size(px(TEXT_12_PX))
                                    .text_color(p.on_surface_variant.hsla())
                                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla()))
                                    .child(t("Reset"))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting2, Value::Null, cx))),
                            )
                            .into_any_element()
                    }
                }
            }
            Control::Command { button, args } => div()
                .id(SharedString::from(format!("cmd{}", setting.path)))
                .h(px(DIM_32_PX))
                .px(px(SPACE_16_PX))
                .rounded_full()
                .row()
                .items_center()
                .cursor_pointer()
                .bg(p.primary.with_alpha(ALPHA_14).hsla())
                .text_color(p.primary.hsla())
                .text_size(px(TEXT_12_5_PX))
                .font_weight(gpui::FontWeight::MEDIUM)
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_22).hsla()))
                .child(mandara_ui::bidi::paragraph(tr(button)))
                .on_click(move |_: &ClickEvent, _, cx| {
                    if let Some(ipc) = cx.try_global::<perf::ShellIpc>().cloned() {
                        let words: Vec<String> = args.iter().map(|s| s.to_string()).collect();
                        std::thread::spawn(move || {
                            if let Err(err) = (ipc.0)(words.clone()) {
                                tracing::warn!("`{}` failed: {err}", words.join(" "));
                            }
                        });
                    }
                })
                .into_any_element(),
            Control::Icon => {
                let current = value.as_str().unwrap_or("grid").to_string();
                div()
                    .size(px(DIM_52_PX))
                    .rounded(px(RADIUS_16_PX))
                    .bg(p.primary.with_alpha(ALPHA_12).hsla())
                    .row()
                    .items_center()
                    .justify_center()
                    .child(mandara_ui::icons::any_icon(&current, ICON_26_PX, p.primary))
                    .into_any_element()
            }
        }
    }

    fn rows<'a>(&mut self, settings: impl IntoIterator<Item = &'a Setting>, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        // Rows of an opt-in plugin that is off stay out of the page (the
        // plugin's store card offers it instead).
        let registry = cx.try_global::<mandara_plugins::PluginRegistry>();
        let shown = |s: &&Setting| registry::required_plugin(s.path).is_none_or(|id| registry.is_some_and(|r| r.is_enabled(id, &cfg.plugins)));
        let settings: Vec<&Setting> = settings.into_iter().filter(shown).collect();
        let groups = group_rows(settings);
        let mut col = div().row().flex_col().gap(px(SPACE_26_PX));
        for (title, list) in groups {
            let mut card = div().row().flex_col().gap(px(2.0)).p(px(SPACE_6_PX)).rounded(px(RADIUS_20_PX)).bg(p.surface_container.hsla());
            // "Reset" restores the group's defaults; shown only once a
            // setting in it differs from them.
            let resettable: Vec<Setting> = list.iter().filter(|s| !matches!(s.control, Control::Command { .. })).map(|s| (*s).clone()).collect();
            let changed = resettable.iter().any(|s| registry::get(cfg, s) != registry::default_value(s));
            for s in list.iter() {
                // A control that does nothing in the current setup shows why
                // instead (toggles render their own locked state).
                let control = match registry::locked(s, cfg) {
                    Some(reason) if s.control != Control::Toggle => div()
                        .max_w(px(LOCKED_NOTE_MAX_W_PX))
                        .text_size(px(TEXT_12_PX))
                        .text_color(p.on_surface_variant.with_alpha(LOCKED_OPACITY).hsla())
                        .child(mandara_ui::bidi::paragraph(tr(reason)))
                        .into_any_element(),
                    _ => self.control(s, cfg, p, cx),
                };
                let picker = (s.control == Control::Icon).then(|| self.icon_picker(s, cfg, p, cx));
                let stack = self.stack_rows;
                card = card.child(
                    div()
                        .min_h(px(DIM_68_PX))
                        .px(px(if stack { SPACE_14_PX } else { SPACE_18_PX }))
                        .py(px(SPACE_14_PX))
                        .rounded(px(RADIUS_14_PX))
                        .row()
                        // Narrow: the control sits under its label.
                        .when(stack, |el| el.flex_col().cross_start().gap(px(SPACE_10_PX)))
                        .when(!stack, |el| el.items_center().gap(px(SPACE_24_PX)))
                        .hover(|el| el.bg(p.on_surface.with_alpha(ALPHA_3_5).hsla()))
                        .child(
                            div()
                                .flex_1()
                                // A wide control (a long choice list such as a
                                // hot corner's actions) wraps instead of
                                // squeezing its label to one letter per line.
                                .min_w(px(ROW_LABEL_MIN_PX))
                                .row()
                                .flex_col()
                                .gap(px(SPACE_3_PX))
                                .child(div().text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(s.label))))
                                .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(s.help)))),
                        )
                        .child(control),
                );
                if let Some(picker) = picker {
                    card = card.child(div().px(px(SPACE_18_PX)).pb(px(SPACE_14_PX)).child(picker));
                }
            }
            let reset = changed.then(|| {
                div()
                    .id(SharedString::from(format!("reset-{}-{title}", resettable.first().map_or("", |s| s.path))))
                    .px(px(SPACE_10_PX))
                    .py(px(SPACE_3_PX))
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(TEXT_12_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                    .child(t("Reset"))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_group(&resettable, cx)))
            });
            col = col.child(
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .child(div().flex_1().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(mandara_ui::bidi::paragraph(tr(title))))
                            .children(reset),
                    )
                    .child(card),
            );
        }
        col.into_any_element()
    }

    /// Keyboard shortcuts: Mandara's compositor binds with chord capture,
    /// conflict detection, actions and quick-add suggestions.
    fn shortcuts(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        use mandara_config::compositor::{BindAction, Keybind, Target, existing_chords};
        let target = match mandara_compositor::detect() {
            Some(mandara_compositor::CompositorKind::MangoWM) => Some(Target::Mango),
            Some(mandara_compositor::CompositorKind::Niri) => Some(Target::Niri),
            Some(mandara_compositor::CompositorKind::Hyprland) => Some(Target::Hyprland),
            None => None,
        };
        let _ = existing_chords;
        let taken = keybind_conflicts::user_binds(target, &cfg.compositor, cx);
        let binds = &cfg.compositor.keybinds;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for b in binds {
            *counts.entry(b.chord()).or_default() += 1;
        }
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, b) in binds.iter().enumerate() {
            let capturing = self.capturing == Some(i);
            let (is_mandara, text) = match &b.action {
                BindAction::Mandara(w) => (true, w.as_str()),
                BindAction::Spawn(c) => (false, c.as_str()),
            };
            let action = self.list_field(
                format!("bind-{i}-action"),
                text,
                if is_mandara { t("Mandara command, e.g. launcher toggle") } else { t("Program, e.g. ghostty") },
                Rc::new(move |cfg, v| {
                    if let Some(k) = cfg.compositor.keybinds.get_mut(i) {
                        k.action = match k.action {
                            BindAction::Mandara(_) => BindAction::Mandara(v.trim().to_string()),
                            BindAction::Spawn(_) => BindAction::Spawn(v.trim().to_string()),
                        };
                    }
                }),
                cx,
            );
            let conflict = if b.key.is_empty() {
                None
            } else if b.problem().is_some() {
                // Skipped by the drop-in (it would block typing that key).
                Some((t("Needs Super, Ctrl or Alt"), p.warning_amber))
            } else if counts.get(&b.chord()).copied().unwrap_or(0) > 1 {
                Some((t("Duplicate"), p.urgent_coral))
            } else {
                None
            };
            let used = (!b.key.is_empty() && conflict.is_none()).then(|| taken.get(&b.chord())).flatten().map(|u| keybind_conflicts::badge(i, u, p));
            let chord_label = if capturing {
                t("Press keys…").to_string()
            } else if b.key.is_empty() {
                t("Set shortcut").to_string()
            } else {
                b.display().replace('+', " + ")
            };
            list = list.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(
                        div()
                            .id(("chord", i))
                            .min_w(px(DIM_170_PX))
                            .h(px(DIM_34_PX))
                            .px(px(SPACE_12_PX))
                            .rounded(px(RADIUS_10_PX))
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .font_family(mandara_ui::fonts::fonts(cx).mono.clone())
                            .text_size(px(TEXT_12_5_PX))
                            .border_2()
                            .border_color(if capturing { p.primary.hsla() } else { gpui::transparent_black() })
                            .bg(if capturing { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container.hsla() })
                            .child(chord_label)
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.capturing = Some(i);
                                this.focus.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id(("bind-kind", i))
                            .row()
                            .p(px(SPACE_3_PX))
                            .rounded(px(RADIUS_10_PX))
                            .bg(p.surface_container.hsla())
                            .children([(tk("Mandara"), true), (tk("Program"), false)].map(|(label, mandara)| {
                                let active = mandara == is_mandara;
                                div()
                                    .id(SharedString::from(format!("kind-{i}-{label}")))
                                    .px(px(SPACE_10_PX))
                                    .h(px(DIM_28_PX))
                                    .rounded(px(RADIUS_8_PX))
                                    .row()
                                    .items_center()
                                    .cursor_pointer()
                                    .text_size(px(TEXT_12_PX))
                                    .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()))
                                    .child(mandara_ui::bidi::paragraph(tr(label)))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.list_fields.remove(&format!("bind-{i}-action"));
                                        update_config(cx, |c| {
                                            if let Some(k) = c.compositor.keybinds.get_mut(i) {
                                                let text = match &k.action {
                                                    BindAction::Mandara(t) | BindAction::Spawn(t) => t.clone(),
                                                };
                                                k.action = if mandara { BindAction::Mandara(text) } else { BindAction::Spawn(text) };
                                            }
                                        });
                                    }))
                            })),
                    )
                    .child(div().flex_1().min_w(px(DIM_200_PX)).child(action))
                    .when_some(conflict, |el, (label, color)| {
                        el.child(div().px(px(SPACE_10_PX)).h(px(DIM_24_PX)).rounded_full().row().items_center().bg(color.with_alpha(ALPHA_16).hsla()).text_color(color.hsla()).text_size(px(TEXT_11_PX)).child(label))
                    })
                    .when_some(used, |el, badge| el.child(badge))
                    .child(
                        div()
                            .id(("bind-del", i))
                            .size(px(DIM_32_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .child(icon("trash", ICON_15_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.list_fields.clear();
                                this.capturing = None;
                                update_config(cx, |c| {
                                    if i < c.compositor.keybinds.len() {
                                        c.compositor.keybinds.remove(i);
                                    }
                                });
                            })),
                    ),
            );
        }
        const QUICK: &[(&str, &str)] = &[
            (tk("Launcher"), "launcher toggle"),
            (tk("Wallpapers"), "wallpaper picker toggle"),
            (tk("Screenshot"), "screenshot region"),
            (tk("Record region"), "record region"),
            (tk("Stop recording"), "record stop"),
            (tk("Sidebar"), "sidebar toggle"),
            (tk("Power menu"), "session menu toggle"),
            (tk("Clipboard"), "panel toggle clipboard"),
            (tk("Light / dark"), "theme mode toggle"),
            (tk("Pomodoro"), "pomodoro toggle"),
            (tk("Studio"), "studio"),
        ];
        let quick = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(QUICK.iter().map(|(label, cmd)| {
            let cmd = cmd.to_string();
            div()
                .id(SharedString::from(format!("quick-{label}")))
                .px(px(SPACE_12_PX))
                .h(px(DIM_30_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .cursor_pointer()
                .bg(p.surface_container_high.hsla())
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_14).hsla()))
                .text_size(px(TEXT_12_5_PX))
                .child(icon("plus", ICON_12_PX, p.primary))
                .child(mandara_ui::bidi::paragraph(tr(label)))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    let cmd = cmd.clone();
                    this.list_fields.clear();
                    let index = config(cx).read(cx).get().compositor.keybinds.len();
                    update_config(cx, |c| c.compositor.keybinds.push(Keybind { mods: Vec::new(), key: String::new(), action: BindAction::Mandara(cmd) }));
                    // Straight into chord capture for the new row.
                    this.capturing = Some(index);
                    this.focus.focus(window, cx);
                    cx.notify();
                }))
        }));
        let target_name = match target {
            Some(Target::Mango) => "MangoWM",
            Some(Target::Niri) => "Niri",
            Some(Target::Hyprland) => "Hyprland",
            None => t("your compositor"),
        };
        div()
            .id("shortcuts")
            .track_focus(&self.focus)
            .row()
            .flex_col()
            .gap(px(SPACE_18_PX))
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                let Some(i) = this.capturing else { return };
                let k = &e.keystroke;
                if k.key == "escape" {
                    this.capturing = None;
                    // Cancelling a just-added row removes it: a bind with
                    // no key is never written, so it would only linger.
                    update_config(cx, |c| {
                        if c.compositor.keybinds.get(i).is_some_and(|b| b.key.trim().is_empty()) {
                            c.compositor.keybinds.remove(i);
                        }
                    });
                    this.list_fields.clear();
                    cx.notify();
                    return;
                }
                // Wait for a non-modifier key.
                if matches!(k.key.as_str(), "shift" | "control" | "alt" | "platform" | "function" | "super") {
                    return;
                }
                let key = mandara_config::compositor::keysym_for(&k.key);
                let mut mods = Vec::new();
                if k.modifiers.platform {
                    mods.push("super".to_string());
                }
                if k.modifiers.control {
                    mods.push("ctrl".to_string());
                }
                if k.modifiers.alt {
                    mods.push("alt".to_string());
                }
                if k.modifiers.shift {
                    mods.push("shift".to_string());
                }
                this.capturing = None;
                update_config(cx, |c| {
                    if let Some(b) = c.compositor.keybinds.get_mut(i) {
                        b.mods = mods;
                        b.key = key;
                    }
                });
                cx.notify();
            }))
            .child(div().text_color(p.on_surface_variant.hsla()).line_height(px(LEADING_19_PX)).child(tf(
                "Mandara writes these to its own drop-in for {target_name} (never your config) and reloads it. Chords your own config already uses are skipped and marked “taken”. Click a shortcut, then press the keys; Esc cancels.",
                &[("target_name", &target_name)],
            )))
            .child(list)
            .child(div().row().flex_col().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Add a shortcut"))).child(quick))
            .when(target.is_some(), |el| {
                let ours: Rc<[String]> = binds.iter().map(|b| b.chord()).collect();
                el.child(
                    div()
                        .row()
                        .flex_col()
                        .gap(px(SPACE_8_PX))
                        .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(tf("Shortcuts in your {target_name} config", &[("target_name", &target_name)])))
                        .child(keybind_conflicts::own_binds(taken.clone(), ours, mandara_ui::fonts::fonts(cx).mono.clone(), p)),
                )
            })
            .into_any_element()
    }

    /// A text field bound to one field of a list row; `apply` writes it back.
    fn list_field(&mut self, key: String, current: &str, placeholder: &'static str, apply: Rc<dyn Fn(&mut ShellConfig, String)>, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = self.list_fields.get(&key) {
            f.update(cx, |f, _| f.sync(current));
            return f.clone();
        }
        let field = cx.new(|cx| {
            TextField::new(
                current.to_string(),
                placeholder,
                false,
                Rc::new(move |value, _, cx| {
                    let apply = apply.clone();
                    update_config(cx, move |c| apply(c, value));
                }),
                cx,
            )
        });
        self.list_fields.insert(key, field.clone());
        field
    }

    /// Slash commands and skills: edit, add and remove.
    fn launcher_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let delete_btn = |id: SharedString, p: &MandaraPalette| {
            div()
                .id(id)
                .size(px(DIM_32_PX))
                .flex_none()
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                .child(icon("trash", ICON_15_PX, p.on_surface_variant))
        };
        let add_btn = |id: &'static str, label: &'static str, p: &MandaraPalette| {
            div()
                .id(id)
                .h(px(DIM_34_PX))
                .px(px(SPACE_14_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .cursor_pointer()
                .bg(p.primary.with_alpha(ALPHA_14).hsla())
                .text_color(p.primary.hsla())
                .font_weight(FontWeight::MEDIUM)
                .child(icon("plus", ICON_14_PX, p.primary))
                .child(label)
        };
        let stack = self.stack_rows;
        // Slash commands.
        let mut cmds = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, c) in cfg.launcher.commands.iter().enumerate() {
            let trigger = self.list_field(format!("cmd-{i}-trigger"), &c.trigger, t("trigger"), Rc::new(move |cfg, v| if let Some(c) = cfg.launcher.commands.get_mut(i) { c.trigger = v.trim().trim_start_matches('/').to_string() }), cx);
            let title = self.list_field(format!("cmd-{i}-title"), &c.title, t("Title"), Rc::new(move |cfg, v| if let Some(c) = cfg.launcher.commands.get_mut(i) { c.title = v }), cx);
            let action = self.list_field(format!("cmd-{i}-action"), &c.action, "url:https://…{query}", Rc::new(move |cfg, v| if let Some(c) = cfg.launcher.commands.get_mut(i) { c.action = v.trim().to_string() }), cx);
            cmds = cmds.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .when(stack, |el| el.flex_col())
                    .gap(px(SPACE_8_PX))
                    .child(div().row().gap(px(SPACE_8_PX)).items_center().child(div().text_color(p.primary.hsla()).font_weight(FontWeight::SEMIBOLD).child("/")).child(div().w(px(DIM_90_PX)).child(trigger)).child(div().flex_1().min_w(px(DIM_120_PX)).child(title)))
                    .child(div().row().flex_1().min_w(px(0.0)).gap(px(SPACE_8_PX)).items_center().child(div().flex_1().min_w(px(0.0)).child(action)).child(delete_btn(SharedString::from(format!("cmd-del-{i}")), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.list_fields.clear();
                        update_config(cx, |c| {
                            if i < c.launcher.commands.len() {
                                c.launcher.commands.remove(i);
                            }
                        });
                    })))),
            );
        }
        cmds = cmds.child(div().row().child(add_btn("cmd-add", t("Add slash command"), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.list_fields.clear();
            update_config(cx, |c| {
                c.launcher.commands.push(mandara_config::SlashCommand {
                    trigger: format!("cmd{}", c.launcher.commands.len() + 1),
                    title: t("New command").into(),
                    description: String::new(),
                    action: "url:https://duckduckgo.com/?q={query}".into(),
                    icon: "search".into(),
                })
            });
        }))));
        // Skills.
        let mut skills = div().row().flex_col().gap(px(SPACE_8_PX));
        for (i, sk) in cfg.launcher.skills.iter().enumerate() {
            let name = self.list_field(format!("skill-{i}-name"), &sk.name, t("Name"), Rc::new(move |cfg, v| if let Some(s) = cfg.launcher.skills.get_mut(i) { s.name = v }), cx);
            let desc = self.list_field(format!("skill-{i}-desc"), &sk.description, t("What it does"), Rc::new(move |cfg, v| if let Some(s) = cfg.launcher.skills.get_mut(i) { s.description = v }), cx);
            let steps = self.list_field(
                format!("skill-{i}-steps"),
                &sk.steps.join("; "),
                t("commands separated by ;  e.g. notifications dnd on; pomodoro start"),
                Rc::new(move |cfg, v| {
                    if let Some(s) = cfg.launcher.skills.get_mut(i) {
                        s.steps = v.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
                    }
                }),
                cx,
            );
            skills = skills.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(div().row().when(stack, |el| el.flex_col()).gap(px(SPACE_8_PX)).child(div().w(px(DIM_180_PX)).child(name)).child(div().flex_1().min_w(px(0.0)).child(desc)))
                    .child(div().row().gap(px(SPACE_8_PX)).items_center().child(div().flex_1().min_w(px(0.0)).child(steps)).child(delete_btn(SharedString::from(format!("skill-del-{i}")), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.list_fields.clear();
                        update_config(cx, |c| {
                            if i < c.launcher.skills.len() {
                                c.launcher.skills.remove(i);
                            }
                        });
                    })))),
            );
        }
        skills = skills.child(div().row().child(add_btn("skill-add", t("Add skill"), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            this.list_fields.clear();
            update_config(cx, |c| {
                c.launcher.skills.push(mandara_config::Skill { name: t("New skill").into(), description: String::new(), steps: vec!["theme mode toggle".into()], icon: "grid".into() })
            });
        }))));
        let group = |title: &'static str, help: &'static str, body: gpui::Div, p: &MandaraPalette| {
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(title))
                .child(div().p(px(SPACE_14_PX)).rounded(px(RADIUS_20_PX)).bg(p.surface_container.hsla()).row().flex_col().gap(px(SPACE_10_PX)).child(div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(help)).child(body))
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_26_PX))
            .child(group(
                t("Slash commands"),
                t("Type /trigger and your words in the launcher. Actions: url:<address with {query}> opens the browser, mandara:<command> runs a Mandara command (see `mandara commands`), exec:<shell command> runs a program; {query} is replaced by what you typed."),
                cmds,
                p,
            ))
            .child(group(t("Skills"), t("Named routines: each step is a Mandara command, run in order. Find them by name in the launcher or as /name."), skills, p))
            .into_any_element()
    }

    /// Visual icon chooser: bundled set, distro logo, themed suggestions,
    /// live search over the whole icon theme, and a custom path.
    fn icon_picker(&mut self, setting: &Setting, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        const BUNDLED: &[&str] = &["grid", "search", "circle", "sun", "moon", "crescent", "music", "monitor", "image", "note", "bolt", "cloud-sun", "coffee", "power"];
        const SUGGESTED: &[&str] = &[
            "distro", "start-here", "start-here-symbolic", "distributor-logo", "view-app-grid-symbolic", "view-grid-symbolic", "applications-all-symbolic",
            "open-menu-symbolic", "archlinux-logo", "cachyos", "fedora-logo-icon", "ubuntu-logo-icon", "debian-logo", "nix-snowflake", "manjaro",
            "endeavouros", "linuxmint-logo", "pop-os", "opensuse-logo",
        ];
        let current = registry::get(cfg, setting).as_str().unwrap_or("grid").to_string();
        let query = self.icon_query.trim().to_lowercase();
        // Resolved once per query (the full theme listing is thousands of names).
        let theme_names = match &self.icon_matches {
            Some((q, names)) if *q == query => names.clone(),
            _ => {
                let names: Rc<[String]> = if query.is_empty() {
                    SUGGESTED.iter().filter(|n| **n == "distro" || mandara_ui::icon_theme::resolve(n, None, false).is_some()).map(|n| n.to_string()).collect()
                } else {
                    mandara_ui::icon_theme::all_names().into_iter().filter(|n| n.to_lowercase().contains(&query)).take(ICON_MATCHES_MAX).collect()
                };
                self.icon_matches = Some((query.clone(), names.clone()));
                names
            }
        };
        let tile = |spec: String, label: Option<String>, p: &MandaraPalette, cx: &mut Context<Self>| {
            let active = spec == current;
            let value = spec.clone();
            div()
                .id(SharedString::from(format!("icon-{spec}")))
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_4_PX))
                .cursor_pointer()
                .child(
                    div()
                        .size(px(DIM_46_PX))
                        .rounded(px(RADIUS_14_PX))
                        .row()
                        .items_center()
                        .justify_center()
                        .bg(if active { p.primary.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() })
                        .border_2()
                        .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                        .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                        .child(mandara_ui::icons::any_icon(&spec, ICON_22_PX, if active { p.primary } else { p.on_surface })),
                )
                .when_some(label, |el, l| el.child(div().w_full().text_center().truncate().text_size(px(TEXT_10_5_PX)).text_color(p.on_surface_variant.hsla()).child(l)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    let v = value.clone();
                    update_config(cx, |c| c.appearance.launcher_icon = v);
                }))
        };
        let section_title = |t: &'static str, p: &MandaraPalette| div().pt(px(SPACE_6_PX)).text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t);
        let bundled: Vec<_> = BUNDLED.iter().map(|n| tile(n.to_string(), None, p, cx)).collect();
        let themed: Vec<_> = theme_names.iter().map(|n| tile(n.clone(), Some(if n == "distro" { t("Distro logo").into() } else { n.trim_end_matches("-symbolic").to_string() }), p, cx)).collect();
        let empty = themed.is_empty();
        // Inside the settings card (its padding + the picker row's) and the picker's own padding.
        let fit = grid::fit(self.page.column_w - 2.0 * (SPACE_6_PX + SPACE_18_PX + SPACE_14_PX), ICON_TILE_GRID);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .p(px(SPACE_14_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_base.with_alpha(ALPHA_55).hsla())
            .child(section_title(t("Mandara"), p))
            .child(fit.render(bundled))
            .child(section_title(if query.is_empty() { t("From your system") } else { t("Matching icons") }, p))
            .child(div().child(self.icon_search.clone()))
            .when(!empty, |el| el.child(fit.render(themed)))
            .when(empty, |el| {
                el.child(div().py(px(SPACE_8_PX)).text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tf("No icons named like “{query}” in your theme", &[("query", &query)])))
            })
            .child(section_title(t("Your own"), p))
            .child(div().child(self.icon_path.clone()))
            .into_any_element()
    }

    /// Language picker: every supported language as a card, in its own
    /// script, plus "System".
    fn language_picker(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        use mandara_config::Locale;
        use mandara_i18n::Lang;
        let current = cfg.general.locale;
        let system = Lang::from_env();
        let fonts = mandara_ui::fonts::fonts(cx);
        let card = |id: String, sample: String, native: String, english: String, selected: bool, rtl: bool, locale: Locale, cx: &mut Context<Self>| {
            div()
                .id(SharedString::from(id))
                .flex_1()
                .rounded(px(RADIUS_16_PX))
                .p(px(SPACE_14_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_6_PX))
                .cursor_pointer()
                .bg(if selected { p.primary.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
                .border_2()
                .border_color(if selected { p.primary.hsla() } else { gpui::transparent_black() })
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                .child(
                    div()
                        .row()
                        .items_center()
                        .justify_between()
                        .child(div().text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).when(rtl, |el| el.font_family(fonts.arabic.clone())).child(sample))
                        .when(selected, |el| el.child(div().size(px(DIM_20_PX)).rounded_full().bg(p.primary.hsla()).row().items_center().justify_center().child(icon("check", ICON_12_PX, p.on_primary)))),
                )
                .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).when(rtl, |el| el.font_family(fonts.arabic.clone())).child(native))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(english))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    if let Some(setting) = registry::by_path(LOCALE_PATH) {
                        this.apply(setting, serde_json::to_value(locale).unwrap_or(Value::Null), cx);
                    }
                }))
        };
        let mut cards = vec![card(
            "lang-auto".into(),
            system.sample().into(),
            mandara_i18n::t("System").into(),
            format!("{} · {}", mandara_i18n::t("Follows your desktop"), system.native_name()),
            current == Locale::Auto,
            system.is_rtl(),
            Locale::Auto,
            cx,
        )];
        for lang in Lang::ALL {
            let locale = Locale::from_lang(lang);
            cards.push(card(format!("lang-{}", lang.code()), lang.sample().into(), lang.native_name().into(), tr(lang.english_name()), current == locale, lang.is_rtl(), locale, cx));
        }
        let grid = grid::fit(self.page.column_w, LANGUAGE_GRID).balanced(cards.len()).render(cards);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(div().px(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(mandara_i18n::t("Language")))
            .child(grid)
            .into_any_element()
    }

    fn performance(&self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        // Sample only while this page is on screen.
        let this = cx.entity().downgrade();
        perf::poll_while(&self.perf, move |cx| this.read_with(cx, |v, _| v.section == Section::Performance && v.query.is_empty()).unwrap_or(false), cx);
        let m = self.perf.read(cx);
        let card = || div().rounded(px(RADIUS_18_PX)).bg(p.surface_container.hsla()).p(px(SPACE_20_PX)).row().flex_col().gap(px(SPACE_14_PX));
        let Some(now) = m.latest() else {
            return card()
                .child(div().font_weight(FontWeight::MEDIUM).child(t("The shell isn't running")))
                .child(div().text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Start it with `mandara`; its CPU and memory appear here live."))))
                .into_any_element();
        };
        let cpu_hist: Vec<f32> = m.shell.iter().map(|u| u.cpu).collect();
        let mem_hist: Vec<f32> = m.shell.iter().map(|u| u.rss_mib).collect();
        let vram_hist: Vec<f32> = m.shell.iter().map(|u| u.vram_mib.unwrap_or(0.0)).collect();
        let tile = |label: &'static str, icon_name: &'static str, value: String, detail: String, hist: &[f32], floor: f32, color: mandara_theme::Color| {
            // Headroom keeps steady values mid-height instead of pinned flat at the top.
            let max = hist.iter().copied().fold(floor, f32::max) * SPARK_HEADROOM;
            div()
                .flex_1()
                .min_w(px(0.0))
                .rounded(px(RADIUS_14_PX))
                .bg(p.surface_container_high.hsla())
                .p(px(SPACE_16_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_6_PX))
                .child(div().row().items_center().gap(px(SPACE_8_PX)).child(icon(icon_name, ICON_15_PX, color)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(label)))
                .child(div().text_size(px(TEXT_26_PX)).font_weight(FontWeight::MEDIUM).font_family(mandara_ui::fonts::fonts(cx).mono.clone()).child(value))
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(detail))
                .child(
                    div()
                        .mt(px(SPACE_6_PX))
                        .h(px(DIM_56_PX))
                        // A time series: oldest → newest left to right everywhere.
                        .ltr_row()
                        .items_end()
                        .gap(px(SPACE_3_PX))
                        .children((0..perf::HISTORY).map(|i| {
                            // Right-align the history so the newest bar is last.
                            let offset = perf::HISTORY - hist.len();
                            let v = if i >= offset { hist[i - offset] } else { 0.0 };
                            let h = if i >= offset { (v / max).clamp(0.0, 1.0) * SPARK_RANGE_PX + SPARK_MIN_PX } else { SPARK_MIN_PX };
                            div()
                                .flex_1()
                                .h(px(h))
                                .rounded(px(RADIUS_1_5_PX))
                                .bg(if i >= offset { color.with_alpha(SPARK_OLDEST_ALPHA + (1.0 - SPARK_OLDEST_ALPHA) * (i as f32 / perf::HISTORY as f32)).hsla() } else { p.on_surface.with_alpha(ALPHA_6).hsla() })
                        })),
                )
        };
        let shell = card()
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(t("Mandara Shell")))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(tf("pid {pid} · {threads} threads", &[("pid", &m.shell_pid.unwrap_or(0)), ("threads", &now.threads)]))),
            )
            .child(
                grid::fit(self.page.column_w - 2.0 * SPACE_20_PX, STAT_TILE_GRID).balanced(if now.vram_mib.is_some() { 3 } else { 2 }).render([tile(
                        t("CPU"),
                        "cpu",
                        format!("{:.1}%", now.cpu),
                        tf("of one core · avg {average}% · {cores} cores", &[("average", &format!("{:.1}", m.average_cpu())), ("cores", &m.cores)]),
                        &cpu_hist,
                        CPU_SPARK_FLOOR_PERCENT,
                        p.primary,
                    ),
                    tile(t("Memory"), "memory", format!("{:.0} MB", now.rss_mib), tf("resident · peak {peak} MB", &[("peak", &format!("{:.0}", m.peak_rss()))]), &mem_hist, 1.0, p.tertiary)]
                .into_iter()
                .chain(now.vram_mib.map(|v| tile(t("Video memory"), "monitor", format!("{v:.0} MB"), tf("on the GPU · peak {peak} MB", &[("peak", &format!("{:.0}", m.peak_vram()))]), &vram_hist, 1.0, p.secondary)))),
            );
        let studio = card().child(
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(icon("window", ICON_15_PX, p.on_surface_variant))
                .child(div().flex_1().min_w(px(0.0)).child(t("Mandara Studio (this window)")))
                .child(div().font_family(mandara_ui::fonts::fonts(cx).mono.clone()).child(format!("{:.1}% · {:.0} MB", m.studio.cpu, m.studio.rss_mib))),
        )
        .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Studio runs as its own process, so its usage is measured separately and never counted in the shell figures above."))));
        let action = |id: &'static str, icon_name: &'static str, label: &'static str, danger: bool, args: &'static [&'static str]| {
            div()
                .id(id)
                .h(px(DIM_36_PX))
                .px(px(SPACE_14_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .cursor_pointer()
                .bg(if danger { p.urgent_coral.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
                .text_color(if danger { p.urgent_coral.hsla() } else { p.on_surface.hsla() })
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.opacity(ALPHA_85))
                .child(icon(icon_name, ICON_15_PX, if danger { p.urgent_coral } else { p.on_surface_variant }))
                .child(label)
                .on_click(move |_, _, cx| {
                    if let Some(ipc) = cx.try_global::<crate::perf::ShellIpc>().cloned() {
                        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                        std::thread::spawn(move || {
                            if let Err(err) = (ipc.0)(args) {
                                tracing::warn!("shell command failed: {err}");
                            }
                        });
                    }
                })
        };
        let controls = card()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(t("Shell process")))
            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(
                t("Restart reloads everything from scratch. Quit stops Mandara completely (the supervisor exits too); start it again with `mandara`."),
            ))
            .child(div().row().gap(px(SPACE_10_PX)).child(action("shell-restart", "refresh", t("Restart shell"), false, &["restart"])).child(action("shell-quit", "power", t("Quit Mandara"), true, &["quit"])));
        div().row().flex_col().gap(px(SPACE_14_PX)).child(shell).child(studio).child(controls).into_any_element()
    }

    fn about(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        // Updates: the patch-first update card (updates_card.rs).
        let updates = self.updates_card(cfg, p, cx);
        let build = mandara_core::build_info(cx);
        let registry = cx.global::<PluginRegistry>();
        let commands = registry.commands.all().len();
        let first_party = registry.manifests().filter(|m| m.origin == PluginOrigin::Builtin && !HIDDEN_PLUGINS.contains(&m.id.as_str())).count();
        let compositor = mandara_compositor::detect().map(|k| format!("{k:?}")).unwrap_or_else(|| t("none").into());
        let fonts = mandara_ui::fonts::fonts(cx);
        let link = |id: &'static str, glyph: &'static str, label: SharedString, url: &'static str| {
            div()
                .id(id)
                .h(px(DIM_34_PX))
                .px(px(SPACE_14_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .cursor_pointer()
                .bg(p.surface_container_high.with_alpha(ALPHA_80).hsla())
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_14).hsla()))
                .child(icon(glyph, ICON_14_PX, p.on_surface_variant))
                .child(label)
                .on_click(move |_, _, _| open_link(url))
        };

        // Hero: the logo on a soft OKLCH wash of the theme's accent gradient.
        let hero = div()
            .relative()
            .overflow_hidden()
            .rounded(px(RADIUS_26_PX))
            .bg(p.surface_container.hsla())
            .border_1()
            .border_color(p.specular_rim_light.with_alpha(ALPHA_25).hsla())
            .child(div().absolute().inset_0().bg(mandara_core::oklab_gradient(ABOUT_WASH_ANGLE, p.accent_gradient_start.with_alpha(ALPHA_30), p.accent_gradient_end.with_alpha(ALPHA_8))))
            .child(
                div()
                    .relative()
                    .p(px(SPACE_28_PX))
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_24_PX))
                    .child(gpui::img(gpui::ImageSource::from(mandara_ui::plugin_art::brand_icon())).size(px(DIM_96_PX)).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_220_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_6_PX))
                            .child(div().text_size(px(TEXT_26_PX)).font_weight(FontWeight::BOLD).child(t("Mandara Shell")))
                            .child(div().text_size(px(TEXT_14_PX)).text_color(p.on_surface.with_alpha(ALPHA_85).hsla()).child(mandara_ui::bidi::paragraph(t("A design-first Wayland desktop shell, written entirely in Rust."))))
                            .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Named after El-Mandara (المندرة), the coastal district of Alexandria."))))
                            .child(
                                div()
                                    .pt(px(SPACE_6_PX))
                                    .row()
                                    .flex_wrap()
                                    .gap(px(SPACE_8_PX))
                                    .child(about_chip(format!("v{}", build.version), p.primary, fonts.mono.clone(), p))
                                    .child(about_chip(LICENSE.to_string(), p.secondary, fonts.ui.clone(), p))
                                    .child(about_chip(t("100% Rust").to_string(), p.tertiary, fonts.ui.clone(), p)),
                            ),
                    ),
            );

        // At a glance.
        let stat = |value: String, label: SharedString, glyph: &'static str, tone: mandara_theme::Color| {
            div()
                .flex_1()
                .min_w(px(DIM_140_PX))
                .p(px(SPACE_16_PX))
                .rounded(px(RADIUS_18_PX))
                .bg(p.surface_container.hsla())
                .row()
                .flex_col()
                .gap(px(SPACE_6_PX))
                .child(icon(glyph, ICON_16_PX, tone))
                .child(div().text_size(px(TEXT_24_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(value))
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(label))
        };
        let stats = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_12_PX))
            .child(stat(first_party.to_string(), t("Mandara plugins").into(), "grid", p.primary))
            .child(stat(commands.to_string(), t("Documented commands").into(), "terminal", p.secondary))
            .child(stat(mandara_i18n::Lang::ALL.len().to_string(), t("Languages").into(), "globe", p.tertiary))
            .child(stat(compositor, t("Compositor").into(), "window", p.success_emerald));

        // Details.
        let line = |k: SharedString, v: String, mono: bool| {
            div()
                .row()
                .justify_between()
                .gap(px(SPACE_16_PX))
                .py(px(SPACE_10_PX))
                .child(div().flex_none().text_color(p.on_surface_variant.hsla()).child(k))
                // Long values (the config path) ellipsize instead of overflowing a narrow window.
                .child(div().min_w(px(0.0)).truncate().font_weight(FontWeight::MEDIUM).when(mono, |el| el.font_family(fonts.mono.clone()).text_size(px(TEXT_12_5_PX))).child(v))
        };
        let details = div()
            .rounded(px(RADIUS_18_PX))
            .bg(p.surface_container.hsla())
            .px(px(SPACE_20_PX))
            .py(px(SPACE_8_PX))
            .row()
            .flex_col()
            .child(line(t("Build").into(), build.long.clone(), true))
            .child(line(t("Theme").into(), mandara_core::theme_entity(cx).read(cx).target().name.clone(), false))
            .child(line(t("Appearance").into(), if cfg.theme.mode.is_dark() { t("Dark").into() } else { t("Light").into() }, false))
            .child(line(t("Config").into(), mandara_config::paths::config_file().display().to_string(), true));

        let links = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_8_PX))
            .child(link("about-source", "external", t("Source code").into(), PROJECT_URL))
            .child(link("about-issues", "alert", t("Report a problem").into(), ISSUES_URL))
            .child(link("about-plugins", "package", t("Plugin guide").into(), PLUGIN_GUIDE_URL))
            .child(link("about-releases", "download", t("Releases").into(), RELEASES_URL));

        // Support: optional, friendly, never in the way.
        let support = div()
            .relative()
            .overflow_hidden()
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .child(div().absolute().inset_0().bg(mandara_core::oklab_gradient(ABOUT_WASH_ANGLE, p.urgent_coral.with_alpha(ALPHA_12), p.warning_amber.with_alpha(ALPHA_5))))
            .child(
                div()
                    .relative()
                    .p(px(SPACE_20_PX))
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_16_PX))
                    .child(div().flex_none().size(px(DIM_48_PX)).rounded_full().bg(p.urgent_coral.with_alpha(ALPHA_15).hsla()).row().items_center().justify_center().child(icon("heart", ICON_22_PX, p.urgent_coral)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_220_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_4_PX))
                            .child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Enjoying Mandara?")))
                            .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface_variant.hsla()).child(t(
                                "Mandara is free and always will be. If it makes your desktop a little nicer and you'd like to say thanks, you can chip in toward the AI credits and developer time behind it. Completely optional: using it, sharing it and reporting bugs help just as much.",
                            ))),
                    )
                    .child(
                        div()
                            .id("about-support")
                            .flex_none()
                            .h(px(DIM_38_PX))
                            .px(px(SPACE_16_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .cursor_pointer()
                            .bg(p.urgent_coral.with_alpha(ALPHA_15).hsla())
                            .text_color(p.on_surface.hsla())
                            .text_size(px(TEXT_13_PX))
                            .font_weight(FontWeight::SEMIBOLD)
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_25).hsla()))
                            .child(icon("coffee", ICON_15_PX, p.urgent_coral))
                            .child(t("Buy me a coffee on Ko-fi"))
                            .on_click(|_, _, _| open_link(SUPPORT_URL)),
                    ),
            );

        div().row().flex_col().gap(px(SPACE_16_PX)).child(hero).child(updates).child(stats).child(details).child(links).child(support).into_any_element()
    }
}

impl Render for StudioView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        frame_meter::begin();
        let p = mandara_core::frame_palette(window, cx);
        let cfg = config(cx).read(cx).snapshot();
        registry::prime(&cfg);
        let fonts = mandara_ui::fonts::fonts(cx);
        // Breakpoints: icon rail under 900 px; stacked rows when the content
        // column gets too narrow for label + control side by side.
        self.page = grid::PageMetrics::of(window);
        self.rail = self.page.rail;
        self.stack_rows = self.page.stack_rows;
        let gutter = self.page.gutter;
        let rail = self.rail;

        let nav = self.nav(rail, window, &p, cx);

        let (title, body) = if !self.query.is_empty() {
            let q = self.query.clone();
            let matches: Vec<&Setting> = registry::settings().iter().filter(|s| s.matches(&q)).collect();
            // Pages whose name or topics match (VPN, autostart, cursor…),
            // including pages built from bespoke editors.
            let pages = form::page_matches(&q, &p, cx);
            let body = if matches.is_empty() && pages.is_none() {
                div().text_color(p.on_surface_variant.hsla()).child(tf("No settings match “{query}”", &[("query", &q)])).into_any_element()
            } else {
                let rows = (!matches.is_empty()).then(|| self.rows(matches, &cfg, &p, cx));
                div().row().flex_col().gap(px(SPACE_22_PX)).children(pages).children(rows).into_any_element()
            };
            (tf("Results for “{query}”", &[("query", &q)]), body)
        } else {
            let body = match self.section {
                Section::Home => self.home_page(&cfg, &p, window, cx),
                Section::Themes => self.themes_page(&cfg, &p, cx),
                Section::Plugins => self.store_page(&cfg, &p, window, cx),
                Section::About => self.about(&cfg, &p, cx),
                Section::Performance => {
                    let base = self.performance(&p, cx);
                    let diagnostics = self.diagnostics(&p, cx);
                    div().row().flex_col().gap(px(SPACE_14_PX)).child(base).child(diagnostics).into_any_element()
                }
                Section::Compositor => self.compositor_page(&cfg, &p, cx),
                Section::Audio => self.audio_page(&cfg, &p, cx),
                Section::Notifications => self.notifications_page(&cfg, &p, cx),
                Section::Privacy => self.privacy_page(&cfg, &p, cx),
                Section::Network => self.network_page(&cfg, &p, cx),
                Section::Input => self.input_page(&cfg, &p, cx),
                Section::Applications => self.apps_page(&cfg, &p, window, cx),
                Section::Profiles => self.profiles_page(&cfg, &p, cx),
                Section::Power => self.power_page(&cfg, &p, cx),
                Section::Displays => self.displays_page(&cfg, &p, cx),
                Section::Maintenance => self.maintenance(&p, cx),
                Section::LoginScreen => greeter_page::element(cx),
                Section::Users => users_page::element(cx),
                Section::Bar => {
                    let designer = self.bar_designer(&cfg, &p, cx);
                    let rows = self.rows(registry::in_section(Section::Bar), &cfg, &p, cx);
                    let displays = self.displays_link(mandara_config::outputs::SurfaceKind::Bar, &cfg, &p, cx);
                    div().row().flex_col().gap(px(SPACE_26_PX)).child(designer).child(rows).child(displays).into_any_element()
                }
                Section::Sidebar => {
                    // The tab editor first (as the Bar page's designer), then the settings.
                    let designer = self.sidebar_tabs_editor(&cfg, &p, window, cx);
                    let rows = self.rows(registry::in_section(Section::Sidebar), &cfg, &p, cx);
                    div().row().flex_col().gap(px(SPACE_26_PX)).child(designer).child(rows).into_any_element()
                }
                Section::General => {
                    let picker = self.language_picker(&cfg, &p, cx);
                    let rows = self.rows(registry::in_section(Section::General).filter(|s| s.path != LOCALE_PATH), &cfg, &p, cx);
                    // Under the Keyrings & passwords rows: what unlocks at login.
                    let keyrings = keyrings_card::element(cx);
                    div().row().flex_col().gap(px(SPACE_26_PX)).child(picker).child(rows).child(keyrings).into_any_element()
                }
                Section::Shortcuts => self.shortcuts(&cfg, &p, cx),
                Section::Launcher => {
                    // The placement diagram stands in for its choice row.
                    let placement = self.launcher_placement(&cfg, &p, cx);
                    let rows = self.rows(registry::in_section(Section::Launcher).filter(|s| s.path != LAUNCHER_POSITION_PATH), &cfg, &p, cx);
                    let editor = self.launcher_editor(&cfg, &p, cx);
                    let extras = self.launcher_extras(&cfg, &p, cx);
                    div().row().flex_col().gap(px(SPACE_26_PX)).child(placement).child(rows).child(extras).child(editor).into_any_element()
                }
                Section::Dock => self.dock_page(&cfg, &p, cx),
                Section::DesktopWidgets => {
                    let widgets = self.desktop_widgets_page(&cfg, &p, cx);
                    // The interactive cards and GPU visualizers (looks_page.rs).
                    let cards = self.desktop_cards_section(&cfg, &p);
                    div().row().flex_col().gap(px(SPACE_26_PX)).child(widgets).child(cards).into_any_element()
                }
                Section::WindowSwitcher => self.switcher_page(&cfg, &p, cx),
                Section::Wallpaper => {
                    // The wallpaper generator first (or the card offering it).
                    let generator = self.wallgen_panel(&cfg, &p, cx);
                    let rows = self.rows(registry::in_section(Section::Wallpaper), &cfg, &p, cx);
                    // Wallhaven's rows show with its plugin on; off, a
                    // one-line card offers it.
                    let wallhaven = (!cfg.wallhaven_enabled()).then(|| self.plugin_offer(mandara_ui::plugins::wallhaven::ID, &p, cx));
                    div().row().flex_col().gap(px(SPACE_26_PX)).child(generator).child(rows).children(wallhaven).into_any_element()
                }
                section => {
                    self.rows(registry::in_section(section), &cfg, &p, cx)
                }
            };
            (self.section.label(), body)
        };

        // Client-side title bar: the whole strip drags the window (double-
        // click maximizes), with minimize / maximize / close at the end.
        let win_btn = |id: &'static str, icon_name: &'static str, danger: bool| {
            div()
                .id(id)
                .size(px(DIM_30_PX))
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(if danger { p.urgent_coral.with_alpha(ALPHA_90).hsla() } else { p.on_surface.with_alpha(ALPHA_8).hsla() }))
                .child(icon(icon_name, ICON_14_PX, p.on_surface_variant))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let titlebar = div()
            .id("studio-titlebar")
            .h(px(DIM_46_PX))
            .flex_none()
            .px(px(SPACE_12_PX))
            .row()
            .items_center()
            .gap(px(SPACE_4_PX))
            .on_mouse_down(MouseButton::Left, |e, window, _| {
                if e.click_count >= 2 {
                    window.zoom_window();
                } else {
                    begin_move(window);
                }
            })
            .child(div().flex_1())
            .child(win_btn("win-min", "minus", false).on_click(|_, window, _| window.minimize_window()))
            .child(win_btn("win-max", "maximize", false).on_click(|_, window, _| window.zoom_window()))
            .child(win_btn("win-close", "close", true).on_click(|_, window, _| window.remove_window()));
        // `MANDARA_STUDIO_SCROLL=<px>` opens the page scrolled down, once
        // (screenshots of long pages without driving the pointer).
        static SCROLLED: std::sync::Once = std::sync::Once::new();
        if let Some(y) = std::env::var("MANDARA_STUDIO_SCROLL").ok().and_then(|v| v.parse::<f32>().ok()) {
            // Wait for pages that load their lists asynchronously.
            static OPENED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            const SCROLL_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);
            let handle = self.content_scroll.clone();
            if OPENED.get_or_init(std::time::Instant::now).elapsed() >= SCROLL_SETTLE {
                SCROLLED.call_once(|| handle.set_offset(gpui::point(px(0.0), -px(y).min(handle.max_offset().y))));
            } else if !SCROLLED.is_completed() {
                window.request_animation_frame();
            }
        }
        let content_bar = mandara_ui::scrollbar::keyed_state("studio-content");
        let content = div()
            .id("studio-content")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.content_scroll)
            .px(px(gutter))
            .pb(px(SPACE_30_PX))
            .child(
                div()
                    .w_full()
                    .max_w(px(PAGE_MAX_W_PX))
                    .mx_auto()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_22_PX))
                    // With the icon rail, search moves into the page header.
                    .when(rail, |el| el.child(self.search.clone()))
                    .child(div().font_family(fonts.display.clone()).text_size(px(if gutter < GUTTER_MEDIUM_PX { PAGE_TITLE_SMALL_PX } else { PAGE_TITLE_PX })).font_weight(FontWeight::MEDIUM).child(title))
                    .when_some(self.error.clone(), |el, e| {
                        el.child(div().px(px(SPACE_14_PX)).py(px(SPACE_10_PX)).rounded(px(RADIUS_12_PX)).bg(p.urgent_coral.with_alpha(ALPHA_15).hsla()).text_color(p.urgent_coral.hsla()).child(e))
                    })
                    .child(body),
            );
        // The page's overlay scroll bar (it fills the content's place).
        // Left to right, the page's end edge is the window's border.
        let edge = if mandara_i18n::language().is_rtl() { px(0.0) } else { px(mandara_ui::scrollbar::WINDOW_EDGE_CLEARANCE_PX) };
        let content = div().relative().flex_1().min_h(px(0.0)).flex().flex_col().child(content).child(mandara_ui::scrollbar::overlay_scrollbar(&self.content_scroll, &content_bar, &p).end_inset(edge));

        let modal = self.plugin_modal_layer(&cfg, &p, window, cx);
        let dock_picker = self.dock_picker_layer(&p, cx);
        // Right-to-left languages mirror everything (`mandara_ui::bidi`):
        // the sidebar sits on the right and text aligns to the right.
        div()
            .size_full()
            .relative()
            .row()
            .text_start()
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                if this.nav_key(e, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .bg(p.surface_base.hsla())
            .text_color(p.on_surface.hsla())
            .font_family(fonts.ui.clone())
            .text_size(px(TEXT_13_PX))
            .child(nav)
            .child(div().flex_1().min_w(px(0.0)).h_full().row().flex_col().child(titlebar).child(content))
            .children(modal)
            .children(dock_picker)
            .children(frame_meter::end())
    }
}

// ── Layout ──────────────────────────────────────────────────────────────
// Breakpoints, sidebar widths and gutters live in `tokens.rs` (Responsive
// layout) and are applied through `grid::PageMetrics`.
/// Page title size (smaller with the narrow gutter).
const PAGE_TITLE_PX: f32 = 28.0;
const PAGE_TITLE_SMALL_PX: f32 = 23.0;

// ── Performance sparklines ──────────────────────────────────────────────
/// Headroom over the highest sample, so steady values sit mid-height.
const SPARK_HEADROOM: f32 = 1.35;
/// Bar heights: an empty slot, plus the range a sample spans above it.
const SPARK_MIN_PX: f32 = 2.0;
const SPARK_RANGE_PX: f32 = 52.0;
/// Opacity of the oldest bar (the newest is opaque).
const SPARK_OLDEST_ALPHA: f32 = 0.35;
/// The CPU sparkline's scale never drops below this many percent.
const CPU_SPARK_FLOOR_PERCENT: f32 = 5.0;

// ── Settings controls ───────────────────────────────────────────────────
const MINUTES_PER_HOUR: i32 = 60;
const MINUTES_PER_DAY: i32 = 24 * MINUTES_PER_HOUR;
/// The time control's minute arrows move by a quarter hour.
const TIME_STEP_MINUTES: i32 = 15;
/// Slider steps at least this coarse show two decimals (finer: three).
const TWO_DECIMAL_STEP: f64 = 0.05;
/// A switch that can't be changed right now (see `registry::locked`).
const LOCKED_OPACITY: f32 = 0.45;
/// Widest a "doesn't apply here" note grows in place of a control.
const LOCKED_NOTE_MAX_W_PX: f32 = 260.0;
/// Narrowest a setting's label column gets beside a wide control.
const ROW_LABEL_MIN_PX: f32 = 160.0;

/// Most theme icons the icon picker lists for a search.
const ICON_MATCHES_MAX: usize = 64;

/// Config path of the language setting (Studio → General shows it as a picker).
const LOCALE_PATH: &str = "/general/locale";
/// The launcher's placement (drawn as a screen diagram on its page).
const LAUNCHER_POSITION_PATH: &str = "/launcher/look/position";


/// Settings grouped into cards by their group title, in order of first
/// appearance. A page's groups merge even when the registry lists them apart
/// (Themes' app themes come from two registry files); the same title in
/// another section (search results) stays its own card.
pub(crate) fn group_rows<'a>(settings: impl IntoIterator<Item = &'a Setting>) -> Vec<(&'static str, Vec<&'a Setting>)> {
    let mut groups: Vec<(&'static str, Vec<&'a Setting>)> = Vec::new();
    for s in settings {
        match groups.iter_mut().find(|(g, list)| *g == s.group && list.first().is_some_and(|f| f.section == s.section)) {
            Some((_, list)) => list.push(s),
            None => groups.push((s.group, vec![s])),
        }
    }
    groups
}

/// Project links on the About page.
const PROJECT_URL: &str = "https://github.com/theHamdiz/mandara";
const ISSUES_URL: &str = "https://github.com/theHamdiz/mandara/issues";
const PLUGIN_GUIDE_URL: &str = "https://github.com/theHamdiz/mandara/blob/main/PLUGINS.md";
const RELEASES_URL: &str = "https://github.com/theHamdiz/mandara/releases";
/// Optional support for the project (AI credits and developer time).
pub(crate) const SUPPORT_URL: &str = "https://ko-fi.com/thehamdiz";
/// The license, shown as is (an SPDX identifier, never translated).
const LICENSE: &str = "GPL-3.0-or-later";
/// Direction of the About page's accent washes, degrees.
const ABOUT_WASH_ANGLE: f32 = 135.0;

/// Opens a web link in the default browser, off the UI thread.
fn open_link(url: &'static str) {
    std::thread::spawn(move || {
        if let Err(err) = mandara_services::session::open_uri(url) {
            tracing::warn!("opening {url}: {err}");
        }
    });
}

/// A small tinted pill on the About hero (version, license…).
fn about_chip(text: String, tone: mandara_theme::Color, family: SharedString, p: &MandaraPalette) -> AnyElement {
    div()
        .h(px(DIM_24_PX))
        .px(px(SPACE_10_PX))
        .rounded_full()
        .row()
        .items_center()
        .bg(tone.with_alpha(ALPHA_14).hsla())
        .text_color(p.on_surface.hsla())
        .text_size(px(TEXT_11_5_PX))
        .font_weight(FontWeight::MEDIUM)
        .font_family(family)
        .child(text)
        .into_any_element()
}
