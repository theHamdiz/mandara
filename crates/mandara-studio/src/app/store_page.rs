//! Studio → Plugins: the plugin store.
//!
//! One page, five places ([`PluginsTab`]):
//!
//! * **Discover** — the store front: a featured hero that rotates over the
//!   plugins' cover art, curated shelves built from the plugin categories
//!   ("Essentials", "For your desk", …), "New & updated" and community
//!   picks from the marketplace, and a "make your own" band.
//! * **Installed** — the library: everything on the system with its on /
//!   off switch and settings, removed built-ins (restore) and third-party
//!   plugins.
//! * **Updates** — marketplace plugin updates, one by one or all at once.
//! * **Community** — the GitHub marketplace (`market_page.rs`).
//! * **Develop** — the plugin author's handbook (`plugins_local.rs`).
//!
//! A store-wide search with filter chips narrows Discover and Installed as
//! you type and searches the marketplace on Community. Every card opens
//! the plugin's product page (`plugin_modal.rs`) and carries a one-click
//! Get / On button that shows its state (on, off, removed, update).
//!
//! Cost on a 144 Hz desktop: cover art is decoded off the UI thread at the
//! size it is drawn, only once a card is actually on screen, and kept in a
//! small byte-budgeted cache; the hero's timer only runs while Discover is
//! showing in a visible window, and its crossfade only requests frames
//! while it moves.

use mandara_ui::bidi::Bidi as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Div, Entity, FontWeight, ImageSource, InteractiveElement, IntoElement,
    ObjectFit, ParentElement, Pixels, RenderImage, SharedString, Stateful, StatefulInteractiveElement, Styled, StyledImage as _, Subscription,
    Task, WeakEntity, Window, canvas, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_core::{FrameClock, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{PluginManifest, PluginOrigin, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::icons::icon;
use mandara_ui::plugins::lifecycle::Action;

use super::plugin_modal::{self, ModalView};
use super::plugins_local::{LocalSection, PluginsTab};
use super::{StudioView, grid};
use crate::registry::Section;
use crate::text_field::TextField;
use crate::tokens::*;

// ── Catalog ──────────────────────────────────────────────────────────────

/// Plugins Studio never lists (internal infrastructure).
pub(crate) const HIDDEN_PLUGINS: &[&str] = &["compositor"];

/// Built-in plugin categories, in order (built-ins by id; third-party
/// plugins get their own group).
pub(crate) const PLUGIN_CATEGORIES: &[(&str, &[&str])] = &[
    (tk("Bar essentials"), &["workspaces", "active-window", "clock", "language", "keyboard-layout", "main-menu", "session", "quick-settings", "dock"]),
    (tk("System"), &["network", "bluetooth", "audio", "tray", "resources", "mini-stats", "processes", "notifications", "clipboard", "drives", "power", "privacy"]),
    (tk("Maintenance"), &["updates", "services", "containers", "app-installer"]),
    (tk("Media & capture"), &["media", "screenshot", "color-picker", "soundscapes"]),
    (tk("Look & feel"), &["wallpaper", "wallpaper-generator", "wallhaven", "cursors"]),
    (tk("Productivity"), &["todo", "notes", "pomodoro", "authenticator", "screen-time"]),
    (tk("Everyday info"), &["weather", "prayer-times", "world-clock", "net-speed"]),
    (tk("Third-party"), &[]),
];

/// A curated Discover shelf: title, one line, and the categories it shows.
struct Shelf {
    key: &'static str,
    title: &'static str,
    line: &'static str,
    categories: &'static [&'static str],
}

/// Discover's shelves, in order. A built-in outside every category joins
/// the first shelf.
const SHELVES: &[Shelf] = &[
    Shelf { key: "essentials", title: tk("Essentials"), line: tk("The pieces every Mandara bar is built from"), categories: &[tk("Bar essentials")] },
    Shelf { key: "desk", title: tk("For your desk"), line: tk("Weather, prayer times, world clocks and more at a glance"), categories: &[tk("Everyday info")] },
    Shelf { key: "productivity", title: tk("Productivity"), line: tk("Tasks, notes, focus timers and sign-in codes"), categories: &[tk("Productivity")] },
    Shelf { key: "media", title: tk("Media & capture"), line: tk("Music, screenshots, color and ambient sound"), categories: &[tk("Media & capture")] },
    Shelf { key: "look", title: tk("Look & feel"), line: tk("Wallpapers, generated art, online galleries and cursor themes"), categories: &[tk("Look & feel")] },
    Shelf { key: "system", title: tk("System tools"), line: tk("Network, sound, power and upkeep, one click away"), categories: &[tk("System"), tk("Maintenance")] },
];

/// Featured on the Discover hero, in order (built-ins with cover art).
const FEATURED: &[&str] = &["media", "wallpaper", "weather", "clipboard", "screenshot", "soundscapes"];

// ── Layout ───────────────────────────────────────────────────────────────

/// The store search field beside the navigation.
const SEARCH_W_PX: f32 = 300.0;
/// Hero: height per unit of the column width, within these bounds.
const HERO_ASPECT: f32 = 0.36;
const HERO_MIN_H_PX: f32 = 300.0;
const HERO_MAX_H_PX: f32 = 380.0;
const HERO_RADIUS_PX: f32 = 28.0;
const HERO_PAD_PX: f32 = 32.0;
/// The hero's text column needs this much beside the framed cover;
/// narrower heroes drop the frame and keep the blurred backdrop.
const HERO_TEXT_MIN_W_PX: f32 = 320.0;
/// The framed cover: corner radius and drop shadow.
const HERO_FRAME_RADIUS_PX: f32 = 20.0;
const HERO_FRAME_SHADOW_Y_PX: f32 = 14.0;
const HERO_FRAME_SHADOW_BLUR_PX: f32 = 36.0;
const HERO_TITLE_PX: f32 = 36.0;
const HERO_PITCH_LINES: usize = 3;
/// The scrim behind the hero text: the dim surface at this opacity on the
/// text side, fading out across the art.
const HERO_SCRIM_ALPHA: f32 = 0.88;
const HERO_SCRIM_END_ALPHA: f32 = 0.35;
/// Page dots: a dot, and the stretched dot of the current slide.
const DOT_PX: f32 = 7.0;
const DOT_ACTIVE_W_PX: f32 = 24.0;
/// Time each featured plugin stays on the hero, and the shortest wait
/// between two checks of that clock.
const HERO_INTERVAL: Duration = Duration::from_secs(7);
const HERO_MIN_WAIT: Duration = Duration::from_millis(500);
/// Hero crossfade: critically damped, so neither slide flashes back.
const HERO_FADE_STIFFNESS: f32 = 120.0;
/// Angle of a reading-direction gradient (start → end) and its mirror.
const GRADIENT_START_TO_END_DEG: f32 = 90.0;
const GRADIENT_END_TO_START_DEG: f32 = 270.0;
const GRADIENT_DIAGONAL_DEG: f32 = 135.0;
/// Store cards.
const CARD_RADIUS_PX: f32 = 20.0;
const CARD_PAD_PX: f32 = 14.0;
/// The icon tile beside a card's name.
const CARD_TILE_PX: f32 = 36.0;
/// A card's pitch is clamped to this many lines (cards stay one height).
const CARD_PITCH_LINES: usize = 2;
/// Placeholder glyph on a cover without art, as a share of its height.
const COVER_GLYPH: f32 = 0.32;
/// Filter chips.
const CHIP_H_PX: f32 = 32.0;
/// Store buttons: compact (cards) and large (hero, product page).
const BUTTON_SMALL_H_PX: f32 = 30.0;
const BUTTON_LARGE_H_PX: f32 = 40.0;
const BUTTON_SMALL_MIN_W_PX: f32 = 64.0;
const BUTTON_LARGE_MIN_W_PX: f32 = 104.0;
/// Hover lift of a filled button (OKLCH lightness).
const BUTTON_HOVER_LIGHTEN: f32 = 0.04;
/// Segmented navigation.
const SEGMENT_H_PX: f32 = 36.0;
const SEGMENT_PAD_PX: f32 = 4.0;
/// Library summary tiles.
const STAT_TILE_MIN_W_PX: f32 = 150.0;
const STAT_VALUE_PX: f32 = 26.0;

// ── Plugin tiles ─────────────────────────────────────────────────────────

/// Artwork tile on a library card.
const PLUGIN_TILE_PX: f32 = 40.0;
/// Tile corner radius and glyph size, as fractions of its side.
const PLUGIN_TILE_RADIUS: f32 = 0.3;
const PLUGIN_TILE_GLYPH: f32 = 0.48;

// ── Cover art cache ──────────────────────────────────────────────────────

/// Decode widths snap up to this many device pixels, so resizing the
/// window re-decodes a cover only now and then.
const COVER_BUCKET_PX: f32 = 64.0;
/// Width an ambient (blurred) backdrop is decoded at: the GPU's filtering
/// of a tiny image stretched over the hero is the blur.
const AMBIENT_PX: f32 = 40.0;
/// Decoded covers kept at most (least recently shown go first).
const COVER_BUDGET_BYTES: usize = 24 * 1024 * 1024;
const BGRA_BYTES: usize = 4;
/// Cover aspect ratios are snapped to hundredths.
const RATIO_STEPS: f32 = 100.0;

/// Where a plugin's cover comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Art {
    /// A built-in's embedded cover (`mandara_ui::plugin_art`).
    Builtin(String),
    /// A third-party plugin's `cover.png`.
    File(PathBuf),
}

/// How a cover is drawn.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Detail {
    /// Crisp, at the drawn size.
    Sharp,
    /// A tiny decode stretched over the box: a soft, blurred backdrop.
    Ambient,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CoverKey {
    art: Art,
    w: u32,
    h: u32,
}

enum CoverState {
    Loading,
    Ready(Arc<RenderImage>),
    Missing,
}

struct CoverSlot {
    state: CoverState,
    /// Frame tick it was last on screen.
    used: u64,
    bytes: usize,
}

#[derive(Default)]
struct Covers {
    slots: HashMap<CoverKey, CoverSlot>,
    tick: u64,
    bytes: usize,
    /// Repainted when a decode lands.
    view: Option<WeakEntity<StudioView>>,
}

impl Covers {
    /// Drops the least recently shown covers until the cache fits its
    /// budget (never one shown this frame); returns them for the atlas.
    fn evict(&mut self) -> Vec<Arc<RenderImage>> {
        let mut out = Vec::new();
        while self.bytes > COVER_BUDGET_BYTES {
            let oldest = self.slots.iter().filter(|(_, s)| s.used < self.tick && matches!(s.state, CoverState::Ready(_))).min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone());
            let Some(key) = oldest else { break };
            if let Some(slot) = self.slots.remove(&key) {
                self.bytes = self.bytes.saturating_sub(slot.bytes);
                if let CoverState::Ready(image) = slot.state {
                    out.push(image);
                }
            }
        }
        out
    }
}

thread_local! {
    static COVERS: RefCell<Covers> = RefCell::default();
    /// Third-party artwork files found (or not) in plugin folders: checked
    /// once per page visit rather than with a `stat` per card per frame.
    static ART_FILES: RefCell<HashMap<PathBuf, bool>> = RefCell::default();
}

/// Starts a frame of cover drawing (the store page and the product page
/// call it before laying out covers).
pub(super) fn covers_begin(view: WeakEntity<StudioView>) {
    COVERS.with(|c| {
        let mut c = c.borrow_mut();
        c.tick += 1;
        c.view = Some(view);
    });
}

/// A plugin's cover, when it has one.
pub(super) fn art_of(m: &PluginManifest) -> Option<Art> {
    match &m.origin {
        PluginOrigin::Builtin => mandara_ui::plugin_art::has_art(&m.id).then(|| Art::Builtin(m.id.clone())),
        PluginOrigin::ThirdParty { path } => third_party_art(path, PLUGIN_COVER_FILE).map(Art::File),
    }
}

fn cover_key(art: Art, w: f32, h: f32, scale: f32, detail: Detail) -> CoverKey {
    // Snapped too, so a box whose height rounds differently reuses the decode.
    let ratio = (h / w.max(1.0) * RATIO_STEPS).round() / RATIO_STEPS;
    let dw = match detail {
        Detail::Sharp => ((w * scale) / COVER_BUCKET_PX).ceil().max(1.0) * COVER_BUCKET_PX,
        Detail::Ambient => AMBIENT_PX,
    };
    CoverKey { art, w: dw as u32, h: (dw * ratio).round().max(1.0) as u32 }
}

/// Draws `art` filling a `w × h` box: decoded at the drawn size the first
/// time the box is on screen, `fallback` until then (or without art).
pub(super) fn art_box(art: Option<Art>, w: f32, h: f32, scale: f32, detail: Detail, fallback: AnyElement) -> AnyElement {
    let Some(art) = art else { return fallback };
    let key = cover_key(art, w, h, scale, detail);
    let ready = COVERS.with(|c| match c.borrow().slots.get(&key) {
        Some(CoverSlot { state: CoverState::Ready(image), .. }) => Some(image.clone()),
        _ => None,
    });
    let content = match ready {
        Some(image) => img(ImageSource::Render(image)).size_full().object_fit(ObjectFit::Cover).into_any_element(),
        None => fallback,
    };
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .child(content)
        .child(canvas(move |bounds, window, cx| want_cover(key, bounds, window, cx), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
        .into_any_element()
}

/// Called while a cover box is laid out: starts its decode once it is
/// actually inside the visible area.
fn want_cover(key: CoverKey, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    if !bounds.intersects(&window.content_mask().bounds) {
        return;
    }
    let fresh = COVERS.with(|c| {
        let mut c = c.borrow_mut();
        let tick = c.tick;
        match c.slots.get_mut(&key) {
            Some(slot) => {
                slot.used = tick;
                false
            }
            None => {
                c.slots.insert(key.clone(), CoverSlot { state: CoverState::Loading, used: tick, bytes: 0 });
                true
            }
        }
    });
    if !fresh {
        return;
    }
    let job = key.clone();
    let task = cx.background_executor().spawn(async move {
        match &job.art {
            Art::Builtin(id) => mandara_ui::plugin_art::cover_thumb(id, job.w, job.h),
            Art::File(path) => mandara_ui::plugin_art::file_thumb(path, job.w, job.h),
        }
    });
    cx.spawn(async move |cx| {
        let image = task.await;
        let (view, evicted) = COVERS.with(|c| {
            let mut c = c.borrow_mut();
            // The slot may be gone (evicted while decoding): drop the result.
            if c.slots.contains_key(&key) {
                let bytes = image.as_ref().map_or(0, |i| {
                    let size = i.size(0);
                    size.width.0.max(0) as usize * size.height.0.max(0) as usize * BGRA_BYTES
                });
                c.bytes += bytes;
                if let Some(slot) = c.slots.get_mut(&key) {
                    slot.bytes = bytes;
                    slot.state = match image {
                        Some(i) => CoverState::Ready(i),
                        None => CoverState::Missing,
                    };
                }
            }
            (c.view.clone(), c.evict())
        });
        cx.update(|cx| {
            for image in evicted {
                cx.drop_image(image, None);
            }
            if let Some(view) = view {
                let _ = view.update(cx, |_, cx| cx.notify());
            }
        });
    })
    .detach();
}

/// A third-party plugin's own artwork file.
pub(crate) const PLUGIN_ICON_FILE: &str = "icon.png";
/// A third-party plugin's own cover file.
pub(crate) const PLUGIN_COVER_FILE: &str = "cover.png";

/// Forgets which plugin artwork files exist (on page navigation).
pub(crate) fn plugin_art_cache_clear() {
    ART_FILES.with(|files| files.borrow_mut().clear());
}

/// `dir/file` when that artwork file exists.
pub(crate) fn third_party_art(dir: &std::path::Path, file: &str) -> Option<PathBuf> {
    let path = dir.join(file);
    let exists = ART_FILES.with(|files| *files.borrow_mut().entry(path.clone()).or_insert_with(|| path.is_file()));
    exists.then_some(path)
}

pub(crate) fn plugin_icon(id: &str) -> &'static str {
    match id {
        "workspaces" => "grid",
        "active-window" => "window",
        "clock" => "calendar",
        "keyboard-layout" => "keyboard",
        "language" => "globe",
        "main-menu" => "sidebar",
        "session" => "power",
        "quick-settings" => "bolt",
        "network" => "wifi-4",
        "bluetooth" => "bluetooth",
        "audio" => "volume-high",
        "tray" => "grid",
        "resources" => "cpu",
        "processes" => "crosshair",
        "drives" => "disk",
        "power" => "gauge",
        "updates" => "package",
        "services" => "server",
        "containers" => "box",
        "notifications" => "bell",
        "clipboard" => "clipboard",
        "media" => "music",
        "screenshot" => "camera",
        "wallpaper" => "image",
        "wallpaper-generator" => "sparkles",
        "wallhaven" => "cloud",
        "cursors" => "arrow",
        "mini-stats" => "cpu",
        "todo" => "list-check",
        "notes" => "note",
        "pomodoro" => "timer",
        "weather" => "cloud-sun",
        "prayer-times" => "crescent",
        "world-clock" => "globe",
        "net-speed" => "gauge",
        "authenticator" => "shield",
        "color-picker" => "eyedropper",
        "soundscapes" => "waves",
        "dock" => "tiles",
        "app-installer" => "download",
        "privacy" => "eye-off",
        "screen-time" => "clock",
        _ => "grid",
    }
}

/// A plugin's artwork tile: the built-in's color icon (the 256 px variant
/// when `side` × `scale` needs it), a third-party plugin's own `icon.png`,
/// or a tinted symbol.
pub(crate) fn plugin_tile(m: &PluginManifest, side: f32, enabled: bool, scale: f32, p: &MandaraPalette) -> AnyElement {
    let radius = side * PLUGIN_TILE_RADIUS;
    let art: Option<ImageSource> = match &m.origin {
        PluginOrigin::Builtin => mandara_ui::plugin_art::icon(&m.id, side * scale / mandara_ui::plugin_art::ICON_PX).map(ImageSource::from),
        PluginOrigin::ThirdParty { path } => third_party_art(path, PLUGIN_ICON_FILE).map(ImageSource::from),
    };
    match art {
        Some(src) => img(src).size(px(side)).flex_none().rounded(px(radius)).when(!enabled, |el| el.opacity(ALPHA_45)).into_any_element(),
        None => div()
            .size(px(side))
            .flex_none()
            .rounded(px(radius))
            .row()
            .items_center()
            .justify_center()
            .bg(if enabled { p.primary.with_alpha(ALPHA_14).hsla() } else { p.surface_container_high.hsla() })
            .child(icon(plugin_icon(&m.id), side * PLUGIN_TILE_GLYPH, if enabled { p.primary } else { p.on_surface_variant }))
            .into_any_element(),
    }
}

/// A cover stand-in: an OKLCH glass gradient around `tone` with a glyph.
pub(super) fn cover_placeholder(glyph: &'static str, tone: Color, w: f32, h: f32, p: &MandaraPalette) -> AnyElement {
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .bg(mandara_core::oklab_gradient(GRADIENT_DIAGONAL_DEG, tone.with_alpha(ALPHA_55), tone.rotate_hue(HUE_SHIFT_DEG).with_alpha(ALPHA_30)))
        .row()
        .items_center()
        .justify_center()
        .child(mandara_core::grain_overlay())
        .child(icon(glyph, h * COVER_GLYPH, p.on_primary.with_alpha(ALPHA_90)))
        .into_any_element()
}

/// A built-in's cover when it has no art: in the style of the drawn
/// covers — a glass gradient in a hue of its own, its name in the corner
/// and its symbol on a lit tile.
pub(super) fn plugin_placeholder(m: &PluginManifest, w: f32, h: f32, p: &MandaraPalette) -> AnyElement {
    let hue = m.id.bytes().fold(0u32, |acc, b| acc.wrapping_mul(HASH_MULTIPLIER).wrapping_add(u32::from(b))) % FULL_TURN_DEG;
    let tone = p.primary.rotate_hue(hue as f32);
    let tile = (h * PLACEHOLDER_TILE_SHARE).floor();
    // Label and glyph take whichever of the theme's ink and paper reads best.
    let label = tone.with_lightness((PLACEHOLDER_DARK_L + PLACEHOLDER_LIGHT_L) / 2.0).best_on(p.on_surface, p.surface_base);
    let glyph = tone.with_lightness((PLACEHOLDER_TILE_TOP_L + PLACEHOLDER_TILE_BOTTOM_L) / 2.0).best_on(p.on_surface, p.surface_base);
    div()
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .bg(mandara_core::oklab_gradient(GRADIENT_DIAGONAL_DEG, tone.with_lightness(PLACEHOLDER_DARK_L).with_alpha(1.0), tone.rotate_hue(HUE_SHIFT_DEG).with_lightness(PLACEHOLDER_LIGHT_L).with_alpha(1.0)))
        .row()
        .items_center()
        .justify_center()
        .child(mandara_core::grain_overlay())
        .child(div().absolute().top(px(h * PLACEHOLDER_INSET_SHARE)).inset_start(px(h * PLACEHOLDER_INSET_SHARE)).text_size(px(TEXT_14_PX)).font_weight(FontWeight::MEDIUM).text_color(label.with_alpha(ALPHA_90).hsla()).child(m.display_name()))
        .child(
            div()
                .size(px(tile))
                .rounded(px(tile * PLUGIN_TILE_RADIUS))
                .row()
                .items_center()
                .justify_center()
                .bg(mandara_core::oklab_gradient(GRADIENT_DIAGONAL_DEG, tone.with_lightness(PLACEHOLDER_TILE_TOP_L), tone.with_lightness(PLACEHOLDER_TILE_BOTTOM_L)))
                .shadow({
                    let mut s = mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow);
                    s.push(gpui::BoxShadow { color: gpui::black().opacity(ALPHA_30), offset: gpui::point(px(0.0), px(tile * PLACEHOLDER_SHADOW_SHARE)), blur_radius: px(tile * PLACEHOLDER_SHADOW_SHARE * 2.0), spread_radius: px(0.0), inset: false });
                    s
                })
                .child(icon(plugin_icon(&m.id), tile * PLUGIN_TILE_GLYPH, glyph)),
        )
        .into_any_element()
}

/// Placeholder covers: the tile's share of the height, the label inset,
/// OKLCH lightness of the backdrop and tile stops, and the tile's shadow.
const PLACEHOLDER_TILE_SHARE: f32 = 0.42;
const PLACEHOLDER_INSET_SHARE: f32 = 0.11;
const PLACEHOLDER_DARK_L: f32 = 0.3;
const PLACEHOLDER_LIGHT_L: f32 = 0.5;
const PLACEHOLDER_TILE_TOP_L: f32 = 0.72;
const PLACEHOLDER_TILE_BOTTOM_L: f32 = 0.52;
const PLACEHOLDER_SHADOW_SHARE: f32 = 0.08;
/// A plugin id hashed to a placeholder hue.
const HASH_MULTIPLIER: u32 = 31;
const FULL_TURN_DEG: u32 = 360;

/// Hue turn between a placeholder cover's two gradient stops.
const HUE_SHIFT_DEG: f32 = 40.0;

/// The 1px specular rim drawn above a card's art (insets paint under
/// children, so the rim gets its own top layer).
pub(super) fn rim_overlay(radius: f32, p: &MandaraPalette) -> Div {
    div().absolute().top_0().left_0().size_full().rounded(px(radius)).shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
}

// ── Buttons, chips, badges ───────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ButtonSize {
    Small,
    Large,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    /// Filled with the primary color: the action (Get, Update, Add back).
    Primary,
    /// Tinted: a settled state (On).
    Tonal,
    /// Neutral glass: secondary actions.
    Glass,
}

/// A store pill button.
pub(super) fn pill(id: impl Into<SharedString>, glyph: Option<&'static str>, label: impl Into<SharedString>, tone: Tone, size: ButtonSize, p: &MandaraPalette) -> Stateful<Div> {
    let (bg, fg) = match tone {
        Tone::Primary => (p.primary, p.on_primary),
        Tone::Tonal => (p.primary.with_alpha(ALPHA_16), p.primary),
        Tone::Glass => (p.surface_container_high, p.on_surface),
    };
    let (h, min_w, text, glyph_px, pad) = match size {
        ButtonSize::Small => (BUTTON_SMALL_H_PX, BUTTON_SMALL_MIN_W_PX, TEXT_12_PX, ICON_13_PX, SPACE_12_PX),
        ButtonSize::Large => (BUTTON_LARGE_H_PX, BUTTON_LARGE_MIN_W_PX, TEXT_13_5_PX, ICON_15_PX, SPACE_18_PX),
    };
    div()
        .id(id.into())
        .flex_none()
        .h(px(h))
        .min_w(px(min_w))
        .px(px(pad))
        .rounded_full()
        .row()
        .items_center()
        .justify_center()
        .gap(px(SPACE_6_PX))
        .cursor_pointer()
        .bg(bg.hsla())
        .text_color(fg.hsla())
        .text_size(px(text))
        .font_weight(FontWeight::SEMIBOLD)
        .when(tone != Tone::Tonal, |el| el.shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow)))
        .hover(move |s| s.bg(bg.lighten(BUTTON_HOVER_LIGHTEN).hsla()))
        .when_some(glyph, |el, g| el.child(icon(g, glyph_px, fg)))
        .child(label.into())
}

/// A small tinted label (origin, state).
pub(super) fn badge(text: impl Into<SharedString>, color: Color) -> Div {
    div()
        .flex_none()
        .px(px(SPACE_8_PX))
        .h(px(DIM_20_PX))
        .rounded_full()
        .row()
        .items_center()
        .bg(color.with_alpha(ALPHA_14).hsla())
        .text_color(color.hsla())
        .text_size(px(TEXT_10_5_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// Who made a plugin, as a badge: Mandara (bundled), development,
/// system package or third-party.
pub(super) fn origin_badge(m: &PluginManifest, p: &MandaraPalette) -> (&'static str, Color) {
    match &m.origin {
        PluginOrigin::Builtin => (t("Mandara"), p.primary),
        PluginOrigin::ThirdParty { path } => {
            let s = path.to_string_lossy();
            if s.contains("plugins-dev") {
                (t("Development"), p.tertiary)
            } else if s.starts_with("/usr") {
                (t("System"), p.on_surface_variant)
            } else {
                (t("Third-party"), p.secondary)
            }
        }
    }
}

/// A store section heading: title, one line and an optional "See all".
fn shelf_head(title: SharedString, line: SharedString, see_all: Option<Stateful<Div>>, p: &MandaraPalette) -> Div {
    div()
        .row()
        .items_end()
        .gap(px(SPACE_12_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(SPACE_3_PX))
                .child(div().text_size(px(TEXT_18_PX)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(line)),
        )
        .children(see_all)
}

/// "See all ›" beside a shelf title.
fn see_all(id: impl Into<SharedString>, count: Option<usize>, p: &MandaraPalette) -> Stateful<Div> {
    let label = match count {
        Some(n) => tf("See all {count}", &[("count", &n)]),
        None => t("See all").to_string(),
    };
    div()
        .id(id.into())
        .flex_none()
        .h(px(DIM_28_PX))
        .px(px(SPACE_10_PX))
        .rounded_full()
        .row()
        .items_center()
        .gap(px(SPACE_4_PX))
        .cursor_pointer()
        .text_size(px(TEXT_12_5_PX))
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.primary.hsla())
        .font_features(mandara_ui::fonts::tabular_nums())
        .hover(|s| s.bg(p.primary.with_alpha(ALPHA_10).hsla()))
        .child(label)
        .child(icon(mandara_ui::bidi::dir_icon("chevron-right"), ICON_12_PX, p.primary))
}

/// A plugins-page section heading: icon, title, count and one line.
fn section_head(glyph: &'static str, title: impl Into<SharedString>, count: usize, line: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    div()
        .row()
        .items_center()
        .gap(px(SPACE_12_PX))
        .child(div().flex_none().size(px(DIM_34_PX)).rounded(px(RADIUS_12_PX)).bg(p.primary.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_16_PX, p.primary)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .row()
                .flex_col()
                .gap(px(SPACE_3_PX))
                .child(
                    div()
                        .row()
                        .items_center()
                        .gap(px(SPACE_8_PX))
                        .child(div().text_size(px(TEXT_17_PX)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
                        .child(div().px(px(SPACE_8_PX)).h(px(DIM_22_PX)).rounded_full().row().items_center().bg(p.surface_container_high.hsla()).text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).font_features(mandara_ui::fonts::tabular_nums()).child(count.to_string())),
                )
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(line.into())),
        )
        .into_any_element()
}

// ── State ────────────────────────────────────────────────────────────────

/// A filter chip under the search field.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Filter {
    #[default]
    All,
    /// Discover: one curated shelf.
    Shelf(usize),
    /// Discover and Installed: third-party plugins only.
    ThirdParty,
    /// Installed: by state or maker.
    On,
    Off,
    Mandara,
}

/// The Discover hero carousel.
struct Hero {
    index: usize,
    /// The slide fading out underneath, while the crossfade runs.
    prev: Option<usize>,
    fade: SpringValue<f32>,
    clock: FrameClock,
    shown_at: Instant,
    hovered: bool,
    /// Featured plugins last frame (the timer wraps around them).
    count: usize,
    /// The rotation timer is alive (it ends itself once the hero is gone).
    running: bool,
    task: Option<Task<()>>,
}

impl Default for Hero {
    fn default() -> Self {
        Self {
            index: 0,
            prev: None,
            fade: SpringValue::new(1.0, SpringConfig::critical(HERO_FADE_STIFFNESS)),
            clock: FrameClock::new(),
            shown_at: Instant::now(),
            hovered: false,
            count: 0,
            running: false,
            task: None,
        }
    }
}

impl Hero {
    fn stop(&mut self) {
        self.task = None;
        self.running = false;
    }
}

/// Store state, owned by [`StudioView`].
#[derive(Default)]
pub struct Store {
    search: Option<Entity<TextField>>,
    pub(super) query: String,
    filter: Filter,
    hero: Hero,
    visibility: Option<Subscription>,
}

/// The plugins a store view lists: (enabled, manifest), by name.
fn listed(cfg: &ShellConfig, cx: &App) -> Vec<(bool, PluginManifest)> {
    let registry = cx.global::<PluginRegistry>();
    let mut all: Vec<(bool, PluginManifest)> = registry.manifests().filter(|m| !HIDDEN_PLUGINS.contains(&m.id.as_str())).map(|m| (registry.is_enabled(&m.id, &cfg.plugins), m.clone())).collect();
    all.sort_by_cached_key(|(_, m)| m.display_name().to_lowercase());
    all
}

/// The Discover shelf a built-in sits on (third-party plugins: none).
fn shelf_of(m: &PluginManifest) -> Option<usize> {
    if m.origin != PluginOrigin::Builtin {
        return None;
    }
    let category = plugin_modal::category(m);
    Some(SHELVES.iter().position(|s| s.categories.contains(&category)).unwrap_or(0))
}

/// Lowercased search terms.
fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether every term appears in the plugin's name, pitch, id or category.
fn matches(m: &PluginManifest, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let hay = format!("{} {} {} {}", m.display_name(), m.display_description(), m.id, tr(plugin_modal::category(m))).to_lowercase();
    terms.iter().all(|t| hay.contains(t.as_str()))
}

impl StudioView {
    /// The Plugins page: store navigation + search, then the current place.
    pub(super) fn store_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        covers_begin(cx.entity().downgrade());
        self.updates_prime(cx);
        if self.store.visibility.is_none() {
            // Back on screen: render again (which restarts the hero's
            // timer); hidden: stop it at once.
            self.store.visibility = Some(cx.observe_window_visibility(window, |this, visibility, _, cx| {
                if visibility.is_visible() {
                    cx.notify();
                } else {
                    this.store.hero.stop();
                }
            }));
        }
        let header = self.store_header(p, cx);
        let body = match self.plugins_tab {
            PluginsTab::Discover => self.discover(cfg, p, window, cx),
            PluginsTab::Installed => self.library(cfg, p, cx),
            PluginsTab::Updates => self.updates_view(p, cx),
            PluginsTab::Marketplace => self.marketplace(p, cx),
            PluginsTab::Local => self.plugins_local(cfg, p, cx),
        };
        div().row().flex_col().gap(px(SPACE_20_PX)).child(header).child(body).into_any_element()
    }

    /// Switches the store to `tab` (navigation clicks): fresh filter, top
    /// of the page, and the marketplace searched for the current query.
    pub(super) fn store_tab(&mut self, tab: PluginsTab, cx: &mut Context<Self>) {
        self.plugins_tab = tab;
        self.store.filter = Filter::All;
        self.content_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        if tab == PluginsTab::Marketplace {
            self.market_follow_query(self.store.query.clone(), false, cx);
        }
        cx.notify();
    }

    fn store_set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.store.filter = filter;
        self.content_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// The store search field (made on first use).
    fn store_search(&mut self, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(field) = &self.store.search {
            return field.clone();
        }
        let this = cx.entity().downgrade();
        let field = cx.new(|cx| {
            TextField::new(
                String::new(),
                t("Search plugins…"),
                true,
                Rc::new(move |value: String, _, cx| {
                    let _ = this.update(cx, |studio, cx| {
                        studio.store.query = value.clone();
                        if studio.plugins_tab == PluginsTab::Marketplace {
                            studio.market_follow_query(value, true, cx);
                        }
                        cx.notify();
                    });
                }),
                cx,
            )
        });
        self.store.search = Some(field.clone());
        field
    }

    /// Navigation (Discover · Installed · Updates · Community · Develop)
    /// and, where it applies, the search field.
    fn store_header(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let local = self.local_follow(cx);
        let installed = cx.global::<PluginRegistry>().manifests().filter(|m| !HIDDEN_PLUGINS.contains(&m.id.as_str())).count();
        let updates = self.updates.plugins.iter().filter(|u| u.available).count();
        let current = self.plugins_tab;
        let segments: Vec<AnyElement> = PluginsTab::ALL
            .into_iter()
            .map(|tab| {
                let active = tab == current;
                let (count, alert) = match tab {
                    PluginsTab::Installed => (Some(installed), false),
                    PluginsTab::Updates if updates > 0 => (Some(updates), true),
                    PluginsTab::Local => (local, false),
                    _ => (None, false),
                };
                let fg = if active { p.on_primary } else { p.on_surface_variant };
                div()
                    .id(SharedString::from(format!("plugins-tab-{}", tab.slug())))
                    .h(px(SEGMENT_H_PX))
                    .px(px(SPACE_14_PX))
                    .rounded_full()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .cursor_pointer()
                    .text_size(px(TEXT_13_PX))
                    .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                    .when(active, |el| el.bg(p.primary.hsla()).text_color(p.on_primary.hsla()).shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow)))
                    .when(!active, |el| el.text_color(p.on_surface_variant.hsla()).hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
                    .child(icon(tab.icon(), ICON_14_PX, fg))
                    .child(tab.label())
                    .when_some(count, |el, n| {
                        let tint = if alert && !active { p.warning_amber } else { fg };
                        el.child(
                            div()
                                .min_w(px(DIM_20_PX))
                                .h(px(DIM_18_PX))
                                .px(px(SPACE_6_PX))
                                .rounded_full()
                                .row()
                                .items_center()
                                .justify_center()
                                .bg(tint.with_alpha(ALPHA_18).hsla())
                                .text_color(tint.hsla())
                                .text_size(px(TEXT_11_PX))
                                .font_weight(FontWeight::SEMIBOLD)
                                .font_features(mandara_ui::fonts::tabular_nums())
                                .child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.store_tab(tab, cx)))
                    .into_any_element()
            })
            .collect();
        let nav = div()
            .p(px(SEGMENT_PAD_PX))
            .rounded_full()
            .row()
            .flex_wrap()
            .gap(px(SEGMENT_PAD_PX))
            .bg(p.surface_container.hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .children(segments);
        let searchable = !matches!(current, PluginsTab::Local | PluginsTab::Updates);
        let search = searchable.then(|| self.store_search(cx));
        div()
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(nav)
            .child(div().flex_1())
            .when_some(search, |el, field| el.child(div().w(px(SEARCH_W_PX)).max_w_full().row().items_center().gap(px(SPACE_8_PX)).child(icon("search", ICON_15_PX, p.on_surface_variant)).child(div().flex_1().min_w(px(0.0)).child(field))))
            .into_any_element()
    }

    /// A row of filter chips; `chips` are (filter, label, count).
    fn chip_row(&mut self, chips: Vec<(Filter, String, Option<usize>)>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let current = self.store.filter;
        let row = chips.into_iter().enumerate().map(|(i, (filter, label, count))| {
            let active = filter == current;
            let fg = if active { p.on_primary } else { p.on_surface };
            div()
                .id(SharedString::from(format!("store-chip-{i}")))
                .h(px(CHIP_H_PX))
                .px(px(SPACE_14_PX))
                .rounded_full()
                .row()
                .items_center()
                .gap(px(SPACE_6_PX))
                .cursor_pointer()
                .text_size(px(TEXT_12_5_PX))
                .font_weight(FontWeight::MEDIUM)
                .bg(if active { p.primary.hsla() } else { p.surface_container.hsla() })
                .text_color(fg.hsla())
                .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                .when(!active, |el| el.hover(|s| s.bg(p.surface_container_high.hsla())))
                .child(label)
                .when_some(count, |el, n| el.child(div().text_color(fg.with_alpha(ALPHA_70).hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(n.to_string())))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.store_set_filter(filter, cx)))
        });
        div().row().flex_wrap().gap(px(SPACE_8_PX)).children(row).into_any_element()
    }

    // ── Get / On ─────────────────────────────────────────────────────────

    /// The one-click state button of a plugin: Get (off), On (on; a click
    /// turns it off), Add back (a removed built-in), Update (a newer
    /// release), or the "Always on" chip of a core plugin. `scope` keeps
    /// element ids unique when a plugin shows twice on a page.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn get_button(&mut self, scope: &str, m: &PluginManifest, enabled: bool, size: ButtonSize, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let builtin = m.origin == PluginOrigin::Builtin;
        if builtin && mandara_plugins::lifecycle::essential_reason(&m.id).is_some() {
            return plugin_modal::always_on_chip(p).into_any_element();
        }
        let id = m.id.clone();
        let el_id = format!("store-get-{scope}-{}", m.id);
        if builtin && cfg.plugins.is_removed(&m.id) {
            return pill(el_id, Some("undo"), t("Add back"), Tone::Primary, size, p)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.plugin_lifecycle(&id, Action::Restore, cx);
                }))
                .into_any_element();
        }
        if self.updates.plugin_jobs.contains_key(&m.id) {
            return pill(el_id, None, t("Updating…"), Tone::Tonal, size, p).cursor_default().into_any_element();
        }
        if self.updates.plugins.iter().any(|u| u.id == m.id && u.available) {
            return pill(el_id, Some("download"), t("Update"), Tone::Primary, size, p)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.plugin_update_start(id.clone(), false, cx);
                }))
                .into_any_element();
        }
        let (glyph, label, tone, action) = if enabled { ("check", t("On"), Tone::Tonal, Action::Disable) } else { ("plus", t("Get"), Tone::Primary, Action::Enable) };
        pill(el_id, Some(glyph), label, tone, size, p)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                this.plugin_lifecycle(&id, action, cx);
            }))
            .into_any_element()
    }

    /// A store card: cover art, icon tile with name and category, a
    /// two-line pitch, maker badge, state and the Get / On button. A click
    /// opens the product page.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn store_card(&mut self, scope: &str, enabled: bool, m: &PluginManifest, card_w: f32, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let scale = self.modal.scale;
        let cover_h = (card_w * STORE_COVER_ASPECT).floor();
        let cover = art_box(art_of(m), card_w, cover_h, scale, Detail::Sharp, plugin_placeholder(m, card_w, cover_h, p));
        let (badge_text, badge_color) = origin_badge(m, p);
        let removed = m.origin == PluginOrigin::Builtin && cfg.plugins.is_removed(&m.id);
        let state = if removed {
            Some(badge(t("Removed"), p.urgent_coral))
        } else if self.updates.plugins.iter().any(|u| u.id == m.id && u.available) {
            Some(badge(t("Update available"), p.warning_amber))
        } else {
            None
        };
        let button = self.get_button(scope, m, enabled, ButtonSize::Small, cfg, p, cx);
        let detail_id = m.id.clone();
        div()
            .id(SharedString::from(format!("store-card-{scope}-{}", m.id)))
            .relative()
            .flex_1()
            .h_full()
            .rounded(px(CARD_RADIUS_PX))
            .overflow_hidden()
            .bg(p.surface_container.hsla())
            .cursor_pointer()
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .row()
            .flex_col()
            .child(cover)
            .child(
                div()
                    .flex_1()
                    .p(px(CARD_PAD_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_8_PX))
                    .child(
                        div()
                            .row()
                            .items_center()
                            .gap(px(SPACE_10_PX))
                            .child(plugin_tile(m, CARD_TILE_PX, true, scale, p))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .row()
                                    .flex_col()
                                    .gap(px(SPACE_3_PX))
                                    .child(div().text_size(px(TEXT_14_5_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(m.display_name()))
                                    .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(tr(plugin_modal::category(m)))),
                            ),
                    )
                    .child(div().h(px(LEADING_17_PX * CARD_PITCH_LINES as f32)).text_size(px(TEXT_12_PX)).line_height(px(LEADING_17_PX)).text_color(p.on_surface_variant.hsla()).line_clamp(CARD_PITCH_LINES).child(m.display_description()))
                    .child(div().flex_1())
                    .child(
                        div()
                            .pt(px(SPACE_8_PX))
                            .row()
                            .items_center()
                            .gap(px(SPACE_6_PX))
                            .child(badge(badge_text, badge_color))
                            .children(state)
                            .child(div().flex_1())
                            .child(button),
                    ),
            )
            .child(rim_overlay(CARD_RADIUS_PX, p))
            .when(!enabled && !removed, |el| el.opacity(ALPHA_88))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&detail_id, ModalView::Detail, window, cx)))
            .into_any_element()
    }

    // ── Discover ─────────────────────────────────────────────────────────

    fn discover(&mut self, cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let all = listed(cfg, cx);
        self.market_prime(cx);
        let mut chips = vec![(Filter::All, t("All").to_string(), None)];
        for (i, shelf) in SHELVES.iter().enumerate() {
            let n = all.iter().filter(|(_, m)| shelf_of(m) == Some(i)).count();
            chips.push((Filter::Shelf(i), tr(shelf.title), Some(n)));
        }
        let third = all.iter().filter(|(_, m)| m.origin != PluginOrigin::Builtin).count();
        if third > 0 {
            chips.push((Filter::ThirdParty, t("Third-party").to_string(), Some(third)));
        }
        let chips = self.chip_row(chips, p, cx);
        let terms = terms(&self.store.query);
        if !terms.is_empty() || self.store.filter != Filter::All {
            self.store.hero.stop();
            let results = self.discover_results(&all, &terms, cfg, p, cx);
            return div().row().flex_col().gap(px(SPACE_20_PX)).child(chips).child(results).into_any_element();
        }

        let fit = grid::fit(self.page.column_w, STORE_CARD_GRID);
        let featured: Vec<(bool, PluginManifest)> = FEATURED.iter().filter_map(|id| all.iter().find(|(_, m)| m.id == *id).cloned()).collect();
        let mut page = div().row().flex_col().gap(px(SPACE_30_PX));
        if !featured.is_empty() {
            page = page.child(self.discover_hero(&featured, cfg, p, window, cx));
        }
        page = page.child(chips);
        page = page.children(self.new_and_updated(&all, fit, cfg, p, cx));
        for (i, shelf) in SHELVES.iter().enumerate() {
            let items: Vec<&(bool, PluginManifest)> = all.iter().filter(|(_, m)| shelf_of(m) == Some(i)).collect();
            if items.is_empty() {
                continue;
            }
            let total = items.len();
            let cards: Vec<AnyElement> = items.into_iter().take(fit.cols).map(|(on, m)| self.store_card(shelf.key, *on, m, fit.card_w, cfg, p, cx)).collect();
            let more = (total > fit.cols).then(|| see_all(format!("store-see-{}", shelf.key), Some(total), p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.store_set_filter(Filter::Shelf(i), cx))));
            page = page.child(div().row().flex_col().gap(px(SPACE_14_PX)).child(shelf_head(tr(shelf.title).into(), tr(shelf.line).into(), more, p)).child(fit.render(cards)));
        }
        page = page.children(self.community_picks(fit, p, cx));
        page = page.child(self.build_band(p, cx));
        page.into_any_element()
    }

    /// Discover with a query or a chip: one grid of matches, and a way on
    /// to the community marketplace.
    fn discover_results(&mut self, all: &[(bool, PluginManifest)], terms: &[String], cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.store.filter;
        let found: Vec<&(bool, PluginManifest)> = all
            .iter()
            .filter(|(_, m)| match filter {
                Filter::Shelf(i) => shelf_of(m) == Some(i),
                Filter::ThirdParty => m.origin != PluginOrigin::Builtin,
                _ => true,
            })
            .filter(|(_, m)| matches(m, terms))
            .collect();
        let query = self.store.query.trim().to_string();
        let (title, line): (String, String) = match filter {
            Filter::Shelf(i) => (tr(SHELVES[i].title), tr(SHELVES[i].line)),
            Filter::ThirdParty => (t("Third-party").to_string(), t("Plugins from the marketplace, your plugin folders or your distribution.").to_string()),
            _ => (tf("Results for “{query}”", &[("query", &query)]), tf("{count} plugins", &[("count", &found.len())])),
        };
        let fit = grid::fit(self.page.column_w, STORE_CARD_GRID);
        let body = if found.is_empty() {
            empty_state("search", t("No plugins match"), t("Try another word, or look in the community marketplace."), p)
        } else {
            let cards: Vec<AnyElement> = found.into_iter().map(|(on, m)| self.store_card("results", *on, m, fit.card_w, cfg, p, cx)).collect();
            fit.render(cards)
        };
        let community = (!query.is_empty()).then(|| {
            pill("store-search-community", Some("globe"), tf("Search the community for “{query}”", &[("query", &query)]), Tone::Glass, ButtonSize::Large, p)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.store_tab(PluginsTab::Marketplace, cx)))
        });
        div().row().flex_col().gap(px(SPACE_16_PX)).child(shelf_head(title.into(), line.into(), None, p)).child(body).children(community.map(|c| div().row().child(c))).into_any_element()
    }

    /// The featured hero: the plugin's cover across the whole width, its
    /// name and pitch over a soft scrim, Get / On and Details, page dots.
    /// Rotates while Discover is on screen (paused under the pointer).
    fn discover_hero(&mut self, featured: &[(bool, PluginManifest)], cfg: &ShellConfig, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let count = featured.len();
        let rtl = mandara_i18n::language().is_rtl();
        let scale = self.modal.scale;
        {
            let h = &mut self.store.hero;
            h.count = count;
            h.index %= count;
            h.fade.set_instant(cfg.motion.reduce_motion);
            let dt = h.clock.tick(Instant::now());
            if h.fade.step(dt) {
                window.request_animation_frame();
            } else {
                h.clock.idle();
                h.prev = None;
            }
        }
        self.hero_schedule(window, cx);
        let (index, prev, fade) = (self.store.hero.index, self.store.hero.prev.filter(|i| *i < count), self.store.hero.fade.value().clamp(0.0, 1.0));
        let w = self.page.column_w;
        let h = (w * HERO_ASPECT).clamp(HERO_MIN_H_PX, HERO_MAX_H_PX);
        // The whole cover, framed on the end side (covers carry their own
        // lettering, so they are never cropped), over a blurred copy of it.
        let frame_h = h - HERO_PAD_PX * 2.0;
        let frame_w = (frame_h / STORE_COVER_ASPECT).floor();
        let framed = w - frame_w - HERO_PAD_PX >= HERO_TEXT_MIN_W_PX;
        let text_w = if framed { w - frame_w - HERO_PAD_PX } else { w };
        let backdrop = |m: &PluginManifest| art_box(art_of(m), w, h, scale, Detail::Ambient, plugin_placeholder(m, w, h, p));
        let cover = |m: &PluginManifest| art_box(art_of(m), frame_w, frame_h, scale, Detail::Sharp, plugin_placeholder(m, frame_w, frame_h, p));
        let (enabled, m) = featured[index].clone();
        let layer = || div().absolute().top_0().left_0().size_full();
        let mut art = layer();
        let mut frame = div().relative().size_full();
        if let Some(prev) = prev {
            art = art.child(layer().child(backdrop(&featured[prev].1)));
            frame = frame.child(layer().child(cover(&featured[prev].1)));
        }
        let top_alpha = if prev.is_some() { fade } else { 1.0 };
        art = art.child(layer().opacity(top_alpha).child(backdrop(&m)));
        frame = frame.child(layer().opacity(top_alpha).child(cover(&m)));
        let frame = framed.then(|| {
            div()
                .absolute()
                .top(px(HERO_PAD_PX))
                .inset_end(px(HERO_PAD_PX))
                .w(px(frame_w))
                .h(px(frame_h))
                .rounded(px(HERO_FRAME_RADIUS_PX))
                .overflow_hidden()
                .shadow(vec![gpui::BoxShadow { color: gpui::black().opacity(ALPHA_35), offset: gpui::point(px(0.0), px(HERO_FRAME_SHADOW_Y_PX)), blur_radius: px(HERO_FRAME_SHADOW_BLUR_PX), spread_radius: px(0.0), inset: false }])
                .child(frame)
                .child(rim_overlay(HERO_FRAME_RADIUS_PX, p))
        });
        let scrim = layer().bg(mandara_core::oklab_gradient(
            if rtl { GRADIENT_END_TO_START_DEG } else { GRADIENT_START_TO_END_DEG },
            p.surface_dim.with_alpha(HERO_SCRIM_ALPHA),
            p.surface_dim.with_alpha(HERO_SCRIM_END_ALPHA),
        ));
        let get = self.get_button("hero", &m, enabled, ButtonSize::Large, cfg, p, cx);
        let detail_id = m.id.clone();
        let details = pill("store-hero-details", None, t("Details"), Tone::Glass, ButtonSize::Large, p).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&detail_id, ModalView::Detail, window, cx)));
        let text = div()
            .absolute()
            .top_0()
            .bottom_0()
            .inset_start(px(0.0))
            .w(px(text_w))
            .p(px(HERO_PAD_PX))
            .pb(px(HERO_PAD_PX + DIM_32_PX))
            .row()
            .flex_col()
            .justify_center()
            .gap(px(SPACE_12_PX))
            .opacity(if prev.is_some() { fade } else { 1.0 })
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(badge(t("Featured"), p.primary))
                    .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(tr(plugin_modal::category(&m)))),
            )
            .child(div().font_family(mandara_ui::fonts::fonts(cx).display.clone()).text_size(px(HERO_TITLE_PX)).font_weight(FontWeight::SEMIBOLD).line_height(px(HERO_TITLE_PX * HERO_TITLE_LEADING)).child(m.display_name()))
            .child(div().text_size(px(TEXT_14_PX)).line_height(px(LEADING_21_PX)).text_color(p.on_surface.with_alpha(ALPHA_86).hsla()).line_clamp(HERO_PITCH_LINES).child(mandara_ui::bidi::paragraph(m.display_description())))
            .child(div().pt(px(SPACE_6_PX)).row().items_center().gap(px(SPACE_10_PX)).child(get).child(details));
        // Page dots and arrows under the text.
        let dots = (0..count).map(|i| {
            let active = i == index;
            div()
                .id(SharedString::from(format!("store-hero-dot-{i}")))
                .w(px(if active { DOT_ACTIVE_W_PX } else { DOT_PX }))
                .h(px(DOT_PX))
                .rounded_full()
                .cursor_pointer()
                .bg(if active { p.on_surface.hsla() } else { p.on_surface.with_alpha(ALPHA_40).hsla() })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.hero_go(i, cx)))
        });
        let arrow = |id: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .size(px(DIM_32_PX))
                .rounded_full()
                .row()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .bg(p.surface_base.with_alpha(ALPHA_72).hsla())
                .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                .hover(|s| s.bg(p.surface_container_high.hsla()))
                .child(icon(glyph, ICON_14_PX, p.on_surface))
        };
        let controls = div()
            .absolute()
            .bottom(px(SPACE_18_PX))
            .inset_start(px(HERO_PAD_PX - SPACE_10_PX))
            .px(px(SPACE_10_PX))
            .py(px(SPACE_6_PX))
            .rounded_full()
            .bg(p.surface_dim.with_alpha(ALPHA_55).hsla())
            .row()
            .items_center()
            .gap(px(SPACE_10_PX))
            .child(arrow("store-hero-prev", mandara_ui::bidi::dir_icon("chevron-left")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.hero_go((index + count - 1) % count, cx))))
            .child(div().row().items_center().gap(px(SPACE_6_PX)).children(dots))
            .child(arrow("store-hero-next", mandara_ui::bidi::dir_icon("chevron-right")).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.hero_go((index + 1) % count, cx))));
        div()
            .id("store-hero")
            .relative()
            .w_full()
            .h(px(h))
            .rounded(px(HERO_RADIUS_PX))
            .overflow_hidden()
            .bg(p.surface_container.hsla())
            .on_hover(cx.listener(|this, hovered: &bool, _, _| this.store.hero.hovered = *hovered))
            .child(art)
            .child(scrim)
            .child(mandara_core::grain_overlay())
            .children(frame)
            .child(text)
            .when(count > 1, |el| el.child(controls))
            .child(rim_overlay(HERO_RADIUS_PX, p))
            .into_any_element()
    }

    /// Shows featured plugin `index`, crossfading from the current one.
    fn hero_go(&mut self, index: usize, cx: &mut Context<Self>) {
        let h = &mut self.store.hero;
        h.shown_at = Instant::now();
        if index == h.index {
            return;
        }
        h.prev = Some(h.index);
        h.index = index;
        h.fade.snap_to(0.0);
        h.fade.set_target(1.0);
        cx.notify();
    }

    /// Whether the hero is on screen right now.
    fn hero_on_screen(&self, window: &Window) -> bool {
        self.section == Section::Plugins
            && self.plugins_tab == PluginsTab::Discover
            && self.query.is_empty()
            && self.store.query.trim().is_empty()
            && self.store.filter == Filter::All
            && !self.modal_open()
            && window.is_visible()
    }

    /// Starts the rotation timer unless it runs: it waits out each slide's
    /// time and ends itself as soon as the hero is no longer on screen
    /// (another page or tab, a search, the product page, a hidden window).
    fn hero_schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.hero.running || self.store.hero.count < 2 || !self.hero_on_screen(window) {
            return;
        }
        self.store.hero.running = true;
        self.store.hero.task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(wait) = this.update(cx, |this, _| HERO_INTERVAL.saturating_sub(this.store.hero.shown_at.elapsed())) else { break };
                cx.background_executor().timer(wait.max(HERO_MIN_WAIT)).await;
                let keep = this.update_in(cx, |this, window, cx| {
                    if !this.hero_on_screen(window) {
                        this.store.hero.running = false;
                        return false;
                    }
                    let h = &this.store.hero;
                    if !h.hovered && h.shown_at.elapsed() >= HERO_INTERVAL {
                        let next = (h.index + 1) % h.count.max(1);
                        this.hero_go(next, cx);
                    }
                    true
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
    }

    /// "New & updated": plugins with a newer release, the third-party
    /// plugins on this system, and the community's most recently updated.
    fn new_and_updated(&mut self, all: &[(bool, PluginManifest)], fit: grid::GridFit, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending: Vec<String> = self.updates.plugins.iter().filter(|u| u.available).map(|u| u.id.clone()).collect();
        let mut local: Vec<&(bool, PluginManifest)> = all.iter().filter(|(_, m)| m.origin != PluginOrigin::Builtin).collect();
        local.sort_by_key(|(_, m)| !pending.contains(&m.id));
        let mut cards: Vec<AnyElement> = local.into_iter().take(fit.cols).map(|(on, m)| self.store_card("new", *on, m, fit.card_w, cfg, p, cx)).collect();
        if cards.len() < fit.cols {
            let room = fit.cols - cards.len();
            for listing in self.community_recent(room) {
                cards.push(self.listing_card("new", &listing, fit.card_w, p, cx));
            }
        }
        if cards.is_empty() {
            return None;
        }
        Some(div().row().flex_col().gap(px(SPACE_14_PX)).child(shelf_head(t("New & updated").into(), t("Fresh releases and the plugins you added yourself").into(), None, p)).child(fit.render(cards)).into_any_element())
    }

    /// The marketplace's most-starred plugins, with "See all" to Community.
    fn community_picks(&mut self, fit: grid::GridFit, p: &MandaraPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let skip = self.community_recent(fit.cols).into_iter().map(|l| l.full_name).collect::<Vec<_>>();
        let picks = self.community_top(fit.cols, &skip);
        if picks.is_empty() {
            return None;
        }
        let cards: Vec<AnyElement> = picks.iter().map(|l| self.listing_card("picks", l, fit.card_w, p, cx)).collect();
        let more = see_all("store-see-community", None, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.store_tab(PluginsTab::Marketplace, cx)));
        Some(div().row().flex_col().gap(px(SPACE_14_PX)).child(shelf_head(t("Community picks").into(), t("Loved by Mandara users: third-party plugins that run sandboxed").into(), Some(more), p)).child(fit.render(cards)).into_any_element())
    }

    /// "Made by Mandara": one row of bundled plugins, those still off
    /// first (Community shows it while the marketplace is empty or out of
    /// reach, so the tab is never a dead end).
    pub(super) fn mandara_suggestions(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let cfg = mandara_ui::state::config(cx).read(cx).snapshot();
        let mut bundled: Vec<(bool, PluginManifest)> = listed(&cfg, cx).into_iter().filter(|(_, m)| m.origin == PluginOrigin::Builtin && !cfg.plugins.is_removed(&m.id)).collect();
        bundled.sort_by_key(|(on, m)| (*on, art_of(m).is_none()));
        let fit = grid::fit(self.page.column_w, STORE_CARD_GRID);
        let cards: Vec<AnyElement> = bundled.iter().take(fit.cols).map(|(on, m)| self.store_card("suggest", *on, m, fit.card_w, &cfg, p, cx)).collect();
        let more = see_all("store-see-discover", None, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.store_tab(PluginsTab::Discover, cx)));
        div().row().flex_col().gap(px(SPACE_14_PX)).child(shelf_head(t("Made by Mandara").into(), t("Included with Mandara, no download needed").into(), Some(more), p)).child(fit.render(cards)).into_any_element()
    }

    /// The closing band: make your own plugin.
    fn build_band(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let folder_btn = |id: &'static str, label: &'static str, dir: PathBuf| {
            pill(id, Some("folder"), label, Tone::Glass, ButtonSize::Small, p).on_click(move |_, _, _| {
                let _ = std::fs::create_dir_all(&dir);
                let _ = mandara_services::session::open_uri(&format!("file://{}", dir.display()));
            })
        };
        div()
            .relative()
            .rounded(px(CARD_RADIUS_PX))
            .overflow_hidden()
            .p(px(SPACE_24_PX))
            .bg(mandara_core::oklab_gradient(GRADIENT_DIAGONAL_DEG, p.primary.with_alpha(ALPHA_20), p.tertiary.with_alpha(ALPHA_12)))
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_18_PX))
            .child(div().flex_none().size(px(DIM_56_PX)).rounded(px(RADIUS_18_PX)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_20).hsla()).child(icon("terminal", ICON_24_PX, p.primary)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_240_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_17_PX)).font_weight(FontWeight::SEMIBOLD).child(t("Make it yours")))
                    .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Plugins are Rust compiled to WebAssembly. Drop one into a plugin folder and it shows up here within a second; publish it with one GitHub tag.")))),
            )
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(folder_btn("store-open-plugins", t("Installed folder"), mandara_config::paths::data_dir().join("plugins")))
                    .child(folder_btn("store-open-plugins-dev", t("Development folder"), mandara_config::paths::data_dir().join("plugins-dev")))
                    .child(pill("store-build", Some("terminal"), t("Build your own"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_plugins_tab(PluginsTab::Local, Some(LocalSection::QuickStart), cx)))),
            )
            .child(rim_overlay(CARD_RADIUS_PX, p))
            .into_any_element()
    }

    // ── Installed (library) ──────────────────────────────────────────────

    fn library(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let manifests = listed(cfg, cx);
        // Removed built-ins leave the grid for their own collapsed section.
        let (removed, kept): (Vec<_>, Vec<_>) = manifests.into_iter().partition(|(_, m)| m.origin == PluginOrigin::Builtin && cfg.plugins.is_removed(&m.id));
        let removed: Vec<PluginManifest> = removed.into_iter().map(|(_, m)| m).collect();
        let total = kept.len();
        let on = kept.iter().filter(|(e, _)| *e).count();
        let third_count = kept.iter().filter(|(_, m)| m.origin != PluginOrigin::Builtin).count();
        let updates = self.updates.plugins.iter().filter(|u| u.available).count();

        // Summary: on / off / updates, as tiles.
        let stat = |id: &'static str, value: usize, label: &'static str, tone: Color, filter: Option<Filter>| {
            div()
                .id(id)
                .flex_1()
                .min_w(px(STAT_TILE_MIN_W_PX))
                .p(px(SPACE_16_PX))
                .rounded(px(RADIUS_18_PX))
                .bg(p.surface_container.hsla())
                .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                .row()
                .flex_col()
                .gap(px(SPACE_4_PX))
                .cursor_pointer()
                .hover(|s| s.bg(p.surface_container_high.hsla()))
                .child(div().text_size(px(STAT_VALUE_PX)).font_weight(FontWeight::SEMIBOLD).text_color(tone.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(value.to_string()))
                .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(label))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match filter {
                    Some(f) => this.store_set_filter(f, cx),
                    None => this.store_tab(PluginsTab::Updates, cx),
                }))
        };
        let summary = div()
            .row()
            .flex_wrap()
            .gap(px(SPACE_12_PX))
            .child(stat("store-stat-on", on, t("On"), p.primary, Some(Filter::On)))
            .child(stat("store-stat-off", total - on, t("Off"), p.on_surface, Some(Filter::Off)))
            .child(stat("store-stat-third", third_count, t("Third-party"), p.secondary, Some(Filter::ThirdParty)))
            .child(stat("store-stat-updates", updates, t("Updates"), if updates > 0 { p.warning_amber } else { p.on_surface }, None));

        let chips = self.chip_row(
            vec![
                (Filter::All, t("All").to_string(), Some(total)),
                (Filter::On, t("On").to_string(), Some(on)),
                (Filter::Off, t("Off").to_string(), Some(total - on)),
                (Filter::Mandara, t("Mandara").to_string(), Some(total - third_count)),
                (Filter::ThirdParty, t("Third-party").to_string(), Some(third_count)),
            ],
            p,
            cx,
        );
        let filter = self.store.filter;
        let terms = terms(&self.store.query);
        let shown: Vec<(bool, PluginManifest)> = kept
            .into_iter()
            .filter(|(on, m)| match filter {
                Filter::On => *on,
                Filter::Off => !*on,
                Filter::Mandara => m.origin == PluginOrigin::Builtin,
                Filter::ThirdParty => m.origin != PluginOrigin::Builtin,
                _ => true,
            })
            .filter(|(_, m)| matches(m, &terms))
            .collect();

        // Group into categories, in a fixed order.
        let mut groups: Vec<(&'static str, Vec<(bool, PluginManifest)>)> = PLUGIN_CATEGORIES.iter().map(|(title, _)| (*title, Vec::new())).collect();
        for (enabled, m) in shown {
            let title = plugin_modal::category(&m);
            match groups.iter_mut().find(|(t, _)| *t == title) {
                Some((_, list)) => list.push((enabled, m)),
                None => groups.push((title, vec![(enabled, m)])),
            }
        }
        let fit = grid::fit(self.page.column_w, PLUGIN_GRID);
        let third_title = tk("Third-party");
        let (third, first): (Vec<_>, Vec<_>) = groups.into_iter().filter(|(_, l)| !l.is_empty()).partition(|(title, _)| *title == third_title);
        let first_count: usize = first.iter().map(|(_, l)| l.len()).sum();
        let third: Vec<_> = third.into_iter().flat_map(|(_, l)| l).collect();
        let narrowed = filter != Filter::All || !terms.is_empty();
        let mut page = div().row().flex_col().gap(px(SPACE_26_PX)).child(summary).children(self.plugin_updates_banner(p, cx)).child(chips);

        if first_count == 0 && third.is_empty() && narrowed {
            return page.child(empty_state("search", t("No plugins match"), t("Try another word or another filter."), p)).into_any_element();
        }
        // First party: bundled with Mandara, in categories.
        if first_count > 0 {
            page = page.child(section_head(
                "grid",
                t("Mandara plugins"),
                first_count,
                t("Made by Mandara and built into the shell, so they update with it. Turn on what you want; turn off or remove what you don't (core plugins stay on)."),
                p,
            ));
            for (title, list) in first {
                let cards: Vec<AnyElement> = list.into_iter().map(|(enabled, m)| self.plugin_card(enabled, m, p, cx)).collect();
                page = page.child(
                    div()
                        .row()
                        .flex_col()
                        .gap(px(SPACE_10_PX))
                        .child(div().px(px(SPACE_6_PX)).text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(mandara_ui::bidi::paragraph(tr(title))))
                        .child(fit.render(cards)),
                );
            }
        }
        if !removed.is_empty() && !narrowed {
            page = page.child(self.removed_plugins(removed, p, cx));
        }
        // Third party: the marketplace, your plugin folders, distribution packages.
        if !narrowed || !third.is_empty() {
            page = page.child(section_head(
                "package",
                t("Third-party plugins"),
                third.len(),
                t("Sandboxed WebAssembly plugins from the marketplace, your plugin folders or your distribution."),
                p,
            ));
            if third.is_empty() {
                page = page.child(self.third_party_empty(p, cx));
            } else {
                let cards: Vec<AnyElement> = third.into_iter().map(|(enabled, m)| self.plugin_card(enabled, m, p, cx)).collect();
                page = page.child(fit.render(cards));
            }
        }
        page.into_any_element()
    }

    /// One plugin card in the library: tile, name, description, on / off
    /// switch, origin badge (Mandara or third-party), version and settings.
    fn plugin_card(&mut self, enabled: bool, m: PluginManifest, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = m.id.clone();
        let has_settings = plugin_modal::has_settings(&m);
        let (badge_text, badge_color) = origin_badge(&m, p);
        let toggle_id = id.clone();
        let essential = m.origin == PluginOrigin::Builtin && mandara_plugins::lifecycle::essential_reason(&id).is_some();
        let update = self.updates.plugins.iter().any(|u| u.id == id && u.available);
        // A release installed despite failing the compliance check.
        self.market_local_prime(cx);
        let flag = self.installed_flag(&id);
        let settings_id = id.clone();
        let detail_id = id.clone();
        let scale = self.modal.scale;
        div()
            .id(SharedString::from(format!("plugin-card-{id}")))
            .relative()
            .flex_1()
            .cursor_pointer()
            .rounded(px(RADIUS_18_PX))
            .bg(p.surface_container.hsla())
            .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
            .hover(|s| s.bg(p.surface_container_high.hsla()))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&detail_id, ModalView::Detail, window, cx)))
            .p(px(SPACE_16_PX))
            .row()
            .flex_col()
            .justify_between()
            .gap(px(SPACE_12_PX))
            .when(!enabled, |el| el.opacity(ALPHA_78))
            .child(
                div()
                    .row()
                    .items_start()
                    .gap(px(SPACE_12_PX))
                    .child(plugin_tile(&m, PLUGIN_TILE_PX, enabled, scale, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(m.display_name()))
                            .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_16_PX)).text_color(if flag.is_some() { p.urgent_coral.hsla() } else { p.on_surface_variant.hsla() }).line_clamp(CARD_PITCH_LINES).child(flag.as_ref().map_or_else(|| m.display_description(), |f| f.short_warning()))),
                    )
                    .child(if essential {
                        // Core plugins have no switch.
                        plugin_modal::always_on_chip(p).into_any_element()
                    } else {
                        div()
                            .id(SharedString::from(format!("plugin-{id}")))
                            .flex_none()
                            .cursor_pointer()
                            .child(mandara_ui::plugins::network::switch(enabled, p))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                let action = if enabled { Action::Disable } else { Action::Enable };
                                this.plugin_lifecycle(&toggle_id, action, cx);
                            }))
                            .into_any_element()
                    }),
            )
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(badge(badge_text, badge_color))
                    .when(update, |el| el.child(badge(t("Update available"), p.warning_amber)))
                    .when_some(flag.as_ref(), |el, f| el.child(badge(f.class.label(), p.urgent_coral)))
                    .when(!m.version.is_empty(), |el| el.child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(format!("v{}", m.version))))
                    .child(div().flex_1())
                    .when(has_settings, |el| {
                        el.child(
                            pill(format!("plugin-settings-{id}"), Some("sliders"), t("Settings"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.open_plugin(&settings_id, ModalView::Settings, window, cx);
                            })),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "Removed Mandara plugins": a collapsed list of the built-ins the user
    /// removed, each with a Restore button.
    fn removed_plugins(&mut self, removed: Vec<PluginManifest>, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let open = self.modal.removed_open;
        let scale = self.modal.scale;
        let head = div()
            .id("plugins-removed-toggle")
            .row()
            .items_center()
            .gap(px(SPACE_8_PX))
            .px(px(SPACE_6_PX))
            .cursor_pointer()
            .hover(|s| s.opacity(ALPHA_85))
            .child(icon(if open { "arrow-down" } else { mandara_ui::bidi::dir_icon("chevron-right") }, ICON_14_PX, p.on_surface_variant))
            .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(tf("Removed Mandara plugins ({n})", &[("n", &removed.len())])))
            .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(t("Off and out of your bar and sidebar. Restore one to bring it back where it was.")))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.modal.removed_open = !this.modal.removed_open;
                cx.notify();
            }));
        let mut section = div().row().flex_col().gap(px(SPACE_10_PX)).child(head);
        if open {
            let rows: Vec<AnyElement> = removed
                .iter()
                .map(|m| {
                    let (restore_id, detail_id) = (m.id.clone(), m.id.clone());
                    div()
                        .id(SharedString::from(format!("plugin-removed-{}", m.id)))
                        .flex_1()
                        .min_w(px(0.0))
                        .rounded(px(RADIUS_14_PX))
                        .bg(p.surface_container.hsla())
                        .px(px(SPACE_12_PX))
                        .py(px(SPACE_10_PX))
                        .row()
                        .items_center()
                        .gap(px(SPACE_10_PX))
                        .cursor_pointer()
                        .hover(|s| s.bg(p.surface_container_high.hsla()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(&detail_id, ModalView::Detail, window, cx)))
                        .child(plugin_tile(m, REMOVED_TILE_PX, false, scale, p))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .row()
                                .flex_col()
                                .gap(px(SPACE_3_PX))
                                .child(div().text_size(px(TEXT_12_5_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(m.display_name()))
                                .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(m.display_description())),
                        )
                        .child(pill(format!("plugin-restore-{}", m.id), Some("undo"), t("Restore"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.plugin_lifecycle(&restore_id, Action::Restore, cx);
                        })))
                        .into_any_element()
                })
                .collect();
            section = section.child(grid::fit(self.page.column_w, REMOVED_GRID).render(rows));
        }
        section.into_any_element()
    }

    /// The third-party section before any is installed: where they come from.
    fn third_party_empty(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .p(px(SPACE_16_PX))
            .rounded(px(RADIUS_18_PX))
            .border_1()
            .border_color(p.outline.with_alpha(ALPHA_50).hsla())
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_200_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_3_PX))
                    .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).child(t("No third-party plugins yet")))
                    .child(div().text_size(px(TEXT_12_PX)).line_height(px(LEADING_16_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Install one from the marketplace with a click, or drop a plugin folder into your plugin folders. It shows up here within a second.")))),
            )
            .child(pill("third-party-market", Some("globe"), t("Browse the marketplace"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.store_tab(PluginsTab::Marketplace, cx))))
            .child(pill("third-party-build", Some("terminal"), t("Build your own"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show_plugins_tab(PluginsTab::Local, None, cx))))
            .into_any_element()
    }

    // ── Updates ──────────────────────────────────────────────────────────

    /// Marketplace plugin updates: check, update one or all, and the
    /// result of the last run.
    fn updates_view(&mut self, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let pending: Vec<_> = self.updates.plugins.iter().filter(|u| u.available).cloned().collect();
        let checking = self.updates.checking;
        let busy = !self.updates.plugin_jobs.is_empty();
        let title = match pending.len() {
            0 => t("Everything is up to date").to_string(),
            1 => t("1 plugin update").to_string(),
            n => tf("{count} plugin updates", &[("count", &n)]),
        };
        let header = div()
            .relative()
            .p(px(SPACE_20_PX))
            .rounded(px(CARD_RADIUS_PX))
            .bg(mandara_core::oklab_gradient(GRADIENT_DIAGONAL_DEG, p.primary.with_alpha(ALPHA_16), p.tertiary.with_alpha(ALPHA_10)))
            .row()
            .flex_wrap()
            .items_center()
            .gap(px(SPACE_16_PX))
            .child(div().flex_none().size(px(DIM_52_PX)).rounded(px(RADIUS_16_PX)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_18).hsla()).child(icon(if pending.is_empty() { "check" } else { "download" }, ICON_22_PX, p.primary)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(DIM_240_PX))
                    .row()
                    .flex_col()
                    .gap(px(SPACE_4_PX))
                    .child(div().text_size(px(TEXT_18_PX)).font_weight(FontWeight::SEMIBOLD).font_features(mandara_ui::fonts::tabular_nums()).child(title))
                    .child(div().text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(t("Marketplace plugins update with a small binary patch when there is one; their settings and on / off state are kept. Mandara's own plugins update with Mandara.")))),
            )
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_8_PX))
                    .child(pill("store-updates-check", Some("refresh"), if checking { t("Checking…") } else { t("Check now") }, Tone::Glass, ButtonSize::Small, p).when(!checking, |el| el.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.updates_check(true, cx)))))
                    .when(pending.len() > 1 && !busy, |el| el.child(pill("store-updates-all", Some("download"), t("Update all"), Tone::Primary, ButtonSize::Small, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.plugin_update_all(cx))))),
            )
            .child(rim_overlay(CARD_RADIUS_PX, p));
        let message = self.updates.plugin_message.clone().map(|(ok, text)| super::form::note(&text, !ok, p));
        let manifests: Vec<PluginManifest> = cx.global::<PluginRegistry>().manifests().cloned().collect();
        let scale = self.modal.scale;
        let mut list = div().row().flex_col().gap(px(SPACE_8_PX));
        for u in &pending {
            let m = manifests.iter().find(|m| m.id == u.id);
            let action = self.plugin_update_action(&u.id, p, cx);
            let name = m.map(PluginManifest::display_name).unwrap_or_else(|| u.id.clone());
            // A new release that failed the compliance check: why, on its own line.
            let warning = self.update_flag(u, cx).map(|f| div().w_full().child(super::market_page::flag_note(&f, p)));
            let notes = (!u.notes_url.is_empty()).then(|| {
                let url = u.notes_url.clone();
                pill(format!("store-update-notes-{}", u.id), Some("external"), t("Release notes"), Tone::Glass, ButtonSize::Small, p).on_click(move |_, _, _| {
                    let _ = mandara_services::session::open_uri(&url);
                })
            });
            list = list.child(
                div()
                    .p(px(SPACE_14_PX))
                    .rounded(px(RADIUS_18_PX))
                    .bg(p.surface_container.hsla())
                    .shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow))
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(match m {
                        Some(m) => plugin_tile(m, PLUGIN_TILE_PX, true, scale, p),
                        None => div().size(px(PLUGIN_TILE_PX)).rounded(px(PLUGIN_TILE_PX * PLUGIN_TILE_RADIUS)).row().items_center().justify_center().bg(p.primary.with_alpha(ALPHA_14).hsla()).child(icon("package", ICON_17_PX, p.primary)).into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(DIM_180_PX))
                            .row()
                            .flex_col()
                            .gap(px(SPACE_3_PX))
                            .child(div().text_size(px(TEXT_14_PX)).font_weight(FontWeight::SEMIBOLD).truncate().child(name))
                            .child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).font_features(mandara_ui::fonts::tabular_nums()).child(mandara_ui::bidi::ltr_isolate(&format!("{} → {}", u.installed, u.latest)))),
                    )
                    .children(notes)
                    .children(action)
                    .children(warning),
            );
        }
        let body = if pending.is_empty() {
            div()
                .p(px(SPACE_24_PX))
                .rounded(px(CARD_RADIUS_PX))
                .bg(p.surface_container.hsla())
                .row()
                .flex_col()
                .items_center()
                .gap(px(SPACE_8_PX))
                .child(icon("check", ICON_26_PX, p.success_emerald))
                .child(div().font_weight(FontWeight::MEDIUM).child(t("Your marketplace plugins are current")))
                .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("Mandara itself updates from About.")))
                .child(pill("store-updates-about", Some("arrow-up"), t("Open About"), Tone::Glass, ButtonSize::Small, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.show(Section::About, cx))))
                .into_any_element()
        } else {
            list.into_any_element()
        };
        div().row().flex_col().gap(px(SPACE_16_PX)).child(header).children(message).child(body).into_any_element()
    }
}

/// A centered empty state: glyph, title and a hint.
pub(super) fn empty_state(glyph: &'static str, title: &'static str, hint: &'static str, p: &MandaraPalette) -> AnyElement {
    div()
        .rounded(px(CARD_RADIUS_PX))
        .bg(p.surface_container.hsla())
        .p(px(SPACE_24_PX))
        .row()
        .flex_col()
        .items_center()
        .gap(px(SPACE_8_PX))
        .child(icon(glyph, ICON_26_PX, p.on_surface_variant.with_alpha(ALPHA_60)))
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .child(div().text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(hint))
        .into_any_element()
}

/// Line height of the hero title, per unit of its size.
const HERO_TITLE_LEADING: f32 = 1.15;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_lands_on_a_shelf() {
        for (title, ids) in PLUGIN_CATEGORIES.iter().filter(|(t, _)| *t != tk("Third-party")) {
            assert!(SHELVES.iter().any(|s| s.categories.contains(title)), "{title}");
            for id in *ids {
                let m = PluginManifest::builtin(id, id, "", None);
                assert!(shelf_of(&m).is_some(), "{id}");
            }
        }
    }

    #[test]
    fn featured_plugins_have_cover_art() {
        for id in FEATURED {
            assert!(mandara_ui::plugin_art::has_art(id), "{id}");
            assert!(PLUGIN_CATEGORIES.iter().any(|(_, ids)| ids.contains(id)), "{id}");
        }
    }

    #[test]
    fn search_matches_every_term() {
        let m = PluginManifest::builtin("weather", "Weather", "Forecasts in the bar", None);
        assert!(matches(&m, &terms("weath BAR")));
        assert!(matches(&m, &terms("")));
        assert!(!matches(&m, &terms("weather music")));
    }

    #[test]
    fn cover_keys_snap_to_buckets() {
        let a = cover_key(Art::Builtin("media".into()), 230.0, 129.0, 1.5, Detail::Sharp);
        let b = cover_key(Art::Builtin("media".into()), 236.0, 133.0, 1.5, Detail::Sharp);
        assert_eq!((a.w, a.w % COVER_BUCKET_PX as u32), (384, 0));
        assert_eq!(a, b, "a few pixels of resize reuse the decode");
        let ambient = cover_key(Art::Builtin("media".into()), 1000.0, 360.0, 2.0, Detail::Ambient);
        assert_eq!(ambient.w, AMBIENT_PX as u32);
    }
}
