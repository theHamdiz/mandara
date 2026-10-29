//! System tray plugin (StatusNotifierItem + dbusmenu): compact item icons in
//! the bar — left-click activates (or opens the menu for menu-only items),
//! right-click opens the item's menu as a styled dropdown panel, middle-click
//! is the secondary action and the wheel scrolls the item. The panel drills
//! into submenus in place and shows check / radio state, icons, shortcuts
//! and disabled entries; opened from the chip itself it lists every item
//! (passive ones included).
//!
//! The watcher / host actor starts the first time the plugin is placed.
//! `[plugins.settings.tray] show_passive = true` keeps passive items in the
//! bar.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, Global, ImageSource, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, ObjectFit, ParentElement, RenderImage,
    ScrollDelta, ScrollEvent, StatefulInteractiveElement as _, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::tray::{ItemStatus, MenuNode, Pixmap, ToggleKind, TrayCommand, TrayItem, TrayService, TrayState};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::panel_fx::{ANGLE_DIAGONAL_DEG, HEADER_CHIP_PX, HEADER_H, PanelShape, SMALL_ICON_PX, empty_state, glass_card, partner, pill_button, readable};
use super::{FnView, json};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};
use crate::state::{config, shell};

pub const ID: &str = "tray";

// ── Layout ───────────────────────────────────────────────────────────────

const ICON: f32 = 16.0;
const CELL: f32 = 22.0;
const PANEL_W: f32 = 312.0;
const PAD: f32 = 12.0;
const GAP: f32 = 10.0;
const ROW_H: f32 = 34.0;
const SEP_H: f32 = 11.0;
const ITEM_ROW_H: f32 = 46.0;
const MAX_LIST_H: f32 = 600.0;
/// Panel height before the first render sizes it.
const INITIAL_PANEL_H: f32 = 200.0;
/// Height of the empty states.
const EMPTY_H: f32 = 150.0;
/// The "Ask the app for its menu" button under the no-menu state.
const ASK_BUTTON_H: f32 = 36.0;
/// Padding inside the menu card (and the matching height allowance).
const MENU_CARD_PAD: f32 = tokens::XS;
/// Bar: gap between cells, cell radius, hover / open alphas, attention dot
/// (inset, side).
const BAR_GAP_PX: f32 = 3.0;
const CELL_RADIUS_PX: f32 = 7.0;
const CELL_HOVER_ALPHA: f32 = 0.10;
const CELL_OPEN_ALPHA: f32 = 0.18;
const ATTENTION_INSET_PX: f32 = 1.0;
const ATTENTION_DOT_PX: f32 = 6.0;
/// Menu rows: padding, radius, gap, hover alpha `(dark, light)`, disabled
/// opacity, lead-slot side, chevron size.
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_RADIUS_PX: f32 = 10.0;
const ROW_GAP_PX: f32 = 10.0;
const ROW_HOVER_ALPHA: (f32, f32) = (0.07, 0.06);
const ROW_DISABLED_OPACITY: f32 = 0.42;
const LEAD_PX: f32 = 16.0;
const CHEVRON_PX: f32 = 14.0;
/// Toggle marks: ring alpha, radio dot, check box radius, indeterminate fill.
const MARK_RING_ALPHA: f32 = 0.55;
const RADIO_DOT_PX: f32 = 7.0;
const CHECK_RADIUS_PX: f32 = 5.0;
const MIXED_ALPHA: f32 = 0.35;
/// Separator hairline alpha `(dark, light)`.
const SEP_ALPHA: (f32, f32) = (0.08, 0.09);
/// Back row: inset and gap, hover alpha.
const BACK_PAD_X_PX: f32 = tokens::SM;
const SOFT_HOVER_ALPHA: f32 = 0.06;
/// Item header: icon chip fill alphas and glyph size.
const ITEM_CHIP_FILL: (f32, f32) = (0.20, 0.10);
const ITEM_CHIP_RADIUS_PX: f32 = 12.0;
const ITEM_CHIP_ICON_PX: f32 = 20.0;
/// Overview rows: radius, gap, icon tile (side, radius, alpha), glyph.
const OVERVIEW_RADIUS_PX: f32 = 12.0;
const OVERVIEW_GAP_PX: f32 = 12.0;
const OVERVIEW_TILE_PX: f32 = 30.0;
const OVERVIEW_TILE_RADIUS_PX: f32 = 10.0;
const OVERVIEW_TILE_ALPHA: (f32, f32) = (0.08, 0.06);
const OVERVIEW_ICON_PX: f32 = 18.0;
/// Panel size changes smaller than this are ignored.
const SIZE_EPSILON_PX: f32 = 0.5;

// ── Behaviour ────────────────────────────────────────────────────────────

/// How long after the panel closed the open menu is released (it keeps
/// showing while the panel morphs away).
const MENU_RELEASE_DELAY: Duration = Duration::from_millis(600);
/// Pixels of smooth scrolling per tray scroll step.
const SCROLL_PX_PER_STEP: f32 = 24.0;
/// Smaller wheel deltas still count as one step.
const MIN_SCROLL_STEP: f32 = 0.5;
/// Rendered tray icons kept (animated icons churn pixmaps).
const IMAGE_CACHE_CAP: usize = 96;
/// Icon-theme-path lookups kept.
const THEME_PATH_CACHE_CAP: usize = 256;
/// Directory depth searched inside an item's private icon theme path.
const THEME_PATH_DEPTH: u32 = 4;
/// Score of a scalable icon (beats any raster size).
const SVG_SCORE: u32 = 10_000;
/// Pixels with at least this alpha decide whether an icon is monochrome.
const MONO_ALPHA_MIN: u8 = 48;
/// Mean per-pixel chroma (max − min channel) below this is monochrome.
const MONO_CHROMA_MAX: u64 = 22;
/// Rec. 709 luma weights.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// Recolour coverage ramp: luminance below the floor is transparent and
/// the ramp spans this width.
const COVERAGE_FLOOR: f32 = 0.12;
const COVERAGE_SPAN: f32 = 0.76;
/// Light-on-transparent glyphs average at least this luminance.
const LIGHT_GLYPH_LUM: f32 = 0.5;
const CHANNEL_MAX: f32 = 255.0;
/// RGBA / BGRA bytes per pixel and the alpha byte's index.
const BYTES_PER_PIXEL: usize = 4;
const ALPHA: usize = 3;

// ── Model ────────────────────────────────────────────────────────────────

/// Tray state shared by every bar surface plus the panel's navigation.
pub struct TrayModel {
    pub state: Rc<TrayState>,
    service: Option<TrayService>,
    /// Item whose menu the panel shows (`None` → the item overview).
    selected: Option<String>,
    /// Submenu drill-down path (menu node ids).
    stack: Vec<i32>,
    /// The panel closed since the last right-click: reopening it from the
    /// chip shows the overview.
    stale: bool,
}

#[derive(Clone)]
struct GlobalTray(Entity<TrayModel>);
impl Global for GlobalTray {}

/// The tray model, starting the actor on first use.
pub fn model(cx: &mut App) -> Entity<TrayModel> {
    if let Some(g) = cx.try_global::<GlobalTray>() {
        return g.0.clone();
    }
    let service = mandara_services::tray::spawn_tray_service();
    let entity = cx.new(|_| TrayModel { state: Rc::default(), service: Some(service.clone()), selected: None, stack: Vec::new(), stale: true });
    cx.set_global(GlobalTray(entity.clone()));
    let updates = service.updates.clone();
    let weak = entity.downgrade();
    cx.spawn(async move |cx| {
        while let Ok(state) = updates.recv().await {
            // Resolve icons from items' private theme paths (a directory
            // walk) off the UI thread before the state reaches a render.
            let missing = missing_theme_icons(&state);
            if !missing.is_empty() {
                let found = cx
                    .background_executor()
                    .spawn(async move { missing.into_iter().map(|(root, name)| { let path = walk_theme_path(&root, &name); ((root, name), path) }).collect::<Vec<_>>() })
                    .await;
                THEME_PATHS.with(|m| {
                    let mut m = m.borrow_mut();
                    if m.len() + found.len() > THEME_PATH_CACHE_CAP {
                        m.clear();
                    }
                    m.extend(found);
                });
            }
            let Some(entity) = weak.upgrade() else { break };
            entity.update(cx, |m, cx| {
                if *m.state == state {
                    return;
                }
                // The open item vanished: fall back to the overview.
                if m.selected.as_ref().is_some_and(|k| !state.items.iter().any(|i| &i.key == k)) {
                    m.selected = None;
                    m.stack.clear();
                }
                m.state = Rc::new(state);
                prune_item_images(&m.state);
                cx.notify();
            });
        }
    })
    .detach();
    entity
}

/// Stops the tray host because the plugin was turned off: the actor ends
/// and releases the StatusNotifierWatcher name. The next [`model`] call
/// (the plugin back on) starts a fresh one.
pub fn stop(cx: &mut App) {
    if let Some(GlobalTray(entity)) = cx.try_global::<GlobalTray>().cloned() {
        entity.update(cx, |m, _| m.service = None);
        cx.remove_global::<GlobalTray>();
    }
}

fn service(cx: &mut App) -> Option<TrayService> {
    model(cx).read(cx).service.clone()
}

fn send(cx: &mut App, command: TrayCommand) {
    if let Some(s) = service(cx) {
        s.send(command);
    }
}

fn request(cx: &mut App, command: TrayCommand) -> Result<Output, String> {
    let s = service(cx).ok_or_else(|| t("The tray service is not running").to_string())?;
    Ok(Output::Pending(s.request(command)))
}

fn show_passive(cx: &App) -> bool {
    config(cx).read(cx).get().plugins.setting(ID, "show_passive", false)
}

fn panel_open(cx: &App) -> bool {
    shell(cx).read(cx).panel.as_deref() == Some(ID)
}

/// Shows `key`'s menu in the panel (toggles it closed when it already is).
pub fn open_menu(key: &str, cx: &mut App) {
    let m = model(cx);
    let (same, stale) = { let r = m.read(cx); (r.selected.as_deref() == Some(key), r.stale) };
    if panel_open(cx) && same && !stale {
        close_panel(cx);
        return;
    }
    m.update(cx, |m, cx| {
        m.selected = Some(key.to_string());
        m.stack.clear();
        m.stale = false;
        cx.notify();
    });
    send(cx, TrayCommand::OpenMenu(key.to_string()));
    shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
}

/// Shows the item overview in the panel.
pub fn open_overview(cx: &mut App) {
    model(cx).update(cx, |m, cx| {
        m.selected = None;
        m.stack.clear();
        m.stale = false;
        cx.notify();
    });
    shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
}

fn close_panel(cx: &mut App) {
    shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
}

/// Left-click: `Activate`, or the menu for menu-only items (and for items
/// that turn out not to implement `Activate`).
fn primary(item: &TrayItem, x: f32, y: f32, cx: &mut App) {
    if item.item_is_menu && item.menu.is_some() {
        open_menu(&item.key, cx);
        return;
    }
    let Some(s) = service(cx) else { return };
    let rx = s.request(TrayCommand::Activate { item: item.key.clone(), x: x as i32, y: y as i32 });
    let (key, has_menu) = (item.key.clone(), item.menu.is_some());
    cx.spawn(async move |cx| {
        if let Ok(Err(err)) = rx.recv().await {
            tracing::debug!("tray: activate failed ({err}); showing the menu instead");
            if has_menu {
                let _ = cx.update(|cx| open_menu(&key, cx));
            }
        }
    })
    .detach();
}

// ── Icons ────────────────────────────────────────────────────────────────

/// Which image cache a pixmap belongs to: bar / overview icons live as long
/// as their items, menu icons only while a menu is open.
#[derive(Copy, Clone, PartialEq)]
enum IconCache {
    Bar,
    Menu,
}

type ImageCache = HashMap<(u64, u32), Arc<RenderImage>>;

thread_local! {
    /// `(pixmap hash, requested tint)` → GPU image (decoded once, reused by
    /// every frame) for item icons.
    static IMAGES: RefCell<ImageCache> = RefCell::new(HashMap::new());
    /// The same for menu entry icons; emptied when the menu is released.
    static MENU_IMAGES: RefCell<ImageCache> = RefCell::new(HashMap::new());
    /// Images evicted from a full cache, whose GPU textures are released
    /// on the next bar render.
    static EVICTED: RefCell<Vec<Arc<RenderImage>>> = const { RefCell::new(Vec::new()) };
    /// `(theme path, icon name)` → file, filled off the UI thread.
    static THEME_PATHS: RefCell<HashMap<(String, String), Option<PathBuf>>> = RefCell::new(HashMap::new());
}

/// Whether an icon is effectively monochrome (a symbolic-style glyph).
fn is_monochrome(p: &Pixmap) -> bool {
    let (mut n, mut chroma) = (0u64, 0u64);
    for px in p.rgba.chunks_exact(BYTES_PER_PIXEL).filter(|px| px[ALPHA] > MONO_ALPHA_MIN) {
        let (mx, mn) = (px[0].max(px[1]).max(px[2]), px[0].min(px[1]).min(px[2]));
        chroma += u64::from(mx - mn);
        n += 1;
    }
    n > 0 && chroma / n < MONO_CHROMA_MAX
}

/// The pixmap as a GPU image. Monochrome glyphs are recoloured with `tint`
/// (their alpha is the shape) so white-on-transparent icons stay legible on
/// light themes. Cached by the *requested* tint, so a hit costs one hash
/// lookup — the per-pixel monochrome scan only runs when decoding.
fn render_image(p: &Pixmap, tint: Option<Color>, cache: IconCache) -> Arc<RenderImage> {
    let store = if cache == IconCache::Bar { &IMAGES } else { &MENU_IMAGES };
    let key = (p.hash, tint.map_or(0, |c| c.to_rgba_u32()));
    if let Some(hit) = store.with(|m| m.borrow().get(&key).cloned()) {
        return hit;
    }
    let tint = tint.filter(|_| is_monochrome(p));
    let mut bgra = p.rgba.to_vec();
    let rgb = tint.map(|c| c.to_rgba8());
    let lum = |px: &[u8]| (LUMA[0] * f32::from(px[0]) + LUMA[1] * f32::from(px[1]) + LUMA[2] * f32::from(px[2])) / CHANNEL_MAX;
    // Glyph polarity: light-on-transparent (most tray glyphs) keeps its
    // light parts; dark tiles with a light logo keep the tile and cut the
    // logo out, so inner detail survives the recolour.
    let light = {
        let (mut sum, mut weight) = (0.0f32, 0.0f32);
        for px in bgra.chunks_exact(BYTES_PER_PIXEL) {
            let a = f32::from(px[ALPHA]) / CHANNEL_MAX;
            sum += a * lum(px);
            weight += a;
        }
        weight == 0.0 || sum / weight >= LIGHT_GLYPH_LUM
    };
    for px in bgra.chunks_exact_mut(BYTES_PER_PIXEL) {
        match rgb {
            Some([r, g, b, _]) => {
                let l = lum(px);
                let coverage = (((if light { l } else { 1.0 - l }) - COVERAGE_FLOOR) / COVERAGE_SPAN).clamp(0.0, 1.0);
                px[ALPHA] = (f32::from(px[ALPHA]) * coverage).round().clamp(0.0, CHANNEL_MAX) as u8;
                px[0] = b;
                px[1] = g;
                px[2] = r;
            }
            None => px.swap(0, 2),
        }
    }
    let image = image::RgbaImage::from_raw(p.width, p.height, bgra).unwrap_or_else(|| image::RgbaImage::new(1, 1));
    let render = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(image)]));
    store.with(|m| {
        let mut m = m.borrow_mut();
        // Animated icons churn pixmaps; keep the cache bounded.
        if m.len() >= IMAGE_CACHE_CAP {
            EVICTED.with(|e| e.borrow_mut().extend(m.drain().map(|(_, img)| img)));
        }
        m.insert(key, render.clone());
    });
    render
}

/// Drops cached icons of items (or icon frames) that are gone; their GPU
/// textures are released on the next bar render.
fn prune_item_images(state: &TrayState) {
    let live: Vec<u64> = state.items.iter().flat_map(|i| [i.icon.as_ref(), i.attention_icon.as_ref()]).flatten().map(|p| p.hash).collect();
    IMAGES.with(|m| {
        let mut m = m.borrow_mut();
        let gone: Vec<_> = m.keys().filter(|(hash, _)| !live.contains(hash)).copied().collect();
        if gone.is_empty() {
            return;
        }
        EVICTED.with(|e| e.borrow_mut().extend(gone.iter().filter_map(|k| m.remove(k))));
    });
}

/// Releases the GPU textures of images evicted from a full cache.
fn drop_evicted(window: &mut gpui::Window, cx: &mut App) {
    let evicted = EVICTED.with(|e| std::mem::take(&mut *e.borrow_mut()));
    for img in evicted {
        cx.drop_image(img, Some(window));
    }
}

/// Frees the decoded menu icons (memory and GPU textures) once the menu
/// is released.
fn release_menu_images(cx: &mut App) {
    let images = MENU_IMAGES.with(|m| std::mem::take(&mut *m.borrow_mut()));
    for (_, img) in images {
        cx.drop_image(img, None);
    }
}

/// Looks `name` up in an item's private `IconThemePath` (flat or themed)
/// among the lookups already done off the UI thread; never touches the
/// file system (renders must not block).
fn theme_path_icon(root: &str, name: &str) -> Option<PathBuf> {
    if root.is_empty() || name.is_empty() {
        return None;
    }
    THEME_PATHS.with(|m| m.borrow().get(&(root.to_string(), name.to_string())).cloned().flatten())
}

/// `(theme path, icon name)` pairs of `state` not looked up yet.
fn missing_theme_icons(state: &TrayState) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    THEME_PATHS.with(|m| {
        let m = m.borrow();
        for item in &state.items {
            for name in [&item.icon_name, &item.attention_icon_name] {
                let key = (item.icon_theme_path.clone(), name.clone());
                if !key.0.is_empty() && !key.1.is_empty() && !m.contains_key(&key) && !out.contains(&key) {
                    out.push(key);
                }
            }
        }
    });
    out
}

/// Searches `root` for the best file named `name` (blocking).
fn walk_theme_path(root: &str, name: &str) -> Option<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: u32, best: &mut Option<(u32, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < THEME_PATH_DEPTH {
                    walk(&p, name, depth + 1, best);
                }
            } else if p.file_stem().is_some_and(|s| s == name) {
                // Prefer scalable, then the largest raster size in the path.
                let score = if p.extension().is_some_and(|x| x == "svg") {
                    SVG_SCORE
                } else {
                    p.components().filter_map(|c| c.as_os_str().to_str()?.split('x').next()?.parse::<u32>().ok()).max().unwrap_or(1)
                };
                if best.as_ref().is_none_or(|(s, _)| score > *s) {
                    *best = Some((score, p));
                }
            }
        }
    }
    let mut best = None;
    walk(Path::new(root), name, 0, &mut best);
    best.map(|(_, p)| p)
}

fn file_icon(path: PathBuf, side: f32, tint: Color) -> AnyElement {
    if path.to_string_lossy().contains("-symbolic") {
        gpui::svg().external_path(path.to_string_lossy().to_string()).size(px(side)).flex_none().text_color(tint.hsla()).into_any_element()
    } else {
        div().size(px(side)).flex_none().child(img(path).size_full().object_fit(ObjectFit::Contain)).into_any_element()
    }
}

fn named_icon(name: &str, theme_path: &str, side: f32, tint: Color, monochrome: bool) -> Option<AnyElement> {
    if name.is_empty() {
        return None;
    }
    if let Some(path) = theme_path_icon(theme_path, name) {
        return Some(file_icon(path, side, tint));
    }
    crate::icon_theme::resolve(name, None, monochrome).map(|r| {
        if r.symbolic {
            gpui::svg().external_path(r.path.to_string_lossy().to_string()).size(px(side)).flex_none().text_color(tint.hsla()).into_any_element()
        } else {
            file_icon(r.path, side, tint)
        }
    })
}

fn pixmap_icon(p: &Pixmap, side: f32, tint: Option<Color>, cache: IconCache) -> AnyElement {
    div().size(px(side)).flex_none().child(img(ImageSource::Render(render_image(p, tint, cache))).size_full().object_fit(ObjectFit::Contain)).into_any_element()
}

/// An item's current icon (attention icon while it needs attention), from
/// its theme path, the icon theme, its pixmap, or a monogram tile.
pub fn item_icon(item: &TrayItem, side: f32, tint: Color, monochrome: bool, palette: &MandaraPalette) -> AnyElement {
    let attention = item.status == ItemStatus::NeedsAttention && (!item.attention_icon_name.is_empty() || item.attention_icon.is_some());
    let (name, pixmap) = if attention { (&item.attention_icon_name, item.attention_icon.as_ref()) } else { (&item.icon_name, item.icon.as_ref()) };
    if let Some(el) = named_icon(name, &item.icon_theme_path, side, tint, monochrome) {
        return el;
    }
    if let Some(p) = pixmap {
        return pixmap_icon(p, side, Some(tint), IconCache::Bar);
    }
    super::workspaces::monogram(&item.display_name(), side, palette)
}

// ── Bar ──────────────────────────────────────────────────────────────────

fn scroll_steps(e: &ScrollEvent) -> (i32, bool) {
    let (dx, dy) = match e.delta {
        ScrollDelta::Lines(p) => (p.x, p.y),
        ScrollDelta::Pixels(p) => (f32::from(p.x) / SCROLL_PX_PER_STEP, f32::from(p.y) / SCROLL_PX_PER_STEP),
    };
    let horizontal = dx.abs() > dy.abs();
    let v = if horizontal { dx } else { dy };
    // GPUI's positive y scrolls content up (wheel away); SNI's positive
    // delta means down.
    let steps = if v.abs() < MIN_SCROLL_STEP { v.signum() } else { v.round() };
    ((-steps) as i32, horizontal)
}

fn bar_view(palette: &MandaraPalette, window: &mut gpui::Window, cx: &mut App) -> AnyElement {
    drop_evicted(window, cx);
    let m = model(cx);
    // Remember that the panel closed so the chip reopens the overview.
    if !panel_open(cx) && !m.read(cx).stale {
        m.update(cx, |m, _| m.stale = true);
        // Release the menu once the panel has morphed away (it keeps
        // showing it while closing).
        cx.spawn(async move |cx| {
            cx.background_executor().timer(MENU_RELEASE_DELAY).await;
            let _ = cx.update(|cx| {
                let m = model(cx);
                if m.read(cx).stale && !panel_open(cx) {
                    // The service drops the menu tree and publishes the
                    // state without it; free the menu's icons and path too.
                    send(cx, TrayCommand::CloseMenu);
                    release_menu_images(cx);
                    m.update(cx, |m, _| m.stack = Vec::new());
                }
            });
        })
        .detach();
    }
    let passive = show_passive(cx);
    let monochrome = config(cx).read(cx).get().appearance.bar_icons == mandara_config::IconStyle::Monochrome;
    // Shared state: no per-frame clone of the items (or their pixmaps).
    let state = m.read(cx).state.clone();
    let mut items = state.items.iter().filter(|i| passive || i.status != ItemStatus::Passive).peekable();
    if items.peek().is_none() {
        return div().into_any_element();
    }
    let selected = if panel_open(cx) && !m.read(cx).stale { m.read(cx).selected.clone() } else { None };
    let hover = palette.on_surface.with_alpha(CELL_HOVER_ALPHA);
    let active_bg = palette.primary.with_alpha(CELL_OPEN_ALPHA);
    div()
        .flex()
        // A side bar stacks the icons.
        .when(crate::bar::vertical(cx), |el| el.flex_col())
        .items_center()
        .gap(px(BAR_GAP_PX))
        .children(items.enumerate().map(|(i, item)| {
            let is_open = selected.as_deref() == Some(item.key.as_str());
            let attention = item.status == ItemStatus::NeedsAttention;
            let key: Rc<str> = item.key.as_str().into();
            let (k_left, k_right, k_mid, k_scroll) = (key.clone(), key.clone(), key.clone(), key);
            div()
                .id(("tray-item", i))
                .relative()
                .size(px(CELL))
                .flex_none()
                .rounded(px(crate::style::radius(CELL_RADIUS_PX)))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(is_open, |el| el.bg(active_bg.hsla()))
                .hover(move |s| s.bg(hover.hsla()))
                .child(item_icon(item, crate::style::bar_icon(ICON), palette.on_surface, monochrome, palette))
                .when(attention, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(ATTENTION_INSET_PX))
                            .right(px(ATTENTION_INSET_PX))
                            .size(px(ATTENTION_DOT_PX))
                            .rounded_full()
                            .bg(palette.urgent_coral.hsla())
                            .border_1()
                            .border_color(palette.surface_container.hsla()),
                    )
                })
                .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let item = model(cx).read(cx).state.items.iter().find(|i| *i.key == *k_left).cloned();
                    if let Some(item) = item {
                        primary(&item, f32::from(e.position.x), f32::from(e.position.y), cx);
                    }
                })
                .on_mouse_down(MouseButton::Right, move |_: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    open_menu(&k_right, cx);
                })
                .on_mouse_down(MouseButton::Middle, move |e: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    send(cx, TrayCommand::SecondaryActivate { item: k_mid.to_string(), x: f32::from(e.position.x) as i32, y: f32::from(e.position.y) as i32 });
                })
                .on_scroll(move |e: &ScrollEvent, _, cx| {
                    let (delta, horizontal) = scroll_steps(e);
                    if delta != 0 {
                        send(cx, TrayCommand::Scroll { item: k_scroll.to_string(), delta, horizontal });
                    }
                })
        }))
        .into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

/// Check box / radio indicator for toggle entries.
fn toggle_mark(kind: ToggleKind, on: Option<bool>, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let ring = palette.on_surface_variant.with_alpha(MARK_RING_ALPHA);
    let fill = oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary, partner(palette.primary).with_lightness(palette.primary.lightness()));
    let base = div().size(px(LEAD_PX)).flex_none().flex().items_center().justify_center();
    match (kind, on) {
        (ToggleKind::Radio, Some(true)) => base.rounded_full().border_2().border_color(accent.hsla()).child(div().size(px(RADIO_DOT_PX)).rounded_full().bg(accent.hsla())).into_any_element(),
        (ToggleKind::Radio, _) => base.rounded_full().border_1().border_color(ring.hsla()).into_any_element(),
        (_, Some(true)) => base.rounded(px(crate::style::radius(CHECK_RADIUS_PX))).bg(fill).child(icon("check", SMALL_ICON_PX, palette.on_primary)).into_any_element(),
        (_, None) => base.rounded(px(crate::style::radius(CHECK_RADIUS_PX))).bg(accent.with_alpha(MIXED_ALPHA).hsla()).child(icon("minus", SMALL_ICON_PX, palette.on_primary)).into_any_element(),
        (_, Some(false)) => base.rounded(px(crate::style::radius(CHECK_RADIUS_PX))).border_1().border_color(ring.hsla()).into_any_element(),
    }
}

fn menu_row(node: &MenuNode, lead_slot: bool, monochrome: bool, palette: &MandaraPalette) -> AnyElement {
    let hover = palette.on_surface.with_alpha(super::panel_fx::alpha(ROW_HOVER_ALPHA, palette));
    let enabled = node.enabled;
    let lead: Option<AnyElement> = if node.toggle != ToggleKind::None {
        Some(toggle_mark(node.toggle, node.toggled, palette))
    } else if let Some(p) = &node.icon {
        Some(pixmap_icon(p, LEAD_PX, Some(palette.on_surface), IconCache::Menu))
    } else {
        named_icon(&node.icon_name, "", LEAD_PX, palette.on_surface_variant, monochrome)
    };
    let (id, submenu) = (node.id, node.submenu);
    div()
        .id(("tray-row", usize::try_from(node.id).unwrap_or(0)))
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .when(enabled, |el| el.cursor_pointer().hover(move |s| s.bg(hover.hsla())))
        .when(!enabled, |el| el.opacity(ROW_DISABLED_OPACITY))
        .when(lead_slot, |el| el.child(div().size(px(LEAD_PX)).flex_none().flex().items_center().justify_center().when_some(lead, |el, l| el.child(l))))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY)).child(node.label.clone()))
        .when(!node.shortcut.is_empty(), |el| el.child(div().flex_none().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(node.shortcut.clone())))
        .when(submenu, |el| el.child(icon("chevron-right", CHEVRON_PX, palette.on_surface_variant)))
        .when(enabled, |el| {
            el.on_click(move |_, _, cx| {
                if submenu {
                    send(cx, TrayCommand::MenuAboutToShow { id });
                    model(cx).update(cx, |m, cx| {
                        m.stack.push(id);
                        cx.notify();
                    });
                } else {
                    send(cx, TrayCommand::MenuClick { id });
                    close_panel(cx);
                }
            })
        })
        .into_any_element()
}

fn separator(palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(SEP_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .flex()
        .items_center()
        .child(div().h(px(1.0)).w_full().bg(palette.on_surface.with_alpha(super::panel_fx::alpha(SEP_ALPHA, palette)).hsla()))
        .into_any_element()
}

fn back_row(label: String, palette: &MandaraPalette) -> AnyElement {
    let hover = palette.on_surface.with_alpha(SOFT_HOVER_ALPHA);
    let accent = readable(palette.primary, palette);
    div()
        .id("tray-back")
        .h(px(ROW_H))
        .flex_none()
        .px(px(BACK_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(BACK_PAD_X_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(gpui::svg().path("icons/chevron-right.svg").size(px(CHEVRON_PX)).flex_none().text_color(accent.hsla()).with_transformation(gpui::Transformation::rotate(gpui::radians(std::f32::consts::PI))))
        .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).truncate().child(label))
        .on_click(|_, _, cx| {
            model(cx).update(cx, |m, cx| {
                m.stack.pop();
                cx.notify();
            })
        })
        .into_any_element()
}

/// The visible entries of a menu level with separators tidied (no leading,
/// trailing or doubled separators). Returns rows and their total height.
fn level_rows(node: &MenuNode, monochrome: bool, palette: &MandaraPalette) -> (Vec<AnyElement>, f32) {
    let visible: Vec<&MenuNode> = node.children.iter().filter(|c| c.visible).collect();
    let lead_slot = visible.iter().any(|c| !c.separator && (c.toggle != ToggleKind::None || c.icon.is_some() || !c.icon_name.is_empty()));
    let mut rows = Vec::new();
    let mut h = 0.0;
    let mut pending_sep = false;
    for c in visible {
        if c.separator {
            pending_sep = !rows.is_empty();
            continue;
        }
        if pending_sep {
            rows.push(separator(palette));
            h += SEP_H;
            pending_sep = false;
        }
        rows.push(menu_row(c, lead_slot, monochrome, palette));
        h += ROW_H;
    }
    (rows, h)
}

/// The tooltip text worth showing under the item's name (not a repeat of it).
fn tooltip_line(item: &TrayItem) -> Option<String> {
    let name = item.display_name();
    let tip = item.tooltip.as_ref()?;
    [tip.title.as_str(), tip.body.as_str()].into_iter().map(str::trim).find(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&name)).map(|s| s.lines().next().unwrap_or(s).to_string())
}

fn item_header(item: &TrayItem, monochrome: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let subtitle = tooltip_line(item).unwrap_or_else(|| match item.status {
            ItemStatus::NeedsAttention => t("Needs attention").to_string(),
            ItemStatus::Passive => t("Idle").to_string(),
            ItemStatus::Active => t("Running").to_string(),
        });
    let chip = div()
        .size(px(HEADER_CHIP_PX))
        .flex_none()
        .rounded(px(crate::style::radius(ITEM_CHIP_RADIUS_PX)))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(ITEM_CHIP_FILL.0), partner(accent).with_alpha(ITEM_CHIP_FILL.1)))
        .flex()
        .items_center()
        .justify_center()
        .child(item_icon(item, ITEM_CHIP_ICON_PX, accent, monochrome, palette));
    let key = item.key.clone();
    let can_activate = !item.item_is_menu;
    div()
        .h(px(HEADER_H))
        .flex()
        .items_center()
        .gap(px(tokens::MD))
        .child(chip)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).truncate().child(item.display_name()))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(subtitle)),
        )
        .when(can_activate, |el| {
            el.child(pill_button("tray-open", None, t("Open"), accent, palette, move |_, cx| {
                send(cx, TrayCommand::Activate { item: key.clone(), x: 0, y: 0 });
                close_panel(cx);
            }))
        })
        .into_any_element()
}

fn overview_row(i: usize, item: &TrayItem, monochrome: bool, palette: &MandaraPalette) -> AnyElement {
    let hover = palette.on_surface.with_alpha(SOFT_HOVER_ALPHA);
    let key = item.key.clone();
    let detail = match item.status {
        ItemStatus::NeedsAttention => t("Needs attention").to_string(),
        ItemStatus::Passive => t("Idle · hidden from the bar").to_string(),
        ItemStatus::Active => tooltip_line(item).unwrap_or_else(|| t("Running").to_string()),
    };
    div()
        .id(("tray-overview", i))
        .h(px(ITEM_ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(OVERVIEW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(OVERVIEW_GAP_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(
            div()
                .size(px(OVERVIEW_TILE_PX))
                .flex_none()
                .rounded(px(crate::style::radius(OVERVIEW_TILE_RADIUS_PX)))
                .bg(palette.on_surface.with_alpha(super::panel_fx::alpha(OVERVIEW_TILE_ALPHA, palette)).hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(item_icon(item, OVERVIEW_ICON_PX, palette.on_surface, monochrome, palette)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(item.display_name()))
                .child(div().text_size(px(text::LABEL)).text_color(if item.status == ItemStatus::NeedsAttention { palette.urgent_coral.hsla() } else { palette.on_surface_variant.hsla() }).truncate().child(detail)),
        )
        .when(item.menu.is_some(), |el| el.child(icon("chevron-right", CHEVRON_PX, palette.on_surface_variant)))
        .on_click(move |_, _, cx| {
            let item = model(cx).read(cx).state.items.iter().find(|i| i.key == key).cloned();
            if let Some(item) = item {
                if item.menu.is_some() {
                    open_menu(&item.key, cx);
                } else {
                    close_panel(cx);
                    primary(&item, 0.0, 0.0, cx);
                }
            }
        })
        .into_any_element()
}

fn panel_view(p: PanelCx, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>) -> impl Fn(&MandaraPalette, &mut gpui::Window, &mut App) -> AnyElement {
    move |palette, _, cx| {
        let m = model(cx);
        let monochrome = config(cx).read(cx).get().appearance.bar_icons == mandara_config::IconStyle::Monochrome;
        let shape = PanelShape::of(cx);
        let accent = readable(palette.primary, palette);
        let (state, selected, stack) = {
            let r = m.read(cx);
            // While closed the content keeps its last look (close morph).
            let selected = if r.stale && panel_open(cx) { None } else { r.selected.clone() };
            (r.state.clone(), selected, r.stack.clone())
        };
        let item = selected.as_ref().and_then(|k| state.items.iter().find(|i| &i.key == k));
        let mut h = PAD * 2.0 + HEADER_H + GAP;
        let mut col = div().size_full().p(px(PAD)).flex().flex_col();

        match item {
            None => {
                let n = state.items.len();
                let subtitle = match n {
                    0 => t("No apps in the tray").to_string(),
                    1 => t("1 app").to_string(),
                    n => tf("{n} apps", &[("n", &n)]),
                };
                col = col.child(reveal(&p, 0, super::panel_fx::header("grid", accent, t("System tray"), subtitle, palette, None)));
                if n == 0 {
                    h += EMPTY_H;
                    col = col.child(div().h(px(GAP))).child(reveal(&p, 1, empty_state("grid", t("Nothing here yet"), t("Apps that support the system tray appear here"), EMPTY_H, palette)));
                } else {
                    let list_h = (n as f32 * ITEM_ROW_H).min(MAX_LIST_H);
                    h += list_h;
                    col = col.child(div().h(px(GAP))).child(reveal(
                        &p,
                        1,
                        div().id("tray-overview-list").h(px(list_h)).overflow_y_scroll().flex().flex_col().children(state.items.iter().enumerate().map(|(i, it)| overview_row(i, it, monochrome, palette))),
                    ));
                }
            }
            Some(item) => {
                col = col.child(reveal(&p, 0, item_header(item, monochrome, palette)));
                let menu = state.menu.as_ref().filter(|mm| mm.key == item.key);
                if item.menu.is_none() {
                    h += EMPTY_H + ASK_BUTTON_H;
                    let key = item.key.clone();
                    col = col.child(div().h(px(GAP))).child(reveal(
                        &p,
                        1,
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(empty_state("grid", t("No menu"), t("This app doesn't share a menu with the shell"), EMPTY_H, palette))
                            .child(pill_button("tray-ctx", None, t("Ask the app for its menu"), accent, palette, move |_, cx| {
                                send(cx, TrayCommand::ContextMenu { item: key.clone(), x: 0, y: 0 });
                                close_panel(cx);
                            })),
                    ));
                } else if let Some(menu) = menu {
                    // Follow the drill-down path as far as it still exists.
                    let mut level = &menu.root;
                    let mut crumbs: Vec<&str> = Vec::new();
                    for id in &stack {
                        match level.children.iter().find(|c| c.id == *id) {
                            Some(next) => {
                                crumbs.push(&next.label);
                                level = next;
                            }
                            None => break,
                        }
                    }
                    let (rows, rows_h) = level_rows(level, monochrome, palette);
                    let back = !crumbs.is_empty();
                    let body_h = if rows.is_empty() { ROW_H } else { rows_h } + if back { ROW_H + SEP_H } else { 0.0 };
                    let list_h = body_h.min(MAX_LIST_H);
                    h += list_h + MENU_CARD_PAD * 2.0;
                    let mut list = div().id("tray-menu-list").h(px(list_h)).overflow_y_scroll().flex().flex_col();
                    if back {
                        let parent = crumbs.iter().rev().nth(1).map_or_else(|| item.display_name(), |s| s.to_string());
                        list = list.child(back_row(parent, palette)).child(separator(palette));
                    }
                    if rows.is_empty() {
                        list = list.child(div().h(px(ROW_H)).px(px(ROW_PAD_X_PX)).flex().items_center().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(t("Empty")));
                    } else {
                        list = list.children(rows);
                    }
                    col = col.child(div().h(px(GAP))).child(reveal(&p, 1, glass_card(palette).rounded(px(shape.inner(PAD))).p(px(MENU_CARD_PAD)).child(list)));
                } else {
                    // Same height as a one-row menu so the panel doesn't jump when it loads.
                    h += ROW_H + MENU_CARD_PAD * 2.0;
                    col = col.child(div().h(px(GAP))).child(reveal(
                        &p,
                        1,
                        div()
                            .h(px(ROW_H + MENU_CARD_PAD * 2.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(text::BODY_S))
                            .text_color(palette.on_surface_variant.hsla())
                            .child(t("Loading menu…")),
                    ));
                }
            }
        }
        if (f32::from(size_cell.get().height) - h).abs() > SIZE_EPSILON_PX {
            size_cell.set(size(px(PANEL_W), px(h)));
        }
        col.into_any_element()
    }
}

// ── Registration ─────────────────────────────────────────────────────────

/// An integer argument clamped into `i32` (no silent wrap-around).
fn saturate_i32(v: i64) -> i32 {
    i32::try_from(v).unwrap_or(if v < 0 { i32::MIN } else { i32::MAX })
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("System tray"), tk("Status icons and menus of background apps"), Some(Zone::Right)), |window, cx, panel_cx| {
        let m = model(cx);
        let bar = cx
            .new(|cx| {
                let v = FnView::new(None, bar_view, window, cx);
                cx.observe(&m, |_, _, cx| cx.notify()).detach();
                v
            })
            .into();
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_PANEL_H))));
        let render = panel_view(panel_cx.clone(), size_cell.clone());
        let panel = cx
            .new(|cx| {
                let v = FnView::new(None, render, window, cx);
                cx.observe(&m, |_, _, cx| cx.notify()).detach();
                v
            })
            .into();
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            // The chip (outside the icons) shows every item, passive ones too.
            on_click: Some(Rc::new(|_, cx| {
                if panel_open(cx) {
                    close_panel(cx);
                } else {
                    open_overview(cx);
                }
            })),
            ..PluginInstance::empty()
        }
    });

    r.command(ID, CommandSpec::new("tray status", tk("Tray items (role, status, icons, menus)"), |_, cx| Ok(json(&*model(cx).read(cx).state))));
    r.command(
        ID,
        CommandSpec::new_output("tray activate", tk("Activate a tray item (its primary action)"), |a, cx| request(cx, TrayCommand::Activate { item: a.rest(0), x: 0, y: 0 }))
            .arg(ArgSpec::required("item", ArgKind::String, tk("item id, title or key"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("tray secondary", tk("Secondary action of a tray item (middle-click)"), |a, cx| request(cx, TrayCommand::SecondaryActivate { item: a.rest(0), x: 0, y: 0 }))
            .arg(ArgSpec::required("item", ArgKind::String, tk("item id, title or key"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("tray scroll", tk("Scroll a tray item by steps (negative = up)"), |a, cx| {
            let delta = saturate_i32(a.int(1)?);
            request(cx, TrayCommand::Scroll { item: a.str(0)?.to_string(), delta, horizontal: false })
        })
        .arg(ArgSpec::required("item", ArgKind::String, tk("item id, title or key")))
        .arg(ArgSpec::required("delta", ArgKind::Integer, tk("steps"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("tray menu", tk("Load a tray item's menu and print its entries with ids"), |a, cx| request(cx, TrayCommand::OpenMenu(a.rest(0))))
            .arg(ArgSpec::required("item", ArgKind::String, tk("item id, title or key"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("tray click", tk("Click an entry of the loaded tray menu by id"), |a, cx| {
            let id = saturate_i32(a.int(0)?);
            request(cx, TrayCommand::MenuClick { id })
        })
            .arg(ArgSpec::required("id", ArgKind::Integer, tk("menu entry id (from `tray menu`)"))),
    );
    r.command(
        ID,
        CommandSpec::new("tray open", tk("Open the tray panel (an item's menu, or the overview)"), |a, cx| {
            let query = a.rest(0);
            if query.is_empty() {
                open_overview(cx);
                return Ok(json!({ "open": "overview" }));
            }
            let key = model(cx).read(cx).state.find(&query).map(|i| i.key.clone()).ok_or_else(|| tf("No tray item matching \"{query}\"", &[("query", &query)]))?;
            if !(panel_open(cx) && model(cx).read(cx).selected.as_deref() == Some(key.as_str()) && !model(cx).read(cx).stale) {
                open_menu(&key, cx);
            }
            Ok(json!({ "open": key }))
        })
        .arg(ArgSpec::optional("item", ArgKind::String, tk("item id, title or key"))),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monochrome_detection() {
        let white = Pixmap::from_rgba(2, 1, vec![255, 255, 255, 255, 250, 250, 250, 200]).unwrap();
        let color = Pixmap::from_rgba(2, 1, vec![255, 0, 0, 255, 0, 200, 40, 255]).unwrap();
        assert!(is_monochrome(&white));
        assert!(!is_monochrome(&color));
    }

    #[test]
    fn theme_path_lookup_is_graceful() {
        assert!(theme_path_icon("", "x").is_none());
        assert!(theme_path_icon("/nonexistent", "x").is_none());
    }
}
