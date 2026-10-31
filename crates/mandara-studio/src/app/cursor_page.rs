//! Studio → Themes → Cursor: the cursor theme gallery and every cursor
//! setting.
//!
//! * **Your cursor** — the applied theme drawn from its own XCursor files
//!   (pointer, link, text, busy, precision, resize and move) at the chosen
//!   size, the common sizes as chips, and the cursor rows of the registry
//!   (size slider, hide while typing / when idle, Hyprland's software
//!   cursor and hide on touch; rows a compositor ignores say so).
//! * **Gallery** — the ten catalog themes (`mandara_services::cursors`):
//!   a preview strip, author, licence, variants as dots, and one action
//!   that follows the theme's state: Get → Installing (progress, cancel) →
//!   Apply → Active, Remove for what Mandara installed, errors in place
//!   with Retry. Downloads and decoding run off the UI thread; previews
//!   are decoded at the drawn size and dropped when you leave the page.
//! * **Also installed** — themes from packages or your own folders.
//!
//! Cards are keyboard reachable: Tab moves between them, Enter / Space
//! runs the action, ← / → (mirrored right-to-left) pick a variant and
//! Delete removes (twice, to confirm).

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use gpui::{AnyElement, Bounds, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, Pixels, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, canvas, div, img, point, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_config::input;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_services::cursors::catalog::{CATALOG, CursorTheme, CursorVariant};
use mandara_services::cursors::{self, Role, install};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::state::update_config;

use super::compositor_page::{badge, button, chip, target_name, tip};
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
const HORIZONTAL_DEG: f32 = 90.0;
/// The install progress bar.
const PROGRESS_H_PX: f32 = DIM_5_PX;
/// The bar never draws narrower than this share (it reads as "started").
const PROGRESS_MIN: f32 = 0.03;
/// Repaint interval while an install runs (progress moves; stops with it).
const JOB_REPAINT: Duration = Duration::from_millis(120);
/// Deep links: frames to wait for the section's anchor, the re-check delay
/// and the space kept above it.
const ANCHOR_FRAMES: u8 = 2;
const ANCHOR_SETTLE: Duration = Duration::from_millis(32);
const SCROLL_MARGIN_PX: f32 = SPACE_12_PX;
const PERMILLE: f32 = 1000.0;
const PERCENT_PER_PERMILLE: u32 = 10;
/// Stars above this read as "4.2k".
const THOUSAND: f32 = 1000.0;

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

/// An install in flight.
struct Job {
    /// Permille done.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
struct CursorStudio {
    /// Installed theme names (every icon folder), read once per visit.
    installed: Option<Rc<[String]>>,
    /// The ones Mandara installed (and may remove).
    ours: Option<Rc<[String]>>,
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
    /// A remove waiting for its confirming second press.
    confirm_remove: Option<&'static str>,
    /// Why X11 apps may not follow the last apply.
    note: Option<String>,
    /// Where the section was painted, and a pending deep-link scroll.
    anchor: Rc<Cell<Option<Bounds<Pixels>>>>,
    pending_scroll: Option<u8>,
}
impl Global for CursorStudio {}

fn st(cx: &mut gpui::App) -> &mut CursorStudio {
    cx.default_global::<CursorStudio>()
}

/// Drops what the page read from disk (on leaving Themes). Running
/// installs keep going.
pub(super) fn release(cx: &mut gpui::App) {
    let s = st(cx);
    s.installed = None;
    s.ours = None;
    s.previews.clear();
    s.loading.clear();
    s.confirm_remove = None;
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

/// A GitHub star count in words ("4.2k stars"), no star glyph.
fn stars(n: u32) -> String {
    let count = if n as f32 >= THOUSAND { format!("{:.1}k", n as f32 / THOUSAND) } else { n.to_string() };
    if n == 1 { t("1 star").to_string() } else { tf("{count} stars", &[("count", &count)]) }
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
    fn cursor_lists(cx: &mut Context<Self>) -> (Rc<[String]>, Rc<[String]>) {
        let env = install::Env::current();
        let mtime = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let stamp = (mtime(&env.manifest), mtime(&env.icons));
        let s = st(cx);
        if s.installed.is_none() || s.ours.is_none() || s.stamp != stamp {
            s.stamp = stamp;
            s.installed = Some(input::cursor_themes().into());
            s.ours = Some(install::installed(&env).into_iter().map(|i| i.theme).collect());
        }
        (s.installed.clone().unwrap_or_else(|| Rc::from([])), s.ours.clone().unwrap_or_else(|| Rc::from([])))
    }

    fn cursor_refresh(cx: &mut gpui::App) {
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
                let s = st(cx);
                s.loading.remove(&key);
                // One live size per theme besides the strip's: dragging the
                // size slider doesn't pile up every step.
                s.previews.retain(|(t, size), _| *t != key.0 || *size == STRIP_CURSOR_PX);
                s.previews.insert(key, Rc::from(images));
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
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let s = st(cx);
        s.errors.remove(name);
        s.jobs.insert(name, Job { progress: progress.clone(), cancel: cancel.clone() });
        let task = cx.background_executor().spawn(async move {
            let env = install::Env::current();
            install::install(theme.id, variant, &env, &|p| progress.store((p.fraction() * PERMILLE) as u32, Ordering::Relaxed), &cancel)
        });
        // Repaint while it runs so progress moves (stops with the job).
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(JOB_REPAINT).await;
            let running = this.update(cx, |_, cx| {
                cx.notify();
                st(cx).jobs.contains_key(name)
            });
            if !matches!(running, Ok(true)) {
                break;
            }
        })
        .detach();
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

    fn cursor_remove(&mut self, name: &'static str, cx: &mut Context<Self>) {
        if st(cx).confirm_remove != Some(name) {
            st(cx).confirm_remove = Some(name);
            cx.notify();
            return;
        }
        st(cx).confirm_remove = None;
        let task = cx.background_executor().spawn(async move { install::remove(name, &install::Env::current()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |_, cx| {
                match result {
                    Ok(_) => {
                        // A removed theme can't stay the cursor.
                        let active = mandara_ui::state::config(cx).read(cx).get().compositor.input.cursor.theme.clone();
                        if active.as_deref() == Some(name) {
                            apply_theme(None, cx);
                        }
                        st(cx).previews.retain(|(t, _), _| t != name);
                        Self::cursor_refresh(cx);
                    }
                    Err(e) => {
                        st(cx).errors.insert(name.to_string(), e);
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

    /// Opens Themes scrolled to the Cursor section (`mandara studio
    /// themes/cursor`, and the Input page's link).
    pub fn show_cursor_section(&mut self, cx: &mut Context<Self>) {
        self.show(Section::Themes, cx);
        st(cx).pending_scroll = Some(0);
        cx.notify();
    }

    fn cursor_scroll(&mut self, cx: &mut Context<Self>) {
        let Some(frames) = st(cx).pending_scroll else { return };
        let anchor = st(cx).anchor.get();
        let viewport = self.content_scroll.bounds();
        match anchor {
            Some(b) if frames >= ANCHOR_FRAMES && viewport.size.height > px(0.0) => {
                let offset = self.content_scroll.offset();
                let max = self.content_scroll.max_offset();
                let y = (offset.y - (b.top() - viewport.top()) + px(SCROLL_MARGIN_PX)).min(px(0.0)).max(-max.y);
                self.content_scroll.set_offset(point(offset.x, y));
                st(cx).pending_scroll = None;
                cx.notify();
            }
            _ => {
                st(cx).pending_scroll = Some(frames.saturating_add(1));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(ANCHOR_SETTLE).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
        }
    }

    /// The whole Cursor section of the Themes page.
    pub(super) fn cursor_section(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        // Cursor themes are an opt-in plugin: off, a one-line card offers it
        // and nothing here reads or writes the cursor.
        if !cfg.cursors_enabled() {
            return self.plugin_offer(mandara_ui::plugins::cursors::ID, p, cx);
        }
        self.cursor_scroll(cx);
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let current = cfg.compositor.input.cursor.theme.clone();
        let reset = chip("cursor-your-config", t("Your config"), current.is_none(), p)
            .tab_index(0)
            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| apply_theme(None, cx)))
            .into_any_element();
        let head = section_head(
            t("Cursor"),
            tf("{count} popular themes, downloaded on demand from their authors' GitHub releases, checked against a pinned checksum and installed to ~/.local/share/icons", &[("count", &CATALOG.len())]),
            vec![reset],
            display,
            p,
        );
        let yours = self.cursor_yours(cfg, p, cx);
        let gallery = self.cursor_gallery(cfg, p, cx);
        let others = self.cursor_others(cfg, p, cx);
        let anchor = st(cx).anchor.clone();
        div()
            .relative()
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(head)
            .child(yours)
            .child(gallery)
            .children(others)
            .child(canvas(move |bounds, _, _| anchor.set(Some(bounds)), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .into_any_element()
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
        let tiles = Role::ALL.iter().enumerate().map(|(i, role)| {
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
        });
        let mut rows_of_tiles = div().row().flex_col().gap(px(SPACE_8_PX));
        let all: Vec<AnyElement> = tiles.collect();
        let mut iter = all.into_iter();
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
            None => t("Saved now; applied the next time Mandara runs under MangoWM, Niri or Hyprland.").to_string(),
        };
        let note = st(cx).note.clone();
        let panel = div()
            .flex_1()
            .min_w(px(0.0))
            .rounded(px(RADIUS_22_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
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
        let fit = grid::fit(self.page.column_w, CURSOR_GRID);
        let cards: Vec<AnyElement> = CATALOG.iter().map(|theme| self.cursor_card(theme, active.as_deref(), &installed, &ours, p, cx)).collect();
        fit.render(cards)
    }

    fn cursor_card(&mut self, theme: &'static CursorTheme, active: Option<&str>, installed: &[String], ours: &[String], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
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
        let job = st(cx).jobs.get(variant.theme).map(|j| j.progress.load(Ordering::Relaxed));
        let error = st(cx).errors.get(variant.theme).cloned();
        let state = match (job, error.is_some(), is_installed(variant), Some(variant.theme) == active) {
            (Some(pm), ..) => CardState::Installing(pm),
            (None, true, false, _) => CardState::Failed,
            (None, _, true, true) => CardState::Active,
            (None, _, true, false) => CardState::Apply,
            (None, _, false, _) => CardState::Get,
        };
        let card_active = theme.variants.iter().any(|v| Some(v.theme) == active);
        let removable = ours.iter().any(|n| n == variant.theme) && is_installed(variant) && job.is_none();
        let arming = st(cx).confirm_remove == Some(variant.theme);
        let tint = swatch(variant);

        // The preview strip.
        let previews = is_installed(variant).then(|| Self::cursor_previews(variant.theme, STRIP_CURSOR_PX, cx)).flatten();
        let strip_bg = if is_installed(variant) { backdrop(p, Some(tint)) } else { backdrop(p, None) };
        let strip = match (is_installed(variant), previews) {
            (true, Some(images)) => div().row().items_center().justify_around().px(px(SPACE_8_PX)).children(Role::STRIP.iter().map(|role| {
                let i = Role::ALL.iter().position(|r| r == role).and_then(|i| images.get(i).cloned().flatten());
                div().id(SharedString::from(format!("cursor-{}-{role:?}", variant.theme))).size(px(DIM_40_PX)).row().items_center().justify_center().tooltip(tip(tr(role.label()), p)).children(i.map(|i| img(i.image).w(px(i.w)).h(px(i.h)).object_fit(gpui::ObjectFit::Fill)))
            })),
            (true, None) => div().row().items_center().justify_center().child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Loading preview…"))),
            (false, _) => div()
                .row()
                .items_center()
                .justify_center()
                .gap(px(SPACE_10_PX))
                .child(div().size(px(DIM_40_PX)).rounded_full().bg(tint.hsla()).border_2().border_color(tint.best_on(p.on_surface, p.surface_base).with_alpha(ALPHA_30).hsla()).row().items_center().justify_center().child(icon("pointer", ICON_17_PX, tint.best_on(Color::from_rgba8(u8::MAX, u8::MAX, u8::MAX, u8::MAX), Color::from_rgba8(0, 0, 0, u8::MAX)))))
                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Preview after download"))),
        };
        let strip = div().h(px(STRIP_H_PX)).rounded(px(RADIUS_16_PX)).bg(strip_bg).overflow_hidden().relative().child(strip.size_full()).when(state == CardState::Active, |el| {
            el.child(
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
                    .child(t("Active")),
            )
        });

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
            let name = variant.theme;
            div()
                .id(SharedString::from(format!("cursor-rm-{name}")))
                .h(px(DIM_34_PX))
                .px(px(if arming { SPACE_12_PX } else { SPACE_9_PX }))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .flex_none()
                .cursor_pointer()
                .bg(if arming { p.urgent_coral.hsla() } else { p.urgent_coral.with_alpha(ALPHA_14).hsla() })
                .text_color(if arming { p.on_primary.hsla() } else { p.urgent_coral.hsla() })
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .tooltip(tip(t("Remove (only what Mandara installed)"), p))
                .child(icon("trash", ICON_14_PX, if arming { p.on_primary } else { p.urgent_coral }))
                .when(arming, |el| el.child(t("Remove?")))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.cursor_remove(name, cx)))
        });
        let homepage = div()
            .id(SharedString::from(format!("cursor-home-{}", theme.id)))
            .size(px(DIM_34_PX))
            .rounded_full()
            .flex_none()
            .row()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla()))
            .tooltip(tip(theme.homepage, p))
            .child(icon("external", ICON_14_PX, p.on_surface_variant))
            .on_click(move |_: &ClickEvent, _, _| super::open_link(theme.homepage));
        let progress = job.map(|pm| {
            div().h(px(PROGRESS_H_PX)).w_full().rounded_full().bg(p.surface_container_high.hsla()).child(div().h_full().w(gpui::relative((pm.min(PERMILLE as u32) as f32 / PERMILLE).max(PROGRESS_MIN))).rounded_full().bg(mandara_core::oklab_gradient(HORIZONTAL_DEG, p.accent_gradient_start, p.accent_gradient_end)))
        });
        let error_row = error.map(|e| {
            div().row().items_start().gap(px(SPACE_6_PX)).child(icon("alert", ICON_13_PX, p.urgent_coral)).child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.urgent_coral.hsla()).child(mandara_ui::bidi::paragraph(e)))
        });
        let n = theme.variants.len();
        div()
            .id(SharedString::from(format!("cursor-card-{}", theme.id)))
            .flex_1()
            .tab_index(0)
            .p(px(SPACE_8_PX))
            .rounded(px(RADIUS_22_PX))
            .bg(p.surface_container.hsla())
            .border_2()
            .border_color(if card_active { p.primary.hsla() } else { gpui::transparent_black() })
            .hover(|s| s.border_color(p.primary.with_alpha(ALPHA_35).hsla()))
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
                            .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(stars(theme.stars))),
                    )
                    .child(div().row().items_center().gap(px(SPACE_6_PX)).child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(theme.author)).child(badge(theme.license, p.tertiary)))
                    .child(div().row().items_center().gap(px(SPACE_8_PX)).child(dots).child(div().flex_1().min_w(px(0.0)).truncate().text_start().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).child(tr(variant.label))))
                    .child(div().h(px(DIM_32_PX)).text_size(px(TEXT_11_5_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(2).child(mandara_ui::bidi::paragraph(tr(theme.description))))
                    .children(progress)
                    .children(error_row),
            )
            .child(div().px(px(SPACE_4_PX)).pb(px(SPACE_4_PX)).row().items_center().gap(px(SPACE_6_PX)).child(primary).child(div().flex_1()).children(remove).child(homepage))
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
                    "delete" | "backspace" if removable => this.cursor_remove(variant.theme, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    /// Installed themes outside the catalog (packages, your own folders).
    fn cursor_others(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (installed, _) = Self::cursor_lists(cx);
        let others: Vec<String> = installed.iter().filter(|n| cursors::catalog::by_theme_name(n).is_none()).cloned().collect();
        if others.is_empty() {
            return None;
        }
        let current = cfg.compositor.input.cursor.theme.clone();
        let chips = others.into_iter().map(|name| {
            let active = current.as_deref() == Some(name.as_str());
            let n = name.clone();
            chip(SharedString::from(format!("cursor-other-{name}")), name, active, p).tab_index(0).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| apply_theme(Some(n.clone()), cx))).into_any_element()
        });
        Some(
            div()
                .rounded(px(RADIUS_22_PX))
                .bg(p.surface_container.hsla())
                .p(px(SPACE_16_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Also installed")))
                .child(div().row().flex_wrap().gap(px(SPACE_6_PX)).children(chips))
                .into_any_element(),
        )
    }
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
    }

    #[test]
    fn swatches_and_stars_render() {
        let v = &CATALOG[0].variants[0];
        assert_eq!(swatch(v).to_rgba8()[3], u8::MAX);
        assert_eq!(stars(4233), "4.2k stars");
        assert_eq!(stars(186), "186 stars");
        assert_eq!(stars(1), "1 star");
        assert_eq!(catalog_name("Bibata-Modern-Ice").as_deref(), Some("Bibata Modern · Ice"));
        assert_eq!(catalog_name("Adwaita"), None);
    }

    #[test]
    fn every_cursor_setting_is_on_the_themes_page() {
        let rows: Vec<_> = registry::in_section(Section::Themes).filter(|s| s.group == CURSOR).map(|s| s.path).collect();
        for p in [CURSOR_THEME_PATH, crate::registry::system_settings::CURSOR_SIZE_PATH, "/compositor/input/cursor/hide_when_typing", "/compositor/input/cursor/hide_after_secs", "/compositor/input/cursor/no_hardware_cursors", "/compositor/input/cursor/hide_on_touch"] {
            assert!(rows.contains(&p), "{p}");
        }
        // Search finds them.
        assert!(registry::settings().iter().any(|s| s.path == CURSOR_THEME_PATH && s.matches("cursor theme")));
    }
}
