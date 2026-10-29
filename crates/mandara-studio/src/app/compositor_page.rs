//! Studio → Compositor: the adaptive compositor control center.
//!
//! * The running compositor (MangoWM, Niri or Hyprland) is detected and its
//!   tab opens first; the others stay editable (their drop-ins are written
//!   whenever that compositor runs).
//! * **Window rules**: a visual builder over `compositor.window_rules` with
//!   tri-state option chips, per-compositor support dots, and "Pick running
//!   window" (the shell's `compositor clients`, with app icons).
//! * **Tag & Layout Studio** (MangoWM): a 3×3 tag grid with layout glyphs
//!   editing `compositor.tag_rules`, plus SceneFX tuning.
//! * **Columns & outputs** (Niri) and **Decoration** (Hyprland).
//!
//! Everything writes the config; the shell's compositor sync regenerates
//! `~/.config/mandara/generated/<compositor>` and reloads — the user's own
//! compositor files are never edited here.

use mandara_ui::bidi::Bidi as _;
use mandara_ui::style::ClipRounded as _;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px,
};
use mandara_compositor::{ClientInfo, MonitorInfo};
use mandara_config::ShellConfig;
use mandara_config::compositor::{
    BorderColors, CompositorConfig, ManagedConfig, OutputRule, RULE_OPTIONS, TagRule, Target, WindowRule, emit, include_installed, rule_option_supported,
};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::StudioView;
use crate::registry::{self, Section, Setting};
use crate::tokens::*;

// ── Small shared building blocks (also used by the audio / diagnostics pages) ──

/// A hover tooltip bubble.
pub(super) struct Tip {
    text: SharedString,
    bg: gpui::Hsla,
    fg: gpui::Hsla,
}

impl gpui::Render for Tip {
    fn render(&mut self, _: &mut gpui::Window, _: &mut Context<Self>) -> impl IntoElement {
        div().max_w(px(DIM_420_PX)).px(px(SPACE_10_PX)).py(px(SPACE_6_PX)).rounded(px(RADIUS_9_PX)).bg(self.bg).text_color(self.fg).text_size(px(TEXT_12_PX)).child(self.text.clone())
    }
}

/// Builds a [`Tip`] for `.tooltip(…)`.
pub(super) fn tip(text: impl Into<SharedString>, p: &MandaraPalette) -> impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let (text, bg, fg) = (text.into(), p.surface_container_high.hsla(), p.on_surface.hsla());
    move |_, cx| gpui::AppContext::new(cx, |_| Tip { text: text.clone(), bg, fg }).into()
}

/// A settings card. Children (hover fills, bands, previews) are clipped to
/// its rounded corners.
pub(super) fn card(p: &MandaraPalette) -> Div {
    div().clip_rounded().rounded(px(RADIUS_20_PX)).bg(p.surface_container.hsla()).p(px(SPACE_18_PX)).row().flex_col().gap(px(SPACE_12_PX))
}

pub(super) fn group_title(text: impl Into<SharedString>, p: &MandaraPalette) -> Div {
    div().px(px(SPACE_10_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(text.into())
}

pub(super) fn heading(title: impl Into<SharedString>, detail: impl Into<SharedString>, p: &MandaraPalette) -> Div {
    div()
        .row()
        .flex_col()
        .gap(px(SPACE_3_PX))
        .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
        .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(detail)))
}

pub(super) fn badge(text: impl Into<SharedString>, color: Color) -> Div {
    div().px(px(SPACE_9_PX)).h(px(DIM_22_PX)).rounded_full().row().items_center().flex_none().bg(color.with_alpha(ALPHA_16).hsla()).text_color(color.hsla()).text_size(px(TEXT_11_PX)).child(text.into())
}

pub(super) fn button(id: impl Into<SharedString>, icon_name: &'static str, label: impl Into<SharedString>, filled: bool, p: &MandaraPalette) -> Stateful<Div> {
    let (bg, fg) = if filled { (p.primary, p.on_primary) } else { (p.surface_container_high, p.on_surface) };
    div()
        .id(id.into())
        .h(px(DIM_34_PX))
        .px(px(SPACE_14_PX))
        .rounded_full()
        .row()
        .flex_none()
        .items_center()
        .gap(px(SPACE_7_PX))
        .cursor_pointer()
        .bg(bg.hsla())
        .text_color(fg.hsla())
        .text_size(px(TEXT_12_5_PX))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.opacity(ALPHA_88))
        .child(icon(icon_name, ICON_14_PX, if filled { p.on_primary } else { p.primary }))
        .child(label.into())
}

pub(super) fn chip(id: impl Into<SharedString>, label: impl Into<SharedString>, active: bool, p: &MandaraPalette) -> Stateful<Div> {
    div()
        .id(id.into())
        .h(px(DIM_30_PX))
        .px(px(SPACE_12_PX))
        .rounded(px(RADIUS_10_PX))
        .row()
        .flex_none()
        .items_center()
        .gap(px(SPACE_6_PX))
        .cursor_pointer()
        .text_size(px(TEXT_12_5_PX))
        .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).font_weight(FontWeight::MEDIUM))
        .when(!active, |el| el.bg(p.surface_container_high.hsla()).text_color(p.on_surface_variant.hsla()).hover(|s| s.bg(p.primary.with_alpha(ALPHA_12).hsla())))
        .child(label.into())
}

/// An inline slider for an optional number: "Off" chip when unset.
#[allow(clippy::too_many_arguments)]
pub(super) fn opt_slider(
    id: SharedString,
    value: Option<f32>,
    (min, max, step): (f32, f32, f32),
    default: f32,
    format: fn(f32) -> String,
    unset: &'static str,
    apply: Rc<dyn Fn(&mut ShellConfig, Option<f32>)>,
    p: &MandaraPalette,
) -> AnyElement {
    match value {
        None => {
            let apply = apply.clone();
            chip(SharedString::from(format!("{id}-set")), tr(unset), false, p)
                .child(icon("plus", ICON_11_PX, p.primary))
                .on_click(move |_, _, cx| {
                    let apply = apply.clone();
                    update_config(cx, move |c| apply(c, Some(default)));
                })
                .into_any_element()
        }
        Some(v) => {
            let (slide, reset) = (apply.clone(), apply);
            div()
                .row()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().w(px(DIM_150_PX)).child(mandara_ui::kit::slider(id.clone(), ((v - min) / (max - min)).clamp(0.0, 1.0), p, move |f, _, cx| {
                    let raw = min + (max - min) * f;
                    let snapped = ((raw / step).round() * step).clamp(min, max);
                    let slide = slide.clone();
                    update_config(cx, move |c| slide(c, Some(snapped)));
                })))
                .child(div().w(px(DIM_44_PX)).text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(format(v)))
                .child(
                    div()
                        .id(SharedString::from(format!("{id}-reset")))
                        .size(px(DIM_24_PX))
                        .rounded_full()
                        .row()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                        .child(icon("close", ICON_11_PX, p.on_surface_variant))
                        .on_click(move |_, _, cx| {
                            let reset = reset.clone();
                            update_config(cx, move |c| reset(c, None));
                        }),
                )
                .into_any_element()
        }
    }
}

pub(super) fn percent(v: f32) -> String {
    format!("{:.0}%", v * PERCENT)
}

// ── Page state ──────────────────────────────────────────────────────────────

/// Clients for the window picker: `None` while loading.
type Loaded<T> = Option<Result<Vec<T>, String>>;

#[derive(Default)]
struct CompositorStudio {
    /// Selected compositor tab (`None` = the running one).
    tab: Option<Target>,
    /// Window picker: the rule it fills (`None` = a new rule) and its list.
    picker: Option<(Option<usize>, Loaded<ClientInfo>)>,
    /// Tag & Layout Studio selection (1..=9).
    tag: u32,
    monitors: Loaded<MonitorInfo>,
    monitors_at: Option<Instant>,
    /// Drop-in preview expanded.
    preview: bool,
    /// The previewed drop-in text and what it was generated from.
    preview_text: Option<(Target, CompositorConfig, Option<BorderColors>, SharedString)>,
    /// Cached "include line installed" per target.
    included: Option<(Instant, [bool; Target::ALL.len()])>,
    /// Open the window picker on first show (`…_TAB=<tab>,picker`).
    auto_picker: bool,
}
impl Global for CompositorStudio {}

/// Percent per whole.
const PERCENT: f32 = 100.0;
/// MangoWM tags (1…9).
const TAGS: u32 = 9;
/// Tag & Layout Studio: the board and chip grids are `TAG_GRID` and
/// `LAYOUT_CHIP_GRID` (tokens.rs). Layout sketch inside a layout chip:
const LAYOUT_CHIP_GLYPH_W_PX: f32 = 58.0;
const LAYOUT_CHIP_GLYPH_H_PX: f32 = 32.0;
/// Sketches wider than this use the large "default" label.
const GLYPH_LARGE_W_PX: f32 = 70.0;
/// Gap between the windows of a layout sketch.
const GLYPH_GAP_PX: f32 = 1.5;
/// Opacity of a sketch's secondary windows (a deck's stacked one shows more).
const GLYPH_TILE_ALPHA: f32 = 0.32;
const GLYPH_DECK_ALPHA: f32 = 0.55;
/// App icons in a rule card and in the window picker.
const RULE_APP_ICON_PX: f32 = 30.0;
const PICKER_APP_ICON_PX: f32 = 28.0;
/// Outputs for the monitor pickers are re-read this often while shown.
const MONITORS_REFRESH: Duration = Duration::from_secs(10);
/// "Include line installed" is re-read from the user's config this often.
const INCLUDE_REFRESH: Duration = Duration::from_secs(3);
/// Column widths closer than this are the same preset.
const SAME_WIDTH_EPSILON: f32 = 0.01;

fn running() -> Option<Target> {
    match mandara_compositor::detect() {
        Some(mandara_compositor::CompositorKind::MangoWM) => Some(Target::Mango),
        Some(mandara_compositor::CompositorKind::Niri) => Some(Target::Niri),
        Some(mandara_compositor::CompositorKind::Hyprland) => Some(Target::Hyprland),
        None => None,
    }
}

pub(super) fn target_name(t: Target) -> &'static str {
    match t {
        Target::Mango => "MangoWM",
        Target::Niri => "Niri",
        Target::Hyprland => "Hyprland",
    }
}

fn target_index(t: Target) -> usize {
    match t {
        Target::Mango => 0,
        Target::Niri => 1,
        Target::Hyprland => 2,
    }
}

/// `^…$` around `s` with regex metacharacters escaped.
pub fn exact_regex(s: &str) -> String {
    let mut out = String::from("^");
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('$');
    out
}

/// A cheap sanity check (no regex engine in Studio): unbalanced groups or a
/// dangling escape.
pub fn regex_problem(s: &str) -> Option<&'static str> {
    let mut depth = (0i32, 0i32);
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.next().is_none() => return Some(tk("ends with a lone backslash")),
            '(' => depth.0 += 1,
            ')' => depth.0 -= 1,
            '[' => depth.1 += 1,
            ']' => depth.1 -= 1,
            _ => {}
        }
        if depth.0 < 0 || depth.1 < 0 {
            return Some(tk("closes a group it never opened"));
        }
    }
    (depth != (0, 0)).then_some(tk("has an unclosed group"))
}

/// Display name + English help of a window-rule option.
fn option_label(name: &str) -> (&'static str, &'static str) {
    match name {
        "floating" => (tk("Floating"), tk("Open as a floating window")),
        "global" => (tk("Global"), tk("Visible on every tag / workspace (pinned)")),
        "overlay" => (tk("Overlay"), tk("Always above other windows")),
        "focused_opacity" => (tk("Focused opacity"), tk("Opacity while focused")),
        "unfocused_opacity" => (tk("Unfocused opacity"), tk("Opacity while another window has focus")),
        "no_blur" => (tk("No blur"), tk("Never blur behind this window")),
        "no_shadow" => (tk("No shadow"), tk("Draw no drop shadow")),
        "confine_pointer" => (tk("Confine pointer"), tk("Keep the pointer inside the window (games)")),
        "force_tiled_state" => (tk("Force tiled state"), tk("Tell the app it is tiled, even when floating")),
        _ => ("", ""),
    }
}

fn rule_flag<'a>(r: &'a mut WindowRule, name: &str) -> Option<&'a mut Option<bool>> {
    Some(match name {
        "floating" => &mut r.floating,
        "global" => &mut r.global,
        "overlay" => &mut r.overlay,
        "no_blur" => &mut r.no_blur,
        "no_shadow" => &mut r.no_shadow,
        "confine_pointer" => &mut r.confine_pointer,
        "force_tiled_state" => &mut r.force_tiled_state,
        _ => return None,
    })
}

fn rule_opacity<'a>(r: &'a mut WindowRule, name: &str) -> Option<&'a mut Option<f32>> {
    Some(match name {
        "focused_opacity" => &mut r.focused_opacity,
        "unfocused_opacity" => &mut r.unfocused_opacity,
        _ => return None,
    })
}

/// MangoWM layouts: (config name, label).
pub const MANGO_LAYOUTS: &[(&str, &str)] = &[
    ("tile", tk("Master-Stack")),
    ("scroller", tk("Scroller")),
    ("monocle", tk("Monocle")),
    ("grid", tk("Grid")),
    ("deck", tk("Deck")),
    ("center_tile", tk("Center tile")),
    ("right_tile", tk("Right tile")),
    ("dwindle", tk("Dwindle")),
    ("vertical_tile", tk("Vertical tile")),
    ("vertical_scroller", tk("Vertical scroller")),
    ("vertical_grid", tk("Vertical grid")),
    ("vertical_deck", tk("Vertical deck")),
];

/// Window rectangles (fractions of the screen) that sketch a layout.
pub fn layout_sketch(layout: &str) -> &'static [(f32, f32, f32, f32)] {
    match layout {
        "tile" => &[(0.0, 0.0, 0.56, 1.0), (0.56, 0.0, 0.44, 0.5), (0.56, 0.5, 0.44, 0.5)],
        "right_tile" => &[(0.44, 0.0, 0.56, 1.0), (0.0, 0.0, 0.44, 0.5), (0.0, 0.5, 0.44, 0.5)],
        "center_tile" => &[(0.24, 0.0, 0.52, 1.0), (0.0, 0.0, 0.24, 1.0), (0.76, 0.0, 0.24, 1.0)],
        "scroller" => &[(0.2, 0.0, 0.6, 1.0), (0.0, 0.0, 0.17, 1.0), (0.83, 0.0, 0.17, 1.0)],
        "vertical_scroller" => &[(0.0, 0.2, 1.0, 0.6), (0.0, 0.0, 1.0, 0.17), (0.0, 0.83, 1.0, 0.17)],
        "monocle" => &[(0.0, 0.0, 1.0, 1.0)],
        "grid" | "vertical_grid" => &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5)],
        "deck" => &[(0.0, 0.0, 0.56, 1.0), (0.62, 0.0, 0.38, 1.0), (0.56, 0.08, 0.38, 0.92)],
        "vertical_deck" => &[(0.0, 0.0, 1.0, 0.56), (0.0, 0.62, 1.0, 0.38), (0.08, 0.56, 0.92, 0.38)],
        "vertical_tile" => &[(0.0, 0.0, 1.0, 0.56), (0.0, 0.56, 0.5, 0.44), (0.5, 0.56, 0.5, 0.44)],
        "dwindle" => &[(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 0.5, 0.5), (0.5, 0.5, 0.25, 0.5), (0.75, 0.5, 0.25, 0.5)],
        _ => &[],
    }
}

/// A miniature drawing of a layout: rounded window tiles, the first (the
/// master / focused one) in the accent color.
pub(super) fn layout_glyph(layout: Option<&str>, w: f32, h: f32, accent: Color, p: &MandaraPalette) -> Div {
    let frame = div().relative().w(px(w)).h(px(h)).flex_none().rounded(px(RADIUS_5_PX)).bg(p.surface_dim.hsla());
    let Some(layout) = layout else {
        // No rule: an empty frame that says it keeps the default.
        return frame.border_1().border_color(p.outline.with_alpha(ALPHA_25).hsla()).row().items_center().justify_center().child(div().text_size(px(if w > GLYPH_LARGE_W_PX { TEXT_18_PX } else { TEXT_13_PX })).text_color(p.on_surface_variant.with_alpha(ALPHA_45).hsla()).child("·  ·  ·"));
    };
    let gap = GLYPH_GAP_PX;
    frame.children(layout_sketch(layout).iter().enumerate().map(|(i, (x, y, rw, rh))| {
        let c = if i == 0 { accent } else { p.on_surface_variant.with_alpha(if layout.contains("deck") && i == 2 { GLYPH_DECK_ALPHA } else { GLYPH_TILE_ALPHA }) };
        div()
            .absolute()
            .left(px(x * w + gap))
            .top(px(y * h + gap))
            .w(px((rw * w - 2.0 * gap).max(1.0)))
            .h(px((rh * h - 2.0 * gap).max(1.0)))
            .rounded(px(RADIUS_3_PX))
            .bg(c.hsla())
    }))
}

impl StudioView {
    fn comp_state<'a>(cx: &'a mut Context<Self>) -> &'a mut CompositorStudio {
        if !cx.has_global::<CompositorStudio>() {
            // `MANDARA_STUDIO_COMPOSITOR_TAB=niri` opens another tab first
            // (screenshots, layout testing).
            let env = std::env::var("MANDARA_STUDIO_COMPOSITOR_TAB").unwrap_or_default();
            let auto_picker = env.ends_with(",picker");
            let tab = Some(env.trim_end_matches(",picker")).and_then(|v| match v {
                "mango" => Some(Target::Mango),
                "niri" => Some(Target::Niri),
                "hyprland" => Some(Target::Hyprland),
                _ => None,
            });
            cx.set_global(CompositorStudio { tag: 1, tab, auto_picker, ..Default::default() });
        }
        cx.global_mut::<CompositorStudio>()
    }

    /// Loads the outputs (for monitor pickers) every 10 s while shown.
    fn refresh_monitors(cx: &mut Context<Self>) {
        let st = Self::comp_state(cx);
        if st.monitors_at.is_some_and(|t| t.elapsed() < MONITORS_REFRESH) {
            return;
        }
        st.monitors_at = Some(Instant::now());
        let Some(task) = crate::perf::query(&["compositor", "monitors"], cx) else { return };
        cx.spawn(async move |this, cx| {
            let result = task.await.and_then(|v| serde_json::from_value::<Vec<MonitorInfo>>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |_, cx| {
                Self::comp_state(cx).monitors = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_picker(&mut self, rule: Option<usize>, cx: &mut Context<Self>) {
        Self::comp_state(cx).picker = Some((rule, None));
        cx.notify();
        let Some(task) = crate::perf::query(&["compositor", "clients"], cx) else {
            Self::comp_state(cx).picker = Some((rule, Some(Err(t("Studio isn't connected to the shell").into()))));
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = task.await.and_then(|v| serde_json::from_value::<Vec<ClientInfo>>(v).map_err(|e| e.to_string()));
            let _ = this.update(cx, |_, cx| {
                if let Some((r, list)) = Self::comp_state(cx).picker.as_mut()
                    && *r == rule
                {
                    *list = Some(result);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn compositor_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        Self::refresh_monitors(cx);
        if std::mem::take(&mut Self::comp_state(cx).auto_picker) {
            self.open_picker(None, cx);
        }
        let live = running();
        let tab = Self::comp_state(cx).tab.or(live).unwrap_or(Target::Mango);
        let all = registry::settings();
        let group = |g: &str| -> Vec<&'static Setting> { all.iter().filter(|s| s.section == Section::Compositor && s.group == g).collect() };

        // Include-line status (cached: it reads the user's config files).
        let included = {
            let st = Self::comp_state(cx);
            match st.included {
                Some((at, v)) if at.elapsed() < INCLUDE_REFRESH => v,
                _ => {
                    let v = Target::ALL.map(include_installed);
                    st.included = Some((Instant::now(), v));
                    v
                }
            }
        };

        // Compositor tabs.
        let tabs = div().row().flex_wrap().gap(px(SPACE_8_PX)).children(Target::ALL.map(|target| {
            let active = target == tab;
            let is_live = live == Some(target);
            div()
                .id(SharedString::from(format!("comp-tab-{}", target_name(target))))
                .h(px(DIM_40_PX))
                .px(px(SPACE_16_PX))
                .rounded(px(RADIUS_14_PX))
                .row()
                .items_center()
                .gap(px(SPACE_8_PX))
                .cursor_pointer()
                .when(active, |el| el.bg(p.primary_container.hsla()).border_1().border_color(p.primary.with_alpha(ALPHA_50).hsla()))
                .when(!active, |el| el.bg(p.surface_container.hsla()).hover(|s| s.bg(p.surface_container_high.hsla())))
                .child(div().font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }).child(target_name(target)))
                .when(is_live, |el| el.child(badge(t("Running"), p.success_emerald)))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    Self::comp_state(cx).tab = Some(target);
                    cx.notify();
                }))
        }));
        let file = target_file(tab);
        let status = if live == Some(tab) {
            if included[target_index(tab)] {
                tf("Writing {file} and reloading {compositor} on every change.", &[("file", &file), ("compositor", &target_name(tab))])
            } else {
                tf("{file} is written, but your config doesn't include it yet — Maintenance → Compositor adds the one include line.", &[("file", &file)])
            }
        } else {
            tf("Saved now; {file} is generated the next time Mandara runs under {compositor}.", &[("file", &file), ("compositor", &target_name(tab))])
        };
        let header = div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(tabs)
            .child(div().min_w(px(0.0)).overflow_hidden().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(status));

        let common = {
            let mut list = group("Integration");
            list.extend(group("Window look"));
            list.extend(group("Effects"));
            self.rows(list, cfg, p, cx)
        };
        let rules = self.window_rules(cfg, tab, p, cx);
        let specific = match tab {
            Target::Mango => {
                let tags = self.tag_studio(cfg, p, cx);
                let fx = self.rows(group("MangoWM"), cfg, p, cx);
                div().row().flex_col().gap(px(SPACE_26_PX)).child(tags).child(fx).into_any_element()
            }
            Target::Niri => {
                let rows = self.rows(group("Niri"), cfg, p, cx);
                let cols = self.niri_studio(cfg, p, cx);
                div().row().flex_col().gap(px(SPACE_26_PX)).child(rows).child(cols).into_any_element()
            }
            Target::Hyprland => {
                let mut list = group("Hyprland");
                list.extend(group(crate::registry::system_settings::HYPR_LAYOUT));
                self.rows(list, cfg, p, cx)
            }
        };
        let preview = self.dropin_preview(cfg, tab, p, cx);
        let presets = self.comp_presets(cfg, p, cx);
        div().row().flex_col().gap(px(SPACE_26_PX)).child(header).child(presets).child(specific).child(rules).child(common).child(preview).into_any_element()
    }

    fn dropin_preview(&mut self, cfg: &ShellConfig, tab: Target, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let open = Self::comp_state(cx).preview;
        let toggle = button("dropin-preview", "window", if open { t("Hide generated file") } else { t("Preview generated file") }, false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
            let st = Self::comp_state(cx);
            st.preview = !st.preview;
            cx.notify();
        }));
        let mut col = div().row().flex_col().gap(px(SPACE_10_PX)).child(div().row().items_center().gap(px(SPACE_10_PX)).child(toggle).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(target_file(tab))));
        if open {
            // Generated once per config change, not per frame.
            // What the shell writes: gaps that follow the bar and the
            // palette's border colours included.
            let effective = cfg.effective_compositor().into_owned();
            let borders = cfg.integrations.compositor_borders.then(|| preview_borders(p));
            let cached = Self::comp_state(cx).preview_text.as_ref().filter(|(t, c, b, _)| *t == tab && *c == effective && *b == borders).map(|(.., text)| text.clone());
            let text = cached.unwrap_or_else(|| {
                let text = SharedString::from(preview_text(tab, &effective, borders.clone()));
                Self::comp_state(cx).preview_text = Some((tab, effective, borders, text.clone()));
                text
            });
            col = col.child(
                div()
                    .id("dropin-text")
                    .max_h(px(DIM_320_PX))
                    .overflow_y_scroll()
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_dim.hsla())
                    .p(px(SPACE_14_PX))
                    .font_family(mandara_ui::fonts::fonts(cx).mono.clone())
                    .text_size(px(TEXT_11_5_PX))
                    .line_height(px(LEADING_16_PX))
                    .text_color(p.on_surface_variant.hsla())
                    // One text element: GPUI lays out each line itself.
                    .child(text),
            );
        }
        col.into_any_element()
    }

    // ── Window rules ────────────────────────────────────────────────────────

    fn window_rules(&mut self, cfg: &ShellConfig, tab: Target, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rules = cfg.compositor.window_rules.clone();
        let mut list = div().row().flex_col().gap(px(SPACE_10_PX));
        if rules.is_empty() {
            list = list.child(div().p(px(SPACE_14_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("No window rules yet. Add one, or pick a running window to start from it."))));
        }
        for (i, rule) in rules.iter().enumerate() {
            list = list.child(self.rule_card(i, rule, tab, p, cx));
        }
        let actions = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_8_PX))
            .child(button("rule-pick-new", "crosshair", t("Pick running window"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.open_picker(None, cx))))
            .child(button("rule-add", "plus", t("Add rule"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.list_fields.retain(|k, _| !k.starts_with("wr-"));
                update_config(cx, |c| c.compositor.window_rules.push(WindowRule { floating: Some(true), ..Default::default() }));
            })));
        let picker = self.picker(p, cx);
        let legend = div().row().flex_wrap().items_center().gap(px(SPACE_12_PX)).text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Supported by:")).children(Target::ALL.map(|target| {
            div().row().items_center().gap(px(SPACE_5_PX)).child(support_dot(target, true, target == tab, p)).child(target_name(target))
        }));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Window rules"), p))
            .child(
                card(p)
                    .child(heading(
                        t("Visual window rule builder"),
                        t("Match windows by app id or title (regular expressions) and choose what happens to them. Options a compositor can't do are dimmed for it and left out of its file."),
                        p,
                    ))
                    .child(legend)
                    .child(list)
                    .child(actions)
                    .when_some(picker, |el, picker| el.child(picker)),
            )
            .into_any_element()
    }

    fn rule_card(&mut self, i: usize, rule: &WindowRule, tab: Target, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let field = |this: &mut Self, key: &str, current: &str, placeholder: &'static str, title: bool, cx: &mut Context<Self>| {
            this.list_field(
                format!("wr-{i}-{key}"),
                current,
                placeholder,
                Rc::new(move |c: &mut ShellConfig, v: String| {
                    if let Some(r) = c.compositor.window_rules.get_mut(i) {
                        let v = v.trim().to_string();
                        let slot = if title { &mut r.title } else { &mut r.app_id };
                        *slot = (!v.is_empty()).then_some(v);
                    }
                }),
                cx,
            )
        };
        let app_field = field(self, "app", rule.app_id.as_deref().unwrap_or_default(), t("App id, e.g. ^pavucontrol$"), false, cx);
        let title_field = field(self, "title", rule.title.as_deref().unwrap_or_default(), t("Title, e.g. Picture-in-Picture"), true, cx);
        let problem = [rule.app_id.as_deref(), rule.title.as_deref()].into_iter().flatten().find_map(regex_problem);
        let matches_nothing = rule.app_id.is_none() && rule.title.is_none();
        // A plain `^id$` app id shows that app's icon.
        let literal_id = rule.app_id.as_deref().and_then(|a| a.strip_prefix('^')).and_then(|a| a.strip_suffix('$')).map(|a| a.replace('\\', ""));
        let icon_el = match literal_id.as_deref() {
            Some(id) if !id.is_empty() => mandara_ui::plugins::workspaces::app_icon(id, RULE_APP_ICON_PX, p.on_surface, false, p),
            _ => div().size(px(DIM_30_PX)).rounded(px(RADIUS_9_PX)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_12).hsla()).child(icon("window", ICON_15_PX, p.primary)).into_any_element(),
        };
        let fields = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(icon_el)
            .child(div().flex_1().min_w(px(DIM_180_PX)).child(app_field))
            .child(div().flex_1().min_w(px(DIM_180_PX)).child(title_field))
            .child(
                div()
                    .id(("rule-pick", i))
                    .size(px(DIM_34_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(p.primary.with_alpha(ALPHA_12).hsla())
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_22).hsla()))
                    .child(icon("crosshair", ICON_15_PX, p.primary))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open_picker(Some(i), cx))),
            )
            .child(
                div()
                    .id(("rule-del", i))
                    .size(px(DIM_34_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                    .child(icon("trash", ICON_15_PX, p.on_surface_variant))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.list_fields.retain(|k, _| !k.starts_with("wr-"));
                        update_config(cx, |c| {
                            if i < c.compositor.window_rules.len() {
                                c.compositor.window_rules.remove(i);
                            }
                        });
                    })),
            );
        let mut options = div().row().flex_wrap().gap(px(SPACE_8_PX));
        for (name, support) in RULE_OPTIONS {
            let name: &'static str = name;
            let supported_here = rule_option_supported(tab, name);
            let (label, help) = option_label(name);
            let dots = div().row().gap(px(SPACE_3_PX)).children(Target::ALL.map(|target| support_dot(target, support[target_index(target)], target == tab, p)));
            let mut r = rule.clone();
            let el = if let Some(flag) = rule_flag(&mut r, name).map(|f| *f) {
                // Tri-state: unset → on → off → unset.
                let (state, color) = match flag {
                    None => (t("Unset"), p.on_surface_variant),
                    Some(true) => (t("On"), p.success_emerald),
                    Some(false) => (t("Off"), p.urgent_coral),
                };
                div()
                    .id(SharedString::from(format!("rule-{i}-{name}")))
                    .h(px(DIM_50_PX))
                    .min_w(px(DIM_138_PX))
                    .px(px(SPACE_12_PX))
                    .rounded(px(RADIUS_12_PX))
                    .row()
                    .flex_col()
                    .justify_center()
                    .gap(px(SPACE_4_PX))
                    .cursor_pointer()
                    .bg(if flag.is_some() { color.with_alpha(ALPHA_12).hsla() } else { p.surface_container_high.hsla() })
                    .border_1()
                    .border_color(if flag.is_some() { color.with_alpha(ALPHA_45).hsla() } else { gpui::transparent_black() })
                    .when(!supported_here, |el| el.opacity(ALPHA_45))
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(label)))).child(div().text_size(px(TEXT_11_PX)).text_color(color.hsla()).child(state)))
                    .child(dots)
                    .tooltip(tip(tr(help), p))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        update_config(cx, |c| {
                            if let Some(slot) = c.compositor.window_rules.get_mut(i).and_then(|r| rule_flag(r, name)) {
                                *slot = match *slot {
                                    None => Some(true),
                                    Some(true) => Some(false),
                                    Some(false) => None,
                                };
                            }
                        });
                    }))
                    .into_any_element()
            } else {
                let value = rule_opacity(&mut r, name).and_then(|v| *v);
                let slider = opt_slider(
                    SharedString::from(format!("rule-{i}-{name}")),
                    value,
                    (0.3, 1.0, 0.01),
                    0.9,
                    percent,
                    tk("Unset"),
                    Rc::new(move |c: &mut ShellConfig, v| {
                        if let Some(slot) = c.compositor.window_rules.get_mut(i).and_then(|r| rule_opacity(r, name)) {
                            *slot = v;
                        }
                    }),
                    p,
                );
                div()
                    .px(px(SPACE_12_PX))
                    .py(px(SPACE_6_PX))
                    .rounded(px(RADIUS_12_PX))
                    .bg(p.surface_container_high.hsla())
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .when(!supported_here, |el| el.opacity(ALPHA_45))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(mandara_ui::bidi::paragraph(tr(label)))).child(dots))
                    .child(slider)
                    .into_any_element()
            };
            options = options.child(el);
        }
        div()
            .p(px(SPACE_12_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_container_low.hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(fields)
            .when(matches_nothing, |el| el.child(badge(t("Matches nothing yet — set an app id or a title"), p.warning_amber)))
            .when_some(problem, |el, why| el.child(badge(tf("This pattern {problem}", &[("problem", &tr(why))]), p.urgent_coral)))
            .child(options)
            .into_any_element()
    }

    fn picker(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (rule, list) = Self::comp_state(cx).picker.clone()?;
        let close = div()
            .id("picker-close")
            .size(px(DIM_30_PX))
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
            .child(icon("close", ICON_13_PX, p.on_surface_variant))
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                Self::comp_state(cx).picker = None;
                cx.notify();
            }));
        let body = match list {
            None => div().p(px(SPACE_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Asking the compositor for its windows…")).into_any_element(),
            Some(Err(e)) => div().p(px(SPACE_12_PX)).text_color(p.urgent_coral.hsla()).child(tf("Couldn't list windows: {error}", &[("error", &e)])).into_any_element(),
            Some(Ok(mut clients)) => {
                // Studio itself last, then by app.
                clients.sort_by_key(|c| (c.app_id == mandara_core::STUDIO_APP_ID, c.app_id.to_lowercase(), c.title.clone()));
                let mut col = div().id("picker-list").max_h(px(DIM_360_PX)).overflow_y_scroll().row().flex_col().gap(px(SPACE_4_PX));
                for (n, c) in clients.into_iter().enumerate() {
                    let name = mandara_ui::apps::name_for_app_id(&c.app_id).unwrap_or_else(|| c.app_id.clone());
                    let (app_id, title) = (c.app_id.clone(), c.title.clone());
                    let (a2, t2) = (app_id.clone(), title.clone());
                    col = col.child(
                        div()
                            .id(("picker-row", n))
                            .px(px(SPACE_10_PX))
                            .py(px(SPACE_8_PX))
                            .rounded(px(RADIUS_12_PX))
                            .row()
                            .items_center()
                            .gap(px(SPACE_12_PX))
                            .cursor_pointer()
                            .hover(|s| s.bg(p.primary.with_alpha(ALPHA_8).hsla()))
                            .child(mandara_ui::plugins::workspaces::app_icon(&c.app_id, PICKER_APP_ICON_PX, p.on_surface, false, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .row()
                                    .flex_col()
                                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(div().font_weight(FontWeight::MEDIUM).child(name)).child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(c.app_id.clone())))
                                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).overflow_hidden().text_ellipsis().whitespace_nowrap().child(c.title.clone())),
                            )
                            .when(c.floating, |el| el.child(badge(t("Floating"), p.tertiary)))
                            .child(
                                chip(SharedString::from(format!("picker-title-{n}")), t("App + title"), false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.apply_pick(rule, &a2, Some(&t2), cx);
                                })),
                            )
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.apply_pick(rule, &app_id, None, cx))),
                    );
                }
                col.into_any_element()
            }
        };
        Some(
            div()
                .mt(px(SPACE_4_PX))
                .p(px(SPACE_14_PX))
                .rounded(px(RADIUS_18_PX))
                .bg(p.surface_container_high.hsla())
                .border_1()
                .border_color(p.primary.with_alpha(ALPHA_35).hsla())
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(icon("crosshair", ICON_16_PX, p.primary))
                        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(if rule.is_some() { t("Pick a window for this rule") } else { t("Pick a window to make a rule for") }))
                        .child(close),
                )
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Click a window to match its app; “App + title” matches that exact title too."))))
                .child(body)
                .into_any_element(),
        )
    }

    fn apply_pick(&mut self, rule: Option<usize>, app_id: &str, title: Option<&str>, cx: &mut Context<Self>) {
        let app = exact_regex(app_id);
        let title = title.filter(|t| !t.is_empty()).map(exact_regex);
        self.list_fields.retain(|k, _| !k.starts_with("wr-"));
        update_config(cx, move |c| match rule.and_then(|i| c.compositor.window_rules.get_mut(i)) {
            Some(r) => {
                r.app_id = Some(app);
                if title.is_some() {
                    r.title = title;
                }
            }
            None => c.compositor.window_rules.push(WindowRule { app_id: Some(app), title, floating: Some(true), ..Default::default() }),
        });
        Self::comp_state(cx).picker = None;
        cx.notify();
    }

    // ── Tag & Layout Studio (MangoWM) ───────────────────────────────────────

    fn tag_studio(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let selected = Self::comp_state(cx).tag.clamp(1, TAGS);
        let rule_for = |c: &CompositorConfig, id: u32| c.tag_rules.iter().find(|r| r.id == id).cloned();
        // The board (3 × 3) beside its editor when both fit, else stacked.
        let avail = self.page.column_w - 2.0 * SECTION_CARD_PAD_PX;
        let board_min = TAG_GRID_COLS as f32 * TAG_GRID.min_w + (TAG_GRID_COLS - 1) as f32 * TAG_GRID.gap;
        let side_by_side = avail >= board_min + TAG_PANE_GAP_PX + TAG_EDITOR_MIN_W_PX;
        let board_avail = if side_by_side { avail - TAG_PANE_GAP_PX - TAG_EDITOR_MIN_W_PX } else { avail };
        let board = super::grid::fit(board_avail, TAG_GRID).at_most(TAG_GRID_COLS);
        let editor_w = if side_by_side { avail - TAG_PANE_GAP_PX - board.width } else { avail };
        let glyph_w = (board.card_w - 2.0 * (SPACE_10_PX + BORDER_1_PX)).max(0.0);
        let mut tiles: Vec<AnyElement> = Vec::with_capacity(TAGS as usize);
        for id in 1..=TAGS {
            let rule = rule_for(&cfg.compositor, id);
            let active = id == selected;
            let layout = rule.as_ref().and_then(|r| r.layout.clone());
            let label = layout.as_deref().map(layout_label).map(tr).unwrap_or_else(|| t("Default").to_string());
            let mut facts: Vec<String> = Vec::new();
            if let Some(r) = &rule {
                if let Some(m) = r.master_factor {
                    facts.push(format!("{:.0}%", m * 100.0));
                }
                if let Some(s) = r.scroller_default_proportion {
                    facts.push(format!("↔{:.0}%", s * 100.0));
                }
                if let Some(m) = &r.monitor {
                    facts.push(m.clone());
                }
            }
            tiles.push(
                div()
                    .id(("tag-tile", id as usize))
                    .flex_1()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_16_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .bg(if active { p.primary_container.hsla() } else { p.surface_container_high.hsla() })
                    .border_1()
                    .border_color(if active { p.primary.with_alpha(ALPHA_60).hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(div().text_size(px(TEXT_18_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).text_color(if active { p.primary.hsla() } else { p.on_surface.hsla() }).child(id.to_string()))
                            .child(div().flex_1().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label)),
                    )
                    .child(layout_glyph(layout.as_deref(), glyph_w, (glyph_w * TAG_GLYPH_ASPECT).floor(), p.primary, p))
                    .child(div().h(px(DIM_14_PX)).truncate().text_size(px(TEXT_10_5_PX)).text_color(p.on_surface_variant.hsla()).child(facts.join(" · ")))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        Self::comp_state(cx).tag = id;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let grid = div().flex_none().w(px(board.width)).max_w_full().child(board.render(tiles));
        let editor = self.tag_editor(selected, rule_for(&cfg.compositor, selected), editor_w, p, cx);
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Tag & layout studio"), p))
            .child(
                card(p)
                    .child(heading(t("Per-tag layouts"), t("Pick a tag, then its layout, master share and scroller width. Tags without a rule keep MangoWM's default."), p))
                    .child(
                        div()
                            .row()
                            .when(!side_by_side, |el| el.flex_col())
                            .items_start()
                            .gap(px(TAG_PANE_GAP_PX))
                            .child(grid)
                            .child(div().flex_1().min_w(px(0.0)).when(!side_by_side, |el| el.w_full()).child(editor)),
                    ),
            )
            .into_any_element()
    }

    fn tag_editor(&mut self, id: u32, rule: Option<TagRule>, width: f32, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let edit = move |f: Box<dyn Fn(&mut TagRule)>| {
            let f: Rc<dyn Fn(&mut TagRule)> = f.into();
            move |c: &mut ShellConfig| {
                let rules = &mut c.compositor.tag_rules;
                let i = match rules.iter().position(|r| r.id == id) {
                    Some(i) => i,
                    None => {
                        rules.push(TagRule { id, ..Default::default() });
                        rules.sort_by_key(|r| r.id);
                        rules.iter().position(|r| r.id == id).unwrap_or(0)
                    }
                };
                f(&mut rules[i]);
                // A rule with nothing set is no rule.
                if rules[i] == (TagRule { id, ..Default::default() }) {
                    rules.remove(i);
                }
            }
        };
        let current = rule.clone().unwrap_or(TagRule { id, ..Default::default() });
        // The twelve layouts as an even grid; "MangoWM's default" (no rule)
        // is a chip in the heading, so no row ends with a lone sketch.
        let default_chip = {
            let apply = edit(Box::new(|r: &mut TagRule| r.layout = None));
            chip(SharedString::from(format!("tag-{id}-layout-default")), t("Default"), current.layout.is_none(), p).on_click(move |_, _, cx| update_config(cx, &apply))
        };
        let chips: Vec<_> = MANGO_LAYOUTS.iter().map(|(n, l)| (Some(*n), *l)).enumerate().map(|(n, (layout, label))| {
                let n = n + 1;
                let active = current.layout.as_deref() == layout;
                let apply = edit(Box::new(move |r: &mut TagRule| r.layout = layout.map(str::to_string)));
                div()
                    .id(("tag-layout", n))
                    .p(px(SPACE_6_PX))
                    .rounded(px(RADIUS_12_PX))
                    .row()
                    .flex_col()
                    .items_center()
                    .gap(px(SPACE_4_PX))
                    .flex_1()
                    .cursor_pointer()
                    .bg(if active { p.primary.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() })
                    .border_1()
                    .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                    .child(layout_glyph(layout, LAYOUT_CHIP_GLYPH_W_PX, LAYOUT_CHIP_GLYPH_H_PX, if active { p.primary } else { p.on_surface_variant.with_alpha(ALPHA_70) }, p))
                    .child(div().text_size(px(TEXT_10_5_PX)).text_color(if active { p.primary.hsla() } else { p.on_surface_variant.hsla() }).text_center().child(mandara_ui::bidi::paragraph(tr(label))))
                    .on_click(move |_, _, cx| update_config(cx, &apply))
            }).collect();
        let layouts = super::grid::fit(width, LAYOUT_CHIP_GRID).balanced(chips.len()).render(chips);
        let row = |label: String, control: AnyElement| {
            div().row().items_center().justify_between().gap(px(SPACE_12_PX)).min_h(px(DIM_36_PX)).child(div().text_size(px(TEXT_12_5_PX)).child(label)).child(control)
        };
        let mfact = {
            opt_slider(
                SharedString::from(format!("tag-{id}-mfact")),
                current.master_factor,
                (0.1, 0.9, 0.05),
                0.55,
                percent,
                tk("Default"),
                Rc::new(move |c: &mut ShellConfig, v| edit(Box::new(move |r: &mut TagRule| r.master_factor = v))(c)),
                p,
            )
        };
        let proportion = opt_slider(
            SharedString::from(format!("tag-{id}-prop")),
            current.scroller_default_proportion,
            (0.2, 1.0, 0.05),
            0.8,
            percent,
            tk("Default"),
            Rc::new(move |c: &mut ShellConfig, v| edit(Box::new(move |r: &mut TagRule| r.scroller_default_proportion = v))(c)),
            p,
        );
        let count = current.master_count;
        let step = |label: &'static str, delta: i32| {
            let apply = edit(Box::new(move |r: &mut TagRule| {
                let next = r.master_count.unwrap_or(1) as i32 + delta;
                r.master_count = if next < 1 || (delta < 0 && r.master_count.is_none()) { None } else { Some(next as u32) };
            }));
            div()
                .id(SharedString::from(format!("tag-{id}-count{label}")))
                .size(px(DIM_28_PX))
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(p.surface_container_high.hsla())
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_14).hsla()))
                .child(label)
                .on_click(move |_, _, cx| update_config(cx, &apply))
        };
        let counter = div()
            .row()
            .items_center()
            .gap(px(SPACE_6_PX))
            .child(step("−", -1))
            .child(div().w(px(DIM_56_PX)).text_center().font_features(mandara_ui::fonts::tabular_nums()).child(count.map(|n| n.to_string()).unwrap_or_else(|| t("Default").to_string())))
            .child(step("+", 1));
        // Monitor: any, or one of the outputs.
        let monitors: Vec<String> = match &Self::comp_state(cx).monitors {
            Some(Ok(m)) => m.iter().map(|m| m.name.clone()).collect(),
            _ => Vec::new(),
        };
        let mut names: Vec<Option<String>> = vec![None];
        names.extend(monitors.into_iter().map(Some));
        if let Some(m) = &current.monitor
            && !names.contains(&Some(m.clone()))
        {
            names.push(Some(m.clone()));
        }
        let monitor = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(names.into_iter().enumerate().map(|(n, name)| {
            let active = current.monitor == name;
            let label = name.clone().unwrap_or_else(|| t("Every monitor").to_string());
            let apply = edit(Box::new(move |r: &mut TagRule| r.monitor = name.clone()));
            chip(SharedString::from(format!("tag-{id}-mon-{n}")), label, active, p).on_click(move |_, _, cx| update_config(cx, &apply))
        }));
        let clear = rule.is_some().then(|| {
            button(SharedString::from(format!("tag-{id}-clear")), "trash", t("Clear rule"), false, p).on_click(move |_, _, cx| {
                update_config(cx, |c| c.compositor.tag_rules.retain(|r| r.id != id));
            })
        });
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .child(div().row().items_center().justify_between().child(div().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(tf("Tag {id}", &[("id", &id)]))).when_some(clear, |el, c| el.child(c)))
            .child(div().row().flex_wrap().items_center().justify_between().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Layout"))).child(default_chip))
            .child(layouts)
            .child(row(t("Master share").into(), mfact))
            .child(row(t("Windows in master").into(), counter.into_any_element()))
            .child(row(t("Scroller column width").into(), proportion))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_12_5_PX)).child(t("Monitor"))).child(monitor))
            .into_any_element()
    }

    // ── Niri: columns & outputs ─────────────────────────────────────────────

    fn niri_studio(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let presets = cfg.compositor.niri.preset_column_widths.clone();
        const SUGGESTED: &[f32] = &[0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 1.0];
        let mut chips = div().row().flex_wrap().items_center().gap(px(SPACE_6_PX));
        for (n, w) in presets.iter().enumerate() {
            chips = chips.child(
                chip(SharedString::from(format!("niri-preset-{n}")), percent(*w), true, p).child(icon("close", ICON_10_PX, p.on_primary)).on_click(move |_, _, cx| {
                    update_config(cx, |c| {
                        if n < c.compositor.niri.preset_column_widths.len() {
                            c.compositor.niri.preset_column_widths.remove(n);
                        }
                    });
                }),
            );
        }
        for (n, w) in SUGGESTED.iter().copied().enumerate().filter(|(_, w)| !presets.iter().any(|p| (p - w).abs() < SAME_WIDTH_EPSILON)) {
            chips = chips.child(chip(SharedString::from(format!("niri-suggest-{n}")), format!("+ {}", percent(w)), false, p).on_click(move |_, _, cx| {
                update_config(cx, |c| {
                    let list = &mut c.compositor.niri.preset_column_widths;
                    list.push(w);
                    list.sort_by(|a, b| a.total_cmp(b));
                });
            }));
        }
        // Column preview: each preset width as a bar on a screen strip.
        let strip = div().row().flex_col().gap(px(SPACE_4_PX)).children(presets.iter().map(|w| {
            div().h(px(DIM_10_PX)).w_full().rounded_full().bg(p.surface_dim.hsla()).child(div().h_full().w(gpui::relative(w.clamp(0.0, 1.0))).rounded_full().bg(p.primary.with_alpha(ALPHA_60).hsla()))
        }));

        // Outputs.
        let outputs = cfg.compositor.niri.outputs.clone();
        let known: Vec<MonitorInfo> = match &Self::comp_state(cx).monitors {
            Some(Ok(m)) => m.clone(),
            _ => Vec::new(),
        };
        let mut out_list = div().row().flex_col().gap(px(SPACE_8_PX));
        for (n, o) in outputs.iter().enumerate() {
            let pos_field = |this: &mut Self, axis: char, current: Option<i32>, cx: &mut Context<Self>| {
                this.list_field(
                    format!("niri-out-{n}-{axis}"),
                    &current.map(|v| v.to_string()).unwrap_or_default(),
                    if axis == 'x' { "x" } else { "y" },
                    Rc::new(move |c: &mut ShellConfig, v: String| {
                        if let Some(o) = c.compositor.niri.outputs.get_mut(n) {
                            let v = v.trim().parse::<i32>().ok();
                            if axis == 'x' {
                                o.x = v;
                            } else {
                                o.y = v;
                            }
                        }
                    }),
                    cx,
                )
            };
            let (x, y) = (pos_field(self, 'x', o.x, cx), pos_field(self, 'y', o.y, cx));
            let scale = opt_slider(
                SharedString::from(format!("niri-out-{n}-scale")),
                o.scale,
                (0.5, 3.0, 0.05),
                known.iter().find(|m| m.name == o.name).map(|m| m.scale).filter(|s| *s > 0.0).unwrap_or(1.0),
                |v| format!("{v:.2}×"),
                tk("Your config"),
                Rc::new(move |c: &mut ShellConfig, v| {
                    if let Some(o) = c.compositor.niri.outputs.get_mut(n) {
                        o.scale = v;
                    }
                }),
                p,
            );
            out_list = out_list.child(
                div()
                    .p(px(SPACE_10_PX))
                    .rounded(px(RADIUS_14_PX))
                    .bg(p.surface_container_low.hsla())
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(icon("monitor", ICON_16_PX, p.primary))
                    .child(div().w(px(DIM_110_PX)).font_weight(FontWeight::MEDIUM).child(o.name.clone()))
                    .child(scale)
                    .child(div().row().items_center().gap(px(SPACE_6_PX)).child(t("Position")).child(div().w(px(DIM_70_PX)).child(x)).child(div().w(px(DIM_70_PX)).child(y)))
                    .child(
                        div()
                            .id(("niri-out-del", n))
                            .size(px(DIM_30_PX))
                            .rounded_full()
                            .row()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.urgent_coral.with_alpha(ALPHA_14).hsla()))
                            .child(icon("trash", ICON_14_PX, p.on_surface_variant))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.list_fields.retain(|k, _| !k.starts_with("niri-out-"));
                                update_config(cx, |c| {
                                    if n < c.compositor.niri.outputs.len() {
                                        c.compositor.niri.outputs.remove(n);
                                    }
                                });
                            })),
                    ),
            );
        }
        let addable: Vec<MonitorInfo> = known.into_iter().filter(|m| !outputs.iter().any(|o| o.name == m.name)).collect();
        let add = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(addable.into_iter().enumerate().map(|(n, m)| {
            let name = m.name.clone();
            chip(SharedString::from(format!("niri-out-add-{n}")), format!("+ {}", m.name), false, p).on_click(move |_, _, cx| {
                let o = OutputRule { name: name.clone(), scale: (m.scale > 0.0).then_some(m.scale), x: Some(m.x), y: Some(m.y) };
                update_config(cx, move |c| c.compositor.niri.outputs.push(o));
            })
        }));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(group_title(t("Columns & outputs"), p))
            .child(
                card(p)
                    .child(heading(t("Preset column widths"), t("The widths Mod+R cycles through (switch-preset-column-width)"), p))
                    .child(chips)
                    .when(!presets.is_empty(), |el| el.child(strip))
                    .child(heading(t("Outputs"), t("Scale and position per monitor. Only what you add here is written; other outputs keep your config."), p))
                    .child(out_list)
                    .child(add),
            )
            .into_any_element()
    }
}

/// `~/.config/mandara/generated/<file>` with `~` for display.
/// The border colours the shell pushes for palette `p` (the same roles as
/// `mandara_ui::plugins::compositor::border_colors`).
pub(super) fn preview_borders(p: &MandaraPalette) -> BorderColors {
    BorderColors { active: p.primary.to_hex(), active_end: p.accent_gradient_end.to_hex(), inactive: p.surface_container_high.to_hex(), urgent: p.urgent_coral.to_hex() }
}

/// The drop-in the shell would write for `tab` (Studio's preview).
pub(super) fn preview_text(tab: Target, config: &CompositorConfig, borders: Option<BorderColors>) -> String {
    let managed = ManagedConfig { config, borders, existing_chords: Default::default(), mandara_bin: "mandara".into(), version: None, user_autostarts: false };
    emit(tab, &managed).text
}

fn target_file(t: Target) -> String {
    let path = t.generated_file().display().to_string();
    match dirs::home_dir() {
        Some(home) => path.replacen(&home.display().to_string(), "~", 1),
        None => path,
    }
}

/// A tiny initial (M / N / H): tinted when the compositor supports an
/// option, faint when it doesn't; the selected tab's is filled.
fn support_dot(target: Target, supported: bool, current: bool, p: &MandaraPalette) -> Div {
    let letter = &target_name(target)[..1];
    let base = div().w(px(DIM_15_PX)).h(px(DIM_15_PX)).rounded(px(RADIUS_5_PX)).row().flex_none().items_center().justify_center().text_size(px(TEXT_9_5_PX)).font_weight(FontWeight::BOLD);
    match (supported, current) {
        (true, true) => base.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).child(letter.to_string()),
        (true, false) => base.bg(p.primary.with_alpha(ALPHA_16).hsla()).text_color(p.primary.hsla()).child(letter.to_string()),
        (false, _) => base.border_1().border_color(p.outline.with_alpha(ALPHA_30).hsla()).text_color(p.outline.with_alpha(ALPHA_50).hsla()).child(letter.to_string()),
    }
}

/// The label of a MangoWM layout name.
fn layout_label(layout: &str) -> &'static str {
    MANGO_LAYOUTS.iter().find(|(n, _)| *n == layout).map(|(_, l)| *l).unwrap_or(tk("Custom"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_regex_escapes_metacharacters() {
        assert_eq!(exact_regex("org.gnome.Nautilus"), "^org\\.gnome\\.Nautilus$");
        assert_eq!(exact_regex("a+b (1)"), "^a\\+b \\(1\\)$");
        assert!(regex_problem(&exact_regex("weird[name](x)\\")).is_none());
    }

    #[test]
    fn regex_problems() {
        assert!(regex_problem("^firefox$").is_none());
        assert!(regex_problem("(a|b").is_some());
        assert!(regex_problem("a)").is_some());
        assert!(regex_problem("trail\\").is_some());
        assert!(regex_problem("[abc]\\]").is_none());
    }

    #[test]
    fn every_layout_has_a_sketch_inside_the_frame() {
        for (name, _) in MANGO_LAYOUTS {
            let sketch = layout_sketch(name);
            assert!(!sketch.is_empty(), "{name}");
            for (x, y, w, h) in sketch {
                assert!(*x >= 0.0 && *y >= 0.0 && x + w <= 1.0001 && y + h <= 1.0001, "{name}");
            }
        }
    }

    #[test]
    fn every_rule_option_has_a_label_and_an_editor() {
        for (name, _) in RULE_OPTIONS {
            assert!(!option_label(name).0.is_empty(), "{name}");
            let mut r = WindowRule::default();
            assert!(rule_flag(&mut r, name).is_some() || rule_opacity(&mut r, name).is_some(), "{name}");
        }
    }
}
