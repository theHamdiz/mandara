//! Studio's sidebar and Home page.
//!
//! Home is a widget dashboard: a greeting over the live wallpaper with an
//! accent wash, a bento of live cards (shell status, the theme with a
//! one-click switch, quick settings, recent changes and profiles) and a
//! card per sidebar group with a pill for each of its pages.
//!
//! The sidebar lists Home, then every section under its group
//! ([`NavGroup`]): collapsible (remembered per user in
//! [`crate::ui_state`]), scrollable, compact enough to fit expanded at
//! 1440 px, keyboard-navigable and mirrored for right-to-left languages.
//! While searching, every group opens and sections without matches fade.
//!
//! Keyboard: Ctrl+F / Ctrl+K focus the search; Alt+↑/↓ (or
//! Ctrl+PgUp/PgDn) step through the sections from anywhere. With the
//! sidebar focused (click it, or Ctrl+L): ↑/↓ step, Home/End jump,
//! ←/→ (mirrored in RTL) collapse or expand the current group, Enter
//! toggles it.

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;
use std::collections::HashMap;

use chrono::Timelike as _;
use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::{ShellConfig, ThemeMode};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::compositor_page::{badge, button, card};
use super::themes_page::{apply_theme, section_head, shell_mock};
use super::{StudioView, begin_move, grid};
use crate::registry::{self, Control, NavGroup, Section, Setting};
use crate::tokens::*;

/// The active section's label is lifted off its container (darker in light mode).
const NAV_ACTIVE_LIGHTEN_DARK: f32 = 0.1;
const NAV_ACTIVE_LIGHTEN_LIGHT: f32 = -0.05;
/// The chevron points down when a group is open (a quarter turn).
const CHEVRON_OPEN_RAD: f32 = std::f32::consts::FRAC_PI_2;
/// …and towards the reading direction's end when collapsed (RTL: a half turn).
const CHEVRON_RTL_RAD: f32 = std::f32::consts::PI;
/// Leading accent bar on the active row while the sidebar has focus.
const FOCUS_BAR_W_PX: f32 = 3.0;
const FOCUS_BAR_H_PX: f32 = 14.0;
/// Angle of the Home hero's fallback gradient (no wallpaper) and of the
/// group chips' and theme swatches' fills.
const HERO_GRADIENT_DEG: f32 = 135.0;
/// The hero's accent wash runs from the reading start (opaque, under the
/// text) to the end (the wallpaper showing through).
const HERO_WASH_DEG: f32 = 90.0;
const HERO_WASH_RTL_DEG: f32 = 270.0;
const HERO_MIN_H_PX: f32 = 230.0;
const HERO_GREETING_PX: f32 = 34.0;
/// Greeting by local hour: morning from 5, afternoon from 12, evening from 18.
const MORNING_FROM_H: u32 = 5;
const AFTERNOON_FROM_H: u32 = 12;
const EVENING_FROM_H: u32 = 18;
/// The bento: two widgets per row from this column width (else stacked),
/// the wider one taking this share of the row.
const BENTO_SPLIT_MIN_W_PX: f32 = 720.0;
const BENTO_WIDE_SHARE: f32 = 0.58;
const WIDGET_PAD_PX: f32 = SPACE_18_PX;
/// Shell status: the CPU chart's samples (one a second, a minute kept),
/// its least height, the shortest bar as a share of it and the gap; the
/// scale never drops below this many percent.
const SPARK_BARS: usize = crate::perf::HISTORY;
const SPARK_H_PX: f32 = 56.0;
const SPARK_MIN_FRAC: f32 = 0.06;
const SPARK_GAP_PX: f32 = 2.0;
const CPU_SPARK_FLOOR_PERCENT: f32 = 5.0;
/// Theme widget: the mock's height per unit of width, and how many theme
/// swatches it offers.
const LOOK_MOCK_ASPECT: f32 = 0.5;
const HOME_THEME_SWATCHES: usize = 8;
/// Quick settings tiles.
const QUICK_TILE_GRID: CardGrid = CardGrid { min_w: 170.0, max_w: 400.0, gap: SPACE_8_PX };
const QUICK_TILE_MIN_H_PX: f32 = 60.0;
const QUICK_LOCKED_OPACITY: f32 = 0.45;
/// A tile's hover lifts its fill by this much lightness (darkens in light mode).
const QUICK_HOVER_LIFT: f32 = 0.04;
/// Recent changes listed, and saved profiles named.
const HOME_RECENT_SHOWN: usize = 4;
const HOME_PROFILES_SHOWN: usize = 3;
/// Emphasis of the active row, group headers and Home card titles: medium,
/// since the Arabic UI font has no semibold cut (synthesized bold smears
/// small Arabic text).
const NAV_STRONG: FontWeight = FontWeight::MEDIUM;
/// The settings Home offers directly (config paths in the registry).
pub(super) const QUICK_SETTINGS: &[&str] = &["/theme/mode", "/bar/layout", "/night_light/enabled", "/motion/reduce_motion", "/theme/adjustments/high_contrast"];

/// Matching settings and pages per section for a search query.
fn search_hits(query: &str) -> HashMap<Section, usize> {
    let mut hits: HashMap<Section, usize> = HashMap::new();
    for s in registry::settings().iter().filter(|s| s.matches(query)) {
        *hits.entry(s.section).or_default() += 1;
    }
    for (_, section) in super::form::matching_pages(&query.trim().to_lowercase()) {
        hits.entry(section).or_insert(1);
    }
    hits
}

impl StudioView {
    /// Collapses or expands a sidebar group and remembers it.
    pub(super) fn toggle_group(&mut self, group: NavGroup, cx: &mut Context<Self>) {
        self.ui.toggle(group);
        crate::ui_state::save(&self.ui);
        cx.notify();
    }

    fn set_group_collapsed(&mut self, group: NavGroup, collapsed: bool, cx: &mut Context<Self>) {
        if self.ui.is_collapsed(group) != collapsed {
            self.ui.set_collapsed(group, collapsed);
            crate::ui_state::save(&self.ui);
            cx.notify();
        }
    }

    /// Moves `delta` sections along the sidebar order (wrapping).
    fn step_section(&mut self, delta: isize, cx: &mut Context<Self>) {
        let order: Vec<Section> = registry::nav_order().collect();
        let at = order.iter().position(|s| *s == self.section).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(order.len() as isize) as usize;
        self.show(order[next], cx);
        self.nav_scroll.scroll_to_item(self.nav_row_index(order[next]));
    }

    /// Index of `section`'s row among the scroller's children (for
    /// scroll-into-view): the title block, the search, Home, then a header
    /// per group followed by its visible rows.
    fn nav_row_index(&self, section: Section) -> usize {
        // Title block, search field, Home.
        const LEADING_ROWS: usize = 3;
        let mut i = LEADING_ROWS;
        if section == Section::Home {
            return i - 1;
        }
        for g in NavGroup::ALL {
            i += 1;
            for s in g.sections() {
                if !self.ui.is_collapsed(g) || *s == self.section || *s == section {
                    if *s == section {
                        return i;
                    }
                    i += 1;
                }
            }
        }
        i
    }

    /// Studio-wide keys (bubbled from wherever focus is); returns whether
    /// the key was used.
    pub(super) fn nav_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        // A shortcut being recorded gets every key.
        if self.capturing.is_some() {
            return false;
        }
        let k = &e.keystroke;
        let m = k.modifiers;
        let rtl = mandara_i18n::language().is_rtl();
        let nav_focused = self.nav_focus.is_focused(window);
        match k.key.as_str() {
            "f" | "k" if m.control && !m.alt && !m.shift => {
                self.search.update(cx, |f, cx| f.focus(window, cx));
            }
            "l" if m.control && !m.alt && !m.shift => self.nav_focus.focus(window, cx),
            "down" if m.alt => self.step_section(1, cx),
            "up" if m.alt => self.step_section(-1, cx),
            "pagedown" if m.control => self.step_section(1, cx),
            "pageup" if m.control => self.step_section(-1, cx),
            // Tab walks the page's tab stops (cards and chips that set a
            // `tab_index`, such as Themes → Cursor's gallery).
            "tab" if !m.control && !m.alt && !m.platform => {
                if m.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
            }
            _ if !nav_focused || m.control || m.alt || m.platform => return false,
            "down" => self.step_section(1, cx),
            "up" => self.step_section(-1, cx),
            "home" => self.show(Section::Home, cx),
            "end" => {
                let last = registry::nav_order().last().unwrap_or(Section::Home);
                self.show(last, cx);
                self.nav_scroll.scroll_to_item(self.nav_row_index(last));
            }
            "left" | "right" | "enter" | "space" => {
                let Some(group) = self.section.group() else { return false };
                let collapse = match k.key.as_str() {
                    "enter" | "space" => !self.ui.is_collapsed(group),
                    "left" => !rtl,
                    _ => rtl,
                };
                self.set_group_collapsed(group, collapse, cx);
            }
            "escape" => window.blur(cx),
            _ => return false,
        }
        true
    }

    fn nav_item(&self, section: Section, rail: bool, hits: Option<&HashMap<Section, usize>>, focused: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let active = section == self.section && self.query.is_empty();
        let rtl = mandara_i18n::language().is_rtl();
        let count = hits.map(|h| h.get(&section).copied().unwrap_or(0));
        div()
            .id(SharedString::from(format!("nav-{}", section.title())))
            .relative()
            .h(px(NAV_ITEM_H_PX))
            .flex_none()
            .when(rail, |el| el.w(px(DIM_44_PX)).justify_center())
            .when(!rail, |el| el.px(px(SPACE_10_PX)))
            .rounded(px(RADIUS_9_PX))
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .cursor_pointer()
            .when(active, |el| el.bg(p.primary_container.hsla()).text_color(p.primary.lighten(if p.is_dark { NAV_ACTIVE_LIGHTEN_DARK } else { NAV_ACTIVE_LIGHTEN_LIGHT }).hsla()))
            .when(!active, |el| el.hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_6).hsla())))
            .when(count == Some(0), |el| el.opacity(NAV_NO_MATCH_ALPHA))
            // Keyboard focus: a short accent bar at the row's leading edge.
            .when(active && focused, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px((NAV_ITEM_H_PX - FOCUS_BAR_H_PX) / 2.0))
                        .when(rtl, |el| el.right_0())
                        .when(!rtl, |el| el.left_0())
                        .w(px(FOCUS_BAR_W_PX))
                        .h(px(FOCUS_BAR_H_PX))
                        .rounded_full()
                        .bg(p.primary.hsla()),
                )
            })
            .child(icon(section.icon(), NAV_ICON_PX, if active { p.primary } else { p.on_surface_variant }))
            .when(!rail, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .when(rtl, |el| el.text_right())
                        .font_weight(if active { NAV_STRONG } else { FontWeight::NORMAL })
                        .child(section.label()),
                )
                .when_some(count.filter(|n| *n > 0), |el, n| {
                    el.child(div().flex_none().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.primary.hsla()).child(n.to_string()))
                })
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.nav_focus.focus(window, cx);
                this.show(section, cx);
            }))
            .into_any_element()
    }

    fn nav_header(&self, group: NavGroup, open: bool, searching: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rtl = mandara_i18n::language().is_rtl();
        let turn = if open { CHEVRON_OPEN_RAD } else if rtl { CHEVRON_RTL_RAD } else { 0.0 };
        div()
            .id(SharedString::from(format!("nav-group-{}", group.id())))
            .mt(px(NAV_GROUP_GAP_PX))
            .h(px(NAV_HEADER_H_PX))
            .flex_none()
            .px(px(SPACE_10_PX))
            .rounded(px(RADIUS_8_PX))
            .row()
            .items_center()
            .gap(px(SPACE_6_PX))
            .text_size(px(TEXT_11_5_PX))
            .font_weight(NAV_STRONG)
            .text_color(p.on_surface_variant.hsla())
            .when(!searching, |el| el.cursor_pointer().hover(|s| s.text_color(p.on_surface.hsla())))
            .child(div().flex_1().min_w(px(0.0)).truncate().when(rtl, |el| el.text_right()).child(tr(group.title())))
            .when(!searching, |el| {
                el.child(gpui::svg().path("icons/chevron-right.svg").size(px(NAV_CHEVRON_PX)).flex_none().text_color(p.on_surface_variant.hsla()).with_transformation(gpui::Transformation::rotate(gpui::radians(turn))))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_group(group, cx)))
            })
            .into_any_element()
    }

    /// The whole sidebar: title (drags the window), search, Home and the
    /// groups.
    pub(super) fn nav(&mut self, rail: bool, window: &mut Window, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let rtl = mandara_i18n::language().is_rtl();
        let focused = self.nav_focus.is_focused(window);
        let searching = !self.query.is_empty();
        let hits = searching.then(|| search_hits(&self.query));
        let mut nav = div()
            .id("studio-nav")
            .track_focus(&self.nav_focus)
            .track_scroll(&self.nav_scroll)
            .w(px(if rail { RAIL_W_PX } else { NAV_W_PX }))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(p.surface_container_low.hsla())
            .px(px(SPACE_12_PX))
            .py(px(SPACE_14_PX))
            .row()
            .flex_col()
            .when(rail, |el| el.items_center())
            .gap(px(NAV_ITEM_GAP_PX))
            .child(
                div()
                    .id("studio-drag")
                    .flex_none()
                    .when(!rail, |el| el.px(px(SPACE_8_PX)))
                    .pt(px(SPACE_6_PX))
                    .pb(px(SPACE_14_PX))
                    .row()
                    .flex_col()
                    .when(rtl, |el| el.items_end())
                    .on_mouse_down(MouseButton::Left, |e, window, _| if e.click_count >= 2 { window.zoom_window() } else { begin_move(window) })
                    .when(rail, |el| el.child(icon("grid", ICON_22_PX, p.primary)))
                    .when(!rail, |el| {
                        el.child(div().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Mandara Studio")))
                            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Every setting, live")))
                    }),
            )
            // The rail moves the search into the page header; keep the
            // child count stable for scroll-into-view.
            .child(div().flex_none().when(!rail, |el| el.pb(px(SPACE_10_PX)).child(self.search.clone())))
            .child(self.nav_item(Section::Home, rail, hits.as_ref(), focused, p, cx));
        for group in NavGroup::ALL {
            let open = searching || rail || !self.ui.is_collapsed(group);
            nav = nav.child(if rail { div().h(px(NAV_GROUP_GAP_PX)).flex_none().into_any_element() } else { self.nav_header(group, open, searching, p, cx) });
            for section in group.sections().iter().copied() {
                // A collapsed group still shows the page you're on.
                if open || section == self.section {
                    nav = nav.child(self.nav_item(section, rail, hits.as_ref(), focused, p, cx));
                }
            }
        }
        nav.into_any_element()
    }

    // ── Home ────────────────────────────────────────────────────────────

    /// Home: a greeting over the wallpaper, a bento of live widgets (shell
    /// status, the theme with a one-click switch, quick settings, recent
    /// changes and profiles), then a card per sidebar group.
    pub(super) fn home_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Sample the shell only while Home is on screen and the window can
        // be seen.
        let this = cx.entity().downgrade();
        let handle = window.window_handle();
        crate::perf::poll_while(
            &self.perf,
            move |cx| this.read_with(cx, |v, _| v.section == Section::Home && v.query.is_empty()).unwrap_or(false) && cx.update_window(handle, |_, window, _| window.is_visible()).unwrap_or(false),
            cx,
        );
        let fonts = mandara_ui::fonts::fonts(cx);
        let column = self.page.column_w;
        let split = column >= BENTO_SPLIT_MIN_W_PX;
        let wide_w = if split { ((column - BENTO_GAP_PX) * BENTO_WIDE_SHARE).floor() } else { column };
        let narrow_w = if split { column - BENTO_GAP_PX - wide_w } else { column };
        let hero = self.home_hero(cfg, p, cx);
        let status = self.home_status(cfg, p, cx);
        let look = self.home_look(cfg, narrow_w - 2.0 * WIDGET_PAD_PX, p, cx);
        let recent = self.home_recent(p, cx);
        let quick = self.home_quick(cfg, wide_w - 2.0 * WIDGET_PAD_PX, p, cx);
        let bento = div().row().flex_col().gap(px(BENTO_GAP_PX)).child(bento_row(split, [(status, wide_w), (look, narrow_w)])).child(bento_row(split, [(recent, narrow_w), (quick, wide_w)]));
        let fit = grid::fit(column, HOME_GROUP_GRID).balanced(NavGroup::ALL.len());
        let cards: Vec<AnyElement> = NavGroup::ALL.into_iter().map(|g| self.home_group_card(g, p, cx)).collect();
        let browse = div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Browse settings"), t("Every page of Studio, by group"), Vec::new(), fonts.display.clone(), p))
            .child(fit.render(cards));
        div().row().flex_col().gap(px(PAGE_SECTION_GAP_PX)).child(div().row().flex_col().gap(px(BENTO_GAP_PX)).child(hero).child(bento)).child(browse).into_any_element()
    }

    /// The greeting: the wallpaper under an accent wash, the date, the
    /// theme and compositor, and the most used shortcuts.
    fn home_hero(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let rtl = mandara_i18n::language().is_rtl();
        let thumb = cfg.wallpaper.path.clone().and_then(|path| self.wallpaper_assets(&path, cx).0);
        let now = chrono::Local::now();
        let greeting = match now.hour() {
            h if (MORNING_FROM_H..AFTERNOON_FROM_H).contains(&h) => t("Good morning"),
            h if (AFTERNOON_FROM_H..EVENING_FROM_H).contains(&h) => t("Good afternoon"),
            _ => t("Good evening"),
        };
        let theme = mandara_core::theme_entity(cx).read(cx).target().name.clone();
        let mode = if cfg.theme.mode.is_dark() { t("Dark") } else { t("Light") };
        let compositor = mandara_compositor::detect().map(|k| format!("{k:?}")).unwrap_or_else(|| t("no compositor detected").into());
        // Legible text on the reading-start side, the wallpaper showing
        // through towards the end.
        let wash = mandara_core::oklab_gradient(if rtl { HERO_WASH_RTL_DEG } else { HERO_WASH_DEG }, p.surface_base.with_alpha(ALPHA_88), p.primary.with_alpha(ALPHA_15));
        let glass = |el: gpui::Div| el.bg(p.surface_base.with_alpha(ALPHA_55).hsla()).rounded_full();
        div()
            .relative()
            .overflow_hidden()
            .min_h(px(HERO_MIN_H_PX))
            .rounded(px(RADIUS_26_PX))
            .bg(mandara_core::oklab_gradient(HERO_GRADIENT_DEG, p.primary_container, p.tertiary.with_alpha(ALPHA_35)))
            // Sized explicitly: an image only covers a box whose size it
            // doesn't have to infer.
            .when_some(thumb, |el, thumb| el.child(img(thumb).absolute().top_0().left_0().w(px(self.page.column_w)).h_full().object_fit(gpui::ObjectFit::Cover)))
            .child(div().absolute().inset_0().bg(wash))
            .child(
                div()
                    .relative()
                    .p(px(SPACE_28_PX))
                    .row()
                    .flex_col()
                    .cross_start()
                    .gap(px(SPACE_6_PX))
                    .child(div().text_size(px(TEXT_13_PX)).font_weight(NAV_STRONG).text_color(p.primary.hsla()).child(mandara_ui::widgets::clock::long_date(&now)))
                    .child(div().font_family(fonts.display.clone()).text_size(px(HERO_GREETING_PX)).font_weight(FontWeight::SEMIBOLD).child(greeting))
                    .child(div().text_size(px(TEXT_13_PX)).text_color(p.on_surface_variant.hsla()).child(format!("{theme} · {}", tf("{mode} theme · {compositor}", &[("mode", &mode), ("compositor", &compositor)]))))
                    .child(
                        div()
                            .pt(px(SPACE_14_PX))
                            .row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(button("home-themes", "droplet", t("Change theme"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Themes, cx))))
                            .child(button("home-wallpaper", "image", t("Wallpaper"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Wallpaper, cx))))
                            .child(button("home-plugins", "zap", t("Plugins"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Plugins, cx))))
                            .child(
                                glass(div())
                                    .h(px(DIM_34_PX))
                                    .px(px(SPACE_12_PX))
                                    .row()
                                    .items_center()
                                    .gap(px(SPACE_6_PX))
                                    .text_size(px(TEXT_12_PX))
                                    .font_features(mandara_ui::fonts::tabular_nums())
                                    .child(icon("note", ICON_13_PX, p.on_surface_variant))
                                    .child(format!("{} {}", t("Mandara"), mandara_ui::bidi::ltr_isolate(&mandara_core::build_info(cx).version))),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Shell status: CPU (with its last minute), memory and video memory,
    /// sampled once a second while Home shows.
    fn home_status(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (usage, history) = {
            let monitor = self.perf.read(cx);
            let history: Vec<f32> = monitor.shell.iter().skip(monitor.shell.len().saturating_sub(SPARK_BARS)).map(|u| u.cpu).collect();
            (monitor.latest(), history)
        };
        let compositor = mandara_compositor::detect().map(|k| format!("{k:?}")).unwrap_or_else(|| t("no compositor detected").into());
        let (on, total) = cx.try_global::<PluginRegistry>().map(|r| (r.manifests().filter(|m| r.is_enabled(&m.id, &cfg.plugins)).count(), r.manifests().count())).unwrap_or_default();
        let state = match usage {
            Some(_) => badge(t("Running"), p.success_emerald),
            None => badge(t("Not running"), p.urgent_coral),
        };
        let details = div()
            .id("home-performance")
            .cursor_pointer()
            .size(px(DIM_30_PX))
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
            .child(gpui::svg().path("icons/chevron-right.svg").size(px(ICON_14_PX)).text_color(p.on_surface_variant.hsla()).when(mandara_i18n::language().is_rtl(), |el| el.with_transformation(gpui::Transformation::rotate(gpui::radians(CHEVRON_RTL_RAD)))))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Performance, cx)));
        let head = widget_head("gauge", t("Shell status"), vec![state.into_any_element(), details.into_any_element()], p);
        let metric = |label: String, value: String, tone: mandara_theme::Color| {
            div()
                .flex_1()
                .min_w(px(0.0))
                .p(px(SPACE_12_PX))
                .rounded(px(RADIUS_16_PX))
                .bg(p.surface_container_high.with_alpha(ALPHA_60).hsla())
                .row()
                .flex_col()
                .cross_start()
                .gap(px(SPACE_4_PX))
                .child(div().row().items_center().gap(px(SPACE_6_PX)).child(div().size(px(DIM_6_PX)).rounded_full().bg(tone.hsla())).child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(label)))
                .child(div().text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(mandara_ui::bidi::ltr_isolate(&value)))
        };
        let body = match usage {
            Some(u) => {
                // The last minute of CPU, oldest first, padded with empty
                // slots so the bars keep their width from the first second.
                let peak = history.iter().copied().fold(CPU_SPARK_FLOOR_PERCENT, f32::max);
                let empty = SPARK_BARS.saturating_sub(history.len());
                let bars = std::iter::repeat_n(None, empty).chain(history.iter().map(|v| Some(*v))).map(|v| {
                    let frac = v.map_or(0.0, |v| (v / peak).clamp(0.0, 1.0));
                    div().flex_1().h(gpui::relative(SPARK_MIN_FRAC + (1.0 - SPARK_MIN_FRAC) * frac)).rounded_full().bg(if v.is_some() { p.primary.with_alpha(ALPHA_70).hsla() } else { p.on_surface.with_alpha(ALPHA_8).hsla() })
                });
                let chart = div()
                    .flex_1()
                    .min_h(px(SPARK_H_PX))
                    .p(px(SPACE_12_PX))
                    .rounded(px(RADIUS_16_PX))
                    .bg(p.surface_container_high.with_alpha(ALPHA_60).hsla())
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("CPU graph")))
                    .child(div().flex_1().w_full().ltr_row().items_end().gap(px(SPARK_GAP_PX)).children(bars));
                let vram = u.vram_mib.map(|v| format!("{v:.0} MB")).unwrap_or_else(|| t("n/a").into());
                div()
                    .flex_1()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_14_PX))
                    .child(
                        div()
                            .row()
                            .flex_wrap()
                            .gap(px(SPACE_10_PX))
                            .child(metric(t("CPU").into(), format!("{:.1}%", u.cpu), p.primary))
                            .child(metric(t("Memory").into(), format!("{:.0} MB", u.rss_mib), p.tertiary))
                            .child(metric(t("Video memory").into(), vram, p.secondary)),
                    )
                    .child(chart)
                    .into_any_element()
            }
            None => div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Start it with `mandara`; its CPU and memory appear here live."))).into_any_element(),
        };
        let foot = format!("{compositor} · {}", tf("{on} of {total} plugins on", &[("on", &on), ("total", &total)]));
        widget(p).child(head).child(body).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(foot))).into_any_element()
    }

    /// The theme: a live mock in the current palette, a light / dark flip
    /// and one-click swatches of the designed themes.
    fn home_look(&mut self, cfg: &ShellConfig, inner_w: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let dark = cfg.theme.mode.is_dark();
        let theme = mandara_core::theme_entity(cx).read(cx).target().name.clone();
        let thumb = cfg.wallpaper.path.clone().and_then(|path| self.wallpaper_assets(&path, cx).0);
        let active = match &cfg.theme.source {
            mandara_theme::ThemeSource::Preset { preset } => Some(preset.clone()),
            _ => None,
        };
        let source = if active.is_some() { t("Designed theme") } else { t("From your wallpaper") };
        let flip = div()
            .id("home-mode")
            .cursor_pointer()
            .size(px(DIM_30_PX))
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .bg(p.surface_container_high.hsla())
            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
            .child(icon(if dark { "sun" } else { "moon" }, ICON_14_PX, p.primary))
            .on_click(move |_: &ClickEvent, _, cx| update_config(cx, move |c| c.theme.mode = if dark { ThemeMode::Light } else { ThemeMode::Dark }));
        let head = widget_head("droplet", t("Theme"), vec![flip.into_any_element()], p);
        let mock_w = inner_w.max(0.0);
        let mock = shell_mock(p, thumb, mock_w, (mock_w * LOOK_MOCK_ASPECT).floor(), RADIUS_14_PX, true);
        let gallery = self.theme_gallery(dark);
        let swatches = gallery.themes_iter().take(HOME_THEME_SWATCHES).map(|(def, tp)| {
            let on = active.as_deref() == Some(def.id.as_str());
            let id = def.id.clone();
            div()
                .id(SharedString::from(format!("home-theme-{}", def.id)))
                .size(px(DIM_28_PX))
                .flex_none()
                .rounded_full()
                .cursor_pointer()
                .border_2()
                .border_color(if on { p.on_surface.hsla() } else { tp.surface_base.hsla() })
                .bg(mandara_core::oklab_gradient(HERO_GRADIENT_DEG, tp.primary, tp.tertiary))
                .hover(|s| s.border_color(p.on_surface.with_alpha(ALPHA_50).hsla()))
                .on_click(move |_: &ClickEvent, _, cx| apply_theme(id.clone(), cx))
        });
        let all = div()
            .id("home-all-themes")
            .cursor_pointer()
            .h(px(DIM_28_PX))
            .px(px(SPACE_10_PX))
            .rounded_full()
            .row()
            .items_center()
            .text_size(px(TEXT_12_PX))
            .text_color(p.primary.hsla())
            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
            .child(t("All themes"))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Themes, cx)));
        widget(p)
            .child(head)
            .child(mock)
            .child(
                div()
                    .row()
                    .flex_col()
                    .cross_start()
                    .gap(px(SPACE_3_PX))
                    .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_17_PX)).font_weight(FontWeight::SEMIBOLD).child(theme.to_string()))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(format!("{} · {source}", if dark { t("Dark") } else { t("Light") }))),
            )
            .child(div().row().flex_wrap().items_center().gap(px(SPACE_6_PX)).children(swatches).child(all))
            .into_any_element()
    }

    /// Quick settings: choices as rows, switches as Control Center tiles
    /// (the whole tile flips it).
    fn home_quick(&mut self, cfg: &ShellConfig, inner_w: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let head = widget_head("sliders", t("Quick settings"), Vec::new(), p);
        let settings: Vec<Setting> = QUICK_SETTINGS.iter().filter_map(|path| registry::by_path(path)).cloned().collect();
        let mut choices = div().row().flex_col().gap(px(SPACE_10_PX));
        let mut tiles = Vec::new();
        for s in &settings {
            if s.control == Control::Toggle {
                tiles.push(self.quick_tile(s, cfg, p, cx));
                continue;
            }
            let control = self.control(s, cfg, p, cx);
            choices = choices.child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(SPACE_10_PX))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(icon(quick_icon(s.path), ICON_15_PX, p.on_surface_variant)).child(div().text_size(px(TEXT_13_PX)).font_weight(NAV_STRONG).child(tr(s.label))))
                    .child(control),
            );
        }
        let fit = grid::fit(inner_w, QUICK_TILE_GRID).balanced(tiles.len());
        widget(p).child(head).child(choices).child(fit.render(tiles)).into_any_element()
    }

    fn quick_tile(&mut self, s: &Setting, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let on = registry::get(cfg, s).as_bool().unwrap_or(false);
        let locked = registry::locked(s, cfg);
        let setting = s.clone();
        let (bg, chip_bg, glyph) = if on { (p.primary_container, p.primary, p.on_primary) } else { (p.surface_container_high, p.surface_container, p.on_surface_variant) };
        let state: String = match locked {
            Some(reason) => tr(reason),
            None if on => t("On").into(),
            None => t("Off").into(),
        };
        div()
            .id(SharedString::from(format!("quick{}", s.path)))
            .flex_1()
            .min_h(px(QUICK_TILE_MIN_H_PX))
            .p(px(SPACE_12_PX))
            .rounded(px(RADIUS_18_PX))
            .bg(bg.hsla())
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .when(locked.is_some(), |el| el.opacity(QUICK_LOCKED_OPACITY))
            .when(locked.is_none(), |el| {
                el.cursor_pointer().hover(|st| st.bg(bg.lighten(if p.is_dark { QUICK_HOVER_LIFT } else { -QUICK_HOVER_LIFT }).hsla())).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply(&setting, serde_json::Value::from(!on), cx)))
            })
            .child(div().size(px(DIM_32_PX)).flex_none().rounded_full().bg(chip_bg.hsla()).row().items_center().justify_center().child(icon(quick_icon(s.path), ICON_15_PX, glyph)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .cross_start()
                    .child(div().w_full().truncate().text_start().text_size(px(TEXT_13_PX)).font_weight(NAV_STRONG).child(tr(s.label)))
                    .child(div().w_full().truncate().text_start().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(state)),
            )
            .into_any_element()
    }

    /// Recent changes (newest first, with Undo) and the saved profiles.
    fn home_recent(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (changes, total) = super::profiles_page::recent_changes(HOME_RECENT_SHOWN, cx);
        let names = super::profiles_page::profile_names(cx);
        let undo = (total > 0).then(|| button("home-undo", "undo", t("Undo"), false, p).on_click(|_, _, cx| super::profiles_page::undo_last(cx)).into_any_element());
        let head = widget_head("clock", t("Recent changes"), undo.into_iter().collect(), p);
        let list = if changes.is_empty() {
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_3_PX))
                .child(div().text_size(px(TEXT_13_PX)).font_weight(NAV_STRONG).child(t("No changes yet")))
                .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Edits you make in Studio appear here, newest first, with an Undo."))))
                .into_any_element()
        } else {
            div()
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .children(changes.into_iter().enumerate().map(|(i, (what, ago))| {
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(div().size(px(DIM_8_PX)).flex_none().rounded_full().bg(if i == 0 { p.primary.hsla() } else { p.on_surface.with_alpha(ALPHA_25).hsla() }))
                        .child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_12_5_PX)).child(what))
                        .child(div().flex_none().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(ago))
                }))
                .into_any_element()
        };
        let profiles = names.iter().take(HOME_PROFILES_SHOWN).enumerate().map(|(i, name)| {
            div()
                .id(("home-profile", i))
                .cursor_pointer()
                .h(px(DIM_28_PX))
                .px(px(SPACE_10_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .bg(p.surface_container_high.hsla())
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
                .text_size(px(TEXT_12_PX))
                .child(icon("user", ICON_12_PX, p.primary))
                .child(name.clone())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Profiles, cx)))
        });
        let shortcut = button("home-profiles", Section::Profiles.icon(), Section::Profiles.label(), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::Profiles, cx)));
        widget(p)
            .child(head)
            .child(div().flex_1().child(list))
            .child(
                div()
                    .row()
                    .flex_col()
                    .cross_start()
                    .gap(px(SPACE_8_PX))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(NAV_STRONG).text_color(p.on_surface_variant.hsla()).child(t("Saved profiles"))).child(badge(names.len().to_string(), p.primary).font_features(mandara_ui::fonts::tabular_nums())))
                    .child(div().row().flex_wrap().items_center().gap(px(SPACE_6_PX)).children(profiles).child(shortcut)),
            )
            .into_any_element()
    }

    /// A sidebar group: its icon, title and summary (opens its first page)
    /// and a pill per page.
    fn home_group_card(&mut self, group: NavGroup, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let first = group.sections().first().copied().unwrap_or(Section::Home);
        let links = group.sections().iter().copied().map(|section| {
            div()
                .id(SharedString::from(format!("home-{}", section.title())))
                .h(px(HOME_LINK_H_PX))
                .px(px(SPACE_10_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .cursor_pointer()
                .bg(p.surface_container_high.with_alpha(ALPHA_70).hsla())
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla()))
                .text_size(px(TEXT_12_PX))
                .child(icon(section.icon(), ICON_13_PX, p.on_surface_variant))
                .child(section.label())
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show(section, cx)))
        });
        card(p)
            .flex_1()
            .gap(px(SPACE_14_PX))
            .child(
                div()
                    .id(SharedString::from(format!("home-group-{}", group.id())))
                    .cursor_pointer()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(
                        div()
                            .size(px(HOME_CHIP_PX))
                            .flex_none()
                            .rounded(px(RADIUS_14_PX))
                            .bg(mandara_core::oklab_gradient(HERO_GRADIENT_DEG, p.primary_container, p.tertiary.with_alpha(ALPHA_35)))
                            .row()
                            .items_center()
                            .justify_center()
                            .child(icon(group.icon(), ICON_17_PX, p.primary)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .cross_start()
                            .child(div().text_size(px(TEXT_14_5_PX)).font_weight(NAV_STRONG).child(mandara_ui::bidi::paragraph(tr(group.title()))))
                            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(group.blurb())))),
                    )
                    .child(badge(tf("{count} pages", &[("count", &group.sections().len())]), p.primary))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.show(first, cx))),
            )
            .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(links))
            .into_any_element()
    }
}

/// A Home widget card.
fn widget(p: &MandaraPalette) -> gpui::Div {
    div().flex_1().clip_rounded().rounded(px(RADIUS_26_PX)).bg(p.surface_container.hsla()).p(px(WIDGET_PAD_PX)).row().flex_col().gap(px(SPACE_14_PX))
}

/// A widget's title row: an icon chip, the title and controls at the end.
fn widget_head(glyph: &'static str, title: impl Into<SharedString>, trailing: Vec<AnyElement>, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .items_center()
        .gap(px(SPACE_10_PX))
        .child(div().size(px(DIM_30_PX)).flex_none().rounded(px(RADIUS_10_PX)).bg(p.primary_container.hsla()).row().items_center().justify_center().child(icon(glyph, ICON_15_PX, p.primary)))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_14_PX)).font_weight(NAV_STRONG).child(title.into()))
        .children(trailing)
        .into_any_element()
}

/// Two widgets side by side at their widths, or stacked when narrow.
fn bento_row(split: bool, items: [(AnyElement, f32); 2]) -> AnyElement {
    let mut row = div().row().gap(px(BENTO_GAP_PX)).when(!split, |el| el.flex_col());
    for (item, w) in items {
        row = row.child(div().when(split, |el| el.w(px(w))).flex_none().min_w(px(0.0)).row().flex_col().child(item));
    }
    row.into_any_element()
}

/// The icon of a quick setting.
fn quick_icon(path: &str) -> &'static str {
    match path {
        "/theme/mode" => "moon",
        "/bar/layout" => "window",
        "/night_light/enabled" => "sunset",
        "/motion/reduce_motion" => "waves",
        "/theme/adjustments/high_contrast" => "eye",
        _ => "sliders",
    }
}
