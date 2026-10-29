//! Studio → Wallpaper → Generator: the wallpaper generator plugin's
//! studio, shown at the top of the Wallpaper page while the plugin is on
//! (a compact card offers to turn it on otherwise).
//!
//! * **Hero** — a large live preview of the current recipe (rendered off
//!   the UI thread at [`MAIN_W`] × [`MAIN_H`], debounced while sliders move,
//!   the last picture kept until the next is ready), seed controls
//!   (shuffle, lock, surprise me), clickable variations, light / dark,
//!   the size and format choosers (with the expected file size), the
//!   Generate and Generate video loop buttons with progress and cancel,
//!   and the result with Set as wallpaper / Show in folder / Remix.
//! * **Styles** — a card per style previewing the current palette and seed.
//! * **Palette** — the curated palettes, Match theme, and custom stops.
//! * **Fine-tune** — sliders built from the engine's `ParamSpec`s: the
//!   style's own, then tone, light, texture and layout, each group with a
//!   reset.
//! * **Recent** — files in the output folder (read from their sidecars):
//!   Remix, Set, Delete (twice, to confirm).
//! * **Schedule** — opt-in, off by default, with the next run.
//!
//! Generation itself runs in the shell (`wallgen generate … --detach`,
//! progress through `wallgen status`), so it carries on when Studio
//! closes and never competes with Studio's frames. Previews are dropped
//! when you leave the page.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, Task, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::wallgen::{ScheduleMode, WallgenFormat, WallgenKind, WallgenSettings, WallgenSize, WallgenTone};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::lifecycle::Action;
use mandara_ui::plugins::wallpaper_generator::{self as wg, model, schedule};
use mandara_ui::style::ClipRounded as _;
use mandara_wallgen::{PaletteSpec, ParamSpec, Recipe, Style};

use super::compositor_page::{button, chip};
use super::themes_page::section_head;
use super::wallgen_logic::{self as logic, Next, PreviewQueue};
use super::{StudioView, form, grid};
use crate::text_field::TextField;
use crate::tokens::*;

/// The large preview's render size (upscaled smoothly to the card).
pub const MAIN_W: u32 = 640;
pub const MAIN_H: u32 = 360;
/// Gallery and variation thumbnails.
const THUMB_W: u32 = 256;
const THUMB_H: u32 = 144;
/// Wallpapers are drawn 16:9 here.
const ASPECT: f32 = 9.0 / 16.0;
/// Variations offered under the preview.
const VARIATIONS: usize = 5;
/// Recent files shown.
const RECENT_MAX: usize = 12;
/// How often a running job's progress is asked for.
const POLL: Duration = Duration::from_millis(400);

/// The hero: preview and controls side by side from this column width
/// (else stacked), the preview taking this share of the row.
const SPLIT_MIN_W_PX: f32 = 2.0 * DIM_380_PX;
const PREVIEW_SHARE: f32 = 0.56;
const HERO_PAD_PX: f32 = SPACE_18_PX;
const HERO_RADIUS_PX: f32 = RADIUS_26_PX;
const PREVIEW_RADIUS_PX: f32 = RADIUS_18_PX;
/// The Generate button.
const GENERATE_H_PX: f32 = DIM_44_PX;
/// Progress bar height, and the smallest share it draws (reads "started").
const PROGRESS_H_PX: f32 = DIM_6_PX;
const PROGRESS_MIN: f32 = 0.03;
/// Indeterminate bar (stills report no fraction): the moving share and
/// its period.
const SWEEP_SHARE: f32 = 0.3;
const SWEEP_PERIOD_S: f64 = 1.6;
/// Thumbnails dim while their recipe re-renders.
const STALE_ALPHA: f32 = ALPHA_75;
/// Slider rows: label, slider and value columns.
const SLIDER_W_PX: f32 = DIM_180_PX;
const VALUE_W_PX: f32 = DIM_64_PX;
const ROW_H_PX: f32 = DIM_34_PX;
/// Swatch strips on palette tiles, and custom stop swatches.
const STRIP_H_PX: f32 = DIM_26_PX;
const STOP_SWATCH_PX: f32 = DIM_28_PX;
const STOP_FIELD_W_PX: f32 = DIM_96_PX;
/// The off-state card's art tile.
const PROMO_TILE_PX: f32 = DIM_56_PX;
/// Gap between a title and its detail line.
const LINE_GAP_PX: f32 = 2.0;

const STYLE_GRID: CardGrid = CardGrid { min_w: 168.0, max_w: 260.0, gap: SPACE_12_PX };
const PALETTE_GRID: CardGrid = CardGrid { min_w: 150.0, max_w: 240.0, gap: SPACE_10_PX };
const PARAM_GRID: CardGrid = CardGrid { min_w: 360.0, max_w: 620.0, gap: SPACE_14_PX };
const RECENT_GRID: CardGrid = CardGrid { min_w: 176.0, max_w: 280.0, gap: SPACE_12_PX };

/// What the panel keeps between frames (a global, dropped on leaving).
#[derive(Default)]
struct Gen {
    /// Rendered pictures by [`logic::key`].
    images: HashMap<u64, Arc<RenderImage>>,
    /// The main preview: its queue, the recipe it wants, the picture on
    /// screen (kept while the next renders) and its pixels for estimates.
    queue: PreviewQueue,
    wanted: Option<(u64, Recipe)>,
    main: Option<(u64, Arc<RenderImage>)>,
    pixels: Option<(u64, Arc<image::RgbImage>)>,
    /// Encoded sizes of the main picture, `(key, format) → bytes`.
    samples: HashMap<(u64, WallgenFormat), u64>,
    /// Thumbnails wanted (gallery, variations), in priority order.
    thumbs: Vec<(u64, Recipe)>,
    /// A render is running (one at a time); the debounce wake-up.
    busy: bool,
    wake: Option<Task<()>>,
    /// Variations come from this seed (New variations changes it).
    variation_seed: u64,
    /// The primary display's size (asked once).
    display: Option<(u32, u32)>,
    display_asked: bool,
    job: Option<Job>,
    poll: Option<Task<()>>,
    result: Option<Result<Made, String>>,
    recent: Option<Vec<model::Entry>>,
    recent_dir: Option<PathBuf>,
    recent_loading: bool,
    recent_thumbs: HashMap<PathBuf, PathBuf>,
    thumbs_loading: HashSet<PathBuf>,
    confirm_delete: Option<PathBuf>,
    /// Custom palette stop editors by index.
    stop_fields: HashMap<usize, Entity<TextField>>,
    folder_field: Option<Entity<TextField>>,
}
impl gpui::Global for Gen {}

/// A generation the shell runs for us.
#[derive(Clone, Debug, Default)]
struct Job {
    id: Option<u64>,
    video: bool,
    progress: Option<f32>,
    seconds: f64,
    cancelling: bool,
}

/// A finished wallpaper.
#[derive(Clone, Debug)]
struct Made {
    path: PathBuf,
    bytes: u64,
    width: u32,
    height: u32,
    seconds: f64,
    video: bool,
}

fn st(cx: &mut App) -> &mut Gen {
    cx.default_global::<Gen>()
}

/// Drops previews and lists when leaving the Wallpaper page.
pub(super) fn release(cx: &mut App) {
    if let Some(g) = cx.try_global::<Gen>() {
        // A running job keeps its poll so its result still shows on return.
        let (job, poll, result) = (g.job.clone(), g.job.is_some(), g.result.clone());
        let keep_poll = if poll { cx.global_mut::<Gen>().poll.take() } else { None };
        cx.set_global(Gen { job, poll: keep_poll, result, ..Gen::default() });
    }
}

/// The settings and the current recipe (light from the tone).
fn current(cfg: &ShellConfig, dark: bool) -> (WallgenSettings, Recipe) {
    let s = WallgenSettings::from_plugins(&cfg.plugins);
    let r = model::recipe(&s, dark);
    (s, r)
}

/// Edits the settings and the recipe together (one config write).
fn edit(dark: bool, cx: &mut App, f: impl FnOnce(&mut WallgenSettings, &mut Recipe)) {
    wg::update_settings(cx, |s| {
        let mut r = model::recipe(s, dark);
        f(s, &mut r);
        r.sanitize();
        model::store_recipe(s, &r);
    });
}

/// RGB pixels as a GPUI image (BGRA).
fn to_image(rgb: &image::RgbImage) -> Arc<RenderImage> {
    let mut bgra = image::RgbaImage::new(rgb.width(), rgb.height());
    for (dst, src) in bgra.pixels_mut().zip(rgb.pixels()) {
        *dst = image::Rgba([src[2], src[1], src[0], u8::MAX]);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(bgra)]))
}

/// The thumbnail recipe of `style`: its defaults in the current palette,
/// seed and tone.
fn style_recipe(style: Style, r: &Recipe) -> Recipe {
    Recipe { style, params: Default::default(), ..r.clone() }
}

fn variations(r: &Recipe, s: &WallgenSettings, seed: u64) -> Vec<Recipe> {
    (0..VARIATIONS as u64).map(|i| model::variation(r, s.variation, seed.wrapping_add(i), s.seed_locked)).collect()
}

/// A file name or folder with `~` for the home folder.
fn tilde(path: &Path) -> String {
    super::apps_page::tilde(path)
}

/// "in 5 h 12 min", "in 40 s".
fn in_label(secs: u64) -> String {
    const MIN: u64 = 60;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;
    let text = if secs >= DAY {
        format!("{} {} {} {}", secs / DAY, t("d"), (secs % DAY) / HOUR, t("h"))
    } else if secs >= HOUR {
        format!("{} {} {} {}", secs / HOUR, t("h"), (secs % HOUR) / MIN, t("min"))
    } else if secs >= MIN {
        format!("{} {}", secs / MIN, t("min"))
    } else {
        format!("{secs} {}", t("s"))
    };
    tf("in {time}", &[("time", &mandara_ui::bidi::ltr_isolate(&text))])
}

/// Opens a folder with the file manager, off the UI thread.
fn reveal(path: &Path) {
    let dir = if path.is_dir() { path.to_path_buf() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
    let uri = format!("file://{}", mandara_services::app_uninstall::percent_encode(&dir));
    std::thread::spawn(move || {
        if let Err(e) = mandara_services::session::open_uri(&uri) {
            tracing::warn!("wallpaper generator: opening {uri}: {e}");
        }
    });
}

/// A pressable pill (icon + label), filled or tonal.
fn pill(id: impl Into<SharedString>, icon_name: &'static str, label: impl Into<SharedString>, filled: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    button(id, icon_name, label, filled, p).active(|s| s.opacity(ALPHA_75))
}

/// A round icon button with a tooltip.
fn icon_button(id: impl Into<SharedString>, icon_name: &'static str, hint: &'static str, on: bool, p: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let (bg, fg) = if on { (p.primary, p.on_primary) } else { (p.surface_container_high, p.on_surface_variant) };
    div()
        .id(id.into())
        .size(px(DIM_32_PX))
        .flex_none()
        .rounded_full()
        .row()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(bg.hsla())
        .hover(|s| s.opacity(ALPHA_85))
        .active(|s| s.opacity(ALPHA_70))
        .tooltip(super::compositor_page::tip(t(hint), p))
        .child(icon(icon_name, ICON_15_PX, fg))
}

/// A glass tag over a picture (readable on any image).
fn glass_tag(icon_name: &'static str, text: impl Into<SharedString>, p: &MandaraPalette) -> gpui::Div {
    div()
        .h(px(DIM_22_PX))
        .px(px(SPACE_8_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_4_PX))
        .bg(p.surface_base.with_alpha(ALPHA_80).hsla())
        .text_size(px(TEXT_11_PX))
        .font_weight(FontWeight::MEDIUM)
        .child(icon(icon_name, ICON_10_PX, p.primary))
        .child(text.into())
}

/// A small caps-like group label.
fn label(text: impl Into<SharedString>, p: &MandaraPalette) -> gpui::Div {
    div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(text.into())
}

/// A strip of equal color segments.
fn strip(colors: &[Color], h: f32, radius: f32) -> gpui::Div {
    div().h(px(h)).w_full().rounded(px(radius)).overflow_hidden().ltr_row().children(colors.iter().map(|c| div().flex_1().h_full().bg(c.hsla())))
}

fn hex_colors(spec: &PaletteSpec) -> Vec<Color> {
    spec.hex_stops().iter().filter_map(|h| Color::from_hex(h).ok()).collect()
}

/// A picture box: the image when ready, a soft placeholder otherwise.
fn picture(image: Option<Arc<RenderImage>>, w: f32, h: f32, radius: f32, stale: bool, p: &MandaraPalette) -> gpui::Div {
    div()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .rounded(px(radius))
        .clip_rounded()
        .bg(p.surface_container_high.hsla())
        .child(match image {
            Some(i) => img(i).size_full().object_fit(gpui::ObjectFit::Cover).when(stale, |el| el.opacity(STALE_ALPHA)).into_any_element(),
            None => div().size_full().row().items_center().justify_center().child(icon("image", ICON_22_PX, p.on_surface_variant.with_alpha(ALPHA_50))).into_any_element(),
        })
}

impl StudioView {
    /// The generator panel (or the card that offers the plugin).
    pub(super) fn wallgen_panel(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        if !wg::enabled(cx) {
            return self.wallgen_promo(p, cx);
        }
        let dark = p.is_dark;
        let (s, r) = current(cfg, dark);
        self.wallgen_prepare(&s, &r, cfg, cx);
        let hero = self.wallgen_hero(&s, &r, cfg, p, cx);
        let styles = self.wallgen_styles(&r, p, cx);
        let palette = self.wallgen_palette(&r, p, cx);
        let params = self.wallgen_params(&r, p, cx);
        let recent = self.wallgen_recent(&s, cfg, p, cx);
        let schedule = self.wallgen_schedule(&s, p, cx);
        div().row().flex_col().gap(px(SPACE_26_PX)).child(hero).child(styles).child(palette).child(params).child(recent).child(schedule).into_any_element()
    }

    /// The plugin is off: a compact card to turn it on or read about it.
    fn wallgen_promo(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let manifest = cx.global::<mandara_plugins::PluginRegistry>().manifests().find(|m| m.id == wg::ID).cloned();
        let tile = manifest.as_ref().map(|m| super::plugin_tile(m, PROMO_TILE_PX, false, 1.0, p));
        div()
            .id("wallgen-promo")
            .clip_rounded()
            .rounded(px(RADIUS_20_PX))
            .bg(p.surface_container.hsla())
            .p(px(SPACE_16_PX))
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_14_PX))
            .children(tile)
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_240_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Wallpaper generator")))
                    .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Make your own wallpapers and video loops on this machine: twelve styles, any palette, every detail adjustable, up to 8K. A plugin in the store; turn it on to design here.")))),
            )
            .child(
                div()
                    .row()
                    .gap(px(SPACE_8_PX))
                    .child(pill("wallgen-details", "grid", t("In the store"), false, p).on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_plugin(wg::ID, super::plugin_modal::ModalView::Detail, window, cx))))
                    .child(pill("wallgen-enable", "sparkles", t("Turn on"), true, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.plugin_lifecycle(wg::ID, Action::Enable, cx)))),
            )
            .into_any_element()
    }

    /// Queues what this frame shows and starts the next render.
    fn wallgen_prepare(&mut self, s: &WallgenSettings, r: &Recipe, cfg: &ShellConfig, cx: &mut Context<Self>) {
        let now = Instant::now();
        let main_key = logic::key(r, MAIN_W, MAIN_H);
        let mut thumbs: Vec<(u64, Recipe)> = Vec::new();
        let g = st(cx);
        if g.variation_seed == 0 {
            g.variation_seed = model::fresh_seed();
        }
        for v in variations(r, s, g.variation_seed) {
            thumbs.push((logic::key(&v, THUMB_W, THUMB_H), v));
        }
        for style in Style::ALL {
            let sr = style_recipe(style, r);
            thumbs.push((logic::key(&sr, THUMB_W, THUMB_H), sr));
        }
        g.queue.want(main_key, now);
        g.wanted = Some((main_key, r.clone()));
        g.thumbs = thumbs;
        // Keep only what is on screen (dragging makes a new key per step).
        let keep: HashSet<u64> = g.thumbs.iter().map(|(k, _)| *k).chain(g.main.as_ref().map(|(k, _)| *k)).collect();
        g.images.retain(|k, _| keep.contains(k));
        g.samples.retain(|(k, _), _| *k == main_key || g.pixels.as_ref().is_some_and(|(pk, _)| pk == k));
        if !g.display_asked {
            g.display_asked = true;
            form::job(cx, || mandara_core::query_outputs().map_err(|e| e.to_string()), |_, res, cx| {
                st(cx).display = res.ok().and_then(|o| o.first().map(|g| (g.width.max(1) as u32, g.height.max(1) as u32)));
            });
        }
        let dir = s.output_dir(&mandara_config::paths::expand_tilde(&cfg.wallpaper.directory));
        let g = st(cx);
        if g.recent_dir.as_ref() != Some(&dir) {
            g.recent_dir = Some(dir);
            g.recent = None;
        }
        if g.recent.is_none() && !g.recent_loading {
            self.wallgen_load_recent(cx);
        }
        self.wallgen_pump(cx);
    }

    /// Starts the most important render, if none is running: the main
    /// preview (when its debounce allows), its size samples, variations,
    /// then the gallery.
    fn wallgen_pump(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let g = st(cx);
        if g.busy {
            return;
        }
        match g.queue.poll(now) {
            Next::Render(key) => {
                let Some((_, recipe)) = g.wanted.clone().filter(|(k, _)| *k == key) else { return };
                g.busy = true;
                form::job(cx, move || Ok(mandara_wallgen::preview(&recipe, MAIN_W, MAIN_H)), move |view, res: Result<image::RgbImage, String>, cx| {
                    let g = st(cx);
                    g.busy = false;
                    if let Ok(rgb) = res {
                        let image = to_image(&rgb);
                        g.main = Some((key, image));
                        g.pixels = Some((key, Arc::new(rgb)));
                    }
                    g.queue.done(key, Instant::now());
                    view.wallgen_pump(cx);
                });
                return;
            }
            Next::Wait(after) => {
                if g.wake.is_none() {
                    let wake = cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(after).await;
                        let _ = this.update(cx, |view, cx| {
                            st(cx).wake = None;
                            view.wallgen_pump(cx);
                            cx.notify();
                        });
                    });
                    st(cx).wake = Some(wake);
                }
            }
            Next::Idle => {}
        }
        let g = st(cx);
        // While the main preview waits for its debounce, smaller jobs may run.
        if let Some((key, pixels)) = g.pixels.clone().filter(|(k, _)| Some(*k) == g.queue.shown() && !g.queue.stale()) {
            // Lossless is estimated from the WebP and PNG samples, not encoded.
            let missing: Vec<WallgenFormat> = WallgenFormat::ALL.into_iter().filter(|f| *f != WallgenFormat::Lossless && !g.samples.contains_key(&(key, *f))).collect();
            if !missing.is_empty() {
                g.busy = true;
                form::job(
                    cx,
                    move || {
                        let jpeg = mandara_wallgen::Format::Jpeg { quality: mandara_config::wallgen::defaults::JPEG_QUALITY };
                        Ok(missing
                            .into_iter()
                            .filter_map(|f| {
                                let format = match f {
                                    WallgenFormat::Lossless => mandara_wallgen::Format::Lossless,
                                    WallgenFormat::Webp => mandara_wallgen::Format::WebpLossless,
                                    WallgenFormat::Png => mandara_wallgen::Format::Png,
                                    WallgenFormat::Jpeg => jpeg,
                                };
                                mandara_wallgen::encode(&pixels, format).ok().map(|b| (f, b.len() as u64))
                            })
                            .collect::<Vec<_>>())
                    },
                    move |view, res, cx| {
                        let g = st(cx);
                        g.busy = false;
                        for (f, bytes) in res.unwrap_or_default() {
                            g.samples.insert((key, f), bytes);
                        }
                        view.wallgen_pump(cx);
                    },
                );
                return;
            }
        }
        let next = g.thumbs.iter().find(|(k, _)| !g.images.contains_key(k)).cloned();
        if let Some((key, recipe)) = next {
            g.busy = true;
            form::job(cx, move || Ok(mandara_wallgen::preview(&recipe, THUMB_W, THUMB_H)), move |view, res: Result<image::RgbImage, String>, cx| {
                let g = st(cx);
                g.busy = false;
                if let Ok(rgb) = res {
                    g.images.insert(key, to_image(&rgb));
                }
                view.wallgen_pump(cx);
            });
        }
    }

    fn wallgen_load_recent(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = st(cx).recent_dir.clone() else { return };
        st(cx).recent_loading = true;
        form::job(cx, move || Ok(model::library(&dir, RECENT_MAX)), |view, res, cx| {
            let g = st(cx);
            g.recent_loading = false;
            g.recent = Some(res.unwrap_or_default());
            view.wallgen_load_thumbs(cx);
        });
    }

    /// Picker thumbnails of the recent files (cached on disk), one at a time.
    fn wallgen_load_thumbs(&mut self, cx: &mut Context<Self>) {
        let g = st(cx);
        if !g.thumbs_loading.is_empty() {
            return;
        }
        let Some(path) = g.recent.as_ref().and_then(|list| list.iter().find(|e| !g.recent_thumbs.contains_key(&e.path)).map(|e| e.path.clone())) else { return };
        g.thumbs_loading.insert(path.clone());
        let p2 = path.clone();
        form::job(cx, move || mandara_ui::wallpaper::thumbnail(&p2), move |view, res, cx| {
            let g = st(cx);
            g.thumbs_loading.remove(&path);
            // A failed thumbnail shows the placeholder (and isn't retried).
            g.recent_thumbs.insert(path, res.unwrap_or_default());
            view.wallgen_load_thumbs(cx);
        });
    }

    /// Asks the shell to generate the current recipe (still or loop).
    fn wallgen_generate(&mut self, video: bool, p: &MandaraPalette, cx: &mut Context<Self>) {
        let Some(query) = cx.try_global::<crate::perf::ShellQuery>().cloned() else {
            st(cx).result = Some(Err(t("Studio isn't connected to the shell").into()));
            cx.notify();
            return;
        };
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        let (_, recipe) = current(&cfg, p.is_dark);
        let toml = recipe.to_toml();
        let g = st(cx);
        g.job = Some(Job { video, ..Job::default() });
        g.result = None;
        form::job(
            cx,
            move || {
                // The exact recipe on screen, then the job.
                (query.0)(vec!["wallgen".into(), "recipe".into(), "set".into(), toml])?;
                let mut args: Vec<String> = ["wallgen", "generate", "recipe", "--detach"].map(String::from).to_vec();
                if video {
                    args.push("--video".into());
                }
                (query.0)(args)
            },
            |view, res, cx| match res.map(|v| v["id"].as_u64()) {
                Ok(Some(id)) => {
                    if let Some(job) = st(cx).job.as_mut() {
                        job.id = Some(id);
                    }
                    view.wallgen_poll(id, cx);
                }
                Ok(None) => {
                    let g = st(cx);
                    g.job = None;
                    g.result = Some(Err(t("The shell did not start the job").into()));
                }
                Err(e) => {
                    let g = st(cx);
                    g.job = None;
                    g.result = Some(Err(e));
                }
            },
        );
        cx.notify();
    }

    /// Follows job `id` through `wallgen status` until it ends.
    fn wallgen_poll(&mut self, id: u64, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let Ok(Some(query)) = this.update(cx, |_, cx| crate::perf::query(&["wallgen", "status"], cx)) else { break };
                let status = query.await;
                let finished = this.update(cx, |view, cx| {
                    let g = st(cx);
                    let status = match status {
                        Ok(s) => s,
                        Err(e) => {
                            g.job = None;
                            g.result = Some(Err(e));
                            return true;
                        }
                    };
                    let running = &status["running"];
                    if running["id"].as_u64() == Some(id) {
                        if let Some(job) = g.job.as_mut() {
                            job.progress = running["progress"].as_f64().map(|v| v as f32);
                            job.seconds = running["seconds"].as_f64().unwrap_or(0.0);
                            job.cancelling = running["cancelling"].as_bool().unwrap_or(false);
                        }
                        cx.notify();
                        return false;
                    }
                    let last = &status["last"];
                    g.job = None;
                    g.result = Some(if last["id"].as_u64() != Some(id) {
                        Err(t("The job ended without a result").into())
                    } else if last["ok"].as_bool() == Some(true) {
                        Ok(Made {
                            path: PathBuf::from(last["path"].as_str().unwrap_or_default()),
                            bytes: last["bytes"].as_u64().unwrap_or(0),
                            width: last["width"].as_u64().unwrap_or(0) as u32,
                            height: last["height"].as_u64().unwrap_or(0) as u32,
                            seconds: last["seconds"].as_f64().unwrap_or(0.0),
                            video: last["kind"].as_str() == Some("video"),
                        })
                    } else {
                        Err(last["error"].as_str().unwrap_or_default().to_string())
                    });
                    // The new file joins Recent.
                    g.recent = None;
                    view.wallgen_load_recent(cx);
                    cx.notify();
                    true
                });
                if finished.unwrap_or(true) {
                    break;
                }
            }
        });
        st(cx).poll = Some(task);
    }

    fn wallgen_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(job) = st(cx).job.as_mut() {
            job.cancelling = true;
        }
        form::shell(vec!["wallgen".into(), "cancel".into()], cx, |_, _, _| {});
    }

    // ── Hero ──────────────────────────────────────────────────────────────

    fn wallgen_hero(&mut self, s: &WallgenSettings, r: &Recipe, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let column = self.page.column_w;
        let inner = (column - 2.0 * HERO_PAD_PX).max(0.0);
        let split = column >= SPLIT_MIN_W_PX;
        let preview_w = if split { (inner * PREVIEW_SHARE).floor() } else { inner };
        let preview_h = (preview_w * ASPECT).floor();
        let dark = p.is_dark;
        let g = st(cx);
        let stale = g.queue.stale();
        let main = g.main.as_ref().map(|(_, i)| i.clone());

        // The preview, its labels and the seed controls.
        let resolution = model::still_resolution(s, g.display);
        let (rw, rh) = resolution.size();
        let style_chip = div()
            .absolute()
            .bottom(px(SPACE_10_PX))
            .inset_start(px(SPACE_10_PX))
            .px(px(SPACE_10_PX))
            .h(px(DIM_26_PX))
            .rounded_full()
            .row()
            .items_center()
            .gap(px(SPACE_6_PX))
            .bg(p.surface_base.with_alpha(ALPHA_80).hsla())
            .text_size(px(TEXT_12_PX))
            .font_weight(FontWeight::MEDIUM)
            .child(icon("sparkles", ICON_12_PX, p.primary))
            .child(format!("{} · {}", tr(r.style.label()), tr(r.palette.label())));
        let size_chip = div()
            .absolute()
            .top(px(SPACE_10_PX))
            .inset_end(px(SPACE_10_PX))
            .px(px(SPACE_8_PX))
            .h(px(DIM_22_PX))
            .rounded_full()
            .row()
            .items_center()
            .bg(p.surface_base.with_alpha(ALPHA_80).hsla())
            .text_size(px(TEXT_11_PX))
            .font_features(mandara_ui::fonts::tabular_nums())
            .child(mandara_ui::bidi::ltr_isolate(&format!("{rw} × {rh}")));
        let preview = div().relative().child(picture(main, preview_w, preview_h, PREVIEW_RADIUS_PX, stale, p)).child(style_chip).child(size_chip);

        let locked = s.seed_locked;
        let seed_row = div()
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .child(
                div()
                    .px(px(SPACE_10_PX))
                    .h(px(DIM_32_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .bg(p.surface_container_high.hsla())
                    .text_size(px(TEXT_12_PX))
                    .child(div().text_color(p.on_surface_variant.hsla()).child(t("Seed")))
                    .child(div().font_family(fonts.mono.clone()).font_features(mandara_ui::fonts::tabular_nums()).child(r.short_seed())),
            )
            .child(icon_button("wallgen-shuffle", "refresh", tk("New layout (shuffle the seed)"), false, p).on_click(move |_: &ClickEvent, _, cx| {
                if !locked {
                    edit(dark, cx, |_, r| r.seed = model::fresh_seed());
                }
            }))
            .child(icon_button("wallgen-lock", if locked { "lock" } else { "lock-open" }, tk("Lock the layout: shuffles and variations keep this seed"), locked, p).on_click(|_: &ClickEvent, _, cx| wg::update_settings(cx, |s| s.seed_locked = !s.seed_locked)))
            .child(div().flex_1())
            .child(pill("wallgen-surprise", "sparkles", t("Surprise me"), false, p).on_click(move |_: &ClickEvent, _, cx| {
                edit(dark, cx, |s, r| {
                    let keep = s.seed_locked.then_some(r.seed);
                    *r = model::random_recipe(None, model::fresh_seed(), r.light, keep);
                });
            }));

        // Variations: click to adopt, refresh for new ones.
        let var_gap = SPACE_8_PX;
        let var_w = ((preview_w - var_gap * (VARIATIONS as f32 - 1.0)) / VARIATIONS as f32).floor().max(0.0);
        let var_h = (var_w * ASPECT).floor();
        let g = st(cx);
        let vars = variations(r, s, g.variation_seed);
        let var_tiles: Vec<AnyElement> = vars
            .into_iter()
            .enumerate()
            .map(|(i, v)| {
                let image = g.images.get(&logic::key(&v, THUMB_W, THUMB_H)).cloned();
                div()
                    .id(SharedString::from(format!("wallgen-var-{i}")))
                    .rounded(px(RADIUS_10_PX))
                    .cursor_pointer()
                    .border_2()
                    .border_color(gpui::transparent_black())
                    .hover(|st| st.border_color(p.primary.with_alpha(ALPHA_60).hsla()))
                    .active(|st| st.opacity(ALPHA_75))
                    .tooltip(super::compositor_page::tip(t("Use this variation"), p))
                    .child(picture(image, (var_w - 2.0 * BORDER_2_PX).max(0.0), (var_h - 2.0 * BORDER_2_PX).max(0.0), RADIUS_8_PX, false, p))
                    .on_click(move |_: &ClickEvent, _, cx| {
                        let v = v.clone();
                        edit(dark, cx, move |_, r| *r = v);
                    })
                    .into_any_element()
            })
            .collect();
        let variations_block = div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .child(label(t("Variations"), p))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("wallgen-new-variations")
                            .row()
                            .items_center()
                            .gap(px(SPACE_4_PX))
                            .cursor_pointer()
                            .text_size(px(TEXT_11_5_PX))
                            .text_color(p.primary.hsla())
                            .hover(|st| st.opacity(ALPHA_80))
                            .child(icon("refresh", ICON_12_PX, p.primary))
                            .child(t("New variations"))
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                st(cx).variation_seed = model::fresh_seed();
                                cx.notify();
                            })),
                    ),
            )
            .child(div().row().gap(px(var_gap)).children(var_tiles));

        let (right, video) = self.wallgen_controls(s, cfg, p, cx);
        let left = div().w(px(preview_w)).flex_none().row().flex_col().gap(px(SPACE_14_PX)).child(preview).child(seed_row).child(variations_block).child(video);
        let body = if split {
            div().row().gap(px(SPACE_20_PX)).items_start().child(left).child(div().flex_1().min_w_0().child(right))
        } else {
            div().row().flex_col().gap(px(SPACE_18_PX)).child(left).child(right)
        };
        div()
            .id("wallgen-hero")
            .clip_rounded()
            .rounded(px(HERO_RADIUS_PX))
            .bg(p.surface_container.hsla())
            .border_1()
            .border_color(p.specular_rim_light.with_alpha(ALPHA_12).hsla())
            .p(px(HERO_PAD_PX))
            .child(body)
            .into_any_element()
    }

    /// The hero's right column: title, look, size, format, actions, result.
    fn wallgen_controls(&mut self, s: &WallgenSettings, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> (AnyElement, AnyElement) {
        let fonts = mandara_ui::fonts::fonts(cx);
        let title = div()
            .row()
            .flex_col()
            .gap(px(SPACE_3_PX))
            .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Wallpaper generator")))
            .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Design a wallpaper, then generate it at full size. Every file remembers its recipe, so you can remix it later."))));

        // Light / dark.
        let tones = [(WallgenTone::Theme, tk("Follow theme")), (WallgenTone::Dark, tk("Dark")), (WallgenTone::Light, tk("Light"))];
        let tone_row = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(tones.map(|(tone, name)| {
            chip(SharedString::from(format!("wallgen-tone-{}", tone.id())), tr(name), s.tone == tone, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.tone = tone))
        }));

        // Size.
        let display = st(cx).display;
        let sizes = [
            (WallgenSize::Display, tk("Match display")),
            (WallgenSize::Fhd1080, tk("1080p")),
            (WallgenSize::Qhd1440, tk("2K")),
            (WallgenSize::Uhd4k, tk("4K")),
            (WallgenSize::Uhd5k, tk("5K")),
            (WallgenSize::Uhd8k, tk("8K")),
            (WallgenSize::Ultrawide1440, tk("Ultrawide")),
            (WallgenSize::Portrait4k, tk("Portrait")),
        ];
        let size_row = div().row().flex_wrap().gap(px(SPACE_6_PX)).children(sizes.map(|(size, name)| {
            let (w, h) = model::resolution(size, (s.custom_width, s.custom_height), display).size();
            chip(SharedString::from(format!("wallgen-size-{}", size.id())), tr(name), s.size == size, p)
                .tooltip(super::compositor_page::tip(format!("{w} × {h}"), p))
                .on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.size = size))
        }));

        // Format, with the expected size.
        let (tw, th) = model::still_resolution(s, display).size();
        let g = st(cx);
        let sample_key = g.pixels.as_ref().map(|(k, _)| *k);
        let formats = [
            (WallgenFormat::Lossless, tk("Lossless"), tk("Smallest exact file")),
            (WallgenFormat::Png, tk("PNG"), tk("Lossless")),
            (WallgenFormat::Webp, tk("WebP"), tk("Lossless")),
            (WallgenFormat::Jpeg, tk("JPEG"), tk("Lossy, smallest")),
        ];
        let (sample_px, target_px) = (u64::from(MAIN_W * MAIN_H), u64::from(tw) * u64::from(th));
        let format_row = div().row().gap(px(SPACE_8_PX)).children(formats.map(|(format, name, detail)| {
            let active = s.format == format;
            let sample = |f: WallgenFormat| sample_key.and_then(|k| g.samples.get(&(k, f)).copied());
            let bytes = match format {
                WallgenFormat::Lossless => sample(WallgenFormat::Webp).zip(sample(WallgenFormat::Png)).map(|(w, p)| logic::estimate_smallest(w, p, sample_px, target_px)),
                _ => sample(format).map(|b| logic::estimate_bytes(format, b, sample_px, target_px)),
            };
            let estimate = bytes.map(|b| tf("≈ {size}", &[("size", &model::human_bytes(b))]));
            div()
                .id(SharedString::from(format!("wallgen-format-{}", format.id())))
                .flex_1()
                .min_w_0()
                .px(px(SPACE_10_PX))
                .py(px(SPACE_8_PX))
                .rounded(px(RADIUS_12_PX))
                .cursor_pointer()
                .border_1()
                .border_color(if active { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_18).hsla() })
                .bg(if active { p.primary.with_alpha(ALPHA_12).hsla() } else { p.surface_container_high.hsla() })
                .hover(|st| st.border_color(p.primary.with_alpha(ALPHA_55).hsla()))
                .active(|st| st.opacity(ALPHA_80))
                .row()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(if active { p.primary.hsla() } else { p.on_surface.hsla() }).child(tr(name)))
                .child(div().truncate().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(tr(detail)))
                .child(div().text_size(px(TEXT_11_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface.hsla()).child(estimate.unwrap_or_else(|| "…".into())))
                .on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.format = format))
        }));

        // Actions, progress and result.
        let job = st(cx).job.clone();
        let running = job.is_some();
        let generate = div()
            .id("wallgen-generate")
            .h(px(GENERATE_H_PX))
            .w_full()
            .rounded_full()
            .row()
            .items_center()
            .justify_center()
            .gap(px(SPACE_8_PX))
            .cursor_pointer()
            .bg(if running { p.surface_container_high.hsla() } else { p.primary.hsla() })
            .text_color(if running { p.on_surface_variant.hsla() } else { p.on_primary.hsla() })
            .text_size(px(TEXT_14_PX))
            .font_weight(FontWeight::SEMIBOLD)
            .when(!running, |el| el.shadow(vec![gpui::BoxShadow { color: p.primary.with_alpha(ALPHA_30).hsla(), offset: gpui::point(px(0.0), px(SPACE_4_PX)), blur_radius: px(SPACE_14_PX), spread_radius: px(0.0), inset: false }]))
            .hover(|st| st.opacity(ALPHA_90))
            .active(|st| st.opacity(ALPHA_75))
            .child(icon("sparkles", ICON_17_PX, if running { p.on_surface_variant } else { p.on_primary }))
            .child(t("Generate wallpaper"))
            .when(!running, |el| el.on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                let p = mandara_core::frame_palette(window, cx);
                this.wallgen_generate(false, &p, cx);
            })));
        let seconds_choice = logic::SECONDS_CHOICES.map(|sec| {
            let on = (s.video.seconds - sec).abs() < f32::EPSILON;
            chip(SharedString::from(format!("wallgen-secs-{sec}")), format!("{sec:.0} {}", t("s")), on, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.video.seconds = sec))
        });
        let fps_choice = logic::FPS_CHOICES.map(|fps| {
            chip(SharedString::from(format!("wallgen-fps-{fps}")), format!("{fps} {}", t("fps")), s.video.fps == fps, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.video.fps = fps))
        });
        let (vw, vh) = model::video_resolution(s, display).size();
        let video_row = div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .p(px(SPACE_12_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_container_high.with_alpha(ALPHA_60).hsla())
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(icon("play", ICON_14_PX, p.tertiary))
                    .child(div().flex_1().min_w_0().row().flex_col().child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(t("Video loop"))).child(div().truncate().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(tf("Seamless AV1 loop at {size}", &[("size", &mandara_ui::bidi::ltr_isolate(&format!("{vw} × {vh}")))]))))
                    .when(!running, |el| el.child(pill("wallgen-video", "play", t("Generate loop"), false, p).on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        let p = mandara_core::frame_palette(window, cx);
                        this.wallgen_generate(true, &p, cx);
                    })))),
            )
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .children(seconds_choice)
                    .child(div().w(px(BORDER_1_PX)).h(px(DIM_18_PX)).mx(px(SPACE_4_PX)).bg(p.outline.with_alpha(ALPHA_25).hsla()))
                    .children(fps_choice),
            );

        let progress = job.map(|j| self.wallgen_progress(&j, p, cx));
        let result = st(cx).result.clone().map(|res| self.wallgen_result(res, p, cx));

        // Where files go, and whether they become the wallpaper.
        let dir = s.output_dir(&mandara_config::paths::expand_tilde(&cfg.wallpaper.directory));
        let folder = self.wallgen_folder_field(s, cx);
        let apply = s.apply;
        let output = div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(icon("folder", ICON_14_PX, p.on_surface_variant))
                    .child(div().flex_1().min_w_0().child(folder))
                    .child(icon_button("wallgen-open-folder", "external", tk("Show in folder"), false, p).on_click(move |_: &ClickEvent, _, _| {
                        let _ = std::fs::create_dir_all(&dir);
                        reveal(&dir);
                    })),
            )
            .child(
                div()
                    .id("wallgen-apply")
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .cursor_pointer()
                    .child(div().flex_1().text_size(px(TEXT_12_5_PX)).child(t("Set as wallpaper when it's done")))
                    .child(form::switch(apply, p))
                    .on_click(|_: &ClickEvent, _, cx| wg::update_settings(cx, |s| s.apply = !s.apply)),
            );

        let right = div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(title)
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Mode"), p)).child(tone_row))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Size"), p)).child(size_row))
            .child(div().row().flex_col().gap(px(SPACE_6_PX)).child(label(t("Format"), p)).child(format_row))
            .child(generate)
            .children(progress)
            .children(result)
            .child(output)
            .into_any_element();
        (right, video_row.into_any_element())
    }

    /// The output folder field (empty: `Generated` in the wallpaper folder).
    fn wallgen_folder_field(&mut self, s: &WallgenSettings, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(f) = st(cx).folder_field.clone() {
            f.update(cx, |f, _| f.sync(&s.directory));
            return f;
        }
        let field = cx.new(|cx| {
            TextField::new(
                s.directory.clone(),
                t("Folder (empty: Generated in your wallpaper folder)"),
                false,
                Rc::new(|value, _, cx| {
                    let value = value.trim().to_string();
                    wg::update_settings(cx, move |s| s.directory = value);
                }),
                cx,
            )
        });
        st(cx).folder_field = Some(field.clone());
        field
    }

    fn wallgen_progress(&mut self, job: &Job, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        // Stills report no fraction: a sweep shows it's alive.
        let (start, share) = match job.progress {
            Some(f) => (0.0, f.clamp(PROGRESS_MIN, 1.0)),
            None => {
                let phase = (job.seconds / SWEEP_PERIOD_S).fract() as f32;
                ((phase * (1.0 + SWEEP_SHARE) - SWEEP_SHARE).max(0.0), SWEEP_SHARE)
            }
        };
        let text = match (job.cancelling, job.progress, job.video) {
            (true, ..) => t("Cancelling…").to_string(),
            (false, Some(f), _) => tf("Encoding the loop… {percent}%", &[("percent", &((f * 100.0).round() as u32))]),
            (false, None, true) => t("Starting the loop…").to_string(),
            (false, None, false) => t("Rendering at full size…").to_string(),
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_8_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(div().flex_1().text_size(px(TEXT_12_5_PX)).font_features(mandara_ui::fonts::tabular_nums()).child(text))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(format!("{:.0} {}", job.seconds, t("s"))))
                    .when(!job.cancelling, |el| el.child(pill("wallgen-cancel", "close", t("Cancel"), false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.wallgen_cancel(cx))))),
            )
            .child(
                div()
                    .relative()
                    .h(px(PROGRESS_H_PX))
                    .w_full()
                    .rounded_full()
                    .clip_rounded()
                    .bg(p.surface_container_high.hsla())
                    .child(div().absolute().top_0().bottom_0().inset_start(gpui::relative(start)).w(gpui::relative(share)).rounded_full().bg(p.primary.hsla())),
            )
            .into_any_element()
    }

    fn wallgen_result(&mut self, res: Result<Made, String>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let made = match res {
            Ok(m) => m,
            Err(e) => return form::note(&e, true, p),
        };
        let name = made.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let detail = format!("{} × {} · {} · {:.1} {}", made.width, made.height, model::human_bytes(made.bytes), made.seconds, t("s"));
        let (set_path, show_path, remix_path) = (made.path.clone(), made.path.clone(), made.path.clone());
        div()
            .p(px(SPACE_12_PX))
            .rounded(px(RADIUS_16_PX))
            .bg(p.success_emerald.with_alpha(ALPHA_10).hsla())
            .border_1()
            .border_color(p.success_emerald.with_alpha(ALPHA_25).hsla())
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(icon(if made.video { "play" } else { "check" }, ICON_15_PX, p.success_emerald))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .row()
                            .flex_col()
                            .child(div().truncate().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).child(name))
                            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(mandara_ui::bidi::ltr_isolate(&detail))),
                    ),
            )
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .gap(px(SPACE_6_PX))
                    .child(pill("wallgen-result-set", "image", t("Set as wallpaper"), true, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        form::shell(vec!["wallpaper".into(), "set".into(), set_path.display().to_string()], cx, |_, res, cx| {
                            if let Err(e) = res {
                                st(cx).result = Some(Err(e));
                            }
                        });
                    })))
                    .child(icon_button("wallgen-result-show", "folder", tk("Show in folder"), false, p).on_click(move |_: &ClickEvent, _, _| reveal(&show_path)))
                    .child(icon_button("wallgen-result-remix", "sliders", tk("Remix: load its recipe"), false, p).on_click(move |_: &ClickEvent, _, cx| {
                        if let Some(recipe) = mandara_wallgen::recipe_for(&remix_path) {
                            wg::update_settings(cx, |s| model::store_recipe(s, &recipe));
                        }
                    })),
            )
            .into_any_element()
    }

    // ── Styles ────────────────────────────────────────────────────────────

    fn wallgen_styles(&mut self, r: &Recipe, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let fit = grid::fit(self.page.column_w, STYLE_GRID).balanced(Style::ALL.len());
        let thumb_w = (fit.card_w - 2.0 * (SPACE_6_PX + BORDER_2_PX)).max(0.0);
        let thumb_h = (thumb_w * ASPECT).floor();
        let dark = p.is_dark;
        let g = st(cx);
        let cards: Vec<AnyElement> = Style::ALL
            .into_iter()
            .map(|style| {
                let active = r.style == style;
                let image = g.images.get(&logic::key(&style_recipe(style, r), THUMB_W, THUMB_H)).cloned();
                div()
                    .id(SharedString::from(format!("wallgen-style-{}", style.id())))
                    .p(px(SPACE_6_PX))
                    .rounded(px(RADIUS_18_PX))
                    .cursor_pointer()
                    .bg(if active { p.primary.with_alpha(ALPHA_10).hsla() } else { p.surface_container.hsla() })
                    .border_2()
                    .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                    .hover(|st| st.border_color(p.primary.with_alpha(ALPHA_55).hsla()))
                    .active(|st| st.opacity(ALPHA_80))
                    .child(picture(image, thumb_w, thumb_h, RADIUS_12_PX, false, p))
                    .child(
                        div()
                            .px(px(SPACE_4_PX))
                            .pt(px(SPACE_8_PX))
                            .pb(px(SPACE_4_PX))
                            .row()
                            .flex_col()
                            .gap(px(LINE_GAP_PX))
                            .child(
                                div()
                                    .row()
                                    .items_center()
                                    .gap(px(SPACE_6_PX))
                                    .child(div().flex_1().truncate().font_family(fonts.display.clone()).text_size(px(TEXT_13_5_PX)).font_weight(FontWeight::SEMIBOLD).text_color(if active { p.primary.hsla() } else { p.on_surface.hsla() }).child(tr(style.label())))
                                    .when(active, |el| el.child(icon("check", ICON_13_PX, p.primary))),
                            )
                            .child(div().h(px(DIM_32_PX)).text_size(px(TEXT_11_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(tr(style.description())))),
                    )
                    .on_click(move |_: &ClickEvent, _, cx| {
                        // A new style keeps the palette, seed and the common
                        // parameters; its own start from their defaults.
                        edit(dark, cx, move |_, r| {
                            let common: std::collections::BTreeMap<String, f32> = r.params.iter().filter(|(k, _)| mandara_wallgen::COMMON_PARAMS.iter().any(|s| s.key == k.as_str())).map(|(k, v)| (k.clone(), *v)).collect();
                            r.style = style;
                            r.params = common;
                        });
                    })
                    .into_any_element()
            })
            .collect();
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Styles"), t("Twelve looks, each with its own controls. Previews use your palette and seed."), Vec::new(), fonts.display.clone(), p))
            .child(fit.render(cards))
            .into_any_element()
    }

    // ── Palette ───────────────────────────────────────────────────────────

    fn wallgen_palette(&mut self, r: &Recipe, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let fit = grid::fit(self.page.column_w, PALETTE_GRID).balanced(mandara_wallgen::palettes().len() + 2);
        let dark = p.is_dark;
        let tile = |id: SharedString, name: String, colors: Vec<Color>, active: bool| {
            div()
                .id(id)
                .p(px(SPACE_8_PX))
                .rounded(px(RADIUS_14_PX))
                .cursor_pointer()
                .bg(if active { p.primary.with_alpha(ALPHA_10).hsla() } else { p.surface_container.hsla() })
                .border_2()
                .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                .hover(|st| st.border_color(p.primary.with_alpha(ALPHA_55).hsla()))
                .active(|st| st.opacity(ALPHA_80))
                .row()
                .flex_col()
                .gap(px(SPACE_6_PX))
                .child(strip(&colors, STRIP_H_PX, RADIUS_8_PX))
                .child(div().row().items_center().gap(px(SPACE_4_PX)).child(div().flex_1().truncate().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).child(name)).when(active, |el| el.child(icon("check", ICON_12_PX, p.primary))))
        };
        let mut tiles: Vec<AnyElement> = Vec::new();
        let theme = model::theme_palette(p);
        let theme_active = matches!(r.palette, PaletteSpec::Theme(_));
        tiles.push(
            tile("wallgen-palette-theme".into(), t("Match theme").into(), hex_colors(&theme), theme_active)
                .on_click(move |_: &ClickEvent, _, cx| {
                    let theme = theme.clone();
                    edit(dark, cx, move |_, r| r.palette = theme);
                })
                .into_any_element(),
        );
        for np in mandara_wallgen::palettes() {
            let active = r.palette == PaletteSpec::Named(np.id.into());
            let colors: Vec<Color> = np.colors.iter().filter_map(|h| Color::from_hex(h).ok()).collect();
            let id = np.id;
            tiles.push(tile(SharedString::from(format!("wallgen-palette-{id}")), tr(np.label), colors, active).on_click(move |_: &ClickEvent, _, cx| edit(dark, cx, move |_, r| r.palette = PaletteSpec::Named(id.into()))).into_any_element());
        }
        let custom_active = matches!(r.palette, PaletteSpec::Custom(_));
        let custom_colors = if custom_active { hex_colors(&r.palette) } else { hex_colors(&r.palette).into_iter().rev().collect() };
        let start_stops = r.palette.hex_stops();
        tiles.push(
            tile("wallgen-palette-custom".into(), t("Custom").into(), custom_colors, custom_active)
                .on_click(move |_: &ClickEvent, _, cx| {
                    // Start from the colors on screen.
                    let stops = start_stops.clone();
                    edit(dark, cx, move |_, r| {
                        if !matches!(r.palette, PaletteSpec::Custom(_)) {
                            r.palette = PaletteSpec::Custom(stops.into_iter().take(mandara_wallgen::MAX_STOPS).collect());
                        }
                    });
                })
                .into_any_element(),
        );
        let editor = custom_active.then(|| self.wallgen_stops(r, p, cx));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Palette"), t("Curated sets, your theme's colors, or your own. Every blend happens in OKLab, so gradients stay clean."), Vec::new(), fonts.display.clone(), p))
            .child(fit.render(tiles))
            .children(editor)
            .into_any_element()
    }

    /// The custom palette's stops: swatch, hex field, remove; add.
    fn wallgen_stops(&mut self, r: &Recipe, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let dark = p.is_dark;
        let stops = r.palette.hex_stops();
        let n = stops.len();
        let mut row = div().row().flex_wrap().items_center().gap(px(SPACE_10_PX));
        for (i, hex) in stops.iter().enumerate() {
            let field = match st(cx).stop_fields.get(&i).cloned() {
                Some(f) => {
                    f.update(cx, |f, _| f.sync(hex));
                    f
                }
                None => {
                    let f = cx.new(|cx| {
                        TextField::new(
                            hex.clone(),
                            "#rrggbb",
                            false,
                            Rc::new(move |value, _, cx| {
                                let value = value.trim().to_string();
                                if mandara_wallgen::parse_hex(&value).is_none() {
                                    return;
                                }
                                let value = if value.starts_with('#') { value } else { format!("#{value}") };
                                edit(dark, cx, move |_, r| {
                                    if let PaletteSpec::Custom(stops) = &mut r.palette
                                        && let Some(slot) = stops.get_mut(i)
                                    {
                                        *slot = value;
                                    }
                                });
                            }),
                            cx,
                        )
                    });
                    st(cx).stop_fields.insert(i, f.clone());
                    f
                }
            };
            let color = Color::from_hex(hex).unwrap_or(p.surface_container_high);
            row = row.child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .p(px(SPACE_4_PX))
                    .rounded(px(RADIUS_12_PX))
                    .bg(p.surface_container.hsla())
                    .child(div().size(px(STOP_SWATCH_PX)).rounded(px(RADIUS_8_PX)).bg(color.hsla()).border_1().border_color(p.outline.with_alpha(ALPHA_25).hsla()))
                    .child(div().w(px(STOP_FIELD_W_PX)).child(field))
                    .when(n > mandara_wallgen::MIN_STOPS, |el| {
                        el.child(icon_button(SharedString::from(format!("wallgen-stop-del-{i}")), "close", tk("Remove this color"), false, p).on_click(move |_: &ClickEvent, _, cx| {
                            edit(dark, cx, move |_, r| {
                                if let PaletteSpec::Custom(stops) = &mut r.palette
                                    && i < stops.len()
                                {
                                    stops.remove(i);
                                }
                            });
                        }))
                    }),
            );
        }
        if n < mandara_wallgen::MAX_STOPS {
            row = row.child(pill("wallgen-stop-add", "plus", t("Add color"), false, p).on_click(move |_: &ClickEvent, _, cx| {
                edit(dark, cx, |_, r| {
                    if let PaletteSpec::Custom(stops) = &mut r.palette {
                        let next = stops.last().cloned().unwrap_or_else(|| "#808080".into());
                        stops.push(next);
                    }
                });
            }));
        }
        row.into_any_element()
    }

    // ── Fine-tune ─────────────────────────────────────────────────────────

    fn wallgen_params(&mut self, r: &Recipe, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let fit = grid::fit(self.page.column_w, PARAM_GRID).at_most(2);
        let dark = p.is_dark;
        let own: Vec<&'static ParamSpec> = r.style.params().iter().collect();
        let mut groups: Vec<(String, Vec<&'static ParamSpec>)> = vec![(tf("{style} controls", &[("style", &tr(r.style.label()))]), own)];
        for (title, _) in logic::COMMON_GROUPS {
            groups.push((tr(title), logic::common_group(title)));
        }
        let more = logic::common_group(logic::MORE_GROUP);
        if !more.is_empty() {
            groups.push((tr(logic::MORE_GROUP), more));
        }
        let cards: Vec<AnyElement> = groups
            .into_iter()
            .filter(|(_, specs)| !specs.is_empty())
            .enumerate()
            .map(|(gi, (title, specs))| {
                let pristine = logic::at_defaults(r, &specs);
                let reset_specs = specs.clone();
                let rows = specs.iter().map(|spec| {
                    let spec: &'static ParamSpec = spec;
                    let v = r.param(spec.key);
                    div()
                        .h(px(ROW_H_PX))
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .child(div().flex_1().min_w_0().truncate().text_size(px(TEXT_12_5_PX)).child(tr(spec.label)))
                        .child(div().w(px(SLIDER_W_PX)).flex_none().child(mandara_ui::kit::slider(SharedString::from(format!("wallgen-p-{}", spec.key)), logic::to_fraction(spec, v), p, move |f, _, cx| {
                            let value = logic::from_fraction(spec, f);
                            edit(dark, cx, move |_, r| r.set_param(spec.key, value));
                        })))
                        .child(div().w(px(VALUE_W_PX)).flex_none().text_end().text_size(px(TEXT_12_PX)).font_features(mandara_ui::fonts::tabular_nums()).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::ltr_isolate(&logic::value_label(spec, v))))
                });
                div()
                    .clip_rounded()
                    .rounded(px(RADIUS_20_PX))
                    .bg(p.surface_container.hsla())
                    .px(px(SPACE_16_PX))
                    .py(px(SPACE_12_PX))
                    .row()
                    .flex_col()
                    .gap(px(LINE_GAP_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .pb(px(SPACE_4_PX))
                            .child(div().flex_1().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(title))
                            .when(!pristine, |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("wallgen-reset-{gi}")))
                                        .row()
                                        .items_center()
                                        .gap(px(SPACE_4_PX))
                                        .px(px(SPACE_8_PX))
                                        .h(px(DIM_24_PX))
                                        .rounded_full()
                                        .cursor_pointer()
                                        .text_size(px(TEXT_11_5_PX))
                                        .text_color(p.on_surface_variant.hsla())
                                        .hover(|st| st.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
                                        .active(|st| st.opacity(ALPHA_75))
                                        .child(icon("undo", ICON_12_PX, p.on_surface_variant))
                                        .child(t("Reset"))
                                        .on_click(move |_: &ClickEvent, _, cx| {
                                            let specs = reset_specs.clone();
                                            edit(dark, cx, move |_, r| *r = logic::reset(r, &specs));
                                        }),
                                )
                            }),
                    )
                    .children(rows)
                    .into_any_element()
            })
            .collect();
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Fine-tune"), t("Every variable of the style, then tone, light, texture and layout. The preview follows as you drag."), Vec::new(), fonts.display.clone(), p))
            .child(fit.render(cards))
            .into_any_element()
    }

    // ── Recent ────────────────────────────────────────────────────────────

    fn wallgen_recent(&mut self, s: &WallgenSettings, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let dir = s.output_dir(&mandara_config::paths::expand_tilde(&cfg.wallpaper.directory));
        let shown = st(cx).recent.as_ref().map_or(0, Vec::len);
        let fit = grid::fit(self.page.column_w, RECENT_GRID).balanced(shown);
        let thumb_w = fit.card_w;
        let thumb_h = (thumb_w * ASPECT).floor();
        let g = st(cx);
        let confirm = g.confirm_delete.clone();
        let list = g.recent.clone();
        let recent_thumbs = g.recent_thumbs.clone();
        let tiles: Vec<AnyElement> = list
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                let thumb = recent_thumbs.get(&e.path).filter(|t| !t.as_os_str().is_empty()).cloned();
                let name = e.path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let confirming = confirm.as_ref() == Some(&e.path);
                let (remix, set, del) = (e.recipe.clone(), e.path.clone(), e.path.clone());
                let slug = SharedString::from(format!("{:x}", logic::key(&e.recipe, e.bytes as u32, 0)));
                div()
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(
                        div()
                            .relative()
                            .w(px(thumb_w))
                            .h(px(thumb_h))
                            .rounded(px(RADIUS_14_PX))
                            .clip_rounded()
                            .bg(p.surface_container_high.hsla())
                            .child(match thumb {
                                Some(t) => img(t).size_full().object_fit(gpui::ObjectFit::Cover).into_any_element(),
                                None => div().size_full().row().items_center().justify_center().child(icon("image", ICON_22_PX, p.on_surface_variant.with_alpha(ALPHA_50))).into_any_element(),
                            })
                            .when(e.video, |el| el.child(glass_tag("play", t("Video"), p).absolute().top(px(SPACE_8_PX)).inset_start(px(SPACE_8_PX))))
                            // The actions float on the picture in a glass pill.
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(SPACE_8_PX))
                                    .inset_end(px(SPACE_8_PX))
                                    .p(px(SPACE_3_PX))
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .gap(px(SPACE_3_PX))
                                    .bg(p.surface_base.with_alpha(ALPHA_80).hsla())
                                    .child(icon_button(SharedString::from(format!("wallgen-recent-remix-{slug}")), "sliders", tk("Remix: load its recipe"), false, p).on_click(move |_: &ClickEvent, _, cx| {
                                let recipe = remix.clone();
                                wg::update_settings(cx, move |s| model::store_recipe(s, &recipe));
                            }))
                            .child(icon_button(SharedString::from(format!("wallgen-recent-set-{slug}")), "image", tk("Set as wallpaper"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                form::shell(vec!["wallpaper".into(), "set".into(), set.display().to_string()], cx, |_, _, _| {});
                            })))
                            .child(
                                div()
                                    .id(SharedString::from(format!("wallgen-recent-del-{slug}")))
                                    .h(px(DIM_32_PX))
                                    .min_w(px(DIM_32_PX))
                                    .px(px(if confirming { SPACE_10_PX } else { 0.0 }))
                                    .flex_none()
                                    .rounded_full()
                                    .row()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(SPACE_4_PX))
                                    .cursor_pointer()
                                    .bg(if confirming { p.urgent_coral.hsla() } else { p.surface_container_high.hsla() })
                                    .text_color(p.on_primary.hsla())
                                    .text_size(px(TEXT_11_5_PX))
                                    .hover(|st| st.opacity(ALPHA_85))
                                    .active(|st| st.opacity(ALPHA_70))
                                    .tooltip(super::compositor_page::tip(if confirming { t("Click again to delete the file") } else { t("Delete") }, p))
                                    .child(icon("trash", ICON_14_PX, if confirming { p.on_primary } else { p.urgent_coral }))
                                    .when(confirming, |el| el.child(t("Delete?")))
                                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                                        let g = st(cx);
                                        if g.confirm_delete.as_ref() != Some(&del) {
                                            g.confirm_delete = Some(del.clone());
                                        } else {
                                            g.confirm_delete = None;
                                            if let Err(e) = model::delete(&del) {
                                                st(cx).result = Some(Err(e));
                                            }
                                            st(cx).recent = None;
                                            view.wallgen_load_recent(cx);
                                        }
                                        cx.notify();
                                    })),
                            ),
                    ),
                    )
                    .child(
                        div()
                            .px(px(SPACE_4_PX))
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(div().id(SharedString::from(format!("wallgen-recent-name-{slug}"))).flex_1().min_w_0().truncate().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::MEDIUM).tooltip(super::compositor_page::tip(name, p)).child(tr(e.recipe.style.label())))
                            .child(div().flex_none().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(model::human_bytes(e.bytes))),
                    )
                    .into_any_element()
            })
            .collect();
        let count = tiles.len();
        let open = dir.clone();
        let trailing = vec![pill("wallgen-recent-open", "folder", t("Open folder"), false, p).on_click(move |_: &ClickEvent, _, _| {
            let _ = std::fs::create_dir_all(&open);
            reveal(&open);
        })
        .into_any_element()];
        let body = if count == 0 {
            div()
                .py(px(SPACE_22_PX))
                .rounded(px(RADIUS_20_PX))
                .bg(p.surface_container.hsla())
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_6_PX))
                .child(icon("sparkles", ICON_22_PX, p.on_surface_variant.with_alpha(ALPHA_60)))
                .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(if st(cx).recent_loading { t("Looking for generated wallpapers…") } else { t("Nothing generated yet. Your wallpapers appear here.") }))
                .into_any_element()
        } else {
            fit.render(tiles).into_any_element()
        };
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Recent"), tf("Generated wallpapers in {folder}; the picker lists them too.", &[("folder", &mandara_ui::bidi::ltr_isolate(&tilde(&dir)))]), trailing, fonts.display.clone(), p))
            .child(body)
            .into_any_element()
    }

    // ── Schedule ──────────────────────────────────────────────────────────

    fn wallgen_schedule(&mut self, s: &WallgenSettings, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let on = s.schedule.enabled;
        let state = schedule::ScheduleState::load(&schedule::ScheduleState::path());
        let now = SystemTime::now();
        let next = schedule::next_run(&s.schedule, true, state.last(), now);
        let status = match next {
            None => t("Off. Nothing runs in the background until you turn it on.").to_string(),
            Some(at) => {
                let secs = at.duration_since(now).map(|d| d.as_secs()).unwrap_or(0);
                if secs == 0 { t("Next wallpaper: due now (shortly after the shell starts)").to_string() } else { tf("Next wallpaper {when}", &[("when", &in_label(secs))]) }
            }
        };
        let header = div()
            .id("wallgen-schedule-toggle")
            .row()
            .items_center()
            .gap(px(SPACE_12_PX))
            .cursor_pointer()
            .child(div().size(px(DIM_40_PX)).flex_none().rounded(px(RADIUS_12_PX)).bg(if on { p.primary.with_alpha(ALPHA_16).hsla() } else { p.surface_container_high.hsla() }).row().items_center().justify_center().child(icon("clock", ICON_17_PX, if on { p.primary } else { p.on_surface_variant })))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .row()
                    .flex_col()
                    .gap(px(LINE_GAP_PX))
                    .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Generate on a schedule")))
                    .child(div().text_size(px(TEXT_12_PX)).text_color(if on { p.primary.hsla() } else { p.on_surface_variant.hsla() }).font_features(mandara_ui::fonts::tabular_nums()).child(status)),
            )
            .child(form::switch(on, p))
            .on_click(|_: &ClickEvent, _, cx| wg::update_settings(cx, |s| s.schedule.enabled = !s.schedule.enabled));

        let every = logic::EVERY_CHOICES.map(|m| {
            chip(SharedString::from(format!("wallgen-every-{m}")), schedule::every_label(m), s.schedule.every_minutes == m, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.schedule.every_minutes = m))
        });
        let modes = [(ScheduleMode::Random, tk("A random style")), (ScheduleMode::Variation, tk("A variation of my recipe")), (ScheduleMode::Reseed, tk("My recipe, new layout"))];
        let mode_row = modes.map(|(mode, name)| chip(SharedString::from(format!("wallgen-mode-{}", mode.id())), tr(name), s.schedule.mode == mode, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.schedule.mode = mode)));
        let kinds = [(WallgenKind::Still, tk("Still image")), (WallgenKind::Video, tk("Video loop"))];
        let kind_row = kinds.map(|(kind, name)| {
            let id = if kind == WallgenKind::Video { "video" } else { "still" };
            chip(SharedString::from(format!("wallgen-kind-{id}")), tr(name), s.schedule.kind == kind, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.schedule.kind = kind))
        });
        let keep = logic::KEEP_CHOICES.map(|n| {
            let text = if n == 0 { t("All").to_string() } else { n.to_string() };
            chip(SharedString::from(format!("wallgen-keep-{n}")), text, s.schedule.keep == n, p).on_click(move |_: &ClickEvent, _, cx| wg::update_settings(cx, move |s| s.schedule.keep = n))
        });
        let line = |title: &'static str, controls: AnyElement| {
            div()
                .row()
                .flex_wrap()
                .items_center()
                .gap(px(SPACE_12_PX))
                .child(div().w(px(DIM_140_PX)).flex_none().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t(title)))
                .child(div().flex_1().min_w(px(DIM_240_PX)).child(controls))
        };
        let apply = s.schedule.apply;
        let body = div()
            .row()
            .flex_col()
            .gap(px(SPACE_12_PX))
            .pt(px(SPACE_14_PX))
            .border_t_1()
            .border_color(p.outline.with_alpha(ALPHA_12).hsla())
            .when(!on, |el| el.opacity(ALPHA_60))
            .child(line(tk("Every"), div().row().flex_wrap().gap(px(SPACE_6_PX)).children(every).into_any_element()))
            .child(line(tk("Make"), div().row().flex_wrap().gap(px(SPACE_6_PX)).children(mode_row).into_any_element()))
            .child(line(tk("As"), div().row().flex_wrap().gap(px(SPACE_6_PX)).children(kind_row).into_any_element()))
            .child(line(tk("Keep the newest"), div().row().flex_wrap().gap(px(SPACE_6_PX)).children(keep).into_any_element()))
            .child(line(
                tk("Set as wallpaper"),
                div().id("wallgen-schedule-apply").row().items_center().gap(px(SPACE_10_PX)).cursor_pointer().child(form::switch(apply, p)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Each new one replaces the current wallpaper"))).on_click(|_: &ClickEvent, _, cx| wg::update_settings(cx, |s| s.schedule.apply = !s.schedule.apply)).into_any_element(),
            ));
        div()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(section_head(t("Schedule"), t("Opt-in: a fresh wallpaper every few hours. Missed runs never pile up; older files are cleaned up."), Vec::new(), fonts.display.clone(), p))
            .child(div().id("wallgen-schedule").clip_rounded().rounded(px(RADIUS_20_PX)).bg(p.surface_container.hsla()).p(px(SPACE_16_PX)).row().flex_col().child(header).child(body))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_labels() {
        assert!(in_label(30).contains("30"));
        assert!(in_label(5 * 3600 + 12 * 60).contains('5'));
        assert!(in_label(2 * 86_400).contains('2'));
    }

    #[test]
    fn style_thumbnails_keep_palette_and_seed() {
        let mut r = Recipe::random(Some(Style::Aurora), 3);
        r.light = true;
        let t = style_recipe(Style::Dunes, &r);
        assert_eq!((t.style, t.seed, t.light, t.palette.clone()), (Style::Dunes, r.seed, true, r.palette.clone()));
        assert!(t.params.is_empty(), "the style's defaults");
        let s = WallgenSettings::default();
        let v = variations(&r, &s, 10);
        assert_eq!(v.len(), VARIATIONS);
        assert!(v.iter().all(|x| x.style == r.style));
        assert_ne!(v[0], v[1]);
    }

    #[test]
    fn images_convert_to_bgra() {
        let mut rgb = image::RgbImage::new(2, 1);
        rgb.put_pixel(0, 0, image::Rgb([10, 20, 30]));
        let img = to_image(&rgb);
        let frame = img.as_bytes(0).unwrap();
        assert_eq!(&frame[..4], &[30, 20, 10, 255]);
    }
}
