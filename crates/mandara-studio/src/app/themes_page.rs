//! Studio → Themes, the theme gallery half (the looks gallery and the page
//! layout are in `looks_page.rs`):
//!
//! * **Live preview** — a large mock of the shell (bar, desktop clock, a
//!   panel, a media card and the dock) drawn in the current palette over
//!   the current wallpaper, beside the theme's name, palette strip, text
//!   contrast and the light / dark switch.
//! * **Designed themes** — every catalog theme as a card: a mock in the
//!   current mode with the other mode inset in its corner, a palette strip
//!   and the description. Palettes are computed once per visit.
//! * **Colors from your wallpaper** — the wallpaper thumbnail, its seed
//!   colors (pick the lead) and every scheme style as a palette strip
//!   generated from the lead seed (cached per seed and mode).
//!
//! Everything here is static between config changes: no timers, and the
//! wallpaper is decoded once (off the UI thread) into a small thumbnail.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_theme::{Color, MandaraPalette, SchemeVariant, ThemeSource, catalog};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::{StudioView, apps_page, grid};
use crate::registry;
use crate::tokens::*;

/// Seed colors extracted from the wallpaper for the "Lead color" swatches.
const WALLPAPER_SEEDS: usize = 4;
/// Config path of the light / dark mode setting (the preview's switch).
pub(super) const THEME_MODE_PATH: &str = "/theme/mode";

// ── The shell mock ──────────────────────────────────────────────────────
/// The mock is laid out on this reference width; every part scales by
/// `width / MOCK_REF_W_PX` (heights follow the box it is given).
const MOCK_REF_W_PX: f32 = 480.0;
const MOCK_INSET: f32 = 10.0;
const MOCK_BAR_TOP: f32 = 8.0;
const MOCK_BAR_H: f32 = 20.0;
const MOCK_BAR_PAD: f32 = 8.0;
const MOCK_DOT: f32 = 8.0;
const MOCK_WS_ACTIVE_W: f32 = 22.0;
const MOCK_STATUS_W: f32 = 14.0;
const MOCK_STATUS_H: f32 = 6.0;
const MOCK_BAR_TEXT: f32 = 9.0;
const MOCK_GAP: f32 = 4.0;
/// The desktop clock widget (start side) and its date line.
const MOCK_CLOCK_TEXT: f32 = 40.0;
const MOCK_DATE_TEXT: f32 = 11.0;
const MOCK_CLOCK_LEFT: f32 = 26.0;
const MOCK_CLOCK_TOP: f32 = 40.0;
/// The clock is left out of mocks drawn smaller than this scale.
const MOCK_CLOCK_MIN_SCALE: f32 = 0.3;
/// The panel (end side, under the bar).
const MOCK_PANEL_TOP: f32 = 36.0;
const MOCK_PANEL_W: f32 = 150.0;
const MOCK_PANEL_H: f32 = 156.0;
const MOCK_PANEL_PAD: f32 = 10.0;
const MOCK_PANEL_GAP: f32 = 8.0;
const MOCK_AVATAR: f32 = 18.0;
const MOCK_LINE_H: f32 = 6.0;
const MOCK_LINE_W: [f32; 3] = [60.0, 40.0, 70.0];
const MOCK_TILE_H: f32 = 26.0;
const MOCK_TILE_RADIUS: f32 = 9.0;
const MOCK_SLIDER_FILL: f32 = 0.6;
/// The media card (start side, above the dock).
const MOCK_MEDIA_W: f32 = 150.0;
const MOCK_MEDIA_H: f32 = 48.0;
const MOCK_MEDIA_BOTTOM: f32 = 52.0;
const MOCK_ART: f32 = 32.0;
/// The dock (bottom centre).
const MOCK_DOCK_H: f32 = 32.0;
const MOCK_DOCK_PAD: f32 = 6.0;
const MOCK_DOCK_ICON: f32 = 20.0;
const MOCK_DOCK_BOTTOM: f32 = 8.0;
/// Space the dock and bar leave the panel (the panel never covers them).
const MOCK_PANEL_CLEAR: f32 = MOCK_DOCK_H + MOCK_DOCK_BOTTOM + MOCK_INSET;
/// How much of the accent tints the mock's sky when there is no wallpaper.
const MOCK_SKY_TINT: f32 = 0.35;
const DIAGONAL_DEG: f32 = 135.0;
const HORIZONTAL_DEG: f32 = 90.0;
/// Static digits on the mock clocks (a preview, not a live clock).
const MOCK_TIME: &str = "12:45";

// ── Live preview ────────────────────────────────────────────────────────
/// The preview's height per unit of its width.
const HERO_MOCK_ASPECT: f32 = 0.6;
/// Beside each other from this column width (else the info stacks under).
const HERO_SPLIT_MIN_W_PX: f32 = 760.0;
/// Share of the row the preview takes when side by side.
const HERO_MOCK_GROW: f32 = 3.0;
const HERO_INFO_GROW: f32 = 2.0;
/// Palette strips: the preview's and a card's.
const STRIP_H_PX: f32 = 28.0;
const CARD_STRIP_H_PX: f32 = 8.0;

// ── Designed theme cards ────────────────────────────────────────────────
/// The other-mode inset: this share of the card's mock width.
const INSET_SHARE: f32 = 0.28;
/// Ring around the inset (the card's own surface).
const INSET_RING_PX: f32 = 2.0;

// ── Wallpaper scheme styles ─────────────────────────────────────────────
const STYLE_GRID: CardGrid = CardGrid { min_w: 104.0, max_w: 180.0, gap: SPACE_8_PX };
const STYLE_STRIP_H_PX: f32 = 34.0;
const SEED_SWATCH_PX: f32 = DIM_44_PX;

/// Every theme with its palette in the current mode and the other one
/// (Studio → Themes and Home), read once per visit.
#[derive(Clone)]
pub(super) struct ThemeGallery {
    dark: bool,
    themes: Rc<[(catalog::ThemeDef, MandaraPalette, MandaraPalette)]>,
}

impl ThemeGallery {
    /// Each theme with its palette in the current mode.
    pub(super) fn themes_iter(&self) -> impl Iterator<Item = (&catalog::ThemeDef, &MandaraPalette)> {
        self.themes.iter().map(|(def, now, _)| (def, now))
    }
}

thread_local! {
    /// Wallpaper scheme strips, by lead seed and mode: one palette per
    /// [`SchemeVariant::ALL`] entry.
    static STYLE_PALETTES: RefCell<StylePalettes> = RefCell::new(HashMap::new());
}

/// Scheme palettes by (lead seed as RGBA, dark).
type StylePalettes = HashMap<([u8; 4], bool), Rc<[MandaraPalette]>>;

fn style_palettes(seed: Color, dark: bool) -> Rc<[MandaraPalette]> {
    STYLE_PALETTES.with(|cache| {
        cache.borrow_mut().entry((seed.to_rgba8(), dark)).or_insert_with(|| SchemeVariant::ALL.iter().map(|v| mandara_theme::material::palette_from_seed(seed, *v, dark)).collect()).clone()
    })
}

/// Makes `id` the theme (the wallpaper sync would switch straight back,
/// so it turns off).
pub(super) fn apply_theme(id: String, cx: &mut App) {
    update_config(cx, |c| {
        c.theme.source = ThemeSource::Preset { preset: id };
        c.wallpaper.match_colors = false;
    });
}

/// A palette's signature colors, for strips (accents only: surfaces would
/// vanish into the card they sit on).
fn strip_colors(tp: &MandaraPalette) -> [Color; 5] {
    [tp.primary, tp.accent_gradient_end, tp.secondary, tp.tertiary, tp.primary_container]
}

/// A rounded strip of equal color segments.
fn palette_strip(colors: &[Color], h: f32, radius: f32) -> gpui::Div {
    div().h(px(h)).w_full().rounded(px(radius)).overflow_hidden().ltr_row().children(colors.iter().map(|c| div().flex_1().h_full().bg(c.hsla())))
}

/// A section's heading: title, a line of detail and controls at the end.
pub(super) fn section_head(title: impl Into<SharedString>, detail: impl Into<SharedString>, trailing: Vec<AnyElement>, display: SharedString, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .flex_wrap()
        .items_end()
        .gap(px(SPACE_12_PX))
        .px(px(SPACE_4_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(DIM_240_PX))
                .row()
                .flex_col()
                .cross_start()
                .gap(px(SPACE_3_PX))
                .child(div().font_family(display).text_size(px(TEXT_18_PX)).font_weight(FontWeight::MEDIUM).child(title.into()))
                .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(detail))),
        )
        .children(trailing)
        .into_any_element()
}

/// A mock of the shell in `tp`, `w` × `h` logical px: the bar, desktop
/// clock, panel and dock, plus (with `detail`) the date and a media card. With a
/// wallpaper thumbnail it fills the sky; otherwise a tinted gradient does.
pub(super) fn shell_mock(tp: &MandaraPalette, wallpaper: Option<PathBuf>, w: f32, h: f32, radius: f32, detail: bool) -> gpui::Div {
    let k = w / MOCK_REF_W_PX;
    let s = move |v: f32| px(v * k);
    let frame = mandara_ui::shell::frame_color(tp).hsla();
    let accent = mandara_core::oklab_gradient(HORIZONTAL_DEG, tp.accent_gradient_start, tp.accent_gradient_end);
    let line = |w: f32, alpha: f32| div().w(s(w)).h(s(MOCK_LINE_H)).rounded_full().bg(tp.on_surface.with_alpha(alpha).hsla());
    let mut root = div().relative().w(px(w)).h(px(h)).flex_none().rounded(px(radius)).overflow_hidden().bg(mandara_core::oklab_gradient(DIAGONAL_DEG, tp.surface_dim, tp.surface_dim.mix(tp.primary, MOCK_SKY_TINT)));
    if let Some(wall) = wallpaper {
        root = root.child(img(wall).absolute().top_0().left_0().w(px(w)).h(px(h)).object_fit(gpui::ObjectFit::Cover));
    }
    // The bar: launcher and workspaces, the clock centred, status at the end.
    let workspaces = div()
        .row()
        .items_center()
        .gap(s(MOCK_GAP))
        .child(div().size(s(MOCK_DOT)).rounded_full().bg(tp.primary.hsla()))
        .child(div().w(s(MOCK_WS_ACTIVE_W)).h(s(MOCK_DOT)).rounded_full().bg(tp.primary.with_alpha(ALPHA_70).hsla()))
        .children((0..2).map(|_| div().size(s(MOCK_DOT)).rounded_full().bg(tp.on_surface.with_alpha(ALPHA_35).hsla())));
    let status = div().row().items_center().gap(s(MOCK_GAP)).children((0..3).map(|_| div().w(s(MOCK_STATUS_W)).h(s(MOCK_STATUS_H)).rounded_full().bg(tp.on_surface.with_alpha(ALPHA_45).hsla()))).child(div().size(s(MOCK_STATUS_H)).rounded_full().bg(tp.tertiary.hsla()));
    root = root.child(
        div()
            .absolute()
            .top(s(MOCK_BAR_TOP))
            .left(s(MOCK_INSET))
            .right(s(MOCK_INSET))
            .h(s(MOCK_BAR_H))
            .rounded_full()
            .bg(frame)
            .px(s(MOCK_BAR_PAD))
            .row()
            .items_center()
            .justify_between()
            .child(workspaces)
            .child(status)
            .child(div().absolute().inset_0().ltr_row().items_center().justify_center().text_size(s(MOCK_BAR_TEXT)).font_weight(FontWeight::MEDIUM).font_features(mandara_ui::fonts::tabular_nums()).text_color(tp.on_surface.hsla()).child(MOCK_TIME)),
    );
    let date = detail.then(|| mandara_ui::widgets::clock::short_date(&chrono::Local::now()));
    // Too small to read in a thumbnail-sized mock.
    root = root.when(k >= MOCK_CLOCK_MIN_SCALE, |root| root.child(
        div()
            .absolute()
            .inset_start(s(MOCK_CLOCK_LEFT))
            .top(s(MOCK_CLOCK_TOP))
            .row()
            .flex_col()
            .cross_start()
            .child(div().text_size(s(MOCK_CLOCK_TEXT)).line_height(s(MOCK_CLOCK_TEXT)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).text_color(tp.on_surface.with_alpha(ALPHA_90).hsla()).child(MOCK_TIME))
            .children(date.map(|d| div().text_size(s(MOCK_DATE_TEXT)).text_color(tp.on_surface.with_alpha(ALPHA_70).hsla()).child(d))),
    ));
    if detail {
        root = root.child(
            div()
                .absolute()
                .inset_start(s(MOCK_INSET))
                .bottom(s(MOCK_MEDIA_BOTTOM))
                .w(s(MOCK_MEDIA_W))
                .h(s(MOCK_MEDIA_H))
                .rounded(s(RADIUS_14_PX))
                .bg(frame)
                .p(s(SPACE_8_PX))
                .row()
                .items_center()
                .gap(s(SPACE_8_PX))
                .child(div().size(s(MOCK_ART)).flex_none().rounded(s(RADIUS_8_PX)).bg(accent))
                .child(div().flex_1().row().flex_col().cross_start().gap(s(MOCK_GAP)).child(line(MOCK_LINE_W[2], ALPHA_80)).child(line(MOCK_LINE_W[1], ALPHA_40))),
        );
    }
    // The panel: a header, four quick tiles and a slider.
    let panel_h = (MOCK_PANEL_H * k).min(h - (MOCK_PANEL_TOP + MOCK_PANEL_CLEAR) * k).max(0.0);
    let tile = |on: bool| div().flex_1().h(s(MOCK_TILE_H)).rounded(s(MOCK_TILE_RADIUS)).bg(if on { tp.primary.hsla() } else { tp.surface_container_high.hsla() });
    root = root.child(
        div()
            .absolute()
            .inset_end(s(MOCK_INSET))
            .top(s(MOCK_PANEL_TOP))
            .w(s(MOCK_PANEL_W))
            .h(px(panel_h))
            .rounded(s(RADIUS_16_PX))
            .overflow_hidden()
            .bg(frame)
            .p(s(MOCK_PANEL_PAD))
            .row()
            .flex_col()
            .gap(s(MOCK_PANEL_GAP))
            .child(div().row().items_center().gap(s(SPACE_6_PX)).child(div().size(s(MOCK_AVATAR)).flex_none().rounded_full().bg(accent)).child(div().row().flex_col().cross_start().gap(s(MOCK_GAP)).child(line(MOCK_LINE_W[0], ALPHA_70)).child(line(MOCK_LINE_W[1], ALPHA_35))))
            .child(div().row().gap(s(SPACE_6_PX)).child(tile(true)).child(tile(false)))
            .child(div().row().gap(s(SPACE_6_PX)).child(tile(false)).child(tile(true)))
            .child(div().h(s(MOCK_LINE_H)).w_full().rounded_full().bg(tp.surface_container_high.hsla()).child(div().h_full().w(s((MOCK_PANEL_W - 2.0 * MOCK_PANEL_PAD) * MOCK_SLIDER_FILL)).rounded_full().bg(accent))),
    );
    // The dock.
    let icons = [tp.primary, tp.secondary, tp.tertiary, tp.primary_container, tp.surface_container_high, tp.secondary.mix(tp.tertiary, ALPHA_50)];
    root.child(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(s(MOCK_DOCK_BOTTOM))
            .ltr_row()
            .justify_center()
            .child(div().h(s(MOCK_DOCK_H)).p(s(MOCK_DOCK_PAD)).rounded(s(RADIUS_12_PX)).bg(frame).row().gap(s(SPACE_6_PX)).children(icons.iter().map(|c| div().size(s(MOCK_DOCK_ICON)).rounded(s(RADIUS_6_PX)).bg(c.hsla())))),
    )
}

impl StudioView {
    /// The catalog with both modes' palettes, computed once per visit.
    pub(super) fn theme_gallery(&mut self, dark: bool) -> ThemeGallery {
        match &self.themes {
            Some(g) if g.dark == dark => g.clone(),
            _ => {
                let g = ThemeGallery { dark, themes: catalog::all().into_iter().map(|t| { let (now, other) = (t.palette(dark), t.palette(!dark)); (t, now, other) }).collect() };
                self.themes = Some(g.clone());
                g
            }
        }
    }

    /// The current wallpaper's thumbnail and seed colors, made off the UI
    /// thread once per wallpaper (`None` / empty until ready).
    pub(super) fn wallpaper_assets(&mut self, path: &PathBuf, cx: &mut Context<Self>) -> (Option<PathBuf>, Rc<[Color]>) {
        let have = self.wallpaper_seeds.as_ref().is_some_and(|(p, _)| p == path);
        if !have && self.seeds_loading.as_ref() != Some(path) {
            self.seeds_loading = Some(path.clone());
            let job = path.clone();
            let done = path.clone();
            let task = cx.background_executor().spawn(async move {
                let thumb = mandara_ui::wallpaper::thumbnail(&job).ok();
                let seeds = mandara_theme::material::extract_seeds(&job, WALLPAPER_SEEDS).map(|s| s.into_iter().map(|s| s.color).collect::<Vec<_>>());
                (thumb, seeds)
            });
            cx.spawn(async move |this, cx| {
                let (thumb, result) = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.seeds_loading = None;
                    if let Some(thumb) = thumb {
                        this.wallpaper_thumb = Some((done.clone(), thumb));
                    }
                    if let Ok(seeds) = result {
                        this.wallpaper_seeds = Some((done, Rc::from(seeds)));
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        let thumb = self.wallpaper_thumb.as_ref().filter(|(wall, _)| wall == path).map(|(_, thumb)| thumb.clone());
        let seeds = self.wallpaper_seeds.as_ref().filter(|(p, _)| p == path).map(|(_, s)| s.clone()).unwrap_or_else(|| Rc::from([]));
        (thumb, seeds)
    }

    /// The live preview: the shell mock in the current palette over the
    /// wallpaper, with the theme's name, palette, contrast and light / dark.
    pub(super) fn theme_preview(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let theme = mandara_core::theme_entity(cx).read(cx).target().name.clone();
        let thumb = cfg.wallpaper.path.clone().and_then(|path| self.wallpaper_assets(&path, cx).0);
        let detail: String = match &cfg.theme.source {
            ThemeSource::Preset { preset } => catalog::all().into_iter().find(|t| t.id == *preset).map(|t| tr(&t.description)).unwrap_or_default(),
            ThemeSource::Wallpaper { variant, .. } => tf("Generated from your wallpaper · {style}", &[("style", &tr(variant.display_name()))]),
            _ => String::new(),
        };
        let mode = registry::by_path(THEME_MODE_PATH).map(|setting| self.control(setting, cfg, p, cx));
        let contrast = p.on_surface.contrast_ratio(p.surface_base);
        let split = self.page.column_w >= HERO_SPLIT_MIN_W_PX;
        let inner_w = self.page.column_w - 2.0 * SPACE_16_PX;
        let grows = HERO_MOCK_GROW + HERO_INFO_GROW;
        let mock_w = if split { ((inner_w - SPACE_20_PX) * HERO_MOCK_GROW / grows).floor() } else { inner_w };
        let mock = shell_mock(p, thumb, mock_w, (mock_w * HERO_MOCK_ASPECT).floor(), RADIUS_18_PX, true);
        let info = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .cross_start()
            .justify_center()
            .gap(px(SPACE_12_PX))
            .p(px(SPACE_6_PX))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Current theme")))
            .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_26_PX)).font_weight(FontWeight::SEMIBOLD).child(theme.to_string()))
            .when(!detail.is_empty(), |el| el.child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(detail))))
            .child(palette_strip(&strip_colors(p), STRIP_H_PX, RADIUS_10_PX))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_6_PX))
                    .child(super::compositor_page::badge(tf("Text contrast {ratio}:1", &[("ratio", &format!("{contrast:.1}"))]), if contrast >= mandara_theme::palette::HC_MIN_CONTRAST { p.success_emerald } else { p.primary })),
            )
            .children(mode);
        div()
            .rounded(px(RADIUS_26_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .p(px(SPACE_16_PX))
            .row()
            .when(!split, |el| el.flex_col())
            .gap(px(SPACE_20_PX))
            .child(mock)
            .child(info)
            .into_any_element()
    }

    /// The designed themes grid (current mode, the other mode inset).
    pub(super) fn designed_themes(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let dark = cfg.theme.mode.is_dark();
        let active = match &cfg.theme.source {
            ThemeSource::Preset { preset } => Some(preset.clone()),
            _ => None,
        };
        let gallery = self.theme_gallery(dark);
        let count = gallery.themes.len();
        let fit = grid::fit(self.page.column_w, THEME_GRID);
        let preview_w = (fit.card_w - 2.0 * (SPACE_8_PX + BORDER_2_PX)).max(0.0);
        let preview_h = fit.aspect_h(THEME_PREVIEW_ASPECT, SPACE_8_PX + BORDER_2_PX);
        let inset_w = (preview_w * INSET_SHARE).floor();
        let inset_h = (inset_w * preview_h / preview_w.max(1.0)).floor();
        let cards = gallery.themes.iter().map(|(def, tp, other)| {
            let is_active = active.as_deref() == Some(def.id.as_str());
            let id = def.id.clone();
            let inset = div()
                .absolute()
                .inset_start(px(SPACE_8_PX))
                .bottom(px(SPACE_8_PX))
                .rounded(px(RADIUS_10_PX))
                .p(px(INSET_RING_PX))
                .bg(tp.surface_base.hsla())
                .child(shell_mock(other, None, inset_w, inset_h, RADIUS_8_PX, false))
                .child(div().absolute().top(px(SPACE_4_PX)).inset_start(px(SPACE_4_PX)).size(px(DIM_16_PX)).rounded_full().bg(other.surface_base.hsla()).row().items_center().justify_center().child(icon(if dark { "sun" } else { "moon" }, ICON_10_PX, other.primary)));
            div()
                .id(SharedString::from(format!("theme-{}", def.id)))
                .flex_1()
                .p(px(SPACE_8_PX))
                .rounded(px(RADIUS_22_PX))
                .cursor_pointer()
                .bg(tp.surface_base.hsla())
                .border_2()
                .border_color(if is_active { p.primary.hsla() } else { tp.outline.with_alpha(ALPHA_18).hsla() })
                .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_55).hsla()))
                .child(
                    div()
                        .relative()
                        .child(shell_mock(tp, None, preview_w, preview_h, RADIUS_15_PX, false))
                        .child(inset)
                        .when(is_active, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .bottom(px(SPACE_8_PX))
                                    .inset_end(px(SPACE_8_PX))
                                    .px(px(SPACE_8_PX))
                                    .h(px(DIM_20_PX))
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .gap(px(SPACE_4_PX))
                                    .bg(p.primary.hsla())
                                    .text_color(p.on_primary.hsla())
                                    .text_size(px(TEXT_10_5_PX))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(icon("check", ICON_11_PX, p.on_primary))
                                    .child(t("Active")),
                            )
                        }),
                )
                .child(
                    div()
                        .px(px(SPACE_6_PX))
                        .pt(px(SPACE_12_PX))
                        .pb(px(SPACE_6_PX))
                        .row()
                        .flex_col()
                        .gap(px(SPACE_8_PX))
                        .child(div().truncate().text_start().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).text_color(tp.on_surface.hsla()).child(def.name.clone()))
                        .child(palette_strip(&strip_colors(tp), CARD_STRIP_H_PX, RADIUS_4_PX))
                        .child(div().h(px(DIM_32_PX)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(tp.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(tr(&def.description)))),
                )
                .on_click(move |_: &ClickEvent, _, cx| apply_theme(id.clone(), cx))
        });
        let folder = apps_page::tilde(&catalog::user_theme_dir());
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Designed themes"), tf("{count} themes · add your own in {folder}", &[("count", &count), ("folder", &folder)]), Vec::new(), fonts.display.clone(), p))
            .child(fit.render(cards))
            .into_any_element()
    }

    /// Material You from the wallpaper: on / off, the thumbnail, the seed
    /// swatches (pick the lead) and the scheme styles as palette strips.
    pub(super) fn wallpaper_schemes(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let (on, variant, seed) = match &cfg.theme.source {
            ThemeSource::Wallpaper { variant, seed, .. } => (true, *variant, *seed),
            _ => (false, SchemeVariant::default(), 0),
        };
        let toggle = div().id("wall-colors-toggle").cursor_pointer().child(mandara_ui::plugins::network::switch(on, p)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| update_config(cx, |c| c.wallpaper.match_colors = !on))).into_any_element();
        let head = section_head(t("Colors from your wallpaper"), t("Material You: a full tonal palette generated from the wallpaper, re-derived whenever it changes"), vec![toggle], fonts.display.clone(), p);
        let Some(path) = cfg.wallpaper.path.clone() else {
            let empty = div()
                .rounded(px(RADIUS_22_PX))
                .bg(p.surface_container.hsla())
                .p(px(SPACE_20_PX))
                .text_color(p.on_surface_variant.hsla())
                .child(mandara_ui::bidi::paragraph(t("Pick a wallpaper (bar → wallpaper button, or `mandara msg wallpaper picker open`) to theme Mandara from its colors.")));
            return div().row().flex_col().gap(px(SPACE_14_PX)).child(head).child(empty).into_any_element();
        };
        let (thumb, seeds) = self.wallpaper_assets(&path, cx);
        let dark = cfg.theme.mode.is_dark();
        let swatches = seeds.iter().enumerate().map(|(i, c)| {
            let active = on && i == seed;
            let check = c.best_on(p.on_surface, p.surface_base);
            div()
                .id(("seed", i))
                .size(px(SEED_SWATCH_PX))
                .rounded_full()
                .cursor_pointer()
                .bg(c.hsla())
                .border_3()
                .border_color(if active { p.on_surface.hsla() } else { p.surface_container.hsla() })
                .hover(|s| s.border_color(p.on_surface.with_alpha(ALPHA_50).hsla()))
                .row()
                .items_center()
                .justify_center()
                .when(active, |el| el.child(icon("check", ICON_16_PX, check)))
                .on_click(move |_: &ClickEvent, _, cx| {
                    update_config(cx, |c| {
                        if let ThemeSource::Preset { preset } = &c.theme.source {
                            c.theme.last_preset = Some(preset.clone());
                        }
                        c.wallpaper.match_colors = true;
                        if let Some(wp) = c.wallpaper.path.clone() {
                            c.theme.source = ThemeSource::Wallpaper { path: wp, variant, seed: i };
                        }
                    });
                })
        });
        // Each style as the palette it would make from the lead seed.
        let lead = seeds.get(seed).or(seeds.first()).copied();
        let strips = lead.map(|c| style_palettes(c, dark));
        let styles = SchemeVariant::ALL.into_iter().enumerate().map(|(n, v)| {
            let active = on && v == variant;
            let colors = strips.as_ref().map(|s| strip_colors(&s[n]));
            div()
                .id(SharedString::from(format!("style-{v:?}")))
                .flex_1()
                .p(px(SPACE_6_PX))
                .rounded(px(RADIUS_14_PX))
                .cursor_pointer()
                .row()
                .flex_col()
                .gap(px(SPACE_6_PX))
                .bg(if active { p.primary.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
                .border_2()
                .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
                .child(match colors {
                    Some(colors) => palette_strip(&colors, STYLE_STRIP_H_PX, RADIUS_9_PX),
                    None => div().h(px(STYLE_STRIP_H_PX)).w_full().rounded(px(RADIUS_9_PX)).bg(p.surface_container.hsla()),
                })
                .child(div().px(px(SPACE_4_PX)).truncate().text_start().text_size(px(TEXT_12_5_PX)).font_weight(if active { FontWeight::MEDIUM } else { FontWeight::NORMAL }).child(tr(v.display_name())))
                .on_click(move |_: &ClickEvent, _, cx| {
                    update_config(cx, |c| {
                        if let ThemeSource::Preset { preset } = &c.theme.source {
                            c.theme.last_preset = Some(preset.clone());
                        }
                        c.wallpaper.match_colors = true;
                        let seed = match &c.theme.source {
                            ThemeSource::Wallpaper { seed, .. } => *seed,
                            _ => 0,
                        };
                        if let Some(wp) = c.wallpaper.path.clone() {
                            c.theme.source = ThemeSource::Wallpaper { path: wp, variant: v, seed };
                        }
                    });
                })
        });
        let stack = self.stack_rows;
        let inner_w = self.page.column_w - 2.0 * SPACE_16_PX;
        let thumb_w = if stack { inner_w } else { WALLPAPER_THUMB_W_PX };
        let thumb_h = (thumb_w * WALLPAPER_THUMB_ASPECT).floor();
        let controls_w = if stack { inner_w } else { inner_w - WALLPAPER_THUMB_W_PX - SPACE_20_PX };
        let style_fit = grid::fit(controls_w, STYLE_GRID).balanced(SchemeVariant::ALL.len());
        let label = |text: String| div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(text);
        let body = div()
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_16_PX))
            .row()
            .when(stack, |el| el.flex_col())
            .gap(px(if stack { SPACE_14_PX } else { SPACE_20_PX }))
            .child(
                div()
                    .w(px(thumb_w))
                    .h(px(thumb_h))
                    .flex_none()
                    .relative()
                    .overflow_hidden()
                    .rounded(px(RADIUS_16_PX))
                    .bg(p.surface_container_high.hsla())
                    // Sized explicitly so the box, not the image's natural
                    // size, decides what it covers.
                    .when_some(thumb, |el, t| el.child(img(t).absolute().top_0().left_0().w(px(thumb_w)).h(px(thumb_h)).rounded(px(RADIUS_16_PX)).object_fit(gpui::ObjectFit::Cover))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .cross_start()
                    .gap(px(SPACE_10_PX))
                    .child(label(t("Lead color").into()))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).children(swatches).when(seeds.is_empty(), |el| el.child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Extracting…")))))
                    .child(div().pt(px(SPACE_4_PX)).child(label(t("Style").into())))
                    .child(div().w_full().child(style_fit.render(styles))),
            );
        div().row().flex_col().gap(px(SPACE_14_PX)).child(head).child(body).into_any_element()
    }
}
