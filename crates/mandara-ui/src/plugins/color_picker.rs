//! Color picker plugin: an eyedropper over a frozen frame of the screen
//! (magnifier loupe, live HEX), and a panel with the picked-color history
//! as swatches; the selected color shows HEX / RGB / HSL / OKLCH with copy
//! buttons, an OKLCH tonal ramp and WCAG contrast against white, black and
//! the current theme surface.
//!
//! History lives in `~/.local/state/mandara/colors.toml` (capped). Settings
//! (`[plugins.settings.color-picker]`): `copy_on_pick`, `copy_format`
//! (`hex` / `rgb` / `hsl` / `oklch`), `open_after_pick`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::color_picker::{self as cp, Grade, Picked};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{Flash, caption, toast};
use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, HEADER_H, LINK_HOVER_ALPHA, PanelShape, SECTION_H_PX, ViewSet, WASH_TRACK, empty_state, header, pill_button,
    readable, redraw_in, section,
};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{reveal, text};

pub const ID: &str = "color-picker";

/// Panel height changes smaller than this don't resize the surface.
const RESIZE_EPSILON_PX: f32 = 0.5;

const PANEL_W: f32 = 404.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const SECTION_H: f32 = SECTION_H_PX;
/// Gap between a section label and its content.
const SECTION_GAP: f32 = 6.0;
/// Gap between the hero and the format rows.
const HERO_GAP: f32 = 8.0;
const HERO_H: f32 = 92.0;
const FORMAT_H: f32 = 32.0;
const FORMATS: usize = 4;
const RAMP_H: f32 = 34.0;
const CONTRAST_H: f32 = 78.0;
const SWATCH: f32 = 34.0;
const SWATCH_GAP: f32 = 8.0;
const SWATCHES_PER_ROW: usize = 9;
const EMPTY_H: f32 = 190.0;
/// A copy button shows its check mark this long.
const COPIED_FOR: Duration = Duration::from_millis(1400);
/// Wait for the panel to slide away before freezing the screen.
const PANEL_CLEAR_DELAY: Duration = Duration::from_millis(280);

// Hero.
/// Hero gradient: lightness lift of the top stop / drop of the bottom.
const HERO_SHEEN: f32 = 0.03;
/// Hero glow: alpha, blur, spread and y offset.
const HERO_GLOW_ALPHA: f32 = 0.35;
const HERO_GLOW_BLUR_PX: f32 = 18.0;
const HERO_GLOW_SPREAD_PX: f32 = -4.0;
const HERO_GLOW_DY_PX: f32 = 6.0;
const HERO_PAD_PX: f32 = 14.0;
const HERO_HEX_PX: f32 = 26.0;
const HERO_HEX_LINE_PX: f32 = 28.0;
const HERO_META_ALPHA: f32 = 0.75;
/// Ink on light / dark swatches: white and near-black (OKLCH lightness).
const INK_DARK_L: f32 = 0.16;
const PERCENT: f32 = 100.0;

// Format rows.
const FORMAT_PAD_X_PX: f32 = 8.0;
const FORMAT_RADIUS_PX: f32 = 9.0;
const FORMAT_GAP_PX: f32 = 10.0;
const FORMAT_LABEL_W_PX: f32 = 48.0;
const COPY_ICON_PX: f32 = 14.0;
const FORMAT_HOVER_ALPHA: f32 = 0.045;

// Tonal ramp.
const RAMP_GAP_PX: f32 = 3.0;
const RAMP_MARK_PAD_PX: f32 = 4.0;
const RAMP_END_RADIUS_PX: f32 = 10.0;
const RAMP_RADIUS_PX: f32 = 3.0;
const RAMP_MARK_PX: f32 = 5.0;
const RAMP_HOVER_OPACITY: f32 = 0.85;

// Contrast cards.
const CONTRAST_GAP_PX: f32 = 8.0;
/// Contrast cards are this much tighter than the panel's concentric radius.
const CONTRAST_RADIUS_INSET_PX: f32 = 4.0;
const CONTRAST_PAD_PX: f32 = 10.0;
const CONTRAST_SAMPLE_PX: f32 = 20.0;
const GRADE_PAD_X_PX: f32 = 6.0;
const GRADE_H_PX: f32 = 18.0;
const GRADE_FILL_ALPHA: f32 = 0.18;
const GRADE_TEXT_PX: f32 = 10.0;
const CONTRAST_LABEL_ALPHA: f32 = 0.65;
/// Ratios are reported rounded to this many steps per unit.
const RATIO_STEPS: f32 = 100.0;

// History swatches.
const SWATCH_RADIUS_PX: f32 = 11.0;
const SWATCH_SELECTED_PAD_PX: f32 = 3.0;
const SWATCH_SELECTED_RADIUS_PX: f32 = 7.0;

// "Clear" link.
const CLEAR_H_PX: f32 = 18.0;
const CLEAR_PAD_X_PX: f32 = 6.0;

// Bar item.
const BAR_GAP_PX: f32 = 6.0;
const BAR_ICON_PX: f32 = 15.0;
const BAR_SWATCH_PX: f32 = 10.0;
const BAR_SWATCH_EDGE_ALPHA: f32 = 0.25;

#[derive(Default)]
struct State {
    loaded: bool,
    colors: Vec<Picked>,
    selected: Option<String>,
    copied: Option<(&'static str, Instant)>,
}

thread_local! {
    static STATE: Rc<RefCell<State>> = Rc::new(RefCell::new(State::default()));
    /// The bar chip and panel: history changes notify just these.
    static VIEWS: ViewSet = ViewSet::default();
}

fn state() -> Rc<RefCell<State>> {
    STATE.with(|s| s.clone())
}

fn views() -> ViewSet {
    VIEWS.with(ViewSet::clone)
}

/// Loads the history synchronously (commands only — views load it off the
/// UI thread at startup via [`load_in_background`]).
fn ensure_loaded() {
    let s = state();
    let mut st = s.borrow_mut();
    if !st.loaded {
        st.colors = cp::load_history();
        st.selected = st.colors.first().map(|c| c.hex.clone());
        st.loaded = true;
    }
}

/// Reads the history file on the background executor; views show an empty
/// history until it lands (the file is tiny, so that's a single frame).
fn load_in_background(cx: &mut App) {
    if state().borrow().loaded {
        return;
    }
    let task = cx.background_executor().spawn(async { cp::load_history() });
    cx.spawn(async move |cx| {
        let colors = task.await;
        {
            let s = state();
            let mut st = s.borrow_mut();
            if !st.loaded {
                st.selected = colors.first().map(|c| c.hex.clone());
                st.colors = colors;
                st.loaded = true;
            }
        }
        views().notify_async(cx);
    })
    .detach();
}

fn persist(colors: Vec<Picked>) {
    let _ = std::thread::Builder::new().name("mandara-colors-save".into()).spawn(move || {
        if let Err(e) = cp::save_history(&colors) {
            tracing::warn!("color history: {e}");
        }
    });
}

fn format_of(rgb: [u8; 3], key: &str) -> String {
    let f = cp::formats(rgb);
    match key {
        "rgb" => f.rgb,
        "hsl" => f.hsl,
        "oklch" => f.oklch,
        _ => f.hex,
    }
}

fn copy_text(s: &str) -> Result<(), String> {
    mandara_services::clipboard::serve(s.as_bytes().to_vec(), None)
}

/// Records a picked color (and copies / opens the panel per settings).
fn picked(rgb: [u8; 3], cx: &mut App) -> String {
    ensure_loaded();
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let s = state();
    let colors = {
        let mut st = s.borrow_mut();
        cp::remember(&mut st.colors, rgb, chrono::Local::now().timestamp());
        st.selected = Some(cp::to_hex(rgb));
        st.colors.clone()
    };
    persist(colors);
    if cfg.plugins.setting(ID, "copy_on_pick", true) {
        let text = format_of(rgb, &cfg.plugins.setting(ID, "copy_format", String::from("hex")));
        if let Err(e) = copy_text(&text) {
            tracing::warn!("color picker copy: {e}");
        }
    }
    if cfg.plugins.setting(ID, "open_after_pick", true) {
        crate::state::shell(cx).update(cx, |sh, cx| sh.set_panel(Some(ID.into()), cx));
    }
    views().notify(cx);
    cp::to_hex(rgb)
}

/// Sends the reply exactly once; dropping it unanswered reports a cancel.
struct ReplyOnce(Option<async_channel::Sender<Result<String, String>>>);

impl ReplyOnce {
    fn send(mut self, result: Result<String, String>) {
        if let Some(tx) = self.0.take() {
            let _ = tx.try_send(result);
        }
    }
}

impl Drop for ReplyOnce {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.try_send(Err(t("Cancelled").into()));
        }
    }
}

/// Closes the panel, waits for it to clear, then opens the eyedropper.
fn start_pick(reply: Option<ReplyOnce>, cx: &mut App) {
    crate::state::shell(cx).update(cx, |sh, cx| sh.set_panel(None, cx));
    cx.spawn(async move |cx| {
        cx.background_executor().timer(PANEL_CLEAR_DELAY).await;
        cx.update(|cx| {
            super::screen_pick::pick_color(cx, move |rgb, cx| {
                let hex = picked(rgb, cx);
                if let Some(r) = reply {
                    r.send(Ok(serde_json::to_string(&cp::formats(rgb)).unwrap_or(hex)));
                }
            })
        });
    })
    .detach();
}

/// Copies `value`, flashes the result and re-renders the views.
fn copy_and_flash(value: &str, copied_key: Option<&'static str>, flash: &Flash, cx: &mut App) {
    match copy_text(value) {
        Ok(()) => {
            if let Some(key) = copied_key {
                state().borrow_mut().copied = Some((key, Instant::now()));
            }
            flash.show(true, tf("Copied {value}", &[("value", &value)]));
        }
        Err(e) => flash.show(false, e),
    }
    views().notify(cx);
}

// ── Views ────────────────────────────────────────────────────────────────

fn to_color(rgb: [u8; 3]) -> Color {
    cp::color_of(rgb)
}

fn ink_on(c: Color) -> Color {
    c.best_on(Color::oklch(1.0, 0.0, 0.0), Color::oklch(INK_DARK_L, 0.0, 0.0))
}

/// Everything derived from the selected color — formats, ramp, contrast
/// texts — computed once per selection (and theme surface), not per frame.
struct Detail {
    key: ([u8; 3], Option<i64>, Color),
    hex: SharedString,
    lch: SharedString,
    when: SharedString,
    /// `(copy key, label, value)` per format row.
    formats: [(&'static str, &'static str, SharedString); FORMATS],
    /// `(tone, hex)` per ramp step, and the step nearest the color.
    tones: Vec<([u8; 3], SharedString)>,
    nearest: Option<usize>,
    /// `(label, background, ratio text, grade)` per contrast card.
    contrast: [(&'static str, Color, SharedString, Grade); 3],
}

impl Detail {
    fn new(rgb: [u8; 3], picked_at: Option<i64>, surface: Color) -> Self {
        let c = to_color(rgb);
        let f = cp::formats(rgb);
        let tones: Vec<([u8; 3], SharedString)> = cp::tonal_ramp(rgb).into_iter().map(|t| (t, cp::to_hex(t).into())).collect();
        let base_l = c.lightness();
        // The tone closest to the picked color gets a marker.
        let nearest = tones
            .iter()
            .enumerate()
            .min_by(|a, b| (to_color(a.1.0).lightness() - base_l).abs().total_cmp(&(to_color(b.1.0).lightness() - base_l).abs()))
            .map(|(i, _)| i);
        let card = |label: &'static str, bg: Color| {
            let ratio = c.contrast_ratio(bg);
            (label, bg, SharedString::from(format!("{ratio:.2}:1")), Grade::of(ratio))
        };
        Self {
            key: (rgb, picked_at, surface),
            hex: f.hex.to_uppercase().into(),
            lch: format!("L {:.0}% · C {:.3} · H {:.0}°", c.lightness() * PERCENT, c.chroma(), c.hue()).into(),
            when: picked_at
                .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
                .map(|d| d.with_timezone(&chrono::Local).format("%b %-d · %H:%M").to_string())
                .unwrap_or_default()
                .into(),
            formats: [("hex", "HEX", f.hex.to_uppercase().into()), ("rgb", "RGB", f.rgb.into()), ("hsl", "HSL", f.hsl.into()), ("oklch", "OKLCH", f.oklch.into())],
            tones,
            nearest,
            contrast: [card(t("on white"), Color::oklch(1.0, 0.0, 0.0)), card(t("on black"), Color::oklch(0.0, 0.0, 0.0)), card(t("on theme"), surface)],
        }
    }
}

fn hero(rgb: [u8; 3], d: &Detail, display: SharedString, radius: f32) -> AnyElement {
    let c = to_color(rgb);
    let ink = ink_on(c);
    div()
        .h(px(HERO_H))
        .rounded(px(radius))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, c.lighten(HERO_SHEEN), c.lighten(-HERO_SHEEN)))
        .shadow(vec![super::panel_fx::glow(c.with_alpha(HERO_GLOW_ALPHA), HERO_GLOW_BLUR_PX, HERO_GLOW_SPREAD_PX, HERO_GLOW_DY_PX)])
        .p(px(HERO_PAD_PX))
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .font_family(display)
                .font_features(tabular_nums())
                .text_size(px(HERO_HEX_PX))
                .line_height(px(HERO_HEX_LINE_PX))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ink.hsla())
                .child(d.hex.clone()),
        )
        .child(div().flex().justify_between().text_size(px(text::LABEL)).text_color(ink.with_alpha(HERO_META_ALPHA).hsla()).child(d.lch.clone()).child(d.when.clone()))
        .into_any_element()
}

fn format_rows(d: &Detail, copied: Option<&'static str>, flash: &Flash, mono: SharedString, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = palette.on_surface.with_alpha(FORMAT_HOVER_ALPHA);
    div()
        .flex()
        .flex_col()
        .children(d.formats.iter().enumerate().map(|(i, (key, label, value))| {
            let key = *key;
            let done = copied == Some(key);
            let v = value.clone();
            let flash = flash.clone();
            div()
                .id(("cp-fmt", i))
                .h(px(FORMAT_H))
                .px(px(FORMAT_PAD_X_PX))
                .rounded(px(crate::style::radius(FORMAT_RADIUS_PX)))
                .flex()
                .items_center()
                .gap(px(FORMAT_GAP_PX))
                .cursor_pointer()
                .hover(move |s| s.bg(hover.hsla()))
                .child(div().w(px(FORMAT_LABEL_W_PX)).text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(*label))
                .child(div().flex_1().min_w(px(0.0)).font_family(mono.clone()).font_features(tabular_nums()).text_size(px(text::BODY_S)).truncate().child(value.clone()))
                .child(icon(if done { "check" } else { "copy" }, COPY_ICON_PX, if done { readable(palette.success_emerald, palette) } else { accent }))
                .on_click(move |_, _, cx| copy_and_flash(&v, Some(key), &flash, cx))
        }))
        .into_any_element()
}

fn ramp(d: &Detail, flash: &Flash) -> AnyElement {
    let n = d.tones.len();
    div()
        .h(px(RAMP_H))
        .flex()
        .gap(px(RAMP_GAP_PX))
        .children(d.tones.iter().enumerate().map(|(i, (tone, hex))| {
            let c = to_color(*tone);
            let hex = hex.clone();
            let flash = flash.clone();
            div()
                .id(("cp-ramp", i))
                .flex_1()
                .h_full()
                .bg(c.hsla())
                .cursor_pointer()
                .flex()
                .items_end()
                .justify_center()
                .pb(px(RAMP_MARK_PAD_PX))
                .when(i == 0, |el| el.rounded_l(px(crate::style::radius(RAMP_END_RADIUS_PX))))
                .when(i == n - 1, |el| el.rounded_r(px(crate::style::radius(RAMP_END_RADIUS_PX))))
                .when(i != 0 && i != n - 1, |el| el.rounded(px(crate::style::radius(RAMP_RADIUS_PX))))
                .when(Some(i) == d.nearest, |el| el.child(div().size(px(RAMP_MARK_PX)).rounded_full().bg(ink_on(c).hsla())))
                .hover(|s| s.opacity(RAMP_HOVER_OPACITY))
                .on_click(move |_, _, cx| copy_and_flash(&hex, None, &flash, cx))
        }))
        .into_any_element()
}

fn grade_tint(g: Grade, palette: &MandaraPalette) -> Color {
    readable(
        match g {
            Grade::Aaa | Grade::Aa => palette.success_emerald,
            Grade::AaLarge => palette.warning_amber,
            Grade::Fail => palette.urgent_coral,
        },
        palette,
    )
}

fn contrast(rgb: [u8; 3], d: &Detail, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let c = to_color(rgb);
    div()
        .flex()
        .gap(px(CONTRAST_GAP_PX))
        .children(d.contrast.iter().map(|(label, bg, ratio, grade)| {
            let tint = grade_tint(*grade, palette);
            let bg_ink = ink_on(*bg);
            div()
                .flex_1()
                .h(px(CONTRAST_H))
                .rounded(px(crate::style::concentric(radius, CONTRAST_RADIUS_INSET_PX)))
                .bg(bg.hsla())
                .border_1()
                .border_color(palette.on_surface.with_alpha(WASH_TRACK.0).hsla())
                .p(px(CONTRAST_PAD_PX))
                .flex()
                .flex_col()
                .justify_between()
                .child(div().flex().items_center().justify_between().child(div().text_size(px(CONTRAST_SAMPLE_PX)).font_weight(FontWeight::BOLD).text_color(c.hsla()).child("Aa")).child(
                    div()
                        .px(px(GRADE_PAD_X_PX))
                        .h(px(GRADE_H_PX))
                        .rounded_full()
                        .bg(tint.with_alpha(GRADE_FILL_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .text_size(px(GRADE_TEXT_PX))
                        .font_weight(FontWeight::BOLD)
                        .text_color(tint.hsla())
                        .child(grade.label()),
                ))
                .child(
                    div()
                        .flex()
                        .items_end()
                        .justify_between()
                        .child(div().font_features(tabular_nums()).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(bg_ink.hsla()).child(ratio.clone()))
                        .child(div().text_size(px(GRADE_TEXT_PX)).text_color(bg_ink.with_alpha(CONTRAST_LABEL_ALPHA).hsla()).child(*label)),
                )
        }))
        .into_any_element()
}

fn swatches(colors: &[Picked], selected: Option<&str>, palette: &MandaraPalette) -> AnyElement {
    let ring = palette.on_surface;
    div()
        .flex()
        .flex_wrap()
        .gap(px(SWATCH_GAP))
        .children(colors.iter().enumerate().map(|(i, p)| {
            let rgb = p.rgb().unwrap_or_default();
            let c = to_color(rgb);
            let is_sel = selected == Some(p.hex.as_str());
            let hex = p.hex.clone();
            let hex_remove = p.hex.clone();
            div()
                .id(("cp-swatch", i))
                .size(px(SWATCH))
                .rounded(px(crate::style::radius(SWATCH_RADIUS_PX)))
                .p(px(if is_sel { SWATCH_SELECTED_PAD_PX } else { 0.0 }))
                .when(is_sel, |el| el.border_2().border_color(ring.hsla()))
                .cursor_pointer()
                .child(
                    div()
                        .size_full()
                        .rounded(px(crate::style::radius(if is_sel { SWATCH_SELECTED_RADIUS_PX } else { SWATCH_RADIUS_PX })))
                        .bg(c.hsla())
                        .border_1()
                        .border_color(palette.on_surface.with_alpha(WASH_TRACK.0).hsla()),
                )
                .on_click(move |_, _, cx| {
                    state().borrow_mut().selected = Some(hex.clone());
                    views().notify(cx);
                })
                // Right-click forgets a color.
                .on_mouse_down(MouseButton::Right, move |_, _, cx| {
                    let s = state();
                    let colors = {
                        let mut st = s.borrow_mut();
                        st.colors.retain(|c| c.hex != hex_remove);
                        if st.selected.as_deref() == Some(hex_remove.as_str()) {
                            st.selected = st.colors.first().map(|c| c.hex.clone());
                        }
                        st.colors.clone()
                    };
                    persist(colors);
                    views().notify(cx);
                })
        }))
        .into_any_element()
}

fn swatch_rows(n: usize) -> usize {
    n.div_ceil(SWATCHES_PER_ROW).max(1)
}

fn panel_height(has_selection: bool, n: usize) -> f32 {
    let head = PAD * 2.0 + HEADER_H;
    if n == 0 {
        return head + GAP + EMPTY_H;
    }
    let detail = if has_selection {
        GAP + HERO_H + HERO_GAP + FORMAT_H * FORMATS as f32 + GAP + SECTION_H + SECTION_GAP + RAMP_H + GAP + SECTION_H + SECTION_GAP + CONTRAST_H
    } else {
        0.0
    };
    let rows = swatch_rows(n) as f32;
    head + detail + GAP + SECTION_H + SECTION_GAP + rows * SWATCH + (rows - 1.0) * SWATCH_GAP
}

struct View {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<Pixels>>>,
    flash: Flash,
    detail: RefCell<Option<Detail>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        views().track(window);
        let s = state();
        let copied = {
            let mut st = s.borrow_mut();
            if st.copied.is_some_and(|(_, at)| at.elapsed() >= COPIED_FOR) {
                st.copied = None;
            }
            if let Some((_, at)) = st.copied {
                redraw_in(COPIED_FOR.saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
            }
            st.copied.map(|(k, _)| k)
        };
        // Borrow the history for this frame instead of cloning it.
        let st = s.borrow();
        let colors = &st.colors;
        let sel = st.selected.as_deref().and_then(|h| colors.iter().find(|c| c.hex == h)).or(colors.first());
        let h = panel_height(sel.is_some(), colors.len());
        if (f32::from(self.size_cell.get().height) - h).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }
        let f = fonts(cx);
        let radius = PanelShape::of(cx).inner(PAD);
        let accent = readable(palette.primary, palette);
        let subtitle: SharedString = if colors.is_empty() { t("Pick any pixel on screen").into() } else { tf("Saved colors: {n}", &[("n", &colors.len())]).into() };
        let pick = pill_button("cp-pick", Some("eyedropper"), t("Pick"), accent, palette, |_, cx| start_pick(None, cx));
        let head = header("eyedropper", accent, t("Color picker"), subtitle, palette, Some(pick.into_any_element()));
        let p = &self.p;
        let mut col = div().relative().size_full().p(px(PAD)).flex().flex_col().child(reveal(p, 0, head));
        if colors.is_empty() {
            col = col.child(div().h(px(GAP))).child(reveal(p, 1, empty_state("eyedropper", t("No colors yet"), t("Pick a pixel anywhere on screen — it lands here"), EMPTY_H, palette)));
        } else {
            if let Some(sel) = sel {
                let rgb = sel.rgb().unwrap_or_default();
                let key = (rgb, Some(sel.at), palette.surface_base);
                let mut cache = self.detail.borrow_mut();
                if cache.as_ref().is_none_or(|d| d.key != key) {
                    *cache = Some(Detail::new(rgb, Some(sel.at), palette.surface_base));
                }
                if let Some(d) = cache.as_ref() {
                    col = col
                        .child(div().h(px(GAP)))
                        .child(reveal(p, 1, div().flex().flex_col().gap(px(HERO_GAP)).child(hero(rgb, d, f.display.clone(), radius)).child(format_rows(d, copied, &self.flash, f.mono.clone(), palette))))
                        .child(div().h(px(GAP)))
                        .child(reveal(
                            p,
                            2,
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(SECTION_GAP))
                                .child(section(t("TONAL RAMP · OKLCH"), palette, Some(caption(t("click to copy"), palette).into_any_element())))
                                .child(ramp(d, &self.flash)),
                        ))
                        .child(div().h(px(GAP)))
                        .child(reveal(p, 3, div().flex().flex_col().gap(px(SECTION_GAP)).child(section(t("CONTRAST · WCAG 2"), palette, None)).child(contrast(rgb, d, palette, radius))));
                }
            }
            let clear = {
                let fg = readable(palette.primary, palette);
                div()
                    .id("cp-clear")
                    .h(px(CLEAR_H_PX))
                    .px(px(CLEAR_PAD_X_PX))
                    .rounded_full()
                    .cursor_pointer()
                    .text_size(px(text::LABEL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(fg.hsla())
                    .hover(move |s| s.bg(fg.with_alpha(LINK_HOVER_ALPHA).hsla()))
                    .child(t("Clear"))
                    .on_click(|_, _, cx| {
                        {
                            let s = state();
                            let mut st = s.borrow_mut();
                            st.colors.clear();
                            st.selected = None;
                        }
                        persist(Vec::new());
                        views().notify(cx);
                    })
                    .into_any_element()
            };
            col = col.child(div().h(px(GAP))).child(reveal(
                p,
                4,
                div().flex().flex_col().gap(px(SECTION_GAP)).child(section(t("HISTORY"), palette, Some(clear))).child(swatches(colors, sel.map(|s| s.hex.as_str()), palette)),
            ));
        }
        drop(st);
        col.when_some(self.flash.current(window, cx), |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette))).into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Color picker"), tk("Pick colors from the screen, with formats, tonal ramps and contrast"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, window, _| {
                views().track(window);
                // The latest pick as a tiny swatch beside the dropper.
                let last = STATE.with(|s| s.borrow().colors.first().and_then(|c| c.rgb()));
                div()
                    .flex()
                    .items_center()
                    .gap(px(BAR_GAP_PX))
                    .child(icon("eyedropper", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface))
                    .when_some(last, |el, rgb| {
                        el.child(
                            div().size(px(BAR_SWATCH_PX)).rounded_full().bg(to_color(rgb).hsla()).border_1().border_color(palette.on_surface.with_alpha(BAR_SWATCH_EDGE_ALPHA).hsla()),
                        )
                    })
                    .into_any_element()
            },
            window,
            cx,
        );
        load_in_background(cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height(true, 1)))));
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone(), flash: Flash::default(), detail: RefCell::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), sidebar: None, on_click: None, on_secondary_click: Some(Rc::new(|_, cx| start_pick(None, cx))) }
    });
    r.command(
        ID,
        CommandSpec::new_output("color-picker pick", tk("Open the eyedropper; replies with the picked color's formats (or an error if cancelled)"), |_, cx| {
            let (tx, rx) = async_channel::bounded(1);
            start_pick(Some(ReplyOnce(Some(tx))), cx);
            Ok(Output::Pending(rx))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("color-picker demo", tk("Preview the eyedropper overlay for a few seconds without taking input"), |_, cx| {
            super::screen_pick::demo_color(cx);
            Ok(json!({ "closes_after_ms": u64::try_from(super::screen_pick::DEMO_FOR.as_millis()).unwrap_or(u64::MAX) }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("color-picker history", tk("Picked colors, newest first"), |_, _| {
            ensure_loaded();
            Ok(json!(state().borrow().colors))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("color-picker show", tk("Formats, tonal ramp and WCAG contrast of a color"), |a, cx| {
            let rgb = cp::parse_hex(a.str(0)?).ok_or_else(|| t("Expected a color like #1e88e5").to_string())?;
            let surface = mandara_core::theme_entity(cx).read(cx).target().surface_base;
            let c = to_color(rgb);
            let grade = |bg: Color| {
                let r = c.contrast_ratio(bg);
                json!({ "ratio": (r * RATIO_STEPS).round() / RATIO_STEPS, "grade": Grade::of(r).label() })
            };
            Ok(json!({
                "formats": cp::formats(rgb),
                "ramp": cp::tonal_ramp(rgb).into_iter().map(cp::to_hex).collect::<Vec<_>>(),
                "contrast": { "white": grade(Color::oklch(1.0, 0.0, 0.0)), "black": grade(Color::oklch(0.0, 0.0, 0.0)), "theme_surface": grade(surface) },
            }))
        })
        .arg(ArgSpec::required("color", ArgKind::String, tk("hex color, e.g. #1e88e5")))
        .example("color-picker show #1e88e5"),
    );
    r.command(
        ID,
        CommandSpec::new("color-picker select", tk("Add a color to the history and select it"), |a, cx| {
            let rgb = cp::parse_hex(a.str(0)?).ok_or_else(|| t("Expected a color like #1e88e5").to_string())?;
            ensure_loaded();
            let s = state();
            let colors = {
                let mut st = s.borrow_mut();
                cp::remember(&mut st.colors, rgb, chrono::Local::now().timestamp());
                st.selected = Some(cp::to_hex(rgb));
                st.colors.clone()
            };
            persist(colors);
            views().notify(cx);
            Ok(json!(cp::formats(rgb)))
        })
        .arg(ArgSpec::required("color", ArgKind::String, tk("hex color")))
        .example("color-picker select #ffb347"),
    );
    r.command(
        ID,
        CommandSpec::new("color-picker clear", tk("Forget every picked color"), |_, cx| {
            let s = state();
            {
                let mut st = s.borrow_mut();
                st.loaded = true;
                st.colors.clear();
                st.selected = None;
            }
            persist(Vec::new());
            views().notify(cx);
            Ok(json!("ok"))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_height_tracks_history_rows() {
        assert!(panel_height(false, 0) < panel_height(true, 1));
        assert_eq!(panel_height(true, SWATCHES_PER_ROW + 1) - panel_height(true, SWATCHES_PER_ROW), SWATCH + SWATCH_GAP);
        assert_eq!(swatch_rows(cp::MAX_HISTORY), cp::MAX_HISTORY.div_ceil(SWATCHES_PER_ROW));
    }

    #[test]
    fn format_keys() {
        assert_eq!(format_of([255, 0, 0], "hex"), "#ff0000");
        assert_eq!(format_of([255, 0, 0], "rgb"), "rgb(255 0 0)");
        assert_eq!(format_of([255, 0, 0], "nope"), "#ff0000");
    }

    #[test]
    fn ink_contrasts_with_swatch() {
        assert!(ink_on(Color::oklch(0.95, 0.02, 90.0)).lightness() < 0.5);
        assert!(ink_on(Color::oklch(0.3, 0.1, 250.0)).lightness() > 0.5);
    }
}
