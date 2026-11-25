//! Studio → Themes → Cursors: the cursor theme gallery and every cursor
//! setting (the opt-in *Cursor themes* plugin; while it is off, the tab
//! shows its gate, `plugin_gate.rs`).
//!
//! * **Your cursor** — the applied theme drawn from its own XCursor files
//!   (pointer, link, text, busy, precision, resize and move) at the chosen
//!   size, the common sizes as chips, and the cursor rows of the registry
//!   (size slider, hide while typing / when idle, Hyprland's software
//!   cursor and hide on touch; rows a compositor ignores say so).
//! * **Gallery** — the ten catalog themes (`mandara_services::cursors`),
//!   searchable and filtered by tone: glass cards that lift on hover, a
//!   preview strip whose busy cursor plays its animation while hovered,
//!   author, licence, variants as dots, and one action that follows the
//!   theme's state: Get → Installing (progress, cancel) → Apply → Active,
//!   Remove for what Mandara installed, errors in place with Retry.
//! * **Your own cursors** — an archive, a folder or a link
//!   (`theme_galleries::import_panel`).
//! * **Also installed** — themes from packages, your own folders or
//!   imports, as cards with their own previews.
//!
//! Downloads and decoding run off the UI thread; previews are decoded at
//! the drawn size and dropped when you leave the tab. Cards are keyboard
//! reachable: Tab moves between them, Enter / Space runs the action, ← / →
//! (mirrored right-to-left) pick a variant and Delete removes (twice, to
//! confirm).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::input;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_services::cursors::catalog::{CATALOG, CursorTheme, CursorVariant};
use mandara_services::cursors::{self, Role, install, xcursor};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::compositor_page::{badge, button, chip, target_name, tip};
use super::plugin_gate::Gate;
use super::theme_galleries::{self as g, ImportKind, Job, PERCENT_PER_PERMILLE};
use super::themes_page::section_head;
use super::{StudioView, grid};
use crate::registry::system_settings::{CURSOR, CURSOR_THEME_PATH};
use crate::registry::{self, Section};
use crate::tokens::*;

/// Theme cards.
const CURSOR_GRID: CardGrid = CardGrid { min_w: 250.0, max_w: 380.0, gap: SPACE_16_PX };
/// A card's preview strip: its height and the size cursors are drawn at.
const STRIP_H_PX: f32 = DIM_76_PX;
const STRIP_CURSOR_PX: u32 = 24;
/// Previews are decoded for screens up to this density (sharp at 1.5× and
/// 2× without decoding per window scale).
const PREVIEW_DENSITY: f32 = 2.0;
/// Most frames of a hovered busy cursor's animation kept (sampled evenly).
const ANIMATION_FRAMES: usize = 24;
/// The common sizes offered as chips.
const SIZE_PRESETS: [u32; 4] = [24, 32, 48, 64];
/// The size a theme is shown at while the compositor keeps its own.
const DEFAULT_SIZE: u32 = 24;
/// "Your cursor" tiles: the space around the cursor, and the smallest tile
/// (small sizes still get a comfortable tile).
const TILE_PAD_PX: f32 = SPACE_14_PX;
const TILE_MIN_PX: f32 = DIM_60_PX;
/// Tiles across "Your cursor" (three rows of three shapes).
const TILE_COLUMNS: usize = 3;
/// "Your cursor" and the setting rows sit side by side from this column
/// width (else they stack), the panel taking this share of the row (the
/// rows' sliders need the rest).
const SPLIT_MIN_W_PX: f32 = 2.0 * DIM_420_PX;
const PANEL_SHARE: f32 = 0.42;
/// A variant dot, and its ring when picked.
const DOT_PX: f32 = DIM_18_PX;
/// How much a card's swatch tints its preview strip.
const STRIP_TINT: f32 = 0.22;
/// Below this contrast between a cursor's body and the strip, the strip
/// leans this far toward the opposite tone, so dark cursors stay visible on
/// a dark theme (and light ones on a light theme).
const STRIP_MIN_CONTRAST: f32 = 2.2;
const STRIP_LEAN: f32 = 0.5;
const DIAGONAL_DEG: f32 = 135.0;
/// A variant whose body is darker than this is a dark cursor (tone filter).
const DARK_BODY: f32 = 0.5;
/// Tone filter chips.
const TONES: [&str; 3] = [tk("All"), tk("Dark"), tk("Light")];

/// What the tab shows while the plugin is off.
const GATE: Gate = Gate {
    will: &[
        tk("Lists ten popular open-source cursor themes and downloads the ones you pick from their authors' GitHub releases, checked against a pinned checksum"),
        tk("Installs them, or an archive, folder or link of your own, to ~/.local/share/icons"),
        tk("Sets the cursor and its size live in your compositor and in Mandara's compositor drop-in"),
    ],
    wont: &[
        tk("Change your cursor until you pick one"),
        tk("Run anything that comes with a theme"),
        tk("Remove themes it didn't install"),
    ],
};

/// A decoded preview: the image and the logical size it is drawn at.
#[derive(Clone)]
struct CursorImage {
    image: Arc<RenderImage>,
    w: f32,
    h: f32,
}

/// One preview set: an image per [`Role::ALL`] entry (`None` when the
/// theme lacks that shape).
type Previews = Rc<[Option<CursorImage>]>;

#[derive(Default)]
struct CursorStudio {
    /// Installed theme names (every icon folder), read once per visit.
    installed: Option<Rc<[String]>>,
    /// The ones Mandara installed (and may remove), with whether each was
    /// checked against a pinned checksum.
    ours: Option<Rc<[(String, bool)]>>,
    /// When the lists were read: the manifest's and the user icon folder's
    /// modification times (an install from `mandara msg cursor …` changes
    /// them, so the gallery follows without a revisit).
    stamp: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// The variant each card shows (catalog id → index).
    picked: HashMap<&'static str, usize>,
    jobs: HashMap<&'static str, Job>,
    /// The last failure per theme name, shown on its card with Retry.
    errors: HashMap<String, String>,
    /// Decoded previews by (theme, logical px).
    previews: HashMap<(String, u32), Previews>,
    loading: HashSet<(String, u32)>,
    /// The busy cursor's animation per theme (hovered cards only).
    animations: HashMap<String, Option<CursorImage>>,
    animating: HashSet<String>,
    /// The theme whose card the pointer is over.
    hovered: Option<String>,
    /// The tone filter ([`TONES`] index).
    tone: usize,
    /// A remove waiting for its confirming second press.
    confirm_remove: Option<String>,
    /// Why X11 apps may not follow the last apply.
    note: Option<String>,
}
impl Global for CursorStudio {}

impl CursorStudio {
    /// Stores the previews decoded for `key` and hands back the sets they
    /// replace, for the atlas. `None` (and nothing stored) when leaving the
    /// page cancelled the request: a late result never refills the cache.
    fn landed(&mut self, key: (String, u32), images: Vec<Option<CursorImage>>) -> Option<Vec<Previews>> {
        if !self.loading.remove(&key) {
            return None;
        }
        // One live size per theme besides the strip's: dragging the size
        // slider doesn't pile up every step.
        let replaced = self.previews.extract_if(|(t, size), _| *t == key.0 && *size != STRIP_CURSOR_PX).map(|(_, set)| set).collect();
        self.previews.insert(key, Rc::from(images));
        Some(replaced)
    }
}

fn st(cx: &mut gpui::App) -> &mut CursorStudio {
    cx.default_global::<CursorStudio>()
}

/// Drops what the tab read from disk (on leaving it or Themes). Running
/// installs keep going.
pub(super) fn release(cx: &mut gpui::App) {
    let s = st(cx);
    s.installed = None;
    s.ours = None;
    let previews = std::mem::take(&mut s.previews);
    let animations = std::mem::take(&mut s.animations);
    s.loading.clear();
    s.animating.clear();
    s.hovered = None;
    s.confirm_remove = None;
    // Their textures go too (Studio calls this outside window updates).
    for set in previews.into_values() {
        for preview in set.iter().flatten() {
            cx.drop_image(preview.image.clone(), None);
        }
    }
    for preview in animations.into_values().flatten() {
        cx.drop_image(preview.image, None);
    }
}

fn swatch(v: &CursorVariant) -> Color {
    let [_, r, g, b] = v.swatch.to_be_bytes();
    Color::from_rgba8(r, g, b, u8::MAX)
}

/// The backdrop cursors of body colour `tint` are drawn on.
fn backdrop(p: &MandaraPalette, tint: Option<Color>) -> gpui::Background {
    let base = p.surface_container_high;
    let Some(tint) = tint else { return base.hsla().into() };
    let bg = if tint.contrast_ratio(base) >= STRIP_MIN_CONTRAST { base } else { base.mix(tint.best_on(p.on_surface, p.surface_base), STRIP_LEAN) };
    mandara_core::oklab_gradient(DIAGONAL_DEG, bg, bg.mix(tint, STRIP_TINT))
}

/// Makes `theme` the cursor (`None`: your compositor's own), and points X11
/// apps at it through `~/.icons/default` when that file is Mandara's.
fn apply_theme(theme: Option<String>, cx: &mut gpui::App) {
    let next = theme.clone();
    update_config(cx, move |c| c.compositor.input.cursor.theme = next);
    st(cx).note = match input::sync_default_cursor(theme.as_deref()) {
        Ok(true) => None,
        Ok(false) => theme.map(|theme| tf("~/.icons/default/index.theme is yours, so X11 apps keep its cursor; change it there for {theme}.", &[("theme", &theme)])),
        Err(e) => Some(e.to_string()),
    };
}

fn set_size(size: Option<u32>, cx: &mut gpui::App) {
    update_config(cx, move |c| c.compositor.input.cursor.size = size);
}

fn set_tone(i: usize, cx: &mut gpui::App) {
    st(cx).tone = i;
}

/// Whether `theme` passes the tone filter.
fn tone_matches(theme: &CursorTheme, tone: usize) -> bool {
    match tone {
        1 => theme.variants.iter().any(|v| swatch(v).lightness() < DARK_BODY),
        2 => theme.variants.iter().any(|v| swatch(v).lightness() >= DARK_BODY),
        _ => true,
    }
}

/// What a card's action does for the picked variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CardState {
    Get,
    Installing(u32),
    Failed,
    Apply,
    Active,
}

impl StudioView {
    fn cursor_lists(cx: &mut Context<Self>) -> (Rc<[String]>, Rc<[(String, bool)]>) {
        let env = install::Env::current();
        let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let stamp = (mtime(&env.manifest), mtime(&env.icons));
        let s = st(cx);
        if s.installed.is_none() || s.ours.is_none() || s.stamp != stamp {
            s.stamp = stamp;
            s.installed = Some(input::cursor_themes().into());
            s.ours = Some(install::installed(&env).into_iter().map(|i| (i.theme, i.verified)).collect());
        }
        (s.installed.clone().unwrap_or_else(|| Rc::from([])), s.ours.clone().unwrap_or_else(|| Rc::from([])))
    }

    pub(super) fn cursor_refresh(cx: &mut gpui::App) {
        let s = st(cx);
        s.installed = None;
        s.ours = None;
    }

    /// The previews of `theme` at `logical` px, decoded off the UI thread
    /// on first use (`None` until ready).
    fn cursor_previews(theme: &str, logical: u32, cx: &mut Context<Self>) -> Option<Previews> {
        let key = (theme.to_string(), logical);
        if let Some(p) = st(cx).previews.get(&key) {
            return Some(p.clone());
        }
        if !st(cx).loading.insert(key.clone()) {
            return None;
        }
        let job = key.clone();
        let task = cx.background_executor().spawn(async move {
            let dirs = input::icon_dirs();
            let wanted = (job.1 as f32 * PREVIEW_DENSITY).round() as u32;
            Role::ALL
                .iter()
                .map(|role| {
                    let i = cursors::preview(&job.0, *role, wanted, &dirs)?;
                    let k = job.1 as f32 / i.nominal.max(1) as f32;
                    let (w, h) = (i.width as f32 * k, i.height as f32 * k);
                    Some(CursorImage { image: mandara_ui::backdrop::widgets::render_image(i.to_rgba_image()?), w, h })
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let images = task.await;
            let _ = this.update(cx, |_, cx| {
                let Some(dropped) = st(cx).landed(key, images) else { return };
                for set in dropped {
                    for preview in set.iter().flatten() {
                        cx.drop_image(preview.image.clone(), None);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// The busy cursor of `theme` as an animation at the strip size,
    /// decoded off the UI thread when its card is first hovered.
    fn cursor_animation(theme: &str, cx: &mut Context<Self>) -> Option<CursorImage> {
        if let Some(a) = st(cx).animations.get(theme) {
            return a.clone();
        }
        if !st(cx).animating.insert(theme.to_string()) {
            return None;
        }
        let name = theme.to_string();
        let task = cx.background_executor().spawn(async move {
            let dirs = input::icon_dirs();
            let wanted = (STRIP_CURSOR_PX as f32 * PREVIEW_DENSITY).round() as u32;
            let path = cursors::find_cursor(&name, Role::Wait, &dirs)?;
            let frames = xcursor::load_frames(&path, wanted, ANIMATION_FRAMES).ok()?;
            let first = frames.first()?;
            let k = STRIP_CURSOR_PX as f32 / first.nominal.max(1) as f32;
            let (w, h) = (first.width as f32 * k, first.height as f32 * k);
            let mut out: smallvec::SmallVec<[image::Frame; 1]> = smallvec::SmallVec::with_capacity(frames.len());
            for f in frames {
                let delay = image::Delay::from_numer_denom_ms(f.delay_ms.max(1), 1);
                let mut rgba = f.to_rgba_image()?;
                for px in rgba.as_chunks_mut::<4>().0 {
                    px.swap(0, 2);
                }
                out.push(image::Frame::from_parts(rgba, 0, 0, delay));
            }
            Some(CursorImage { image: Arc::new(RenderImage::new(out)), w, h })
        });
        let key = theme.to_string();
        cx.spawn(async move |this, cx| {
            let image = task.await;
            let _ = this.update(cx, |_, cx| {
                // Left since: the late animation is dropped.
                if !st(cx).animating.remove(&key) {
                    if let Some(i) = image {
                        cx.drop_image(i.image, None);
                    }
                    return;
                }
                st(cx).animations.insert(key, image);
                cx.notify();
            });
        })
        .detach();
        None
    }

    fn cursor_install(&mut self, theme: &'static CursorTheme, variant: &'static CursorVariant, cx: &mut Context<Self>) {
        let name = variant.theme;
        if st(cx).jobs.contains_key(name) {
            return;
        }
        let job = Job::new();
        let (progress, cancel) = (job.progress.clone(), job.cancel.clone());
        let s = st(cx);
        s.errors.remove(name);
        s.jobs.insert(name, job);
        let task = cx.background_executor().spawn(async move {
            let env = install::Env::current();
            install::install(theme.id, variant, &env, &|p| progress.store((p.fraction() * g::PERMILLE) as u32, Ordering::Relaxed), &cancel)
        });
        Self::repaint_while(cx, move |cx| st(cx).jobs.contains_key(name));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                let s = st(cx);
                s.jobs.remove(name);
                match result {
                    Ok(_) => {
                        s.previews.retain(|(t, _), _| t != name);
                        Self::cursor_refresh(cx);
                    }
                    Err(e) => {
                        s.errors.insert(name.to_string(), e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn cursor_remove(&mut self, name: String, cx: &mut Context<Self>) {
        if st(cx).confirm_remove.as_deref() != Some(name.as_str()) {
            st(cx).confirm_remove = Some(name);
            cx.notify();
            return;
        }
        st(cx).confirm_remove = None;
        let theme = name.clone();
        let task = cx.background_executor().spawn(async move { install::remove(&theme, &install::Env::current()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                match result {
                    Ok(_) => {
                        // A removed theme can't stay the cursor.
                        let active = mandara_ui::state::config(cx).read(cx).get().compositor.input.cursor.theme.clone();
                        if active.as_deref() == Some(name.as_str()) {
                            apply_theme(None, cx);
                        }
                        st(cx).previews.retain(|(t, _), _| *t != name);
                        Self::cursor_refresh(cx);
                    }
                    Err(e) => {
                        st(cx).errors.insert(name, e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The card's primary action for its state.
    fn cursor_act(&mut self, theme: &'static CursorTheme, variant: &'static CursorVariant, state: CardState, cx: &mut Context<Self>) {
        match state {
            CardState::Get | CardState::Failed => self.cursor_install(theme, variant, cx),
            CardState::Installing(_) => {
                if let Some(job) = st(cx).jobs.get(variant.theme) {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            CardState::Apply => apply_theme(Some(variant.theme.to_string()), cx),
            CardState::Active => {}
        }
        cx.notify();
    }

    /// Opens Themes on its Cursors tab (`mandara studio themes/cursors`,
    /// the older `themes/cursor` and `cursor` links, and the Input page's
    /// link).
    pub fn show_cursor_section(&mut self, cx: &mut Context<Self>) {
        self.show_themes_tab(super::looks_page::ThemesTab::Cursors, cx);
    }

    /// The Cursors tab of the Themes page.
    pub(super) fn cursor_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Cursor themes are an opt-in plugin: off, the tab offers it and
        // nothing here reads or writes the cursor.
        if !cfg.cursors_enabled() {
            return self.plugin_gate(mandara_ui::plugins::cursors::ID, &GATE, p, window, cx);
        }
        g::step_lifts(cfg, window, cx);
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let current = cfg.compositor.input.cursor.theme.clone();
        let reset = chip("cursor-your-config", t("Your config"), current.is_none(), p)
            .tab_index(0)
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| apply_theme(None, cx)))
            .into_any_element();
        let head = section_head(
            t("Cursors"),
            tf("{count} popular themes, downloaded on demand from their authors' GitHub releases, checked against a pinned checksum and installed to ~/.local/share/icons, plus your own from an archive, a folder or a link", &[("count", &CATALOG.len())]),
            vec![reset],
            display,
            p,
        );
        let yours = self.cursor_yours(cfg, p, cx);
        let search = self.gallery_search(ImportKind::Cursors, t("Search cursor themes…").to_string(), p, cx);
        let tone = st(cx).tone;
        let tones = self.filter_chips("cursor-tone", TONES.iter().map(|l| tr(l)).collect(), tone, set_tone, p, cx);
        let toolbar = div().row().flex_wrap().items_center().gap(px(SPACE_12_PX)).child(tones).child(div().flex_1()).child(search);
        let gallery = self.cursor_gallery(cfg, p, cx);
        let import = self.import_panel(ImportKind::Cursors, p, cx);
        let others = self.cursor_others(cfg, p, cx);
        div().row().flex_col().gap(px(SPACE_16_PX)).child(head).child(yours).child(toolbar).child(gallery).child(import).children(others).into_any_element()
    }

    /// "Your cursor": the applied theme at its size, size chips, and the
    /// cursor setting rows.
    fn cursor_yours(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let c = &cfg.compositor.input.cursor;
        let size = c.size.unwrap_or(DEFAULT_SIZE);
        // With no theme set, show what toolkit apps fall back to.
        let shown = c.theme.clone().or_else(|| std::env::var("XCURSOR_THEME").ok()).unwrap_or_else(|| "default".into());
        let title = match (&c.theme, catalog_name(&shown)) {
            (Some(_), Some(name)) => name,
            (Some(t), None) => t.clone(),
            (None, _) => t("Your compositor's cursor").to_string(),
        };
        let previews = Self::cursor_previews(&shown, size, cx);
        let tile_bg = backdrop(p, cursors::catalog::by_theme_name(&shown).map(|(_, v)| swatch(v)));
        let tile = (size as f32 + 2.0 * TILE_PAD_PX).max(TILE_MIN_PX);
        let tiles: Vec<AnyElement> = Role::ALL
            .iter()
            .enumerate()
            .map(|(i, role)| {
                let image = previews.as_ref().and_then(|p| p.get(i).cloned().flatten());
                div()
                    .flex_1()
                    .min_w(px(tile))
                    .row()
                    .flex_col()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .child(
                        div()
                            .w_full()
                            .h(px(tile))
                            .rounded(px(RADIUS_14_PX))
                            .bg(tile_bg)
                            .row()
                            .items_center()
                            .justify_center()
                            .child(match image {
                                Some(i) => img(i.image).w(px(i.w)).h(px(i.h)).object_fit(gpui::ObjectFit::Fill).into_any_element(),
                                None => div().size(px(DIM_8_PX)).rounded_full().bg(p.on_surface_variant.with_alpha(ALPHA_30).hsla()).into_any_element(),
                            }),
                    )
                    .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(tr(role.label())))
                    .into_any_element()
            })
            .collect();
        let mut rows_of_tiles = div().row().flex_col().gap(px(SPACE_8_PX));
        let mut iter = tiles.into_iter();
        loop {
            let row: Vec<AnyElement> = iter.by_ref().take(TILE_COLUMNS).collect();
            if row.is_empty() {
                break;
            }
            rows_of_tiles = rows_of_tiles.child(div().row().gap(px(SPACE_8_PX)).children(row));
        }
        let size_chips = div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_6_PX))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(t("Size")))
            .child(chip("cursor-size-default", t("Your config"), c.size.is_none(), p).tab_index(0).on_click(cx.listener(|_, _: &ClickEvent, _, cx| set_size(None, cx))))
            .children(SIZE_PRESETS.iter().map(|n| {
                let n = *n;
                chip(SharedString::from(format!("cursor-size-{n}")), format!("{n} px"), c.size == Some(n), p).tab_index(0).font_features(mandara_ui::fonts::tabular_nums()).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| set_size(Some(n), cx)))
            }));
        let status = match registry::running_compositor() {
            Some(target) => tf("Applies live on {compositor}; apps started from now on use it too.", &[("compositor", &target_name(target))]),
            None => tf("Saved now; applied the next time Mandara runs under a supported compositor ({compositors}).", &[("compositors", &mandara_config::compositor::Target::ALL.map(mandara_config::compositor::Target::name).join(", "))]),
        };
        let note = st(cx).note.clone();
        let panel = div()
            .flex_1()
            .min_w(px(0.0))
            .rounded(px(RADIUS_22_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Your cursor")))
                            .child(div().truncate().font_family(fonts.display.clone()).text_size(px(TEXT_20_PX)).font_weight(FontWeight::SEMIBOLD).child(title)),
                    )
                    .child(badge(format!("{size} px"), p.primary).font_features(mandara_ui::fonts::tabular_nums())),
            )
            .child(rows_of_tiles)
            .child(size_chips)
            .child(div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(status)))
            .children(note.map(|n| div().text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.warning_amber.hsla()).child(mandara_ui::bidi::paragraph(n))));
        let rows = self.rows(registry::in_section(Section::Themes).filter(|s| s.group == CURSOR && s.path != CURSOR_THEME_PATH), cfg, p, cx);
        let split = self.page.column_w >= SPLIT_MIN_W_PX && !self.stack_rows;
        let panel_w = ((self.page.column_w - SPACE_16_PX) * PANEL_SHARE).floor();
        div()
            .row()
            .when(!split, |el| el.flex_col())
            .gap(px(SPACE_16_PX))
            .child(if split { panel.w(px(panel_w)).flex_none() } else { panel })
            .child(div().flex_1().min_w(px(0.0)).child(rows))
            .into_any_element()
    }

    fn cursor_gallery(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let (installed, ours) = Self::cursor_lists(cx);
        let active = cfg.compositor.input.cursor.theme.clone();
        let query = Self::gallery_query(ImportKind::Cursors, cx);
        let tone = st(cx).tone;
        let shown: Vec<&'static CursorTheme> = CATALOG
            .iter()
            .filter(|th| tone_matches(th, tone))
            .filter(|th| {
                // No search: nothing to compare (and nothing to format).
                if query.is_empty() {
                    return true;
                }
                let labels: Vec<String> = th.variants.iter().map(|v| tr(v.label)).collect();
                let mut fields: Vec<&str> = vec![th.name, th.author, th.license];
                let desc = tr(th.description);
                fields.push(&desc);
                fields.extend(labels.iter().map(String::as_str));
                fields.extend(th.variants.iter().map(|v| v.theme));
                g::matches(&query, &fields)
            })
            .collect();
        if shown.is_empty() {
            return div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("No cursor theme matches.")).into_any_element();
        }
        let fit = grid::fit(self.page.column_w, CURSOR_GRID);
        let cards: Vec<AnyElement> = shown.into_iter().map(|theme| self.cursor_card(theme, active.as_deref(), &installed, &ours, p, cx)).collect();
        fit.render(cards)
    }

    /// The preview strip of installed theme `name` (animating the busy
    /// cursor while `hovered`).
    fn cursor_strip(name: &str, hovered: bool, p: &MandaraPalette, cx: &mut Context<Self>) -> gpui::Div {
        let previews = Self::cursor_previews(name, STRIP_CURSOR_PX, cx);
        let animation = if hovered { Self::cursor_animation(name, cx) } else { None };
        match previews {
            Some(images) => div().row().items_center().justify_around().px(px(SPACE_8_PX)).children(Role::STRIP.iter().map(|role| {
                let still = Role::ALL.iter().position(|r| r == role).and_then(|i| images.get(i).cloned().flatten());
                let i = if *role == Role::Wait { animation.clone().or(still) } else { still };
                div()
                    .id(SharedString::from(format!("cursor-{name}-{role:?}")))
                    .size(px(DIM_40_PX))
                    .row()
                    .items_center()
                    .justify_center()
                    .tooltip(tip(tr(role.label()), p))
                    .children(i.map(|i| img(i.image).w(px(i.w)).h(px(i.h)).object_fit(gpui::ObjectFit::Fill).id(SharedString::from(format!("cursor-img-{name}-{role:?}")))))
            })),
            None => div().row().items_center().justify_center().child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Loading preview…"))),
        }
    }

    fn cursor_card(&mut self, theme: &'static CursorTheme, active: Option<&str>, installed: &[String], ours: &[(String, bool)], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let is_installed = |v: &CursorVariant| installed.iter().any(|n| n == v.theme);
        // The picked variant: your choice, else the active one, an installed
        // one, or the first.
        let picked = st(cx)
            .picked
            .get(theme.id)
            .copied()
            .or_else(|| theme.variants.iter().position(|v| Some(v.theme) == active))
            .or_else(|| theme.variants.iter().position(&is_installed))
            .unwrap_or(0)
            .min(theme.variants.len() - 1);
        let variant: &'static CursorVariant = &theme.variants[picked];
        let job = st(cx).jobs.get(variant.theme).map(Job::permille);
        let error = st(cx).errors.get(variant.theme).cloned();
        let state = match (job, error.is_some(), is_installed(variant), Some(variant.theme) == active) {
            (Some(pm), ..) => CardState::Installing(pm),
            (None, true, false, _) => CardState::Failed,
            (None, _, true, true) => CardState::Active,
            (None, _, true, false) => CardState::Apply,
            (None, _, false, _) => CardState::Get,
        };
        let card_active = theme.variants.iter().any(|v| Some(v.theme) == active);
        let removable = ours.iter().any(|(n, _)| n == variant.theme) && is_installed(variant) && job.is_none();
        let arming = st(cx).confirm_remove.as_deref() == Some(variant.theme);
        let tint = swatch(variant);
        let card_id = SharedString::from(format!("cursor-card-{}", theme.id));
        let amount = g::lift(&card_id, cx);
        let hovered = st(cx).hovered.as_deref() == Some(variant.theme);

        // The preview strip.
        let strip_bg = if is_installed(variant) { backdrop(p, Some(tint)) } else { backdrop(p, None) };
        let strip = if is_installed(variant) {
            Self::cursor_strip(variant.theme, hovered, p, cx)
        } else {
            div()
                .row()
                .items_center()
                .justify_center()
                .gap(px(SPACE_10_PX))
                .child(div().size(px(DIM_40_PX)).rounded_full().bg(tint.hsla()).border_2().border_color(tint.best_on(p.on_surface, p.surface_base).with_alpha(ALPHA_30).hsla()).row().items_center().justify_center().child(icon("pointer", ICON_17_PX, tint.best_on(Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX), Color::from_rgba8(0, 0, 0, u8::MAX)))))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Preview after download")))
        };
        let strip = div().h(px(STRIP_H_PX)).rounded(px(RADIUS_16_PX)).bg(strip_bg).overflow_hidden().relative().child(strip.size_full()).when(state == CardState::Active, |el| el.child(active_badge(p)));

        // Variant dots.
        let rtl = mandara_i18n::language().is_rtl();
        let dots = div().row().items_center().gap(px(SPACE_6_PX)).children(theme.variants.iter().enumerate().map(|(i, v)| {
            let on = i == picked;
            let id = theme.id;
            div()
                .id(SharedString::from(format!("cursor-dot-{}-{}", theme.id, v.id)))
                .size(px(DOT_PX))
                .rounded_full()
                .cursor_pointer()
                .bg(swatch(v).hsla())
                .border_2()
                .border_color(if on { p.primary.hsla() } else { p.outline.with_alpha(ALPHA_30).hsla() })
                .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_60).hsla()))
                .tooltip(tip(tr(v.label), p))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    st(cx).picked.insert(id, i);
                    st(cx).confirm_remove = None;
                    cx.notify();
                }))
        }));

        // The action row.
        let (label, glyph, filled): (String, &'static str, bool) = match state {
            CardState::Get => (tf("Get · {size}", &[("size", &mandara_ui::widgets::system::format_bytes(variant.asset.size))]), "download", false),
            CardState::Installing(pm) => (tf("Installing {percent}% · Cancel", &[("percent", &(pm / PERCENT_PER_PERMILLE))]), "close", false),
            CardState::Failed => (t("Retry").to_string(), "refresh", false),
            CardState::Apply => (t("Apply").to_string(), "check", true),
            CardState::Active => (t("Active").to_string(), "check", false),
        };
        let primary = button(SharedString::from(format!("cursor-act-{}", variant.theme)), glyph, label, filled, p)
            .when(state == CardState::Active, |el| el.opacity(ALPHA_75).cursor_default())
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cursor_act(theme, variant, state, cx)));
        let remove = removable.then(|| {
            let name = variant.theme.to_string();
            g::remove_button(SharedString::from(format!("cursor-rm-{name}")), arming, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cursor_remove(name.clone(), cx)))
        });
        let progress = job.map(|pm| g::progress_bar(pm, p));
        let error_row = error.map(|e| g::error_row(e, p));
        let n = theme.variants.len();
        let hover_name = variant.theme;
        let frame = g::lift_frame(&card_id).on_hover(cx.listener({
            let id = card_id.clone();
            move |_, h: &bool, _, cx| {
                g::set_hover(id.clone(), *h, cx);
                let s = st(cx);
                if *h {
                    s.hovered = Some(hover_name.to_string());
                } else if s.hovered.as_deref() == Some(hover_name) {
                    s.hovered = None;
                }
                cx.notify();
            }
        }));
        let card = g::glass_card(card_id, tint, amount, p)
            .flex_1()
            .tab_index(0)
            .p(px(SPACE_8_PX))
            .border_2()
            .border_color(if card_active { p.primary.hsla() } else { gpui::transparent_black() })
            .group_hover(g::CARD_GROUP, |s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
            .focus_visible(|s| s.border_color(p.primary.with_alpha(ALPHA_80).hsla()))
            .row()
            .flex_col()
            .gap(px(SPACE_10_PX))
            .child(strip)
            .child(
                div()
                    .px(px(SPACE_6_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_6_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_8_PX))
                            .child(div().flex_1().min_w(px(0.0)).truncate().text_start().font_family(fonts.display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(theme.name))
                            .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(g::stars(theme.stars))),
                    )
                    .child(div().row().items_center().gap(px(SPACE_6_PX)).child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(theme.author)).child(badge(theme.license, p.tertiary)))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(dots).child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).child(tr(variant.label))))
                    .child(div().h(px(DIM_32_PX)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(tr(theme.description))))
                    .children(progress)
                    .children(error_row),
            )
            .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().items_center().gap(px(SPACE_6_PX)).child(primary).child(div().flex_1()).children(remove).child(g::homepage_button(SharedString::from(format!("cursor-home-{}", theme.id)), theme.homepage, p)))
            .on_key_down(cx.listener(move |this, e: &gpui::KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
                    return;
                }
                let step = |forward: bool| if forward ^ rtl { 1 } else { n - 1 };
                match k.key.as_str() {
                    "enter" | "space" => this.cursor_act(theme, variant, state, cx),
                    "right" => {
                        st(cx).picked.insert(theme.id, (picked + step(true)) % n);
                        cx.notify();
                    }
                    "left" => {
                        st(cx).picked.insert(theme.id, (picked + step(false)) % n);
                        cx.notify();
                    }
                    "delete" | "backspace" if removable => this.cursor_remove(variant.theme.to_string(), cx),
                    _ => return,
                }
                cx.stop_propagation();
            }));
        frame.child(card).into_any_element()
    }

    /// Installed themes outside the catalog (packages, your own folders,
    /// imports), as cards with their own previews.
    fn cursor_others(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (installed, ours) = Self::cursor_lists(cx);
        let query = Self::gallery_query(ImportKind::Cursors, cx);
        let others: Vec<String> = installed.iter().filter(|n| cursors::catalog::by_theme_name(n).is_none() && g::matches(&query, &[n.as_str()])).cloned().collect();
        if others.is_empty() {
            return None;
        }
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let current = cfg.compositor.input.cursor.theme.clone();
        let fit = grid::fit(self.page.column_w, CURSOR_GRID);
        let cards: Vec<AnyElement> = others
            .into_iter()
            .map(|name| {
                let active = current.as_deref() == Some(name.as_str());
                let mine = ours.iter().find(|(n, _)| *n == name).map(|(_, verified)| *verified);
                let card_id = SharedString::from(format!("cursor-other-{name}"));
                let amount = g::lift(&card_id, cx);
                let hovered = st(cx).hovered.as_deref() == Some(name.as_str());
                let strip = div().h(px(STRIP_H_PX)).rounded(px(RADIUS_16_PX)).bg(backdrop(p, None)).overflow_hidden().relative().child(Self::cursor_strip(&name, hovered, p, cx).size_full()).when(active, |el| el.child(active_badge(p)));
                let origin = match mine {
                    Some(false) => g::tag(t("Imported · unverified"), p.warning_amber),
                    Some(true) => g::tag(t("Installed by Mandara"), p.primary),
                    None => g::tag(t("From a package or your folders"), p.on_surface_variant),
                };
                let arming = st(cx).confirm_remove.as_deref() == Some(name.as_str());
                let apply_name = name.clone();
                let action = button(SharedString::from(format!("cursor-other-act-{name}")), "check", if active { t("Active") } else { t("Apply") }, !active, p)
                    .when(active, |el| el.opacity(ALPHA_75).cursor_default())
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        if !active {
                            apply_theme(Some(apply_name.clone()), cx);
                        }
                    }));
                let remove = mine.is_some().then(|| {
                    let n = name.clone();
                    g::remove_button(SharedString::from(format!("cursor-other-rm-{name}")), arming, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cursor_remove(n.clone(), cx)))
                });
                let error = st(cx).errors.get(&name).cloned().map(|e| g::error_row(e, p));
                let hover_name = name.clone();
                let hover_id = card_id.clone();
                let frame = g::lift_frame(&card_id).on_hover(cx.listener(move |_, h: &bool, _, cx| {
                    g::set_hover(hover_id.clone(), *h, cx);
                    let s = st(cx);
                    if *h {
                        s.hovered = Some(hover_name.clone());
                    } else if s.hovered.as_deref() == Some(hover_name.as_str()) {
                        s.hovered = None;
                    }
                    cx.notify();
                }));
                let card = g::glass_card(card_id, p.secondary, amount, p)
                    .flex_1()
                    .p(px(SPACE_8_PX))
                    .border_2()
                    .border_color(if active { p.primary.hsla() } else { gpui::transparent_black() })
                    .group_hover(g::CARD_GROUP, |s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_10_PX))
                    .child(strip)
                    .child(div().px(px(SPACE_6_PX)).row().flex_col().gap(px(SPACE_6_PX)).child(div().truncate().text_start().font_family(display.clone()).text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(name.clone())).child(div().row().child(origin)).children(error))
                    .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().items_center().gap(px(SPACE_6_PX)).child(action).child(div().flex_1()).children(remove));
                frame.child(card).into_any_element()
            })
            .collect();
        Some(div().row().flex_col().gap(px(SPACE_10_PX)).child(div().px(px(SPACE_4_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Also installed"))).child(fit.render(cards)).into_any_element())
    }
}

/// The "Active" pill over a strip's corner.
fn active_badge(p: &MandaraPalette) -> gpui::Div {
    div()
        .absolute()
        .top(px(SPACE_8_PX))
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
        .child(t("Active"))
}

/// "Bibata Modern · Ice" for a catalog theme name.
fn catalog_name(theme: &str) -> Option<String> {
    cursors::catalog::by_theme_name(theme).map(|(t, v)| format!("{} · {}", t.name, tr(v.label)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_presets_fit_the_slider() {
        use crate::registry::system_settings::{CURSOR_SIZE_MAX, CURSOR_SIZE_MIN};
        let _ = crate::registry::by_path(crate::registry::system_settings::CURSOR_SIZE_PATH).expect("the size row");
        assert!(SIZE_PRESETS.iter().all(|s| (CURSOR_SIZE_MIN..=CURSOR_SIZE_MAX).contains(&f64::from(*s))));
        assert!(SIZE_PRESETS.contains(&DEFAULT_SIZE));
        assert!(Role::STRIP.iter().all(|r| Role::ALL.contains(r)));
        assert!(Role::STRIP.contains(&Role::Wait), "the strip shows the busy cursor it animates");
    }

    fn preview_set() -> Vec<Option<CursorImage>> {
        let frame = image::Frame::new(image::RgbaImage::new(2, 2));
        vec![Some(CursorImage { image: Arc::new(RenderImage::new(smallvec::smallvec![frame])), w: 2.0, h: 2.0 }), None]
    }

    /// Leaving Themes cancels pending decodes; their late results stay out,
    /// and a newer size of a theme replaces the older one (for the atlas).
    #[test]
    fn late_previews_never_refill_a_released_cache() {
        let mut s = CursorStudio::default();
        let key = ("Bibata".to_string(), 32);
        s.loading.insert(key.clone());
        s.loading.clear(); // the page was left
        assert!(s.landed(key.clone(), preview_set()).is_none());
        assert!(s.previews.is_empty());

        let strip = ("Bibata".to_string(), STRIP_CURSOR_PX);
        s.loading.insert(strip.clone());
        assert_eq!(s.landed(strip.clone(), preview_set()).map(|r| r.len()), Some(0));
        s.loading.insert(key.clone());
        assert_eq!(s.landed(key, preview_set()).map(|r| r.len()), Some(0));
        let bigger = ("Bibata".to_string(), 48);
        s.loading.insert(bigger.clone());
        assert_eq!(s.landed(bigger.clone(), preview_set()).map(|r| r.len()), Some(1), "the 32 px set is handed back");
        assert!(s.previews.contains_key(&bigger) && s.previews.contains_key(&strip) && s.previews.len() == 2);
    }

    #[test]
    fn swatches_names_and_tones() {
        let v = &CATALOG[0].variants[0];
        assert_eq!(swatch(v).to_rgba8()[3], u8::MAX);
        assert_eq!(catalog_name("Bibata-Modern-Ice").as_deref(), Some("Bibata Modern · Ice"));
        assert_eq!(catalog_name("Adwaita"), None);
        // Every theme passes "All"; the tone chips split the rest.
        assert!(CATALOG.iter().all(|th| tone_matches(th, 0)));
        assert!(CATALOG.iter().any(|th| tone_matches(th, 1)) && CATALOG.iter().any(|th| tone_matches(th, 2)));
        let bibata = cursors::catalog::find("bibata").unwrap();
        assert!(tone_matches(bibata, 1) && tone_matches(bibata, 2), "Classic is dark, Ice is light");
        assert_eq!(TONES.len(), 3);
    }

    #[test]
    fn every_cursor_setting_is_on_the_cursors_tab() {
        let rows: Vec<_> = registry::in_section(Section::Themes).filter(|s| s.group == CURSOR).map(|s| s.path).collect();
        for p in [CURSOR_THEME_PATH, crate::registry::system_settings::CURSOR_SIZE_PATH, "/compositor/input/cursor/hide_when_typing", "/compositor/input/cursor/hide_after_secs", "/compositor/input/cursor/no_hardware_cursors", "/compositor/input/cursor/hide_on_touch"] {
            assert!(rows.contains(&p), "{p}");
        }
        // Search finds them.
        assert!(registry::settings().iter().any(|s| s.path == CURSOR_THEME_PATH && s.matches("cursor theme")));
        assert!(GATE.will.len() >= 2 && !GATE.wont.is_empty());
    }
}
