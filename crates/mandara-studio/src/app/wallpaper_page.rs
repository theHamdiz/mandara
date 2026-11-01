//! Studio → Wallpaper: the page opens on the wallpaper you have, then
//! tabs for everything else.
//!
//! * **Wallpaper** (always first, and where the page always opens): a
//!   large card of the wallpaper on your desktop at its own shape (name,
//!   resolution, size, folder, its colors from the theme engine, quick
//!   actions), then every wallpaper in the same folder as a grid of
//!   thumbnails. The grid is paged (only one page of tiles exists at a
//!   time), thumbnails come from the shared thumbnail cache and are decoded
//!   at the size they are drawn, the name filter narrows it, the arrow
//!   keys move through it and Enter or a click applies (with the desktop's
//!   transition). Picker settings follow.
//! * **Slideshow**: an on / off card with the next change, previous / next,
//!   interval presets and a custom interval, the order, then folds for the
//!   source (folders, light / dark folders, playlist), when to change
//!   (startup, pauses, hours), the transition and per-display behaviour.
//!   The engine runs in the shell (`mandara_ui::slideshow`); Studio asks it
//!   for its status only while this tab shows.
//! * **Fit & effects**: every other wallpaper setting (fill, transition,
//!   light / dark wallpapers, video, depth). New wallpaper settings land
//!   here unless a tab claims their group; effects with their own cards
//!   add them through [`StudioView::wallpaper_effects_sections`].
//! * **Generator** (with the wallpaper generator plugin on) and **Online**
//!   (with the Wallhaven plugin on).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, Context, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    StyledImage as _, Subscription, Task, Window, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::desktop::{SlideshowSource, WallpaperSlideshow, defaults, effective_wallpaper, parse_interval};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::wallpaper_generator as wg;
use mandara_ui::state::update_config;
use mandara_ui::style::ClipRounded as _;
use mandara_ui::wallpaper::Preview;
use serde_json::Value;

use super::compositor_page::{button, chip, tip};
use super::themes_page::section_head;
use super::{StudioView, form, grid};
use crate::registry::{self, Section};
use crate::tokens::*;

/// Registry groups that live on a tab of their own; every other group of
/// the Wallpaper section shows under Fit & effects.
const PICKER_GROUP: &str = "Picker";
const ONLINE_GROUP: &str = "Wallhaven";
/// The Slideshow tab's header card (drawn by hand, not as folds).
const SLIDESHOW_HEAD_GROUP: &str = "Slideshow";
const SLIDESHOW_GROUPS: &[&str] = &["Source", "When to change", "Slideshow transition", "Displays"];

/// The current wallpaper card: picture and details side by side from this
/// column width (else stacked), the picture taking this share of the row.
const HERO_SPLIT_MIN_W_PX: f32 = 2.0 * DIM_320_PX;
const HERO_PICTURE_SHARE: f32 = 0.6;
const HERO_PAD_PX: f32 = SPACE_16_PX;
const HERO_GAP_PX: f32 = SPACE_20_PX;
/// The picture keeps the wallpaper's shape within these heights per unit
/// of width (very tall or very wide wallpapers are letterboxed).
const HERO_MIN_RATIO: f32 = 0.3;
const HERO_MAX_RATIO: f32 = 0.8;
/// Shape assumed until the wallpaper's size is known (16:9).
const DEFAULT_RATIO: f32 = 9.0 / 16.0;
/// Direction of the card's wash, degrees.
const WASH_DEG: f32 = 135.0;
/// Palette swatches under the name.
const SWATCH_PX: f32 = DIM_28_PX;
/// An aspect ratio is named ("16:9") while both terms are this small;
/// otherwise it reads as a decimal ("2.39:1").
const NAMED_RATIO_MAX: u32 = 32;

/// The folder's thumbnails: card sizes, the thumbnail cache's 16:10 shape
/// and the rows on one page of the grid.
const TILE_GRID: CardGrid = CardGrid { min_w: DIM_150_PX, max_w: DIM_240_PX, gap: SPACE_12_PX };
const TILE_RATIO: f32 = 10.0 / 16.0;
const PAGE_ROWS: usize = 4;
/// The playlist's smaller tiles.
const PLAYLIST_GRID: CardGrid = CardGrid { min_w: DIM_120_PX, max_w: DIM_180_PX, gap: SPACE_10_PX };
/// Round buttons on tiles.
const TILE_BUTTON_PX: f32 = DIM_26_PX;
/// The selected tab's underline.
const TAB_UNDERLINE_PX: f32 = 2.0;
/// The name filter's width.
const SEARCH_W_PX: f32 = DIM_220_PX;
/// The custom interval field's width.
const INTERVAL_FIELD_W_PX: f32 = DIM_140_PX;
/// After the next change was due, ask the shell again this much later.
const STATUS_SLACK: Duration = Duration::from_secs(2);
/// Seconds in a day (past it, the next change shows its weekday).
const S_PER_DAY: u64 = 24 * 60 * 60;
/// Minutes per hour and day (interval labels).
const MIN_PER_HOUR: u32 = 60;
const MIN_PER_DAY: u32 = 24 * 60;

/// The Wallpaper page's tabs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum WallpaperTab {
    #[default]
    Wallpaper,
    Slideshow,
    Effects,
    Generator,
    Online,
}

impl WallpaperTab {
    pub const ALL: [WallpaperTab; 5] = [WallpaperTab::Wallpaper, WallpaperTab::Slideshow, WallpaperTab::Effects, WallpaperTab::Generator, WallpaperTab::Online];

    /// Deep-link name (`mandara studio wallpaper --tab slideshow`).
    pub fn slug(self) -> &'static str {
        match self {
            WallpaperTab::Wallpaper => "wallpaper",
            WallpaperTab::Slideshow => "slideshow",
            WallpaperTab::Effects => "effects",
            WallpaperTab::Generator => "generator",
            WallpaperTab::Online => "online",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|t| t.slug() == s).or(match s.as_str() {
            "current" | "library" | "folder" | "wallpapers" | "picker" => Some(WallpaperTab::Wallpaper),
            "slides" | "cycle" | "timer" => Some(WallpaperTab::Slideshow),
            "fit" | "fit-effects" | "fit-and-effects" | "depth" | "video" | "light-dark" | "desktop" => Some(WallpaperTab::Effects),
            "wallgen" | "generate" => Some(WallpaperTab::Generator),
            "wallhaven" | "web" => Some(WallpaperTab::Online),
            _ => None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            WallpaperTab::Wallpaper => t("Wallpaper"),
            WallpaperTab::Slideshow => t("Slideshow"),
            WallpaperTab::Effects => t("Fit & effects"),
            WallpaperTab::Generator => t("Generator"),
            WallpaperTab::Online => t("Online"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            WallpaperTab::Wallpaper => "image",
            WallpaperTab::Slideshow => "play",
            WallpaperTab::Effects => "sliders",
            WallpaperTab::Generator => "brush",
            WallpaperTab::Online => "globe",
        }
    }
}

/// The tabs shown under `cfg` (Generator and Online need their plugins).
fn available(cfg: &ShellConfig, cx: &App) -> Vec<WallpaperTab> {
    WallpaperTab::ALL
        .into_iter()
        .filter(|tab| match tab {
            WallpaperTab::Generator => wg::enabled(cx),
            WallpaperTab::Online => cfg.wallhaven_enabled(),
            _ => true,
        })
        .collect()
}

/// What the page keeps between frames (a global, dropped on leaving).
#[derive(Default)]
struct Page {
    tab: WallpaperTab,
    /// The folder's wallpapers, read off the UI thread.
    listing: Option<(PathBuf, Rc<[PathBuf]>)>,
    listing_loading: Option<PathBuf>,
    /// The current wallpaper's large copy and facts.
    preview: Option<(PathBuf, Result<Preview, String>)>,
    preview_loading: Option<PathBuf>,
    /// Grid page, keyboard cursor (index into the filtered list), the
    /// filter they belong to, and whether the page was placed on the
    /// current wallpaper yet.
    grid_page: usize,
    cursor: usize,
    query: String,
    placed: bool,
    focus: Option<FocusHandle>,
    thumbs_watch: Option<Subscription>,
    /// Thumbnails drawn (released on leaving) and their size.
    drawn: HashSet<(PathBuf, u32)>,
    scale: f32,
    editing_folder: bool,
    custom_interval: bool,
    /// The shell's slideshow status, when it was asked, and for which
    /// settings.
    status: Option<(Result<Value, String>, Instant)>,
    status_for: Option<(WallpaperSlideshow, Option<PathBuf>)>,
    status_loading: bool,
    status_wake: Option<Task<()>>,
    note: Option<(String, bool)>,
}
impl gpui::Global for Page {}

fn st(cx: &mut App) -> &mut Page {
    cx.default_global::<Page>()
}

/// Leaving the page: drops the listing, the decoded pictures and the
/// status timer; the next visit opens on the Wallpaper tab again.
pub(super) fn release(cx: &mut App) {
    let Some(page) = cx.try_global::<Page>() else { return };
    let scale = page.scale;
    let drawn: Vec<(PathBuf, u32)> = page.drawn.iter().cloned().collect();
    let preview = page.preview.as_ref().and_then(|(_, r)| r.as_ref().ok()).map(|p| p.file.clone());
    for (thumb, w) in drawn {
        mandara_ui::icon_raster::forget_cached(&thumb, w as f32, scale, cx);
    }
    if let Some(file) = preview {
        mandara_ui::image_cache::forget_path(&file, cx);
    }
    cx.remove_global::<Page>();
}

/// Text for an interval: "5 min", "1 h 30 min", "1 day".
fn interval_label(minutes: u32) -> String {
    let (d, h, m) = (minutes / MIN_PER_DAY, (minutes % MIN_PER_DAY) / MIN_PER_HOUR, minutes % MIN_PER_HOUR);
    let mut parts = Vec::new();
    if d > 0 {
        parts.push(if d == 1 { t("1 day").to_string() } else { tf("{count} days", &[("count", &d)]) });
    }
    if h > 0 {
        parts.push(format!("{h} {}", t("h")));
    }
    if m > 0 || parts.is_empty() {
        parts.push(format!("{m} {}", t("min")));
    }
    parts.join(" ")
}

/// "16:9", or "2.39:1" for shapes without a short name.
fn aspect_label((w, h): (u32, u32)) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w, h).max(1);
    let (a, b) = (w / g, h / g);
    if a <= NAMED_RATIO_MAX && b <= NAMED_RATIO_MAX { format!("{a}:{b}") } else { format!("{:.2}:1", w as f32 / h.max(1) as f32) }
}

/// The local time `secs` from now ("14:30", or "Tue 14:30" past a day).
fn clock_in(secs: u64, h24: bool) -> String {
    let at = chrono::Local::now() + chrono::Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX / 2));
    let time = if h24 { "%H:%M" } else { "%-I:%M %p" };
    let pattern = if secs >= S_PER_DAY { format!("%a {time}") } else { time.to_string() };
    mandara_ui::bidi::ltr_isolate(&at.format(&pattern).to_string())
}

/// A file's name without its folder.
fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// A glass tag over a picture (readable on any image).
fn tag(icon_name: &'static str, text: impl Into<SharedString>, p: &MandaraPalette) -> gpui::Div {
    div()
        .h(px(DIM_22_PX))
        .px(px(SPACE_8_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_4_PX))
        .bg(p.surface_base.with_alpha(ALPHA_85).hsla())
        .text_color(p.on_surface.hsla())
        .text_size(px(TEXT_11_PX))
        .font_weight(FontWeight::MEDIUM)
        .child(icon(icon_name, ICON_10_PX, p.primary))
        .child(text.into())
}

/// A fact about the wallpaper (resolution, size…).
fn fact(text: impl Into<SharedString>, p: &MandaraPalette) -> gpui::Div {
    div()
        .h(px(DIM_24_PX))
        .px(px(SPACE_10_PX))
        .rounded_full()
        .row()
        .items_center()
        .bg(p.on_surface.with_alpha(ALPHA_8).hsla())
        .text_size(px(TEXT_11_5_PX))
        .font_features(mandara_ui::fonts::tabular_nums())
        .child(text.into())
}

/// A centred placeholder for a picture that isn't ready (or failed).
fn placeholder(glyph: &'static str, p: &MandaraPalette) -> AnyElement {
    div().size_full().row().items_center().justify_center().child(icon(glyph, ICON_22_PX, p.on_surface_variant.with_alpha(ALPHA_50))).into_any_element()
}

impl StudioView {
    /// Opens the Wallpaper page on `tab` (deep links).
    pub fn show_wallpaper_tab(&mut self, tab: WallpaperTab, cx: &mut Context<Self>) {
        self.show(Section::Wallpaper, cx);
        self.set_wallpaper_tab(tab, cx);
    }

    fn set_wallpaper_tab(&mut self, tab: WallpaperTab, cx: &mut Context<Self>) {
        let from = st(cx).tab;
        if from == WallpaperTab::Generator && tab != from {
            // The generator's previews go with its tab.
            super::wallgen_page::release(cx);
        }
        let page = st(cx);
        page.tab = tab;
        page.note = None;
        self.content_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// The whole Wallpaper page.
    pub(super) fn wallpaper_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tabs = available(cfg, cx);
        let tab = Some(st(cx).tab).filter(|t| tabs.contains(t)).unwrap_or_default();
        let bar = self.wallpaper_tab_bar(&tabs, tab, p, cx);
        let note = st(cx).note.clone().map(|(text, error)| form::note(&text, error, p));
        let body = match tab {
            WallpaperTab::Wallpaper => self.wp_current_tab(cfg, p, window, cx),
            WallpaperTab::Slideshow => self.wp_slideshow_tab(cfg, p, cx),
            WallpaperTab::Effects => self.wp_effects_tab(cfg, p, cx),
            WallpaperTab::Generator => self.wallgen_panel(cfg, p, cx),
            WallpaperTab::Online => self.wp_online_tab(cfg, p, cx),
        };
        div().row().flex_col().gap(px(SPACE_22_PX)).child(bar).children(note).child(body).into_any_element()
    }

    /// One slim row of underlined tabs (it scrolls sideways when narrow).
    fn wallpaper_tab_bar(&mut self, tabs: &[WallpaperTab], current: WallpaperTab, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let mut bar = div().id("wallpaper-tabs").row().overflow_x_scroll().border_b_1().border_color(p.outline.with_alpha(ALPHA_35).hsla());
        for &tab in tabs {
            let on = tab == current;
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("wallpaper-tab-{}", tab.slug())))
                    .relative()
                    .flex_none()
                    .h(px(DIM_36_PX))
                    .px(px(SPACE_10_PX))
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .cursor_pointer()
                    .tab_index(0)
                    .text_size(px(TEXT_13_PX))
                    .when(on, |el| el.text_color(p.on_surface.hsla()).font_weight(FontWeight::SEMIBOLD))
                    .when(!on, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.text_color(p.on_surface.hsla())))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_wallpaper_tab(tab, cx)))
                    .child(icon(tab.icon(), ICON_14_PX, if on { p.primary } else { p.on_surface_variant }))
                    .child(tab.label())
                    .when(on, |el| el.child(div().absolute().bottom_0().left(px(SPACE_8_PX)).right(px(SPACE_8_PX)).h(px(TAB_UNDERLINE_PX)).rounded_full().bg(p.primary.hsla()))),
            );
        }
        bar.into_any_element()
    }

    // ── Wallpaper tab ────────────────────────────────────────────────────

    fn wp_current_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let shown = effective_wallpaper(cfg.wallpaper.path.as_ref(), &cfg.wallpaper.variants, None, cfg.theme.mode.is_dark());
        let hero = self.wp_hero(cfg, shown.as_ref(), p, cx);
        let folder = st(cx).editing_folder.then(|| self.wp_folder_editor(cfg, p, cx));
        let library = self.wp_library(cfg, shown.as_ref(), p, window, cx);
        let picker = self.rows(registry::in_section(Section::Wallpaper).filter(|s| s.group == PICKER_GROUP), cfg, p, cx);
        // The opt-in sources, offered while off (their tabs appear once on).
        let mut offers = Vec::new();
        if !wg::enabled(cx) {
            offers.push(self.wallgen_panel(cfg, p, cx));
        }
        if !cfg.wallhaven_enabled() {
            offers.push(self.plugin_offer(mandara_ui::plugins::wallhaven::ID, p, cx));
        }
        let more = (!offers.is_empty()).then(|| div().row().flex_col().gap(px(SPACE_10_PX)).child(super::compositor_page::group_title(t("More wallpapers"), p)).children(offers));
        div().row().flex_col().gap(px(SPACE_26_PX)).child(hero).children(folder).child(library).child(picker).children(more).into_any_element()
    }

    /// Reads the current wallpaper's facts and large copy off the UI thread.
    fn wp_load_preview(&mut self, path: &Path, cx: &mut Context<Self>) {
        let page = st(cx);
        if page.preview.as_ref().is_some_and(|(p, _)| p == path) || page.preview_loading.as_deref() == Some(path) {
            return;
        }
        page.preview_loading = Some(path.to_path_buf());
        let (job, done) = (path.to_path_buf(), path.to_path_buf());
        form::job(cx, move || Ok(mandara_ui::wallpaper::preview(&job)), move |_, res, cx| {
            let page = st(cx);
            if page.preview_loading.as_ref() == Some(&done) {
                page.preview_loading = None;
            }
            let old = page.preview.replace((done, res.and_then(|r| r))).and_then(|(_, r)| r.ok()).map(|p| p.file);
            // The previous wallpaper's copy is no longer drawn.
            if let Some(file) = old {
                mandara_ui::image_cache::forget_path(&file, cx);
            }
        });
    }

    /// The wallpaper on the desktop, large, with what it is and quick
    /// actions.
    fn wp_hero(&mut self, cfg: &ShellConfig, shown: Option<&PathBuf>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let card = div().rounded(px(RADIUS_26_PX)).bg(mandara_core::oklab_gradient(WASH_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35))).p(px(HERO_PAD_PX));
        let Some(path) = shown.cloned() else {
            return card
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_10_PX))
                .py(px(SPACE_30_PX))
                .child(icon("image", ICON_26_PX, p.primary))
                .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::MEDIUM).child(t("No wallpaper yet")))
                .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Pick one from your folder below; it fades in on the desktop.")))
                .into_any_element();
        };
        self.wp_load_preview(&path, cx);
        let preview = st(cx).preview.as_ref().filter(|(w, _)| *w == path).map(|(_, r)| r.clone());
        let seeds = self.wallpaper_assets(&path, cx).1;
        let split = self.page.column_w >= HERO_SPLIT_MIN_W_PX;
        let inner_w = (self.page.column_w - 2.0 * HERO_PAD_PX).max(0.0);
        let pic_w = if split { ((inner_w - HERO_GAP_PX) * HERO_PICTURE_SHARE).floor() } else { inner_w };
        let info = preview.as_ref().and_then(|r| r.as_ref().ok()).cloned();
        let ratio = info.as_ref().map_or(DEFAULT_RATIO, |i| i.size.1 as f32 / i.size.0.max(1) as f32).clamp(HERO_MIN_RATIO, HERO_MAX_RATIO);
        let pic_h = (pic_w * ratio).floor();
        let video = info.as_ref().is_some_and(|i| i.video) || mandara_ui::wallpaper::is_video(&path);
        let picture = div()
            .relative()
            .w(px(pic_w))
            .h(px(pic_h))
            .flex_none()
            .rounded(px(RADIUS_18_PX))
            .clip_rounded()
            .bg(p.surface_dim.hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .child(match &preview {
                Some(Ok(pv)) => img(pv.file.clone()).size_full().object_fit(gpui::ObjectFit::Contain).into_any_element(),
                Some(Err(_)) => placeholder("alert", p),
                None => placeholder("image", p),
            })
            .when(video, |el| el.child(div().absolute().top(px(SPACE_10_PX)).inset_start(px(SPACE_10_PX)).child(tag("play", t("Video"), p))));

        let name = file_name(&path);
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let dark = cfg.theme.mode.is_dark();
        // A light / dark wallpaper set for the current mode is the one shown.
        let themed = if dark { cfg.wallpaper.variants.dark.as_ref() } else { cfg.wallpaper.variants.light.as_ref() };
        let variant = themed.is_some_and(|v| !v.as_os_str().is_empty()).then(|| if dark { t("Dark theme wallpaper") } else { t("Light theme wallpaper") });
        let mut facts: Vec<gpui::Div> = Vec::new();
        if let Some(i) = &info {
            facts.push(fact(mandara_ui::bidi::ltr_isolate(&format!("{} × {}", i.size.0, i.size.1)), p));
            facts.push(fact(aspect_label(i.size), p));
            facts.push(fact(wg::model::human_bytes(i.bytes), p));
        }
        let kind = path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        if !kind.is_empty() {
            facts.push(fact(kind, p));
        }
        if let Some(Err(e)) = &preview {
            facts.push(div().text_size(px(TEXT_11_5_PX)).text_color(p.urgent_coral.hsla()).child(e.clone()));
        }

        // Its colors, as the theme engine reads them.
        let swatches = seeds.iter().enumerate().map(|(i, c): (usize, &Color)| {
            div()
                .id(("wp-seed", i))
                .size(px(SWATCH_PX))
                .rounded_full()
                .bg(c.hsla())
                .border_2()
                .border_color(p.on_surface.with_alpha(ALPHA_25).hsla())
                .tooltip(tip(c.to_hex(), p))
        });
        let matching = cfg.wallpaper.match_colors;
        let colors = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_8_PX))
            .children(swatches)
            .child(
                chip("wp-match-colors", if matching { t("Theme follows these colors") } else { t("Use for my theme") }, matching, p)
                    .child(icon(if matching { "check" } else { "palette" }, ICON_12_PX, if matching { p.on_primary } else { p.primary }))
                    .rounded_full()
                    .when(!matching, |el| el.bg(p.surface_base.with_alpha(ALPHA_50).hsla()).text_color(p.on_surface.hsla()))
                    .tab_index(0)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| update_config(cx, |c| c.wallpaper.match_colors = !c.wallpaper.match_colors))),
            );

        let share = cfg.wallpaper.share_with_login;
        let reveal_dir = folder.clone();
        let actions = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_8_PX))
            .child(button("wp-change-folder", "folder", t("Change folder"), false, p).tab_index(0).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                let page = st(cx);
                page.editing_folder = !page.editing_folder;
                cx.notify();
            })))
            .child(button("wp-open-folder", "external", t("Open folder"), false, p).tab_index(0).on_click(move |_: &ClickEvent, _, _| super::wallgen_page::reveal(&reveal_dir)))
            .child(button("wp-random", "dice", t("Surprise me"), false, p).tab_index(0).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                form::shell(vec!["wallpaper".into(), "random".into()], cx, |_, res, cx| {
                    if let Err(e) = res {
                        st(cx).note = Some((e, true));
                    }
                });
            })))
            .child(
                chip("wp-login", t("Show on the login screen"), share, p)
                    .h(px(DIM_34_PX))
                    .rounded_full()
                    .child(icon(if share { "check" } else { "user" }, ICON_12_PX, if share { p.on_primary } else { p.primary }))
                    .tab_index(0)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| update_config(cx, |c| c.wallpaper.share_with_login = !c.wallpaper.share_with_login))),
            );

        let reveal_folder = folder.clone();
        let details = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .cross_start()
            .justify_center()
            .gap(px(SPACE_12_PX))
            .p(px(SPACE_6_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("On your desktop")))
                    .children(variant.map(|v| super::compositor_page::badge(v, p.tertiary))),
            )
            .child(div().w_full().font_family(fonts.display.clone()).text_size(px(TEXT_24_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(name))
            .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(facts))
            .child(
                div()
                    .id("wp-folder")
                    .max_w_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .cursor_pointer()
                    .text_size(px(TEXT_12_5_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .hover(|s| s.text_color(p.on_surface.hsla()))
                    .tooltip(tip(t("Open folder"), p))
                    .on_click(move |_: &ClickEvent, _, _| super::wallgen_page::reveal(&reveal_folder))
                    .child(icon("folder", ICON_13_PX, p.on_surface_variant))
                    .child(div().min_w(px(0.0)).truncate().child(mandara_ui::bidi::ltr_isolate(&super::apps_page::tilde(&folder)))),
            )
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t("Colors"))).child(colors));
        let top = div().row().when(!split, |el| el.flex_col()).gap(px(HERO_GAP_PX)).child(picture).child(details);
        // Quick actions along the bottom of the card.
        let footer = div().pt(px(SPACE_14_PX)).border_t_1().border_color(p.outline.with_alpha(ALPHA_20).hsla()).child(actions);
        card.row().flex_col().gap(px(SPACE_14_PX)).child(top).child(footer).into_any_element()
    }

    /// The folder setting, opened from "Change folder".
    fn wp_folder_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let Some(setting) = registry::by_path("/wallpaper/directory") else { return div().into_any_element() };
        let rows = self.group_card(&[setting], cfg, p, cx);
        div()
            .rounded(px(RADIUS_20_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_6_PX))
            .row()
            .flex_col()
            .child(rows)
            .child(
                div().row().justify_end().px(px(SPACE_12_PX)).pb(px(SPACE_10_PX)).child(button("wp-folder-done", "check", t("Done"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    st(cx).editing_folder = false;
                    cx.notify();
                }))),
            )
            .into_any_element()
    }

    /// Lists the folder off the UI thread (again on each visit).
    fn wp_load_listing(&mut self, dir: &Path, force: bool, cx: &mut Context<Self>) {
        let page = st(cx);
        let have = page.listing.as_ref().is_some_and(|(d, _)| d == dir);
        if (have && !force) || page.listing_loading.as_deref() == Some(dir) {
            return;
        }
        page.listing_loading = Some(dir.to_path_buf());
        let (job, done) = (dir.to_path_buf(), dir.to_path_buf());
        form::job(cx, move || Ok(mandara_ui::wallpaper::list(&job)), move |_, res, cx| {
            let page = st(cx);
            if page.listing_loading.as_ref() == Some(&done) {
                page.listing_loading = None;
            }
            if page.listing.as_ref().is_none_or(|(d, _)| *d != done) {
                page.placed = false;
            }
            page.listing = Some((done, Rc::from(res.unwrap_or_default())));
        });
    }

    /// Every wallpaper in the current folder: a paged, keyboard-driven
    /// grid of cached thumbnails.
    fn wp_library(&mut self, cfg: &ShellConfig, shown: Option<&PathBuf>, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let dir = mandara_ui::wallpaper::directory(cx);
        self.wp_load_listing(&dir, false, cx);
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let listing = st(cx).listing.clone().filter(|(d, _)| *d == dir).map(|(_, l)| l);
        let search = form::input("wallpaper-search", t("Filter by name"), false, cx);
        let query = form::value("wallpaper-search", cx).to_lowercase();
        if st(cx).query != query {
            let page = st(cx);
            page.query = query.clone();
            page.grid_page = 0;
            page.cursor = 0;
        }
        let filtered: Rc<[PathBuf]> = listing.as_ref().map(|l| l.iter().filter(|p| query.is_empty() || file_name(p).to_lowercase().contains(&query)).cloned().collect()).unwrap_or_else(|| Rc::from([]));
        let fit = grid::fit(self.page.column_w, TILE_GRID);
        let per_page = (fit.cols * PAGE_ROWS).max(1);
        let n = filtered.len();
        let pages = n.div_ceil(per_page).max(1);
        // Open on the page with the current wallpaper.
        if listing.is_some() && !st(cx).placed {
            let at = shown.and_then(|s| filtered.iter().position(|f| f == s)).unwrap_or(0);
            let page = st(cx);
            page.placed = true;
            page.cursor = at;
            page.grid_page = at / per_page;
        }
        let page_no = st(cx).grid_page.min(pages - 1);
        let cursor = st(cx).cursor.min(n.saturating_sub(1));
        let range = (page_no * per_page)..((page_no + 1) * per_page).min(n);

        // Thumbnails for this page only (first tiles first).
        let thumbs = mandara_ui::wallpaper::thumbnails(cx);
        if st(cx).thumbs_watch.is_none() {
            let watch = cx.observe(&thumbs, |_, _, cx| cx.notify());
            st(cx).thumbs_watch = Some(watch);
        }
        let wanted: Vec<PathBuf> = filtered[range.clone()].iter().rev().filter(|w| thumbs.read(cx).get(w).is_none() && !thumbs.read(cx).failed(w)).cloned().collect();
        if !wanted.is_empty() {
            thumbs.update(cx, |t, cx| t.request(wanted, cx));
        }

        let focus = match st(cx).focus.clone() {
            Some(f) => f,
            None => {
                let f = cx.focus_handle();
                st(cx).focus = Some(f.clone());
                f
            }
        };
        let focused = focus.is_focused(window);
        let scale = window.scale_factor();
        let tile_w = fit.card_w;
        let tile_h = (tile_w * TILE_RATIO).floor();
        st(cx).scale = scale;
        let playlist: HashSet<PathBuf> = cfg.wallpaper.slideshow.playlist.iter().cloned().collect();
        let mut tiles = Vec::with_capacity(range.len());
        for i in range.clone() {
            let path = filtered[i].clone();
            let (thumb, failed) = {
                let t = thumbs.read(cx);
                (t.get(&path).cloned(), t.failed(&path))
            };
            if let Some(thumb) = &thumb {
                st(cx).drawn.insert((thumb.clone(), tile_w as u32));
            }
            tiles.push(self.wp_tile(i, &path, thumb, failed, shown == Some(&path), focused && i == cursor, playlist.contains(&path), (tile_w, tile_h), p, cx));
        }

        let head = section_head(
            t("In this folder"),
            match &listing {
                Some(l) => tf("{count} wallpapers · {folder}", &[("count", &l.len()), ("folder", &mandara_ui::bidi::ltr_isolate(&super::apps_page::tilde(&dir)))]),
                None => t("Reading the folder…").to_string(),
            },
            vec![
                div().w(px(SEARCH_W_PX)).h(px(DIM_34_PX)).px(px(SPACE_10_PX)).rounded_full().bg(p.surface_container.hsla()).row().items_center().gap(px(SPACE_6_PX)).child(icon("search", ICON_13_PX, p.on_surface_variant)).child(div().flex_1().min_w(px(0.0)).child(search)).into_any_element(),
                div()
                    .id("wp-refresh")
                    .size(px(DIM_34_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(p.surface_container.hsla())
                    .hover(|s| s.bg(p.surface_container_high.hsla()))
                    .tooltip(tip(t("Read the folder again"), p))
                    .child(icon("refresh", ICON_14_PX, p.on_surface_variant))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let dir = mandara_ui::wallpaper::directory(cx);
                        this.wp_load_listing(&dir, true, cx);
                    }))
                    .into_any_element(),
            ],
            display,
            p,
        );

        let body = if listing.is_none() {
            div().h(px(DIM_120_PX)).row().items_center().justify_center().child(placeholder("image", p)).into_any_element()
        } else if n == 0 {
            let text = if query.is_empty() { tf("No wallpapers in {folder} yet: add images or AV1 videos (.webm, .mkv, .ivf), or pick another folder.", &[("folder", &super::apps_page::tilde(&dir))]) } else { tf("Nothing here matches “{query}”.", &[("query", &query)]) };
            div().rounded(px(RADIUS_20_PX)).bg(p.surface_container.hsla()).p(px(SPACE_18_PX)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(text)).into_any_element()
        } else {
            let keys = filtered.clone();
            let cols = fit.cols;
            div()
                .id("wp-grid")
                .track_focus(&focus)
                .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                    if this.wp_grid_key(e, &keys, cols, per_page, cx) {
                        cx.stop_propagation();
                    }
                }))
                .child(fit.render(tiles))
                .into_any_element()
        };

        let pager = (pages > 1).then(|| {
            let from = range.start + 1;
            let to = range.end;
            let step = |id: &'static str, glyph: &'static str, to_page: Option<usize>, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .size(px(DIM_32_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .bg(p.surface_container.hsla())
                    .when(to_page.is_none(), |el| el.opacity(ALPHA_35))
                    .when_some(to_page, |el, to_page| {
                        el.cursor_pointer().hover(|s| s.bg(p.surface_container_high.hsla())).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                            let page = st(cx);
                            page.grid_page = to_page;
                            page.cursor = to_page * per_page;
                            cx.notify();
                        }))
                    })
                    .child(icon(glyph, ICON_14_PX, p.on_surface_variant))
            };
            let rtl = mandara_i18n::language().is_rtl();
            let (back, forward) = if rtl { ("chevron-right", "chevron-left") } else { ("chevron-left", "chevron-right") };
            div()
                .row()
                .items_center()
                .justify_center()
                .gap(px(SPACE_12_PX))
                .child(step("wp-page-prev", back, page_no.checked_sub(1), cx))
                .child(
                    div()
                        .text_size(px(TEXT_12_PX))
                        .text_color(p.on_surface_variant.hsla())
                        .font_features(mandara_ui::fonts::tabular_nums())
                        .child(tf("{from}–{to} of {count}", &[("from", &from), ("to", &to), ("count", &n)])),
                )
                .child(step("wp-page-next", forward, (page_no + 1 < pages).then_some(page_no + 1), cx))
        });
        div().row().flex_col().gap(px(SPACE_14_PX)).child(head).child(body).children(pager).into_any_element()
    }

    /// One wallpaper of the grid.
    fn wp_tile(&mut self, i: usize, path: &Path, thumb: Option<PathBuf>, failed: bool, current: bool, cursor: bool, listed: bool, (w, h): (f32, f32), p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let group = SharedString::from(format!("wp-tile-{i}"));
        let video = mandara_ui::wallpaper::is_video(path);
        let name = file_name(path);
        let apply_path = path.to_path_buf();
        let list_path = path.to_path_buf();
        let picture = div()
            .relative()
            .w(px(w))
            .h(px(h))
            .rounded(px(RADIUS_14_PX))
            .clip_rounded()
            .bg(p.surface_container_high.hsla())
            .border_2()
            .border_color(if current {
                p.primary.hsla()
            } else if cursor {
                p.primary.with_alpha(ALPHA_60).hsla()
            } else {
                gpui::transparent_black()
            })
            .group_hover(group.clone(), |s| s.border_color(p.primary.with_alpha(ALPHA_45).hsla()))
            .child(match thumb {
                Some(t) => img(mandara_ui::icon_raster::source(&t, w)).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
                None => placeholder(if failed { "alert" } else { "image" }, p),
            })
            .when(current, |el| el.child(div().absolute().top(px(SPACE_6_PX)).inset_start(px(SPACE_6_PX)).child(tag("check", t("Current"), p))))
            .when(video, |el| el.child(div().absolute().bottom(px(SPACE_6_PX)).inset_start(px(SPACE_6_PX)).child(tag("play", t("Video"), p))))
            .child(
                div()
                    .id(SharedString::from(format!("wp-list-{i}")))
                    .absolute()
                    .top(px(SPACE_6_PX))
                    .inset_end(px(SPACE_6_PX))
                    .size(px(TILE_BUTTON_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(if listed { p.primary.hsla() } else { p.surface_base.with_alpha(ALPHA_85).hsla() })
                    .when(!listed, |el| el.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0)))
                    .tooltip(tip(if listed { t("In the slideshow playlist: click to remove") } else { t("Add to the slideshow playlist") }, p))
                    .child(icon(if listed { "check" } else { "plus" }, ICON_12_PX, if listed { p.on_primary } else { p.primary }))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        let path = list_path.clone();
                        update_config(cx, move |c| {
                            let list = &mut c.wallpaper.slideshow.playlist;
                            if let Some(at) = list.iter().position(|x| *x == path) {
                                list.remove(at);
                            } else {
                                list.push(path);
                            }
                        });
                    })),
            );
        div()
            .id(SharedString::from(format!("wp-tile-{i}-card")))
            .group(group)
            .w_full()
            .row()
            .flex_col()
            .gap(px(SPACE_6_PX))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if let Some(focus) = st(cx).focus.clone() {
                    focus.focus(window, cx);
                }
                st(cx).cursor = i;
                this.wp_apply(apply_path.clone(), cx);
            }))
            .child(picture)
            .child(
                div()
                    .px(px(SPACE_4_PX))
                    .truncate()
                    .text_size(px(TEXT_12_PX))
                    .when(current, |el| el.font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface.hsla()))
                    .when(!current, |el| el.text_color(p.on_surface_variant.hsla()))
                    .child(name),
            )
            .into_any_element()
    }

    /// Makes `path` the wallpaper (the shell crossfades with the desktop's
    /// transition).
    fn wp_apply(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        update_config(cx, move |c| c.wallpaper.path = Some(path));
        cx.notify();
    }

    /// Arrow keys, Home / End and Page Up / Down move through the grid;
    /// Enter or Space applies. Returns whether the key was used.
    fn wp_grid_key(&mut self, e: &KeyDownEvent, list: &[PathBuf], cols: usize, per_page: usize, cx: &mut Context<Self>) -> bool {
        let m = e.keystroke.modifiers;
        if m.control || m.alt || m.platform || list.is_empty() {
            return false;
        }
        let rtl = mandara_i18n::language().is_rtl();
        let last = list.len() - 1;
        let at = st(cx).cursor.min(last);
        let to = match e.keystroke.key.as_str() {
            "left" if rtl => at + 1,
            "right" if rtl => at.saturating_sub(1),
            "left" => at.saturating_sub(1),
            "right" => at + 1,
            "up" => at.saturating_sub(cols),
            "down" => at + cols,
            "pageup" => at.saturating_sub(per_page),
            "pagedown" => at + per_page,
            "home" => 0,
            "end" => last,
            "enter" | "space" => {
                self.wp_apply(list[at].clone(), cx);
                return true;
            }
            _ => return false,
        }
        .min(last);
        let page = st(cx);
        page.cursor = to;
        page.grid_page = to / per_page.max(1);
        cx.notify();
        true
    }

    // ── Slideshow tab ────────────────────────────────────────────────────

    /// Asks the shell for the slideshow's status when the settings or the
    /// wallpaper changed, or the next change is past.
    fn wp_refresh_status(&mut self, cfg: &ShellConfig, cx: &mut Context<Self>) {
        let key = (cfg.wallpaper.slideshow.clone(), cfg.wallpaper.path.clone());
        let page = st(cx);
        let stale = page.status.as_ref().is_some_and(|(s, at)| s.as_ref().ok().and_then(|v| v["next_in_seconds"].as_u64()).is_some_and(|n| at.elapsed() > Duration::from_secs(n) + STATUS_SLACK));
        if page.status_loading || (page.status_for.as_ref() == Some(&key) && !stale) {
            return;
        }
        page.status_loading = true;
        page.status_for = Some(key);
        form::shell(vec!["wallpaper".into(), "slideshow".into(), "status".into()], cx, |_, res, cx| {
            let next = res.as_ref().ok().and_then(|v| v["next_in_seconds"].as_u64());
            // Look again once the next change has happened.
            let wake = next.map(|secs| {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(secs) + STATUS_SLACK).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
            });
            let page = st(cx);
            page.status_loading = false;
            page.status = Some((res, Instant::now()));
            page.status_wake = wake;
        });
    }

    /// Runs a slideshow command and keeps its status reply.
    fn wp_slideshow_command(&mut self, args: &[&str], cx: &mut Context<Self>) {
        let words = ["wallpaper", "slideshow"].iter().chain(args).map(|s| s.to_string()).collect();
        form::shell(words, cx, |_, res, cx| {
            let page = st(cx);
            match res {
                Ok(v) => {
                    page.status = Some((Ok(v), Instant::now()));
                    page.note = None;
                }
                Err(e) => page.note = Some((e, true)),
            }
        });
    }

    fn wp_slideshow_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        self.wp_refresh_status(cfg, cx);
        let hero = self.wp_slideshow_hero(cfg, p, cx);
        let variants = &cfg.wallpaper.variants;
        let fixed = [&variants.light, &variants.dark].iter().any(|v| v.as_ref().is_some_and(|p| !p.as_os_str().is_empty()));
        let warning = fixed.then(|| {
            div()
                .rounded(px(RADIUS_16_PX))
                .bg(p.warning_amber.with_alpha(ALPHA_12).hsla())
                .px(px(SPACE_14_PX))
                .py(px(SPACE_10_PX))
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(icon("alert", ICON_14_PX, p.warning_amber))
                .child(div().flex_1().min_w(px(DIM_240_PX)).text_size(px(TEXT_12_5_PX)).child(mandara_ui::bidi::paragraph(t("A fixed light or dark wallpaper is set under Fit & effects; it shows instead of the slideshow while the theme is in that mode."))))
                .child(button("wp-clear-variants", "close", t("Clear them"), false, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    update_config(cx, |c| {
                        c.wallpaper.variants.light = None;
                        c.wallpaper.variants.dark = None;
                    });
                })))
        });
        let playlist = (cfg.wallpaper.slideshow.source == SlideshowSource::Playlist).then(|| self.wp_playlist(cfg, p, cx));
        let folds = self.rows(registry::in_section(Section::Wallpaper).filter(|s| SLIDESHOW_GROUPS.contains(&s.group)), cfg, p, cx);
        div().row().flex_col().gap(px(SPACE_22_PX)).child(hero).children(warning).children(playlist).child(folds).into_any_element()
    }

    /// The slideshow's card: on / off, what happens next, previous / next,
    /// interval and order.
    fn wp_slideshow_hero(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let s = cfg.wallpaper.slideshow.clone();
        let status = st(cx).status.clone();
        let reply = status.as_ref().and_then(|(r, _)| r.as_ref().ok()).cloned();
        let elapsed = status.as_ref().map_or(0, |(_, at)| at.elapsed().as_secs());
        let count = reply.as_ref().and_then(|v| v["count"].as_u64()).unwrap_or(0);
        let error = reply.as_ref().and_then(|v| v["error"].as_str().map(str::to_string)).or_else(|| status.as_ref().and_then(|(r, _)| r.as_ref().err().cloned()));
        let (line, tone) = if !s.enabled {
            (t("Off: your wallpaper stays until you change it").to_string(), p.on_surface_variant)
        } else if !cfg.wallpaper.enabled {
            (t("Mandara isn't drawing the wallpaper (Fit & effects)").to_string(), p.warning_amber)
        } else if let Some(e) = error {
            (e, p.urgent_coral)
        } else {
            let paused = reply.as_ref().and_then(|v| v["paused"].as_str().map(str::to_string));
            let next = reply.as_ref().and_then(|v| v["next_in_seconds"].as_u64()).map(|n| n.saturating_sub(elapsed));
            let at = next.map(|n| clock_in(n, cfg.general.clock_24h));
            match (paused.as_deref(), at) {
                (Some("fullscreen"), _) => (t("Paused while a fullscreen app is open").to_string(), p.warning_amber),
                (Some("battery"), _) => (t("Paused while on battery").to_string(), p.warning_amber),
                (Some("locked"), _) => (t("Paused while the screen is locked").to_string(), p.warning_amber),
                (Some("hours"), Some(at)) => (tf("Waiting for its hours: next change at {time}", &[("time", &at)]), p.warning_amber),
                (_, Some(at)) => (tf("Next wallpaper at {time} · {count} in the rotation", &[("time", &at), ("count", &count)]), p.on_surface_variant),
                _ => (t("Starting…").to_string(), p.on_surface_variant),
            }
        };
        let can_back = reply.as_ref().and_then(|v| v["can_go_back"].as_bool()).unwrap_or(false);
        let on = s.enabled;
        let switch = div()
            .id("wp-slideshow-switch")
            .flex_none()
            .cursor_pointer()
            .tab_index(0)
            .child(form::switch(on, p))
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| update_config(cx, |c| c.wallpaper.slideshow.enabled = !c.wallpaper.slideshow.enabled)));
        let head = div()
            .row()
            .items_center()
            .gap(px(SPACE_14_PX))
            .child(div().size(px(DIM_44_PX)).flex_none().rounded(px(RADIUS_14_PX)).row().items_center().justify_center().bg(if on { p.primary.hsla() } else { p.surface_container_high.hsla() }).child(icon(if on { "play" } else { "pause" }, ICON_17_PX, if on { p.on_primary } else { p.on_surface_variant })))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::MEDIUM).child(t("Slideshow")))
                    .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(tone.hsla()).child(mandara_ui::bidi::paragraph(line))),
            )
            .child(switch);

        let transport = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_8_PX))
            .child(
                button("wp-slide-prev", "previous", t("Previous"), false, p)
                    .tab_index(0)
                    .when(!can_back, |el| el.opacity(ALPHA_50))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.wp_slideshow_command(&["previous"], cx))),
            )
            .child(button("wp-slide-next", "next", t("Next wallpaper"), true, p).tab_index(0).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.wp_slideshow_command(&["next"], cx))));

        // Interval: presets, or a custom time typed as 45, 90m, 2h, 1d.
        let minutes = s.interval();
        let preset = defaults::SLIDESHOW_PRESETS.contains(&minutes);
        let custom_open = st(cx).custom_interval || !preset;
        let mut chips: Vec<AnyElement> = defaults::SLIDESHOW_PRESETS
            .iter()
            .map(|&m| {
                chip(SharedString::from(format!("wp-every-{m}")), interval_label(m), m == minutes && !st(cx).custom_interval, p)
                    .tab_index(0)
                    .font_features(mandara_ui::fonts::tabular_nums())
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        st(cx).custom_interval = false;
                        update_config(cx, move |c| c.wallpaper.slideshow.interval_minutes = m);
                    }))
                    .into_any_element()
            })
            .collect();
        chips.push(
            chip("wp-every-custom", if preset { t("Custom…").to_string() } else { tf("Custom: {time}", &[("time", &interval_label(minutes))]) }, custom_open, p)
                .tab_index(0)
                .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                    let page = st(cx);
                    page.custom_interval = !page.custom_interval;
                    cx.notify();
                }))
                .into_any_element(),
        );
        let custom = custom_open.then(|| {
            let field = form::input("wp-interval", t("e.g. 45, 90m, 2h, 1d"), false, cx);
            let typed = form::value("wp-interval", cx);
            let parsed = parse_interval(&typed);
            let set = button("wp-interval-set", "check", t("Set"), true, p).when(parsed.is_none(), |el| el.opacity(ALPHA_50)).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                if let Some(m) = parse_interval(&form::value("wp-interval", cx)) {
                    let m = m.clamp(defaults::SLIDESHOW_INTERVAL_RANGE.0, defaults::SLIDESHOW_INTERVAL_RANGE.1);
                    update_config(cx, move |c| c.wallpaper.slideshow.interval_minutes = m);
                    form::clear("wp-interval", cx);
                }
            }));
            let hint = match parsed {
                Some(m) => tf("Every {time}", &[("time", &interval_label(m.clamp(defaults::SLIDESHOW_INTERVAL_RANGE.0, defaults::SLIDESHOW_INTERVAL_RANGE.1)))]),
                None => t("Minutes, or with m, h or d (up to a week)").to_string(),
            };
            div()
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_10_PX))
                .child(div().w(px(INTERVAL_FIELD_W_PX)).h(px(DIM_34_PX)).px(px(SPACE_10_PX)).rounded(px(RADIUS_10_PX)).bg(p.surface_container_high.hsla()).row().items_center().child(div().flex_1().min_w(px(0.0)).child(field)))
                .child(set)
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(hint))
        });

        let orders: Vec<AnyElement> = match registry::by_path("/wallpaper/slideshow/order").map(|s| s.control.clone()) {
            Some(registry::Control::Choice(choices)) => choices
                .iter()
                .map(|&(value, label)| {
                    let active = serde_json::to_value(s.order).ok().and_then(|v| v.as_str().map(str::to_string)).as_deref() == Some(value);
                    let glyph = match value {
                        "shuffle" => "dice",
                        "random" => "sparkles",
                        "newest" => "clock",
                        _ => "list-check",
                    };
                    chip(SharedString::from(format!("wp-order-{value}")), tr(label), active, p)
                        .tab_index(0)
                        .child(icon(glyph, ICON_12_PX, if active { p.on_primary } else { p.primary }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.wp_slideshow_command(&["order", value], cx)))
                        .into_any_element()
                })
                .collect(),
            _ => Vec::new(),
        };
        let source = match s.source {
            SlideshowSource::Playlist => tf("From your playlist ({count})", &[("count", &s.playlist.len())]),
            SlideshowSource::Folder => {
                let typed = s.folder_for(cfg.theme.mode.is_dark());
                let dir = if typed.is_empty() { mandara_ui::wallpaper::directory(cx) } else { mandara_config::paths::expand_tilde(typed) };
                tf("From {folder}", &[("folder", &mandara_ui::bidi::ltr_isolate(&super::apps_page::tilde(&dir)))])
            }
        };
        let label = |text: &'static str| div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(t(text));
        div()
            .rounded(px(RADIUS_26_PX))
            .bg(mandara_core::oklab_gradient(WASH_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .p(px(SPACE_18_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_16_PX))
            .child(head)
            .child(div().row().flex_wrap().items_center().justify_between().gap(px(SPACE_10_PX)).child(transport).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(source)))
            .child(div().row().flex_col().gap(px(SPACE_8_PX)).child(label("Change every")).child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(chips)).children(custom))
            .child(div().row().flex_col().gap(px(SPACE_8_PX)).child(label("Order")).child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(orders)))
            .into_any_element()
    }

    /// The playlist (source = playlist): its wallpapers with remove
    /// buttons; wallpapers are added from the Wallpaper tab's grid.
    fn wp_playlist(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let list = cfg.wallpaper.slideshow.playlist.clone();
        let thumbs = mandara_ui::wallpaper::thumbnails(cx);
        if st(cx).thumbs_watch.is_none() {
            let watch = cx.observe(&thumbs, |_, _, cx| cx.notify());
            st(cx).thumbs_watch = Some(watch);
        }
        let wanted: Vec<PathBuf> = list.iter().rev().filter(|w| w.is_file() && thumbs.read(cx).get(w).is_none() && !thumbs.read(cx).failed(w)).cloned().collect();
        if !wanted.is_empty() {
            thumbs.update(cx, |t, cx| t.request(wanted, cx));
        }
        let fit = grid::fit(self.page.column_w, PLAYLIST_GRID);
        let (w, h) = (fit.card_w, (fit.card_w * TILE_RATIO).floor());
        let tiles: Vec<AnyElement> = list
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let thumb = thumbs.read(cx).get(path).cloned();
                if let Some(thumb) = &thumb {
                    st(cx).drawn.insert((thumb.clone(), w as u32));
                }
                let missing = !path.is_file();
                let remove = path.clone();
                div()
                    .w_full()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(
                        div()
                            .relative()
                            .w(px(w))
                            .h(px(h))
                            .rounded(px(RADIUS_12_PX))
                            .clip_rounded()
                            .bg(p.surface_container_high.hsla())
                            .child(match thumb {
                                Some(t) => img(mandara_ui::icon_raster::source(&t, w)).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
                                None => placeholder(if missing { "alert" } else { "image" }, p),
                            })
                            .child(
                                div()
                                    .id(("wp-playlist-remove", i))
                                    .absolute()
                                    .top(px(SPACE_4_PX))
                                    .inset_end(px(SPACE_4_PX))
                                    .size(px(TILE_BUTTON_PX))
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .bg(p.surface_base.with_alpha(ALPHA_85).hsla())
                                    .tooltip(tip(t("Remove from the playlist"), p))
                                    .child(icon("close", ICON_11_PX, p.on_surface_variant))
                                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                        let path = remove.clone();
                                        update_config(cx, move |c| c.wallpaper.slideshow.playlist.retain(|x| *x != path));
                                    })),
                            ),
                    )
                    .child(div().px(px(SPACE_4_PX)).truncate().text_size(px(TEXT_11_5_PX)).text_color(if missing { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() }).child(file_name(path)))
                    .into_any_element()
            })
            .collect();
        let pick = button("wp-playlist-pick", "plus", t("Pick wallpapers"), list.is_empty(), p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_wallpaper_tab(WallpaperTab::Wallpaper, cx)));
        let clear = (!list.is_empty()).then(|| button("wp-playlist-clear", "trash", t("Clear"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.wp_slideshow_command(&["playlist", "clear"], cx))));
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let detail = if list.is_empty() { t("Empty: on the Wallpaper tab, hover a wallpaper and press + to add it.").to_string() } else { tf("{count} wallpapers, shown in the slideshow's order", &[("count", &list.len())]) };
        let mut trailing = vec![pick.into_any_element()];
        trailing.extend(clear.map(IntoElement::into_any_element));
        let head = section_head(t("Playlist"), detail, trailing, display, p);
        div().row().flex_col().gap(px(SPACE_12_PX)).child(head).when(!tiles.is_empty(), |el| el.child(fit.render(tiles))).into_any_element()
    }

    // ── Fit & effects, Online ────────────────────────────────────────────

    fn wp_effects_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let claimed = |g: &str| g == PICKER_GROUP || g == ONLINE_GROUP || g == SLIDESHOW_HEAD_GROUP || SLIDESHOW_GROUPS.contains(&g);
        let rows = self.rows(registry::in_section(Section::Wallpaper).filter(|s| !claimed(s.group)), cfg, p, cx);
        let extra = self.wallpaper_effects_sections(cfg, p, cx);
        div().row().flex_col().gap(px(SPACE_26_PX)).child(rows).children(extra).into_any_element()
    }

    /// Cards of their own under Fit & effects, after the settings folds:
    /// wallpaper effects that come with more than settings rows (a
    /// plugin's depth options, previews) add theirs here. Plain settings
    /// need nothing: any Wallpaper registry group no other tab claims shows
    /// on this tab by itself.
    pub(super) fn wallpaper_effects_sections(&mut self, _cfg: &ShellConfig, _p: &MandaraPalette, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        Vec::new()
    }

    fn wp_online_tab(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let rows = self.rows(registry::in_section(Section::Wallpaper).filter(|s| s.group == ONLINE_GROUP), cfg, p, cx);
        let browse = div()
            .rounded(px(RADIUS_20_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_16_PX))
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_14_PX))
            .child(icon("globe", ICON_22_PX, p.primary))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_240_PX))
                    .text_size(px(TEXT_12_5_PX))
                    .line_height(px(LEADING_17_PX))
                    .text_color(p.on_surface_variant.hsla())
                    .child(mandara_ui::bidi::paragraph(t("Search wallhaven.cc in the wallpaper picker: type a search and pick a result, it downloads at full size into your folder."))),
            )
            .child(button("wp-online-browse", "search", t("Browse in the picker"), true, p).on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                form::shell(vec!["wallpaper".into(), "picker".into(), "open".into()], cx, |_, res, cx| {
                    if let Err(e) = res {
                        st(cx).note = Some((e, true));
                    }
                });
            })));
        div().row().flex_col().gap(px(SPACE_22_PX)).child(browse).child(rows).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_parse_their_names_and_aliases() {
        for tab in WallpaperTab::ALL {
            assert_eq!(WallpaperTab::parse(tab.slug()), Some(tab));
        }
        assert_eq!(WallpaperTab::parse("Fit"), Some(WallpaperTab::Effects));
        assert_eq!(WallpaperTab::parse("wallhaven"), Some(WallpaperTab::Online));
        assert_eq!(WallpaperTab::parse("wallgen"), Some(WallpaperTab::Generator));
        assert_eq!(WallpaperTab::parse("nope"), None);
        assert_eq!(WallpaperTab::default(), WallpaperTab::Wallpaper, "the page opens on the wallpaper");
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(aspect_label((2560, 1440)), "16:9");
        assert_eq!(aspect_label((1920, 1200)), "8:5");
        assert_eq!(aspect_label((3440, 1440)), "2.39:1");
        assert_eq!(interval_label(90), format!("1 {} 30 {}", t("h"), t("min")));
        assert_eq!(interval_label(5), format!("5 {}", t("min")));
        assert_eq!(interval_label(MIN_PER_DAY), t("1 day"));
    }

    /// Every Wallpaper registry group shows on exactly one tab.
    #[test]
    fn every_wallpaper_group_has_a_tab() {
        let groups: HashSet<&str> = registry::in_section(Section::Wallpaper).map(|s| s.group).collect();
        for g in SLIDESHOW_GROUPS.iter().chain([&PICKER_GROUP, &ONLINE_GROUP, &SLIDESHOW_HEAD_GROUP]) {
            assert!(groups.contains(g), "no Wallpaper group {g:?}");
        }
        assert!(groups.contains("Depth clock") && groups.contains("Desktop") && groups.contains("Video"), "Fit & effects keeps the desktop, video and depth settings");
    }
}
